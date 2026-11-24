//! 设置窗口宿主：创建窗口、接线回调、把改动写回 settings 并下发 worker。
//!
//! equiv: origin/src/settings.js + electron.js 的 createSettings（L3645）
//! + ipcMain.on('send-settings')（L2367-L2374）+ writeSettings/processSettings
//! （L1221-L1471）的 UI 可达子集。
//!
//! 线程模型（AGENTS.md §5）：Slint 窗口只在主线程（事件循环线程）创建，
//! 与面板共享同一个 Slint 后端与事件循环——等价 Electron 的多 BrowserWindow。

use crate::settings_view;
use crate::window_state;
use serde_json::{json, Value};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// 设置窗口句柄（调用方需持有，drop 即销毁窗口）。
pub struct SettingsWindowHandle {
    pub ui: crate::SettingsWindow,
    /// 显示器列表模型的单实例（首帧注入，之后按行增量更新）。
    /// 等价 React 的 key 复用：整表重建会让正在编辑的重命名框失焦。
    pub monitors_model: Rc<VecModel<crate::ui_types::SettingsMonitorEntry>>,
    /// 上次注入的 worker monitors 快照（用于判重，避免每帧重建模型）。
    pub last_monitors: Mutex<Value>,
    /// 显示器相关设置（remaps/hideDisplays/names/...）已改动，下一 tick 强制刷新列表。
    pub needs_monitor_refresh: std::cell::Cell<bool>,
}

thread_local! {
    /// 设置窗口句柄（主线程独占；Slint 组件不可跨线程）。
    static SETTINGS_WINDOW: std::cell::RefCell<Option<SettingsWindowHandle>> =
        const { std::cell::RefCell::new(None) };
}

/// 设置窗口槽位（等价原版 Electron 的单一 settingsBrowserWindow 引用）。
pub fn settings_window_slot() -> &'static std::cell::RefCell<Option<SettingsWindowHandle>> {
    SETTINGS_WINDOW.with(|slot| unsafe { &*(slot as *const std::cell::RefCell<_>) })
}

/// 标记显示器数据脏（设置窗口内改动 hide/gamma/SDR/remap 等后调用）。
pub fn mark_monitors_dirty() {
    if let Some(handle) = settings_window_slot().borrow().as_ref() {
        handle.needs_monitor_refresh.set(true);
    }
}

/// 在当前（主）线程创建并显示设置窗口。
/// settings 为共享的可写设置；worker 为下发通道（可为 None 的纯预览模式）。
pub fn open_settings_window(
    settings: Arc<Mutex<Value>>,
    host: Arc<crate::monitor_host::MonitorHost>,
) -> Result<SettingsWindowHandle, String> {
    let ui = crate::SettingsWindow::new().map_err(|e| e.to_string())?;
    let snapshot = settings.lock().map(|g| g.clone()).unwrap_or(json!({}));

    window_state::apply_settings_theme(&ui, &snapshot);
    ui.set_pages(settings_view::model(settings_view::build_pages()));
    ui.set_hotkeys(settings_view::model(settings_view::build_hotkeys(&snapshot)));
    ui.set_items(settings_view::model(settings_view::build_items(
        "general",
        &snapshot,
    )));
    ui.set_items_after(settings_view::model(settings_view::build_post_items(
        "general",
        &snapshot,
    )));

    // 显示器列表：打开即注入主进程缓存（equiv SettingsWindow componentDidMount 时
    // this.state.monitors 已由主进程填充）。此前漏注入 → monitors 模型恒为空
    // → 「显示器设置」页整页没有内容（本次修复的核心）。
    let monitors = host.monitors_snapshot();
    let monitors_model = Rc::new(VecModel::from(settings_view::build_monitor_entries(
        &monitors,
        &snapshot,
    )));
    ui.set_monitors(ModelRc::new(monitors_model.clone()));

    apply_texts(&ui);

    wire_callbacks(&ui, settings.clone(), host.clone());
    ui.show().map_err(|e| e.to_string())?;

    // 打开设置即请求一次显示器刷新（equiv getSidebar onClick 的 window.requestMonitors()）
    host.refresh_monitors(false, false);

    // 设置窗口打开即检查更新（equiv SettingsWindow componentDidMount L211 window.checkForUpdates()）
    let snapshot = settings.lock().map(|g| g.clone()).unwrap_or(json!({}));
    crate::update_host::check_for_updates(&snapshot, false, update_path());

    Ok(SettingsWindowHandle {
        ui,
        monitors_model,
        last_monitors: Mutex::new(monitors),
        needs_monitor_refresh: std::cell::Cell::new(false),
    })
}

/// 重新注入显示器列表与热键（settings/monitors 更新时调用）。
pub fn refresh_settings_view(handle: &SettingsWindowHandle, settings: &Value, monitors: &Value) {
    let ui = &handle.ui;
    refresh_settings_monitors(handle, settings, monitors);
    ui.set_hotkeys(settings_view::model(settings_view::build_hotkeys(settings)));
    let page = ui.get_active_page().to_string();
    ui.set_items(settings_view::model(settings_view::build_items(
        page.as_str(),
        settings,
    )));
    ui.set_items_after(settings_view::model(settings_view::build_post_items(
        page.as_str(),
        settings,
    )));
    window_state::apply_settings_theme(ui, settings);
}

/// 刷新设置窗口的显示器列表（equiv 原版 `monitorsUpdated` -> setState({monitors})）。
///
/// 只在 worker monitors 快照变化时重建模型，且按行增量更新：
/// 整表重建会销毁正在编辑的重命名输入框（与面板 Round 10 的教训同源）。
pub fn refresh_settings_monitors(
    handle: &SettingsWindowHandle,
    settings: &Value,
    monitors: &Value,
) {
    let dirty = handle.needs_monitor_refresh.replace(false);
    let changed = {
        let mut last = match handle.last_monitors.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        if *last == *monitors {
            false
        } else {
            *last = monitors.clone();
            true
        }
    };
    if !changed && !dirty {
        return;
    }

    let entries = settings_view::build_monitor_entries(monitors, settings);
    let model = &handle.monitors_model;
    while model.row_count() > entries.len() {
        model.remove(model.row_count() - 1);
    }
    for (index, entry) in entries.into_iter().enumerate() {
        let unchanged = model
            .row_data(index)
            .map(|old| settings_monitor_equal(&old, &entry))
            .unwrap_or(false);
        if unchanged {
            continue;
        }
        if index < model.row_count() {
            model.set_row_data(index, entry);
        } else {
            model.push(entry);
        }
    }
}

/// 两行显示器条目是否等价（用于增量更新判定）。
fn settings_monitor_equal(
    a: &crate::ui_types::SettingsMonitorEntry,
    b: &crate::ui_types::SettingsMonitorEntry,
) -> bool {
    a.key == b.key
        && a.id == b.id
        && a.name == b.name
        && a.title == b.title
        && a.rename == b.rename
        && a.hidden == b.hidden
        && a.gamma == b.gamma
        && a.sdr == b.sdr
        && a.extend_minimum == b.extend_minimum
        && a.breakpoint == b.breakpoint
        && a.connection == b.connection
        && a.hdr == b.hdr
        && a.brightness == b.brightness
        && a.maximum == b.maximum
        && a.normalization == b.normalization
        && a.details == b.details
}

/// 接线全部设置窗口回调。
fn wire_callbacks(
    ui: &crate::SettingsWindow,
    settings: Arc<Mutex<Value>>,
    host: Arc<crate::monitor_host::MonitorHost>,
) {
    // 页面切换 -> 重建该页 items（equiv activePage setState + 渲染分支）
    let weak = ui.as_weak();
    let snap = settings.clone();
    ui.on_page_changed(move |page| {
        let Some(ui) = weak.upgrade() else { return };
        let current = snap.lock().map(|g| g.clone()).unwrap_or(json!({}));
        ui.set_active_page(page.clone());
        ui.set_items(settings_view::model(settings_view::build_items(
            &page, &current,
        )));
        ui.set_items_after(settings_view::model(settings_view::build_post_items(
            &page, &current,
        )));
    });

    // 开关 -> settings 写回 + 下发 worker（equiv sendSettings -> writeSettings -> processSettings）
    let weak = ui.as_weak();
    let snap = settings.clone();
    let worker = host.clone();
    ui.on_item_toggle(move |key, value| {
        let Some(ui) = weak.upgrade() else { return };
        let key = key.to_string();
        let updated = update_setting(&snap, &key, json!(value));
        apply_side_effects(&worker, &key, &updated);
        let page = ui.get_active_page().to_string();
        ui.set_items(settings_view::model(settings_view::build_items(
            &page, &updated,
        )));
        ui.set_items_after(settings_view::model(settings_view::build_post_items(
            &page, &updated,
        )));
    });

    // 下拉/数值 -> 写回（数值键按数字存，equiv target.value * 1）
    let weak = ui.as_weak();
    let snap = settings.clone();
    let worker = host.clone();
    ui.on_item_choice(move |key, value| {
        let Some(ui) = weak.upgrade() else { return };
        let key = key.to_string();
        // ComboBox selected 回调传显示文本：按 items 模型映射回选项值
        // （等价 <select> 的 e.target.value；数值/文本输入项无选项表，原样透传）
        let value = resolve_choice_label(&ui, &key, &value.to_string());
        let parsed: Value = match value.parse::<f64>() {
            Ok(n) if is_numeric_key(&key) => json!(n as i64),
            _ => json!(value),
        };
        let mut updated = update_setting(&snap, &key, parsed);

        // windowsStyle -> isWin11 映射（equiv L1345-L1355）
        if key == "windowsStyle" {
            let style = updated
                .get("windowsStyle")
                .and_then(|v| v.as_str())
                .unwrap_or("system");
            let is_win11 = match style {
                "win11" => true,
                "win10" => false,
                _ => monitor_thread_manager::is_windows_11(),
            };
            updated = update_setting(&snap, "isWin11", json!(is_win11));
        }

        // 语言切换（equiv L1277-L1280：getLocalization() + rebuildTray = true）：
        // 重载 FTL 语料 -> 重建侧边栏页名/窗口文案/面板文案 -> 重建托盘菜单文案
        if key == "language" {
            reload_language(&updated);
            ui.set_pages(settings_view::model(settings_view::build_pages()));
            apply_texts(&ui);
            crate::panel_controller::apply_panel_texts();
            rebuild_tray_menu();
        }

        // theme-settings 全窗口广播（equiv L1252-L1254：settings.theme 恒真，
        // 每次 processSettings 都发；windowsStyle 改 isWin11 后同样由此生效）
        window_state::apply_settings_theme(&ui, &updated);
        crate::panel_controller::apply_panel_theme(&updated);

        apply_side_effects(&worker, &key, &updated);
        // udpEnabled 开关（equiv L1326-L1330）：true 启动 / false 停止
        if key == "udpEnabled" {
            let enable = updated.get("udpEnabled").and_then(|v| v.as_bool());
            crate::udp_host::sync_udp_server(None, &snap, enable);
        }
        let page = ui.get_active_page().to_string();
        ui.set_items(settings_view::model(settings_view::build_items(
            &page, &updated,
        )));
        ui.set_items_after(settings_view::model(settings_view::build_post_items(
            &page, &updated,
        )));
    });

    // 按钮 -> 对应动作（equiv save-report / reset-settings / get-coordinates / ...）
    let worker = host.clone();
    let snap = settings.clone();
    ui.on_item_button(move |key| {
        handle_settings_button(&key, &snap, &worker);
    });

    // 显示器逐台控制（equiv monitors 页的 rename / hide / gamma / SDR / extend minimum / 排序）
    let snap = settings.clone();
    ui.on_monitor_renamed(move |key, name| {
        let mut guard = match snap.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let id = monitor_id_of(&guard, &key);
        if guard.get("names").is_none() {
            guard["names"] = json!({});
        }
        if let Some(names) = guard.get_mut("names").and_then(|v| v.as_object_mut()) {
            names.insert(id, json!(name.to_string()));
        }
    });
    let snap = settings.clone();
    ui.on_monitor_hidden(move |key, value| {
        set_monitor_flag(&snap, "hideDisplays", &key, value);
    });
    let snap = settings.clone();
    ui.on_monitor_gamma(move |key, value| {
        set_monitor_flag(&snap, "gammaAsMainSliderDisplays", &key, value);
    });
    let snap = settings.clone();
    ui.on_monitor_sdr(move |key, value| {
        set_monitor_flag(&snap, "sdrAsMainSliderDisplays", &key, value);
    });
    let snap = settings.clone();
    ui.on_monitor_extend_minimum(move |key, value| {
        set_monitor_flag(&snap, "extendMinimumDisplays", &key, value);
    });
    let snap = settings.clone();
    ui.on_monitor_breakpoint(move |key, value| {
        let mut guard = match snap.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if guard.get("extendMinimumBreakpoints").is_none() {
            guard["extendMinimumBreakpoints"] = json!({});
        }
        if let Some(bp) = guard
            .get_mut("extendMinimumBreakpoints")
            .and_then(|v| v.as_object_mut())
        {
            bp.insert(key.to_string(), json!(value));
        }
    });
    let snap = settings.clone();
    ui.on_monitor_moved(move |key, delta| {
        move_monitor(&snap, &key, delta);
    });

    // 亮度归一化（equiv minMaxChanged L276-L330）：monitor-id + "min"/"max" + 0-100
    let snap = settings.clone();
    let worker = host.clone();
    ui.on_monitor_remap(move |monitor_id, field, value| {
        let field = field.to_string();
        if field != "min" && field != "max" {
            return;
        }
        if let Some(updated) = update_remap(&snap, &monitor_id, &field, value as f64) {
            apply_side_effects(&worker, "remaps", &updated);
            mark_monitors_dirty();
        }
    });

    // 校准点增删改（equiv addCalibrationPoint / updateCalibrationPoint /
    // deleteCalibrationPoint L536-L570）
    let snap = settings.clone();
    let worker = host.clone();
    ui.on_calibration_added(move |monitor_id| {
        if let Some(updated) = with_calibration(&snap, &monitor_id, |points| {
            points.push(json!({"input": 0, "output": 100}));
        }) {
            apply_side_effects(&worker, "remaps", &updated);
            mark_monitors_dirty();
        }
    });

    let snap = settings.clone();
    let worker = host.clone();
    ui.on_calibration_changed(move |monitor_id, index, field, value| {
        let field = field.to_string();
        if field != "input" && field != "output" {
            return;
        }
        if let Some(updated) = with_calibration(&snap, &monitor_id, |points| {
            if let Some(point) = points.get_mut(index as usize) {
                if let Some(obj) = point.as_object_mut() {
                    obj.insert(field.clone(), json!(value));
                }
            }
        }) {
            apply_side_effects(&worker, "remaps", &updated);
            mark_monitors_dirty();
        }
    });

    let snap = settings.clone();
    let worker = host.clone();
    ui.on_calibration_removed(move |monitor_id, index| {
        if let Some(updated) = with_calibration(&snap, &monitor_id, |points| {
            let idx = index as usize;
            if idx < points.len() {
                points.remove(idx);
            }
        }) {
            apply_side_effects(&worker, "remaps", &updated);
            mark_monitors_dirty();
        }
    });

    // 热键（equiv addHotkey / deleteHotkey 的 window.sendSettings({hotkeys}) 全链）：
    // 写回 settings → 落盘调度 → 下发 worker → 重新注册热键（equiv processSettings
    // L1272-L1274 的 newSettings.hotkeys !== undefined -> applyHotkeys）→ 刷新 UI 列表
    // （equiv applyHotkeys 末尾 sendToAllWindows('settings-updated')）。
    let snap = settings.clone();
    let worker = host.clone();
    let weak = ui.as_weak();
    ui.on_hotkey_add(move || {
        if let Some(updated) = add_hotkey(&snap) {
            apply_side_effects(&worker, "hotkeys", &updated);
            schedule_write_settings(&snap);
            crate::panel_controller::reapply_hotkeys(&updated);
            refresh_hotkey_models(&weak, &updated);
        }
    });
    let snap = settings.clone();
    let worker = host.clone();
    let weak = ui.as_weak();
    ui.on_hotkey_remove(move |id| {
        if let Some(updated) = remove_hotkey(&snap, &id) {
            apply_side_effects(&worker, "hotkeys", &updated);
            schedule_write_settings(&snap);
            crate::panel_controller::reapply_hotkeys(&updated);
            refresh_hotkey_models(&weak, &updated);
        }
    });
    ui.on_hotkey_record(move |_id| {
        // 录制由低级键盘钩子驱动（RegisterHotKey 只做注册，录制需 WH_KEYBOARD_LL）
    });

    // 窗口控制（equiv windowMinimize / windowToggleMaximize / windowClose）
    let weak = ui.as_weak();
    ui.on_window_minimize(move || {
        if let Some(ui) = weak.upgrade() {
            ui.window().set_minimized(true);
        }
    });
    let weak = ui.as_weak();
    ui.on_window_maximize(move || {
        if let Some(ui) = weak.upgrade() {
            let maximized = ui.window().is_maximized();
            ui.window().set_maximized(!maximized);
        }
    });
    let weak = ui.as_weak();
    ui.on_window_close(move || {
        if let Some(ui) = weak.upgrade() {
            let _ = ui.hide();
        }
    });
}

/// writeSettings 的磁盘写盘调度（equiv electron.js:L1224-L1233 的 333ms 防抖）。
///
/// 原版语义：writeSettingsTimeout 为空时排期一次 333ms 定时器，期间多次
/// writeSettings 不重置定时器；到期时把当时全局 settings 对象落盘。
/// Rust 等价：防抖标志置位期间不再排期，定时线程到期时读取最新 settings。
///
/// 线程模型：定时线程为短命线程，仅做一次 fs::write；异常走降级日志，
/// 不 panic（AGENTS.md §5）。
static WRITE_DEBOUNCE: std::sync::Mutex<Option<std::path::PathBuf>> =
    std::sync::Mutex::new(None);

/// settings 落盘路径：TT_SETTINGS_PATH（main 启动时写入）优先，
/// 否则退回 TT_CONFIG_DIR/settings.json。
fn settings_write_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("TT_SETTINGS_PATH") {
        return std::path::PathBuf::from(p);
    }
    std::env::var("TT_CONFIG_DIR")
        .map(|c| std::path::PathBuf::from(c).join("settings.json"))
        .unwrap_or_else(|_| std::path::PathBuf::from("settings.json"))
}

/// 排期一次 settings 落盘（333ms 防抖，等价原版 writeSettings 的写盘分支）。
pub fn schedule_write_settings(settings: &Arc<Mutex<Value>>) {
    let mut state = WRITE_DEBOUNCE.lock().unwrap_or_else(|e| e.into_inner());
    if state.is_some() {
        // 防抖中：等价原版 if (!writeSettingsTimeout) 的否定分支，不重置定时器
        return;
    }
    let path = settings_write_path();
    *state = Some(path.clone());
    drop(state);

    let settings = settings.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(333));
        // 核对防抖标志仍属于本次排期（防抖期间可能已由 stop 取消的场景）
        let mut st = WRITE_DEBOUNCE.lock().unwrap_or_else(|e| e.into_inner());
        let still_active = matches!(&*st, Some(p) if *p == path);
        if !still_active {
            return;
        }
        *st = None;
        let snapshot = settings.lock().map(|g| g.clone()).unwrap_or(json!({}));
        if let Err(e) = monitor_thread_manager::write_settings_file(&path, &snapshot) {
            // equiv debug.error("Couldn't save settings.", settingsPath, e)
            crate::logger::log_as("MAIN", "error", &format!("Couldn't save settings: {}", e));
        }
    });
}

/// 写入单个设置键并返回新快照。
fn update_setting(settings: &Arc<Mutex<Value>>, key: &str, value: Value) -> Value {
    let mut guard = match settings.lock() {
        Ok(g) => g,
        Err(_) => return json!({}),
    };
    if let Some(obj) = guard.as_object_mut() {
        obj.insert(key.to_string(), value);
    }
    let updated = guard.clone();
    // equiv writeSettings（L1221-L1237）：内存合并后按 333ms 防抖落盘
    schedule_write_settings(settings);
    updated
}

/// processSettings 的 UI 可达副作用（equiv L1240-L1471）：
/// 影响 worker 的键立即重下发 settings + ddcBrightnessVCPs；影响检测的键触发重扫。
fn apply_side_effects(host: &Arc<crate::monitor_host::MonitorHost>, key: &str, settings: &Value) {
    // 全量下发（equiv L1454-L1463：settings 与 ddcBrightnessVCPs 总是跟随）
    host.send(json!({"type": "settings", "settings": settings}));
    host.send(json!({
        "type": "ddcBrightnessVCPs",
        "ddcBrightnessVCPs": crate::monitor_host::ddc_brightness_vcps(settings),
    }));

    // 需要重扫的键（equiv L1282-L1397 的 shouldRefreshMonitors 分支）
    let refresh = matches!(
        key,
        "monitorFeatures"
            | "userDDCBrightnessVCPs"
            | "disableDDCDisplays"
            | "useSoftwareBrightnessFallback"
            | "disableWMI"
            | "disableWMIC"
            | "disableWin32"
            | "disableAppleStudio"
            | "gammaAsMainSliderDisplays"
            | "extendMinimumDisplays"
    );
    if refresh {
        host.refresh_monitors(true, true);
    } else if key == "extendMinimumBreakpoints" {
        // 只影响滑块映射，读回亮度即可（equiv L1404-L1408）
        host.refresh_monitors(false, true);
    }

    // checkForUpdates 开关（equiv L1376-L1383）
    if key == "checkForUpdates" {
        let enabled = settings.get("checkForUpdates").and_then(|v| v.as_bool()).unwrap_or(true);
        if enabled {
            // 开启:lastCheck=false 允许重新检查（Rust 侧直接触发一次非强制检查）
            crate::update_host::check_for_updates(settings, false, update_path());
        } else {
            // 关闭:latestVersion=false 清 UI
            crate::update_host::clear_update();
        }
    }
}

/// 托盘菜单重建钩子（menu 构建函数在 bin 侧 main.rs，lib 经此间接调用）。
static TRAY_MENU_REBUILDER: std::sync::Mutex<Option<fn() -> Vec<tray::TrayMenuItem>>> =
    std::sync::Mutex::new(None);

/// 注册托盘菜单重建函数（main.rs 启动时调用一次）。
pub fn set_tray_menu_rebuilder(f: fn() -> Vec<tray::TrayMenuItem>) {
    if let Ok(mut slot) = TRAY_MENU_REBUILDER.lock() {
        *slot = Some(f);
    }
}

/// 重建托盘菜单文案（equiv rebuildTray 的菜单部分：图标不重建）。
/// 菜单结构键（has_tod/idle_enabled/profiles/isDev）读 settings 文件快照，
/// 与语言无关；文案经 tt_localize 当前语言表即时生效。
fn rebuild_tray_menu() {
    let f = TRAY_MENU_REBUILDER.lock().ok().and_then(|g| *g);
    if let Some(f) = f {
        tray::update_tray_menu(f());
    }
}

/// 语言设置生效（equiv electron.js getLocalization L2241-L2291 的 desired 装载）：
/// "system" 按系统 locale 检测，否则按设置值；缺语料回退 en。
fn reload_language(settings: &Value) {
    let language = settings
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("system");
    let code = if language == "system" {
        tt_localize::detect_language_code(&window_state::system_locale())
    } else {
        language.to_string()
    };
    if tt_localize::language_source(&code).is_some() {
        tt_localize::set_language(&code);
    } else {
        tt_localize::set_language("en");
    }
}

/// 下拉显示文本 -> 选项值（equiv <select> 的 e.target.value）。
/// 在当前 items 模型里找同键且有选项表的项，按 labels 位置取 values；
/// 无匹配（数值/文本输入项原样透传，或未知文本）时返回原文。
fn resolve_choice_label(ui: &crate::SettingsWindow, key: &str, label: &str) -> String {
    use slint::Model;
    let items = ui.get_items();
    for i in 0..items.row_count() {
        let Some(item) = items.row_data(i) else { continue };
        if item.key.as_str() != key || item.option_values.row_count() == 0 {
            continue;
        }
        for j in 0..item.option_labels.row_count() {
            if item.option_labels.row_data(j).as_deref() == Some(label) {
                if let Some(v) = item.option_values.row_data(j) {
                    return v.to_string();
                }
            }
        }
    }
    label.to_string()
}

/// 数值型设置键（equiv 原版的 target.value * 1 语义）。
fn is_numeric_key(key: &str) -> bool {    matches!(
        key,
        "updateInterval"
            | "ddcPowerOffValue"
            | "hotkeyPercent"
            | "scrollShortcutAmount"
            | "scrollFlyoutAmount"
            | "detectIdleTimeSeconds"
            | "detectIdleTimeMinutes"
            | "idleRestoreSeconds"
            | "wakeRestoreSeconds"
            | "hardwareRestoreSeconds"
            | "checkVCPWaitMS"
            | "udpPortStart"
            | "adjustmentTimeLatitude"
            | "adjustmentTimeLongitude"
            | "sensorPollingInterval"
    )
}

/// 显示器键 -> id（names 表按 id 存，equiv monitorNameChange L346-L351）。
fn monitor_id_of(settings: &Value, key: &str) -> String {
    settings
        .get("lastKnownDisplays")
        .and_then(|v| v.get(key))
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or(key)
        .to_string()
}

/// 设置某显示器的布尔开关（按 key 存）。
fn set_monitor_flag(settings: &Arc<Mutex<Value>>, field: &str, key: &str, value: bool) {
    let mut guard = match settings.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if guard.get(field).is_none() {
        guard[field] = json!({});
    }
    if let Some(map) = guard.get_mut(field).and_then(|v| v.as_object_mut()) {
        map.insert(key.to_string(), json!(value));
    }
}

/// 亮度归一化 remap 写入（equiv minMaxChanged L276-L330 的 min/max 互锁）。
fn update_remap(
    settings: &Arc<Mutex<Value>>,
    monitor_id: &str,
    field: &str,
    value: f64,
) -> Option<Value> {
    let mut guard = settings.lock().ok()?;
    if guard.get("remaps").is_none() {
        guard["remaps"] = json!({});
    }
    let remaps = guard.get_mut("remaps")?.as_object_mut()?;
    let entry = remaps
        .entry(monitor_id.to_string())
        .or_insert_with(|| json!({"min": 0, "max": 100, "calibration": []}));
    if entry.get("calibration").is_none() {
        entry["calibration"] = json!([]);
    }
    let mut min = entry.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let mut max = entry.get("max").and_then(|v| v.as_f64()).unwrap_or(100.0);
    if field == "min" {
        // equiv L289-L304：min 上移时保持 max - min >= 10 且 max <= 100
        min = value;
        if min > max - 10.0 {
            max = min + 10.0;
        }
        if max > 100.0 {
            max = 100.0;
        }
        if min > max - 10.0 {
            min = max - 10.0;
        }
    } else {
        // equiv L306-L322：max 下移时保持 max - min >= 10 且 min >= 0
        max = value;
        if min > max - 10.0 {
            min = max - 10.0;
        }
        if min < 0.0 {
            min = 0.0;
        }
        if min > max - 10.0 {
            max = min + 10.0;
        }
    }
    entry["min"] = json!(min);
    entry["max"] = json!(max);
    let updated = guard.clone();
    drop(guard);
    schedule_write_settings(settings);
    Some(updated)
}

/// 校准点写回（add / update / delete 共用；equiv L536-L570）。
fn with_calibration<F>(settings: &Arc<Mutex<Value>>, monitor_id: &str, f: F) -> Option<Value>
where
    F: FnOnce(&mut Vec<Value>),
{
    let mut guard = settings.lock().ok()?;
    if guard.get("remaps").is_none() {
        guard["remaps"] = json!({});
    }
    let remaps = guard.get_mut("remaps")?.as_object_mut()?;
    let entry = remaps
        .entry(monitor_id.to_string())
        .or_insert_with(|| json!({"min": 0, "max": 100, "calibration": []}));
    if entry.get("calibration").is_none() {
        entry["calibration"] = json!([]);
    }
    let points = entry.get_mut("calibration")?.as_array_mut()?;
    f(points);
    let updated = guard.clone();
    drop(guard);
    schedule_write_settings(settings);
    Some(updated)
}

/// 显示器排序（equiv onDragEnd L228-L258：order 数组存 id）。
fn move_monitor(settings: &Arc<Mutex<Value>>, key: &str, delta: i32) {
    let mut guard = match settings.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let id = monitor_id_of(&guard, key);
    let mut order: Vec<String> = guard
        .get("order")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();
    let pos = match order.iter().position(|v| *v == id) {
        Some(p) => p,
        None => {
            order.push(id);
            guard["order"] = json!(order);
            return;
        }
    };
    let target = pos as i32 + delta;
    if target < 0 || target >= order.len() as i32 {
        return;
    }
    order.swap(pos, target as usize);
    guard["order"] = json!(order);
}

/// 新增热键（equiv addHotkey：accelerator 空，一条 offset 动作）。
/// 返回变更后的 settings 快照（供下发 worker / 重注册热键 / 刷新 UI）。
fn add_hotkey(settings: &Arc<Mutex<Value>>) -> Option<Value> {
    let mut guard = match settings.lock() {
        Ok(g) => g,
        Err(_) => return None,
    };
    let mut list = guard
        .get("hotkeys")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let percent = guard.get("hotkeyPercent").and_then(|v| v.as_i64()).unwrap_or(10);
    list.push(json!({
        "accelerator": "",
        "id": monitor_thread_manager::generate_uuid(),
        "actions": [{
            "monitors": {},
            "target": "brightness",
            "type": "offset",
            "values": [0],
            "value": percent,
            "allMonitors": true,
        }]
    }));
    guard["hotkeys"] = json!(list);
    Some(guard.clone())
}

/// 删除热键（equiv deleteHotkey）。返回变更后的 settings 快照。
fn remove_hotkey(settings: &Arc<Mutex<Value>>, id: &str) -> Option<Value> {
    let mut guard = match settings.lock() {
        Ok(g) => g,
        Err(_) => return None,
    };
    let list: Vec<Value> = guard
        .get("hotkeys")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter(|h| h.get("id").and_then(|v| v.as_str()) != Some(id))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    guard["hotkeys"] = json!(list);
    Some(guard.clone())
}

/// 热键增删后的 UI 刷新（equiv sendToAllWindows('settings-updated') 的
/// 设置窗口子集：热键列表 + 当前页设置项——原生亮度警告随列表条件变化）。
fn refresh_hotkey_models(weak: &slint::Weak<crate::SettingsWindow>, settings: &Value) {
    let Some(ui) = weak.upgrade() else { return };
    ui.set_hotkeys(settings_view::model(settings_view::build_hotkeys(settings)));
    let page = ui.get_active_page().to_string();
    ui.set_items(settings_view::model(settings_view::build_items(
        page.as_str(),
        settings,
    )));
    ui.set_items_after(settings_view::model(settings_view::build_post_items(
        page.as_str(),
        settings,
    )));
}

/// 按钮动作分发（equiv ipcMain.on 表 L3645-L3733）。
fn handle_settings_button(
    key: &str,
    settings: &Arc<Mutex<Value>>,
    host: &Arc<crate::monitor_host::MonitorHost>,
) {
    match key {
        "open-settings-file" => {
            if let Ok(path) = std::env::var("TT_SETTINGS_PATH") {
                let _ = std::process::Command::new("notepad.exe").arg(path).spawn();
            }
        }
        "reset-settings" => {
            // equiv L2389-L2395：settings = defaultSettings，清 lastKnownDisplays，写 userClosedIntro
            let mut guard = match settings.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            let defaults = monitor_thread_manager::default_settings(
                guard.get("isDev").and_then(|v| v.as_bool()).unwrap_or(false),
                guard.get("isWin11").and_then(|v| v.as_bool()).unwrap_or(true),
                crate::app::APP_VERSION,
                crate::app::APP_BUILD,
                None,
                &monitor_thread_manager::generate_uuid(),
                &monitor_thread_manager::generate_uuid(),
            );
            *guard = defaults;
            guard["userClosedIntro"] = json!(true);
            let snapshot = guard.clone();
            drop(guard);
            // equiv writeSettings（L1221-L1237）落盘调度
            schedule_write_settings(settings);
            host.send(json!({"type": "settings", "settings": snapshot}));
            host.refresh_monitors(true, true);
        }
        // debug 页 All Displays（equiv L1723：requestMonitors(true) / flush-vcp-cache）
        "refresh-monitors" => {
            host.refresh_monitors(true, true);
        }
        "clear-cache" => {
            // equiv ipc.send('flush-vcp-cache')：worker 侧清 vcp 缓存
            host.send(json!({"type": "flushvcp"}));
        }
        "save-report" => {
            // equiv L3733-L3740：getReport -> 写报告文件
            host.send(json!({"type": "getReport"}));
        }
        "check-for-updates" => {
            // equiv L5184-L5188:latestVersion.error=false + checkForUpdates(true)
            let snapshot = settings.lock().map(|g| g.clone()).unwrap_or(json!({}));
            crate::update_host::check_for_updates(&snapshot, true, update_path());
        }
        "get-coordinates" => {
            // equiv L5893 getAndApplyUserCoordinates：按 IP 定位后写入经纬度
            let snap = settings.clone();
            std::thread::spawn(move || {
                match crate::net::fetch_geo_coordinates() {
                    Some((lat, long)) => {
                        let mut guard = match snap.lock() {
                            Ok(g) => g,
                            Err(_) => return,
                        };
                        guard["adjustmentTimeLatitude"] = json!(lat.parse::<f64>().unwrap_or(0.0));
                        guard["adjustmentTimeLongitude"] = json!(long.parse::<f64>().unwrap_or(0.0));
                    }
                    None => {
                        println!("Couldn't get coordinates.");
                    }
                }
            });
        }
        _ => {}
    }
}

/// updatePath（equiv L114：configFilesDir/update.exe）。
fn update_path() -> std::path::PathBuf {
    std::env::var("TT_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join("update.exe")
}

/// 设置窗口文案。
fn apply_texts(ui: &crate::SettingsWindow) {
    ui.set_t_title(tt_localize::t("SETTINGS_TITLE").into());
    ui.set_t_monitors_rename(tt_localize::t("SETTINGS_MONITORS_ENTER_NAME").into());
    ui.set_t_monitors_hide(tt_localize::t("SETTINGS_MONITORS_HIDE_DISPLAYS_TITLE").into());
    ui.set_t_monitors_gamma(tt_localize::t("SETTINGS_MONITORS_GAMMA_SLIDER_TITLE").into());
    ui.set_t_monitors_sdr(tt_localize::t("SETTINGS_MONITORS_SDR_SLIDER_TITLE").into());
    ui.set_t_monitors_extend_minimum(tt_localize::t("SETTINGS_MONITORS_EXTEND_MINIMUM_TITLE").into());
    ui.set_t_hotkeys_add(tt_localize::t("SETTINGS_HOTKEYS_ADD").into());
    ui.set_t_hotkeys_remove(tt_localize::t("GENERIC_DELETE").into());
    ui.set_t_hotkeys_record(tt_localize::t("SETTINGS_HOTKEYS_PRESS_KEYS_HINT").into());
    // monitors 页：All Displays 标题与空态提示（equiv getInfoMonitors 的两条分支）
    ui.set_t_all_displays(tt_localize::t("GENERIC_ALL_DISPLAYS").into());
    ui.set_t_no_displays(tt_localize::t("GENERIC_NO_COMPATIBLE_DISPLAYS").into());
    // 亮度归一化段（equiv getMinMaxMonitors / getMonitorCalibration）
    ui.set_t_minimum(tt_localize::t("GENERIC_MINIMUM").into());
    ui.set_t_maximum(tt_localize::t("GENERIC_MAXIMUM").into());
    ui.set_t_calibration_point(tt_localize::t("GENERIC_CALIBRATION_POINT").into());
    ui.set_t_delete(tt_localize::t("GENERIC_DELETE").into());
    // 原版校准点标签为硬编码英文（L520-L525），无本地化键，照抄
    ui.set_t_calibration_input("Input".into());
    ui.set_t_calibration_output("Output".into());
}

/// 便捷入口：用默认 settings 打开（--settings-smoke 用）。
pub fn run_settings_window(_host: Arc<crate::monitor_host::MonitorHost>) -> Result<(), String> {
    let settings = Arc::new(Mutex::new(json!({})));
    let _handle = open_settings_window(settings, _host)?;
    slint::run_event_loop_until_quit().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_keys_typed() {
        assert!(is_numeric_key("updateInterval"));
        assert!(is_numeric_key("ddcPowerOffValue"));
        assert!(!is_numeric_key("theme"));
        assert!(!is_numeric_key("language"));
    }

    #[test]
    fn move_monitor_reorders_and_clamps() {
        let settings = Arc::new(Mutex::new(json!({
            "order": ["a", "b", "c"],
            "lastKnownDisplays": {
                "k1": {"id": "a"}, "k2": {"id": "b"}, "k3": {"id": "c"}
            }
        })));
        move_monitor(&settings, "k2", -1);
        assert_eq!(settings.lock().unwrap()["order"], json!(["b", "a", "c"]));
        move_monitor(&settings, "b", -1);
        assert_eq!(settings.lock().unwrap()["order"], json!(["b", "a", "c"]));
    }

    #[test]
    fn monitor_flag_set() {
        let settings = Arc::new(Mutex::new(json!({})));
        set_monitor_flag(&settings, "hideDisplays", "k1", true);
        assert_eq!(settings.lock().unwrap()["hideDisplays"]["k1"], json!(true));
        set_monitor_flag(&settings, "hideDisplays", "k1", false);
        assert_eq!(settings.lock().unwrap()["hideDisplays"]["k1"], json!(false));
    }

    #[test]
    fn hotkey_add_and_remove() {
        let settings = Arc::new(Mutex::new(json!({"hotkeys": [], "hotkeyPercent": 15})));
        let snapshot = add_hotkey(&settings).expect("add_hotkey returns snapshot");
        assert_eq!(snapshot["hotkeys"].as_array().unwrap().len(), 1);
        let list = settings.lock().unwrap()["hotkeys"].as_array().unwrap().len();
        assert_eq!(list, 1);
        let id = settings.lock().unwrap()["hotkeys"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let snapshot = remove_hotkey(&settings, &id).expect("remove_hotkey returns snapshot");
        assert_eq!(snapshot["hotkeys"].as_array().unwrap().len(), 0);
        assert_eq!(
            settings.lock().unwrap()["hotkeys"].as_array().unwrap().len(),
            0
        );
    }

    #[test]
    fn write_settings_file_serializes_with_tabs() {
        let dir = std::env::temp_dir().join("tt-settings-write-test");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("settings.json");
        let settings = json!({"theme": "dark", "updateInterval": 500});
        monitor_thread_manager::write_settings_file(&file, &settings).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        // 原版 JSON.stringify(settings, null, '\t')：tab 缩进 + 双空格
        assert!(text.starts_with('{'));
        assert!(text.contains("\t"));
        // 再读回等价性
        let back: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(back, settings);
        std::fs::remove_file(&file).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn schedule_write_settings_debounces() {
        // 用带进程号/时间戳的唯一目录，避免与其他测试共享同一路径
        // （TT_SETTINGS_PATH 与 WRITE_DEBOUNCE 都是进程级共享状态）
        let dir = std::env::temp_dir().join(format!(
            "tt-settings-debounce-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("settings.json");
        // 先等待前序测试的防抖落盘线程完成，再强制清空标志——
        // 若仅等待而不清空，残留排期会让本次 schedule 被静默忽略。
        let mut waited = 0;
        loop {
            let busy = WRITE_DEBOUNCE.lock().unwrap_or_else(|e| e.into_inner()).is_some();
            if !busy || waited >= 100 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            waited += 1;
        }
        // 清空残留防抖标志（等价"本测试独占该单例"的前置条件）
        {
            let mut st = WRITE_DEBOUNCE.lock().unwrap_or_else(|e| e.into_inner());
            *st = None;
        }
        std::env::set_var("TT_SETTINGS_PATH", &file);
        let settings = Arc::new(Mutex::new(json!({"a": 1})));
        // 连续两次排期：防抖语义（第二次被忽略，只有一次落盘线程）
        schedule_write_settings(&settings);
        schedule_write_settings(&settings);
        // 轮询等待 333ms 定时线程落盘完成（上限 5s）
        let mut polled = 0;
        while !file.exists() && polled < 50 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            polled += 1;
        }
        assert!(file.exists(), "settings file not written after {}ms", polled * 100);
        let back: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(back["a"], json!(1));
        std::env::remove_var("TT_SETTINGS_PATH");
        std::fs::remove_file(&file).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn update_remap_min_keeps_gap_and_caps_max() {
        // equiv minMaxChanged L289-L304：min 上移时 max = min + 10，且不超过 100
        let settings = Arc::new(Mutex::new(json!({})));
        let updated = update_remap(&settings, "DISPLAY1", "min", 95.0).unwrap();
        assert_eq!(updated["remaps"]["DISPLAY1"]["min"], json!(90.0));
        assert_eq!(updated["remaps"]["DISPLAY1"]["max"], json!(100.0));
    }

    #[test]
    fn update_remap_max_keeps_gap_and_caps_min() {
        // equiv L306-L322：max 下移时 min = max - 10，且不低于 0
        let settings = Arc::new(Mutex::new(json!({})));
        let updated = update_remap(&settings, "DISPLAY1", "max", 5.0).unwrap();
        assert_eq!(updated["remaps"]["DISPLAY1"]["max"], json!(10.0));
        assert_eq!(updated["remaps"]["DISPLAY1"]["min"], json!(0.0));
    }

    #[test]
    fn calibration_points_add_update_remove() {
        let settings = Arc::new(Mutex::new(json!({})));
        let updated = with_calibration(&settings, "DISPLAY1", |points| {
            points.push(json!({"input": 0, "output": 100}));
        })
        .unwrap();
        assert_eq!(updated["remaps"]["DISPLAY1"]["calibration"].as_array().unwrap().len(), 1);

        let updated = with_calibration(&settings, "DISPLAY1", |points| {
            points[0]["input"] = json!(25.0);
            points[0]["output"] = json!(40.0);
        })
        .unwrap();
        assert_eq!(updated["remaps"]["DISPLAY1"]["calibration"][0]["input"], json!(25.0));
        assert_eq!(updated["remaps"]["DISPLAY1"]["calibration"][0]["output"], json!(40.0));

        let updated = with_calibration(&settings, "DISPLAY1", |points| {
            points.remove(0);
        })
        .unwrap();
        assert!(updated["remaps"]["DISPLAY1"]["calibration"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn update_setting_returns_new_snapshot() {
        let dir = std::env::temp_dir().join("tt-settings-update-test");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("settings.json");
        std::env::set_var("TT_SETTINGS_PATH", &file);
        let settings = Arc::new(Mutex::new(json!({"theme": "default"})));
        let updated = update_setting(&settings, "theme", json!("dark"));
        assert_eq!(updated["theme"], json!("dark"));
        assert_eq!(settings.lock().unwrap()["theme"], json!("dark"));
        std::thread::sleep(std::time::Duration::from_millis(500));
        std::env::remove_var("TT_SETTINGS_PATH");
        std::fs::remove_file(&file).ok();
        std::fs::remove_dir_all(&dir).ok();
    }
}
