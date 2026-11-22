//! 全局热键宿主（equiv electron globalShortcut + applyHotkeys）。
//!
//! RegisterHotKeyW 在专用线程的消息循环上注册（AGENTS.md §5），
//! WM_HOTKEY 经 channel 投递给主线程消费。

use serde_json::Value;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

/// 热键动作（equiv settings.hotkeys[].actions[]）。
#[derive(Debug, Clone, PartialEq)]
pub struct HotkeyAction {
    pub target: String,
    pub action_type: String,
    pub value: i64,
    pub all_monitors: bool,
    pub monitors: Vec<String>,
}

/// 解析 settings.hotkeys（v1.16.0 格式）。
pub fn parse_hotkeys(settings: &Value) -> Vec<(String, Vec<HotkeyAction>)> {
    settings
        .get("hotkeys")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|h| {
                    let accelerator = h.get("accelerator").and_then(|v| v.as_str())?;
                    if accelerator.is_empty() {
                        return None;
                    }
                    let actions = h
                        .get("actions")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .map(|action| HotkeyAction {
                                    target: action
                                        .get("target")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("brightness")
                                        .to_string(),
                                    action_type: action
                                        .get("type")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("offset")
                                        .to_string(),
                                    value: action.get("value").and_then(|v| v.as_i64()).unwrap_or(0),
                                    all_monitors: action
                                        .get("allMonitors")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false),
                                    monitors: action
                                        .get("monitors")
                                        .and_then(|v| v.as_object())
                                        .map(|o| {
                                            o.iter()
                                                .filter(|(_, v)| v.as_bool().unwrap_or(false))
                                                .map(|(k, _)| k.clone())
                                                .collect()
                                        })
                                        .unwrap_or_default(),
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    Some((accelerator.to_string(), actions))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Electron accelerator（"Control+Shift+Up"）-> (modifiers, vk)。
/// equiv cleanUpKeyboardKeys 的键名表（SettingsWindow.jsx L55-L101）。
pub fn accelerator_to_hotkey(accel: &str) -> Option<(u32, u32)> {
    let mut modifiers = 0u32;
    let mut vk = None;
    for part in accel.split('+') {
        let lower = part.trim().to_ascii_lowercase();
        match lower.as_str() {
            "control" | "ctrl" | "commandorcontrol" => modifiers |= 0x0002, // MOD_CONTROL
            "shift" => modifiers |= 0x0004,                                // MOD_SHIFT
            "alt" => modifiers |= 0x0001,                                  // MOD_ALT
            "super" | "meta" | "windows" => modifiers |= 0x0008,           // MOD_WIN
            "" => {}
            other => vk = key_to_vk(other),
        }
    }
    vk.map(|vk| (modifiers | 0x4000, vk)) // MOD_NOREPEAT
}

/// 键名 -> 虚拟键码（equiv cleanUpKeyboardKeys 的映射，覆盖常用键）。
fn key_to_vk(key: &str) -> Option<u32> {
    let vk = match key {
        "up" => 0x26,
        "down" => 0x28,
        "left" => 0x25,
        "right" => 0x27,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "home" => 0x24,
        "end" => 0x23,
        "insert" => 0x2D,
        "delete" => 0x2E,
        "space" => 0x20,
        "enter" | "return" => 0x0D,
        "tab" => 0x09,
        "escape" | "esc" => 0x1B,
        "backspace" => 0x08,
        "pause" => 0x13,
        "printscreen" => 0x2C,
        "num0" => 0x60,
        "num1" => 0x61,
        "num2" => 0x62,
        "num3" => 0x63,
        "num4" => 0x64,
        "num5" => 0x65,
        "num6" => 0x66,
        "num7" => 0x67,
        "num8" => 0x68,
        "num9" => 0x69,
        "numdec" => 0x6E,
        "numadd" => 0x6B,
        "numsub" => 0x6D,
        "nummult" => 0x6A,
        "numdiv" => 0x6F,
        _ => {
            let bytes = key.as_bytes();
            if bytes.len() == 1 {
                let c = bytes[0].to_ascii_uppercase();
                if c.is_ascii_alphanumeric() {
                    return Some(c as u32);
                }
            }
            if let Some(rest) = key.strip_prefix('f') {
                if let Ok(n) = rest.parse::<u32>() {
                    if (1..=24).contains(&n) {
                        return Some(0x70 + n - 1);
                    }
                }
            }
            return None;
        }
    };
    Some(vk)
}

/// 热键接收器（主线程消费 WM_HOTKEY）。
pub struct HotkeyHost {
    pub rx: Receiver<i32>,
    _tx: std::sync::mpsc::Sender<i32>,
    pub registrations: Arc<Mutex<Vec<(i32, String)>>>,
}

impl HotkeyHost {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            rx,
            _tx: tx,
            registrations: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Default for HotkeyHost {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accelerator_parsing() {
        assert_eq!(accelerator_to_hotkey("Control+Shift+Up"), Some((0x4002 | 0x0004, 0x26)));
        assert_eq!(accelerator_to_hotkey("Alt+F5"), Some((0x4001, 0x74)));
        assert_eq!(accelerator_to_hotkey("Control+0"), Some((0x4002, 0x30)));
        assert_eq!(accelerator_to_hotkey(""), None);
    }

    #[test]
    fn hotkeys_parsed_from_settings() {
        let settings = json!({
            "hotkeys": [
                {"accelerator": "Control+Up", "actions": [
                    {"target": "brightness", "type": "offset", "value": 10, "allMonitors": true}
                ]},
                {"accelerator": "", "actions": []}
            ]
        });
        let parsed = parse_hotkeys(&settings);
        // 空 accelerator 的条目被跳过（原版行为）
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "Control+Up");
        assert_eq!(parsed[0].1[0].value, 10);
        assert!(parsed[0].1[0].all_monitors);
    }
}
