//! kernel32 包 API 的最小 FFI 声明。
//!
//! windows crate 0.62 未绑定这些 kernel32 API（GetPackageFamilyName/GetPackageId/GetPackagePath）。
//! SAFETY 契约与 MSDN 一致；仅在本 crate 内使用。


/// PACKAGE_ID（kernel32 appmodel.h），只含用到的字段前缀。
#[repr(C)]
#[allow(dead_code)]
pub(crate) struct PackageId {
    pub reserved: u32,
    pub processor_architecture: u16,
    pub version: Version,
    pub name: [u16; 64],
    pub publisher: [u16; 64],
    pub resource_id: [u16; 5],
    pub publisher_id: [u16; 33],
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
#[allow(dead_code)]
pub(crate) struct Version {
    pub major: u16,
    pub minor: u16,
    pub build: u16,
    pub revision: u16,
}

/// APPMODEL_ERROR_NO_PACKAGE（kernel32）
#[allow(dead_code)]
pub(crate) const APPMODEL_ERROR_NO_PACKAGE: i32 = 15700;

windows::core::link!("kernel32.dll" "system" fn GetPackageFamilyName(hprocess : windows::Win32::Foundation::HANDLE, packagefamilynamelength : *mut u32, packagefamilyname : windows::core::PWSTR) -> i32);
windows::core::link!("kernel32.dll" "system" fn GetPackageId(hprocess : windows::Win32::Foundation::HANDLE, bufferlength : *mut u32, buffer : *mut u8) -> i32);
windows::core::link!("kernel32.dll" "system" fn GetPackagePath(packageid : *const PackageId, bufferlength : *mut u32, buffer : windows::core::PWSTR) -> u32);

/// 安全包装：GetPackageFamilyName 两步调用。失败/无包 -> None。
pub(crate) fn get_package_family_name(hproc: windows::Win32::Foundation::HANDLE) -> Option<String> {
    unsafe {
        let mut len: u32 = 0;
        // SAFETY: 第一次调用长度为 0 拿所需长度（MSDN 语义）
        let hr = GetPackageFamilyName(hproc, &mut len, windows::core::PWSTR::null());
        if hr != 0 && len == 0 {
            return None;
        }
        if len == 0 {
            return None;
        }
        let mut buf: Vec<u16> = vec![0; len as usize];
        // SAFETY: buf 长度与 len 匹配
        let hr = GetPackageFamilyName(hproc, &mut len, windows::core::PWSTR(buf.as_mut_ptr()));
        if hr != 0 {
            return None;
        }
        // len 含结尾 NUL（MSDN）
        let end = len.saturating_sub(1) as usize;
        Some(String::from_utf16_lossy(&buf[..end]))
    }
}

/// 安全包装：GetPackageId + GetPackagePath 两步组合。失败 -> None。
#[allow(dead_code)]
pub(crate) fn get_package_path(hproc: windows::Win32::Foundation::HANDLE) -> Option<String> {
    unsafe {
        let mut pkg_len: u32 = 0;
        // SAFETY: 第一次调用取所需长度
        let _ = GetPackageId(hproc, &mut pkg_len, std::ptr::null_mut());
        if pkg_len == 0 {
            return None;
        }
        let mut pkg_buf: Vec<u8> = vec![0; pkg_len as usize];
        // SAFETY: buf 长度匹配
        if GetPackageId(hproc, &mut pkg_len, pkg_buf.as_mut_ptr()) != 0 {
            return None;
        }
        let pkg_id = pkg_buf.as_ptr() as *const PackageId;

        let mut path_len: u32 = 0;
        // SAFETY: 第一次调用取所需长度（bufferlength 为出入参）
        let _ = GetPackagePath(pkg_id, &mut path_len, windows::core::PWSTR::null());
        if path_len == 0 {
            return None;
        }
        let mut buf: Vec<u16> = vec![0; path_len as usize];
        // SAFETY: buf 长度匹配
        if GetPackagePath(pkg_id, &mut path_len, windows::core::PWSTR(buf.as_mut_ptr())) != 0 {
            return None;
        }
        // C++ 侧 buf.begin()..buf.begin()+len-1：去掉结尾 NUL
        let end = path_len.saturating_sub(1) as usize;
        Some(String::from_utf16_lossy(&buf[..end]))
    }
}

#[allow(unused_imports)]
use windows as _;
