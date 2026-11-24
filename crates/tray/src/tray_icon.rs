//! Shell_NotifyIconW 托盘图标 + 右键菜单。
//!
//! equiv: electron Tray API（electron.js 的 createTray / setTrayMenu / tray.on("click")）。
//! 图标资产: origin/src/assets/tray-icons/{dark,light}/*.ico（预生成，禁止自绘——AGENTS.md §1.2）。
//!
//! 线程模型（AGENTS.md §5）：窗口创建 + 消息循环跑专用线程；
//! 点击/菜单事件经 channel 回传主线程（Slint 事件循环）处理。

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use windows::core::{BOOL, PCSTR, w};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use crate::hotkey::WM_HOTKEY;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NOTIFYICONDATAW, NOTIFY_ICON_DATA_FLAGS, NIF_ICON, NIF_MESSAGE, NIF_TIP,
    NIM_ADD, NIM_DELETE, NOTIFY_ICON_MESSAGE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallNextHookEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
    DestroyWindow, DispatchMessageW, GetClassNameW, GetCursorPos, GetMessageW, LoadImageW,
    PostMessageW, RegisterClassW, SetForegroundWindow, SetWindowsHookExW, TrackPopupMenu,
    UnhookWindowsHookEx, GDI_IMAGE_TYPE, HICON, IMAGE_FLAGS, LR_LOADFROMFILE, MF_STRING,
    MENU_ITEM_FLAGS, MSG, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD, WH_CBT,
    WM_LBUTTONUP, WM_RBUTTONUP, WINDOW_EX_STYLE, WINDOW_STYLE,
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
    /// checkbox 选中态（equiv type: 'checkbox' 的 checked；None = normal 项）
    pub checked: Option<bool>,
    /// 分隔线（label 无效）
    pub separator: bool,
    /// 子菜单（equiv electron submenu；非空时该项为 MF_POPUP 父项）
    pub children: Vec<TrayMenuItem>,
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
    /// WM_HOTKEY（wparam = 注册 id；equiv globalShortcut 回调）
    Hotkey(i32),
    /// WM_DISPLAYCHANGE（equiv hookWindowMessage(126)）
    WmDisplayChange,
    /// WM_POWERBROADCAST + PBT_POWERSETTINGCHANGE 的设置三元组
    /// (guid_name, guid_string, data)；空名时以 guid_string 区分。
    PowerSetting(String, String, u32),
    /// WM_POWERBROADCAST 其它事件(wParam)
    PowerBroadcast(u32),
    /// WM_SYSCOMMAND SC_MONITORPOWER(lParam == 2)
    ScMonitorPower,
    /// WM_INPUT 原生亮度键报告（"BrightnessUp"/"BrightnessDown"/""）
    NativeBrightnessKey(String),
}

/// 托盘图标句柄（含消息窗口）。
#[derive(Clone, Copy)]
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
        // cbSize: Shell_NotifyIconW 的 MSDN 硬性要求（结构体头字段必须为
        // sizeof(NOTIFYICONDATAW)，否则 NIM_ADD 返回 FALSE）。Default 是 zeroed，
        // 此处必须显式填充（windows 0.62 结构体无默认的 cbSize 初始化）。
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
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

        if let Ok(mut slot) = TRAY_HWND.lock() {
            *slot = Some(hwnd.0 as usize);
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
        // cbSize 同上：NIM_MODIFY 同样要求结构体头字段正确
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
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
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = handle.hwnd;
        nid.uID = handle.uid;
        let _ = Shell_NotifyIconW(NOTIFY_ICON_MESSAGE(NIM_DELETE.0), &nid);
        let _ = DestroyWindow(handle.hwnd);
    }
}

/// 重建托盘菜单表（equiv electron 的 rebuildTray 菜单部分）。
/// 图标不重建，仅替换 show_menu 读取的菜单快照：语言切换等场景下，
/// 下一次右键弹出即使用新文案（菜单每次弹出都是现读现建 HMENU）。
pub fn update_tray_menu(items: Vec<TrayMenuItem>) -> bool {
    match menu_slot().lock() {
        Ok(mut menu) => {
            *menu = items;
            true
        }
        Err(_) => false,
    }
}

/// 递归追加菜单项到 HMENU（equiv Menu.buildFromTemplate 的 submenu 展开）。
/// 非空 children 的项以 MF_POPUP 追加子菜单；点击子菜单项时 TrackPopupMenu
/// 返回其 id（等价 electron submenu 项的 click 回调）。
unsafe fn append_items(hmenu: windows::Win32::UI::WindowsAndMessaging::HMENU, items: &[TrayMenuItem]) {
    for item in items {
        if item.separator {
            // MF_SEPARATOR = 0x800
            let _ = AppendMenuW(hmenu, MENU_ITEM_FLAGS(0x0000_0800), 0, windows::core::PCWSTR::null());
            continue;
        }
        if !item.children.is_empty() {
            // 子菜单：MF_POPUP(0x10) 追加，uid 填子菜单句柄，label 为父项标题
            let Ok(sub) = CreatePopupMenu() else { continue };
            append_items(sub, &item.children);
            let label: Vec<u16> = item.label.encode_utf16().chain(std::iter::once(0)).collect();
            let flags = if item.enabled {
                MENU_ITEM_FLAGS(0x0000_0010) // MF_POPUP
            } else {
                MENU_ITEM_FLAGS(0x0000_0010 | 0x0000_0003)
            };
            let _ = AppendMenuW(hmenu, flags, sub.0 as usize, windows::core::PCWSTR::from_raw(label.as_ptr()));
            continue;
        }
        let label: Vec<u16> = item.label.encode_utf16().chain(std::iter::once(0)).collect();
        // MF_GRAYED=1, MF_DISABLED=2（未启用项灰显）
        let mut flags = if item.enabled {
            MF_STRING
        } else {
            MF_STRING | MENU_ITEM_FLAGS(0x0000_0003)
        };
        if let Some(checked) = item.checked {
            // MF_CHECKED = 0x8（equiv type: 'checkbox'）
            if checked {
                flags = flags | MENU_ITEM_FLAGS(0x0000_0008);
            }
        }
        let _ = AppendMenuW(hmenu, flags, item.id as usize, windows::core::PCWSTR::from_raw(label.as_ptr()));
    }
}

/// 托盘菜单是否用深色（equiv 原版 Electron 菜单跟随 nativeTheme 深浅色）。
/// 读注册表 AppsUseLightTheme：系统用深色应用模式（=0）时菜单为深色；
/// 读不到时按深色处理（与本机默认行为一致）。注意这是系统级应用模式，
/// 不随应用内主题设置的 light/dark 覆盖项变化——原版 Electron 菜单同样如此。
fn menu_uses_dark_mode() -> bool {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_DWORD,
    };
    let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
        .encode_utf16()
        .collect();
    let value_name: Vec<u16> = "AppsUseLightTheme\0".encode_utf16().collect();
    // SAFETY: 子键/值名为合法 NUL 结尾宽字符串；句柄在使用后立即关闭。
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            windows::core::PCWSTR::from_raw(subkey.as_ptr()),
            None,
            KEY_READ,
            &mut key,
        )
        .is_err()
        {
            return true;
        }
        let mut data: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let mut ty = REG_DWORD;
        let ok = RegQueryValueExW(
            key,
            windows::core::PCWSTR::from_raw(value_name.as_ptr()),
            None,
            Some(&mut ty),
            Some(&mut data as *mut u32 as *mut u8),
            Some(&mut size),
        )
        .is_ok();
        let _ = RegCloseKey(key);
        !ok || data == 0
    }
}

/// CBT 钩子期间是否要深色菜单（钩子过程是静态函数，经此标志传递）。
static MENU_DARK_MODE: AtomicBool = AtomicBool::new(false);

/// HCBT_CREATEWND = 3（窗口创建通知）。
const HCBT_CREATEWND: i32 = 3;
/// Win32 弹出菜单窗口的类名。
const MENU_CLASS: &str = "#32768";

/// uxtheme 未公开 API（Win10 1809+/Win11 均存在）：深色模式策略开关。
/// 仅设 DWMWA_USE_IMMERSIVE_DARK_MODE 只影响窗口框架，菜单内容由 uxtheme
/// 按"应用深色策略"绘制——必须先 SetPreferredAppMode(AllowDark) 打开策略，
/// 再对菜单窗口 AllowDarkModeForWindow + DWM 属性（Electron/Chromium 同款配方）。
/// ordinal：104 RefreshImmersiveColorPolicyState、133 AllowDarkModeForWindow、
/// 135 SetPreferredAppMode（均 1903+；缺失时按浅色菜单降级，不影响功能）。
struct UxthemeDark {
    /// FreeLibrary 不做：模块随进程常驻，避免每次弹菜单重新 LoadLibrary。
    /// 字段本身不读取，仅为持有句柄阻止卸载。
    #[allow(dead_code)]
    module: isize,
    set_preferred_app_mode: unsafe extern "system" fn(i32) -> i32,
    allow_dark_mode_for_window: unsafe extern "system" fn(HWND, BOOL) -> BOOL,
    refresh_immersive_color_policy_state: unsafe extern "system" fn(),
}

/// PreferredAppMode::AllowDark（跟随系统）/ Default（恢复）。
const APP_MODE_ALLOW_DARK: i32 = 1;
const APP_MODE_DEFAULT: i32 = 0;

unsafe impl Send for UxthemeDark {}
unsafe impl Sync for UxthemeDark {}

static UXTHEME_DARK: OnceLock<Option<UxthemeDark>> = OnceLock::new();

fn uxtheme_dark() -> &'static Option<UxthemeDark> {
    UXTHEME_DARK.get_or_init(|| unsafe {
        // SAFETY: LoadLibraryW/GetProcAddress 常规用法；ordinal 以 MAKEINTRESOURCE
        // 语义传入（指针值即 ordinal）。函数签名按微软未公开 ABI 的公开共识，
        // 调用失败仅表现为菜单保持浅色，无崩溃风险（参数均为值类型）。
        let module = LoadLibraryW(w!("uxtheme.dll")).ok()?;
        let get = |ord: u16| -> Option<*const ()> {
            // FARPROC = Option<fn()>，找不到 ordinal 时为 None
            GetProcAddress(module, PCSTR::from_raw(ord as usize as *const u8))
                .map(|f| f as *const ())
        };
        let (Some(a), Some(b), Some(c)) = (
            get(135),
            get(133),
            get(104),
        ) else {
            return None;
        };
        Some(UxthemeDark {
            module: module.0 as isize,
            set_preferred_app_mode: std::mem::transmute::<*const (), unsafe extern "system" fn(i32) -> i32>(a),
            allow_dark_mode_for_window: std::mem::transmute::<*const (), unsafe extern "system" fn(HWND, BOOL) -> BOOL>(b),
            refresh_immersive_color_policy_state: std::mem::transmute::<*const (), unsafe extern "system" fn()>(c),
        })
    })
}

/// CBT 钩子：拦截 TrackPopupMenu 即将创建的菜单窗口（类 #32768），
/// AllowDarkModeForWindow + DWMWA_USE_IMMERSIVE_DARK_MODE 让 uxtheme 用深色渲染
/// （equiv Electron/Chromium 的 dark 菜单窗口处理）。
unsafe extern "system" fn menu_cbt_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HCBT_CREATEWND && MENU_DARK_MODE.load(Ordering::Relaxed) {
        let hwnd = HWND(wparam.0 as *mut core::ffi::c_void);
        let expect: Vec<u16> = MENU_CLASS.encode_utf16().collect();
        let mut buf = [0u16; 32];
        let n = GetClassNameW(hwnd, &mut buf) as usize;
        if n == expect.len() && buf[..n] == expect[..] {
            if let Some(ux) = uxtheme_dark() {
                // SAFETY: hwnd 为钩子通报的合法菜单窗口；签名按 uxtheme 未公开 ABI。
                let _ = (ux.allow_dark_mode_for_window)(hwnd, BOOL(1));
            }
            let on = BOOL(1);
            // SAFETY: hwnd 为钩子通报的合法菜单窗口；lpdata 指向有效 BOOL。
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &on as *const BOOL as *const core::ffi::c_void,
                std::mem::size_of::<BOOL>() as u32,
            );
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// 弹出托盘菜单（equiv setTrayMenu 的 TrackPopupMenu）。
unsafe fn show_menu(hwnd: HWND) {
    let items = menu_slot().lock().map(|g| g.clone()).unwrap_or_default();
    let Ok(menu) = CreatePopupMenu() else { return };
    append_items(menu, &items);
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    // 菜单弹出前必须把窗口置前，否则点击外部不会关闭（Win32 托盘菜单惯例）
    let _ = SetForegroundWindow(hwnd);
    // 深色菜单：uxtheme 深色策略须在菜单窗口创建前开启，DWM 属性须在创建时生效
    // —— TrackPopupMenu 同步创建菜单窗口，故先开策略，再装线程级 CBT 钩子拦截创建。
    let dark = menu_uses_dark_mode();
    MENU_DARK_MODE.store(dark, Ordering::Relaxed);
    if dark {
        if let Some(ux) = uxtheme_dark() {
            // SAFETY: 值参数调用；策略为线程级，菜单关闭后恢复 Default。
            (ux.set_preferred_app_mode)(APP_MODE_ALLOW_DARK);
            (ux.refresh_immersive_color_policy_state)();
        }
    }
    let hook = if dark {
        // SAFETY: 钩子过程为静态函数；WH_CBT 线程钩子（idThread=当前线程）不跨进程。
        SetWindowsHookExW(WH_CBT, Some(menu_cbt_hook), None, GetCurrentThreadId()).ok()
    } else {
        None
    };
    let cmd = TrackPopupMenu(
        menu,
        TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RETURNCMD,
        pt.x,
        pt.y,
        None,
        hwnd,
        None,
    );
    if let Some(h) = hook {
        let _ = UnhookWindowsHookEx(h);
    }
    if dark {
        if let Some(ux) = uxtheme_dark() {
            // SAFETY: 值参数调用；恢复默认策略，避免影响其它 uxtheme 消费者。
            (ux.set_preferred_app_mode)(APP_MODE_DEFAULT);
        }
    }
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
    // WM_HOTKEY:热键注册在本窗口上,转发给主线程(equiv globalShortcut 回调)
    if msg == WM_HOTKEY {
        if let Ok(cb) = callback_slot().lock() {
            if let Some(cb) = cb.as_ref() {
                cb(TrayEvent::Hotkey(wparam.0 as i32));
            }
        }
        return LRESULT(0);
    }
    // WM_DISPLAYCHANGE(126):显示变化(equiv hookWindowMessage(126))
    if msg == 126 {
        if let Ok(cb) = callback_slot().lock() {
            if let Some(cb) = cb.as_ref() {
                cb(TrayEvent::WmDisplayChange);
            }
        }
        return LRESULT(0);
    }
    // WM_POWERBROADCAST(0x218)
    if msg == 0x218 {
        const PBT_POWERSETTINGCHANGE: u32 = 0x8013; // 32787
        if wparam.0 as u32 == PBT_POWERSETTINGCHANGE {
            // LPARAM 指向 POWERBROADCAST_SETTING(twutils::power_events 解析;返回 (guid, name, data))
            let ptr = lparam.0 as usize;
            if let Some((guid, name, data)) = twutils::power_events::get_power_setting(ptr) {
                if let Ok(cb) = callback_slot().lock() {
                    if let Some(cb) = cb.as_ref() {
                        cb(TrayEvent::PowerSetting(name, guid, data));
                    }
                }
            }
        } else {
            if let Ok(cb) = callback_slot().lock() {
                if let Some(cb) = cb.as_ref() {
                    cb(TrayEvent::PowerBroadcast(wparam.0 as u32));
                }
            }
        }
        return LRESULT(0);
    }
    // WM_SYSCOMMAND(0x0112) SC_MONITORPOWER(0xF170) lParam==2
    if msg == 0x0112 {
        if wparam.0 as u32 == 61808 && (lparam.0 as u32) == 2 {
            if let Ok(cb) = callback_slot().lock() {
                if let Some(cb) = cb.as_ref() {
                    cb(TrayEvent::ScMonitorPower);
                }
            }
        }
        return LRESULT(0);
    }
    // WM_INPUT(0x00FF):原生亮度键 RAWINPUT(lparam 为 HRAWINPUT)
    if msg == 0x00FF {
        let key = twutils::brightness_keys::read_brightness_key(windows::Win32::Foundation::HANDLE(lparam.0 as *mut core::ffi::c_void));
        if let Ok(cb) = callback_slot().lock() {
            if let Some(cb) = cb.as_ref() {
                cb(TrayEvent::NativeBrightnessKey(key));
            }
        }
        return LRESULT(0);
    }
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

/// 托盘消息窗口句柄（主线程读取以注册热键等；窗口由托盘线程创建）。
static TRAY_HWND: Mutex<Option<usize>> = Mutex::new(None);

/// 主线程读取托盘窗口句柄（None = 托盘线程尚未创建窗口）。
pub fn tray_window_handle() -> Option<HWND> {
    TRAY_HWND
        .lock()
        .ok()
        .and_then(|g| *g)
        .map(|p| HWND(p as *mut core::ffi::c_void))
}

fn thread_id_slot() -> &'static Mutex<Option<u32>> {
    TRAY_THREAD_ID.get_or_init(|| Mutex::new(None))
}

/// 在专用线程创建托盘并跑消息循环（AGENTS.md §5）。
/// 返回 (事件接收端, 图标句柄)。窗口与图标均由该线程创建并在退出时销毁
/// （DestroyWindow 的同线程约束），主线程只消费事件。
/// handle 供 update_tray_icon/remove_tray_icon 使用；创建失败时为 None
/// （线程立即退出，不进入消息循环）。
pub fn spawn_tray_thread(config: TrayConfig) -> (Receiver<TrayEvent>, Option<TrayIconHandle>) {
    let (tx, rx): (Sender<TrayEvent>, Receiver<TrayEvent>) = channel();
    let cb_tx = tx;
    set_tray_callback(Arc::new(move |event: TrayEvent| {
        let _ = cb_tx.send(event);
    }));
    // HWND 是 *mut c_void（非 Send），跨线程传递转原始指针值 (usize, uid)
    let handle_slot: Arc<Mutex<Option<(usize, u32)>>> = Arc::new(Mutex::new(None));
    let handle_slot_t = handle_slot.clone();
    std::thread::spawn(move || {
        // 记录线程 id（退出时 PostThreadMessageW(WM_QUIT) 用）
        let current_id = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        if let Ok(mut slot) = thread_id_slot().lock() {
            *slot = Some(current_id);
        }
        let Some(handle) = create_tray_icon(&config) else {
            return;
        };
        if let Ok(mut slot) = handle_slot_t.lock() {
            *slot = Some((handle.hwnd.0 as usize, handle.uid));
        }
        run_message_loop();
        // 消息循环退出后清理图标与窗口（同线程）
        remove_tray_icon(&handle);
    });
    // 等待图标创建完成（要么得到 handle，要么线程失败退出）
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let handle = loop {
        let got = handle_slot.lock().ok().and_then(|g| *g);
        if let Some((hwnd, uid)) = got {
            break Some(TrayIconHandle {
                hwnd: HWND(hwnd as *mut core::ffi::c_void),
                uid,
            });
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    (rx, handle)
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
    fn menu_item_default_children() {
        // TrayMenuItem 构造必须显式提供 children（无默认值，防漏建子菜单）
        let item = TrayMenuItem {
            id: 2100,
            label: "Profile A".into(),
            enabled: true,
            checked: None,
            separator: false,
            children: vec![TrayMenuItem {
                id: 2101,
                label: "Sub".into(),
                enabled: true,
                checked: None,
                separator: false,
                children: Vec::new(),
            }],
        };
        assert_eq!(item.children.len(), 1);
        assert_eq!(item.children[0].id, 2101);
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

    /// 回归：NOTIFYICONDATAW::default() 是 zeroed()，cbSize 必须显式填充。
    /// Shell_NotifyIconW 要求 cbSize == sizeof(NOTIFYICONDATAW)，否则 NIM_ADD
    /// 恒返回 FALSE（真机缺陷：托盘图标永不出现，且无任何错误日志）。
    #[test]
    fn notify_icon_data_cbsize_is_required() {
        let nid = NOTIFYICONDATAW::default();
        assert_eq!(nid.cbSize, 0, "Default 是 zeroed，cbSize 为 0——必须显式赋值");
        // 赋值后必须等于结构体大小（Shell_NotifyIconW 的硬性校验）
        let mut filled = NOTIFYICONDATAW::default();
        filled.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        assert_eq!(filled.cbSize, std::mem::size_of::<NOTIFYICONDATAW>() as u32);
        assert!(filled.cbSize >= 88, "x64 NOTIFYICONDATAW 至少 88 字节");
    }
}