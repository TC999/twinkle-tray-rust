//! 全局热键（RegisterHotKeyW）。
//!
//! equiv: electron globalShortcut。RegisterHotKeyW 在 windows crate 0.62 未绑定，
//! 以集中 FFI 声明补齐（AGENTS.md §1.1：必要性成立，声明集中在模块 crate 内）。
//! 热键消息 WM_HOTKEY 在拥有消息循环的线程接收（与托盘共用专用线程）。

use std::sync::mpsc::Receiver;
use windows::Win32::Foundation::{LPARAM, WPARAM};

windows::core::link!("user32.dll" "system" fn RegisterHotKey(hwnd : windows::Win32::Foundation::HWND, id : i32, fsmodifiers : u32, vk : u32) -> i32);
windows::core::link!("user32.dll" "system" fn UnregisterHotKey(hwnd : windows::Win32::Foundation::HWND, id : i32) -> i32);

/// WM_HOTKEY（winuser.h）。
pub const WM_HOTKEY: u32 = 0x0312;

/// 修饰键（MOD_* 常量，winuser.h）。
pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;
/// MOD_NOREPEAT（Win7+）
pub const MOD_NOREPEAT: u32 = 0x4000;

/// 注册全局热键。hwnd 为接收 WM_HOTKEY 的消息窗口（None = 调用线程关联）。
/// 等价 electron globalShortcut.register(accelerator, callback)。
pub fn register_hotkey(hwnd: Option<windows::Win32::Foundation::HWND>, id: i32, modifiers: u32, vk: u32) -> bool {
    // SAFETY: RegisterHotKeyW 按文档语义
    unsafe { RegisterHotKey(hwnd.unwrap_or_default(), id, modifiers, vk) != 0 }
}

/// 注销热键。
pub fn unregister_hotkey(hwnd: Option<windows::Win32::Foundation::HWND>, id: i32) -> bool {
    // SAFETY: 与成功的 RegisterHotKeyW 配对
    unsafe { UnregisterHotKey(hwnd.unwrap_or_default(), id) != 0 }
}

/// WM_HOTKEY 消息解析：wparam = id，lparam 高字 = 修饰键，低字 = vk。
/// 返回 (id, modifiers, vk)。
pub fn parse_wm_hotkey(wparam: WPARAM, lparam: LPARAM) -> (i32, u32, u32) {
    (
        wparam.0 as i32,
        ((lparam.0 >> 16) & 0xFFFF) as u32,
        (lparam.0 & 0xFFFF) as u32,
    )
}

/// 热键事件接收器（主进程注入回调后消费）。
pub struct HotkeyReceiver {
    pub rx: Receiver<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wm_hotkey_parsing() {
        // id=7, MOD_CONTROL|MOD_SHIFT=0x6, vk=0x28（下方向键）
        let lparam = LPARAM(((0x6u32 << 16) | 0x28u32) as isize);
        let (id, mods, vk) = parse_wm_hotkey(WPARAM(7), lparam);
        assert_eq!(id, 7);
        assert_eq!(mods, 0x6);
        assert_eq!(vk, 0x28);
    }
}
