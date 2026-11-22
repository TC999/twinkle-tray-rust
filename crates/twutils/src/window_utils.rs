//! 窗口工具。
//!
//! equiv: windows_window_utils.cc（166 行）。

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, HMONITOR, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, SetForegroundWindow,
    SetWindowPos, SET_WINDOW_POS_FLAGS,
};

/// 位置矩形（等价 RectToObj 输出）。
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowPosition {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
    pub width: i32,
    pub height: i32,
}

fn rect_to_position(rect: &RECT) -> WindowPosition {
    WindowPosition {
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
        left: rect.left,
        width: rect.right - rect.left,
        height: rect.bottom - rect.top,
    }
}

/// setWindowPos 等价。
pub fn set_window_pos(
    hwnd: HWND,
    hwnd_after: HWND,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    flags: SET_WINDOW_POS_FLAGS,
) -> bool {
    // SAFETY: hwnd 由调用方保证合法；无效句柄时 API 返回 Err
    unsafe { SetWindowPos(hwnd, Some(hwnd_after), x, y, width, height, flags) }.is_ok()
}

/// getWindowPos 等价：失败返回 None（等价空对象）。
pub fn get_window_pos(hwnd: HWND) -> Option<WindowPosition> {
    let mut rect = RECT::default();
    // SAFETY: rect 出参合法
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return None;
    }
    Some(rect_to_position(&rect))
}

/// getClientPos 等价。
pub fn get_client_pos(hwnd: HWND) -> Option<WindowPosition> {
    let mut rect = RECT::default();
    // SAFETY: rect 出参合法
    if unsafe { GetClientRect(hwnd, &mut rect) }.is_err() {
        return None;
    }
    Some(rect_to_position(&rect))
}

/// getForegroundWindow 等价。
pub fn get_foreground_window() -> HWND {
    // SAFETY: 无副作用
    unsafe { GetForegroundWindow() }
}

/// setForegroundWindow 等价。
pub fn set_foreground_window(hwnd: HWND) -> bool {
    // SAFETY: hwnd 合法性由调用方保证
    unsafe { SetForegroundWindow(hwnd) }.as_bool()
}

/// getWindowLong 等价。
pub fn get_window_long(hwnd: HWND, index: i32) -> isize {
    // SAFETY: index 由调用方传入（GWL_* 常量）
    unsafe { GetWindowLongPtrW(hwnd, windows::Win32::UI::WindowsAndMessaging::WINDOW_LONG_PTR_INDEX(index)) }
}

/// getWindowFullscreen 等价。
pub fn get_window_fullscreen(hwnd: HWND) -> bool {
    // SAFETY: monitorInfo.cbSize 先设置
    let mut monitor_info = MONITORINFO::default();
    monitor_info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    let monitor: HMONITOR = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY) };
    if monitor.is_invalid() {
        return false;
    }
    if !unsafe { GetMonitorInfoW(monitor, &mut monitor_info) }.as_bool() {
        return false;
    }
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return false;
    }
    rect.left == monitor_info.rcMonitor.left
        && rect.right == monitor_info.rcMonitor.right
        && rect.top == monitor_info.rcMonitor.top
        && rect.bottom == monitor_info.rcMonitor.bottom
}
