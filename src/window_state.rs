//! 窗口外观：主题令牌、亚克力/Mica、任务栏定位。
//!
//! equiv: origin/src/electron.js 的 tryVibrancy / updateSettingsWindowMaterial
//! + origin/src/css/vars.scss 的主题变量 + panel.js 的 updateMica 定位。

use serde_json::Value;
use crate::Theme;
use slint::ComponentHandle;

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