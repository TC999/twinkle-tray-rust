//! Monitor worker 子进程生命周期管理。
//!
//! equiv: electron.js:L274-L545。
//! §3.2 高危区：worker 崩溃不能带倒主进程；自动重启 + 1s 重启节流 + 30s 失败复位；
//! 崩溃对话框只显示一次直到 worker 稳定运行 30 秒。

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// 就绪等待的 30 秒超时（equiv waitForMonitorThreadReady setTimeout 30000）。
const READY_TIMEOUT_MS: u64 = 30_000;
/// 重启节流（equiv recoverMonitorThread 的 await Utils.wait(1000)）。
const RESTART_PACE_MS: u64 = 1000;
/// 失败标志复位（equiv monitorsThreadFailedResetTimer setTimeout 30000）。
const FAILED_RESET_MS: u64 = 30_000;

/// 子进程句柄 + stdin 写端。
pub struct WorkerProcess {
    pub child: Child,
    pub stdin: ChildStdin,
}

pub struct MonitorThreadManager {
    process: Mutex<Option<WorkerProcess>>,
    /// 供 stdout 读取线程共享的就绪标志（等价 monitorsThreadReady 的跨线程可写性）。
    ready: Arc<AtomicBool>,
    starting: AtomicBool,
    failed: AtomicBool,
    failed_reset_at: Mutex<Option<Instant>>,
    quitting: AtomicBool,
    exe_path: Mutex<String>,
    datapath: Mutex<String>,
    recovering: AtomicBool,
    /// 事件回调表（主进程注入；等价 monitorThread.on("message", ...)）。
    /// 原版是 EventEmitter，允许多个监听者（host 缓存 + 面板 UI 各一份），
    /// 且监听器可以晚于 start() 注册 —— 因此这里存成列表，读取线程每收到一条
    /// 消息都取当前列表广播，而不是在 start() 时快照单个回调。
    event_callbacks: Arc<Mutex<Vec<Arc<dyn Fn(&Value) + Send + Sync>>>>,
    /// 失败提示回调（主进程注入；等价 dialog.showMessageBox，默认仅打印）。
    failure_notice: Mutex<Option<Arc<dyn Fn(&str) + Send + Sync>>>,
}

impl MonitorThreadManager {
    pub fn new(exe_path: String, datapath: String) -> Self {
        Self {
            process: Mutex::new(None),
            ready: Arc::new(AtomicBool::new(false)),
            starting: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            failed_reset_at: Mutex::new(None),
            quitting: AtomicBool::new(false),
            exe_path: Mutex::new(exe_path),
            datapath: Mutex::new(datapath),
            recovering: AtomicBool::new(false),
            event_callbacks: Arc::new(Mutex::new(Vec::new())),
            failure_notice: Mutex::new(None),
        }
    }

    /// 追加一个 worker 事件监听器（等价 monitorThread.on("message", ...)）。
    /// 可在 start() 之后调用；读取线程每收到消息都取当前列表广播。
    pub fn add_event_callback(&self, cb: Arc<dyn Fn(&Value) + Send + Sync>) {
        if let Ok(mut list) = self.event_callbacks.lock() {
            list.push(cb);
        }
    }

    /// 注册失败提示回调（等价 dialog.showMessageBox 的替身；未注册时仅打印）。
    pub fn set_failure_notice(&self, cb: Arc<dyn Fn(&str) + Send + Sync>) {
        *self.failure_notice.lock().unwrap() = Some(cb);
    }

    /// 等价 before-quit 钩子。
    pub fn set_quitting(&self, quitting: bool) {
        self.quitting.store(quitting, Ordering::SeqCst);
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    pub fn is_connected(&self) -> bool {
        self.process.lock().unwrap().is_some()
    }

    /// startMonitorThread 等价（equiv L345-L407）。
    pub fn start(&self, _allow_while_windows_idle: bool) -> bool {
        // 等价前置检查（equiv L346）
        if self.is_connected() || self.starting.load(Ordering::SeqCst) {
            return false;
        }
        self.ready.store(false, Ordering::SeqCst);
        self.starting.store(true, Ordering::SeqCst);
        println!("Starting monitor thread");

        // 等价 fork(..., { silent: true })：stdin/stdout 接管为 IPC 行协议
        let exe = self.exe_path.lock().unwrap().clone();
        let datapath = self.datapath.lock().unwrap().clone();
        let child = Command::new(&exe)
            .args([
                "--monitor-worker",
                "--isdev=false",
                format!("--apppath={}", exe).as_str(),
                "--skiptest=true",
                format!("--datapath={}", datapath).as_str(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn();

        match child {
            Ok(mut c) => {
                let stdin = c.stdin.take().expect("stdin 已 piped");
                // stdout 读取线程：行协议 -> 事件回调（等价 monitorThread.on("message", ...)）
                if let Some(stdout) = c.stdout.take() {
                    let callbacks = self.event_callbacks.clone();
                    // 等价 monitorThread.on("message", ...) 的 ready 分支：
                    // 读取线程直接置就绪标志（原版由主进程的 message 监听器完成）。
                    let ready_flag = self.ready.clone();
                    std::thread::spawn(move || {
                        let reader = BufReader::new(stdout);
                        for line in reader.lines() {
                            match line {
                                Ok(l) if !l.trim().is_empty() => {
                                    if let Ok(v) = serde_json::from_str::<Value>(&l) {
                                        if v.get("type").and_then(|t| t.as_str()) == Some("ready") {
                                            ready_flag.store(true, Ordering::SeqCst);
                                        }
                                        // 先克隆出监听器列表再调用，避免回调内注册监听器时自锁；
                                        // 也保证 start() 之后才注册的监听器能收到后续事件。
                                        let listeners: Vec<_> = match callbacks.lock() {
                                            Ok(list) => list.clone(),
                                            Err(_) => Vec::new(),
                                        };
                                        for cb in listeners.iter() {
                                            cb(&v);
                                        }
                                    }
                                }
                                Ok(_) => {}
                                Err(_) => break,
                            }
                        }
                    });
                }
                *self.process.lock().unwrap() = Some(WorkerProcess { child: c, stdin });
                self.starting.store(false, Ordering::SeqCst);
                self.wait_ready()
            }
            Err(e) => {
                self.starting.store(false, Ordering::SeqCst);
                println!("Monitor thread could not be started: {}", e);
                false
            }
        }
    }

    /// waitForMonitorThreadReady 等价（equiv L408-L434）：30s 超时。
    pub fn wait_ready(&self) -> bool {
        let start = Instant::now();
        while !self.ready.load(Ordering::SeqCst) {
            if start.elapsed().as_millis() as u64 >= READY_TIMEOUT_MS {
                return false;
            }
            // worker 进程已死则提前失败（等价 handleExit reject）
            let alive = {
                let mut guard = self.process.lock().unwrap();
                match guard.as_mut() {
                    Some(wp) => matches!(wp.child.try_wait(), Ok(None)),
                    None => false,
                }
            };
            if !alive {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        true
    }

    /// 标记就绪（worker 发来 ready 事件时调用）。
    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::SeqCst);
        self.starting.store(false, Ordering::SeqCst);
        // 等价 monitorsThreadFailedResetTimer（原注释：只有替换线程稳定运行一段时间才清标志）
        *self.failed_reset_at.lock().unwrap() = Some(Instant::now());
    }

    /// 失败标志复位检查（30s 到期时调用；等价 setTimeout 回调）。
    pub fn maybe_reset_failed(&self) {
        if let Some(at) = *self.failed_reset_at.lock().unwrap() {
            if at.elapsed().as_millis() as u64 >= FAILED_RESET_MS {
                self.failed.store(false, Ordering::SeqCst);
                *self.failed_reset_at.lock().unwrap() = None;
            }
        }
    }

    /// sendNow 等价（equiv L228-L247）：写 stdin 行协议。
    pub fn send_now(&self, data: &Value) -> bool {
        let mut guard = self.process.lock().unwrap();
        match guard.as_mut() {
            Some(wp) => {
                let line = serde_json::to_string(data).unwrap_or_default();
                match writeln!(wp.stdin, "{}", line) {
                    Ok(_) => true,
                    Err(e) => {
                        println!("Couldn't communicate with Monitor thread. {}", e);
                        false
                    }
                }
            }
            None => false,
        }
    }

    /// recoverMonitorThread 等价（equiv L443-L480）：
    /// 失败提示（仅首次）+ 1s 节流重启 + 就绪等待。
    pub fn recover(&self, error: &str) -> bool {
        if self.quitting.load(Ordering::SeqCst) || self.recovering.load(Ordering::SeqCst) {
            return false;
        }
        self.recovering.store(true, Ordering::SeqCst);
        println!("Monitor thread error: {}", error);

        if !self.failed.swap(true, Ordering::SeqCst) {
            // 等价 dialog.showMessageBox：只在首次失败时提示一次
            let message = format!(
                "The monitors thread failed with the following message: {}",
                error
            );
            let notice = self.failure_notice.lock().unwrap().clone();
            match notice {
                Some(cb) => cb(&message),
                None => println!("[Monitors thread failed] {}", message),
            }
        }

        self.stop();
        // Pace restarts so a repeatedly-failing worker cannot loop tightly.（原注释）
        std::thread::sleep(std::time::Duration::from_millis(RESTART_PACE_MS));
        if self.quitting.load(Ordering::SeqCst) {
            self.recovering.store(false, Ordering::SeqCst);
            return false;
        }
        let started = self.start(true);
        self.recovering.store(false, Ordering::SeqCst);
        started
    }

    /// stopMonitorThread 等价（equiv L482-L546）。
    pub fn stop(&self) {
        println!("Killing monitor thread");
        self.ready.store(false, Ordering::SeqCst);
        self.starting.store(false, Ordering::SeqCst);

        let mut guard = self.process.lock().unwrap();
        if let Some(wp) = guard.as_mut() {
            let _ = wp.child.kill();
            let _ = wp.child.wait();
        }
        *guard = None;
    }
}

/// CommandSender 实现（供 MonitorCommandQueue 驱动）。
pub struct ManagerSender<'a> {
    pub manager: &'a MonitorThreadManager,
}

impl<'a> crate::queue::CommandSender for ManagerSender<'a> {
    fn send_now(&mut self, data: &Value) -> bool {
        self.manager.send_now(data)
    }
    fn is_connected(&self) -> bool {
        self.manager.is_connected()
    }
    fn ensure_started(&mut self) {
        self.manager.start(false);
    }
    fn is_ready(&self) -> bool {
        self.manager.is_ready()
    }
}
