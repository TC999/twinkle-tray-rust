//! 面板窗口控制器：Slint UI <-> monitor worker 的接线层。
//!
//! equiv: origin/src/components/BrightnessPanel.jsx（状态机 + 事件订阅 + 亮度同步节流）
//! + electron.js 的 IPC 处理（update-brightness / request-monitors / full-refresh /
//! pause-updates / sleep-displays / sleep-display / set-vcp / set-sdr-brightness /
//! set-gamma-brightness / panel-height）+ L3027-L3100 的亮度节流 +
//! L3138-L3350 updateBrightness 的主进程侧决策。
//!
//! 线程模型（AGENTS.md §5）：主线程 = Slint 事件循环。worker 事件回调来自任意线程，
//! 一律经 channel 投递，由事件循环内的 Timer 拉取并更新 model（等价 React 的
//! addEventListener + setState）。

use crate::monitor_host::MonitorHost;
use crate::monitor_view;
use crate::ui_types::PanelMonitor;
use crate::window_state;
use serde_json::{json, Value};
use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 面板状态（equiv BrightnessPanel 的 useState 集合）。
pub struct PanelState {
    pub monitors: Value,
    pub settings: Value,
    pub linked_levels: bool,
    pub sleeping: bool,
    pub is_refreshing: bool,
    /// 已下发亮度缓存（equiv lastLevels：只在变化时下发）
    pub last_levels: HashMap<String, f64>,
    /// 面板可见（equiv window.showPanel）
    pub visible: bool,
    /// 待发送的 VCP 写入（主线程收集，Timer 统一投递）
    pub pending_vcp: Vec<(String, i32, f64)>,
    /// 待发送的亮度写入
    pub pending_brightness: Vec<(String, f64)>,
    /// 待发送的 gamma 写入
    pub pending_gamma: Vec<(String, f64)>,
    /// 待发送的 SDR 写入
    pub pending_sdr: Vec<(String, f64)>,
    /// 待关闭显示器（sleepAction 走 DDC 时需要）
    pub pending_sleep_ddc: Vec<String>,
    /// 面板列表模型的单实例（首帧注入，之后按行增量更新）。
    /// 等价 React 的 key 复用：整表重建会销毁正在拖动的滑块 / 正在编辑的数值框。
    pub panel_model: Option<Rc<VecModel<PanelMonitor>>>,
}

impl Default for PanelState {
    fn default() -> Self {
        Self {
            monitors: json!({}),
            settings: json!({}),
            linked_levels: false,
            sleeping: false,
            is_refreshing: true,
            last_levels: HashMap::new(),
            visible: false,
            pending_vcp: Vec::new(),
            pending_brightness: Vec::new(),
            pending_gamma: Vec::new(),
            pending_sdr: Vec::new(),
            pending_sleep_ddc: Vec::new(),
            panel_model: None,
        }
    }
}

/// 单个 (id, vcp) 目标的节流状态（equiv brightnessThrottleTargets）。
struct ThrottleEntry {
    /// 上次真正写入的时刻；None = 从未写入（首次请求立即放行）。
    /// 原实现用 `now - 3600s` 表示"很久以前"，在系统开机不足 1 小时的机器上
    /// `Instant - Duration` 会 panic（真机复现：点滑条即崩）。
    last_time: Option<Instant>,
    pending: Option<PendingWrite>,
    timeout_at: Option<Instant>,
}

#[derive(Clone)]
struct PendingWrite {
    id: String,
    level: f64,
    vcp: String,
}

/// 亮度节流器（equiv updateBrightnessThrottle / flushBrightnessThrottle L3053-L3100）。
pub struct BrightnessThrottle {
    targets: HashMap<String, ThrottleEntry>,
}

impl BrightnessThrottle {
    pub fn new() -> Self {
        Self {
            targets: HashMap::new(),
        }
    }

    /// vcpKey 归一化（equiv L3059-L3061）。
    pub fn vcp_key(vcp: &str) -> String {
        match vcp {
            "brightness" | "sdr" => vcp.to_string(),
            "volume-raw" => "0x62".to_string(),
            other => match other.parse::<i64>() {
                Ok(n) => format!("0x{:x}", n),
                Err(_) => other.to_string(),
            },
        }
    }

    /// 返回 Some(立即写入) 或 None（已排队）。
    pub fn request(
        &mut self,
        id: &str,
        level: f64,
        vcp: &str,
        interval_ms: u64,
    ) -> Option<(String, f64, String)> {
        let target = format!("{}::{}", id, Self::vcp_key(vcp));
        let now = Instant::now();
        let entry = self.targets.entry(target).or_insert_with(|| ThrottleEntry {
            last_time: None,
            pending: None,
            timeout_at: None,
        });

        let interval_elapsed = entry
            .last_time
            .map(|t| now.saturating_duration_since(t).as_millis() as u64 >= interval_ms)
            .unwrap_or(true);
        if entry.timeout_at.is_none() && interval_elapsed {
            entry.last_time = Some(now);
            return Some((id.to_string(), level, vcp.to_string()));
        }

        entry.pending = Some(PendingWrite {
            id: id.to_string(),
            level,
            vcp: vcp.to_string(),
        });
        if entry.timeout_at.is_none() {
            let base = entry.last_time.unwrap_or(now);
            let due = base + Duration::from_millis(interval_ms);
            entry.timeout_at = Some(due.max(now));
        }
        None
    }

    /// 到期的排队写入（equiv flushBrightnessThrottle）。
    pub fn take_due(&mut self, now: Instant) -> Vec<(String, f64, String)> {
        let mut out = Vec::new();
        for entry in self.targets.values_mut() {
            let Some(due) = entry.timeout_at else { continue };
            if now < due {
                continue;
            }
            entry.timeout_at = None;
            entry.last_time = Some(now);
            if let Some(pending) = entry.pending.take() {
                out.push((pending.id, pending.level, pending.vcp));
            }
        }
        out
    }
}

impl Default for BrightnessThrottle {
    fn default() -> Self {
        Self::new()
    }
}

/// 面板控制器（主线程持有）。
pub struct PanelController {
    pub host: Arc<MonitorHost>,
    /// 共享 settings（UI 写回 -> worker 下发）
    pub settings: Arc<Mutex<Value>>,
    events: Receiver<Value>,
    /// 托盘事件（左键 -> 切换面板；右键菜单项 -> 动作）
    pub tray_events: Option<Receiver<tray::TrayEvent>>,
    /// 运行时状态机（完整模式注入;smoke 模式为 None）
    pub runtime: Option<Arc<crate::runtime_hosts::RuntimeHost>>,
}

impl PanelController {
    pub fn new(host: Arc<MonitorHost>, settings: Arc<Mutex<Value>>) -> Self {
        let (tx, rx) = channel::<Value>();
        let tx_clone: Sender<Value> = tx;
        host.manager
            .add_event_callback(Arc::new(move |data: &Value| {
                let _ = tx_clone.send(data.clone());
            }));
        Self {
            host,
            settings,
            events: rx,
            tray_events: None,
            runtime: None,
        }
    }

    /// 注入运行时状态机(完整模式)。
    pub fn attach_runtime(&mut self, runtime: Arc<crate::runtime_hosts::RuntimeHost>) {
        self.runtime = Some(runtime);
    }

    /// 读取运行时状态机。
    pub fn runtime(&self) -> Option<Arc<crate::runtime_hosts::RuntimeHost>> {
        self.runtime.clone()
    }

    /// 接入托盘事件源（由 main 在创建托盘后调用）。
    pub fn attach_tray(&mut self, rx: Receiver<tray::TrayEvent>) {
        self.tray_events = Some(rx);
    }

    /// 取走积压的托盘事件（主线程调用）。
    pub fn drain_tray_events(&self) -> Vec<tray::TrayEvent> {
        let mut out = Vec::new();
        if let Some(rx) = self.tray_events.as_ref() {
            while let Ok(ev) = rx.try_recv() {
                out.push(ev);
            }
        }
        out
    }

    /// 取走积压的 worker 事件（主线程调用）。
    pub fn drain_events(&self) -> Vec<Value> {
        let mut out = Vec::new();
        while let Ok(ev) = self.events.try_recv() {
            out.push(ev);
        }
        out
    }
}

/// 带运行时状态机的完整版（equiv 原版主进程全量事件源接入）。
/// tick 额外驱动 system_events（热键/电源/idle/TOD/显示变化）与 transition。
pub fn run_panel_controller_with_runtime(
    host: Arc<MonitorHost>,
    settings: Arc<Mutex<Value>>,
    tray_events: Option<Receiver<tray::TrayEvent>>,
    runtime: Arc<crate::runtime_hosts::RuntimeHost>,
    system: crate::system_events::SystemEvents,
) -> Result<(), String> {
    // transition 排程联动:runtime_hosts::transition_brightness 之后由 schedule_transition_next 排程
    // (SystemEvents::tick 内轮询 TRANSITION_DUE_AT)
    // SystemEvents 安装到主线程槽位(Slint 组件/Timer 与其同线程)
    SYSTEM_EVENTS_SLOT.with(|slot| {
        slot.borrow_mut().replace(system);
    });
    run_panel_controller_inner(host, settings, tray_events, runtime)
}

/// 内部实现(共享 run_panel_controller 的接线逻辑;SystemEvents 从主线程槽位读取)。
fn run_panel_controller_inner(
    host: Arc<MonitorHost>,
    settings: Arc<Mutex<Value>>,
    tray_events: Option<Receiver<tray::TrayEvent>>,
    runtime: Arc<crate::runtime_hosts::RuntimeHost>,
) -> Result<(), String> {
    let panel = crate::PanelWindow::new().map_err(|e| e.to_string())?;
    let weak = panel.as_weak();
    // 面板句柄存入主线程槽位：设置窗口的主题/语言变更需要联动刷新面板
    PANEL_WINDOW.with(|slot| {
        *slot.borrow_mut() = Some(panel.as_weak());
    });

    let initial_settings = settings.lock().map(|g| g.clone()).unwrap_or(json!({}));
    // 面板定位的 settings 快照（default_panel_position 读 overrideTaskbarPosition/Gap）
    PANEL_SETTINGS_OVERRIDE.with(|s| *s.borrow_mut() = initial_settings.clone());
    let state = Rc::new(RefCell::new(PanelState {
        settings: initial_settings.clone(),
        ..Default::default()
    }));
    let throttle = Rc::new(RefCell::new(BrightnessThrottle::new()));
    let mut controller = PanelController::new(host.clone(), settings.clone());
    controller.attach_runtime(runtime.clone());
    if let Some(rx) = tray_events {
        controller.attach_tray(rx);
    }
    let controller = Rc::new(controller);

    window_state::apply_theme(&panel, &initial_settings);
    apply_static_texts(&panel);
    panel.set_scroll_amount(
        initial_settings
            .get("scrollFlyoutAmount")
            .and_then(|v| v.as_f64())
            .unwrap_or(2.0) as f32,
    );
    panel.set_show_sleep(
        initial_settings
            .get("sleepAction")
            .and_then(|v| v.as_str())
            .map(|v| v != "none")
            .unwrap_or(true),
    );

    wire_callbacks(&panel, &state, &throttle, &controller, &host, &settings);

    // 首次运行引导
    let user_closed_intro = initial_settings
        .get("userClosedIntro")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !user_closed_intro {
        show_intro(settings.clone());
    }

    // 面板默认隐藏（equiv 原版：面板由托盘左键触发 toggleTray 才显示）
    {
        let mut st = state.borrow_mut();
        st.visible = false;
    }

    // 初始请求
    host.refresh_monitors(true, true);

    // ---- 事件循环定时器:系统事件层 + worker 事件 + 节流冲刷 ----
    let tick_state = state.clone();
    let tick_throttle = throttle.clone();
    let tick_controller = controller.clone();
    let tick_weak = weak.clone();
    let tick_runtime = runtime.clone();
    let tick_throttle_shared: &'static Mutex<BrightnessThrottle> = crate::system_events::shared_throttle();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
        let Some(ui) = tick_weak.upgrade() else { return };
        // 0a) 系统事件层(热键/电源/idle/TOD/显示变化/过渡)
        crate::system_events::transition_due_check();
        system_tick(&tick_runtime, &tick_throttle_shared);
        // 0b) HUD 请求(hotkey overlay)
        if crate::system_events::take_hud_request() {
            crate::panel_controller::show_hud_from_runtime(&tick_runtime);
        }
        tick(&ui, &tick_state, &tick_throttle, &tick_controller);
    });

    let result = slint::run_event_loop_until_quit().map_err(|e| e.to_string());
    timer.stop();
    result
}

/// system_events 的单次 tick(UI 主线程调用;SystemEvents 不是 Send,放在闭包捕获中)。
fn system_tick(rt: &Arc<crate::runtime_hosts::RuntimeHost>, _throttle: &'static Mutex<BrightnessThrottle>) {
    // SystemEvents 保存在主线程槽位(RefCell 语义等价原版单线程事件循环)
    SYSTEM_EVENTS_SLOT.with(|slot| {
        let mut borrow = slot.borrow_mut();
        if let Some(system) = borrow.as_mut() {
            system.tick(rt);
        }
    });
}

thread_local! {
    /// SystemEvents 主线程槽位(由 run_panel_controller_with_runtime 安装)。
    static SYSTEM_EVENTS_SLOT: std::cell::RefCell<Option<crate::system_events::SystemEvents>> =
        const { std::cell::RefCell::new(None) };

    /// 面板窗口槽位（主线程独占，存 Weak：强引用仍由 run 函数持有）。
    /// 设置窗口改主题/语言时联动刷新面板
    /// （equiv sendToAllWindows('theme-settings', ...) / getLocalization 的全窗口广播）。
    static PANEL_WINDOW: std::cell::RefCell<Option<slint::Weak<crate::PanelWindow>>> =
        const { std::cell::RefCell::new(None) };
}

/// 把 settings 主题应用到面板窗口（设置窗口主题键变更时调用；无面板时静默跳过）。
pub fn apply_panel_theme(settings: &Value) {
    PANEL_WINDOW.with(|slot| {
        if let Some(panel) = slot.borrow().as_ref().and_then(|w| w.upgrade()) {
            window_state::apply_theme(&panel, settings);
        }
    });
}

/// 重新注册全局热键（equiv processSettings L1272-L1274：
/// newSettings.hotkeys !== undefined -> applyHotkeys）。
/// 设置窗口与面板同处主线程事件循环，故经 SYSTEM_EVENTS_SLOT 触达；
/// 纯预览模式（无面板）下槽位为空，静默跳过。
pub fn reapply_hotkeys(settings: &Value) {
    SYSTEM_EVENTS_SLOT.with(|slot| {
        let mut borrow = slot.borrow_mut();
        if let Some(system) = borrow.as_mut() {
            system.apply_hotkeys_now(settings);
        }
    });
}

/// 面板静态文案重载（语言切换时调用；equiv getLocalization 后的窗口重渲染）。
pub fn apply_panel_texts() {
    PANEL_WINDOW.with(|slot| {
        if let Some(panel) = slot.borrow().as_ref().and_then(|w| w.upgrade()) {
            apply_static_texts(&panel);
        }
    });
}

/// HUD 快照展示(取当前平均亮度,等价 hotkeyOverlayStart 的覆盖层)。
pub fn show_hud_from_runtime(rt: &Arc<crate::runtime_hosts::RuntimeHost>) {
    let settings = rt.settings_snapshot();
    let monitors = rt.host.monitors_snapshot();
    let mut avg = 0.0;
    let mut n = 0u32;
    if let Some(obj) = monitors.as_object() {
        for (_k, m) in obj {
            let mtype = m.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if mtype == "ddcci" || mtype == "wmi" || mtype == "software" || monitor_view::uses_gamma_slider(m, &settings) {
                n += 1;
                avg += m.get("brightness").and_then(|v| v.as_f64()).unwrap_or(0.0);
            }
        }
    }
    if n > 0 {
        let label = tt_localize::t("PANEL_LABEL_BRIGHTNESS");
        show_hud((avg / n as f64) as f32, &label);
    }
}

/// 启动面板控制器并进入 Slint 事件循环。
/// host/settings 由 main 传入（已启动 worker 并完成 ready 四连发）。
pub fn run_panel_controller(
    host: Arc<MonitorHost>,
    settings: Arc<Mutex<Value>>,
    tray_events: Option<Receiver<tray::TrayEvent>>,
) -> Result<(), String> {
    let panel = crate::PanelWindow::new().map_err(|e| e.to_string())?;
    let weak = panel.as_weak();
    PANEL_WINDOW.with(|slot| {
        *slot.borrow_mut() = Some(panel.as_weak());
    });

    let initial_settings = settings.lock().map(|g| g.clone()).unwrap_or(json!({}));
    let state = Rc::new(RefCell::new(PanelState {
        settings: initial_settings.clone(),
        ..Default::default()
    }));
    let throttle = Rc::new(RefCell::new(BrightnessThrottle::new()));
    let mut controller = PanelController::new(host.clone(), settings.clone());
    if let Some(rx) = tray_events {
        controller.attach_tray(rx);
    }
    let controller = Rc::new(controller);

    window_state::apply_theme(&panel, &initial_settings);
    apply_static_texts(&panel);
    panel.set_scroll_amount(
        initial_settings
            .get("scrollFlyoutAmount")
            .and_then(|v| v.as_f64())
            .unwrap_or(2.0) as f32,
    );
    panel.set_show_sleep(
        initial_settings
            .get("sleepAction")
            .and_then(|v| v.as_str())
            .map(|v| v != "none")
            .unwrap_or(true),
    );

    wire_callbacks(&panel, &state, &throttle, &controller, &host, &settings);

    // 首次运行引导（equiv electron.js 的 createIntro + userClosedIntro）
    let user_closed_intro = initial_settings
        .get("userClosedIntro")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !user_closed_intro {
        show_intro(settings.clone());
    }

    // 面板默认隐藏（equiv 原版：面板由托盘左键触发 toggleTray 才显示）
    {
        let mut st = state.borrow_mut();
        st.visible = false;
    }

    // 初始请求（equiv BrightnessPanel L204-L207：requestSettings/requestMonitors）
    host.refresh_monitors(true, true);

    // ---- 事件循环定时器：拉取 worker 事件 + 冲刷节流队列 ----
    let tick_state = state.clone();
    let tick_throttle = throttle.clone();
    let tick_controller = controller.clone();
    let tick_weak = weak.clone();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
        let Some(ui) = tick_weak.upgrade() else { return };
        tick(&ui, &tick_state, &tick_throttle, &tick_controller);
    });

    let result = slint::run_event_loop_until_quit().map_err(|e| e.to_string());
    timer.stop();
    result
}

thread_local! {
    /// 首次运行引导窗口（主线程独占）。
    static INTRO_WINDOW: std::cell::RefCell<Option<crate::IntroWindow>> =
        const { std::cell::RefCell::new(None) };
}

/// 显示首次运行引导（equiv electron.js createIntroWindow + intro.js）。
/// 关闭后写 userClosedIntro = true（equiv ipcMain.on('close-intro') L4846）。
fn show_intro(settings: Arc<Mutex<Value>>) {
    let Ok(intro) = crate::IntroWindow::new() else {
        return;
    };
    let settings_snapshot = settings.lock().map(|g| g.clone()).unwrap_or(json!({}));
    crate::window_state::apply_theme_generic(&intro, &settings_snapshot);
    intro.set_t_title(tt_localize::t("INTRO_TITLE").into());
    intro.set_t_instructions(tt_localize::t("INTRO_INSTRUCTIONS").into());
    intro.set_t_close(tt_localize::t("GENERIC_CLOSE").into());

    let weak = intro.as_weak();
    intro.on_close_requested(move || {
        if let Some(ui) = weak.upgrade() {
            let _ = ui.hide();
        }
        if let Ok(mut guard) = settings.lock() {
            if let Some(obj) = guard.as_object_mut() {
                obj.insert("userClosedIntro".to_string(), json!(true));
            }
        }
        INTRO_WINDOW.with(|slot| {
            slot.borrow_mut().take();
        });
    });
    let _ = intro.show();
    INTRO_WINDOW.with(|slot| {
        *slot.borrow_mut() = Some(intro);
    });
}

/// 显示亮度 HUD（equiv hotkeyOverlay 系列：2 秒后自动隐藏）。
pub fn show_hud(level: f32, label: &str) {
    let Ok(hud) = crate::HudWindow::new() else {
        return;
    };
    hud.set_level(level);
    hud.set_label(label.into());
    let _ = hud.show();
    HUD_WINDOW.with(|slot| {
        *slot.borrow_mut() = Some(hud);
    });
}

thread_local! {
    static HUD_WINDOW: std::cell::RefCell<Option<crate::HudWindow>> =
        const { std::cell::RefCell::new(None) };
}

/// 静态文案（equiv TranslateReact 注入的 t()）。
fn apply_static_texts(panel: &crate::PanelWindow) {
    use tt_localize::t;
    panel.set_t_title(t("PANEL_TITLE").into());
    panel.set_t_link_levels(t("PANEL_BUTTON_LINK_LEVELS").into());
    panel.set_t_turn_off_displays(t("PANEL_BUTTON_TURN_OFF_DISPLAYS").into());
    panel.set_t_settings(t("GENERIC_SETTINGS").into());
    panel.set_t_no_displays(t("GENERIC_NO_COMPATIBLE_DISPLAYS").into());
    panel.set_t_detecting(t("GENERIC_DETECTING_DISPLAYS").into());
    panel.set_t_all_displays(t("GENERIC_ALL_DISPLAYS").into());
    panel.set_t_turn_on(t("PANEL_LABEL_TURN_ON").into());
    panel.set_t_turn_off(t("PANEL_LABEL_TURN_OFF").into());
    panel.set_t_update_available(t("PANEL_UPDATE_AVAILABLE").into());
    panel.set_t_install(t("GENERIC_INSTALL").into());
    panel.set_t_dismiss(t("GENERIC_DISMISS").into());
}

/// 接线全部 UI 回调（equiv ipcMain.on 表）。
fn wire_callbacks(
    panel: &crate::PanelWindow,
    state: &Rc<RefCell<PanelState>>,
    throttle: &Rc<RefCell<BrightnessThrottle>>,
    _controller: &Rc<PanelController>,
    _host: &Arc<MonitorHost>,
    settings: &Arc<Mutex<Value>>,
) {
    // update-brightness（equiv L3612-L3620）
    let s = state.clone();
    let t = throttle.clone();
    panel.on_brightness_changed(move |key, level| {
        let key = key.to_string();
        request_brightness(&s, &t, &key, level as f64, "brightness");
    });

    // set-vcp / 特征滑块（equiv L3718-L3721 + DDCCISliders 的比例换算）
    let s = state.clone();
    let t = throttle.clone();
    panel.on_feature_changed(move |key, code, level| {
        let key = key.to_string();
        request_feature(&s, &t, &key, code, level as f64);
    });

    // 输入源按钮（equiv 0x60 分支：直接写 VCP，不做比例换算）
    let s = state.clone();
    panel.on_input_picked(move |key, code, value| {
        let key = key.to_string();
        if let Some((hwid, monitor_id)) = monitor_ids(&s, &key) {
            let mut st = s.borrow_mut();
            st.pending_vcp.push((hwid, code, value as f64));
            // 同步更新本地 features 缓存（equiv monitor.features[vcp][0] = value）
            if let Some(m) = st.monitors.get_mut(&key).and_then(|m| m.as_object_mut()) {
                if let Some(f) = m
                    .get_mut("features")
                    .and_then(|f| f.as_object_mut())
                    .and_then(|f| f.get_mut("0x60"))
                    .and_then(|v| v.as_array_mut())
                {
                    if let Some(first) = f.first_mut() {
                        *first = json!(value);
                    }
                }
            }
            let _ = monitor_id;
        }
    });

    // 电源按钮（equiv powerOff L309-L312 + turnOffDisplayDDC L3556-L3591）
    let s = state.clone();
    panel.on_power_clicked(move |key, power_on| {
        let key = key.to_string();
        toggle_power(&s, &key, power_on);
    });

    // 联动开关（equiv toggleLinkedLevels L42-L48）
    let s = state.clone();
    let settings_handle = settings.clone();
    panel.on_link_levels_toggled(move || {
        let mut st = s.borrow_mut();
        st.linked_levels = !st.linked_levels;
        let linked = st.linked_levels;
        if let Some(obj) = st.settings.as_object_mut() {
            obj.insert("linkedLevelsActive".to_string(), json!(linked));
        }
        if let Ok(mut g) = settings_handle.lock() {
            if let Some(obj) = g.as_object_mut() {
                obj.insert("linkedLevelsActive".to_string(), json!(linked));
            }
        }
    });

    // 睡眠显示器（equiv sleep-displays L3710）
    let s = state.clone();
    panel.on_sleep_displays(move || {
        sleep_displays(&s);
    });

    // 设置入口（equiv open-settings L3645）：同线程创建独立窗口（等价 Electron 多 BrowserWindow）
    let s2 = state.clone();
    let settings_handle = settings.clone();
    let host_handle = _host.clone();
    let panel_weak = panel.as_weak();
    settings_window_slot().replace(None);
    panel.on_open_settings(move || {
        let _ = &s2;
        match crate::settings_host::open_settings_window(
            settings_handle.clone(),
            host_handle.clone(),
        ) {
            Ok(handle) => {
                settings_window_slot().replace(Some(handle));
                if let Some(ui) = panel_weak.upgrade() {
                    let _ = ui.hide();
                }
            }
            Err(e) => eprintln!("设置窗口打开失败: {}", e),
        }
    });

    // 刷新（equiv full-refresh L3627-L3633）
    let host = _host.clone();
    let s3 = state.clone();
    panel.on_refresh_requested(move || {
        s3.borrow_mut().is_refreshing = true;
        host.refresh_monitors(true, true);
    });

    // Escape 关闭面板（equiv blur-panel L3690-L3692）
    let weak = panel.as_weak();
    panel.on_escape_pressed(move || {
        if let Some(ui) = weak.upgrade() {
            let _ = ui.hide();
        }
    });

    // 更新条（equiv installUpdate / dismissUpdate）
    // install:下载 + 运行安装器（equiv preload installUpdate -> get-update L3670）
    let s4 = state.clone();
    panel.on_install_update(move || {
        let up = crate::update_host::snapshot();
        if up.download_url.is_empty() {
            return;
        }
        // equiv L3671:latestVersion.error = false
        crate::update_host::clear_error();
        // clear-update（equiv preload L181）
        crate::update_host::clear_update();
        let update_path = std::env::var("TT_CONFIG_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join("update.exe");
        crate::update_host::download_and_install(&up.download_url, up.filesize, update_path);
        let _ = s4;
    });
    // dismiss:写 dismissedUpdate + 清 show（equiv preload dismissUpdate -> ignore-update L5190）
    let snap = settings.clone();
    panel.on_dismiss_update(move || {
        let version = crate::update_host::snapshot().version;
        if let Ok(mut guard) = snap.lock() {
            guard["dismissedUpdate"] = json!(version);
        }
        crate::update_host::ignore_update(&version);
    });
}

/// 设置窗口槽位（实现在 settings_host：设置页回调也要能标记脏状态）。
fn settings_window_slot() -> &'static std::cell::RefCell<Option<crate::settings_host::SettingsWindowHandle>> {
    crate::settings_host::settings_window_slot()
}

/// 取显示器的 (hwid 拼接串, id)。
fn monitor_ids(state: &Rc<RefCell<PanelState>>, key: &str) -> Option<(String, String)> {
    let st = state.borrow();
    let monitor = st.monitors.get(key)?;
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
    let id = monitor
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if hwid.is_empty() {
        None
    } else {
        Some((hwid, id))
    }
}

/// updateBrightness 的 VCP 目标判定 + 节流（equiv L3138-L3187 + L3053-L3100）。
fn request_brightness(
    state: &Rc<RefCell<PanelState>>,
    throttle: &Rc<RefCell<BrightnessThrottle>>,
    key: &str,
    level: f64,
    vcp: &str,
) {
    if !level.is_finite() {
        return;
    }
    let mut st = state.borrow_mut();
    let Some(monitor) = st.monitors.get(key).cloned() else {
        return;
    };
    let id = monitor
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if id.is_empty() {
        return;
    }
    let interval = st
        .settings
        .get("updateInterval")
        .and_then(|v| v.as_u64())
        .unwrap_or(500);

    // 隐藏的显示器直接忽略（equiv L3175-L3177）
    let hidden = st
        .settings
        .get("hideDisplays")
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if hidden {
        return;
    }

    // 目标判定（equiv L3180-L3187）：SDR 主滑块优先，其次 gamma 主滑块
    let mut target = vcp.to_string();
    let mut use_cap = true;
    if vcp == "brightness" {
        let sdr_main = st
            .settings
            .get("sdrAsMainSliderDisplays")
            .and_then(|v| v.get(key))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if monitor.get("hdr").and_then(|v| v.as_str()) == Some("active") && sdr_main {
            target = "sdr".to_string();
            use_cap = false;
        } else if monitor_view::uses_gamma_slider(&monitor, &st.settings) {
            target = "gamma".to_string();
        }
    }

    // 扩展下限断点（equiv L3209-L3217）：断点以下钉住硬件最低值，用 gamma 压暗
    let extended_minimum = target == "brightness"
        && monitor_view::uses_extended_minimum(&monitor, &st.settings);
    let mut hardware_level = level;
    if extended_minimum {
        let breakpoint = monitor_view::get_extended_minimum_breakpoint(&monitor, &st.settings) as f64;
        hardware_level = if level >= breakpoint {
            (level - breakpoint) * 100.0 / (100.0 - breakpoint)
        } else {
            0.0
        };
        // setTrackedGammaLevel：断点以上回到 100，断点以下按比例压暗
        let gamma_level = if level >= breakpoint {
            100.0
        } else {
            monitor_view::GAMMA_BRIGHTNESS_MIN
                + (level * (100.0 - monitor_view::GAMMA_BRIGHTNESS_MIN) / breakpoint)
        };
        let monitor_id = monitor
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        st.pending_gamma.push((monitor_id, gamma_level.round()));
    }

    // normalizeBrightness（equiv L3219）：useCap 决定是否套用用户 min/max 与校准
    let min = if use_cap {
        monitor.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0)
    } else {
        0.0
    };
    let max = if use_cap {
        monitor.get("max").and_then(|v| v.as_f64()).unwrap_or(100.0)
    } else {
        100.0
    };
    let calibration = if use_cap {
        monitor_view::calibration_of(&monitor)
    } else {
        Vec::new()
    };
    let normalized = monitor_view::normalize_brightness(hardware_level, false, min, max, &calibration);

    // 本地模型立即更新（equiv monitor.brightness = level）
    if target == "brightness" {
        if let Some(m) = st.monitors.get_mut(key).and_then(|m| m.as_object_mut()) {
            m.insert("brightness".to_string(), json!(level));
            m.insert("brightnessRaw".to_string(), json!(normalized));
        }
    }
    st.last_levels.insert(key.to_string(), level);

    let is_wmi = monitor.get("type").and_then(|v| v.as_str()) == Some("wmi");
    let due = throttle.borrow_mut().request(&id, level, &target, interval);
    if let Some((id, level, target)) = due {
        enqueue_write_typed(&mut st, &id, level, &target, is_wmi);
    }
}

/// 特征滑块（equiv DDCCISliders 的 setVCP：value * features[vcp][1] / 100）。
fn request_feature(
    state: &Rc<RefCell<PanelState>>,
    _throttle: &Rc<RefCell<BrightnessThrottle>>,
    key: &str,
    code: i32,
    level: f64,
) {
    let mut st = state.borrow_mut();
    let Some(monitor) = st.monitors.get(key).cloned() else {
        return;
    };
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
        return;
    }
    let vcp_str = format!("0x{:X}", code);
    let feature = monitor.get("features").and_then(|f| f.get(&vcp_str)).cloned();
    let Some(feature) = feature else { return };
    let max = feature
        .get(1)
        .and_then(|v| v.as_f64())
        .unwrap_or(100.0);
    let feature_settings = monitor
        .get("hwid")
        .and_then(|v| v.as_array())
        .and_then(|a| a.get(1))
        .and_then(|v| v.as_str())
        .and_then(|hwid1| st.settings.get("monitorFeaturesSettings").and_then(|v| v.get(hwid1)))
        .cloned()
        .unwrap_or(json!({}));

    // 音量走 scaleVolume（equiv DDCCISliders.jsx L76）
    let value = if code == 0x62 {
        monitor_view::scale_volume(level, &feature, &feature_settings)
    } else {
        // equiv val * (monitor.features[vcp][1] / 100)
        (level * (max / 100.0)).round()
    };

    // 本地 features 缓存同步（equiv monitor.features[vcp][0] = val）
    if let Some(m) = st.monitors.get_mut(key).and_then(|m| m.as_object_mut()) {
        if let Some(f) = m
            .get_mut("features")
            .and_then(|f| f.as_object_mut())
            .and_then(|f| f.get_mut(&vcp_str))
            .and_then(|v| v.as_array_mut())
        {
            if let Some(first) = f.first_mut() {
                *first = json!(value);
            }
        }
    }
    st.pending_vcp.push((hwid, code, value));
}

/// 电源切换（equiv powerOff L309-L312 + turnOffDisplayDDC）。
fn toggle_power(state: &Rc<RefCell<PanelState>>, key: &str, power_on: bool) {
    let mut st = state.borrow_mut();
    let Some((hwid, _)) = monitor_ids(state, key) else {
        return;
    };
    let off_value = st
        .settings
        .get("ddcPowerOffValue")
        .and_then(|v| v.as_i64())
        .unwrap_or(5);
    if power_on {
        // 已开 -> 关：按 ddcPowerOffValue 的 4/5/6 组合下发（equiv L3572-L3587）
        if off_value == 4 || off_value == 6 {
            st.pending_vcp.push((hwid.clone(), 0xD6, 4.0));
        }
        if off_value == 5 || off_value == 6 {
            st.pending_vcp.push((hwid, 0xD6, 5.0));
        }
    } else {
        // 已关 -> 开（equiv toggle 分支：值 1 唤醒）
        st.pending_vcp.push((hwid, 0xD6, 1.0));
    }
}

/// sleepDisplays（equiv L3533-L3554）。
fn sleep_displays(state: &Rc<RefCell<PanelState>>) {
    let mut st = state.borrow_mut();
    let action = st
        .settings
        .get("sleepAction")
        .and_then(|v| v.as_str())
        .unwrap_or("none")
        .to_string();
    if action == "none" {
        return;
    }
    // ddcci / ps_ddcci：逐台下发 0xD6（equiv L3538-L3543）
    if action == "ddcci" || action == "ps_ddcci" {
        let keys: Vec<String> = st
            .monitors
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        for key in keys {
            if let Some(monitor) = st.monitors.get(&key) {
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
                if !hwid.is_empty() {
                    st.pending_sleep_ddc.push(hwid);
                }
            }
        }
    }
    // ps / ps_ddcci：广播关闭显示器（equiv L3545-L3547）
    if action == "ps" || action == "ps_ddcci" {
        let _ = twutils::monitor_power::turn_off_displays();
    }
}

/// 把一次已批准的亮度写入加入待发送队列（按目标分派到对应通道）。
fn enqueue_write(st: &mut PanelState, id: &str, level: f64, target: &str) {
    let is_wmi = st
        .monitors
        .as_object()
        .and_then(|o| {
            o.values()
                .find(|m| m.get("id").and_then(|v| v.as_str()) == Some(id))
        })
        .and_then(|m| m.get("type"))
        .and_then(|v| v.as_str())
        == Some("wmi");
    enqueue_write_typed(st, id, level, target, is_wmi);
}

/// 带 type 信息的入队：WMI 显示器的亮度由系统全局接口设置，
/// 原版下发时不带 id（equiv electron.js:L3328-L3337），worker 走 None 分支。
fn enqueue_write_typed(st: &mut PanelState, id: &str, level: f64, target: &str, is_wmi: bool) {
    match target {
        "sdr" => st.pending_sdr.push((id.to_string(), level)),
        "gamma" => {
            // equiv sliderLevelToGammaLevel：GAMMA_BRIGHTNESS_MIN=20
            let gamma = monitor_view::slider_level_to_gamma_level(level);
            st.pending_gamma.push((id.to_string(), gamma));
        }
        _ => {
            if is_wmi {
                // 空 id 表示全局 WMI 写入
                st.pending_brightness.push((String::new(), level));
            } else {
                st.pending_brightness.push((id.to_string(), level));
            }
        }
    }
}

/// 事件循环 tick：拉取 worker 事件、冲刷队列、刷新 model。
fn tick(
    ui: &crate::PanelWindow,
    state: &Rc<RefCell<PanelState>>,
    throttle: &Rc<RefCell<BrightnessThrottle>>,
    controller: &Rc<PanelController>,
) {
    // 0) 托盘事件（equiv tray.on("click") -> toggleTray；菜单项 -> 动作）
    for event in controller.drain_tray_events() {
        handle_tray_event(ui, state, controller, &event);
    }

    // 1) worker 事件（equiv addEventListener 的各个 handler）
    for event in controller.drain_events() {
        handle_worker_event(state, &event);
    }

    // 2) 节流到期写入
    let due = throttle.borrow_mut().take_due(Instant::now());
    {
        let mut st = state.borrow_mut();
        for (id, level, target) in due {
            enqueue_write(&mut st, &id, level, &target);
        }
    }

    // 3) 冲刷待发送命令到 worker（equiv monitorsThread.send）
    flush_pending(state, &controller.host);

    // 3b) 更新条状态（equiv latest-version / updateProgress 事件推送）
    refresh_update_bar(ui);

    // 4) 刷新 UI model
    refresh_ui(ui, state);

    // 5) 设置窗口跟随 worker 的 monitors 事件（equiv monitorsUpdated -> setState）
    sync_settings_window(controller);
}

/// 把最新显示器列表同步进已打开的设置窗口。
///
/// equiv: origin/src/components/SettingsWindow.jsx:L182-L185 的
/// `window.addEventListener("monitorsUpdated", this.recievedMonitors)`：
/// 主进程每次推送显示器列表，设置窗口就重建 monitors 页的数据源。
fn sync_settings_window(controller: &Rc<PanelController>) {
    let slot = settings_window_slot();
    if slot.borrow().is_none() {
        return;
    }
    let monitors = controller.host.monitors_snapshot();
    let settings = controller
        .settings
        .lock()
        .map(|g| g.clone())
        .unwrap_or(json!({}));
    if let Some(handle) = slot.borrow().as_ref() {
        crate::settings_host::refresh_settings_monitors(handle, &settings, &monitors);
    }
}

/// 刷新面板更新条（读取 update_host 共享状态;equiv panel-preload 的 latest-version/updateProgress 监听）。
fn refresh_update_bar(ui: &crate::PanelWindow) {
    let up = crate::update_host::snapshot();
    ui.set_update_version(up.version.clone().into());
    ui.set_update_visible(up.show && !up.version.is_empty());
    ui.set_update_downloading(up.downloading);
    ui.set_update_progress(up.progress as f32);
}

/// 托盘事件处理（equiv toggleTray / setTrayMenu 的 click 回调）。
fn handle_tray_event(
    ui: &crate::PanelWindow,
    state: &Rc<RefCell<PanelState>>,
    controller: &Rc<PanelController>,
    event: &tray::TrayEvent,
) {
    match event {
        // 左键：切换面板显隐（equiv toggleTray）
        tray::TrayEvent::Click => {
            let visible = ui.window().is_visible();
            if visible {
                let _ = ui.hide();
                state.borrow_mut().visible = false;
                // killWhenIdle（equiv L3684-L3688 panel-hidden）：Electron 的
                // mainWindow.close() 在 Slint 无等价销毁 API，以隐藏降级（窗口
                // 资源已由 hide 释放；差异记录于 PROGRESS.md）
                if state
                    .borrow()
                    .settings
                    .get("killWhenIdle")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    println!("killWhenIdle: panel hidden (window destroy unsupported; hide as fallback)");
                }
            } else {
                state.borrow_mut().visible = true;
                let _ = ui.show();
                let _ = ui.window().set_position(default_panel_position(ui));
                ui.invoke_refresh_requested();
                controller.host.refresh_monitors(false, false);
            }
        }
        tray::TrayEvent::RightClick => {}
        tray::TrayEvent::Menu(id) => match *id {
            // 1001：显示面板
            1001 => {
                state.borrow_mut().visible = true;
                let _ = ui.show();
                let _ = ui.window().set_position(default_panel_position(ui));
            }
            // 1002：打开设置
            1002 => {
                ui.invoke_open_settings();
            }
            // 1003：关闭显示器
            1003 => {
                sleep_displays(state);
            }
            // 2000：退出（equiv quitApp L4730-L4732）
            2000 => {
                let _ = ui.window().hide();
                let _ = slint::quit_event_loop();
            }
            // 2001：暂停 TOD（equiv L4653 的 tempSettings.pauseTimeAdjustments）
            2001 => {
                let rt = controller.runtime();
                if let Some(rt) = rt {
                    let mut temp = rt.temp.lock().unwrap();
                    temp.pause_time_adjustments = !temp.pause_time_adjustments;
                }
            }
            // 2002：暂停 idle 检测（equiv L4660 的 tempSettings.pauseIdleDetection）
            2002 => {
                let rt = controller.runtime();
                if let Some(rt) = rt {
                    let mut temp = rt.temp.lock().unwrap();
                    temp.pause_idle_detection = !temp.pause_idle_detection;
                }
            }
            // 2003：刷新显示器（equiv L4635）
            2003 => {
                ui.invoke_refresh_requested();
                controller.host.refresh_monitors(true, true);
            }
            // ---- DEBUG 子菜单（equiv getDebugTrayMenuItems L4683-L4698；仅 isDev 可见）----
            // 2004：RESTART PANEL（equiv restartPanel）——Rust 侧无持久窗口重建，等价重新请求全量刷新
            2004 => {
                controller.host.refresh_monitors(true, true);
            }
            // 2005：RECREATE TRAY（equiv recreateTray）——托盘常驻线程无重建语义，仅日志
            2005 => {
                println!("DEBUG: RECREATE TRAY (tray thread resident; no-op)");
            }
            // 2006：MINIMIZE PANEL（equiv mainWindow.minimize）
            2006 => {
                let _ = ui.window().set_minimized(true);
            }
            // 2007：HIDE PANEL（equiv showPanel(false)）
            2007 => {
                let _ = ui.hide();
                state.borrow_mut().visible = false;
            }
            // 2008/2009：OPACITY 0/1（equiv setOpacity）——Slint 无透明度 API，降级为日志
            2008 => {
                println!("DEBUG: OPACITY 0 (Slint unsupported; no-op)");
            }
            2009 => {
                println!("DEBUG: OPACITY 1 (Slint unsupported; no-op)");
            }
            // 2010：DO CURRENT TOD（equiv applyCurrentAdjustmentEvent(true)）
            2010 => {
                let rt = controller.runtime();
                if let Some(rt) = rt {
                    let tz = crate::system_events::timezone_offset_minutes();
                    crate::runtime_hosts::apply_current_adjustment_event(&rt, true, false, tz);
                }
            }
            // 2011：REMOVE ACRYLIC（equiv tryVibrancy(mainWindow, false)）——
            // Slint 侧窗口材质由主题刷新驱动，此处更新设置后经 apply_theme 生效
            2011 => {
                let rt = controller.runtime();
                if let Some(rt) = rt {
                    rt.update_settings(json!({ "useAcrylic": false }));
                    let settings = rt.settings_snapshot();
                    crate::window_state::apply_theme(ui, &settings);
                    println!("DEBUG: REMOVE ACRYLIC");
                }
            }
            // 2012：PAUSE MOUSE（equiv pauseMouseEvents(true)）——滚动快捷未移植，仅日志
            2012 => {
                println!("DEBUG: PAUSE MOUSE (scroll shortcut not ported; no-op)");
            }
            // 2013：LAST ACTIVE WIN（equiv trySetForegroundWindow(forcedFocusID)）——无强制前台状态，仅日志
            2013 => {
                println!("DEBUG: LAST ACTIVE WIN (forced focus not ported; no-op)");
            }
            // 2100+：应用 profile（equiv applyProfileBrightness;亮度档案的 setBrightness 应用）
            id if id >= 2100 => {
                let rt = controller.runtime();
                if let Some(rt) = rt {
                    let settings = rt.settings_snapshot();
                    let profiles = settings.get("profiles").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                    let idx = (id - 2100) as usize;
                    if let Some(profile) = profiles.get(idx) {
                        // applyProfileBrightness:profile 的 monitors/brightness 应用(等价 applyProfile)
                        let brightness = profile.get("brightness").and_then(|v| v.as_f64());
                        if let Some(level) = brightness {
                            crate::runtime_hosts::update_all_brightness(&rt, level, "set");
                        }
                    }
                }
            }
            _ => {}
        },
        // 系统事件(热键/电源/显示变化)转发给 system_events 层
        tray::TrayEvent::Hotkey(_) | tray::TrayEvent::WmDisplayChange | tray::TrayEvent::PowerSetting(_, _, _)
        | tray::TrayEvent::PowerBroadcast(_) | tray::TrayEvent::ScMonitorPower | tray::TrayEvent::NativeBrightnessKey(_) => {
            crate::system_events::forward_event(event.clone());
        }
    }
}

/// 面板默认位置（equiv positionPanel：任务栏所在边贴边）。
/// 主显示器工作区右下角，按面板尺寸内缩 12px。
fn default_panel_position(ui: &crate::PanelWindow) -> slint::PhysicalPosition {
    // equiv taskbarPosition（L4065-L4118）+ overrideTaskbarPosition/Gap（L4105-L4115）：
    // 真实任务栏探测依赖 Shell 任务栏矩形（Slint 无 work area API），以主屏分辨率
    // 假定 BOTTOM + gap 12 等价降级（PROGRESS.md 记录）；override 键原样支持。
    let settings = PANEL_SETTINGS_OVERRIDE.with(|s| s.borrow().clone());
    let mut position = "BOTTOM";
    if let Some(pos) = settings
        .get("overrideTaskbarPosition")
        .and_then(|v| v.as_str())
        .map(|s| s.to_uppercase())
    {
        if ["BOTTOM", "TOP", "LEFT", "RIGHT"].contains(&pos.as_str()) {
            position = Box::leak(pos.into_boxed_str());
        }
    }
    let mut gap: i32 = 12;
    if let Some(g) = settings.get("overrideTaskbarGap").and_then(|v| v.as_i64()) {
        gap = g as i32;
    }
    // Slint 的 window().set_position 走 winit 的 LogicalPosition 语义：
    // 传物理坐标会被再乘一次 scale_factor，导致 DPI≠100% 时窗口跑到屏幕外
    // （真机缺陷：120 DPI 下面板落在 1940,1123 而屏幕仅 1920x1080）。
    // 因此这里统一用逻辑像素（equiv Electron 的 DIP 坐标）计算。
    let size = ui.window().size();
    let scale = ui.window().scale_factor();
    let w = (size.width as f32 / scale).round() as i32;
    let h = (size.height as f32 / scale).round() as i32;
    // equiv screen.getPrimaryDisplay().workAreaSize（electron.js:L4065-L4118 的定位基准）：
    // 读真实主屏工作区，不再硬编码 1920x1080。
    // SPI_GETWORKAREA 在 DPI-aware 进程下返回物理像素，需换算到逻辑像素
    // 才能与 w/h（逻辑）同口径计算。
    let (work_w_phys, work_h_phys) = primary_work_area_physical().unwrap_or((1920, 1030));
    let work_w = (work_w_phys as f32 / scale).round() as i32;
    let work_h = (work_h_phys as f32 / scale).round() as i32;
    let (x, y) = match position {
        "TOP" => (work_w - w - gap, gap),
        "LEFT" => (gap, work_h - h - gap),
        "RIGHT" => (work_w - w - gap, work_h - h - gap),
        _ => (work_w - w - gap, work_h - h - gap),
    };
    // set_position 需要 PhysicalPosition：逻辑坐标 × scale_factor 换算回物理像素。
    // （此前直接把"逻辑值当物理值"传入，DPI≠100% 时窗口落到屏幕外。）
    slint::PhysicalPosition::new(
        (x.max(0) as f32 * scale) as i32,
        (y.max(0) as f32 * scale) as i32,
    )
}

/// 主屏工作区（物理像素）。equiv Electron screen.getPrimaryDisplay().workAreaSize。
/// 用 SystemParametersInfoW(SPI_GETWORKAREA)（已扣除任务栏）。DPI-aware 进程下
/// 返回物理像素，调用方需按 scale_factor 换算到逻辑口径。
fn primary_work_area_physical() -> Option<(i32, i32)> {
    use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWORKAREA};
    use windows::Win32::Foundation::RECT;
    // SAFETY: SPI_GETWORKAREA 只写 RECT（由系统填充），指针有效且生命周期覆盖调用。
    unsafe {
        let mut rect = RECT::default();
        let ok = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut rect as *mut RECT as *mut core::ffi::c_void),
            Default::default(),
        );
        if ok.is_err() {
            return None;
        }
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        if w > 0 && h > 0 { Some((w, h)) } else { None }
    }
}

// 面板定位用 settings 快照（run_panel_controller 安装；thread_local 等价单线程语义）
thread_local! {
    static PANEL_SETTINGS_OVERRIDE: std::cell::RefCell<serde_json::Value> =
        std::cell::RefCell::new(serde_json::Value::Object(Default::default()));
}

/// worker 事件处理（equiv monitorThread.on("message") 的 UI 侧分支）。
fn handle_worker_event(state: &Rc<RefCell<PanelState>>, event: &Value) {
    let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match event_type {
        "refreshMonitors" => {
            if let Some(monitors) = event.get("monitors") {
                if monitors.is_object() {
                    let mut st = state.borrow_mut();
                    st.monitors = monitors.clone();
                    st.is_refreshing = false;
                }
            }
        }
        "monitorsEnriched" => {
            if event.get("failed").and_then(|v| v.as_bool()) != Some(true) {
                if let Some(monitors) = event.get("monitors") {
                    if monitors.is_object() {
                        state.borrow_mut().monitors = monitors.clone();
                    }
                }
            }
        }
        "settings" => {
            if let Some(settings) = event.get("settings") {
                let mut st = state.borrow_mut();
                st.settings = settings.clone();
                st.linked_levels = settings
                    .get("linkedLevelsActive")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
            }
        }
        _ => {}
    }
}

/// 把队列里的命令真正发给 worker。
fn flush_pending(state: &Rc<RefCell<PanelState>>, host: &Arc<MonitorHost>) {
    let mut st = state.borrow_mut();
    let brightness: Vec<_> = st.pending_brightness.drain(..).collect();
    let sdr: Vec<_> = st.pending_sdr.drain(..).collect();
    let gamma: Vec<_> = st.pending_gamma.drain(..).collect();
    let vcp: Vec<_> = st.pending_vcp.drain(..).collect();
    let sleep: Vec<_> = st.pending_sleep_ddc.drain(..).collect();
    drop(st);

    for (id, level) in brightness {
        // 空 id = WMI 全局写入（equiv 原版不传 id 的 send）
        if id.is_empty() {
            host.send(json!({"type": "brightness", "brightness": level}));
        } else {
            host.send(json!({"type": "brightness", "brightness": level, "id": id}));
        }
    }
    for (id, level) in sdr {
        host.send(json!({"type": "sdr", "brightness": level, "id": id}));
    }
    for (id, level) in gamma {
        host.send(json!({"type": "gamma", "brightness": level, "id": id}));
    }
    for (monitor, code, value) in vcp {
        host.send(json!({"type": "vcp", "monitor": monitor, "code": code, "value": value}));
    }
    for hwid in sleep {
        host.send(json!({"type": "vcp", "monitor": hwid, "code": 0xD6, "value": 5}));
    }
}

/// 刷新 UI model（equiv setState -> React 重渲染）。
///
/// 列表模型只创建一次，之后按行增量更新：等价 React 用 key 复用 DOM 节点。
/// 每帧整表重建会销毁正在拖动的滑块（TouchArea 的 pressed 丢失）和正在编辑的
/// 数值框（LineEdit 失焦），表现为"滑块一拖就跳回/根本拖不动"。
fn refresh_ui(ui: &crate::PanelWindow, state: &Rc<RefCell<PanelState>>) {
    let mut st = state.borrow_mut();
    let (rows, count) = monitor_view::build_panel_model(
        &st.monitors,
        &st.settings,
        st.linked_levels,
    );

    match st.panel_model.clone() {
        Some(model) => {
            // 行数对齐（减少时从尾部裁掉）
            while model.row_count() > rows.len() {
                model.remove(model.row_count() - 1);
            }
            for (index, row) in rows.iter().enumerate() {
                let unchanged = model
                    .row_data(index)
                    .map(|old| panel_monitor_equal(&old, row))
                    .unwrap_or(false);
                if unchanged {
                    continue;
                }
                if index < model.row_count() {
                    model.set_row_data(index, row.clone());
                } else {
                    model.push(row.clone());
                }
            }
        }
        None => {
            let model = Rc::new(VecModel::from(rows));
            ui.set_monitors(ModelRc::new(model.clone()));
            st.panel_model = Some(model);
        }
    }

    ui.set_visible_count(count);
    ui.set_linked_levels(st.linked_levels);
    ui.set_linked_levels_available(count > 1);
    ui.set_sleeping(st.sleeping);
    ui.set_is_refreshing(st.is_refreshing);
    ui.set_show_sleep(
        st.settings
            .get("sleepAction")
            .and_then(|v| v.as_str())
            .map(|v| v != "none")
            .unwrap_or(true),
    );
    ui.set_scroll_amount(
        st.settings
            .get("scrollFlyoutAmount")
            .and_then(|v| v.as_f64())
            .unwrap_or(2.0) as f32,
    );
}

/// 两行渲染数据是否等价（用于增量更新判定）。
/// Slint 生成的 struct 内部含 ModelRc（features/inputs），其 PartialEq 按模型身份比较，
/// 每次构建都是新模型，因此这里逐字段 + 逐特征行比较。
fn panel_monitor_equal(a: &PanelMonitor, b: &PanelMonitor) -> bool {
    if a.key != b.key
        || a.id != b.id
        || a.name != b.name
        || a.display_type != b.display_type
        || a.brightness != b.brightness
        || a.minimum != b.minimum
        || a.maximum != b.maximum
        || a.num != b.num
        || a.adjustable != b.adjustable
        || a.show_power != b.show_power
        || a.power_on != b.power_on
        || a.hdr_only_sdr != b.hdr_only_sdr
        || a.extended != b.extended
    {
        return false;
    }
    if a.features.row_count() != b.features.row_count() {
        return false;
    }
    for index in 0..a.features.row_count() {
        let (Some(fa), Some(fb)) = (a.features.row_data(index), b.features.row_data(index)) else {
            return false;
        };
        if fa.code != fb.code
            || fa.kind != fb.kind
            || fa.level != fb.level
            || fa.max != fb.max
            || fa.icon != fb.icon
            || fa.icon_text != fb.icon_text
            || fa.current_input != fb.current_input
            || fa.inputs.row_count() != fb.inputs.row_count()
        {
            return false;
        }
        for i in 0..fa.inputs.row_count() {
            let (Some(ia), Some(ib)) = (fa.inputs.row_data(i), fb.inputs.row_data(i)) else {
                return false;
            };
            if ia.value != ib.value || ia.label != ib.label {
                return false;
            }
        }
    }
    true
}

/// 亮度闭环对拍（--brightness-smoke 用）：
/// 走 UI 回调 -> request_brightness -> 节流 -> worker 的完整链路，
/// 再读回 worker 的真实亮度，验证主进程侧决策没有把值改错。
/// 返回 (原始亮度, 设定后读回值)。
pub fn brightness_roundtrip_smoke(
    host: Arc<MonitorHost>,
    settings: Arc<Mutex<Value>>,
    target_level: f64,
) -> Result<(f64, f64), String> {
    // 1) 全量刷新，拿到显示器列表
    host.refresh_monitors(true, true);
    let start = Instant::now();
    let mut monitors = json!({});
    while start.elapsed().as_millis() < 30_000 {
        let snapshot = host.monitors_snapshot();
        if snapshot.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
            monitors = snapshot;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let Some((key, monitor)) = monitors
        .as_object()
        .and_then(|o| o.iter().next().map(|(k, v)| (k.clone(), v.clone())))
    else {
        return Err("no monitors enumerated".to_string());
    };
    let original = monitor
        .get("brightness")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| "monitor has no brightness reading".to_string())?;

    // 2) 用 UI 状态 + 节流器走真实决策路径
    let settings_snapshot = settings.lock().map(|g| g.clone()).unwrap_or(json!({}));
    let state = Rc::new(RefCell::new(PanelState {
        monitors: monitors.clone(),
        settings: settings_snapshot,
        ..Default::default()
    }));
    let throttle = Rc::new(RefCell::new(BrightnessThrottle::new()));

    request_brightness(&state, &throttle, &key, target_level, "brightness");

    // 3) 冲刷节流队列（复用与事件循环相同的 flush 路径）
    flush_pending(&state, &host);
    let _ = &state;

    // 3b) 节流验证（equiv updateBrightnessThrottle）：间隔内连发的中间值应被丢弃，
    // 只有最后一个值落地。这里连发 target+10 / target+20，最终应读到 target+20。
    let burst = (target_level + 10.0).min(100.0);
    let burst2 = (target_level + 20.0).min(100.0);
    request_brightness(&state, &throttle, &key, burst, "brightness");
    request_brightness(&state, &throttle, &key, burst2, "brightness");
    flush_pending(&state, &host);
    // 排队值在 updateInterval 后到期，由 take_due 取出
    let interval = settings
        .lock()
        .map(|g| g.get("updateInterval").and_then(|v| v.as_u64()).unwrap_or(500))
        .unwrap_or(500);
    std::thread::sleep(Duration::from_millis(interval + 50));
    let due = throttle.borrow_mut().take_due(Instant::now());
    {
        let mut st = state.borrow_mut();
        for (id, level, target) in due {
            enqueue_write(&mut st, &id, level, &target);
        }
    }
    flush_pending(&state, &host);

    // 3b) 节流验证（equiv updateBrightnessThrottle）：间隔内连发的中间值应被丢弃，
    // 只有最后一个值落地。这里连发 target+10 / target+20，最终应读到 target+20。
    let burst = (target_level + 10.0).min(100.0);
    let burst2 = (target_level + 20.0).min(100.0);
    request_brightness(&state, &throttle, &key, burst, "brightness");
    request_brightness(&state, &throttle, &key, burst2, "brightness");
    flush_pending(&state, &host);
    let interval = settings
        .lock()
        .map(|g| g.get("updateInterval").and_then(|v| v.as_u64()).unwrap_or(500))
        .unwrap_or(500);
    std::thread::sleep(Duration::from_millis(interval + 50));
    let due = throttle.borrow_mut().take_due(Instant::now());
    {
        let mut st = state.borrow_mut();
        for (id, level, target) in due {
            enqueue_write(&mut st, &id, level, &target);
        }
    }
    flush_pending(&state, &host);

    // 4) 等 worker 落地后读回（等价 readKnownBrightness / refreshMonitors）
    std::thread::sleep(Duration::from_millis(1200));
    host.refresh_monitors(false, true);
    let readback_start = Instant::now();
    let mut after = original;
    while readback_start.elapsed().as_millis() < 15_000 {
        let snapshot = host.monitors_snapshot();
        if let Some(level) = snapshot
            .get(&key)
            .and_then(|m| m.get("brightness"))
            .and_then(|v| v.as_f64())
        {
            after = level;
            // 期望值 = 节流后最后落地的 burst2
            if (after - burst2).abs() < 1.5 {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    Ok((original, after))
}

/// 节流期望值（--brightness-smoke 报告用）。
pub fn throttle_expected(target_level: f64) -> f64 {
    (target_level + 20.0).min(100.0)
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_writes_immediately_then_queues() {
        let mut t = BrightnessThrottle::new();
        assert!(t.request("m1", 10.0, "brightness", 500).is_some());
        assert!(t.request("m1", 20.0, "brightness", 500).is_none());
        // 不同 VCP 目标独立计时（equiv "Each target has its own timer"）
        assert!(t.request("m1", 30.0, "0x62", 500).is_some());
    }

    #[test]
    fn throttle_keeps_latest_value() {
        let mut t = BrightnessThrottle::new();
        t.request("m1", 10.0, "brightness", 500);
        t.request("m1", 20.0, "brightness", 500);
        t.request("m1", 30.0, "brightness", 500);
        let due = t.take_due(Instant::now() + Duration::from_secs(2));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].1, 30.0);
    }

    #[test]
    fn vcp_key_normalization() {
        assert_eq!(BrightnessThrottle::vcp_key("brightness"), "brightness");
        assert_eq!(BrightnessThrottle::vcp_key("volume-raw"), "0x62");
        assert_eq!(BrightnessThrottle::vcp_key("18"), "0x12");
        assert_eq!(BrightnessThrottle::vcp_key("0x12"), "0x12");
    }

    #[test]
    fn gamma_level_mapping_matches_js() {
        // sliderLevelToGammaLevel：GAMMA_BRIGHTNESS_MIN=20
        let mut st = PanelState::default();
        enqueue_write(&mut st, "m1", 0.0, "gamma");
        enqueue_write(&mut st, "m1", 50.0, "gamma");
        enqueue_write(&mut st, "m1", 100.0, "gamma");
        assert_eq!(st.pending_gamma[0].1, 20.0);
        assert_eq!(st.pending_gamma[1].1, 60.0);
        assert_eq!(st.pending_gamma[2].1, 100.0);
    }
}