
// ---------------------------------------------------------------------------
// 无边框窗口「拖动/缩放」期间的系统模态循环补充泵（见 PROGRESS Round 16）。
//
// 背景（真机实测）：WM_NCLBUTTONDOWN(HTCAPTION) / WM_SYSCOMMAND(SC_SIZE) 之后立刻进入
// 系统的尺寸调整模态循环。Slint(winit) 的光标消息泵在该循环期间拿不到「有新消息」，
// 应用自家 UI 定时器（50ms tick）与渲染停摆：拖动时窗口只跳一下就停、松手后位置还要
// 再飘一次（迟到的 SetWindowPos 才真正生效）。
//
// 处理方式：在 winit 的 EventLoopBuilder 上装消息钩子（Slint 后端已开放该入口）；
// 模态循环（WM_ENTERSIZEMOVE..WM_EXITSIZEMOVE，覆盖移动/缩放两种循环）期间用
// PostMessageW 把消息转投给 winit 的内部线程事件窗口（窗口过程即 winit 的 WndProc），
// 于是拖动全程定时器与渲染继续跑，且投递顺序保证 WM_EXITSIZEMOVE 处理时位置已设置好。

#[cfg(target_os = "win32")]
pub mod modal_pump {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicIsize, Ordering};

    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumThreadWindows, GetClassNameW, GetCurrentThreadId, PostMessageW, MSG,
    };

    /// 非 0 表示系统模态循环正在进行。
    static IN_MODAL_LOOP: AtomicIsize = AtomicIsize::new(0);
    /// winit 内部线程事件窗口（进入模态循环时按需枚举一次）。
    static THREAD_TARGET: AtomicIsize = AtomicIsize::new(0);
    /// 防止「钩子转投 -> winit 派发 -> 钩子再看见」的无限递归。
    thread_local! {
        static FORWARDING: Cell<bool> = const { Cell::new(false) };
    }

    /// winit 的事件循环消息钩子（在每个 PeekMessageW 之后、Dispatch 之前调用）。
    /// 返回 true 表示已代 winit 处理该消息。
    pub fn msg_hook(raw: *const core::ffi::c_void) -> bool {
        if raw.is_null() {
            return false;
        }
        // SAFETY: winit 只在 PeekMessageW 成功之后调用钩子，传入的是有效 MSG 指针。
        let msg: MSG = unsafe { *(raw as *const MSG) };
        match msg.message {
            0x0231 => IN_MODAL_LOOP.store(1, Ordering::Release),
            0x0232 => IN_MODAL_LOOP.store(0, Ordering::Release),
            _ => {}
        }
        if IN_MODAL_LOOP.load(Ordering::Acquire) == 0 {
            return false;
        }
        FORWARDING.with(|f| {
            if f.get() {
                return false;
            }
            f.set(true);
            let ok = match thread_target() {
                // SAFETY: 只是把同一条消息投递给同线程的另一窗口；PostMessageW 不要求
                // 调用方拥有目标窗口，参数原样转交（winit 的 WndProc 会按句柄重新定位）。
                Some(target) => unsafe {
                    PostMessageW(Some(target), msg.message, msg.wParam, msg.lParam).is_ok()
                },
                None => false,
            };
            f.set(false);
            ok
        })
    }

    /// 枚举本线程窗口找 winit 的事件目标窗口（类名 "Winit Thread Event Target"）。
    fn thread_target() -> Option<HWND> {
        let cached = THREAD_TARGET.load(Ordering::Acquire);
        if cached != 0 {
            return Some(HWND(cached as *mut core::ffi::c_void));
        }
        let mut found: HWND = HWND::default();
        let ctx = &mut found as *mut HWND as isize;
        // SAFETY: 回调只读枚举到的句柄，命中时写入 ctx 指向的栈上变量，其生命周期覆盖枚举。
        let _ = unsafe { EnumThreadWindows(GetCurrentThreadId(), Some(collect), LPARAM(ctx)) };
        if found.is_invalid() {
            return None;
        }
        THREAD_TARGET.store(found.0 as isize, Ordering::Release);
        Some(found)
    }

    /// 回调：类名匹配 winit 事件目标窗口时记录句柄并停止枚举。
    ///
    /// SAFETY: lparam 必须是 thread_target 传入的 *mut HWND。
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> windows::Win32::Foundation::BOOL {
        let mut buf = [0u16; 64];
        // SAFETY: buf 为合法可写缓冲区，长度按元素数传入。
        let len = unsafe { GetClassNameW(hwnd, &mut buf) };
        let class = String::from_utf16_lossy(&buf[..len.max(0) as usize]);
        if class.starts_with("Winit Thread Event Target") {
            // SAFETY: lparam 由 thread_target 以 *mut HWND 传入，生命周期覆盖本次枚举。
            unsafe { *(lparam.0 as *mut HWND) = hwnd };
            return windows::Win32::Foundation::BOOL(0);
        }
        windows::Win32::Foundation::BOOL(1)
    }
}
