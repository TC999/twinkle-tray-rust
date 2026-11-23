//! 系统事件源层(equiv electron.js addEventListeners L5210-L5226 + 电源广播
//! L3901-L3977 + 定时器集合 idleMonitor/backgroundInterval + 热键注册 applyHotkeys)。
//!
//! 线程模型:托盘线程的窗口承载 WM_HOTKEY/WM_DISPLAYCHANGE/WM_POWERBROADCAST/
//! WM_INPUT 路由(事件经 TrayEvent channel 转发);主线程 Timer 消费事件并驱动
//! idle/TOD/transition 心跳(等价原版 setTimeout/setInterval 集合)。

use crate::hotkey_host::{self, HotkeyContext};
use crate::monitor_view;
use crate::runtime_hosts::{self, RuntimeHost};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// 原生亮度键重复（equiv electron.js L636-L639）。
const NATIVE_BRIGHTNESS_KEY_REPEAT_LIMIT: u64 = 10000; // Holding the key longer than this needs a fresh press
const NATIVE_BRIGHTNESS_KEY_REPEAT_INTERVAL: u64 = 120; // Kept clear of doHotkey's 100 ms throttle

/// analytics 周期（equiv L816：1000*60*29）。
const ANALYTICS_FREQUENCY_MS: u64 = 1000 * 60 * 29;
/// analytics 最小间隔（equiv L821：1000*60*28）。
const ANALYTICS_MIN_PING_MS: u64 = 1000 * 60 * 28;

/// 本地时区偏移(分钟,+480 = 东八区)。缓存一次(等价 Date 对象的隐式获取)。
pub fn timezone_offset_minutes() -> i64 {
    static CACHE: OnceLock<i64> = OnceLock::new();
    *CACHE.get_or_init(|| {
        // 等价 -new Date().getTimezoneOffset():用 GetTimeZoneInformation
        // SAFETY: 输出结构体为固定大小可写缓冲
        let mut tzi = windows::Win32::System::Time::TIME_ZONE_INFORMATION::default();
        let r = unsafe { windows::Win32::System::Time::GetTimeZoneInformation(&mut tzi) };
        let bias = tzi.Bias; // UTC = local + bias(分钟)
        // DST 期间(r == 2)用 DaylightBias(等价 getTimezoneOffset 的 DST 语义)
        let effective = match r {
            1 => bias + tzi.StandardBias,
            2 => bias + tzi.DaylightBias,
            _ => bias,
        };
        -effective as i64
    })
}

/// sleepDisplays 运行时入口(equiv L3533-L3554):ddcci 下发 0xD6,ps 用 PostMessage。
/// delay_ms 对应原版 setTimeout(delayMS)。
pub fn sleep_displays_runtime(rt: &Arc<RuntimeHost>, settings: &Value, delay_ms: u64) {
    let mode = settings.get("sleepAction").and_then(|v| v.as_str()).unwrap_or("none").to_string();
    if mode == "none" {
        return;
    }
    // 延迟语义由调用线程 sleep 等价(原版 setTimeout 单发)
    let _ = delay_ms;
    if mode == "ddcci" || mode == "ps_ddcci" {
        let monitors = rt.host.monitors_snapshot();
        if let Some(obj) = monitors.as_object() {
            for (_k, monitor) in obj {
                let hwid = monitor
                    .get("hwid")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join("#")
                    })
                    .unwrap_or_default();
                if hwid.is_empty() {
                    continue;
                }
                turn_off_display_ddc(rt, &hwid, false);
            }
        }
    }
    if mode == "ps" || mode == "ps_ddcci" {
        // PostMessage(0xFFFF,0x0112,0xF170,0x0002)(equiv L3546)
        // SAFETY: PostMessageW 只投递消息,HWND_BROADCAST=0xFFFF
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                Some(windows::Win32::Foundation::HWND(0xFFFF as _)),
                0x0112, // WM_SYSCOMMAND
                windows::Win32::Foundation::WPARAM(0xF170),
                windows::Win32::Foundation::LPARAM(2),
            );
        }
    }
}

/// turnOffDisplayDDC(equiv L3556-L3591):按 ddcPowerOffValue 组合下发。
pub fn turn_off_display_ddc(rt: &Arc<RuntimeHost>, hwid: &str, toggle: bool) -> bool {
    let settings = rt.settings_snapshot();
    let off_val = settings.get("ddcPowerOffValue").and_then(|v| v.as_i64()).unwrap_or(5);
    if toggle {
        let current = rt.host.get_vcp(hwid, 0xD6, 3000);
        let current = current
            .and_then(|v| v.as_array().map(|a| a.first().and_then(|x| x.as_f64()).unwrap_or(0.0)))
            .unwrap_or(0.0);
        if current > 1.0 {
            rt.host.send(json!({ "type": "vcp", "monitor": hwid, "code": 0xD6, "value": 1 }));
            return true;
        }
    }
    if off_val == 4 || off_val == 6 {
        rt.host.send(json!({ "type": "vcp", "monitor": hwid, "code": 0xD6, "value": 4 }));
    }
    if off_val == 5 || off_val == 6 {
        rt.host.send(json!({ "type": "vcp", "monitor": hwid, "code": 0xD6, "value": 5 }));
    }
    false
}

/// 请求 HUD 显示(hotkey overlay;实际 UI 呈现由 panel_controller 的 HUD 窗口)。
pub fn request_hud_show(rt: &Arc<RuntimeHost>) {
    // 等价 hotkeyOverlayStart:2s HUD。HUD 的具体窗口由 UI 层(Timer 轮询)拉起;
    // 这里置位标志。
    HUD_REQUEST.store(true, Ordering::SeqCst);
    let _ = rt;
}

pub static HUD_REQUEST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// HUD 请求消费(UI 层轮询;true = 本 tick 有请求)。
pub fn take_hud_request() -> bool {
    HUD_REQUEST.swap(false, Ordering::SeqCst)
}

/// applyHotkeys(equiv L1801-L1831):在托盘线程窗口上注册全部热键。
/// 返回 (注册表, 成功数)。unregisterAll 语义 = 先注销全部再逐个注册。
/// 原生亮度键 accelerator 表（equiv nativeBrightnessAccelerators L627-L630）。
pub const NATIVE_BRIGHTNESS_ACCELERATORS: [&str; 2] = ["BrightnessUp", "BrightnessDown"];

/// applyHotkeys(equiv L1801-L1831):在托盘线程窗口上注册全部热键。
/// BrightnessUp/Down 不注册为全局热键——由 Raw Input 设备捕获(WM_INPUT)触发,
/// active 取决于 sync_native_brightness_keys 的注册结果(equiv L1811-L1815)。
pub fn apply_hotkeys(settings: &Value, tray_hwnd: windows::Win32::Foundation::HWND) -> Vec<hotkey_host::RegisteredHotkey> {
    let parsed = hotkey_host::parse_hotkeys_with_id(settings);
    let mut registered = Vec::new();
    let mut claimed_native: Vec<String> = Vec::new();
    for (idx, hotkey) in parsed.iter().enumerate() {
        let id_val = (10000 + idx) as i32; // 注册 id 与 accelerator 反查表对齐
        if NATIVE_BRIGHTNESS_ACCELERATORS.contains(&hotkey.accelerator.as_str()) {
            // 原生亮度键:不注册 RegisterHotKeyW(equiv L1812-L1814),
            // 仅由 WM_INPUT 报告触发;active 判定在 trigger 侧按是否存在完成
            if !claimed_native.contains(&hotkey.accelerator) {
                claimed_native.push(hotkey.accelerator.clone());
                registered.push(hotkey.clone());
            }
            continue;
        }
        if let Some((modifiers, vk)) = hotkey_host::accelerator_to_hotkey(&hotkey.accelerator) {
            // 等价 globalShortcut.register 的 try/catch:失败即跳过(hotkey.active=false)
            let ok = tray::register_hotkey(Some(tray_hwnd), id_val, modifiers, vk);
            if ok {
                registered.push(hotkey.clone());
            } else {
                println!("Couldn't register hotkey: {}", hotkey.accelerator);
            }
        }
    }
    println!("Hotkeys registered: {}/{}", registered.len(), parsed.len());
    registered
}

/// syncNativeBrightnessKeys(equiv L694-L714):按是否配置了 BrightnessUp/Down 热键
/// 注册/注销 Raw Input 设备。wanted 变化时重注册。
pub fn sync_native_brightness_keys(settings: &Value, tray_hwnd: windows::Win32::Foundation::HWND) -> bool {
    let wanted = hotkey_host::parse_hotkeys(settings)
        .iter()
        .any(|(accel, _)| NATIVE_BRIGHTNESS_ACCELERATORS.contains(&accel.as_str()));
    if wanted {
        twutils::brightness_keys::register(tray_hwnd)
    } else {
        twutils::brightness_keys::unregister();
        false
    }
}

/// 注销全部热键(equiv globalShortcut.unregisterAll)。
pub fn unregister_all_hotkeys(tray_hwnd: windows::Win32::Foundation::HWND, count: usize) {
    for idx in 0..count {
        tray::unregister_hotkey(Some(tray_hwnd), 10000 + idx as i32);
    }
}

/// 注册 id -> 热键 反查(托盘线程 WM_HOTKEY 的 wparam)。
pub fn hotkey_by_reg_id(registrations: &[hotkey_host::RegisteredHotkey], reg_id: i32) -> Option<&hotkey_host::RegisteredHotkey> {
    let idx = (reg_id - 10000) as usize;
    registrations.get(idx)
}

/// handleNativeBrightnessKey(equiv L666-L689):WM_INPUT 原生亮度键。
/// "up"/"down" 触发 triggerNativeBrightnessHotkey 并启动长按重复;"release" 停止。
/// 重复状态机由 tick 的 drive_native_brightness_key_repeat 驱动(主线程)。
impl SystemEvents {
    fn handle_native_brightness_key(&mut self, ctx: &HotkeyContext, key: &str) {
        // equiv L667-L669:release 停止全部重复定时器
        if key == "release" {
            self.stop_native_brightness_key_repeat();
            return;
        }
        // equiv L671:非 up/down("other"/"")忽略
        if key != "up" && key != "down" {
            return;
        }
        // equiv L672:同键重复报告忽略(按下状态未释放)
        if self.native_key.as_deref() == Some(key) {
            return;
        }
        // equiv L674:更换按键先停止旧重复
        self.stop_native_brightness_key_repeat();
        self.native_key = Some(key.to_string());
        let accelerator = if key == "up" { "BrightnessUp" } else { "BrightnessDown" };
        // equiv L676-L679:触发失败则放弃按下状态
        if !trigger_native_brightness_hotkey(ctx, &self.registrations, accelerator) {
            self.native_key = None;
            return;
        }
        // equiv L680-L688:release 报告可能丢失(蓝牙键盘休眠/断连),
        // 重复在自身超时后停止。
        self.native_repeat_until = Some(std::time::Instant::now() + Duration::from_millis(NATIVE_BRIGHTNESS_KEY_REPEAT_LIMIT));
        self.native_repeat_delay_due = Some(std::time::Instant::now() + Duration::from_millis(400));
    }

    /// stopNativeBrightnessKeyRepeat(equiv L641-L647)。
    fn stop_native_brightness_key_repeat(&mut self) {
        self.native_key = None;
        self.native_repeat_delay_due = None;
        self.native_repeat_interval_due = None;
        self.native_repeat_until = None;
    }

    /// 长按重复驱动(tick 调用;equiv L683-L688 的 setTimeout/setInterval)。
    fn drive_native_brightness_key_repeat(&mut self, ctx: &HotkeyContext) {
        // 400ms 延迟到期 -> 启动 120ms 周期
        if let Some(due) = self.native_repeat_delay_due {
            if std::time::Instant::now() >= due {
                self.native_repeat_delay_due = None;
                self.native_repeat_interval_due =
                    Some(std::time::Instant::now() + Duration::from_millis(NATIVE_BRIGHTNESS_KEY_REPEAT_INTERVAL));
            }
        }
        let Some(due) = self.native_repeat_interval_due else { return };
        if std::time::Instant::now() < due {
            return;
        }
        // equiv L685:超过 10000ms 上限则停止
        if let Some(until) = self.native_repeat_until {
            if std::time::Instant::now() > until {
                self.stop_native_brightness_key_repeat();
                return;
            }
        }
        // equiv L686:仍按住则再次触发
        if let Some(key) = self.native_key.clone() {
            let accelerator = if key == "up" { "BrightnessUp" } else { "BrightnessDown" };
            trigger_native_brightness_hotkey(ctx, &self.registrations, accelerator);
            self.native_repeat_interval_due =
                Some(std::time::Instant::now() + Duration::from_millis(NATIVE_BRIGHTNESS_KEY_REPEAT_INTERVAL));
        }
    }
}

/// triggerNativeBrightnessHotkey(equiv L649-L664)。
/// 返回是否找到并触发了热键(原版 return true/false)。
fn trigger_native_brightness_hotkey(ctx: &HotkeyContext, registrations: &[hotkey_host::RegisteredHotkey], accelerator: &str) -> bool {
    let rt = &ctx.rt;
    let settings = rt.settings_snapshot();
    let hotkey = registrations.iter().find(|h| h.accelerator == accelerator);
    let Some(hotkey) = hotkey else { return false };
    // Windows already applies dedicated brightness keys...(原注释,skip offset)
    hotkey_host::do_hotkey(ctx, hotkey, true);
    let _ = settings;
    true
}

/// 电源设置事件的 GUID 名映射(equiv twutils get_power_setting_name 的白名单)
/// 由 tray 层转发 (name, guid, data);此处只处理 electron.js L3912-L3961 分支。
pub fn handle_power_setting(rt: &Arc<RuntimeHost>, name: &str, guid: &str, data: u32) {
    let settings = rt.settings_snapshot();
    if name == "GUID_SESSION_USER_PRESENCE" {
        if !settings.get("useGuidPresenceEvent").and_then(|v| v.as_bool()).unwrap_or(false) {
            return;
        }
        if data == 2 {
            // Idle: displays going to sleep(equiv L3914-L3924)
            if !rt.is_windows_user_idle.load(Ordering::SeqCst) {
                println!("Displays have gone to sleep.");
            }
            rt.is_windows_user_idle.store(true, Ordering::SeqCst);
        } else if data == 0 {
            // Active(equiv L3925-L3934)
            if rt.is_windows_user_idle.load(Ordering::SeqCst) {
                rt.is_windows_user_idle.store(false, Ordering::SeqCst);
                println!("Displays have woken up.");
                rt.recently_woke_up.store(true, Ordering::SeqCst);
                handle_metrics_change(rt, "GUID_SESSION_USER_PRESENCE");
            }
        }
    } else if name == "GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS" {
        // Internal display brightness change(equiv L3939-L3961)
        if !settings.get("useGuidBrightnessEvent").and_then(|v| v.as_bool()).unwrap_or(false) {
            return;
        }
        // ignoreBrightnessEvent 在 Rust 侧等价于"面板正在拖动"(paused_monitor_updates)
        if !rt.paused_monitor_updates.load(Ordering::SeqCst) {
            let monitors_snapshot = rt.host.monitors_snapshot();
            let mut monitors = monitors_snapshot.clone();
            if let Some(obj) = monitors.as_object_mut() {
                for (_k, monitor) in obj.iter_mut() {
                    let mtype = monitor.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let uses_gamma = monitor_view::uses_gamma_slider(monitor, &settings);
                    if mtype == "wmi" && !uses_gamma {
                        let normalized = monitor_view::normalize_brightness(
                            data as f64,
                            true,
                            monitor.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0),
                            monitor.get("max").and_then(|v| v.as_f64()).unwrap_or(100.0),
                            &monitor_view::calibration_of(monitor),
                        );
                        monitor["brightness"] = json!(normalized);
                        monitor["brightnessRaw"] = json!(data);
                    }
                }
            }
            *rt.host.monitors.lock().unwrap() = monitors;
            set_tray_percent(rt);
        }
    } else if name.is_empty() && !guid.is_empty() {
        println!("Event: {} ({})", guid, data);
    }
}

/// setTrayPercent(equiv L4700-L4719):托盘 tooltip 的平均亮度。
pub fn set_tray_percent(rt: &Arc<RuntimeHost>) {
    let settings = rt.settings_snapshot();
    let monitors = rt.host.monitors_snapshot();
    let mut average_perc = 0.0;
    let mut i = 0u32;
    if let Some(obj) = monitors.as_object() {
        for (_k, monitor) in obj {
            let mtype = monitor.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if mtype == "ddcci" || mtype == "wmi" || mtype == "software" || monitor_view::uses_gamma_slider(monitor, &settings) {
                i += 1;
                average_perc += monitor.get("brightness").and_then(|v| v.as_f64()).unwrap_or(0.0);
            }
        }
    }
    if i > 0 {
        average_perc = (average_perc / i as f64).round();
        let tooltip = format!("Twinkle Tray ({}%)", average_perc as i64);
        TRAY_TOOLTIP.lock().unwrap().replace(tooltip);
    }
}

/// 托盘 tooltip 更新请求(UI 层/托盘线程消费)。
pub static TRAY_TOOLTIP: Mutex<Option<String>> = Mutex::new(None);

/// handleMonitorChange(equiv L5250-L5301):硬件变化(延迟 hardwareRestoreSeconds)。
/// 返回延迟毫秒(0 = 跳过),由 UI 层定时器执行 deferred 段。
pub fn handle_monitor_change(rt: &Arc<RuntimeHost>, tag: &str) -> Option<u64> {
    println!("Event: handleMonitorChange ({})", tag);
    // Skip event that happens at startup(equiv L5254-L5257)
    if !rt.skipped_first_monitor_change.swap(true, Ordering::SeqCst) {
        return None;
    }
    if rt.resume_recovery_in_progress.load(Ordering::SeqCst) {
        println!("Resume recovery is already handling {}.", tag);
        return None;
    }
    println!("Hardware change detected.");
    let settings = rt.settings_snapshot();
    let seconds = settings.get("hardwareRestoreSeconds").and_then(|v| v.as_u64()).unwrap_or(5);
    Some(seconds * 1000)
}

/// handleMonitorChange 的延迟段(equiv L5272-L5295 的 setTimeout 回调)。
pub fn monitor_change_deferred(rt: &Arc<RuntimeHost>) {
    let settings = rt.settings_snapshot();
    // recreateTray 设置在 Rust 侧无对应窗口重建需求(托盘常驻),等价保留分支注释
    rt.host.refresh_monitors(true, false);
    if !settings.get("disableAutoApply").and_then(|v| v.as_bool()).unwrap_or(false) {
        if rt.startup_grace_period.load(Ordering::SeqCst) {
            runtime_hosts::set_known_brightness(rt, true, false, 1.0);
        } else {
            runtime_hosts::set_known_brightness(rt, false, false, 1.0);
        }
    }
    runtime_hosts::handle_background_update(rt, true, timezone_offset_minutes());
    rt.startup_grace_period.store(false, Ordering::SeqCst);
}

/// handleMetricsChange(equiv L5397-L5436):显示度量变化(延迟 idleRestoreSeconds)。
pub fn handle_metrics_change(rt: &Arc<RuntimeHost>, tag: &str) -> Option<u64> {
    println!("Event: handleMetricsChange ({})", tag);
    if rt.resume_recovery_in_progress.load(Ordering::SeqCst) {
        println!("Resume recovery is already handling {}.", tag);
        return None;
    }
    let settings = rt.settings_snapshot();
    let seconds = settings.get("idleRestoreSeconds").and_then(|v| v.as_u64()).unwrap_or(7);
    Some(seconds * 1000)
}

/// handleMetricsChange 的延迟段(equiv L5411-L5431)。
pub fn metrics_change_deferred(rt: &Arc<RuntimeHost>) {
    let settings = rt.settings_snapshot();
    rt.host.refresh_monitors(true, false);
    if !settings.get("disableAutoApply").and_then(|v| v.as_bool()).unwrap_or(false)
        && !rt.has_recently_interacted.load(Ordering::SeqCst)
    {
        if rt.startup_grace_period.load(Ordering::SeqCst) {
            runtime_hosts::set_known_brightness(rt, true, false, 1.0);
        } else {
            runtime_hosts::set_known_brightness(rt, false, false, 1.0);
        }
    }
    runtime_hosts::handle_background_update(rt, true, timezone_offset_minutes());
    rt.startup_grace_period.store(false, Ordering::SeqCst);
}

/// 事件源聚合器:持有托盘事件接收端 + 热键注册表 + 延迟任务。
pub struct SystemEvents {
    pub tray_rx: Option<std::sync::mpsc::Receiver<tray::TrayEvent>>,
    pub registrations: Vec<hotkey_host::RegisteredHotkey>,
    pub tray_hwnd: Option<windows::Win32::Foundation::HWND>,
    /// monitorChange 延迟任务(equiv handleChangeTimeout2)
    pub monitor_change_due: Option<std::time::Instant>,
    /// metricsChange 延迟任务(equiv handleChangeTimeout1)
    pub metrics_change_due: Option<std::time::Instant>,
    /// idleCheckLong 定时(5s)
    pub idle_long_due: std::time::Instant,
    /// idleCheckShort 定时(1s)——激活后运转
    pub idle_short_active: bool,
    pub idle_short_due: std::time::Instant,
    /// backgroundInterval(60s / dev 8s)
    pub background_due: std::time::Instant,
    /// 原生亮度键当前按下的键（"up"/"down"/None;equiv nativeBrightnessKey L633）
    pub native_key: Option<String>,
    /// 400ms 重复启动延迟到期（equiv nativeBrightnessKeyRepeatDelay L634）
    pub native_repeat_delay_due: Option<std::time::Instant>,
    /// 120ms 周期重复的下次触发（equiv nativeBrightnessKeyRepeat L635）
    pub native_repeat_interval_due: Option<std::time::Instant>,
    /// 长按上限到期（equiv repeatUntil L682）
    pub native_repeat_until: Option<std::time::Instant>,
    /// Raw Input 注册状态（equiv nativeBrightnessKeysRegistered L631）
    pub native_brightness_registered: bool,
    /// 是否需要原生亮度键（equiv nativeBrightnessKeysWanted L694）
    pub native_brightness_wanted: bool,
    /// analytics 周期定时（equiv analyticsInterval L815;29 分钟）
    pub analytics_due: Option<std::time::Instant>,
    /// 上次 analytics ping（equiv lastAnalyticsPing L817）
    pub last_analytics_ping: Option<std::time::Instant>,
    /// analytics 开关上次状态（检测设置变化时重启周期;equiv processSettings L1429-L1440）
    pub analytics_enabled: bool,
}


/// UI 层转发的系统事件队列(mpsc 单发约束下的路由层):
/// 托盘 channel 由 panel_controller 持有,系统类事件经此转交。
pub static FORWARDED_EVENTS: OnceLock<Mutex<Vec<tray::TrayEvent>>> = OnceLock::new();

/// SystemEvents 的共享节流器(hotkey 动作与 UI tick 共用;equiv updateBrightnessThrottle 的
/// 单例语义)。SystemEvents 不是 Send,但节流器是纯数据,可以静态共享。
pub static SHARED_THROTTLE: OnceLock<Mutex<crate::panel_controller::BrightnessThrottle>> = OnceLock::new();

pub fn shared_throttle() -> &'static Mutex<crate::panel_controller::BrightnessThrottle> {
    SHARED_THROTTLE.get_or_init(|| Mutex::new(crate::panel_controller::BrightnessThrottle::new()))
}

/// 在共享节流锁上执行操作（等价原版 updateBrightnessThrottle 的模块级单例访问）。
pub fn with_shared_throttle<R>(f: impl FnOnce(&mut crate::panel_controller::BrightnessThrottle) -> R) -> Option<R> {
    let mut guard = shared_throttle().lock().ok()?;
    Some(f(&mut guard))
}

fn forwarded_slot() -> &'static Mutex<Vec<tray::TrayEvent>> {
    FORWARDED_EVENTS.get_or_init(|| Mutex::new(Vec::new()))
}

/// panel_controller 的 handle_tray_event 把系统类事件转发进来。
pub fn forward_event(event: tray::TrayEvent) {
    if let Ok(mut q) = forwarded_slot().lock() {
        q.push(event);
    }
}

/// HotkeyContext 的节流器引用('static 单例)。
pub fn throttle_handle() -> &'static Mutex<crate::panel_controller::BrightnessThrottle> {
    shared_throttle()
}

impl SystemEvents {
    pub fn new(tray_rx: Option<std::sync::mpsc::Receiver<tray::TrayEvent>>) -> Self {
        Self {
            tray_rx,
            registrations: Vec::new(),
            tray_hwnd: None,
            monitor_change_due: None,
            metrics_change_due: None,
            idle_long_due: std::time::Instant::now() + Duration::from_secs(5),
            idle_short_active: false,
            idle_short_due: std::time::Instant::now(),
            background_due: std::time::Instant::now() + Duration::from_secs(60),
            native_key: None,
            native_repeat_delay_due: None,
            native_repeat_interval_due: None,
            native_repeat_until: None,
            native_brightness_registered: false,
            native_brightness_wanted: false,
            analytics_due: None,
            last_analytics_ping: None,
            analytics_enabled: true,
        }
    }

    /// 应用热键注册(托盘 hwnd 可用后调用;equiv applyHotkeys)。
    pub fn apply_hotkeys_now(&mut self, settings: &Value) {
        if let Some(hwnd) = self.tray_hwnd {
            // equiv L1803:applyHotkeys 先 syncNativeBrightnessKeys
            self.native_brightness_registered = sync_native_brightness_keys(settings, hwnd);
            self.registrations = apply_hotkeys(settings, hwnd);
        }
    }

    /// syncAnalytics（equiv L1429-L1440：settings.analytics 开关控制周期定时）。
    pub fn sync_analytics(&mut self, settings: &Value) {
        let enabled = settings.get("analytics").and_then(|v| v.as_bool()).unwrap_or(true);
        if enabled {
            // 立即 ping（等价 L1430 的 pingAnalytics()，内含 28 分钟最小间隔判断）
            self.ping_analytics_now(settings);
            self.analytics_due = Some(std::time::Instant::now() + Duration::from_millis(ANALYTICS_FREQUENCY_MS));
        } else {
            self.analytics_due = None;
            self.last_analytics_ping = None;
        }
    }

    /// pingAnalytics（equiv L819-L841）。28 分钟最小间隔判断在方法内。
    fn ping_analytics_now(&mut self, settings: &Value) {
        let now = std::time::Instant::now();
        if let Some(last) = self.last_analytics_ping {
            if (now.duration_since(last).as_millis() as u64) < ANALYTICS_MIN_PING_MS {
                return;
            }
        }
        let uuid = settings.get("uuid").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if uuid.is_empty() {
            return;
        }
        self.last_analytics_ping = Some(now);
        let os = crate::net::os_release();
        // 网络在独立线程（不阻塞 UI 主线程;等价 fetch 异步）
        std::thread::spawn(move || {
            crate::net::ping_analytics(
                &uuid,
                crate::app::APP_VERSION,
                crate::app::APP_BUILD,
                &os,
            );
        });
    }

    /// 排空转发队列。
    fn drain_forwarded(&self) -> Vec<tray::TrayEvent> {
        if let Ok(mut q) = forwarded_slot().lock() {
            std::mem::take(&mut *q)
        } else {
            Vec::new()
        }
    }

    /// 事件泵 tick(主线程调用):消费转发事件 + 驱动定时心跳(equiv addEventListeners
    /// 的事件回调集合 + idleMonitor/backgroundInterval 定时器)。
    pub fn tick(&mut self, rt: &Arc<RuntimeHost>) {
        let ctx = HotkeyContext { rt: rt.clone(), throttle: throttle_handle() };

        // ---- 托盘转发的系统事件(equiv hookWindowMessage 系列)----
        for event in self.drain_forwarded() {
                match event {
                    tray::TrayEvent::Click | tray::TrayEvent::RightClick | tray::TrayEvent::Menu(_) => {
                        // UI 事件由 panel_controller 处理,不会出现在转发队列
                    }
                    tray::TrayEvent::Hotkey(reg_id) => {
                        if let Some(hotkey) = hotkey_by_reg_id(&self.registrations, reg_id) {
                            hotkey_host::do_hotkey(&ctx, hotkey, false);
                        }
                    }
                    tray::TrayEvent::NativeBrightnessKey(key) => {
                        self.handle_native_brightness_key(&ctx, &key);
                    }
                    tray::TrayEvent::WmDisplayChange => {
                        // equiv L3888-L3890:useWmDisplayChangeEvent 开关
                        let settings = rt.settings_snapshot();
                        if settings.get("useWmDisplayChangeEvent").and_then(|v| v.as_bool()).unwrap_or(true)
                            && !settings.get("disablePowerNotifications").and_then(|v| v.as_bool()).unwrap_or(false)
                        {
                            if let Some(delay) = handle_metrics_change(rt, "wm_displaychange") {
                                self.metrics_change_due = Some(std::time::Instant::now() + Duration::from_millis(delay));
                            }
                        }
                    }
                    tray::TrayEvent::PowerSetting(name, guid, data) => {
                        if !rt.settings_snapshot().get("disablePowerNotifications").and_then(|v| v.as_bool()).unwrap_or(false) {
                            if !name.is_empty() || !guid.is_empty() {
                                println!("Event: {} ({})", if name.is_empty() { &guid } else { &name }, data);
                            }
                            handle_power_setting(rt, &name, &guid, data);
                        }
                    }
                    tray::TrayEvent::PowerBroadcast(wparam) => {
                        // equiv L5319-L5360 powerMonitor.on("resume")：
                        // PBT_APMRESUMEAUTOMATIC(0x12)/PBT_APMRESUMESUSPENDED(0x7) 触发恢复链
                        const PBT_APMRESUMEAUTOMATIC: u32 = 0x12;
                        const PBT_APMRESUMESUSPENDED: u32 = 0x7;
                        if wparam == PBT_APMRESUMEAUTOMATIC || wparam == PBT_APMRESUMESUSPENDED {
                            println!("Resuming......");
                            rt.recently_woke_up.store(true, Ordering::SeqCst);
                            rt.resume_recovery_in_progress.store(true, Ordering::SeqCst);
                            rt.resume_recovery_handled.store(false, Ordering::SeqCst);
                            let settings = rt.settings_snapshot();
                            // restartOnWake：整个 app 重启（equiv L5339-L5345）
                            if settings.get("restartOnWake").and_then(|v| v.as_bool()).unwrap_or(false) {
                                println!("restartOnWake: relaunching app.");
                                let exe = std::env::current_exe().unwrap_or_default();
                                let _ = std::process::Command::new(exe).spawn();
                                // 等价 app.exit()：退出事件循环（托盘线程随后清理）
                                std::process::exit(0);
                            }
                            // 等价 L5347-L5360：停 worker → 带 allowWhileWindowsIdle 重启
                            rt.host.manager.set_quitting(true);
                            rt.host.manager.stop();
                            rt.host.start(true);
                            // 恢复恢复链完成（equiv L5362+ 的恢复逻辑由 refresh 链接管）
                            rt.resume_recovery_in_progress.store(false, Ordering::SeqCst);
                            rt.resume_recovery_handled.store(true, Ordering::SeqCst);
                            // equiv L5462：unlock-screen 的 disableAutoRefresh 条件
                            if !settings.get("disableAutoRefresh").and_then(|v| v.as_bool()).unwrap_or(false) {
                                handle_metrics_change(rt, "resume");
                            }
                        }
                    }
                    tray::TrayEvent::ScMonitorPower => {
                        // equiv L3964-L3975:useScMonitorPowerEvent 开关(原版只打日志)
                        let settings = rt.settings_snapshot();
                        if settings.get("useScMonitorPowerEvent").and_then(|v| v.as_bool()).unwrap_or(false)
                            && !settings.get("disablePowerNotifications").and_then(|v| v.as_bool()).unwrap_or(false)
                        {
                            println!("Event: SC_MONITORPOWER");
                        }
                    }
                } // match
        } // for

        // ---- 显示变化监听(displayconfig::monitor::add_display_change_listener;useWin32Event)----
        let display_events = drain_display_events();
        if display_events > 0 {
            let settings = rt.settings_snapshot();
            if settings.get("useWin32Event").and_then(|v| v.as_bool()).unwrap_or(true) {
                if let Some(delay) = handle_monitor_change(rt, "win32") {
                    self.monitor_change_due = Some(std::time::Instant::now() + Duration::from_millis(delay));
                }
            }
        }

        // ---- idleCheckLong(5s 周期,equiv idleMonitor setInterval(idleCheckLong, 5000))----
        if std::time::Instant::now() >= self.idle_long_due {
            self.idle_long_due = std::time::Instant::now() + Duration::from_secs(5);
            let idle_secs = get_system_idle_time();
            if runtime_hosts::idle_check_long(rt, idle_secs) && !self.idle_short_active {
                // startIdleCheckShort(equiv L5514-L5521)
                rt.is_user_idle.store(true, Ordering::SeqCst);
                runtime_hosts::update_known_displays(rt, true, true);
                *rt.pre_idle_monitors.lock().unwrap() = rt.last_known_displays.lock().unwrap().clone().unwrap_or(json!({}));
                println!("Started short idle monitor.");
                self.idle_short_active = true;
                self.idle_short_due = std::time::Instant::now() + Duration::from_secs(1);
            }
        }

        // ---- idleCheckShort(1s 周期;激活后运转)----
        if self.idle_short_active && std::time::Instant::now() >= self.idle_short_due {
            self.idle_short_due = std::time::Instant::now() + Duration::from_secs(1);
            let idle_secs = get_system_idle_time();
            if !rt.user_idle_dimmed.load(Ordering::SeqCst) && runtime_hosts::should_dim_now(rt, idle_secs) {
                runtime_hosts::dim_displays_for_idle(rt);
            }
            runtime_hosts::wake_from_idle(rt, idle_secs);
        }

        // ---- idle 恢复延迟段(equiv setTimeout idleRestore)----
        runtime_hosts::idle_restore_tick(rt);

        // ---- pauseMonitorUpdates 到期复位 ----
        rt.pause_expire_tick();

        // ---- transition 步进(equiv transitionBrightness 的 setInterval;50ms 粒度轮询)----
        // advance_transition 内部不计时,但 interval 语义由外部节流等价
        // (原版 setInterval(updateInterval*mult);此处每 tick 检查是否到期)
        if TRANSITION_NEXT_DUE.load(Ordering::SeqCst) {
            let active = runtime_hosts::advance_transition(rt);
            if active {
                // 按下次到期时间排程
                let settings = rt.settings_snapshot();
                let interval = settings.get("updateInterval").and_then(|v| v.as_u64()).unwrap_or(500);
                schedule_transition_next(interval);
            } else {
                TRANSITION_NEXT_DUE.store(false, Ordering::SeqCst);
            }
        }

        // ---- monitorChange / metricsChange 延迟段 ----
        if let Some(due) = self.monitor_change_due {
            if std::time::Instant::now() >= due {
                self.monitor_change_due = None;
                monitor_change_deferred(rt);
            }
        }
        if let Some(due) = self.metrics_change_due {
            if std::time::Instant::now() >= due {
                self.metrics_change_due = None;
                // equiv L5414:handleMonitorChange 将运行时 metrics 不做任何事
                if self.monitor_change_due.is_none() {
                    metrics_change_deferred(rt);
                }
            }
        }

        // ---- backgroundInterval(60s;dev 8s;equiv L5474)----
        if std::time::Instant::now() >= self.background_due {
            let is_dev = false;
            let period = if is_dev { 8 } else { 60 };
            self.background_due = std::time::Instant::now() + Duration::from_secs(period);
            runtime_hosts::handle_background_update(rt, false, timezone_offset_minutes());
        }

        // ---- analytics(equiv L1429-L1440:开关变化重启周期 + L1434:29min setInterval) ----
        let settings = rt.settings_snapshot();
        let analytics_on = settings.get("analytics").and_then(|v| v.as_bool()).unwrap_or(true);
        if analytics_on != self.analytics_enabled {
            // 开关变化:重启 interval（等价 clearInterval + 立即 ping + 新 interval）
            self.analytics_enabled = analytics_on;
            if analytics_on {
                self.ping_analytics_now(&settings);
                self.analytics_due = Some(std::time::Instant::now() + Duration::from_millis(ANALYTICS_FREQUENCY_MS));
            } else {
                self.analytics_due = None;
                self.last_analytics_ping = None;
            }
        }
        if let Some(due) = self.analytics_due {
            if std::time::Instant::now() >= due {
                self.ping_analytics_now(&settings);
                self.analytics_due = Some(std::time::Instant::now() + Duration::from_millis(ANALYTICS_FREQUENCY_MS));
            }
        }

        // ---- 原生亮度键长按重复驱动(equiv L683-L688 的定时器) ----
        self.drive_native_brightness_key_repeat(&ctx);
    }
}

/// transition 排程标志(equiv currentTransition 的 setInterval 到期)。
pub static TRANSITION_NEXT_DUE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static TRANSITION_DUE_AT: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// 启动过渡的排程(transition_brightness 之后调用)。
pub fn schedule_transition_next(interval_ms: u64) {
    *TRANSITION_DUE_AT.lock().unwrap() = Some(std::time::Instant::now() + Duration::from_millis(interval_ms));
    TRANSITION_NEXT_DUE.store(false, Ordering::SeqCst);
}

/// 过渡到期检查(tick 前置):到期时置 TRANSITION_NEXT_DUE。
pub fn transition_due_check() {
    let due = TRANSITION_DUE_AT.lock().unwrap().clone();
    if let Some(d) = due {
        if std::time::Instant::now() >= d {
            TRANSITION_NEXT_DUE.store(true, Ordering::SeqCst);
        }
    }
}

/// 系统空闲秒数(equiv powerMonitor.getSystemIdleTime):GetLastInputInfo。
pub fn get_system_idle_time() -> u64 {
    // SAFETY: LASTINPUTINFO 为固定结构体,cbSize 按文档初始化
    unsafe {
        let mut info = windows::Win32::UI::Input::KeyboardAndMouse::LASTINPUTINFO {
            cbSize: std::mem::size_of::<windows::Win32::UI::Input::KeyboardAndMouse::LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        if windows::Win32::UI::Input::KeyboardAndMouse::GetLastInputInfo(&mut info).as_bool() {
            let now = windows::Win32::System::SystemInformation::GetTickCount();
            (now.saturating_sub(info.dwTime)) as u64 / 1000
        } else {
            0
        }
    }
}

/// 显示变化事件转发(displayconfig::add_display_change_listener 的接收端)。
/// Receiver 非 Sync,以 Mutex 包一层(tick 只在主线程消费)。
pub static DISPLAY_EVENTS: Mutex<Option<std::sync::mpsc::Receiver<displayconfig::DisplayChangeEvent>>> = Mutex::new(None);

/// 安装显示变化监听(equiv addDisplayChangeListener L5215)。重复调用幂等。
pub fn install_display_listener() {
    let mut guard = match DISPLAY_EVENTS.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if guard.is_some() {
        return;
    }
    let (_id, rx) = displayconfig::add_display_change_listener();
    *guard = Some(rx);
}

/// 排空显示变化事件(主线程 tick)。
fn drain_display_events() -> usize {
    let count = match DISPLAY_EVENTS.lock() {
        Ok(g) => match g.as_ref() {
            Some(rx) => {
                let mut n = 0;
                while rx.try_recv().is_ok() {
                    n += 1;
                }
                n
            }
            None => 0,
        },
        Err(_) => 0,
    };
    count
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_hosts::RuntimeHost;
    use crate::monitor_host::MonitorHost;
    use std::sync::Arc;

    fn test_ctx(rt: Arc<RuntimeHost>) -> HotkeyContext {
        HotkeyContext { rt, throttle: crate::system_events::shared_throttle() }
    }

    #[test]
    fn native_key_release_clears_state() {
        // 未找到热键时 trigger 返回 false,不启动重复(等价 L676-L679)
        let mut sys = SystemEvents::new(None);
        let rt = runtime_host();
        let ctx = test_ctx(rt);
        sys.handle_native_brightness_key(&ctx, "up");
        assert!(sys.native_key.is_none());
        assert!(sys.native_repeat_delay_due.is_none());
        assert!(sys.native_repeat_until.is_none());
    }

    #[test]
    fn native_key_non_updown_ignored() {
        let mut sys = SystemEvents::new(None);
        let rt = runtime_host();
        let ctx = test_ctx(rt);
        sys.handle_native_brightness_key(&ctx, "other");
        sys.handle_native_brightness_key(&ctx, "");
        sys.handle_native_brightness_key(&ctx, "release");
        assert!(sys.native_key.is_none());
    }

    fn runtime_host() -> Arc<RuntimeHost> {
        let host = MonitorHost::new("test.exe".into(), ".".into());
        let settings = Arc::new(Mutex::new(json!({})));
        RuntimeHost::new(host, settings, std::path::PathBuf::from("known.json"))
    }
}
