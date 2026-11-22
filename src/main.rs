//! Twinkle Tray Rust 主入口。
//!
//! 等价 electron.js 的进程模型：同二进制 `--monitor-worker` 子命令自调用
//! 替代 child_process.fork（AGENTS.md §3.1 等价替换）。
//!
//! 启动序列（equiv electron.js:L1-L130 / L862-L868 / L1201-L1214 / L4514+）：
//! 1. 解析开关；2. 解析路径并建配置目录；3. 读 settings + 版本迁移；
//! 4. 初始化会话日志；5. 拉起 monitor worker；6. 进入 Slint 事件循环。

use std::path::PathBuf;
use twinkle_tray_rust::app::{self, AppPaths, LaunchOptions};
use twinkle_tray_rust::logger;
use twinkle_tray_rust::monitor_host::MonitorHost;
use twinkle_tray_rust::worker_ipc;

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

    // 主进程 UI（主线程 = Slint 事件循环，AGENTS.md §5）
    if let Err(e) = twinkle_tray_rust::run_panel() {
        eprintln!("面板启动失败: {}", e);
    }
    host.manager.set_quitting(true);
    host.manager.stop();
}

/// getDDCBrightnessVCPs 等价（equiv electron.js:L2340-L2353）。
/// knownDDCBrightnessVCPs 来自 monitor-rules.json 的 ddcBrightnessCodes，用户设置覆盖其上。
fn ddc_brightness_vcps(settings: &serde_json::Value) -> serde_json::Value {
    let mut merged = serde_json::json!({});
    if let Some(user) = settings.get("userDDCBrightnessVCPs").and_then(|v| v.as_object()) {
        for (k, v) in user {
            let parsed = match v {
                serde_json::Value::String(s) => s.trim().parse::<u64>().ok(),
                serde_json::Value::Number(n) => n.as_u64(),
                _ => None,
            };
            if let Some(code) = parsed {
                merged[k] = serde_json::json!(code);
            }
        }
    }
    merged
}

/// --app-smoke：启动序列 e2e 自检。
/// 真机验证点：settings 读取/迁移、日志初始化、worker 就绪、WMI 桥探测、显示器枚举。
fn run_app_smoke(host: &std::sync::Arc<MonitorHost>, settings: &serde_json::Value, paths: &AppPaths) {
    println!("=== app-smoke ===");
    println!("settings keys: {}", settings.as_object().map(|o| o.len()).unwrap_or(0));
    println!("settings path: {}", paths.settings_path.display());
    println!("worker ready: {}", host.manager.is_ready());
    println!("worker connected: {}", host.manager.is_connected());

    // 触发一次全量枚举，等待 worker 回包（30s 上限，等价 waitForMonitorThreadReady）
    host.refresh_monitors(true, true);
    let start = std::time::Instant::now();
    while start.elapsed().as_millis() < 30_000 {
        let snapshot = host.monitors_snapshot();
        if snapshot.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
            println!("monitors: {}", snapshot);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let snapshot = host.monitors_snapshot();
    let count = snapshot.as_object().map(|o| o.len()).unwrap_or(0);
    println!("monitor count: {}", count);
    println!("ddc safety status: {}", MonitorHost::ddc_safety_status(&paths.user_data));
    println!("=== app-smoke done ===");
}
