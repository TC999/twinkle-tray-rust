//! 窗口材质：亚克力/模糊（SetWindowCompositionAttribute）+ DWM 属性。
//!
//! equiv: `origin/src/modules/acrylic/acrylic.cc`（124 行）。
//! DWM 部分（windows_window_material.cc）已由 twutils::window_material 移植，
//! 本 crate 等价 acrylic.cc 全文并复导出 DWM API 以统一入口。

use windows::Win32::Foundation::HWND;

/// AccentState（equiv acrylic.cc:L4-L12）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum AccentState {
    #[default]
    Disabled = 0,
    EnableGradient = 1,
    EnableTransparentGradient = 2,
    EnableBlurBehind = 3,
    EnableAcrylicBlurBehind = 4,
    EnableHostBackdrop = 5, // RS5 1809
    InvalidState = 6,
}

/// WindowCompositionAttribute（equiv acrylic.cc:L15-L17）。
const WCA_ACCENT_POLICY: u32 = 19;

/// AccentPolicy（equiv acrylic.cc:L19-L24）。
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct AccentPolicy {
    pub accent_state: AccentState,
    pub accent_flags: i32,
    pub gradient_color: i32,
    pub animation_id: i32,
}

/// WindowCompositionAttributeData（equiv acrylic.cc:L26-L30）。
#[repr(C)]
pub struct WindowCompositionAttributeData {
    pub attribute: u32,
    pub data: *mut core::ffi::c_void,
    pub data_size: u32,
}

/// SetWindowCompositionAttribute 的动态加载调用。
/// 原版通过 LoadLibrary("user32.dll") + GetProcAddress 获取（未文档化的 API）。
/// SAFETY: user32 必然已加载进进程（GUI 应用），GetProcAddress 拿标准导出。
unsafe fn call_set_window_composition_attribute(hwnd: HWND, data: &WindowCompositionAttributeData) -> bool {
    // SAFETY: GetProcAddress 于 user32 模块句柄
    let user32 = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
        .unwrap_or_default();
    if user32.is_invalid() {
        return false;
    }
    let proc_addr = windows::Win32::System::LibraryLoader::GetProcAddress(
        user32,
        windows::core::PCSTR::from_raw(b"SetWindowCompositionAttribute\0".as_ptr()),
    );
    let func: unsafe extern "system" fn(HWND, *mut WindowCompositionAttributeData) -> i32 =
        match proc_addr {
            Some(p) => std::mem::transmute(p),
            None => return false,
        };
    // SAFETY: data 指向的 AccentPolicy 生命周期覆盖调用
    func(hwnd, data as *const WindowCompositionAttributeData as *mut WindowCompositionAttributeData) != 0
}

/// setAcrylic 等价（equiv acrylic.cc:L36-L86）。
/// 参数顺序: hwnd, is_rs4_or_greater, r, g, b, a。
/// gradientColor = (a<<24) + (b<<16) + (g<<8) + r（逐字保留）。
/// accentFlags = 2（逐字保留）。
pub fn set_acrylic(hwnd: HWND, is_rs4_or_greater: i32, red: i32, green: i32, blue: i32, alpha: i32) -> bool {
    let gradient_color = (alpha << 24) + (blue << 16) + (green << 8) + red;
    let blur_type = if is_rs4_or_greater == 1 {
        AccentState::EnableAcrylicBlurBehind
    } else {
        AccentState::EnableBlurBehind
    };
    let mut policy = AccentPolicy {
        accent_state: blur_type,
        accent_flags: 2,
        gradient_color,
        animation_id: 0,
    };
    let data = WindowCompositionAttributeData {
        attribute: WCA_ACCENT_POLICY,
        data: &mut policy as *mut AccentPolicy as *mut core::ffi::c_void,
        data_size: std::mem::size_of::<AccentPolicy>() as u32,
    };
    // SAFETY: policy/data 生命周期覆盖调用
    unsafe { call_set_window_composition_attribute(hwnd, &data) }
}

/// disableAcrylic 等价（equiv acrylic.cc:L88-L113）。
pub fn disable_acrylic(hwnd: HWND) -> bool {
    let mut policy = AccentPolicy {
        accent_state: AccentState::Disabled,
        accent_flags: 0,
        gradient_color: 0,
        animation_id: 0,
    };
    let data = WindowCompositionAttributeData {
        attribute: WCA_ACCENT_POLICY,
        data: &mut policy as *mut AccentPolicy as *mut core::ffi::c_void,
        data_size: std::mem::size_of::<AccentPolicy>() as u32,
    };
    // SAFETY: policy/data 生命周期覆盖调用
    unsafe { call_set_window_composition_attribute(hwnd, &data) }
}

// ---- DWM 材质（复导出 twutils::window_material 语义，保持 AGENTS.md §4 material 单一入口）----

pub const USE_IMMERSIVE_DARK_MODE: u32 = 20;
pub const WINDOW_CORNER_PREFERENCE: u32 = 33;
pub const SYSTEM_BACKDROP_TYPE: u32 = 38;

/// setWindowMaterial 等价（复导出，参数语义同 twutils::window_material::set_window_material）。
pub fn set_window_material(hwnd: HWND, backdrop_type: i32, border_type: i32, dark_mode: i32) -> bool {
    twutils_dep::set_window_material(hwnd, backdrop_type, border_type, dark_mode)
}

/// setWindowAttribute 等价。
pub fn set_window_attribute(hwnd: HWND, attribute: u32, value: i32) -> bool {
    twutils_dep::set_window_attribute(hwnd, attribute, value)
}

// 依赖桥（避免循环 crate 依赖：直接内联 DWM 调用）
mod twutils_dep {
    pub fn set_window_material(hwnd: windows::Win32::Foundation::HWND, backdrop_type: i32, border_type: i32, dark_mode: i32) -> bool {
        use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
        // SAFETY: i32 值指针
        unsafe {
            let dark = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(20), &dark_mode as *const i32 as *const core::ffi::c_void, 4);
            let backdrop = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(38), &backdrop_type as *const i32 as *const core::ffi::c_void, 4);
            let corners = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(33), &border_type as *const i32 as *const core::ffi::c_void, 4);
            dark.is_ok() && backdrop.is_ok() && corners.is_ok()
        }
    }
    pub fn set_window_attribute(hwnd: windows::Win32::Foundation::HWND, attribute: u32, value: i32) -> bool {
        use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
        // SAFETY: i32 值指针
        unsafe {
            DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(attribute as i32), &value as *const i32 as *const core::ffi::c_void, 4).is_ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use windows::Win32::Foundation::HWND;

    /// disableAcrylic 对无效句柄不崩溃（返回 false 或 true 取决于系统，不 panic 即可）。
    #[test]
    fn disable_acrylic_null_window() {
        let result = super::disable_acrylic(HWND::default());
        println!("disable_acrylic(NULL) -> {}", result);
    }
}
