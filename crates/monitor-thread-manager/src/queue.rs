//! 命令队列与节流。
//!
//! equiv: electron.js:L158-L166（集合定义）、L195-L255（flushQueuedMonitorCommands +
//! queuedCommandTargetsCurrentMonitor + send/sendNow 语义）。
//! 节流是功能的一部分：DDC 写入过频会失灵（AGENTS.md §3.2），不得简化。

use serde_json::Value;
use std::sync::{Arc, Mutex};

/// monitorCommandsThatMutateWorkerState 等价（equiv L158-L162）。
pub const COMMANDS_MUTATE_WORKER_STATE: [&str; 10] = [
    "brightness",
    "sdr",
    "gamma",
    "vcp",
    "getVCP",
    "flushvcp",
    "settings",
    "ddcBrightnessVCPs",
    "localization",
    "wmi-bridge-ok",
];

/// monitorCommandsRequiredBeforeRefresh 等价（equiv L163-L166）。
pub const COMMANDS_REQUIRED_BEFORE_REFRESH: [&str; 5] =
    ["flushvcp", "settings", "ddcBrightnessVCPs", "localization", "wmi-bridge-ok"];

/// 队列中的一条命令（equiv queuedMonitorCommands 元素）。
#[derive(Debug)]
pub struct QueuedCommand {
    pub data: Value,
    pub generation: u64,
    pub completed: Arc<Mutex<bool>>,
}

/// 发送通道抽象：把实际 IPC 与队列解耦（测试可注入）。
pub trait CommandSender: Send {
    /// 等价 monitorsThreadReal.send(data)：true 成功。
    fn send_now(&mut self, data: &Value) -> bool;
    /// worker 是否存活（equiv connected && exitCode === null）。
    fn is_connected(&self) -> bool;
    /// 启动新 worker（equiv startMonitorThread）。
    fn ensure_started(&mut self);
    /// worker 就绪标志。
    fn is_ready(&self) -> bool;
}

/// 命令队列（equiv queuedMonitorCommands + flushingMonitorCommands + 排他工作标志）。
pub struct MonitorCommandQueue {
    pub queued: Mutex<Vec<QueuedCommand>>,
    pub flushing: Mutex<bool>,
    /// isRefreshing || activeCapabilitiesEnrichment || queuedRefresh（equiv L170-L172）
    pub exclusive_work: Mutex<u32>,
    /// monitorRefreshGeneration（equiv）
    pub generation: Mutex<u64>,
}

impl MonitorCommandQueue {
    pub fn new() -> Self {
        Self {
            queued: Mutex::new(Vec::new()),
            flushing: Mutex::new(false),
            exclusive_work: Mutex::new(0),
            generation: Mutex::new(0),
        }
    }

    /// monitorWorkerHasExclusiveWork 等价（equiv L170-L172）。
    pub fn has_exclusive_work(&self) -> bool {
        *self.exclusive_work.lock().unwrap() > 0
    }

    /// 等价 queuedCommandTargetsCurrentMonitor（equiv L223-L240）。
    /// monitors 参数是当前显示器表（id -> monitor）。
    pub fn command_targets_current_monitor(&self, data: &Value, monitors: &Value) -> bool {
        let t = data.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if t == "brightness" || t == "sdr" || t == "gamma" {
            match data.get("id").and_then(|v| v.as_str()) {
                None => true,
                Some(id) => {
                    if let Some(list) = monitors.as_object() {
                        list.values().any(|m| {
                            m.get("id")
                                .and_then(|v| v.as_str())
                                .map(|mid| mid == id || mid.contains(id))
                                .unwrap_or(false)
                        })
                    } else {
                        false
                    }
                }
            }
        } else if t == "vcp" || t == "getVCP" {
            let target = data.get("monitor").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(list) = monitors.as_object() {
                list.values().any(|m| {
                    m.get("hwid")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str())
                                .collect::<Vec<_>>()
                                .join("#")
                                == target
                        })
                        .unwrap_or(false)
                })
            } else {
                false
            }
        } else {
            false
        }
    }

    /// send 等价（equiv L249-L259）：
    /// 变更类命令 + 排他工作 -> 入队；否则直发。
    pub fn send(&self, data: &Value, sender: &mut dyn CommandSender, _monitors: &Value) -> bool {
        let t = data.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if COMMANDS_MUTATE_WORKER_STATE.contains(&t) && self.has_exclusive_work() {
            let generation = *self.generation.lock().unwrap();
            self.queued.lock().unwrap().push(QueuedCommand {
                data: data.clone(),
                generation,
                completed: Arc::new(Mutex::new(false)),
            });
            return true; // 入队成功（等价 Promise pending）
        }
        self.send_now(data, sender)
    }

    /// sendNow 等价（equiv L228-L247）：不排队直发；掉线时自动重启 worker 并等待就绪。
    pub fn send_now(&self, data: &Value, sender: &mut dyn CommandSender) -> bool {
        if !sender.is_connected() {
            sender.ensure_started();
            // 等价 while(!monitorsThreadReady) await Utils.wait(50)
            let mut waited = 0u64;
            while !sender.is_ready() {
                std::thread::sleep(std::time::Duration::from_millis(50));
                waited += 50;
                if waited > 30_000 {
                    break; // 等价 waitForMonitorThreadReady 的 30s 超时
                }
            }
        }
        if !sender.is_ready() || !sender.is_connected() {
            println!("Couldn't communicate with Monitor thread.");
            return false;
        }
        sender.send_now(data)
    }

    /// flushQueuedMonitorCommands 等价（equiv L174-L213）。
    /// beforeRefresh: 只 flush REQUIRED_BEFORE_REFRESH 集合内的命令。
    pub fn flush(&self, sender: &mut dyn CommandSender, before_refresh: bool, monitors: &Value) {
        // 等价 flushingMonitorCommands 重入检查
        {
            let flushing = self.flushing.lock().unwrap();
            if *flushing {
                drop(flushing);
                while *self.flushing.lock().unwrap() {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                return self.flush(sender, before_refresh, monitors);
            }
        }
        let exclusive = self.has_exclusive_work();
        let queued_len = self.queued.lock().unwrap().len();
        if exclusive || (!before_refresh && queued_len > 0 && false) || queued_len == 0 {
            // 原版条件: exclusiveWorkInProgress || (!beforeRefresh && queuedRefreshPromise) || !length
            // queuedRefreshPromise 在此实现中并入 exclusive_work 计数
            return;
        }

        *self.flushing.lock().unwrap() = true;

        // 分拣
        let (mut commands, mut remaining): (Vec<QueuedCommand>, Vec<QueuedCommand>) =
            (Vec::new(), Vec::new());
        {
            let mut queued = self.queued.lock().unwrap();
            for command in queued.drain(..) {
                let t = command.data.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if before_refresh && COMMANDS_REQUIRED_BEFORE_REFRESH.contains(&t) {
                    commands.push(command);
                } else if !before_refresh {
                    commands.push(command);
                } else {
                    remaining.push(command);
                }
            }
        }
        *self.queued.lock().unwrap() = remaining;

        let generation = *self.generation.lock().unwrap();
        for command in commands {
            let t = command.data.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let required = COMMANDS_REQUIRED_BEFORE_REFRESH.contains(&t);
            let targets_current = self.command_targets_current_monitor(&command.data, monitors);
            if !required && command.generation != generation && !targets_current {
                // 过期命令直接完成（equiv command.resolve(); continue）
                *command.completed.lock().unwrap() = true;
                continue;
            }
            let sent = sender.send_now(&command.data);
            *command.completed.lock().unwrap() = sent;
        }

        *self.flushing.lock().unwrap() = false;
        // 等价 finally 中 if queued.length && !beforeRefresh flush()
        if !before_refresh && self.queued.lock().unwrap().len() > 0 {
            self.flush(sender, false, monitors);
        }
    }
}

impl Default for MonitorCommandQueue {
    fn default() -> Self {
        Self::new()
    }
}
