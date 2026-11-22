//! 窗口材质（DwmSetWindowAttribute）。
//!
//! equiv: windows_window_material.cc（87 行）。

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};

const USE_IMMERSIVE_DARK_MODE: u32 = 20;
const WINDOW_CORNER_PREFERENCE: u32 = 33;
const SYSTEM_BACKDROP_TYPE: u32 = 38;

/// setWindowMaterial 等价：三个 DWM 属性全部成功才算成功。
pub fn set_window_material(hwnd: HWND, backdrop_type: i32, border_type: i32, dark_mode: i32) -> bool {
    // SAFETY: 各属性均为 i32 值指针
    unsafe {
        let dark_mode_result = DwmSetWindowAttribute(
            hwnd,
            DWMWINDOWATTRIBUTE(USE_IMMERSIVE_DARK_MODE as i32),
            &dark_mode as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
        let backdrop_result = DwmSetWindowAttribute(
            hwnd,
            DWMWINDOWATTRIBUTE(SYSTEM_BACKDROP_TYPE as i32),
            &backdrop_type as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
        let corners_result = DwmSetWindowAttribute(
            hwnd,
            DWMWINDOWATTRIBUTE(WINDOW_CORNER_PREFERENCE as i32),
            &border_type as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
        dark_mode_result.is_ok() && backdrop_result.is_ok() && corners_result.is_ok()
    }
}

/// setWindowAttribute 等价：通用属性写入。
pub fn set_window_attribute(hwnd: HWND, attribute: u32, value: i32) -> bool {
    // SAFETY: i32 值指针
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWINDOWATTRIBUTE(attribute as i32),
            &value as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        )
    }
    .is_ok()
}

/// index.js 便捷封装等价（setWindowCorners/setDarkModeSupported/setTransitionSupported）。
pub fn set_window_corners(hwnd: HWND, corner_type: i32) -> bool {
    set_window_attribute(hwnd, 33, corner_type)
}

pub fn set_dark_mode_supported(hwnd: HWND, enabled: i32) -> bool {
    set_window_attribute(hwnd, 20, enabled)
}

pub fn set_transition_supported(hwnd: HWND, enabled: i32) -> bool {
    set_window_attribute(hwnd, 3, enabled)
}
