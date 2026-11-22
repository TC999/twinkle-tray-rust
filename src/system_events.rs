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
pub fn apply_hotkeys(settings: &Value, tray_hwnd: windows::Win32::Foundation::HWND) -> Vec<hotkey_host::RegisteredHotkey> {
    let parsed = hotkey_host::parse_hotkeys_with_id(settings);
    let mut registered = Vec::new();
    for (idx, hotkey) in parsed.iter().enumerate() {
        let id_val = (10000 + idx) as i32; // 注册 id 与 accelerator 反查表对齐
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

/// handleNativeBrightnessKey(equiv L666+):WM_INPUT 原生亮度键。
/// "up"/"down" 触发 triggerNativeBrightnessHotkey;release 停止重复。
pub fn handle_native_brightness_key(ctx: &HotkeyContext, registrations: &[hotkey_host::RegisteredHotkey], key: &str) {
    match key {
        "release" => { /* stopNativeBrightnessKeyRepeat:重复定时器未实现,等价空 */ }
        "up" | "down" => {
            let accelerator = if key == "up" { "BrightnessUp" } else { "BrightnessDown" };
            trigger_native_brightness_hotkey(ctx, registrations, accelerator);
        }
        _ => {}
    }
}

/// triggerNativeBrightnessHotkey(equiv L649-L664)。
fn trigger_native_brightness_hotkey(ctx: &HotkeyContext, registrations: &[hotkey_host::RegisteredHotkey], accelerator: &str) {
    let rt = &ctx.rt;
    let settings = rt.settings_snapshot();
    let hotkey = registrations.iter().find(|h| h.accelerator == accelerator);
    let Some(hotkey) = hotkey else { return };
    // Windows already applies dedicated brightness keys...(原注释,skip offset)
    hotkey_host::do_hotkey(ctx, hotkey, true);
    let _ = settings;
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
        }
    }

    /// 应用热键注册(托盘 hwnd 可用后调用;equiv applyHotkeys)。
    pub fn apply_hotkeys_now(&mut self, settings: &Value) {
        if let Some(hwnd) = self.tray_hwnd {
            self.registrations = apply_hotkeys(settings, hwnd);
        }
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
                        handle_native_brightness_key(&ctx, &self.registrations, &key);
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
                    tray::TrayEvent::PowerBroadcast(_) => {}
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
