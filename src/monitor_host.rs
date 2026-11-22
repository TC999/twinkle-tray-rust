//! 主进程侧的 worker 宿主（启动、事件分发、命令发送）。
//!
//! equiv: electron.js:L237-L291（monitorsThread 包装）、L345-L406（startMonitorThread 的
//! ready 分支：settings / ddcBrightnessVCPs / wmi-bridge-ok / getLocalization 四连发）、
//! L551-L586（getVCP）、L588-L639（doWMIBridgeTest）。
//!
//! 线程模型：worker 管理对象在主线程持有；IPC 读写各自独立线程（AGENTS.md §5）。

use monitor_thread_manager::{get_ddc_safety_status, Json, MonitorCommandQueue, MonitorThreadManager};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// 主进程与 worker 之间的宿主（等价 electron.js 的 monitorsThread + 模块级状态）。
pub struct MonitorHost {
    pub manager: Arc<MonitorThreadManager>,
    pub queue: MonitorCommandQueue,
    /// 最近一次收到 monitors 的模型（等价 `let monitors`）
    pub monitors: Mutex<Value>,
    /// `let wmiBridgeOK`
    pub wmi_bridge_ok: Mutex<bool>,
    /// `let monitorRefreshGeneration`
    pub refresh_generation: Mutex<u64>,
    /// 最近接到的 worker 事件（供 UI 拉取）
    pub last_event: Mutex<Option<Value>>,
}

impl MonitorHost {
    /// 创建宿主（exe 路径为主程序自身，等价 fork(__filename)）。
    pub fn new(exe_path: String, datapath: String) -> Arc<Self> {
        let manager = Arc::new(MonitorThreadManager::new(exe_path, datapath));
        Arc::new(Self {
            manager,
            queue: MonitorCommandQueue::new(),
            monitors: Mutex::new(json!({})),
            wmi_bridge_ok: Mutex::new(false),
            refresh_generation: Mutex::new(0),
            last_event: Mutex::new(None),
        })
    }

    /// 安装事件回调：等价 monitorThread.on("message", ...) 的主进程分发。
    /// settings_snapshot 与 ddc_brightness_vcps 由调用方提供（worker ready 后立即下发）。
    pub fn install_handlers(self: &Arc<Self>, settings_snapshot: Value, ddc_brightness_vcps: Value) {
        let host = self.clone();
        let settings_snapshot = Arc::new(Mutex::new(settings_snapshot));
        let ddc_vcps = Arc::new(Mutex::new(ddc_brightness_vcps));
        self.manager.set_event_callback(Arc::new(move |data: &Value| {
            let event_type = data.get("type").and_then(|v| v.as_str()).unwrap_or("");
            *host.last_event.lock().unwrap() = Some(data.clone());
            match event_type {
                "ready" => {
                    // 等价 L367-L381：ready 后立刻下发 settings / ddcBrightnessVCPs /
                    // wmi-bridge-ok / localization 四类消息
                    let settings = settings_snapshot.lock().unwrap().clone();
                    host.manager.send_now(&json!({
                        "type": "settings",
                        "settings": settings,
                    }));
                    let vcps = ddc_vcps.lock().unwrap().clone();
                    host.manager.send_now(&json!({
                        "type": "ddcBrightnessVCPs",
                        "ddcBrightnessVCPs": vcps,
                    }));
                    let bridge_ok = *host.wmi_bridge_ok.lock().unwrap();
                    host.manager.send_now(&json!({
                        "type": "wmi-bridge-ok",
                        "value": bridge_ok,
                    }));
                }
                "refreshMonitors" => {
                    // 等价 commitRefreshedMonitors：只接受当前 generation 的结果
                    let generation = data.get("generation").and_then(|v| v.as_u64());
                    let current = *host.refresh_generation.lock().unwrap();
                    if generation.map(|g| g == current).unwrap_or(true) {
                        if let Some(monitors) = data.get("monitors") {
                            if monitors.is_object() {
                                *host.monitors.lock().unwrap() = monitors.clone();
                            }
                        }
                    }
                }
                "monitorsEnriched" => {
                    if data.get("failed").and_then(|v| v.as_bool()) != Some(true) {
                        if let Some(monitors) = data.get("monitors") {
                            if monitors.is_object() {
                                *host.monitors.lock().unwrap() = monitors.clone();
                            }
                        }
                    }
                }
                _ => {}
            }
        }));
    }

    /// 启动 worker（等价 startMonitorThread；返回是否就绪）。
    pub fn start(&self, allow_while_windows_idle: bool) -> bool {
        self.manager.start(allow_while_windows_idle)
    }

    /// 发送命令（等价 monitorsThread.send：互斥命令走队列）。
    pub fn send(&self, data: Value) -> bool {
        let monitors = self.monitors.lock().unwrap().clone();
        let mut sender = monitor_thread_manager::ManagerSender {
            manager: &self.manager,
        };
        if let Some(t) = data.get("type").and_then(|v| v.as_str()) {
            if monitor_thread_manager::COMMANDS_MUTATE_WORKER_STATE.contains(&t) {
                return self.queue.send(&data, &mut sender, &monitors);
            }
        }
        self.queue.send_now(&data, &mut sender)
    }

    /// refreshMonitors 的主进程入口（equiv refreshMonitors L2910+ 的消息侧）。
    pub fn refresh_monitors(&self, full_refresh: bool, clear_cache: bool) -> u64 {
        let generation = {
            let mut g = self.refresh_generation.lock().unwrap();
            *g += 1;
            *g
        };
        self.send(json!({
            "type": "refreshMonitors",
            "fullRefresh": full_refresh,
            "clearCache": clear_cache,
            "generation": generation,
        }));
        generation
    }

    /// getVCP 请求（等价 L551-L586）：等待匹配 getVCP::<monitor>::<code> 响应。
    /// 返回 None 表示超时或 worker 不可用（原版 reject 后由调用方 catch）。
    pub fn get_vcp(&self, monitor: &str, code: u8, timeout_ms: u64) -> Option<Value> {
        if !self.manager.send_now(&json!({
            "type": "getVCP",
            "monitor": monitor,
            "code": code,
        })) {
            return None;
        }
        let expected = format!("getVCP::{}::{}", monitor, code);
        let start = std::time::Instant::now();
        loop {
            if let Some(event) = self.last_event.lock().unwrap().clone() {
                if event.get("type").and_then(|v| v.as_str()) == Some(expected.as_str()) {
                    return event.get("value").cloned();
                }
            }
            if start.elapsed().as_millis() as u64 >= timeout_ms {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// doWMIBridgeTest 等价（equiv L588-L639）：探测原生 WMI 桥可用性。
    pub fn do_wmi_bridge_test(&self) -> bool {
        let result = wmibridge::get_brightness();
        // 桥能返回数据（含 unsupported）就说明 COM 链路可用
        let ok = !result.failed || result.unsupported;
        *self.wmi_bridge_ok.lock().unwrap() = ok;
        self.manager.send_now(&json!({
            "type": "wmi-bridge-ok",
            "value": ok,
        }));
        ok
    }

    /// getDDCSafetyStatus 等价（equiv L293-L300）：读 ddc-unstable.json 的 downgraded。
    pub fn ddc_safety_status(datapath: &std::path::Path) -> Value {
        get_ddc_safety_status(datapath).unwrap_or(json!(false))
    }

    /// 当前显示器模型快照（UI 渲染源）。
    pub fn monitors_snapshot(&self) -> Value {
        self.monitors.lock().unwrap().clone()
    }
}

/// 事件回调类型别名（便于 lib.rs 引用，避免重复写 trait 约束）。
pub type EventCallback = Arc<dyn Fn(&Json) + Send + Sync>;
