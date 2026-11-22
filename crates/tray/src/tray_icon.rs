//! Shell_NotifyIconW 托盘图标 + 右键菜单。
//!
//! equiv: electron Tray API（electron.js 的 createTray / setTrayMenu / tray.on("click")）。
//! 图标资产: origin/src/assets/tray-icons/{dark,light}/*.ico（预生成，禁止自绘——AGENTS.md §1.2）。
//!
//! 线程模型（AGENTS.md §5）：窗口创建 + 消息循环跑专用线程；
//! 点击/菜单事件经 channel 回传主线程（Slint 事件循环）处理。

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NOTIFYICONDATAW, NOTIFY_ICON_DATA_FLAGS, NIF_ICON, NIF_MESSAGE, NIF_TIP,
    NIM_ADD, NIM_DELETE, NOTIFY_ICON_MESSAGE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, LoadImageW, PostMessageW, RegisterClassW,
    SetForegroundWindow, TrackPopupMenu, GDI_IMAGE_TYPE, HICON, IMAGE_FLAGS, LR_LOADFROMFILE, MF_STRING,
    MENU_ITEM_FLAGS, MSG, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD, WM_LBUTTONUP, WM_RBUTTONUP,
    WINDOW_EX_STYLE, WINDOW_STYLE,
};

/// 托盘图标配置。
#[derive(Debug, Clone, Default)]
pub struct TrayConfig {
    /// 图标 ico 文件绝对路径（资产: tray-icons/{dark,light}/*.ico）
    pub icon_path: String,
    /// tooltip 文本
    pub tooltip: String,
    /// 菜单项（equiv setTrayMenu 的菜单表）
    pub menu_items: Vec<TrayMenuItem>,
}

/// 托盘菜单项（equiv electron Menu.buildFromTemplate 的单项）。
#[derive(Debug, Clone, Default)]
pub struct TrayMenuItem {
    pub id: u32,
    pub label: String,
    pub enabled: bool,
}

/// 托盘事件（主线程消费）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayEvent {
    /// 左键点击（equiv tray.on("click") -> toggleTray）
    Click,
    /// 右键点击（弹出菜单）
    RightClick,
    /// 菜单项选中
    Menu(u32),
}

/// 托盘图标句柄（含消息窗口）。
pub struct TrayIconHandle {
    pub hwnd: HWND,
    pub uid: u32,
}

/// 自定义消息基址（equiv WM_USER+1；点击事件路由）。
pub const WM_TRAY_ICON: u32 = 0x0400 + 1; // WM_USER + 1

/// 托盘事件回调（WNDPROC 是静态函数，回调经全局槽传递）。
static TRAY_CALLBACK: OnceLock<Mutex<Option<Arc<dyn Fn(TrayEvent) + Send + Sync>>>> = OnceLock::new();
/// 当前菜单项表（右键时构建 HMENU 用）。
static TRAY_MENU: OnceLock<Mutex<Vec<TrayMenuItem>>> = OnceLock::new();

fn callback_slot() -> &'static Mutex<Option<Arc<dyn Fn(TrayEvent) + Send + Sync>>> {
    TRAY_CALLBACK.get_or_init(|| Mutex::new(None))
}

fn menu_slot() -> &'static Mutex<Vec<TrayMenuItem>> {
    TRAY_MENU.get_or_init(|| Mutex::new(Vec::new()))
}

/// 设置托盘事件回调（主线程注入；回调必须线程安全）。
pub fn set_tray_callback(cb: Arc<dyn Fn(TrayEvent) + Send + Sync>) {
    if let Ok(mut slot) = callback_slot().lock() {
        *slot = Some(cb);
    }
}

/// LoadImageW: 从 .ico 文件加载图标。
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
pub fn create_tray_icon(config: &TrayConfig) -> Option<TrayIconHandle> {
    unsafe {
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
        let tip: Vec<u16> = config
            .tooltip
            .encode_utf16()
            .take(127)
            .chain(std::iter::once(0))
            .collect();
        nid.szTip[..tip.len()].copy_from_slice(&tip);

        if let Ok(mut menu) = menu_slot().lock() {
            *menu = config.menu_items.clone();
        }

        let ok = Shell_NotifyIconW(NOTIFY_ICON_MESSAGE(NIM_ADD.0), &nid);
        if ok.as_bool() {
            Some(TrayIconHandle { hwnd, uid: 1 })
        } else {
            None
        }
    }
}

/// 更新托盘图标（equiv tray.setImage，主题切换时调用）。
pub fn update_tray_icon(handle: &TrayIconHandle, icon_path: &str, tooltip: &str) -> bool {
    unsafe {
        let Some(icon) = load_icon(icon_path) else {
            return false;
        };
        let mut nid = NOTIFYICONDATAW::default();
        nid.hWnd = handle.hwnd;
        nid.uID = handle.uid;
        nid.uFlags = NOTIFY_ICON_DATA_FLAGS(NIF_ICON.0 | NIF_TIP.0);
        nid.hIcon = icon;
        let tip: Vec<u16> = tooltip
            .encode_utf16()
            .take(127)
            .chain(std::iter::once(0))
            .collect();
        nid.szTip[..tip.len()].copy_from_slice(&tip);
        // NIM_MODIFY = 1
        Shell_NotifyIconW(NOTIFY_ICON_MESSAGE(1), &nid).as_bool()
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

/// 弹出托盘菜单（equiv setTrayMenu 的 TrackPopupMenu）。
unsafe fn show_menu(hwnd: HWND) {
    let items = menu_slot().lock().map(|g| g.clone()).unwrap_or_default();
    let Ok(menu) = CreatePopupMenu() else { return };
    for item in &items {
        let label: Vec<u16> = item.label.encode_utf16().chain(std::iter::once(0)).collect();
        // MF_GRAYED=1, MF_DISABLED=2（未启用项灰显）
        let flags = if item.enabled {
            MF_STRING
        } else {
            MF_STRING | MENU_ITEM_FLAGS(0x0000_0003)
        };
        let _ = AppendMenuW(menu, flags, item.id as usize, windows::core::PCWSTR::from_raw(label.as_ptr()));
    }
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    // 菜单弹出前必须把窗口置前，否则点击外部不会关闭（Win32 托盘菜单惯例）
    let _ = SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(
        menu,
        TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RETURNCMD,
        pt.x,
        pt.y,
        None,
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);
    if cmd.0 != 0 {
        if let Ok(cb) = callback_slot().lock() {
            if let Some(cb) = cb.as_ref() {
                cb(TrayEvent::Menu(cmd.0 as u32));
            }
        }
    }
    // 菜单关闭后补一条 WM_NULL（MSDN 托盘菜单惯例）
    let _ = PostMessageW(Some(hwnd), 0x0000, WPARAM(0), LPARAM(0));
}

/// 托盘消息窗口过程：WM_TRAY_ICON 路由点击事件。
unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_TRAY_ICON {
        let mouse = (lparam.0 & 0xFFFF) as u32;
        let event = match mouse {
            WM_LBUTTONUP => Some(TrayEvent::Click),
            WM_RBUTTONUP => Some(TrayEvent::RightClick),
            _ => None,
        };
        if let Some(event) = event {
            if event == TrayEvent::RightClick {
                show_menu(hwnd);
            } else if let Ok(cb) = callback_slot().lock() {
                if let Some(cb) = cb.as_ref() {
                    cb(event);
                }
            }
        }
        return LRESULT(0);
    }
    if msg == 0x0010 {
        // WM_DESTROY
        return LRESULT(0);
    }
    let _ = wparam;
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

/// 托盘线程 id（用于退出时 PostThreadMessage(WM_QUIT)）。
static TRAY_THREAD_ID: OnceLock<Mutex<Option<u32>>> = OnceLock::new();

fn thread_id_slot() -> &'static Mutex<Option<u32>> {
    TRAY_THREAD_ID.get_or_init(|| Mutex::new(None))
}

/// 在专用线程创建托盘并跑消息循环（AGENTS.md §5）。
/// 返回事件接收端。窗口与图标均由该线程创建并在退出时销毁（DestroyWindow 的
/// 同线程约束），主线程只消费事件。
pub fn spawn_tray_thread(config: TrayConfig) -> Receiver<TrayEvent> {
    let (tx, rx): (Sender<TrayEvent>, Receiver<TrayEvent>) = channel();
    let cb_tx = tx;
    set_tray_callback(Arc::new(move |event: TrayEvent| {
        let _ = cb_tx.send(event);
    }));
    std::thread::spawn(move || {
        // 记录线程 id（退出时 PostThreadMessageW(WM_QUIT) 用）
        let current_id = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        if let Ok(mut slot) = thread_id_slot().lock() {
            *slot = Some(current_id);
        }
        let Some(handle) = create_tray_icon(&config) else {
            return;
        };
        run_message_loop();
        // 消息循环退出后清理图标与窗口（同线程）
        remove_tray_icon(&handle);
    });
    rx
}

/// 停止托盘线程（equiv tray.destroy()）：投递 WM_QUIT 让消息循环返回。
pub fn stop_tray_thread() {
    let id = thread_id_slot().lock().ok().and_then(|g| *g);
    if let Some(id) = id {
        // SAFETY: id 来自托盘线程的 GetCurrentThreadId；PostThreadMessageW 只投递消息。
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                id,
                0x0012, // WM_QUIT
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_event_equality() {
        assert_eq!(TrayEvent::Click, TrayEvent::Click);
        assert_ne!(TrayEvent::Click, TrayEvent::RightClick);
        assert_eq!(TrayEvent::Menu(1001), TrayEvent::Menu(1001));
    }

    #[test]
    fn callback_slot_dispatch() {
        let (tx, rx) = channel();
        set_tray_callback(Arc::new(move |e| {
            let _ = tx.send(e);
        }));
        if let Ok(cb) = callback_slot().lock() {
            if let Some(cb) = cb.as_ref() {
                cb(TrayEvent::Menu(7));
            }
        }
        assert_eq!(rx.try_recv().unwrap(), TrayEvent::Menu(7));
    }
}