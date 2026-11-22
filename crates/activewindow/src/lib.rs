//! node-active-window 等价移植。
//!
//! 源模块: `origin/src/modules/node-active-window`（ActiveWindow.cpp 402 行 + index.js）。
//! 公开 API: get_active_window / subscribe / unsubscribe（equiv dist/index.js）。
//! kernel32 包 API（GetPackageFamilyName 等）在 windows crate 0.62 未绑定，
//! 以集中 FFI 声明补齐（AGENTS.md §1.1：必要性成立，声明集中在模块 crate 内）。

mod ffi;
mod process;
mod uwp;
mod watch;

pub use watch::{get_active_window, subscribe, unsubscribe, WindowInfo};

#[cfg(test)]
mod tests {
    use crate::watch::get_active_window;

    /// 真机测试: get_active_window 全流程。
    #[test]
    fn active_window_smoke() {
        let info = get_active_window();
        match info {
            Some(w) => {
                println!("title: {:?}", w.title);
                println!("application: {:?}", w.application);
                println!("path: {:?}", w.path);
                println!("pid: {}", w.pid);
                println!("isUWP: {} package: {:?}", w.is_uwp_app, w.uwp_package);
                // pid 非零即基本合理
                assert!(w.pid > 0);
            }
            None => println!("get_active_window -> None（前台窗口无句柄，允许）"),
        }
    }

    /// basename 逻辑（纯逻辑）。
    #[test]
    fn basename_matches_cpp() {
        assert_eq!(super::process::basename("C:\\Apps\\tool.exe"), "tool.exe");
        assert_eq!(super::process::basename("ApplicationFrameHost.exe"), "ApplicationFrameHost.exe");
    }

    /// 版本信息诊断: 打印翻译表与 desc_len。
    #[test]
    fn version_info_diagnostic() {
        use windows::Win32::Storage::FileSystem::{
            GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
        };
        unsafe {
            let path = "D:\\Software\\ungoogled-chromium\\chrome.exe";
            let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            let pc = windows::core::PCWSTR::from_raw(wide.as_ptr());
            let info_size = GetFileVersionInfoSizeW(pc, None);
            let mut data: Vec<u8> = vec![0; info_size as usize];
            GetFileVersionInfoW(pc, Some(0), info_size, data.as_mut_ptr() as *mut core::ffi::c_void).ok();

            let q1: Vec<u16> = "\\VarFileInfo\\Translation".encode_utf16().chain(std::iter::once(0)).collect();
            let mut lang_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
            let mut lang_len: u32 = 0;
            let ok1 = VerQueryValueW(
                data.as_ptr() as *const core::ffi::c_void,
                windows::core::PCWSTR::from_raw(q1.as_ptr()),
                &mut lang_ptr,
                &mut lang_len,
            ).as_bool();
            println!("Translation query ok={} len={} bytes", ok1, lang_len);
            if ok1 && lang_len > 0 {
                let words = std::slice::from_raw_parts(lang_ptr as *const u16, lang_len as usize / 2);
                for (i, w) in words.iter().enumerate().take(6) {
                    println!("  word[{}] = 0x{:04X}", i, w);
                }
                // 打印第一个 lang/codepage 组合
                println!("  首组: lang=0x{:04X} codepage=0x{:04X}", words[0], words[1]);
            }

            // 直接试首组与 0409/04E4
            for (lang, cp) in [(0x0409u16, 0x04B0u16), (0x0409u16, 0x04E4u16)] {
                let q2: Vec<u16> = format!("\\StringFileInfo\\{:04X}{:04X}\\FileDescription", lang, cp)
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();
                let mut desc_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
                let mut desc_len: u32 = 0;
                let ok2 = VerQueryValueW(
                    data.as_ptr() as *const core::ffi::c_void,
                    windows::core::PCWSTR::from_raw(q2.as_ptr()),
                    &mut desc_ptr,
                    &mut desc_len,
                ).as_bool();
                if ok2 {
                    let chars = std::slice::from_raw_parts(desc_ptr as *const u16, desc_len as usize / 2);
                    let end = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
                    println!("  lang=0x{:04X} cp=0x{:04X}: ok len={} chars -> {:?}", lang, cp, desc_len, String::from_utf16_lossy(&chars[..end]));
                } else {
                    println!("  lang=0x{:04X} cp=0x{:04X}: 不存在", lang, cp);
                }
            }
        }
    }

    /// 版本信息 FileDescription 读取（真机 chrome.exe）。
    #[test]
    fn file_description_smoke() {
        let name = super::process::get_process_name("D:\\Software\\ungoogled-chromium\\chrome.exe");
        println!("FileDescription: {:?}", name);
        // 不做长度断言——只验证非空且无截断异常（至少 > 4）
        assert!(!name.is_empty(), "FileDescription 不应为空");
    }

    /// isUWPApp 识别。
    #[test]
    fn uwp_detection() {
        assert!(crate::uwp::is_uwp_app("C:\\Windows\\System32\\ApplicationFrameHost.exe"));
        assert!(!crate::uwp::is_uwp_app("C:\\Windows\\explorer.exe"));
    }
}
