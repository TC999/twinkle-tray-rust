//! Twinkle Tray Rust 主入口。
//!
//! 等价 electron.js 的进程模型：同二进制 --monitor-worker 子命令自调用
//! 替代 child_process.fork（AGENTS.md §3.1 等价替换）。
//!
//! 启动序列（equiv electron.js:L1-L130 / L862-L868 / L1201-L1214 / L4514+）：
//! 1. 解析开关；2. 解析路径并建配置目录；3. 读 settings + 版本迁移；
//! 4. 初始化会话日志；5. 拉起 monitor worker；6. 托盘/热键；7. 进入 Slint 事件循环。


/// --udp-smoke：UDP 服务器真机对拍。
/// 启动服务器 → 以 UDP 客户端发送 list/get/setvcp 消息 → 校验响应。
fn run_udp_smoke(
    host: &Arc<MonitorHost>,
    settings: &serde_json::Value,
    _paths: &AppPaths,
) {
    use std::net::UdpSocket;

    println!("=== udp-smoke ===");
    // 构造测试 settings：启用 UDP + 固定端口/密钥
    let mut test_settings = settings.clone();
    let obj = test_settings.as_object_mut().unwrap();
    obj.insert("udpEnabled".to_string(), serde_json::json!(true));
    obj.insert("udpPortStart".to_string(), serde_json::json!(14715));
    obj.insert("udpKey".to_string(), serde_json::json!("smoke-test-key"));
    let shared = Arc::new(Mutex::new(test_settings.clone()));

    // 启动（等价 udp.start）
    let rt_for_udp = twinkle_tray_rust::runtime_hosts::RuntimeHost::new(
        host.clone(),
        shared.clone(),
        _paths.known_displays_path.clone(),
    );
    let started = twinkle_tray_rust::udp_host::udp_start(&rt_for_udp, &shared);
    println!("udp_start: {}", started);

    // 等待绑定
    std::thread::sleep(std::time::Duration::from_millis(800));

    // UDP 客户端发包（等价外部控制器）
    let client = UdpSocket::bind("127.0.0.1:0").expect("bind client");
    client.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();

    let send_recv = |msg: &str| -> Option<String> {
        client
            .send_to(msg.as_bytes(), "127.0.0.1:14715")
            .expect("send");
        let mut buf = [0u8; 65536];
        match client.recv_from(&mut buf) {
            Ok((n, _)) => Some(String::from_utf8_lossy(&buf[..n]).to_string()),
            Err(e) => {
                println!("recv error: {}", e);
                None
            }
        }
    };

    // 1) list（须带 key；等价 handleClientMessage 的 key 校验）
    let list_msg = serde_json::json!({"type": "list", "key": "smoke-test-key"}).to_string();
    match send_recv(&list_msg) {
        Some(resp) => {
            let parsed: Result<serde_json::Value, _> = serde_json::from_str(&resp);
            match parsed {
                Ok(v) => println!("list: OK ({} monitors)", v.as_object().map(|o| o.len()).unwrap_or(0)),
                Err(_) => println!("list: NON-JSON response: {}", &resp[..resp.len().min(80)]),
            }
        }
        None => println!("list: NO RESPONSE"),
    }

    // 1b) refresh：先触发显示器枚举（等价 refreshMonitors(true,true)）
    let refresh_msg = serde_json::json!({"type": "refresh", "key": "smoke-test-key"}).to_string();
    client.send_to(refresh_msg.as_bytes(), "127.0.0.1:14715").expect("send refresh");
    // 轮询等待枚举完成（真机 WMI/DDC 扫描；上限 30s）
    let mut polled = 0;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        polled += 1;
        let n = rt_for_udp.host.monitors_snapshot().as_object().map(|o| o.len()).unwrap_or(0);
        if n > 0 || polled >= 60 {
            println!("monitors after refresh: {} (poll {})", n, polled);
            // 打印键名（对拍 findMonitor 的子串匹配）
            let snap = rt_for_udp.host.monitors_snapshot();
            let _ = snap;
            break;
        }
    }

    // 2) get brightness：monitor 参数 = monitors 键的子串（原版 findMonitor 语义）。
    // 内屏键为 hwid[2] 形式（如 5&xxx&0&UID440），用 "UID" 子串匹配。
    let get_msg = serde_json::json!({
        "type": "get", "key": "smoke-test-key",
        "monitor": "UID", "property": "brightness"
    }).to_string();
    match send_recv(&get_msg) {
        Some(resp) => println!("get brightness: {}", resp),
        None => println!("get brightness: NO RESPONSE"),
    }

    // 3) 错误 key：应无响应（recv 超时）
    let bad_msg = serde_json::json!({"type": "list", "key": "WRONG"}).to_string();
    client.send_to(bad_msg.as_bytes(), "127.0.0.1:14715").expect("send bad");
    let mut buf = [0u8; 65536];
    match client.recv_from(&mut buf) {
        Ok(_) => println!("bad key: GOT RESPONSE (unexpected!)"),
        Err(_) => println!("bad key: no response (correct)"),
    }

    // 4) stop
    twinkle_tray_rust::udp_host::udp_stop();
    std::thread::sleep(std::time::Duration::from_millis(300));
    println!("udp stopped");
}
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use twinkle_tray_rust::app::{self, AppPaths, LaunchOptions};
use twinkle_tray_rust::logger;
use twinkle_tray_rust::monitor_host::MonitorHost;
use twinkle_tray_rust::worker_ipc;
use slint::{ComponentHandle, Model};

/// 选定 Slint 后端（winit + femtovg）并给 winit 事件循环装上模态循环补充泵。
///
/// 必须早于任何组件创建：Slint 只有在选后端时才会构建 winit 的 EventLoop，而 winit
/// 0.30 的消息钩子只能在 EventLoopBuilder 上设置（见 window_drag 模块说明）。
fn init_slint_backend() {
    #[allow(unused_mut)]
    let mut builder = slint::BackendSelector::new().backend_name("winit".into());
    #[cfg(target_os = "win32")]
    {
        use slint::winit_030::winit;
        use winit::platform::windows::EventLoopBuilderExtWindows as _;
        let mut event_loop_builder =
            slint::winit_030::EventLoopBuilder::<slint::winit_030::SlintEvent>::new();
        event_loop_builder.with_msg_hook(twinkle_tray_rust::window_drag::modal_pump::msg_hook);
        builder = builder.with_winit_event_loop_builder(event_loop_builder);
    }
    if let Err(e) = builder.select() {
        eprintln!("Couldn't select the Slint backend: {}", e);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let opts = LaunchOptions::parse(&args);

    // 等价 electron.js:L351 fork(Monitors.js, [...])
    if opts.is_monitor_worker {
        worker_ipc::run_monitor_worker(&args);
        return;
    }

    // Slint 后端（winit + femtovg）与事件循环消息钩子：必须早于任何组件创建。
    init_slint_backend();

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

    // 进程级配置目录快照（settings_host 的 update_path / 更新下载用;equiv updatePath 定义）
    // SAFETY: 主进程单线程启动阶段,set_var 在 spawn 前调用,无并发访问
    std::env::set_var("TT_CONFIG_DIR", &paths.config_dir);
    std::env::set_var("TT_USER_DATA", &paths.user_data);

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
        if let Some(pos) = demo_window_position() {
            if let Ok(mut slot) = DEMO_WINDOW_POS.lock() {
                *slot = Some(pos);
            }
        }
        run_ui_demo(&host, &settings, opts.ui_demo_page.as_deref());
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

    // --udp-smoke：UDP 服务器真机对拍（启动 → 发包 → 校验响应 → 退出）
    if opts.udp_smoke {
        run_udp_smoke(&host, &settings, &paths);
        host.manager.set_quitting(true);
        host.manager.stop();
        return;
    }

    let shared_settings = Arc::new(Mutex::new(settings.clone()));

    // settings 路径快照(托盘菜单构建读取;等价原版 setTrayMenu 读内存 settings)
    {
        let mut slot = SETTINGS_PATH_SNAPSHOT.lock().unwrap();
        *slot = Some(paths.settings_path.clone());
        // TT_SETTINGS_PATH：settings_host 落盘与 Open Settings 按钮用
        // （update_setting 的 writeSettings 落盘调度读取此变量）
        std::env::set_var("TT_SETTINGS_PATH", &paths.settings_path);
    }

    // 托盘图标（equiv createTray：图标随主题切换，§1.2 禁止自绘）
    let (_tray_handle, tray_events) = start_tray(&settings, &exe_dir);
    // 注册托盘菜单重建函数：设置页切换语言后重建菜单文案（equiv rebuildTray）
    twinkle_tray_rust::settings_host::set_tray_menu_rebuilder(build_tray_menu);

    // 运行时状态机（equiv electron.js 的模块级状态集合）
    let runtime = twinkle_tray_rust::runtime_hosts::RuntimeHost::new(
        host.clone(),
        shared_settings.clone(),
        paths.known_displays_path.clone(),
    );

    // 系统事件层（equiv addEventListeners + applyHotkeys + 定时器集合）
    let mut system = twinkle_tray_rust::system_events::SystemEvents::new(None);
    twinkle_tray_rust::system_events::install_display_listener();

    // 全局热键（equiv applyHotkeys）：注册在托盘线程消息窗口上
    start_hotkeys(&settings, &mut system);

    // analytics 埋点（equiv L1429-L1440：settings.analytics 开关 + 29min 周期）
    system.sync_analytics(&settings);

    // UDP 服务器（equiv L1326-L1330 的启动分支：udpEnabled === true → udp.start）
    twinkle_tray_rust::udp_host::sync_udp_server(Some(&runtime), &shared_settings, None);

    // 主进程 UI（主线程 = Slint 事件循环，AGENTS.md §5）
    if let Err(e) = twinkle_tray_rust::run_panel(
        host.clone(),
        shared_settings.clone(),
        tray_events,
        runtime.clone(),
        system,
    ) {
        eprintln!("面板启动失败: {}", e);
    }

    // 图标/窗口由托盘线程销毁(同线程约束);只投递 WM_QUIT
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
        tt_localize::detect_language_code(&twinkle_tray_rust::window_state::system_locale())
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
    // equiv electron.js L2459-L2461: themeDir = SystemUsesLightTheme ? 'light' : 'dark'
    // 图标跟随系统任务栏深浅色（深色任务栏 = dark 目录 = 白图标），不随应用内主题设置。
    let dir = if twinkle_tray_rust::window_state::system_uses_light_theme() {
        "light"
    } else {
        "dark"
    };
    let file = format!("{}.ico", icon_name);
    // 图标资产搜索链（AGENTS.md §1.2：加载预生成 ico，禁止自绘）：
    // 1) exe 目录旁 assets/tray-icons（发布布局：assets 随 exe 部署）
    // 2) exe 目录旁的 origin/src/assets（dev 运行 target/debug 时 exe_dir 是 target/debug）
    // 3) 仓库根 origin/src/assets（exe_dir 向上回溯两级）
    // 4) 当前工作目录的 origin/src/assets（任意 cwd 启动兜底）
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    candidates.push(exe_dir.join("assets").join("tray-icons").join(&dir).join(&file));
    candidates.push(exe_dir.join("origin").join("src").join("assets").join("tray-icons").join(&dir).join(&file));
    if let Some(parent) = exe_dir.parent() {
        candidates.push(parent.join("origin").join("src").join("assets").join("tray-icons").join(&dir).join(&file));
    }
    if let Some(grand) = exe_dir.parent().and_then(|p| p.parent()) {
        candidates.push(grand.join("origin").join("src").join("assets").join("tray-icons").join(&dir).join(&file));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("origin").join("src").join("assets").join("tray-icons").join(&dir).join(&file));
    }
    let icon_path = match candidates.iter().find(|p| p.exists()) {
        Some(p) => p.to_string_lossy().to_string(),
        None => {
            logger::log_as("MAIN", "error", "Tray icon asset not found.");
            return (None, None);
        }
    };
    logger::log_as("MAIN", "info", &format!("Tray icon: {}", icon_path));
    let config = tray::TrayConfig {
        icon_path,
        tooltip: "Twinkle Tray".to_string(),
        menu_items: build_tray_menu(),
    };
    // 托盘窗口/图标由专用线程创建与销毁（DestroyWindow 的同线程约束）
    let (events, handle) = tray::spawn_tray_thread(config);
    if handle.is_none() {
        logger::log_as("MAIN", "error", "Tray icon creation failed (Shell_NotifyIconW NIM_ADD).");
    }
    logger::log_as("MAIN", "info", "Tray icon thread started.");
    (handle, Some(events))
}

/// 托盘右键菜单（equiv electron.js setTrayMenu L4627-L4698 的项集合，§1.2 自写 HMENU）。
/// 菜单 id 段位：1001+ 对应 electron 模板的 click 回调。
fn build_tray_menu() -> Vec<tray::TrayMenuItem> {
    let settings = read_settings_snapshot();
    // tempSettings 初始为 false(equiv temp_settings() 默认值);菜单勾选态随 checkbox 点击刷新
    let temp = twinkle_tray_rust::runtime_hosts::TempSettings::default();
    let mut items = Vec::new();

    // getTimeAdjustmentsMenuItem（equiv L4651-L4656）
    let has_tod = settings
        .get("adjustmentTimes")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    if has_tod {
        items.push(tray::TrayMenuItem {
            id: 2001,
            label: tt_localize::t("GENERIC_PAUSE_TOD"),
            enabled: true,
            checked: Some(temp.pause_time_adjustments),
            separator: false,
            children: Vec::new(),
        });
    }
    // getDetectIdleMenuItem（equiv L4658-L4663）
    let idle_enabled = settings
        .get("detectIdleTimeEnabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if idle_enabled {
        items.push(tray::TrayMenuItem {
            id: 2002,
            label: tt_localize::t("GENERIC_PAUSE_IDLE"),
            enabled: true,
            checked: Some(temp.pause_idle_detection),
            separator: false,
            children: Vec::new(),
        });
    }
    // getProfilesMenuItem（equiv L4665-L4681:showInMenu && setBrightness 的 profile 平铺）
    let profiles = settings
        .get("profiles")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut profile_items = Vec::new();
    for (idx, profile) in profiles.iter().enumerate() {
        let show = profile.get("showInMenu").and_then(|v| v.as_bool()).unwrap_or(false);
        let set_b = profile.get("setBrightness").and_then(|v| v.as_bool()).unwrap_or(false);
        if show && set_b {
            let name = profile.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            profile_items.push((2100 + idx as u32, name));
        }
    }
    if !profile_items.is_empty() {
        // SETTINGS_PROFILES_TITLE 子菜单（equiv getProfilesMenuItem L4676 的 submenu）
        let children: Vec<tray::TrayMenuItem> = profile_items
            .iter()
            .map(|(id, name)| tray::TrayMenuItem {
                id: *id,
                label: name.clone(),
                enabled: true,
                checked: None,
                separator: false,
                children: Vec::new(),
            })
            .collect();
        items.push(tray::TrayMenuItem {
            id: 0,
            label: tt_localize::t("SETTINGS_PROFILES_TITLE"),
            enabled: true,
            checked: None,
            separator: false,
            children,
        });
    }
    // getPausableSeparatorMenuItem（equiv L4644-L4649）
    if idle_enabled || has_tod || !profile_items.is_empty() {
        items.push(tray::TrayMenuItem {
            id: 0,
            label: String::new(),
            enabled: true,
            checked: None,
            separator: true,
            children: Vec::new(),
        });
    }
    // refresh displays（equiv L4635）
    items.push(tray::TrayMenuItem {
        id: 2003,
        label: tt_localize::t("GENERIC_REFRESH_DISPLAYS"),
        enabled: true,
        checked: None,
        separator: false,
        children: Vec::new(),
    });
    // settings（equiv L4636）
    items.push(tray::TrayMenuItem {
        id: 1002,
        label: tt_localize::t("GENERIC_SETTINGS"),
        enabled: true,
        checked: None,
        separator: false,
        children: Vec::new(),
    });
    // separator
    items.push(tray::TrayMenuItem {
        id: 0,
        label: String::new(),
        enabled: true,
        checked: None,
        separator: true,
        children: Vec::new(),
    });
    // getDebugTrayMenuItems（equiv L4683-L4698:isDev 时显示）——改为 DEBUG 子菜单
    let is_dev = settings.get("isDev").and_then(|v| v.as_bool()).unwrap_or(false);
    if is_dev {
        let debug_children: Vec<tray::TrayMenuItem> = [
            // RESTART PANEL（equiv restartPanel）：等价重建面板窗口，当前实现无持久窗口，
            // 以重新下发刷新触发等价行为（记录于面板控制器）
            (2004, "RESTART PANEL"),
            // RECREATE TRAY（equiv recreateTray）：托盘常驻线程，无重建语义，保留 id 供路由
            (2005, "RECREATE TRAY"),
            // MINIMIZE PANEL（equiv mainWindow.minimize）：面板最小化
            (2006, "MINIMIZE PANEL"),
            // HIDE PANEL（equiv showPanel(false)）
            (2007, "HIDE PANEL"),
            // OPACITY 0 / OPACITY 1（equiv setOpacity）
            (2008, "OPACITY 0"),
            (2009, "OPACITY 1"),
            // DO CURRENT TOD（equiv applyCurrentAdjustmentEvent(true)）
            (2010, "DO CURRENT TOD"),
            // REMOVE ACRYLIC（equiv tryVibrancy(mainWindow, false)）
            (2011, "REMOVE ACRYLIC"),
            // PAUSE MOUSE（equiv pauseMouseEvents(true)）
            (2012, "PAUSE MOUSE"),
            // LAST ACTIVE WIN（equiv trySetForegroundWindow）
            (2013, "LAST ACTIVE WIN"),
        ]
        .iter()
        .map(|(id, label)| tray::TrayMenuItem {
            id: *id,
            label: label.to_string(),
            enabled: true,
            checked: None,
            separator: false,
            children: Vec::new(),
        })
        .collect();
        items.push(tray::TrayMenuItem {
            id: 0,
            label: "DEBUG".to_string(),
            enabled: true,
            checked: None,
            separator: false,
            children: debug_children,
        });
    }
    // turn off displays（保留原 1003 语义:原版 monitors 段的 turn off）
    items.push(tray::TrayMenuItem {
        id: 1003,
        label: tt_localize::t("PANEL_BUTTON_TURN_OFF_DISPLAYS"),
        enabled: true,
        checked: None,
        separator: false,
        children: Vec::new(),
    });
    // quit（equiv L4639）
    items.push(tray::TrayMenuItem {
        id: 2000,
        label: tt_localize::t("GENERIC_QUIT"),
        enabled: true,
        checked: None,
        separator: false,
        children: Vec::new(),
    });
    items
}

/// 托盘菜单构建时的 settings 快照(启动时的静态副本;checkbox 状态随菜单重建刷新)。
/// 等价原版 setTrayMenu 在每次托盘创建时读取当前 settings/tempSettings。
fn read_settings_snapshot() -> serde_json::Value {
    match std::fs::read_to_string(&SETTINGS_PATH_SNAPSHOT.lock().unwrap().clone().unwrap_or_default()) {
        Ok(text) => serde_json::from_str(&text).unwrap_or(serde_json::json!({})),
        Err(_) => serde_json::json!({}),
    }
}

/// settings 路径快照(main 启动时写入,托盘菜单构建时读取)。
static SETTINGS_PATH_SNAPSHOT: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

/// 全局热键（equiv applyHotkeys）。
/// RegisterHotKeyW 需要一个消息窗口接收 WM_HOTKEY；托盘线程已创建消息窗口，
/// 这里枚举该线程窗口句柄完成注册（注册 id = 10000+ 序号）。
fn start_hotkeys(settings: &serde_json::Value, system: &mut twinkle_tray_rust::system_events::SystemEvents) {
    // 等价 applyHotkeys 的注册主体在 system.apply_hotkeys_now 完成
    // （托盘 hwnd 由托盘线程注册后通过 tray::tray_window_handle() 取得）
    if let Some(hwnd) = tray::tray_window_handle() {
        system.tray_hwnd = Some(hwnd);
        system.apply_hotkeys_now(settings);
        let _ = hwnd;
    } else {
        logger::log_as("MAIN", "warn", "Tray window unavailable; hotkeys not registered.");
    }
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

/// --ui-demo 的窗口摆位（0 = 不指定，交给 Slint 默认位置）。
///
/// 设定后第一个 Slint 窗口放在该物理坐标、第二个放在其右侧 +380：设置窗口宽 760，
/// 面板宽 356，两者都能整窗可见，便于「按标题栏拖动」这类交互对拍时目视观察。
static DEMO_WINDOW_POS: std::sync::Mutex<Option<(i32, i32)>> = std::sync::Mutex::new(None);
static DEMO_WINDOW_SLOT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn demo_window_position() -> Option<(i32, i32)> {
    let args: Vec<String> = std::env::args().collect();
    let raw = args.iter().find_map(|a| a.strip_prefix("--demo-pos="))?;
    let (x, y) = raw.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// 按 --demo-pos 摆放窗口（物理像素 × 该窗口 DPI 比例，与 winit set_position 同口径）。
/// 未指定时不动窗口，返回 false。
fn place_demo_window(window: &slint::Window) -> bool {
    let pos = DEMO_WINDOW_POS.lock().ok().and_then(|g| *g);
    let Some((base_x, base_y)) = pos else {
        return false;
    };
    let slot = DEMO_WINDOW_SLOT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let scale = twinkle_tray_rust::window_state::window_dpi_scale(window);
    let x = (base_x as f32 + if slot == 0 { 0.0 } else { 380.0 }) * scale;
    let _ = window.set_position(slint::PhysicalPosition::new(x as i32, base_y as i32));
    true
}

/// --ui-demo：真实渲染面板与设置窗口（供真机截图对拍）。
/// 注入 worker 真实枚举结果 + 一份演示用的扩展特征，覆盖两条渲染分支。
fn run_ui_demo(host: &std::sync::Arc<MonitorHost>, settings: &serde_json::Value, demo_page: Option<&str>) {
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
    // demo 不联网：打开设置窗口会走真实 checkForUpdates 路径
    demo_settings["checkForUpdates"] = serde_json::json!(false);
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
            // demo 窗口的摆位：--demo-pos 指定时按物理坐标落位（与 winit 同口径换算，
            // 物理 = 逻辑 × GetDpiForWindow/96）；未指定时保持 Slint 的默认摆位。
            place_demo_window(panel.window());
            let _ = panel.show();
        }
    } else {
        println!("demo: no monitors enumerated; panel skipped");
    }

    // 设置窗口：走真实 open_settings_window 路径（验证 monitors 模型注入与刷新接线）。
    // 热键等不依赖显示器的页面在无显示器时也可对拍，故不再被有显示器分支包裹。
    // 此前这里手工 set_monitors，掩盖了"真实打开路径从不注入显示器列表"的缺陷。
    let settings_shared = std::sync::Arc::new(std::sync::Mutex::new(demo_settings.clone()));
    match twinkle_tray_rust::settings_host::open_settings_window(settings_shared, host.clone()) {
        Ok(handle) => {
            let ui = &handle.ui;
            // 初始页：--ui-demo=<page> 指定（默认 monitors；供各页截图对拍）
            let page = demo_page.unwrap_or("monitors");
            ui.set_active_page(page.into());
            ui.set_items(slint::ModelRc::new(slint::VecModel::from(
                twinkle_tray_rust::settings_view::build_items(page, &demo_settings),
            )));
            ui.set_items_after(slint::ModelRc::new(slint::VecModel::from(
                twinkle_tray_rust::settings_view::build_post_items(page, &demo_settings),
            )));
            place_demo_window(ui.window());
            println!("demo windows shown (panel + settings/{})", page);
            println!("settings monitors: {}", handle.monitors_model.row_count());
            let _ = slint::run_event_loop_until_quit();
        }
        Err(e) => println!("settings window FAILED: {}", e),
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

    // 2b) 热键增删链路（走真实 open_settings_window 接线）：
    // invoke hotkey-add → settings 写回 + 模型刷新 + 落盘；
    // invoke hotkey-remove(id) → 反向断言。equiv addHotkey / deleteHotkey 全链。
    {
        let demo = serde_json::json!({
            "checkForUpdates": false,
            "hotkeys": [{
                "id": "hk-smoke-1",
                "accelerator": "",
                "actions": [{"target": "brightness", "type": "offset", "value": 10,
                             "values": [10], "allMonitors": true, "monitors": {}}]
            }]
        });
        let shared = Arc::new(Mutex::new(demo));
        // 落盘指向临时文件，避免污染真实 settings
        let smoke_file = std::env::temp_dir().join("tt-ui-smoke-settings.json");
        let prev_path = std::env::var("TT_SETTINGS_PATH").ok();
        std::env::set_var("TT_SETTINGS_PATH", &smoke_file);
        match twinkle_tray_rust::settings_host::open_settings_window(shared.clone(), host.clone()) {
            Ok(handle) => {
                let before = handle.ui.get_hotkeys().row_count();
                handle.ui.invoke_hotkey_add();
                let after_add = handle.ui.get_hotkeys().row_count();
                let shared_now = shared.lock().unwrap().clone();
                let new_id = shared_now["hotkeys"][1]["id"].as_str().unwrap_or("").to_string();
                handle.ui.invoke_hotkey_remove(new_id.clone().into());
                let after_remove = handle.ui.get_hotkeys().row_count();
                println!(
                    "hotkey chain: before={} after_add={} after_remove={}",
                    before, after_add, after_remove
                );
                let list = shared.lock().unwrap()["hotkeys"].as_array().cloned().unwrap_or_default();
                println!("settings hotkeys after remove: {}", list.len());
                // 落盘（333ms 防抖）后回读
                std::thread::sleep(std::time::Duration::from_millis(900));
                let written = std::fs::read_to_string(&smoke_file)
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                    .map(|v| v["hotkeys"].as_array().map(|a| a.len()).unwrap_or(0));
                let written = written.unwrap_or(usize::MAX);
                println!("settings file hotkeys written: {}", written);
                let ok = before == 1 && after_add == 2 && after_remove == 1
                    && list.len() == 1 && written == 1;
                println!("hotkey chain {} (file={})", if ok { "OK" } else { "MISMATCH" },
                    if written == usize::MAX { "missing".to_string() } else { written.to_string() });
            }
            Err(e) => println!("hotkey chain settings window FAILED: {}", e),
        }
        match prev_path {
            Some(p) => std::env::set_var("TT_SETTINGS_PATH", p),
            None => { std::env::remove_var("TT_SETTINGS_PATH"); }
        }
        let _ = std::fs::remove_file(&smoke_file);
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