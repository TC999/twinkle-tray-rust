//! 关闭显示器（equiv electron.js sleepDisplays 的 PostMessage 分支 L3545-L3547）。
//!
//! 原实现用 PowerShell 调 PostMessage(0xFFFF, WM_SYSCOMMAND, SC_MONITORPOWER, 2)，
//! 这里直接走 User32 的 PostMessageW（等价替换，无外部进程）。

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, SC_MONITORPOWER, WM_SYSCOMMAND};

/// HWND_BROADCAST（0xFFFF）
const HWND_BROADCAST: isize = 0xFFFF;

/// 关闭所有显示器（equiv PostMessage(0xFFFF, 0x0112, 0xF170, 0x0002)）。
pub fn turn_off_displays() -> bool {
    // SAFETY: HWND_BROADCAST 是 User32 约定的伪句柄，PostMessageW 对其广播安全。
    unsafe {
        PostMessageW(
            Some(HWND(HWND_BROADCAST as *mut core::ffi::c_void)),
            WM_SYSCOMMAND,
            WPARAM(SC_MONITORPOWER as usize),
            LPARAM(2), // 2 = 关闭显示器
        )
        .is_ok()
    }
}
