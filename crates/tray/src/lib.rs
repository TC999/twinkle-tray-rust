//! 托盘图标与全局热键。
//!
//! equiv: electron Tray/globalShortcut 的等价实现（AGENTS.md §1.2 预定方案：
//! Shell_NotifyIconW 自写托盘 + RegisterHotKeyW 热键）。
//! Win32 消息循环跑专用线程（AGENTS.md §5 线程模型）。

pub mod hotkey;
pub mod tray_icon;

pub use hotkey::{register_hotkey, unregister_hotkey, HotkeyReceiver};
pub use tray_icon::{
    create_tray_icon, remove_tray_icon, run_message_loop, set_tray_callback, spawn_tray_thread,
    stop_tray_thread, update_tray_icon, TrayConfig, TrayEvent, TrayIconHandle, TrayMenuItem,
};