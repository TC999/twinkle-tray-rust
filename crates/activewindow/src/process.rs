//! 进程与窗口信息读取。
//!
//! equiv: ActiveWindow.cpp:L150-L280。

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Foundation::HWND;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};

/// LANGCODEPAGE（C++ 结构体复刻，等价 version.h 的 struct LANGCODEPAGE）
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct LangCodePage {
    pub w_language: u16,
    pub w_code_page: u16,
}

/// getWindowTitle 等价（equiv cpp:L150-L166）。
pub(crate) fn get_window_title(hwindow: HWND) -> String {
    // SAFETY: 句柄合法性由调用方保证；两步调用与 MSDN 一致
    unsafe {
        let len = windows::Win32::UI::WindowsAndMessaging::GetWindowTextLengthW(hwindow);
        if len == 0 {
            return String::new();
        }
        let mut buf: Vec<u16> = vec![0; (len + 1) as usize];
        let read = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(hwindow, &mut buf);
        if read == 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..read as usize])
    }
}

/// getProcessPath 等价（equiv cpp:L168-L180）。
pub(crate) fn get_process_path(hproc: HANDLE) -> String {
    // SAFETY: buf 按 MAX_PATH 分配，len 出参
    unsafe {
        let mut buf: Vec<u16> = vec![0; 260 /* MAX_PATH */];
        let mut len: u32 = 260;
        let ok = windows::Win32::System::Threading::QueryFullProcessImageNameW(
            hproc,
            windows::Win32::System::Threading::PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        if ok.is_err() {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..len as usize])
    }
}

/// getProcessName 等价（equiv cpp:L182-L247）：版本信息 FileDescription。
/// 默认 lang=0x0409 codePage=0x04E4（与 C++ active 初值一致）。
pub(crate) fn get_process_name(path: &str) -> String {
    // SAFETY: 版本信息 API 两步调用；VerQueryValueW 出参生命周期在 data 内
    unsafe {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let path_pcwstr = windows::core::PCWSTR::from_raw(wide.as_ptr());

        let info_size = GetFileVersionInfoSizeW(path_pcwstr, None);
        if info_size == 0 {
            return String::new();
        }
        let mut data: Vec<u8> = vec![0; info_size as usize];
        if GetFileVersionInfoW(path_pcwstr, Some(0), info_size, data.as_mut_ptr() as *mut core::ffi::c_void).is_err() {
            return String::new();
        }

        // VarFileInfoTranslation
        let mut lang_data: *mut LangCodePage = std::ptr::null_mut();
        let mut lang_data_len: u32 = 0;
        let query_wide: Vec<u16> = "\\VarFileInfo\\Translation"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut active_lang: u16 = 0x0409;
        let mut active_codepage: u16 = 0x04E4;
        if VerQueryValueW(
            data.as_ptr() as *const core::ffi::c_void,
            windows::core::PCWSTR::from_raw(query_wide.as_ptr()),
            &mut lang_data as *mut *mut LangCodePage as *mut *mut core::ffi::c_void,
            &mut lang_data_len,
        ).as_bool()
            && lang_data_len > 0
        {
            // 数组首元素（C++ langData[0]）
            let first = std::slice::from_raw_parts(lang_data, 1);
            active_lang = first[0].w_language;
            active_codepage = first[0].w_code_page;
        }

        // StringFileInfo<lang><codepage>FileDescription（4 位十六进制大写）
        let locale_path = format!(
            "\\StringFileInfo\\{:04X}{:04X}\\FileDescription",
            active_lang, active_codepage
        );
        let locale_wide: Vec<u16> = locale_path.encode_utf16().chain(std::iter::once(0)).collect();

        let mut desc_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut desc_len: u32 = 0;
        if !VerQueryValueW(
            data.as_ptr() as *const core::ffi::c_void,
            windows::core::PCWSTR::from_raw(locale_wide.as_ptr()),
            &mut desc_ptr,
            &mut desc_len,
        ).as_bool()
            || desc_len == 0
        {
            return String::new();
        }

        // C++ std::wstring((wchar_t*)localDesc)：wchar_t* 构造到首个 NUL。
        // 实测 VerQueryValueW 对字符串块返回的 pulen 是【宽字符数（含 NUL）】
        // （"Chromium\0" = 9，与 MSDN 的"字节数"描述不符，以真机对拍为准）。
        let wchars = std::slice::from_raw_parts(desc_ptr as *const u16, desc_len as usize);
        let end = wchars.iter().position(|&c| c == 0).unwrap_or(wchars.len());
        String::from_utf16_lossy(&wchars[..end])
    }
}

/// basename 等价（equiv cpp:L264-L274）。
pub(crate) fn basename(path: &str) -> String {
    match path.rfind('\\') {
        Some(idx) => path[idx + 1..].to_string(),
        None => path.to_string(),
    }
}
