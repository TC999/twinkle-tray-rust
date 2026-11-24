//! 窗口外观：主题令牌、亚克力/Mica、任务栏定位。
//!
//! equiv: origin/src/electron.js 的 tryVibrancy / updateSettingsWindowMaterial
//! + origin/src/css/vars.scss 的主题变量 + panel.js 的 updateMica 定位。

use serde_json::Value;
use crate::{Palette, Theme};
use slint::ComponentHandle;

/// 把解析出的深浅色同步到 Slint 内置 Palette 全局（fluent std-widgets 的配色来源）。
/// 不做这一步时，ComboBox/LineEdit/ScrollView/ToggleBox 等 std-widgets 的深浅色
/// 只跟随 Windows 系统设置，与应用内主题（theme=dark/light/default）脱节。
/// 四窗口同属 all.slint 编译单元 → Palette 全局实例共享，任一窗口设置即全部生效。
pub fn palette_set_dark(palette: &Palette, dark: bool) {
    use slint::private_unstable_api::re_exports::ColorScheme;
    palette.set_color_scheme(if dark { ColorScheme::Dark } else { ColorScheme::Light });
}

/// 把 settings 应用到 Theme 全局（equiv body[data-theme] / [data-is-win11]）。
pub fn apply_theme(panel: &crate::PanelWindow, settings: &Value) {
    let theme_name = settings
        .get("theme")
        .and_then(|v| v.as_str())
        .unwrap_or("default");
    let dark = match theme_name {
        "dark" => true,
        "light" => false,
        // "default" = 跟随系统（等价 determineTheme + nativeTheme.shouldUseDarkColors）
        _ => system_uses_dark_mode(),
    };
    let win11 = settings
        .get("isWin11")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let theme = panel.global::<Theme>();
    theme.set_dark(dark);
    theme.set_win11(win11);
    palette_set_dark(&panel.global::<Palette>(), dark);
    if let Some(accent) = settings.get("accentColor").and_then(|v| v.as_str()) {
        if let Some(color) = parse_color(accent) {
            theme.set_accent(color);
        }
    }
}

/// 主题解析：(dark, win11, accent)。
pub fn resolve_theme(settings: &Value) -> (bool, bool, Option<slint::Color>) {
    let theme_name = settings
        .get("theme")
        .and_then(|v| v.as_str())
        .unwrap_or("default");
    let dark = match theme_name {
        "dark" => true,
        "light" => false,
        _ => system_uses_dark_mode(),
    };
    let win11 = settings
        .get("isWin11")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let accent = settings
        .get("accentColor")
        .and_then(|v| v.as_str())
        .and_then(parse_color);
    (dark, win11, accent)
}

/// Intro 窗口主题（IntroWindow 暴露自己的 dark 属性，无 Theme 全局）。
pub fn apply_theme_generic(intro: &crate::IntroWindow, settings: &Value) {
    let (dark, _, _) = resolve_theme(settings);
    intro.set_dark(dark);
    palette_set_dark(&intro.global::<Palette>(), dark);
}

/// 把 settings 应用到设置窗口的 Theme（equiv body[data-theme]）。
pub fn apply_settings_theme(ui: &crate::SettingsWindow, settings: &Value) {
    let theme_name = settings
        .get("theme")
        .and_then(|v| v.as_str())
        .unwrap_or("default");
    let dark = match theme_name {
        "dark" => true,
        "light" => false,
        _ => system_uses_dark_mode(),
    };
    let win11 = settings
        .get("isWin11")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let theme = ui.global::<Theme>();
    theme.set_dark(dark);
    theme.set_win11(win11);
    palette_set_dark(&ui.global::<Palette>(), dark);
    if let Some(accent) = settings.get("accentColor").and_then(|v| v.as_str()) {
        if let Some(color) = parse_color(accent) {
            theme.set_accent(color);
        }
    }
}

/// 系统深浅色（equiv nativeTheme.shouldUseDarkColors：读注册表 AppsUseLightTheme）。
pub fn system_uses_dark_mode() -> bool {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
        REG_DWORD,
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
            return true; // 读不到时按暗色（原版 lastTheme 缺省为暗）
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
        if !ok {
            return true;
        }
        data == 0
    }
}

/// 系统任务栏深浅色（equiv lastTheme.SystemUsesLightTheme：读注册表
/// SystemUsesLightTheme）。托盘图标目录由此决定（getTrayIconPath L2459）：
/// 系统用浅色任务栏 → light（黑图标），深色任务栏 → dark（白图标）。
/// 注意与 AppsUseLightTheme 是两个不同的值，不可混用。
pub fn system_uses_light_theme() -> bool {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
        REG_DWORD,
    };
    let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
        .encode_utf16()
        .collect();
    let value_name: Vec<u16> = "SystemUsesLightTheme\0".encode_utf16().collect();
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
            return false; // 读不到时按深色任务栏（原版 lastTheme 缺省为暗）
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
        ok && data != 0
    }
}

/// 系统 locale（equiv app.getLocale()）。
/// 主进程启动装载与设置页语言切换共用（equiv getLocalization L2242-L2253 的检测输入）。
pub fn system_locale() -> String {
    use windows::Win32::Globalization::{GetUserDefaultLocaleName, MAX_LOCALE_NAME};
    // SAFETY: 缓冲区为固定长度的可写数组，API 只写入至多 MAX_LOCALE_NAME 个宽字符。
    unsafe {
        let mut buf = [0u16; MAX_LOCALE_NAME as usize];
        let len = GetUserDefaultLocaleName(&mut buf);
        if len > 0 {
            let end = buf.iter().position(|c| *c == 0).unwrap_or(len as usize);
            return String::from_utf16_lossy(&buf[..end]);
        }
    }
    "en-US".to_string()
}

/// "#RRGGBB" / "#AARRGGBB" -> Slint 颜色。
pub fn parse_color(text: &str) -> Option<slint::Color> {
    let hex = text.trim_start_matches('#');
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(match hex.len() {
        6 => slint::Color::from_rgb_u8(
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
        ),
        8 => slint::Color::from_argb_u8(
            (value >> 24) as u8,
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
        ),
        _ => return None,
    })
}

/// 窗口材质（equiv updateSettingsWindowMaterial / tryVibrancy）：
/// Win11 用 Mica，Win10 用亚克力；都关闭时退回纯色背景。
pub fn apply_window_material(hwnd: isize, settings: &Value) {
    let win11 = settings
        .get("isWin11")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let acrylic = settings
        .get("useAcrylic")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let window = windows::Win32::Foundation::HWND(hwnd as *mut core::ffi::c_void);
    if !acrylic {
        // equiv tryVibrancy(false)：禁用并保持透明背景
        let _ = twutils::window_material::set_window_material(window, 0, 0, 0);
        return;
    }
    if win11 {
        // DWMSBT_MAINWINDOW = 2（Mica）
        let _ = twutils::window_material::set_window_material(window, 2, 0, 1);
    } else {
        // equiv L4333-L4340 / L3694-L3701：useNativeAnimation=false 时
        // Win10 亚克力退化为无材质（tryVibrancy(false) 分支）
        let native_animation = settings
            .get("useNativeAnimation")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !native_animation {
            let _ = twutils::window_material::set_window_material(window, 0, 0, 0);
            return;
        }
        // DWMSBT_TRANSIENTWINDOW = 3（亚克力）
        let _ = twutils::window_material::set_window_material(window, 3, 0, 1);
    }
}

/// 窗口句柄的 Win32 原始值（winit → raw-window-handle 0.6）。
///
/// 面板定位需要"物理像素 ↔ 逻辑像素"换算时才拿到 HWND（`GetDpiForWindow`）；
/// 必须在窗口已 show、且处在创建它的线程（主线程）上调用。
pub fn window_hwnd(window: &slint::Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = window.window_handle();
    match handle.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(win32) => Some(win32.hwnd.get()),
        _ => None,
    }
}

/// 窗口自身所在 DPI / 96 的比例（物理像素 ÷ 逻辑像素）。
///
/// 与 `Window::scale_factor()` 的区别：Slint 报告的 scale_factor 是"UI 渲染比例"，
/// 可以来自系统/应用级 DPI 设置；而 winit 的 `set_position` 最终走
/// `SetWindowPos` 并按 `GetDpiForWindow(hwnd)` 做逻辑→物理换算。二者不一致时
/// （窗口被拖到不同 DPI 的显示器、或应用级 DPI 覆盖与系统不同），用 scale_factor
/// 预先换算会让窗口落在别处——这正是"设置窗口实际出现在 (192,192) 而不是 (440,60)"
/// 的成因。HWND 拿不到时退回 scale_factor（等价原行为，不新增失败路径）。
pub fn window_dpi_scale(window: &slint::Window) -> f32 {
    let fallback = window.scale_factor();
    let Some(hwnd) = window_hwnd(window) else {
        return fallback;
    };
    let hwnd = windows::Win32::Foundation::HWND(hwnd as *mut core::ffi::c_void);
    // SAFETY: hwnd 来自本窗口自身的 raw handle，调用线程即窗口创建线程。
    let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) };
    if dpi > 0 {
        dpi as f32 / 96.0
    } else {
        fallback
    }
}

/// 任务栏位置 -> 面板贴边方向（equiv electron.js 的 panel-position / positionPanel）。
/// 返回 (x, y) 相对屏幕的锚点语义由调用方解释。
pub fn taskbar_position_to_panel_edge(position: &str) -> PanelEdge {
    match position {
        "TOP" => PanelEdge::Top,
        "LEFT" => PanelEdge::Left,
        "RIGHT" => PanelEdge::Right,
        _ => PanelEdge::Bottom,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelEdge {
    Top,
    Left,
    Right,
    Bottom,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_parsing() {
        let c = parse_color("#FF8800").unwrap();
        assert_eq!(c, slint::Color::from_rgb_u8(0xFF, 0x88, 0x00));
        let c2 = parse_color("#80FF8800").unwrap();
        assert_eq!(c2, slint::Color::from_argb_u8(0x80, 0xFF, 0x88, 0x00));
        assert!(parse_color("nope").is_none());
    }

    #[test]
    fn taskbar_edge_mapping() {
        assert_eq!(taskbar_position_to_panel_edge("TOP"), PanelEdge::Top);
        assert_eq!(taskbar_position_to_panel_edge("BOTTOM"), PanelEdge::Bottom);
        assert_eq!(taskbar_position_to_panel_edge("unknown"), PanelEdge::Bottom);
    }
}