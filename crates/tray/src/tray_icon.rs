//! Shell_NotifyIconW 托盘图标。
//!
//! equiv: electron Tray API（panel.js 的 tray 图标切换 dark/light 主题）。
//! 图标资产: origin/src/assets/tray-icons/{dark,light}/*.ico（预生成，禁止自绘——AGENTS.md §1.2）。

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NOTIFYICONDATAW, NOTIFY_ICON_DATA_FLAGS, NIF_ICON, NIF_MESSAGE, NIF_TIP,
    NIM_ADD, NIM_DELETE, NOTIFY_ICON_MESSAGE,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, LoadImageW,
    RegisterClassW, GDI_IMAGE_TYPE, HICON, IMAGE_FLAGS, LR_LOADFROMFILE, MSG, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_LBUTTONUP,
};

/// 托盘图标配置。
#[derive(Debug, Clone, Default)]
pub struct TrayConfig {
    /// 图标 ico 文件绝对路径（资产: tray-icons/{dark,light}/*.ico）
    pub icon_path: String,
    /// tooltip 文本
    pub tooltip: String,
}

/// 托盘图标句柄（含消息窗口，点击/悬停事件经此路由）。
pub struct TrayIconHandle {
    pub hwnd: HWND,
    pub uid: u32,
}

/// 自定义消息基址（equiv WM_USER+1；点击事件路由）。
pub const WM_TRAY_ICON: u32 = 0x0400 + 1; // WM_USER + 1

/// LoadImageW: 从 .ico 文件加载图标（0 = LR_DEFAULTSIZE 语义交给系统）。
unsafe fn load_icon(path: &str) -> Option<HICON> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: LR_LOADFROMFILE 时 path 为合法 NUL 结尾宽字符串
    let icon = LoadImageW(
        None,
        windows::core::PCWSTR::from_raw(wide.as_ptr()),
        GDI_IMAGE_TYPE(1), // IMAGE_ICON
        0,
        0,
        IMAGE_FLAGS(LR_LOADFROMFILE.0),
    );
    let handle = icon.ok()?;
    if handle.is_invalid() {
        None
    } else {
        Some(HICON(handle.0))
    }
}

/// 创建托盘图标（NIM_ADD）。
/// 点击事件需主线程在消息循环里处理 WM_TRAY_ICON。
pub fn create_tray_icon(config: &TrayConfig) -> Option<TrayIconHandle> {
    unsafe {
        // 消息窗口（承载托盘回调）
        let class_name = windows::core::w!("TwinkleTrayTrayClass");
        let wc = windows::Win32::UI::WindowsAndMessaging::WNDCLASSW {
            lpfnWndProc: Some(tray_wnd_proc),
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            windows::core::w!(""),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(GetModuleHandleW(None).unwrap_or_default().into()),
            None,
        )
        .ok()?;

        let icon = load_icon(&config.icon_path)?;

        let mut nid = NOTIFYICONDATAW::default();
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NOTIFY_ICON_DATA_FLAGS(NIF_MESSAGE.0 | NIF_ICON.0 | NIF_TIP.0);
        nid.uCallbackMessage = WM_TRAY_ICON;
        nid.hIcon = icon;
        // tooltip 宽字符（最多 127 + NUL）
        let tip: Vec<u16> = config
            .tooltip
            .encode_utf16()
            .take(127)
            .chain(std::iter::once(0))
            .collect();
        nid.szTip[..tip.len()].copy_from_slice(&tip);

        let ok = Shell_NotifyIconW(NOTIFY_ICON_MESSAGE(NIM_ADD.0), &nid);
        if ok.as_bool() {
            Some(TrayIconHandle { hwnd, uid: 1 })
        } else {
            None
        }
    }
}

/// 移除托盘图标（NIM_DELETE）。
pub fn remove_tray_icon(handle: &TrayIconHandle) {
    unsafe {
        let mut nid = NOTIFYICONDATAW::default();
        nid.hWnd = handle.hwnd;
        nid.uID = handle.uid;
        let _ = Shell_NotifyIconW(NOTIFY_ICON_MESSAGE(NIM_DELETE.0), &nid);
        let _ = DestroyWindow(handle.hwnd);
    }
}

/// 托盘消息窗口过程：WM_TRAY_ICON 路由点击事件。
unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_TRAY_ICON {
        // lparam 低字为鼠标事件（WM_LBUTTONUP = 点击等价）
        let mouse = (lparam.0 & 0xFFFF) as u32;
        if mouse == WM_LBUTTONUP {
            // 主进程回调（经全局注册）——骨架先打印
            println!("tray clicked");
        }
        return LRESULT(0);
    }
    if msg == 0x0010 {
        // WM_DESTROY
        return LRESULT(0);
    }
    let _ = (hwnd, wparam);
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// 托盘消息循环（专用线程调用；等价 Electron 主进程的消息泵）。
pub fn run_message_loop() {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = DispatchMessageW(&msg);
        }
    }
}
