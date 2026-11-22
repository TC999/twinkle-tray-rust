//! Apple Studio Display 发现与合并。
//!
//! equiv: Monitors.js:L1085-L1211（isStudioDisplayAlias / getStudioDisplayAliasKey /
//! getStudioDisplay）。
//!
//! 原版优先用 `usb` 原生模块枚举；不可用时退回 WinUSB 直调（本 crate 的 studio_display 模块）。
//! 别名键逻辑（同序列号复用既有键、APPAE* 前缀视为别名）逐字保留。

use serde_json::{json, Value};
use std::collections::HashMap;

use crate::monitors::update_display;
use crate::wmi_fallback::APPLE_STUDIO_UNAVAILABLE;

/// isStudioDisplayAlias 等价（equiv L1085-L1087）：
/// hwid[1] 以 APPAE 开头，或去掉空白后名字含 studiodisplay。
pub fn is_studio_display_alias(monitor: &Value) -> bool {
    let hwid_match = monitor
        .get("hwid")
        .and_then(|v| v.as_array())
        .and_then(|a| a.get(1))
        .and_then(|v| v.as_str())
        .map(|s| s.starts_with("APPAE"))
        .unwrap_or(false);
    if hwid_match {
        return true;
    }
    monitor
        .get("name")
        .and_then(|v| v.as_str())
        .map(|n| {
            n.chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
                .to_lowercase()
                .contains("studiodisplay")
        })
        .unwrap_or(false)
}

/// getStudioDisplayAliasKey 等价（equiv L1089-L1100）。
/// 先按序列号找既有 studio-display 条目，再退回第一个 APPAE 别名条目。
pub fn get_studio_display_alias_key(
    monitors: &HashMap<String, Value>,
    used_keys: &[String],
    serial: &str,
) -> Option<String> {
    let mut keys: Vec<&String> = monitors.keys().collect();
    keys.sort(); // 保证与插入顺序无关的确定性选择

    for key in &keys {
        if used_keys.iter().any(|k| k == *key) {
            continue;
        }
        let monitor = &monitors[*key];
        if monitor.get("type").and_then(|v| v.as_str()) != Some("studio-display") {
            continue;
        }
        if monitor.get("serial").and_then(|v| v.as_str()).unwrap_or("") == serial {
            return Some((*key).clone());
        }
    }

    for key in &keys {
        if used_keys.iter().any(|k| k == *key) {
            continue;
        }
        let monitor = &monitors[*key];
        if is_studio_display_alias(monitor)
            && monitor.get("type").and_then(|v| v.as_str()) != Some("studio-display")
        {
            return Some((*key).clone());
        }
    }
    None
}

/// getStudioDisplay 等价（equiv L1102-L1181）。
/// 枚举 Studio Display 后按别名键合并进 monitors（updateDisplay 语义）。
/// 返回实际参与合并的序列号列表（调用方可用于去重）。
pub fn get_studio_display(monitors: &mut HashMap<String, Value>) -> Vec<String> {
    let mut used_serials: Vec<String> = Vec::new();

    // 已知序列号（等价 existingSerials 的去重来源）
    let existing: Vec<String> = monitors
        .values()
        .filter_map(|m| m.get("serial").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect();

    let discovered = match crate::studio_display::discover(&existing) {
        Ok(d) => d,
        Err(e) => {
            // 原版 getFallbackStudioDisplays 捕获异常后返回 []，并置 appleStudioUnavailable
            println!("getFallbackStudio Display detection failed: {}", e);
            return used_serials;
        }
    };

    for display in discovered {
        let serial = display.serial.clone();
        if serial.is_empty() {
            continue;
        }
        // usedKeys 在每轮后追加，避免同一别名键被占用两次
        let used_keys: Vec<String> = monitors.keys().cloned().collect();
        let key = match get_studio_display_alias_key(monitors, &used_keys, &serial) {
            Some(k) => k,
            None => format!("studio-{}", serial),
        };

        let brightness = match crate::studio_display::get_brightness(&serial) {
            Ok(nits) => crate::settings_state::studio_display_nits_to_percent(nits as f64),
            Err(_) => 50.0,
        };

        let info = json!({
            "id": format!("studio-display-{}", serial),
            "key": key,
            "type": "studio-display",
            "serial": serial,
            "name": "Studio Display",
            "brightness": brightness,
            "brightnessRaw": brightness,
            "brightnessMax": 100,
            "min": 0,
            "max": 100,
            "hwid": [format!("DISPLAY"), format!("APPAE{}", ""), key],
        });
        update_display(monitors, &key, info);
        used_serials.push(serial);
    }

    used_serials
}

/// 设置 appleStudioUnavailable（equiv applyMethodOverride("disableAppleStudio", ...)）。
pub fn set_apple_studio_unavailable(value: bool) {
    APPLE_STUDIO_UNAVAILABLE.store(value, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_detection_matches_js() {
        // APPAE 前缀命中
        assert!(is_studio_display_alias(
            &json!({ "hwid": ["DISPLAY", "APPAE123", "k"] })
        ));
        // 名字去掉空白后含 studiodisplay
        assert!(is_studio_display_alias(&json!({ "name": "Studio Display" })));
        assert!(is_studio_display_alias(&json!({ "name": "STUDIO  DISPLAY" })));
        assert!(!is_studio_display_alias(&json!({ "name": "Dell U2720Q" })));
        assert!(!is_studio_display_alias(&json!({})));
    }

    #[test]
    fn alias_key_prefers_serial_match() {
        let mut monitors: HashMap<String, Value> = HashMap::new();
        monitors.insert(
            "a".to_string(),
            json!({ "type": "studio-display", "serial": "S1" }),
        );
        monitors.insert("b".to_string(), json!({ "type": "ddcci", "hwid": ["X", "APPAE1", "b"] }));
        assert_eq!(get_studio_display_alias_key(&monitors, &[], "S1"), Some("a".to_string()));
        // 无序列号匹配时退回 APPAE 别名
        assert_eq!(get_studio_display_alias_key(&monitors, &[], "S2"), Some("b".to_string()));
        // 已用键被排除
        assert_eq!(
            get_studio_display_alias_key(&monitors, &["a".to_string(), "b".to_string()], "S2"),
            None
        );
    }
}
