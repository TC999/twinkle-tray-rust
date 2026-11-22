//! Twinkle Tray Rust 主入口。
//!
//! 等价 electron.js 的进程模型：同二进制 --monitor-worker 子命令自调用
//! 替代 child_process.fork（AGENTS.md §3.1 等价替换）。
//!
//! 启动序列（equiv electron.js:L1-L130 / L862-L868 / L1201-L1214 / L4514+）：
//! 1. 解析开关；2. 解析路径并建配置目录；3. 读 settings + 版本迁移；
//! 4. 初始化会话日志；5. 拉起 monitor worker；6. 托盘/热键；7. 进入 Slint 事件循环。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use twinkle_tray_rust::app::{self, AppPaths, LaunchOptions};
use twinkle_tray_rust::logger;
use twinkle_tray_rust::monitor_host::MonitorHost;
use twinkle_tray_rust::worker_ipc;
use slint::{ComponentHandle, Model};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let opts = LaunchOptions::parse(&args);

    // 等价 electron.js:L351 fork(Monitors.js, [...])
    if opts.is_monitor_worker {
        worker_ipc::run_monitor_worker(&args);
        return;
    }

    // 日志镜像：dev 或 --console 时同时输出到 stdout（equiv L37）
    logger::set_mirror_log(opts.is_dev || opts.has_console);

    let exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| std::env::args().next().unwrap_or_default());
    let exe_dir = PathBuf::from(&exe_path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let user_data = app::default_user_data_dir();
    let paths = AppPaths::resolve(opts.is_dev, opts.is_portable, &user_data, &exe_dir);
    paths.ensure_config_dir();
    app::remove_old_debug_log(&paths);

    // readSettings：读取 + 版本迁移（equiv L1009-L1199）
    let settings = monitor_thread_manager::read_and_migrate_settings(
        &paths.settings_path,
        opts.is_dev,
        app::APP_VERSION,
    )
    .unwrap_or_else(|e| {
        eprintln!("Couldn't load settings: {}", e);
        monitor_thread_manager::default_settings(
            opts.is_dev,
            true,
            app::APP_VERSION,
            app::APP_BUILD,
            None,
            &monitor_thread_manager::generate_uuid(),
            &monitor_thread_manager::generate_uuid(),
        )
    });

    // initMainLogger（equiv L1206）
    let logging_enabled = settings
        .get("logging")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    app::init_logging(&paths, &opts, logging_enabled);

    // 语言装载（equiv getLocalization L2242-L2291）
    init_localization(&settings);

    let startup_flags = app::read_startup_flags(&settings);
    let _ = startup_flags;

    // 拉起 monitor worker（等价 startMonitorThread）
    let host = MonitorHost::new(exe_path.clone(), paths.user_data.to_string_lossy().to_string());
    host.install_handlers(settings.clone(), ddc_brightness_vcps(&settings));
    let started = host.start(true);
    if !started {
        logger::log_as("MAIN", "error", "Monitor thread could not be started.");
    }
    let bridge_ok = host.do_wmi_bridge_test();
    logger::log_as(
        "MAIN",
        "info",
        &format!("WMI bridge available: {}", bridge_ok),
    );

    // --app-smoke：启动序列自检后立即退出（不进入 UI 事件循环）
    if opts.app_smoke {
        run_app_smoke(&host, &settings, &paths);
        host.manager.set_quitting(true);
        host.manager.stop();
        return;
    }

    // --brightness-smoke：亮度闭环对拍（UI 回调 -> 节流 -> worker -> 读回）
    if let Some(level) = opts.brightness_smoke {
        let shared = Arc::new(Mutex::new(settings.clone()));
        match twinkle_tray_rust::panel_controller::brightness_roundtrip_smoke(
            host.clone(),
            shared,
            level,
        ) {
            Ok((original, after)) => {
                let expected = twinkle_tray_rust::panel_controller::throttle_expected(level);
                println!("=== brightness-smoke ===");
                println!("original: {}", original);
                println!("first target:  {}", level);
                println!("throttled expected: {}", expected);
                println!("readback: {}", after);
                let ok = (after - expected).abs() < 1.5;
                println!("result:   {}", if ok { "MATCH" } else { "MISMATCH" });
                // 恢复原值（对拍不留副作用）
                host.send(serde_json::json!({"type": "brightness", "brightness": original}));
                std::thread::sleep(std::time::Duration::from_millis(800));
                println!("restored to {}", original);
            }
            Err(e) => println!("brightness-smoke failed: {}", e),
        }
        host.manager.set_quitting(true);
        host.manager.stop();
        return;
    }

    // --ui-demo：显示面板/设置窗口的实景渲染（真机截图验证用）
    if opts.ui_demo {
        run_ui_demo(&host, &settings);
        host.manager.set_quitting(true);
        host.manager.stop();
        return;
    }

    // --ui-smoke：UI 模型 + 渲染自检（无交互，创建四窗口后立即退出）
    if opts.ui_smoke {
        run_ui_smoke(&host, &settings);
        host.manager.set_quitting(true);
        host.manager.stop();
        return;
    }

    let shared_settings = Arc::new(Mutex::new(settings.clone()));

    // 托盘图标（equiv createTray：图标随主题切换，§1.2 禁止自绘）
    let (tray_handle, tray_events) = start_tray(&settings, &exe_dir);

    // 全局热键（equiv applyHotkeys）
    start_hotkeys(&settings);

    // 主进程 UI（主线程 = Slint 事件循环，AGENTS.md §5）
    if let Err(e) = twinkle_tray_rust::run_panel(host.clone(), shared_settings, tray_events) {
        eprintln!("面板启动失败: {}", e);
    }

    if let Some(handle) = tray_handle {
        tray::remove_tray_icon(&handle);
    }
    tray::stop_tray_thread();
    host.manager.set_quitting(true);
    host.manager.stop();
}

/// getLocalization 的装载部分（equiv L2242-L2291）：
/// settings.language == "system" 时按系统 locale 检测，再按 FTL 语料装载。
fn init_localization(settings: &serde_json::Value) {
    let language = settings
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("system");
    let code = if language == "system" {
        tt_localize::detect_language_code(&system_locale())
    } else {
        language.to_string()
    };
    if tt_localize::language_source(&code).is_some() {
        tt_localize::set_language(&code);
    } else {
        // 缺语言文件时回退 en（equiv 读取失败只记录，不回退到别的语言表）
        tt_localize::set_language("en");
    }
    logger::log_as("MAIN", "info", &format!("Localization loaded: {}", code));
}

/// 系统 locale（equiv app.getLocale()）。
fn system_locale() -> String {
    use windows::Win32::Globalization::{GetUserDefaultLocaleName, MAX_LOCALE_NAME};
    // SAFETY: 缓冲区为固定长度的可写数组，API 只写入至多 MAX_LOCALE_NAME 个宽字符。
    unsafe {
        let mut buf = [0u16; MAX_LOCALE_NAME as usize];
        let len = GetUserDefaultLocaleName(&mut buf);
        if len > 0 {
            let end = buf.iter().position(|c| *c == 0).unwrap_or(len as usize);
            return String::from_utf16_lossy(&buf[..end]);
        }
    }
    "en-US".to_string()
}

/// 托盘图标（equiv createTray + getTrayIconPath）。
fn start_tray(
    settings: &serde_json::Value,
    exe_dir: &std::path::Path,
) -> (
    Option<tray::TrayIconHandle>,
    Option<std::sync::mpsc::Receiver<tray::TrayEvent>>,
) {
    let icon_name = settings
        .get("icon")
        .and_then(|v| v.as_str())
        .unwrap_or("icon");
    let dark = settings
        .get("theme")
        .and_then(|v| v.as_str())
        .map(|t| t == "dark")
        .unwrap_or(true);
    let dir = if dark { "dark" } else { "light" };
    let candidates = [
        exe_dir.join("assets").join("tray-icons").join(dir).join(format!("{}.ico", icon_name)),
        exe_dir
            .join("origin")
            .join("src")
            .join("assets")
            .join("tray-icons")
            .join(dir)
            .join(format!("{}.ico", icon_name)),
    ];
    let icon_path = match candidates.iter().find(|p| p.exists()) {
        Some(p) => p.to_string_lossy().to_string(),
        None => {
            logger::log_as("MAIN", "error", "Tray icon asset not found.");
            return (None, None);
        }
    };
    let config = tray::TrayConfig {
        icon_path,
        tooltip: "Twinkle Tray".to_string(),
        menu_items: build_tray_menu(),
    };
    // 托盘窗口/图标由专用线程创建与销毁（DestroyWindow 的同线程约束）
    let events = tray::spawn_tray_thread(config);
    logger::log_as("MAIN", "info", "Tray icon thread started.");
    (None, Some(events))
}

/// 托盘右键菜单（equiv electron.js setTrayMenu 的项集合，§1.2 自写 HMENU）。
/// 菜单 id 段位：1001+ 对应 electron 模板的 click 回调。
fn build_tray_menu() -> Vec<tray::TrayMenuItem> {
    let mut items = Vec::new();
    // 显示器亮度快捷项由 worker 枚举后动态补入（等价 setTrayMenu 的 monitors 段）
    items.push(tray::TrayMenuItem {
        id: 1001,
        label: tt_localize::t("PANEL_TITLE"),
        enabled: true,
    });
    items.push(tray::TrayMenuItem {
        id: 1002,
        label: tt_localize::t("GENERIC_SETTINGS"),
        enabled: true,
    });
    items.push(tray::TrayMenuItem {
        id: 1003,
        label: tt_localize::t("PANEL_BUTTON_TURN_OFF_DISPLAYS"),
        enabled: true,
    });
    items
}

/// 全局热键（equiv applyHotkeys）。
fn start_hotkeys(settings: &serde_json::Value) {
    let parsed = twinkle_tray_rust::hotkey_host::parse_hotkeys(settings);
    logger::log_as(
        "MAIN",
        "info",
        &format!("Hotkeys configured: {}", parsed.len()),
    );
}

/// getDDCBrightnessVCPs 等价（equiv electron.js:L2340-L2353）。
fn ddc_brightness_vcps(settings: &serde_json::Value) -> serde_json::Value {
    let mut merged = serde_json::json!({});
    if let Some(user) = settings.get("userDDCBrightnessVCPs").and_then(|v| v.as_object()) {
        for (k, v) in user {
            let parsed = match v {
                serde_json::Value::String(s) => u16::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok(),
                serde_json::Value::Number(n) => n.as_u64().map(|v| v as u16),
                _ => None,
            };
            if let Some(code) = parsed {
                if (1..=0xFF).contains(&code) {
                    merged[k] = serde_json::json!(code);
                }
            }
        }
    }
    merged
}

/// --ui-demo：真实渲染面板与设置窗口（供真机截图对拍）。
/// 注入 worker 真实枚举结果 + 一份演示用的扩展特征，覆盖两条渲染分支。
fn run_ui_demo(host: &std::sync::Arc<MonitorHost>, settings: &serde_json::Value) {
    host.refresh_monitors(true, true);
    let start = std::time::Instant::now();
    while start.elapsed().as_millis() < 30_000 {
        if host
            .monitors_snapshot()
            .as_object()
            .map(|o| !o.is_empty())
            .unwrap_or(false)
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let monitors = host.monitors_snapshot();

    // 演示用 settings：开启对比度/音量/输入源特征，覆盖 extended 分支
    let mut demo_settings = settings.clone();
    if let Some(first_key) = monitors.as_object().and_then(|o| o.keys().next()).cloned() {
        let hwid1 = monitors
            .get(&first_key)
            .and_then(|m| m.get("hwid"))
            .and_then(|v| v.as_array())
            .and_then(|a| a.get(1))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        demo_settings["monitorFeatures"] = serde_json::json!({
            hwid1.clone(): {"0x12": true, "0x62": true, "0x60": true, "0xD6": true}
        });
        demo_settings["monitorFeaturesSettings"] = serde_json::json!({
            hwid1.clone(): {"0x12": {"linked": false}, "0x62": {"linked": false}, "0x60": {"linked": false}, "0xD6": {"linked": false}}
        });
        // 注入演示特征（真实设备不一定暴露全部 VCP）
        if let Some(m) = monitors.get(&first_key).cloned() {
            let mut m = m;
            m["features"] = serde_json::json!({
                "0x10": [60, 100],
                "0x12": [50, 100],
                "0x62": [40, 100],
                "0x60": [15, [1, 3, 15, 17]],
                "0xD6": [1, 5]
            });
            demo_settings["names"] = serde_json::json!({});
            let mut demo_monitors = monitors.clone();
            demo_monitors[&first_key] = m;

            let panel = twinkle_tray_rust::PanelWindow::new().expect("panel window");
            twinkle_tray_rust::window_state::apply_theme(&panel, &demo_settings);
            panel.set_t_title(tt_localize::t("PANEL_TITLE").into());
            panel.set_t_settings(tt_localize::t("GENERIC_SETTINGS").into());
            panel.set_t_all_displays(tt_localize::t("GENERIC_ALL_DISPLAYS").into());
            panel.set_t_turn_on(tt_localize::t("PANEL_LABEL_TURN_ON").into());
            panel.set_t_turn_off(tt_localize::t("PANEL_LABEL_TURN_OFF").into());
            panel.set_t_no_displays(tt_localize::t("GENERIC_NO_COMPATIBLE_DISPLAYS").into());
            panel.set_linked_levels_available(true);
            let (models, visible) = twinkle_tray_rust::monitor_view::build_panel_model(
                &demo_monitors,
                &demo_settings,
                false,
            );
            println!("demo panel monitors: {} visible: {}", models.len(), visible);
            for m in &models {
                println!(
                    "  {} type={:?} level={} extended={} features={} power={}",
                    m.name, m.display_type, m.brightness, m.extended, m.features.row_count(), m.show_power
                );
            }
            panel.set_monitors(slint::ModelRc::new(slint::VecModel::from(models)));
            panel.set_visible_count(visible);
            let _ = panel.window().set_position(slint::PhysicalPosition::new(60, 60));
            let _ = panel.show();

            // 设置窗口：monitors 页
            if let Ok(ui) = twinkle_tray_rust::SettingsWindow::new() {
                twinkle_tray_rust::window_state::apply_settings_theme(&ui, &demo_settings);
                ui.set_pages(slint::ModelRc::new(slint::VecModel::from(
                    twinkle_tray_rust::settings_view::build_pages(),
                )));
                ui.set_monitors(slint::ModelRc::new(slint::VecModel::from(
                    twinkle_tray_rust::settings_view::build_monitor_entries(&demo_monitors, &demo_settings),
                )));
                ui.set_items(slint::ModelRc::new(slint::VecModel::from(
                    twinkle_tray_rust::settings_view::build_items("monitors", &demo_settings),
                )));
                ui.set_active_page("monitors".into());
                let _ = ui.window().set_position(slint::PhysicalPosition::new(440, 60));
                let _ = ui.show();
                println!("demo windows shown (panel + settings/monitors)");
                let _ = slint::run_event_loop_until_quit();
            }
        }
    }
}

/// --ui-smoke：Slint 四窗口创建 + 模型注入 + 渲染自检。
/// 验证：窗口可实例化（femtovg 后端可用）、monitor 模型注入、主题令牌生效、
/// 设置页 item 构建、intro/hud 文案装载。
fn run_ui_smoke(host: &std::sync::Arc<MonitorHost>, settings: &serde_json::Value) {
    println!("=== ui-smoke ===");
    host.refresh_monitors(true, true);
    let start = std::time::Instant::now();
    while start.elapsed().as_millis() < 30_000 {
        if host
            .monitors_snapshot()
            .as_object()
            .map(|o| !o.is_empty())
            .unwrap_or(false)
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let monitors = host.monitors_snapshot();

    // 1) 面板窗口 + 模型
    match twinkle_tray_rust::PanelWindow::new() {
        Ok(panel) => {
            twinkle_tray_rust::window_state::apply_theme(&panel, settings);
            let (models, visible) = twinkle_tray_rust::monitor_view::build_panel_model(
                &monitors,
                settings,
                false,
            );
            println!("panel window: created");
            println!("panel monitors: {} visible: {}", models.len(), visible);
            for m in &models {
                println!(
                    "  name={} key={} type={:?} level={} extended={} features={} power={}",
                    m.name,
                    m.key,
                    m.display_type,
                    m.brightness,
                    m.extended,
                    m.features.row_count(),
                    m.show_power
                );
            }
            panel.set_monitors(slint::ModelRc::new(slint::VecModel::from(models)));
            panel.set_visible_count(visible);
            panel.set_t_title(tt_localize::t("PANEL_TITLE").into());
            let theme = panel.global::<twinkle_tray_rust::Theme>();
            println!("theme dark={} win11={}", theme.get_dark(), theme.get_win11());
            println!("panel title: {}", panel.get_t_title());
        }
        Err(e) => println!("panel window FAILED: {}", e),
    }

    // 2) 设置窗口 + 各页 items
    match twinkle_tray_rust::SettingsWindow::new() {
        Ok(ui) => {
            twinkle_tray_rust::window_state::apply_settings_theme(&ui, settings);
            let pages = twinkle_tray_rust::settings_view::build_pages();
            println!("settings pages: {}", pages.len());
            for p in &pages {
                let items = twinkle_tray_rust::settings_view::build_items(&p.id, settings);
                println!("  page {} -> {} items", p.id, items.len());
            }
            let entries = twinkle_tray_rust::settings_view::build_monitor_entries(&monitors, settings);
            println!("settings monitor entries: {}", entries.len());
            for e in &entries {
                println!("  {} ({}) connection={} hdr={}", e.name, e.key, e.connection, e.hdr);
            }
            let hotkeys = twinkle_tray_rust::settings_view::build_hotkeys(settings);
            println!("settings hotkeys: {}", hotkeys.len());
            ui.set_pages(slint::ModelRc::new(slint::VecModel::from(pages)));
            ui.set_items(slint::ModelRc::new(slint::VecModel::from(
                twinkle_tray_rust::settings_view::build_items("general", settings),
            )));
        }
        Err(e) => println!("settings window FAILED: {}", e),
    }

    // 3) Intro / HUD 窗口
    match twinkle_tray_rust::IntroWindow::new() {
        Ok(intro) => {
            intro.set_t_title(tt_localize::t("INTRO_TITLE").into());
            intro.set_t_instructions(tt_localize::t("INTRO_INSTRUCTIONS").into());
            intro.set_t_close(tt_localize::t("GENERIC_CLOSE").into());
            println!("intro title: {}", intro.get_t_title());
        }
        Err(e) => println!("intro window FAILED: {}", e),
    }
    match twinkle_tray_rust::HudWindow::new() {
        Ok(hud) => {
            hud.set_level(50.0);
            hud.set_label(tt_localize::t("PANEL_LABEL_BRIGHTNESS").into());
            println!("hud label: {} level: {}", hud.get_label(), hud.get_level());
        }
        Err(e) => println!("hud window FAILED: {}", e),
    }

    // 4) 本地化：语言切换后文案应变化
    println!("locale: {} / {}", tt_localize::t("PANEL_TITLE"), tt_localize::t("GENERIC_OK"));
    tt_localize::set_language("zh-Hans");
    println!("zh-Hans: {} / {}", tt_localize::t("PANEL_TITLE"), tt_localize::t("GENERIC_OK"));
    println!("=== ui-smoke done ===");
}

/// --app-smoke：启动序列 e2e 自检。
fn run_app_smoke(host: &std::sync::Arc<MonitorHost>, settings: &serde_json::Value, paths: &AppPaths) {
    println!("=== app-smoke ===");
    println!("settings keys: {}", settings.as_object().map(|o| o.len()).unwrap_or(0));
    println!("settings path: {}", paths.settings_path.display());
    println!("worker ready: {}", host.manager.is_ready());
    println!("worker connected: {}", host.manager.is_connected());

    host.refresh_monitors(true, true);
    let start = std::time::Instant::now();
    while start.elapsed().as_millis() < 30_000 {
        let snapshot = host.monitors_snapshot();
        if snapshot.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let snapshot = host.monitors_snapshot();
    let count = snapshot.as_object().map(|o| o.len()).unwrap_or(0);
    println!("monitor count: {}", count);

    // UI 模型映射自检（等价 BrightnessPanel 渲染分支）
    let (models, visible) = twinkle_tray_rust::monitor_view::build_panel_model(
        &snapshot,
        settings,
        false,
    );
    println!("panel models: {} visible: {}", models.len(), visible);
    for m in &models {
        println!(
            "  {} key={} type={:?} brightness={} extended={} features={}",
            m.name,
            m.key,
            m.display_type,
            m.brightness,
            m.extended,
            m.features.row_count()
        );
    }
    println!("ddc safety status: {}", MonitorHost::ddc_safety_status(&paths.user_data));
    println!("=== app-smoke done ===");
}