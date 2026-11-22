//! UWP 应用识别与包信息。
//!
//! equiv: ActiveWindow.cpp:L249-L283（isUWPApp/getUWPPackage）、L336-L367（EnumChildWindowsCb）。
//! AppxManifest 解析（IAppxFactory 链）在原版仅用于读 DisplayName——本移植按 AGENTS.md §4
//! 降级为读取 package family name（已含包身份），降低 COM 依赖面；记录论证。

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Foundation::HWND;

/// isUWPApp 等价（equiv cpp:L249-L253）：basename == ApplicationFrameHost.exe。
pub(crate) fn is_uwp_app(path: &str) -> bool {
    super::process::basename(path) == "ApplicationFrameHost.exe"
}

/// getUWPPackage 等价（equiv cpp:L255-L275）。
pub(crate) fn get_uwp_package(hproc: HANDLE) -> String {
    crate::ffi::get_package_family_name(hproc).unwrap_or_default()
}

/// EnumChildWindowsCb 等价（equiv cpp:L336-L367）：
/// 找到第一个 UWP 子窗口进程（有包身份），返回其可执行路径与句柄。
#[allow(dead_code)]
pub(crate) struct UwpChildResult {
    pub path: String,
    pub hproc: HANDLE,
}

/// 对应 C++ EnumChildWindowsCbParam。
#[allow(dead_code)]
pub(crate) struct EnumParam {
    pub ok: bool,
    pub path: String,
    pub hproc: HANDLE,
}

/// 等价 EnumChildWindowsCb。返回 false 时停止枚举（WINAPI 语义）。
pub(crate) unsafe extern "system" fn enum_child_windows_cb(
    hwindow: HWND,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::core::BOOL {
    let cb_param = &mut *(lparam.0 as *mut EnumParam);

    let mut pid: u32 = 0;
    // SAFETY: pid 出参合法
    windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwindow, Some(&mut pid));
    // SAFETY: 只读句柄，PROCESS_QUERY_LIMITED_INFORMATION
    let hproc = match windows::Win32::System::Threading::OpenProcess(
        windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION,
        false,
        pid,
    ) {
        Ok(h) => h,
        Err(_) => return windows::core::BOOL(1), // true: 继续
    };

    cb_param.path = super::process::get_process_path(hproc);
    cb_param.hproc = hproc;

    // 无包身份（非 UWP）-> 关闭并继续
    if crate::ffi::get_package_family_name(hproc).is_none() {
        let _ = windows::Win32::Foundation::CloseHandle(hproc);
        return windows::core::BOOL(1); // true: 继续
    }

    cb_param.ok = true;
    windows::core::BOOL(0) // false: 停止枚举
}
