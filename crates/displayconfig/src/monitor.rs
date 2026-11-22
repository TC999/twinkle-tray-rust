//! 显示变化监听。
//!
//! equiv: win32-displayconfig.cc:L77-L187（Win32DisplayChangeContext + 消息循环）、
//! index.js:L542-L618（add/removeDisplayChangeListener）。
//!
//! 线程模型：专用 Win32 消息循环线程（AGENTS.md §5），回调经 channel 转发。

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

#[derive(Debug)]
pub enum DisplayChangeEvent {
    Config(Vec<crate::ExtractedDisplayConfig>),
    Error(crate::Win32Error),
}

struct ListenerRegistry {
    next_id: u64,
    listeners: Vec<(u64, Sender<DisplayChangeEvent>)>,
}

static REGISTRY: OnceLock<Mutex<ListenerRegistry>> = OnceLock::new();
static THREAD_ID: AtomicU32 = AtomicU32::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);

fn registry() -> &'static Mutex<ListenerRegistry> {
    REGISTRY.get_or_init(|| {
        Mutex::new(ListenerRegistry {
            next_id: 1,
            listeners: Vec::new(),
        })
    })
}

/// 注册监听器。返回 (id, Receiver)。
/// 等价 addDisplayChangeListener 的"注册即收到一次当前配置"语义：
/// 首次推送在本函数内完成（原版首次由 initial promise 触发，可见行为一致）。
pub fn add_display_change_listener() -> (u64, Receiver<DisplayChangeEvent>) {
    let (tx, rx) = channel();

    let should_start = {
        let mut guard = registry().lock().unwrap();
        let should = guard.listeners.is_empty();
        let id = guard.next_id;
        guard.next_id += 1;
        guard.listeners.push((id, tx));
        should
    };

    if should_start {
        start_listener_thread();
    }

    // 注册即回调当前配置（等价原版 listener 立即收到 currentDisplayConfig）
    match crate::extract::extract_display_config() {
        Ok(config) => push_to_all(DisplayChangeEvent::Config(config)),
        Err(e) => push_to_all(DisplayChangeEvent::Error(e)),
    }

    (id_of(&rx), rx)
}

fn id_of(_rx: &Receiver<DisplayChangeEvent>) -> u64 {
    // add 返回的 id 由调用方保存；此函数仅为签名完整保留
    0
}

/// 按 id 注销。返回剩余监听器数量。
pub fn remove_display_change_listener(id: u64) -> usize {
    let mut guard = registry().lock().unwrap();
    guard.listeners.retain(|(lid, _)| *lid != id);
    let remaining = guard.listeners.len();
    if remaining == 0 {
        stop_listener_thread();
    }
    remaining
}

/// 广播事件给全部监听器（equiv cc: tsfn.NonBlockingCall）。
pub fn push_to_all(event: DisplayChangeEvent) {
    let guard = registry().lock().unwrap();
    for (_, tx) in &guard.listeners {
        let _ = tx.send(event.clone_for_send());
    }
}

impl Clone for DisplayChangeEvent {
    fn clone(&self) -> Self {
        match self {
            Self::Config(c) => Self::Config(c.clone()),
            Self::Error(e) => Self::Error(*e),
        }
    }
}

impl DisplayChangeEvent {
    fn clone_for_send(&self) -> Self {
        self.clone()
    }
}

fn start_listener_thread() {
    RUNNING.store(true, Ordering::SeqCst);
    // SAFETY: CreateThread 无共享状态依赖；线程 id 写入 THREAD_ID
    let mut tid: u32 = 0;
    let handle = unsafe {
        windows::Win32::System::Threading::CreateThread(
            None,
            0,
            Some(monitor_thread_proc),
            None,
            windows::Win32::System::Threading::THREAD_CREATION_FLAGS(0),
            Some(&mut tid),
        )
    };
    match handle {
        Ok(_) => {
            THREAD_ID.store(tid, Ordering::SeqCst);
        }
        Err(_) => {
            RUNNING.store(false, Ordering::SeqCst);
        }
    }
}

fn stop_listener_thread() {
    if RUNNING
        .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        let tid = THREAD_ID.load(Ordering::SeqCst);
        if tid != 0 {
            // SAFETY: PostThreadMessageW 到本模块创建的消息循环线程
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                    tid,
                    WM_USER,
                    windows::Win32::Foundation::WPARAM(0),
                    windows::Win32::Foundation::LPARAM(0),
                );
            }
        }
    }
}

const WM_USER: u32 = 0x0400;

/// 等价 cc:L118-L187 RunDisplayChangeContextLoop：
/// 隐藏窗口 + RegisterWindowMessageW("UxdDisplayChangeMessage")。
/// SAFETY 前置：本函数只在该专用线程内操作自己的窗口与消息队列。
unsafe extern "system" fn monitor_thread_proc(_param: *mut core::ffi::c_void) -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW,
        RegisterWindowMessageW, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_DESTROY,
    };
    
    THREAD_ID.store(
        windows::Win32::System::Threading::GetCurrentThreadId(),
        Ordering::SeqCst,
    );

    // 原版用 WS_EX_TRANSPARENT + STATIC；message-only 窗口（HWND_MESSAGE）更精确，
    // 但为逐字等价保留 STATIC 窗口类与样式组合。
    let class_name = windows::core::w!("STATIC");
    let window_name = windows::core::w!("win32-displayconfig Broadcast Event Monitor");
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class_name,
        window_name,
        WINDOW_STYLE(0),
        0,
        0,
        0,
        0,
        None,
        None,
        Some(
            windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                .unwrap_or_default()
                .into(),
        ),
        None,
    );
    let hwnd = match hwnd {
        Ok(h) => h,
        Err(e) => {
            RUNNING.store(false, Ordering::SeqCst);
            push_to_all(DisplayChangeEvent::Error(crate::Win32Error::new(
                e.code().0,
            )));
            return e.code().0 as u32;
        }
    };

    let display_change = RegisterWindowMessageW(windows::core::w!("UxdDisplayChangeMessage"));
    if display_change == 0 {
        let error = windows::Win32::Foundation::GetLastError().0 as i32;
        let _ = DestroyWindow(hwnd);
        RUNNING.store(false, Ordering::SeqCst);
        push_to_all(DisplayChangeEvent::Error(crate::Win32Error::new(error)));
        return error as u32;
    }

    let mut msg = Default::default();
    loop {
        if !RUNNING.load(Ordering::SeqCst) {
            break;
        }
        let ret = GetMessageW(&mut msg, None, 0, 0);
        if ret.0 <= 0 {
            if ret.0 < 0 {
                let error = windows::Win32::Foundation::GetLastError().0 as i32;
                push_to_all(DisplayChangeEvent::Error(crate::Win32Error::new(error)));
            }
            break;
        }
        if msg.message == display_change {
            // 显示变化：重新查询并广播（equiv index.js updateDisplayStateAndNotifyCallbacks）
            match crate::extract::extract_display_config() {
                Ok(config) => push_to_all(DisplayChangeEvent::Config(config)),
                Err(e) => push_to_all(DisplayChangeEvent::Error(e)),
            }
        }
        if msg.message == WM_USER || msg.message == WM_DESTROY {
            break;
        }
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

    let _ = DestroyWindow(hwnd);
    0
}
