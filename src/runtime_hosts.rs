//! 主进程运行时状态机：known displays / TOD 亮度计划 / idle 检测 / 亮度节流共享层。
//!
//! equiv: electron.js 的模块级状态集合（updateKnownDisplays L1692-L1723、
//! getKnownDisplays L1726-L1747、setKnownBrightness L1750-L1760、applyProfile
//! L1762-L1798、shouldSkipDisplay L1676-L1689、TOD 事件 L5625-L5827、
//! handleBackgroundUpdate L5835-L5856、idle 检测 L5485-L5622、
//! updateAllBrightness L3353-L3391、transitionBrightness L3426-L3482）。
//!
//! 线程模型（AGENTS.md §5）：原版全部定时器都在 Electron 单线程事件循环上，
//! 这里等价收敛到主线程（Slint Timer 驱动）；热键动作在独立线程执行，
//! 状态一律经 Mutex/Atomic 共享，与原版"事件驱动但同线程时序"可观察行为一致。

use crate::monitor_host::MonitorHost;
use crate::monitor_view;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// tempSettings（equiv L140-L148 的 temp_settings：运行时暂停开关）。
#[derive(Debug, Default, Clone)]
pub struct TempSettings {
    pub pause_idle_detection: bool,
    pub pause_time_adjustments: bool,
}

/// 运行时宿主（等价 electron.js 的模块级 let 状态集合）。
pub struct RuntimeHost {
    pub host: Arc<MonitorHost>,
    pub settings: Arc<Mutex<Value>>,
    pub temp: Mutex<TempSettings>,
    /// known-displays.json 路径（equiv knownDisplaysPath）
    pub known_displays_path: std::path::PathBuf,
    /// lastKnownDisplays（equiv L1629）
    pub last_known_displays: Mutex<Option<Value>>,
    /// isUserIdle（equiv L5486）
    pub is_user_idle: AtomicBool,
    /// userIdleDimmed（equiv L5489）
    pub user_idle_dimmed: AtomicBool,
    /// lastIdleTime（equiv L5495）
    pub last_idle_time: AtomicU64,
    /// isWindowsUserIdle（equiv L5491：显示器已关闭）
    pub is_windows_user_idle: AtomicBool,
    /// preIdleMonitors（equiv L5497）
    pub pre_idle_monitors: Mutex<Value>,
    /// recentlyWokeUp（equiv L5305）
    pub recently_woke_up: AtomicBool,
    /// resumeRecoveryInProgress（equiv L5307）
    pub resume_recovery_in_progress: AtomicBool,
    /// resumeRecoveryHandled（equiv L5308）
    pub resume_recovery_handled: AtomicBool,
    /// hasRecentlyInteracted（equiv L3520 附近的模块级状态）
    pub has_recently_interacted: AtomicBool,
    /// lastTimeEvent（equiv L5830：TOD 上次应用的事件）
    pub last_time_event: Mutex<Option<Value>>,
    /// isStartupGracePeriod（equiv L5246：启动宽限期，显示器变化时保留现场亮度）
    pub startup_grace_period: AtomicBool,
    /// isRefreshing（equiv L2892 附近的模块级 isRefreshing）
    pub is_refreshing: AtomicBool,
    /// pausedMonitorUpdates（equiv pauseMonitorUpdates L3008）
    pub paused_monitor_updates: AtomicBool,
    /// pausedMonitorUpdates 的到期时刻（equiv setTimeout 的到期时间）
    pub paused_until: Mutex<Option<Instant>>,
    /// skippedFirstMonitorChange（equiv skipFirstMonChange L5245：跳过启动时的首个事件）
    pub skipped_first_monitor_change: AtomicBool,
    /// 热键节流（equiv hotkeyThrottle L1834：hotkey.id -> 上次触发 ms）
    pub hotkey_throttle: Mutex<HashMap<String, u64>>,
    /// 热键 cycle 索引（equiv hotkeyCycleIndexes L1836）
    pub hotkey_cycle_indexes: Mutex<HashMap<String, usize>>,
    /// doingHotkey（equiv L1835）
    pub doing_hotkey: AtomicBool,
    /// currentTransition（equiv L3425 的 currentTransition 定时器句柄；
    /// Rust 侧以代数标记，transition tick 由主线程 Timer 驱动）
    pub transition: Mutex<Option<TransitionState>>,
    /// idleRestoreDeadline（等价 idleCheckShort 里 setTimeout(idleRestore, Ns) 的到期时间）
    pub idle_restore_deadline: Mutex<Option<Instant>>,
}

/// 进行中的过渡（equiv currentTransition + setInterval 闭包状态）。
#[derive(Debug, Clone)]
pub struct TransitionState {
    /// 目标亮度（全局值，或 per-monitor 的 eventMonitors[id]）
    pub level: f64,
    /// per-monitor 目标（equiv eventMonitors：monitor.id -> level）
    pub event_monitors: Value,
    /// 每步步长（equiv step = stepSpeed * stepSpeedMult）
    pub step: f64,
    /// tick 间隔（equiv settings.updateInterval * transitionIntervalMult）
    pub interval_ms: u64,
}

impl RuntimeHost {
    pub fn new(host: Arc<MonitorHost>, settings: Arc<Mutex<Value>>, known_displays_path: std::path::PathBuf) -> Arc<Self> {
        Arc::new(Self {
            host,
            settings,
            temp: Mutex::new(TempSettings::default()),
            known_displays_path,
            last_known_displays: Mutex::new(None),
            is_user_idle: AtomicBool::new(false),
            user_idle_dimmed: AtomicBool::new(false),
            last_idle_time: AtomicU64::new(0),
            is_windows_user_idle: AtomicBool::new(false),
            pre_idle_monitors: Mutex::new(json!({})),
            recently_woke_up: AtomicBool::new(false),
            resume_recovery_in_progress: AtomicBool::new(false),
            resume_recovery_handled: AtomicBool::new(false),
            has_recently_interacted: AtomicBool::new(false),
            last_time_event: Mutex::new(None),
            startup_grace_period: AtomicBool::new(true),
            is_refreshing: AtomicBool::new(false),
            paused_monitor_updates: AtomicBool::new(false),
            paused_until: Mutex::new(None),
            skipped_first_monitor_change: AtomicBool::new(false),
            hotkey_throttle: Mutex::new(HashMap::new()),
            hotkey_cycle_indexes: Mutex::new(HashMap::new()),
            doing_hotkey: AtomicBool::new(false),
            transition: Mutex::new(None),
            idle_restore_deadline: Mutex::new(None),
        })
    }

    pub fn settings_snapshot(&self) -> Value {
        self.settings.lock().map(|g| g.clone()).unwrap_or(json!({}))
    }

    /// writeSettings 等价的内存写回（文件落盘由调用方决定；equiv writeSettings 的内存分支）。
    pub fn update_settings(&self, patch: Value) {
        if let (Ok(mut g), Some(obj)) = (self.settings.lock(), patch.as_object()) {
            if let Some(target) = g.as_object_mut() {
                for (k, v) in obj {
                    target.insert(k.clone(), v.clone());
                }
            }
        }
    }

    /// setRecentlyInteracted（equiv L3520-L3525）。
    pub fn set_recently_interacted(&self, v: bool) {
        self.has_recently_interacted.store(v, Ordering::SeqCst);
    }

    /// pauseMonitorUpdates（equiv L3008-L3011）：暂停处理回推 updateInterval*2 毫秒，
    /// 防止连发时的抖动。到期时间由 system_events 的 tick 检查并复位。
    pub fn pause_monitor_updates(&self) {
        let settings = self.settings_snapshot();
        let interval = settings.get("updateInterval").and_then(|v| v.as_u64()).unwrap_or(500);
        self.paused_until
            .lock()
            .unwrap()
            .replace(Instant::now() + Duration::from_millis(interval * 2));
        self.paused_monitor_updates.store(true, Ordering::SeqCst);
    }

    /// pausedMonitorUpdates 到期复位（equiv setTimeout(() => pausedMonitorUpdates = false)）。
    /// 返回 true 表示刚好复位。
    pub fn pause_expire_tick(&self) -> bool {
        let due = {
            let mut until = self.paused_until.lock().unwrap();
            match *until {
                Some(d) if Instant::now() >= d => {
                    *until = None;
                    true
                }
                _ => false,
            }
        };
        if due {
            self.paused_monitor_updates.store(false, Ordering::SeqCst);
        }
        due
    }
}

// ---------------------------------------------------------------------------
// Known displays（equiv L1629-L1760）
// ---------------------------------------------------------------------------

/// getKnownDisplays（equiv L1726-L1747）：读文件 + 可选并入当前显示器。
/// 返回 (known, last_known)——last_known 由调用方写回缓存。
pub fn get_known_displays(rt: &RuntimeHost, use_current_monitors: bool) -> Value {
    // lastKnownDisplays 缓存命中则直接用（equiv !lastKnownDisplays 分支的短路）
    let mut known = {
        let guard = rt.last_known_displays.lock().unwrap();
        match guard.as_ref() {
            Some(v) => v.clone(),
            None => {
                // 读 known-displays.json（equiv fs.readFileSync + JSON.parse 的 catch {}）
                match std::fs::read_to_string(&rt.known_displays_path) {
                    Ok(text) => serde_json::from_str::<Value>(&text).unwrap_or(json!({})),
                    Err(_) => json!({}),
                }
            }
        }
    };

    // useCurrentMonitors 时并入当前显示器深拷贝（equiv Object.assign(known, JSON.parse(JSON.stringify(monitors)))）
    if use_current_monitors {
        let monitors = rt.host.monitors_snapshot();
        if let (Some(dst), Some(src)) = (known.as_object_mut(), monitors.as_object()) {
            for (k, v) in src {
                dst.insert(k.clone(), v.clone());
            }
        }
    }
    *rt.last_known_displays.lock().unwrap() = Some(known.clone());
    known
}

/// updateKnownDisplays（equiv L1692-L1723）：把当前显示器存盘。
/// immediate=false 时由调用方安排 3s 延迟（原版 setTimeout 3000）。
pub fn update_known_displays(rt: &RuntimeHost, force: bool, immediate: bool) -> bool {
    // Skip when idle（equiv !force && isUserIdle）
    if !force && rt.is_user_idle.load(Ordering::SeqCst) {
        return false;
    }
    if !immediate {
        // 延迟版本由 system_events 的定时器处理；这里直接返回"需要延迟保存"
        return false;
    }
    let known = get_known_displays(rt, true);
    // Write back to file（equiv fs.writeFileSync 的 catch -> console.error）
    let text = known.to_string();
    match std::fs::write(&rt.known_displays_path, text) {
        Ok(_) => {
            println!("Saved known displays!");
            true
        }
        Err(e) => {
            eprintln!("Couldn't update known displays file. {}", e);
            false
        }
    }
}

/// shouldSkipDisplay（equiv L1676-L1689）：hwid[1] 命中 skipReapply/userSkipReapply 则跳过。
pub fn should_skip_display(rt: &RuntimeHost, monitor: &Value, skip_event_check: bool) -> bool {
    // displaysMayBeIdleBlocks 为空的快速路径（Rust 侧该集合由 idle/TOD 的 block 区间管理；
    // 等价实现：user_idle_dimmed 或 windows idle 视为"存在 block"）
    let blocked = rt.user_idle_dimmed.load(Ordering::SeqCst);
    if !blocked && !skip_event_check {
        return false;
    }
    let hwid1 = monitor
        .get("hwid")
        .and_then(|v| v.as_array())
        .and_then(|a| a.get(1))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let settings = rt.settings_snapshot();
    let mut rules: Vec<String> = Vec::new();
    if let Some(list) = settings.get("monitorRules").and_then(|v| v.get("skipReapply")).and_then(|v| v.as_array()) {
        rules.extend(list.iter().filter_map(|v| v.as_str().map(|s| s.to_string())));
    }
    if let Some(list) = settings.get("userSkipReapply").and_then(|v| v.as_array()) {
        rules.extend(list.iter().filter_map(|v| v.as_str().map(|s| s.to_string())));
    }
    rules.iter().any(|r| r == hwid1)
}

/// setKnownBrightness（equiv L1750-L1760）：恢复所有已知显示器的亮度 + 重放 gamma ramp。
pub fn set_known_brightness(rt: &RuntimeHost, use_current_monitors: bool, use_transition: bool, transition_speed: f64) {
    println!("Setting brightness for known displays {} {} {}", use_current_monitors, use_transition, transition_speed);
    let known = get_known_displays(rt, use_current_monitors);
    apply_profile(rt, &known, use_transition, transition_speed, false);
    // Brightness alone won't restore a ramp that an event cleared...
    // （equiv reapplyGammaRamps L1589：worker 端在 brightness 设置后自动重放 ramp；
    // 这里发送显式重放指令保持时序等价）
    rt.host.send(json!({ "type": "reapplyGammaRamps" }));
}

/// applyProfile（equiv L1762-L1798）：把 profile（known displays map）应用到显示器。
pub fn apply_profile(rt: &RuntimeHost, profile: &Value, use_transition: bool, transition_speed: f64, skip_bad_displays: bool) {
    let _ = skip_bad_displays; // 原版参数未在函数体中使用，逐字保留签名
    let settings = rt.settings_snapshot();

    if use_transition {
        // If using smooth transition（equiv L1767-L1777）
        let mut transition_monitors = json!({});
        if let Some(obj) = profile.as_object() {
            for (_hwid, monitor) in obj {
                if should_skip_display(rt, monitor, false) {
                    continue;
                }
                let id = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let level = monitor.get("brightness").and_then(|v| v.as_f64()).unwrap_or(0.0);
                if !id.is_empty() {
                    transition_monitors[id] = json!(level);
                }
            }
        }
        transition_brightness(rt, 50.0, &transition_monitors, transition_speed);
    } else {
        // If not using a transition（equiv L1778-L1795）
        if let Some(obj) = profile.as_object() {
            for (_hwid, monitor) in obj {
                if should_skip_display(rt, monitor, false) {
                    continue;
                }
                let mtype = monitor.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let brightness_type = monitor
                    .get("brightnessType")
                    .and_then(|v| v.as_u64())
                    .map(|v| v > 0)
                    .unwrap_or(false);
                let valid = mtype == "wmi"
                    || mtype == "studio-display"
                    || mtype == "software"
                    || (mtype == "ddcci" && brightness_type)
                    || monitor_view::uses_gamma_slider(monitor, &settings);
                if !valid {
                    continue;
                }
                // Replace DDC/CI brightness with SDR（equiv L1788-L1790）
                let mut level = monitor.get("brightness").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let key = monitor.get("key").and_then(|v| v.as_str()).unwrap_or("");
                let sdr_main = settings
                    .get("sdrAsMainSliderDisplays")
                    .and_then(|v| v.get(key))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if sdr_main && monitor.get("hdr").and_then(|v| v.as_str()) == Some("active") {
                    level = monitor.get("sdrLevel").and_then(|v| v.as_f64()).unwrap_or(level);
                }
                let id = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if id.is_empty() {
                    continue;
                }
                update_brightness(rt, id, level, true, "brightness");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// updateBrightness 主进程决策层(equiv L3138-L3300 的主进程侧)
// ---------------------------------------------------------------------------

/// updateBrightness(equiv L3138-L3187 的主进程判定 + 节流入队)。
/// index 为 monitor.id(等价原版字符串 id 前缀匹配);vcp_value 可为
/// "brightness"/"sdr"/"gamma"/"volume-raw"/数字码。
pub fn update_brightness(rt: &RuntimeHost, index: &str, new_level: f64, use_cap: bool, vcp_value: &str) -> bool {
    // Skip if displays are off(equiv L3139)
    if rt.is_windows_user_idle.load(Ordering::SeqCst) {
        return false;
    }
    // NaN reaches the worker as null, which the VCP path writes as 0(equiv L3141)
    if !new_level.is_finite() {
        println!("updateBrightness: Ignoring invalid level for {}: {}", index, new_level);
        return false;
    }

    let monitors = rt.host.monitors_snapshot();
    let settings = rt.settings_snapshot();

    // 找显示器:id 前缀匹配(equiv L3158-L3168:id.indexOf(index) === 0)
    let mut monitor: Option<Value> = None;
    if let Some(obj) = monitors.as_object() {
        for (_k, m) in obj {
            let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if id.starts_with(index) {
                monitor = Some(m.clone());
                break;
            }
        }
    }
    let Some(monitor) = monitor else {
        println!("Monitor does not exist: {}", index);
        return false;
    };

    let key = monitor.get("key").and_then(|v| v.as_str()).unwrap_or("");
    // hideDisplays 过滤(equiv L3175-L3177)
    if settings
        .get("hideDisplays")
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return false;
    }

    // VCP 归一化(equiv L3147-L3155)
    let mut vcp = match vcp_value {
        "volume-raw" => "0x62".to_string(),
        "brightness" => "brightness".to_string(),
        "sdr" => "sdr".to_string(),
        "gamma" => "gamma".to_string(),
        other => {
            match other.parse::<i64>() {
                Ok(n) => format!("0x{:x}", n),
                Err(_) => other.to_string(),
            }
        }
    };
    let mut use_cap = use_cap;

    // HDR SDR 主滑块(equiv L3180-L3183)
    if vcp == "brightness"
        && monitor.get("hdr").and_then(|v| v.as_str()) == Some("active")
        && settings
            .get("sdrAsMainSliderDisplays")
            .and_then(|v| v.get(key))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    {
        vcp = "sdr".to_string();
        use_cap = false;
    }
    // gamma 主滑块(equiv L3185-L3187)
    if vcp == "brightness" && monitor_view::uses_gamma_slider(&monitor, &settings) {
        vcp = "gamma".to_string();
    }

    // 扩展下限断点(equiv L3207-L3217):断点以下钉住硬件最低值,用 gamma 压暗
    let extended_minimum = vcp == "brightness" && monitor_view::uses_extended_minimum(&monitor, &settings);
    let mut hardware_level = new_level;
    if extended_minimum {
        let breakpoint = monitor_view::get_extended_minimum_breakpoint(&monitor, &settings) as f64;
        hardware_level = if new_level >= breakpoint {
            (new_level - breakpoint) * 100.0 / (100.0 - breakpoint)
        } else {
            0.0
        };
        let gamma_level = if new_level >= breakpoint {
            100.0
        } else {
            monitor_view::GAMMA_BRIGHTNESS_MIN
                + (new_level * (100.0 - monitor_view::GAMMA_BRIGHTNESS_MIN) / breakpoint)
        };
        // setTrackedGammaLevel:gamma 写入不节流(原版 gammaBrightnessQueue 即刻 flush)
        let id = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if !id.is_empty() {
            rt.host.send(json!({
                "type": "gamma",
                "brightness": monitor_view::slider_level_to_gamma_level(gamma_level),
                "id": id,
            }));
        }
    }

    // normalizeBrightness(equiv L3219)
    let min = if use_cap { monitor.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0) } else { 0.0 };
    let max = if use_cap { monitor.get("max").and_then(|v| v.as_f64()).unwrap_or(100.0) } else { 100.0 };
    let calibration = if use_cap { monitor_view::calibration_of(&monitor) } else { Vec::new() };
    let normalized = monitor_view::normalize_brightness(hardware_level, false, min, max, &calibration);

    let id = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if id.is_empty() {
        return false;
    }

    // 下发(equiv L3231-L3300 的 send 分支;节流在调用方 updateBrightnessThrottle)
    match vcp.as_str() {
        "sdr" => {
            rt.host.send(json!({ "type": "sdr", "brightness": new_level, "id": id }));
        }
        "gamma" => {
            rt.host.send(json!({ "type": "gamma", "brightness": monitor_view::slider_level_to_gamma_level(new_level), "id": id }));
        }
        _ => {
            let is_wmi = monitor.get("type").and_then(|v| v.as_str()) == Some("wmi");
            if is_wmi {
                // WMI 显示器不带 id(等价原版全局 WMI 分支,worker 走 None 路径)
                rt.host.send(json!({ "type": "brightness", "brightness": normalized }));
            } else {
                rt.host.send(json!({ "type": "brightness", "brightness": normalized, "id": id }));
            }
        }
    }
    true
}

/// updateBrightnessThrottle(equiv L3053-L3086):per-(id,vcp) 节流。
/// Rust 侧节流器在 panel_controller::BrightnessThrottle 已有等价实现;
/// 这里对其转发(同一数据结构,供运行时路径复用)。
pub fn throttled_update(
    throttle: &mut crate::panel_controller::BrightnessThrottle,
    rt: &RuntimeHost,
    id: &str,
    level: f64,
    vcp: &str,
    send_update: bool,
) {
    let settings = rt.settings_snapshot();
    let interval = settings.get("updateInterval").and_then(|v| v.as_u64()).unwrap_or(500);
    let _ = send_update; // sendToAllWindows 等价物 = UI Timer 拉模型,无需显式通知
    if let Some((id, level, vcp)) = throttle.request(id, level, vcp, interval) {
        update_brightness(rt, &id, level, true, &vcp);
    }
}

// ---------------------------------------------------------------------------
// updateAllBrightness(equiv L3353-L3391):全部显示器同步增减
// ---------------------------------------------------------------------------

pub fn update_all_brightness(rt: &RuntimeHost, brightness: f64, mode: &str) {
    let settings = rt.settings_snapshot();
    let mut monitors = rt.host.monitors_snapshot();
    let mut linked_level_val: Option<f64> = None;

    // Update internal brightness values(equiv L3358-L3382)
    if let Some(obj) = monitors.as_object_mut() {
        for (_k, monitor) in obj.iter_mut() {
            let mtype = monitor.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if mtype != "none" || monitor_view::uses_gamma_slider(monitor, &settings) {
                let key = monitor.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
                // Replace DDC/CI brightness with SDR
                if settings
                    .get("sdrAsMainSliderDisplays")
                    .and_then(|v| v.get(&key))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                    && monitor.get("hdr").and_then(|v| v.as_str()) == Some("active")
                {
                    let sdr = monitor.get("sdrLevel").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    monitor["brightness"] = json!(sdr);
                }
                let current = monitor.get("brightness").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let mut normalized_adjust = min_max(if mode == "set" { brightness } else { brightness + current });
                // Use linked levels, if applicable(equiv L3370-L3377)
                if settings.get("linkedLevelsActive").and_then(|v| v.as_bool()).unwrap_or(false) {
                    if let Some(linked) = linked_level_val {
                        normalized_adjust = linked;
                    } else {
                        linked_level_val = Some(normalized_adjust);
                    }
                }
                monitor["brightness"] = json!(normalized_adjust);
                if settings
                    .get("sdrAsMainSliderDisplays")
                    .and_then(|v| v.get(&key))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    monitor["sdrLevel"] = json!(normalized_adjust);
                }
            }
        }
    }
    *rt.host.monitors.lock().unwrap() = monitors.clone();

    // Send brightness updates(equiv L3388-L3390)
    if let Some(obj) = monitors.as_object() {
        for (_k, monitor) in obj {
            let id = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let level = monitor.get("brightness").and_then(|v| v.as_f64()).unwrap_or(0.0);
            if !id.is_empty() {
                update_brightness(rt, id, level, true, "brightness");
            }
        }
    }
}

/// minMax(equiv L2163-L2165)。
pub fn min_max(value: f64) -> f64 {
    value.min(100.0).max(0.0)
}

// ---------------------------------------------------------------------------
// transitionBrightness(equiv L3426-L3482):分步过渡
// ---------------------------------------------------------------------------

/// 启动过渡(equiv transitionBrightness 的 setInterval 启动段)。
/// tick 由 system_events 的定时器驱动 advance_transition。
pub fn transition_brightness(rt: &RuntimeHost, level: f64, event_monitors: &Value, step_speed: f64) {
    let settings = rt.settings_snapshot();
    // Slow down transition(equiv L3430-L3435)
    let transition_interval_mult = match settings.get("adjustmentTimeSpeed").and_then(|v| v.as_str()) {
        Some("slow") => 4.0,
        Some("slowest") => 10.0,
        _ => 1.0,
    };
    // Speed up transition(equiv L3437-L3443)
    let step_speed_mult = match settings.get("adjustmentTimeSpeed").and_then(|v| v.as_str()) {
        Some("fast") => 3.0,
        Some("fastest") => 6.0,
        _ => 1.0,
    };
    let step = step_speed * step_speed_mult;
    let interval_ms = (settings.get("updateInterval").and_then(|v| v.as_f64()).unwrap_or(500.0) * transition_interval_mult) as u64;

    *rt.transition.lock().unwrap() = Some(TransitionState {
        level,
        event_monitors: event_monitors.clone(),
        step,
        interval_ms,
    });
}

/// 过渡 tick(等价 setInterval 回调 L3447-L3481)。由主线程定时器周期调用;
/// 返回 true 表示过渡仍在进行。
pub fn advance_transition(rt: &RuntimeHost) -> bool {
    let Some(state) = rt.transition.lock().unwrap().clone() else {
        return false;
    };
    // recentlyWokeUp 时中止(equiv L3448)
    if rt.recently_woke_up.load(Ordering::SeqCst) || rt.is_windows_user_idle.load(Ordering::SeqCst) {
        *rt.transition.lock().unwrap() = None;
        return false;
    }

    let settings = rt.settings_snapshot();
    let mut monitors = rt.host.monitors_snapshot();
    let mut num_done = 0usize;
    let mut total = 0usize;

    if let Some(obj) = monitors.as_object_mut() {
        total = obj.len();
        for (_k, monitor) in obj.iter_mut() {
            let mut normalized = state.level;
            if settings.get("adjustmentTimeIndividualDisplays").and_then(|v| v.as_bool()).unwrap_or(false) {
                let id = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let per = state.event_monitors.get(id).and_then(|v| v.as_f64()).unwrap_or(-1.0);
                // eventMonitors[id] >= 0 ? eventMonitors[id] : level(equiv L3456)
                normalized = if per >= 0.0 { per } else { state.level };
            }

            let current = monitor.get("brightness").and_then(|v| v.as_f64());
            let Some(current) = current else {
                // Nothing to step from, so don't hold the transition open(equiv L3466-L3469)
                num_done += 1;
                continue;
            };
            if current < normalized + (state.step + 1.0) && current > normalized - (state.step + 1.0) {
                let id = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if !id.is_empty() {
                    update_brightness(rt, &id, normalized, true, "brightness");
                }
                num_done += 1;
            } else {
                let next = if current < normalized { current + state.step } else { current - state.step };
                let id = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if !id.is_empty() {
                    update_brightness(rt, &id, next, true, "brightness");
                }
            }
        }
    }
    *rt.host.monitors.lock().unwrap() = monitors;
    if total > 0 && num_done == total {
        *rt.transition.lock().unwrap() = None;
        return false;
    }
    true
}

/// transitionlessBrightness(equiv L3484-L3495)。
pub fn transitionless_brightness(rt: &RuntimeHost, level: f64, event_monitors: &Value) {
    let settings = rt.settings_snapshot();
    let monitors = rt.host.monitors_snapshot();
    if let Some(obj) = monitors.as_object() {
        for (_k, monitor) in obj {
            let mut normalized = level;
            if settings.get("adjustmentTimeIndividualDisplays").and_then(|v| v.as_bool()).unwrap_or(false) {
                let id = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let per = event_monitors.get(id).and_then(|v| v.as_f64()).unwrap_or(-1.0);
                normalized = if per >= 0.0 { per } else { level };
            }
            let id = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if !id.is_empty() {
                update_brightness(rt, id, normalized, true, "brightness");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Time of Day Adjustments(equiv L5625-L5856)
// ---------------------------------------------------------------------------

/// Utils.parseTime(equiv Utils.js:L273-L275):"H:M" -> 当日分钟数。
pub fn parse_time(time: &str) -> i64 {
    let parts: Vec<&str> = time.split(':').collect();
    let h = parts.first().and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or(0);
    let m = parts.get(1).and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or(0);
    h * 60 + m
}

/// getSunCalcTime(equiv L5733-L5738):sunCalc 时间名 + 偏移 -> "H:M"。
/// timezone_offset_minutes 为本地时区偏移(由 system_events 经 Win32 取得)。
pub fn get_sun_calc_time(settings: &Value, time_name: &str, offset_minutes: i64, timezone_offset_minutes: i64) -> String {
    let lat = settings.get("adjustmentTimeLatitude").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let long = settings.get("adjustmentTimeLongitude").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0);
    let times = suncalc::get_times(now_ms, lat, long, 0.0);
    let default_ms = 0.0f64;
    let ms = times.get(time_name).unwrap_or(&default_ms);
    let (h, m) = suncalc::ms_to_local_hm(*ms, timezone_offset_minutes);
    // offset 加到分钟上再归一(等价 JS Date.setMinutes 的进位)
    let total = (h * 60 + m + offset_minutes).rem_euclid(24 * 60);
    format!("{}:{}", total / 60, total % 60)
}

/// getCurrentAdjustmentEvent(equiv L5626-L5653):找当前时刻最近的事件。
pub fn get_current_adjustment_event(rt: &RuntimeHost, tz_offset: i64) -> Option<Value> {
    let settings = rt.settings_snapshot();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let now_value = local_minute_of_day(now_ms, tz_offset);

    let mut found_event: Option<Value> = None;
    let mut found_value: i64 = 0;
    let events = settings.get("adjustmentTimes").and_then(|v| v.as_array())?;
    for event in events {
        let event_time = if event.get("useSunCalc").and_then(|v| v.as_bool()).unwrap_or(false) {
            let name = event.get("sunCalc").and_then(|v| v.as_str()).unwrap_or("solarNoon").to_string();
            let offset = event.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
            get_sun_calc_time(&settings, &name, offset, tz_offset)
        } else {
            event.get("time").and_then(|v| v.as_str()).unwrap_or("0:0").to_string()
        };
        let event_value = parse_time(&event_time);
        // Check if event is not later than current time...(equiv L5639-L5645)
        if event_value <= now_value && (found_event.is_none() || found_value <= event_value) {
            let mut found = event.clone();
            if let Some(obj) = found.as_object_mut() {
                obj.insert("value".to_string(), json!(event_value));
            }
            found_event = Some(found);
            found_value = event_value;
        }
    }
    found_event
}

/// 当地时间的一天分钟数(等价 (date.getHours()*60)+date.getMinutes())。
fn local_minute_of_day(unix_ms: u64, tz_offset_minutes: i64) -> i64 {
    let local_ms = (unix_ms as i64) + tz_offset_minutes * 60_000;
    let day_ms = local_ms.rem_euclid(86_400_000);
    (day_ms / 60_000) as i64
}

/// getNextAdjustmentEvent(equiv L5655-L5687)。
pub fn get_next_adjustment_event(rt: &RuntimeHost, tz_offset: i64) -> Option<Value> {
    let current = get_current_adjustment_event(rt, tz_offset)?;
    let current_value = current.get("value").and_then(|v| v.as_i64()).unwrap_or(0);

    let settings = rt.settings_snapshot();
    let mut earliest: Option<Value> = None;
    let mut earliest_value: i64 = i64::MAX;
    let mut closest: Option<Value> = None;
    let mut closest_value: i64 = i64::MAX;

    let events = settings.get("adjustmentTimes").and_then(|v| v.as_array())?;
    for event in events {
        let event_time = if event.get("useSunCalc").and_then(|v| v.as_bool()).unwrap_or(false) {
            let name = event.get("sunCalc").and_then(|v| v.as_str()).unwrap_or("solarNoon").to_string();
            let offset = event.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
            get_sun_calc_time(&settings, &name, offset, tz_offset)
        } else {
            event.get("time").and_then(|v| v.as_str()).unwrap_or("0:0").to_string()
        };
        let event_value = parse_time(&event_time);

        // Check if event is later than current time, and less than the last found(equiv L5668)
        if event_value > current_value && event_value < closest_value {
            let mut e = event.clone();
            if let Some(obj) = e.as_object_mut() {
                obj.insert("value".to_string(), json!(event_value));
            }
            closest = Some(e);
            closest_value = event_value;
        }
        // Check if event is the earliest(equiv L5675)
        if event_value < earliest_value {
            let mut e = event.clone();
            if let Some(obj) = e.as_object_mut() {
                obj.insert("value".to_string(), json!(event_value));
            }
            earliest = Some(e);
            earliest_value = event_value;
        }
    }
    closest.or(earliest)
}

/// getCurrentAdjustmentEventLERP(equiv L5690-L5731):动画计划的插值。
/// 返回 Value::Number(全局亮度) 或 Value::Object(per-monitor 亮度)。
pub fn get_current_adjustment_event_lerp(rt: &RuntimeHost, tz_offset: i64) -> Option<Value> {
    let current = get_current_adjustment_event(rt, tz_offset)?;
    let next = get_next_adjustment_event(rt, tz_offset)?;

    let settings = rt.settings_snapshot();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let now_value = local_minute_of_day(now_ms, tz_offset);
    let mut next_value = next.get("value").and_then(|v| v.as_i64()).unwrap_or(0);
    let current_value = current.get("value").and_then(|v| v.as_i64()).unwrap_or(0);
    // Add 24hr if next event is tomorrow(equiv L5700-L5702)
    if current_value > next_value {
        next_value += 1440;
    }

    // Calculate 0-1 percentage of progress(equiv L5705-L5712)
    let lerp_next = next_value - current_value;
    let lerp_now = now_value - current_value;
    let progress = lerp_next - lerp_now;
    let end = lerp_next;
    if end == 0 {
        return None;
    }
    let percent = 1.0 - (progress as f64 / end as f64);

    // Generate result depending on if displays are linked(equiv L5715-L5726)
    if settings.get("adjustmentTimeIndividualDisplays").and_then(|v| v.as_bool()).unwrap_or(false) {
        let mut monitors = current.get("monitors").cloned().unwrap_or(json!({}));
        let next_mon = next.get("monitors").cloned().unwrap_or(json!({}));
        if let (Some(dst), Some(src)) = (monitors.as_object_mut(), next_mon.as_object()) {
            for (k, v) in src {
                let nv = v.as_i64().unwrap_or(-1);
                let cv = dst.get(k).and_then(|v| v.as_i64()).unwrap_or(-1);
                if cv > -1 && nv > -1 {
                    let lerped = lerp(cv as f64, nv as f64, percent as f64);
                    dst.insert(k.clone(), json!(lerped.round() as i64));
                }
            }
        }
        Some(monitors)
    } else {
        let cb = current.get("brightness").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let nb = next.get("brightness").and_then(|v| v.as_f64()).unwrap_or(0.0);
        Some(json!(lerp(cb, nb, percent as f64).round() as i64))
    }
}

/// Utils.lerp(equiv Utils.js:L269-L271)。
pub fn lerp(start: f64, finish: f64, perc: f64) -> f64 {
    start * (1.0 - perc) + finish * perc
}

/// applyCurrentAdjustmentEvent(equiv L5750-L5827)。
/// 返回 foundEvent(用于上层判断是否找到事件,等价原版返回值)。
pub fn apply_current_adjustment_event(rt: &RuntimeHost, force: bool, instant: bool, tz_offset: i64) -> Option<Value> {
    let temp = rt.temp.lock().unwrap().clone();
    let settings = rt.settings_snapshot();
    if temp.pause_time_adjustments {
        return None;
    }
    let has_times = settings
        .get("adjustmentTimes")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    if !has_times || rt.user_idle_dimmed.load(Ordering::SeqCst) {
        return None;
    }

    // Reset when the selected schedule can have changed(equiv L5757-L5762)
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let day = local_day_of_month(now_ms, tz_offset);
    {
        let last_guard = rt.last_time_event.lock().unwrap();
        let stale = match last_guard.as_ref() {
            Some(l) => l.get("day").and_then(|v| v.as_i64()) != Some(day as i64),
            None => false,
        };
        drop(last_guard);
        if force || stale {
            println!("New day (or forced)... resetting lastTimeEvent");
            *rt.last_time_event.lock().unwrap() = None;
        }
    }

    // Find most recent event(equiv L5764-L5765)
    let found_event = get_current_adjustment_event(rt, tz_offset)?;
    let found_value = found_event.get("value").and_then(|v| v.as_i64()).unwrap_or(0);
    let last_value = rt
        .last_time_event
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|l| l.get("value").and_then(|v| v.as_i64()))
        .unwrap_or(i64::MIN);
    let is_new_schedule_event = rt.last_time_event.lock().unwrap().is_none() || last_value < found_value;
    let animate = settings.get("adjustmentTimeAnimate").and_then(|v| v.as_bool()).unwrap_or(false);
    if is_new_schedule_event || animate {
        let mut found = found_event.clone();
        if animate {
            // If LERPing, override foundEvent with interpolated value(equiv L5770-L5778)
            if let Some(l) = get_current_adjustment_event_lerp(rt, tz_offset) {
                match l {
                    Value::Number(n) => found["brightness"] = Value::Number(n),
                    Value::Object(_) => found["monitors"] = l,
                    _ => {}
                }
            }
        }

        // lastTimeEvent = Object.assign({}, foundEvent); lastTimeEvent.day = ...(equiv L5781-L5782)
        if let Some(obj) = found.as_object_mut() {
            obj.insert("day".to_string(), json!(day as i64));
        }
        *rt.last_time_event.lock().unwrap() = Some(found.clone());

        let brightness = found.get("brightness").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let monitors_map = found.get("monitors").cloned().unwrap_or(json!({}));
        let speed = settings.get("adjustmentTimeSpeed").and_then(|v| v.as_str()).unwrap_or("instant").to_string();
        if animate {
            // 原版先 readKnownBrightness 再 applyAnimatedBrightness;等价:以当前亮度为起点步进
            apply_animated_brightness(rt, brightness, &monitors_map);
        } else if instant || speed == "instant" {
            transitionless_brightness(rt, brightness, &monitors_map);
        } else {
            transition_brightness(rt, brightness, &monitors_map, 1.0);
        }
        return Some(found);
    }
    None
}

/// applyAnimatedBrightness(equiv L3497+;动画模式 = 以当前亮度为起点的 transition)。
fn apply_animated_brightness(rt: &RuntimeHost, level: f64, event_monitors: &Value) {
    transition_brightness(rt, level, event_monitors, 1.0);
}

/// 当月的日(等价 new Date().getDate();civil_from_days 算法)。
fn local_day_of_month(unix_ms: u64, tz_offset_minutes: i64) -> u32 {
    let local_days = ((unix_ms as i64) + tz_offset_minutes * 60_000).div_euclid(86_400_000);
    let z = local_days + 719_468;
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    d as u32
}

/// handleBackgroundUpdate(equiv L5835-L5856):后台心跳的 TOD 应用。
/// checkForUpdates 部分由 updates 逻辑承担(更新检查见 settings_host)。
pub fn handle_background_update(rt: &RuntimeHost, force: bool, tz_offset: i64) {
    println!("Event: handleBackgroundUpdate");
    let settings = rt.settings_snapshot();
    let has_times = settings
        .get("adjustmentTimes")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    if has_times && !rt.user_idle_dimmed.load(Ordering::SeqCst) {
        apply_current_adjustment_event(rt, force, false, tz_offset);
    }
    // equiv L5850:if (!force) checkForUpdates()（forced update 只修亮度,不做更新检查）
    if !force {
        let update_path = std::env::var("TT_CONFIG_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join("update.exe");
        crate::update_host::check_for_updates(&settings, false, update_path);
    }
}

// ---------------------------------------------------------------------------
// Idle detection(equiv L5485-L5622)
// ---------------------------------------------------------------------------

/// getIdleSettingValue(equiv L5499-L5502)。
pub fn get_idle_setting_value(settings: &Value) -> u64 {
    let seconds = settings.get("detectIdleTimeSeconds").and_then(|v| v.as_i64()).unwrap_or(0);
    let minutes = settings.get("detectIdleTimeMinutes").and_then(|v| v.as_i64()).unwrap_or(0);
    ((seconds + minutes * 60).max(0)) as u64
}

/// idleCheckLong 的判定(等价 L5504-L5512;定时器由 system_events 持有)。
/// 返回 true 表示应启动 idleCheckShort。
pub fn idle_check_long(rt: &RuntimeHost, system_idle_seconds: u64) -> bool {
    if rt.temp.lock().unwrap().pause_idle_detection {
        return false;
    }
    let settings = rt.settings_snapshot();
    let threshold = if settings.get("detectIdleTimeEnabled").and_then(|v| v.as_bool()).unwrap_or(false) {
        get_idle_setting_value(&settings)
    } else {
        180
    };
    rt.last_idle_time.store(system_idle_seconds, Ordering::SeqCst);
    system_idle_seconds >= threshold
}

/// isFocusedWindowFullscreen(equiv L5523-L5533)。
pub fn is_focused_window_fullscreen(rt: &RuntimeHost) -> bool {
    let settings = rt.settings_snapshot();
    if !settings.get("detectIdleCheckFullscreen").and_then(|v| v.as_bool()).unwrap_or(false) {
        return false;
    }
    let hwnd = twutils::window_utils::get_foreground_window();
    twutils::window_utils::get_window_fullscreen(hwnd)
}

/// isMediaPlaying(equiv L5535-L5544)。
pub fn is_media_playing(rt: &RuntimeHost) -> bool {
    let settings = rt.settings_snapshot();
    if !settings.get("detectIdleMedia").and_then(|v| v.as_bool()).unwrap_or(false) {
        return false;
    }
    twutils::media_status::get_playback_status() == "playing"
}

/// idleCheckShort 的 dim 判定(等价 L5550):全部条件"与",删任一分支改变行为。
pub fn should_dim_now(rt: &RuntimeHost, system_idle_seconds: u64) -> bool {
    let settings = rt.settings_snapshot();
    settings.get("detectIdleTimeEnabled").and_then(|v| v.as_bool()).unwrap_or(false)
        && !settings.get("disableAutoApply").and_then(|v| v.as_bool()).unwrap_or(false)
        && system_idle_seconds >= get_idle_setting_value(&settings)
        && !is_focused_window_fullscreen(rt)
        && !is_media_playing(rt)
}

/// 用户进入 idle:dim 显示器(equiv L5550-L5572 的 dim 分支)。
pub fn dim_displays_for_idle(rt: &RuntimeHost) {
    println!("User idle. Dimming displays.");
    rt.user_idle_dimmed.store(true, Ordering::SeqCst);
    let settings = rt.settings_snapshot();
    let monitors = rt.host.monitors_snapshot();
    let transition_speed = settings.get("idleTransitionSpeed").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let mut transition_monitors = json!({});
    if let Some(obj) = monitors.as_object() {
        for (_k, monitor) in obj {
            if should_skip_display(rt, monitor, true) {
                continue;
            }
            let id = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if id.is_empty() {
                continue;
            }
            if transition_speed > 0.0 {
                transition_monitors[id] = json!(0);
            } else {
                update_brightness(rt, &id, 0.0, true, "brightness");
            }
        }
    }
    if transition_monitors.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
        transition_brightness(rt, 0.0, &transition_monitors, transition_speed);
    }
}

/// 用户退出 idle:恢复(equiv L5574-L5617 的 wake 分支)。
/// 返回 true 表示本次 tick 触发了唤醒恢复。
pub fn wake_from_idle(rt: &RuntimeHost, system_idle_seconds: u64) -> bool {
    if !rt.is_user_idle.load(Ordering::SeqCst) {
        return false;
    }
    let last = rt.last_idle_time.load(Ordering::SeqCst);
    let settings = rt.settings_snapshot();
    let threshold = get_idle_setting_value(&settings) as i64;
    if (system_idle_seconds as i64) < (last as i64) || (system_idle_seconds as i64) < threshold {
        println!("User no longer idle after {} seconds.", last);
        // Different behavior depending on if idle dimming is on(equiv L5581-L5590)
        if settings.get("detectIdleTimeEnabled").and_then(|v| v.as_bool()).unwrap_or(false) {
            set_known_brightness(rt, false, false, 1.0);
        } else {
            let tz = crate::system_events::timezone_offset_minutes();
            let found = apply_current_adjustment_event(rt, true, true, tz);
            if found.is_none() && !settings.get("disableAutoApply").and_then(|v| v.as_bool()).unwrap_or(false) {
                set_known_brightness(rt, false, false, 1.0);
            }
        }
        // Wait a little longer, re-apply known brightness(equiv L5595-L5615)
        let restore_seconds = settings.get("idleRestoreSeconds").and_then(|v| v.as_u64()).unwrap_or(4);
        *rt.idle_restore_deadline.lock().unwrap() = Some(Instant::now() + Duration::from_secs(restore_seconds));
        return true;
    }
    false
}

/// idle 恢复的延迟段(equiv setTimeout 回调):到期后重置状态并再次恢复。
pub fn idle_restore_tick(rt: &RuntimeHost) -> bool {
    let due = {
        let mut deadline = rt.idle_restore_deadline.lock().unwrap();
        match *deadline {
            Some(d) if Instant::now() >= d => {
                *deadline = None;
                true
            }
            _ => false,
        }
    };
    if !due {
        return false;
    }
    rt.is_user_idle.store(false, Ordering::SeqCst);
    rt.user_idle_dimmed.store(false, Ordering::SeqCst);
    rt.last_idle_time.store(1, Ordering::SeqCst);

    let settings = rt.settings_snapshot();
    let tz = crate::system_events::timezone_offset_minutes();
    if settings.get("detectIdleTimeEnabled").and_then(|v| v.as_bool()).unwrap_or(false) {
        // Always restore when dimmed, then check ToD(equiv L5603-L5606)
        set_known_brightness(rt, false, false, 1.0);
        apply_current_adjustment_event(rt, true, false, tz);
    } else {
        // Not dimmed, try checking ToD first. sKB as backup.(equiv L5607-L5611)
        let found = apply_current_adjustment_event(rt, true, true, tz);
        if found.is_none() && !settings.get("disableAutoApply").and_then(|v| v.as_bool()).unwrap_or(false) {
            set_known_brightness(rt, false, false, 1.0);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_time_matches_utils_js() {
        assert_eq!(parse_time("9:30"), 570);
        assert_eq!(parse_time("0:0"), 0);
        assert_eq!(parse_time("23:59"), 1439);
        assert_eq!(parse_time("7:05"), 425);
    }

    #[test]
    fn sun_calc_time_with_offset_carries() {
        let settings = json!({"adjustmentTimeLatitude": 39.9042, "adjustmentTimeLongitude": 116.4074});
        // 2026-11-23 的 solarNoon 东八区为 11:59(node 基线)
        let now_ms = 1_794_412_800_000u64 as f64; // 04:00Z = 本地 12:00(+480)
        let times = suncalc::get_times(now_ms, 39.9042, 116.4074, 0.0);
        let ms = times.get("solarNoon").unwrap();
        let (h, m) = suncalc::ms_to_local_hm(*ms, 480);
        assert_eq!((h, m), (11, 59));
        let _ = &settings;
    }

    #[test]
    fn min_max_clamps() {
        assert_eq!(min_max(120.0), 100.0);
        assert_eq!(min_max(-5.0), 0.0);
        assert_eq!(min_max(50.0), 50.0);
    }
}
