//! gamma ramp 软件亮度（worker 侧）。
//!
//! equiv: Monitors.js:L20-L165（getSoftwareBrightnessAPI / getSoftwareBrightness /
//! gammaFeaturesActive / readGammaBrightness / applySoftwareBrightness /
//! setSoftwareBrightness / setGammaBrightness / restoreSoftwareBrightness）。
//!
//! SOFTWARE_BRIGHTNESS_MIN=20 为真机调校值（§3.2），逐字保留。
//! 底层写盘走 twutils::display_brightness（gamma ramp），每次读数都重新读回，
//! 因为 ramp 的所有者是 Windows 而不是本进程（原注释语义）。

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

use crate::monitors::update_display;
use crate::settings_state::SOFTWARE_BRIGHTNESS_MIN;

/// getSoftwareBrightnessAPI 的可失败探测：twutils 在非 GDI 环境下返回 -1。
/// 原版以模块加载失败置 softwareBrightnessUnavailable，这里以模块可用性等价表达。
fn software_api_available() -> bool {
    true
}

/// getSoftwareBrightness 等价（equiv L38-L51）。
/// 返回 None 等价原版 false。
pub fn get_software_brightness(monitor: &Value) -> Option<i32> {
    if !software_api_available() {
        return None;
    }
    let path = monitor_path(monitor)?;
    let brightness = twutils::display_brightness::get_brightness(&path);
    // typeof brightness !== "number" || !isFinite || brightness < 0 -> false
    if brightness < 0 {
        return None;
    }
    Some(brightness.clamp(SOFTWARE_BRIGHTNESS_MIN, 100))
}

/// monitor?.softwarePath || monitor?.path（原版两处共用）。
fn monitor_path(monitor: &Value) -> Option<String> {
    let path = monitor
        .get("softwarePath")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| monitor.get("path").and_then(|v| v.as_str()).filter(|s| !s.is_empty()))?;
    Some(path.to_string())
}

/// gammaFeaturesActive 等价（equiv L55-L60）。
/// 读 ramp 需要为每台显示器打开设备上下文，只有结果被真正用到时才值得做。
pub fn gamma_features_active(settings: &Value) -> bool {
    if settings
        .get("useSoftwareBrightnessFallback")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return true;
    }
    if object_has_truthy(settings.get("gammaAsMainSliderDisplays")) {
        return true;
    }
    if object_has_truthy(settings.get("extendMinimumDisplays")) {
        return true;
    }
    false
}

fn object_has_truthy(value: Option<&Value>) -> bool {
    value
        .and_then(|v| v.as_object())
        .map(|o| o.values().any(|v| v.as_bool().unwrap_or(false)))
        .unwrap_or(false)
}

/// readGammaBrightness 等价（equiv L64-L83）。
/// 共享同一 path 的显示器共享同一条 ramp，因此按 path 去重读取。
pub fn read_gamma_brightness(monitors: &mut HashMap<String, Value>, settings: &Value) {
    if !gamma_features_active(settings) || !software_api_available() {
        return;
    }

    let mut read_paths: HashMap<String, Option<i32>> = HashMap::new();
    let keys: Vec<String> = monitors.keys().cloned().collect();
    for hwid2 in keys {
        let path = match monitors.get(&hwid2).and_then(monitor_path) {
            Some(p) => p,
            None => continue,
        };
        let brightness = match read_paths.get(&path) {
            Some(cached) => *cached,
            None => {
                let b = monitors.get(&hwid2).and_then(get_software_brightness);
                read_paths.insert(path.clone(), b);
                b
            }
        };
        let brightness = match brightness {
            Some(b) => b,
            // brightness === false -> continue
            None => continue,
        };
        update_display(
            monitors,
            &hwid2,
            json!({ "softwarePath": path, "gammaBrightness": brightness }),
        );
    }
}

/// applySoftwareBrightness 等价（equiv L85-L111）。
/// 仅对 type==="none"、非 HDR active、且路径未被占用的显示器启用软件亮度。
pub fn apply_software_brightness(monitors: &mut HashMap<String, Value>, settings: &Value) {
    if !settings
        .get("useSoftwareBrightnessFallback")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return;
    }

    let mut used_paths: HashSet<String> = HashSet::new();
    let keys: Vec<String> = monitors.keys().cloned().collect();
    for hwid2 in keys {
        let monitor = match monitors.get(&hwid2) {
            Some(m) => m.clone(),
            None => continue,
        };
        let monitor_type = monitor.get("type").and_then(|v| v.as_str()).unwrap_or("none");
        let hdr = monitor.get("hdr").and_then(|v| v.as_str()).unwrap_or("");
        if monitor_type != "none" {
            continue;
        }
        if hdr == "active" {
            continue;
        }
        let path = match monitor_path(&monitor) {
            Some(p) => p,
            None => continue,
        };
        if used_paths.contains(&path) {
            continue;
        }

        // monitor.gammaBrightness >= 0 ? monitor.gammaBrightness : getSoftwareBrightness(monitor)
        let brightness = match monitor.get("gammaBrightness").and_then(|v| v.as_i64()) {
            Some(b) if b >= 0 => b as i32,
            _ => match get_software_brightness(&monitor) {
                Some(b) => b,
                None => continue,
            },
        };

        used_paths.insert(path.clone());
        update_display(
            monitors,
            &hwid2,
            json!({
                "type": "software",
                "softwarePath": path,
                "brightness": brightness,
                "brightnessRaw": brightness,
                "brightnessMax": 100,
                "brightnessValues": [brightness, 100],
                "brightnessType": false,
                "min": SOFTWARE_BRIGHTNESS_MIN,
                "max": 100,
            }),
        );
    }
}

/// setSoftwareBrightness 等价（equiv L113-L133）。
/// 返回实际下发的等级；None 等价原版 false。
pub fn set_software_brightness(
    monitors: &mut HashMap<String, Value>,
    monitor: &Value,
    brightness: f64,
) -> Option<i32> {
    if !software_api_available() {
        return None;
    }
    let path = monitor_path(monitor)?;
    let requested = brightness
        .round()
        .clamp(SOFTWARE_BRIGHTNESS_MIN as f64, 100.0);
    if !requested.is_finite() {
        return None;
    }
    let requested = requested as i32;
    if !twutils::display_brightness::set_brightness(&path, requested as f64) {
        return None;
    }

    // Track the level against every display sharing this ramp（原注释）
    for (_hwid2, other) in monitors.iter_mut() {
        if monitor_path(other).as_deref() == Some(path.as_str()) {
            other["gammaBrightness"] = json!(requested);
        }
    }
    Some(requested)
}

/// setGammaBrightness 等价（equiv L137-L149）：调试工具，任何已知 path 的显示器都可写。
pub fn set_gamma_brightness(
    monitors: &mut HashMap<String, Value>,
    brightness: f64,
    id: &str,
) -> Option<i32> {
    if id.is_empty() {
        return None;
    }
    let found = monitors.iter().find_map(|(_k, m)| {
        let mid = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if mid.contains(id) {
            Some(m.clone())
        } else {
            None
        }
    })?;
    set_software_brightness(monitors, &found, brightness)
}

/// restoreSoftwareBrightness 等价（equiv L152-L165）：
/// 关闭回退时把仍处于 software 类型的 ramp 恢复为 100。
pub fn restore_software_brightness(monitors: &mut HashMap<String, Value>) {
    if !software_api_available() {
        return;
    }
    let mut used_paths: HashSet<String> = HashSet::new();
    let keys: Vec<String> = monitors.keys().cloned().collect();
    for hwid2 in keys {
        let monitor = match monitors.get(&hwid2) {
            Some(m) => m.clone(),
            None => continue,
        };
        if monitor.get("type").and_then(|v| v.as_str()) != Some("software") {
            continue;
        }
        let path = match monitor_path(&monitor) {
            Some(p) => p,
            None => continue,
        };
        if used_paths.contains(&path) {
            continue;
        }
        used_paths.insert(path);
        set_software_brightness(monitors, &monitor, 100.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamma_features_active_matches_js() {
        // 三个条件任一为真即 active
        assert!(gamma_features_active(&json!({ "useSoftwareBrightnessFallback": true })));
        assert!(gamma_features_active(
            &json!({ "gammaAsMainSliderDisplays": { "a": true, "b": false } })
        ));
        assert!(gamma_features_active(&json!({ "extendMinimumDisplays": { "a": true } })));
        assert!(!gamma_features_active(&json!({
            "useSoftwareBrightnessFallback": false,
            "gammaAsMainSliderDisplays": { "a": false },
            "extendMinimumDisplays": {}
        })));
    }

    #[test]
    fn monitor_path_prefers_software_path() {
        let m = json!({ "softwarePath": "\\\\.\\DISPLAY1", "path": "fallback" });
        assert_eq!(monitor_path(&m).as_deref(), Some("\\\\.\\DISPLAY1"));
        let m2 = json!({ "path": "only" });
        assert_eq!(monitor_path(&m2).as_deref(), Some("only"));
        assert_eq!(monitor_path(&json!({ "softwarePath": "" })), None);
    }
}
