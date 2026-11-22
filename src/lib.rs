//! 主程序库入口（Slint UI 生成代码 + 模块）。

slint::include_modules!();

/// 启动面板窗口（主线程 = Slint 事件循环）。
pub fn run_panel() -> Result<(), String> {
    let panel = r#PanelWindow::new().map_err(|e| e.to_string())?;
    panel.show().map_err(|e| e.to_string())?;
    slint::run_event_loop_until_quit().map_err(|e| e.to_string())
}

pub mod worker_ipc;
