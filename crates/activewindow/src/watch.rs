//! 活动窗口监视。
//!
//! equiv: ActiveWindow.cpp:L17-L148（getActiveWindow/watch）、L299-L335（runWatchThread）、
//! L369-L402（WinEventProcCb）。
//!
//! 线程模型：专用 Win32 消息循环线程 + 双 SetWinEventHook（逐事件挂，避免
//! LOCATIONCHANGE 洪泛——原注释语义），500ms 定时器保活消息循环。

use std::collections::HashMap;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

/// 等价 index.js encodeWindowInfo 输出。
#[derive(Debug, Clone, Default)]
pub struct WindowInfo {
    pub title: String,
    pub application: String,
    pub path: String,
    pub pid: u32,
    /// windows.isUWPApp
    pub is_uwp_app: bool,
    /// windows.uwpPackage
    pub uwp_package: String,
}

static WATCHES: Mutex<Option<HashMap<u64, std::sync::mpsc::Sender<Option<WindowInfo>>>>> = Mutex::new(None);
static NEXT_WATCH_ID: AtomicU64 = AtomicU64::new(1);
static THREAD_SHOULD_EXIT: AtomicBool = AtomicBool::new(false);
static THREAD_ID: AtomicU64 = AtomicU64::new(0);
static THREAD_HANDLE: Mutex<usize> = Mutex::new(0);

/// getActiveWindow 等价（equiv cpp:L23-L86）。
pub fn get_active_window() -> Option<WindowInfo> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId,
    };

    // SAFETY: 全部为无副作用查询调用
    unsafe {
        let h = GetForegroundWindow();
        if h.is_invalid() || h.0.is_null() {
            return None;
        }

        let mut info = WindowInfo::default();
        info.title = crate::process::get_window_title(h);

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(h, Some(&mut pid));
        info.pid = pid;

        let hproc = match OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        ) {
            Ok(h) => h,
            Err(_) => return None,
        };

        info.path = crate::process::get_process_path(hproc);
        let _ = windows::Win32::Foundation::CloseHandle(hproc);

        let mut uwp_hproc: Option<HANDLE> = None;

        // check if app is UWP app（原注释语义）
        if crate::uwp::is_uwp_app(&info.path) {
            info.is_uwp_app = true;

            let mut cb_param = crate::uwp::EnumParam {
                ok: false,
                path: String::new(),
                hproc: HANDLE::default(),
            };
            // SAFETY: cb_param 生命周期覆盖枚举
            let _ = windows::Win32::UI::WindowsAndMessaging::EnumChildWindows(
                Some(h),
                Some(crate::uwp::enum_child_windows_cb),
                windows::Win32::Foundation::LPARAM(&mut cb_param as *mut _ as isize),
            );

            if !cb_param.ok {
                return None;
            }
            info.path = cb_param.path;
            uwp_hproc = Some(cb_param.hproc);
        }

        // get app name（原版先 FileDescription，失败回退 basename）
        info.application = crate::process::get_process_name(&info.path);
        if info.application.is_empty() {
            info.application = crate::process::basename(&info.path);
        }

        if let Some(hp) = uwp_hproc {
            info.uwp_package = crate::uwp::get_uwp_package(hp);
            let _ = windows::Win32::Foundation::CloseHandle(hp);
        }

        Some(info)
    }
}

/// subscribe 等价（equiv cpp:L88-L105）。返回 watch id；事件推送到 Receiver。
pub fn subscribe() -> (u64, std::sync::mpsc::Receiver<Option<WindowInfo>>) {
    use std::sync::mpsc::channel;
    let (tx, rx) = channel();
    let watch_id = NEXT_WATCH_ID.fetch_add(1, Ordering::SeqCst);

    {
        let mut watches = WATCHES.lock().unwrap();
        let should_start = watches.as_ref().map(|m| m.is_empty()).unwrap_or(true);
        watches.get_or_insert_with(HashMap::new).insert(watch_id, tx);
        drop(watches);
        if should_start {
            start_watch_thread();
        }
    }

    (watch_id, rx)
}

/// unsubscribe 等价（equiv cpp:L107-L118）。无监听者时停线程。
pub fn unsubscribe(watch_id: u64) {
    let should_stop = {
        let mut watches = WATCHES.lock().unwrap();
        if let Some(map) = watches.as_mut() {
            map.remove(&watch_id);
            map.is_empty()
        } else {
            true
        }
    };
    if should_stop {
        stop_watch_thread();
    }
}

/// stopWatchThread 等价（equiv cpp:L120-L140）：置退出标志 + WM_NULL 唤醒 + join。
pub(crate) fn stop_watch_thread() {
    THREAD_SHOULD_EXIT.store(true, Ordering::SeqCst);
    let tid = THREAD_ID.load(Ordering::SeqCst) as u32;
    if tid != 0 {
        // SAFETY: PostThreadMessageW 到本模块创建的线程
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                tid,
                windows::Win32::UI::WindowsAndMessaging::WM_NULL,
                windows::Win32::Foundation::WPARAM(0),
                windows::Win32::Foundation::LPARAM(0),
            );
        }
    }
    let handle_addr = *THREAD_HANDLE.lock().unwrap();
    if handle_addr != 0 {
        // SAFETY: handle 来自 CreateThread 的合法句柄
        let handle = windows::Win32::Foundation::HANDLE(handle_addr as *mut core::ffi::c_void);
        unsafe {
            let _ = windows::Win32::System::Threading::WaitForSingleObject(handle, 10_000);
            let _ = windows::Win32::Foundation::CloseHandle(handle);
        }
        *THREAD_HANDLE.lock().unwrap() = 0;
        THREAD_ID.store(0, Ordering::SeqCst);
    }
}

/// 启动监视线程（等价 watchActiveWindow 里的 thread 启动）。
fn start_watch_thread() {
    THREAD_SHOULD_EXIT.store(false, Ordering::SeqCst);
    // SAFETY: CreateThread 无共享栈依赖
    let mut tid: u32 = 0;
    let handle = unsafe {
        windows::Win32::System::Threading::CreateThread(
            None,
            0,
            Some(watch_thread_proc),
            None,
            windows::Win32::System::Threading::THREAD_CREATION_FLAGS(0),
            Some(&mut tid),
        )
    };
    if let Ok(h) = handle {
        THREAD_ID.store(tid as u64, Ordering::SeqCst);
        *THREAD_HANDLE.lock().unwrap() = h.0 as usize;
    }
}

/// WinEventProcCb 等价（equiv cpp:L369-L402）。
unsafe extern "system" fn win_event_proc_cb(
    _hhook: HWINEVENTHOOK,
    event: u32,
    _hwindow: HWND,
    _idobject: i32,
    _idchild: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
    const EVENT_OBJECT_NAMECHANGE: u32 = 0x800C;

    if event != EVENT_SYSTEM_FOREGROUND && event != EVENT_OBJECT_NAMECHANGE {
        // not interested in these（原注释）
        return;
    }

    let foreground_window = windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
    if event == EVENT_OBJECT_NAMECHANGE && (_hwindow.0 != foreground_window.0) {
        // name changed, but not for current window（原注释）
        return;
    }

    let info = get_active_window();

    // notify every callback
    let watches = WATCHES.lock().unwrap();
    if let Some(map) = watches.as_ref() {
        for tx in map.values() {
            let _ = tx.send(info.clone());
        }
    }
}

/// runWatchThread 等价（equiv cpp:L299-L335）。
/// SAFETY 前置：本函数只操作本线程的消息队列与钩子。
unsafe extern "system" fn watch_thread_proc(_param: *mut core::ffi::c_void) -> u32 {
    // Hook each event on its own. A single EVENT_SYSTEM_FOREGROUND..EVENT_OBJECT_NAMECHANGE range
    // also covers frequent events like EVENT_OBJECT_LOCATIONCHANGE, which Windows would then have
    // to deliver to us from every app, slowing down their UI.（原注释逐字保留）
    let h_foreground_hook = SetWinEventHook(
        0x0003, // EVENT_SYSTEM_FOREGROUND
        0x0003,
        None,
        Some(win_event_proc_cb),
        0,
        0,
        0x0000_0000u32, // WINEVENT_OUTOFCONTEXT
    );
    let h_name_change_hook = SetWinEventHook(
        0x800C, // EVENT_OBJECT_NAMECHANGE
        0x800C,
        None,
        Some(win_event_proc_cb),
        0,
        0,
        0x0000_0000u32,
    );

    THREAD_ID.store(
        windows::Win32::System::Threading::GetCurrentThreadId() as u64,
        Ordering::SeqCst,
    );

    // run message loop at least every 500 ms（原注释）
    let timer = windows::Win32::UI::WindowsAndMessaging::SetTimer(None, 0usize, 500, None);

    let mut msg = Default::default();
    loop {
        if THREAD_SHOULD_EXIT.load(Ordering::Relaxed) {
            break;
        }
        let ret = windows::Win32::UI::WindowsAndMessaging::GetMessageW(&mut msg, None, 0, 0);
        if ret.0 == -1 {
            continue;
        }
        let _ = windows::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
        windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
    }

    // remove timer
    let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(None, timer);
    // remove hooks
    let _ = windows::Win32::UI::Accessibility::UnhookWinEvent(h_foreground_hook);
    let _ = windows::Win32::UI::Accessibility::UnhookWinEvent(h_name_change_hook);
    0
}
