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
    /// cycle 动作的取值表（equiv action.values）
    pub values: Vec<i64>,
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
                                    values: action
                                        .get("values")
                                        .and_then(|v| v.as_array())
                                        .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
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

/// 热键注册表(等价 applyHotkeys 的注册结果):accelerator -> id,供 WM_HOTKEY 反查。
#[derive(Debug, Clone)]
pub struct RegisteredHotkey {
    pub accelerator: String,
    pub id: String,
    pub actions: Vec<HotkeyAction>,
}

/// 解析 settings.hotkeys 为带 id 的注册表(equiv applyHotkeys 遍历段)。
pub fn parse_hotkeys_with_id(settings: &Value) -> Vec<RegisteredHotkey> {
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
                    let id = h.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let actions: Vec<HotkeyAction> = h
                        .get("actions")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .map(|action| HotkeyAction {
                                    target: action.get("target").and_then(|v| v.as_str()).unwrap_or("brightness").to_string(),
                                    action_type: action.get("type").and_then(|v| v.as_str()).unwrap_or("offset").to_string(),
                                    value: action.get("value").and_then(|v| v.as_i64()).unwrap_or(0),
                                    all_monitors: action.get("allMonitors").and_then(|v| v.as_bool()).unwrap_or(false),
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
                                    values: action.get("values").and_then(|v| v.as_array()).map(|arr| {
                                        arr.iter().filter_map(|v| v.as_i64()).collect()
                                    }).unwrap_or_default(),
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(RegisteredHotkey { accelerator: accelerator.to_string(), id, actions })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// doHotkey 的执行上下文(equiv doHotkey 的模块级依赖,由 system_events 注入)。
pub struct HotkeyContext {
    pub rt: std::sync::Arc<crate::runtime_hosts::RuntimeHost>,
    /// 共享节流器('static:OnceLock 单例,等价原版 updateBrightnessThrottle 的模块级单例)
    pub throttle: &'static std::sync::Mutex<crate::panel_controller::BrightnessThrottle>,
}

/// doHotkey(equiv L1837-L2014):执行一个热键的全部动作。
/// hotkey_id 为 settings.hotkeys[].id;options.skip_internal_brightness_offset
/// 等价原版 options.skipInternalBrightnessOffset。
pub fn do_hotkey(ctx: &HotkeyContext, hotkey: &RegisteredHotkey, skip_internal_brightness_offset: bool) -> bool {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let rt = &ctx.rt;
    // doingHotkey 互斥 + 100ms 节流(equiv L1839)
    if rt.doing_hotkey.load(std::sync::atomic::Ordering::SeqCst) {
        return false;
    }
    {
        let mut throttle_map = rt.hotkey_throttle.lock().unwrap();
        if !hotkey.id.is_empty() {
            if let Some(last) = throttle_map.get(&hotkey.id) {
                if now_ms <= *last + 100 {
                    return false;
                }
            }
            throttle_map.insert(hotkey.id.clone(), now_ms);
        }
    }

    if hotkey.actions.is_empty() {
        return false;
    }
    rt.doing_hotkey.store(true, std::sync::atomic::Ordering::SeqCst);
    rt.set_recently_interacted(true);

    let settings = rt.settings_snapshot();
    let mut show_overlay = false;
    let mut has_checked_first_cycle_action = false;

    for action in &hotkey.actions {
        // Wait for refresh if user hasnt done so recently(equiv L1856-L1858)。
        // Rust 侧 refreshMonitors 由 worker 事件驱动;此处直接请求一次刷新
        // (原版 await refreshMonitors() 的等价:发起请求即可,worker 完成后回推)。
        if action.action_type != "refresh" {
            let _ = rt.host.refresh_monitors(false, false);
        }

        match action.action_type.as_str() {
            "off" => {
                show_overlay = false;
                // sleepDisplays(settings.sleepAction, 500)(equiv L1862)
                crate::system_events::sleep_displays_runtime(rt, &settings, 500);
            }
            "refresh" => {
                show_overlay = false;
                rt.host.refresh_monitors(true, true);
            }
            "set" | "offset" | "cycle" => {
                // Build list of all applicable monitors(equiv L1868-L1962)
                let monitors = rt.host.monitors_snapshot();
                let mut hotkey_monitors: Vec<(Value, i64)> = Vec::new();
                if let Some(obj) = monitors.as_object() {
                    for (_k, monitor) in obj {
                        let mut applicable = false;
                        let linked = settings.get("linkedLevelsActive").and_then(|v| v.as_bool()).unwrap_or(false);
                        let break_linked = settings.get("hotkeysBreakLinkedLevels").and_then(|v| v.as_bool()).unwrap_or(true);
                        if action.all_monitors || (linked && !break_linked) {
                            applicable = true;
                        } else if !action.monitors.is_empty() {
                            let mid = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            if action.monitors.iter().any(|m| m == mid) {
                                applicable = true;
                            }
                        }

                        // skipInternalBrightnessOffset(equiv L1883-L1885)
                        if skip_internal_brightness_offset
                            && action.action_type == "offset"
                            && action.target == "brightness"
                            && monitor.get("type").and_then(|v| v.as_str()) == Some("wmi")
                        {
                            applicable = false;
                        }

                        if applicable {
                            let mut new_value: i64 = 0;
                            if action.action_type == "offset" {
                                let current_value = current_target_value(rt, monitor, &action.target);
                                new_value = current_value + action.value;
                            } else if action.action_type == "cycle" {
                                // Nothing to cycle through(equiv L1911)
                                if action.values.is_empty() {
                                    continue;
                                }
                                // hotkeyCycleIndexes 管理(equiv L1912-L1949)
                                let idx = {
                                    let map = rt.hotkey_cycle_indexes.lock().unwrap();
                                    let idx = *map.get(&hotkey.id).unwrap_or(&0);
                                    idx
                                };
                                let current_cycle_value = current_target_value(rt, monitor, &action.target);
                                let mut next_idx = idx;
                                if !has_checked_first_cycle_action {
                                    has_checked_first_cycle_action = true;
                                    // if (true || ...) 分支恒真,逐字保留(equiv L1936)
                                    if next_idx >= action.values.len() - 1 {
                                        next_idx = 0;
                                    } else {
                                        next_idx += 1;
                                    }
                                }
                                {
                                    let mut map = rt.hotkey_cycle_indexes.lock().unwrap();
                                    map.insert(hotkey.id.clone(), next_idx);
                                }
                                new_value = action.values[next_idx.min(action.values.len() - 1)];
                                let _ = current_cycle_value; // 读取保留(等价原版的读数副作用)
                            } else if action.action_type == "set" {
                                new_value = action.value;
                            }
                            hotkey_monitors.push((monitor.clone(), new_value));
                        }
                    }
                }

                // Apply change(equiv L1965-L1996)
                for (monitor, value) in &hotkey_monitors {
                    let mid = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if mid.is_empty() {
                        continue;
                    }
                    if action.target == "brightness" {
                        let normalized_adjust = crate::runtime_hosts::min_max(*value as f64);
                        // monitors[key].brightness = normalizedAdjust(equiv L1970)
                        let mut snapshot = rt.host.monitors_snapshot();
                        if let Some(obj) = snapshot.as_object_mut() {
                            for (_k, m) in obj.iter_mut() {
                                if m.get("id").and_then(|v| v.as_str()) == Some(&mid) {
                                    m["brightness"] = serde_json::json!(normalized_adjust);
                                }
                            }
                        }
                        *rt.host.monitors.lock().unwrap() = snapshot;
                        // updateBrightnessThrottle(monitor.id, ..., true, false)
                        if let Ok(mut t) = ctx.throttle.lock() {
                            crate::runtime_hosts::throttled_update(&mut t, rt, &mid, normalized_adjust, "brightness", false);
                        }
                        rt.pause_monitor_updates();

                        // Break linked levels(equiv L1976-L1979)
                        let linked = settings.get("linkedLevelsActive").and_then(|v| v.as_bool()).unwrap_or(false);
                        let break_linked = settings.get("hotkeysBreakLinkedLevels").and_then(|v| v.as_bool()).unwrap_or(true);
                        if break_linked && linked {
                            println!("Breaking linked levels due to hotkey.");
                            rt.update_settings(serde_json::json!({ "linkedLevelsActive": false }));
                        }
                        show_overlay = true;
                    } else if action.target == "sdr" {
                        if let Ok(mut t) = ctx.throttle.lock() {
                            crate::runtime_hosts::throttled_update(&mut t, rt, &mid, *value as f64, "sdr", false);
                        }
                    } else {
                        let vcp_code = match action.target.as_str() {
                            "contrast" => "0x12",
                            "volume" => "0x62",
                            "powerState" => "0xD2",
                            other => other,
                        };
                        if let Ok(mut t) = ctx.throttle.lock() {
                            crate::runtime_hosts::throttled_update(&mut t, rt, &mid, *value as f64, vcp_code, false);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Show brightness overlay, if applicable(equiv L2003-L2005)
    if show_overlay {
        crate::system_events::request_hud_show(rt);
    }

    rt.doing_hotkey.store(false, std::sync::atomic::Ordering::SeqCst);
    true
}

/// 动作目标的当前值(equiv L1892-L1906 的 currentValue 分支)。
fn current_target_value(rt: &std::sync::Arc<crate::runtime_hosts::RuntimeHost>, monitor: &serde_json::Value, target: &str) -> i64 {
    match target {
        "brightness" => monitor.get("brightness").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64,
        "sdr" => monitor.get("sdrLevel").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64,
        "contrast" => vcp_value(rt, monitor, 0x12),
        "volume" => vcp_value(rt, monitor, 0x62),
        "powerState" => vcp_value(rt, monitor, 0xD6),
        other => {
            // Get VCP(equiv L1905):按十进制 target 解析为码
            let code = other.parse::<i64>().unwrap_or(0);
            if code <= 0 {
                return 0;
            }
            vcp_value(rt, monitor, code as u8)
        }
    }
}

/// getVCP(equiv L551-L581):hwid 串 + code,3s 超时,-1 表示不可用。
fn vcp_value(rt: &std::sync::Arc<crate::runtime_hosts::RuntimeHost>, monitor: &serde_json::Value, code: u8) -> i64 {
    let hwid = monitor
        .get("hwid")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("#")
        })
        .unwrap_or_default();
    if hwid.is_empty() {
        return -1;
    }
    match rt.host.get_vcp(&hwid, code, 3000) {
        Some(Value::Array(arr)) => arr
            .first()
            .and_then(|v| v.as_f64())
            .map(|f| f as i64)
            .unwrap_or(-1),
        Some(v) => v.as_f64().map(|f| f as i64).unwrap_or(-1),
        None => -1,
    }
}
