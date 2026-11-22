//! 主程序库入口（Slint UI 生成代码 + 模块）。

slint::include_modules!();

pub mod app;
pub mod hotkey_host;
pub mod logger;
pub mod monitor_host;
pub mod monitor_view;
pub mod panel_controller;
pub mod settings_host;
pub mod settings_view;
pub mod ui_types;
pub mod window_state;
pub mod worker_ipc;

use std::sync::{Arc, Mutex};

/// 启动面板窗口（主线程 = Slint 事件循环，AGENTS.md §5）。
pub fn run_panel(
    host: Arc<monitor_host::MonitorHost>,
    settings: Arc<Mutex<serde_json::Value>>,
    tray_events: Option<std::sync::mpsc::Receiver<tray::TrayEvent>>,
) -> Result<(), String> {
    panel_controller::run_panel_controller(host, settings, tray_events)
}

/// 运行设置窗口（由面板/托盘触发，独立窗口）。
pub fn run_settings(
    host: Arc<monitor_host::MonitorHost>,
    settings: Arc<Mutex<serde_json::Value>>,
) -> Result<(), String> {
    settings_host::open_settings_window(settings, host).map(|_| ())
}