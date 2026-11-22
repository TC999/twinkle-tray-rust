//! 亮度/VCP 读写操作（worker 侧）。
//!
//! equiv: Monitors.js:L1727-L1770（setSDRBrightness / setBrightness）、
//! L1636-L1703（getBrightnessDDC）、L1884-L1928（setVCP / getHighLevelBrightness /
//! setHighLevelBrightness）、L1788-L1836（checkVCPIfEnabled / checkIfVCPSupported）。
//!
//! §3.2 高危区：亮度方法降级链 DDC → HighLevel → 软件亮度必须逐分支保留。

use serde_json::{json, Value};
use std::collections::HashMap;

use crate::features;
use crate::monitors::update_display;
use crate::settings_state::{normalize_brightness, vcp_str, WorkerSettings, SOFTWARE_BRIGHTNESS_MIN};
use crate::software_brightness::{set_software_brightness, set_gamma_brightness};
use crate::wmi_fallback::{apply_remap, can_use_wmi_bridge_now, CAN_USE_WMI_BRIDGE, WMI_FAILED};

/// worker 的显示器模型（等价 Monitors.js 模块级 `let monitors`）。
pub type MonitorMap = HashMap<String, Value>;

/// isDDCBlocked 的字符串形式（equiv L444-L452 的 monitorString 分支）。
pub fn is_ddc_blocked_str(settings: &WorkerSettings, monitor_string: &str) -> bool {
    if monitor_string.is_empty() {
        return false;
    }
    settings
        .ddc_blocked_keys()
        .iter()
        .any(|key| monitor_string.contains(key.as_str()))
}

/// checkVCP 等价入口（equiv L1837-L1862）：统一走 features::check_vcp 的缓存语义。
fn check_vcp_for(
    settings: &WorkerSettings,
    monitor: &str,
    code: i64,
    skip_cache_write: bool,
    use_cached_on_error: bool,
) -> Value {
    if is_ddc_blocked_str(settings, monitor) {
        return Value::Bool(false);
    }
    let wait = settings
        .get("checkVCPWaitMS")
        .and_then(|v| v.as_u64())
        .unwrap_or(20);
    features::check_vcp(
        monitor,
        code,
        skip_cache_write,
        use_cached_on_error,
        wait,
        settings.debug_force_brightness_max(),
    )
}

/// setSDRBrightness 等价（equiv L1727-L1736）。
/// nits = brightness * 0.01 * 400 + 80（HDR SDR white level 映射，逐字保留）。
pub fn set_sdr_brightness(settings: &WorkerSettings, brightness: f64, id: &str) -> bool {
    if settings.disable_hdr() {
        return false;
    }
    let nits = (brightness * 0.01 * 400.0) + 80.0;
    let ok = displayconfig::hdr::set_sdr_brightness(id, nits as i32);
    if !ok {
        println!("Couldn't update SDR brightness! [{}]", id);
    }
    ok
}

/// setBrightness 等价（equiv L1738-L1785）。
/// 单显示器：studio-display / software / high-level / DDC 四分支；无 id：WMI 全局。
pub fn set_brightness(
    monitors: &mut MonitorMap,
    settings: &WorkerSettings,
    brightness: f64,
    id: Option<&str>,
    ddc_brightness_vcps: &Value,
) {
    match id {
        Some(id) => {
            // Object.values(monitors).find(mon => mon.id?.indexOf(id) >= 0)
            let found_key = monitors.iter().find_map(|(k, m)| {
                let mid = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if mid.contains(id) {
                    Some(k.clone())
                } else {
                    None
                }
            });
            let key = match found_key {
                Some(k) => k,
                None => return,
            };
            let monitor = monitors.get(&key).cloned().unwrap_or(json!({}));
            let monitor_type = monitor
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("none")
                .to_string();

            // Check if user has set a custom brightness VCP code for this monitor（原注释）
            let hwid1 = monitor
                .get("hwid")
                .and_then(|v| v.as_array())
                .and_then(|a| a.get(1))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let has_custom_brightness_vcp = !hwid1.is_empty()
                && ddc_brightness_vcps
                    .get(&hwid1)
                    .map(|v| !v.is_null())
                    .unwrap_or(false);

            let high_level_supported_brightness = monitor
                .get("highLevelSupported")
                .and_then(|v| v.get("brightness"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if monitor_type == "studio-display" {
                // setStudioDisplayBrightness(monitor.serial, brightness)
                let serial = monitor
                    .get("serial")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                crate::brightness_ops::set_studio_display_brightness(&serial, brightness);
            } else if monitor_type == "software" {
                if set_software_brightness(monitors, &monitor, brightness).is_none() {
                    let mid = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    println!("Couldn't set software brightness for monitor {}", mid);
                    return;
                }
            } else if !settings.disable_high_level()
                && high_level_supported_brightness
                && !has_custom_brightness_vcp
            {
                let path = join_hwid(&monitor);
                set_high_level_brightness(&path, brightness);
            } else {
                let path = join_hwid(&monitor);
                let brightness_type = monitor
                    .get("brightnessType")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0x10);
                set_vcp_for(settings, &path, brightness_type as u8, brightness);
            }

            // Update tracked brightness values（原注释）
            let parsed = brightness as i64;
            let brightness_raw = if monitor_type == "software" {
                parsed.max(SOFTWARE_BRIGHTNESS_MIN as i64)
            } else {
                parsed
            };
            let brightness_max = monitor
                .get("brightnessMax")
                .and_then(|v| v.as_f64())
                .filter(|v| *v != 0.0)
                .unwrap_or(100.0);
            if let Some(entry) = monitors.get_mut(&key) {
                entry["brightness"] = json!(brightness_raw as f64 * (100.0 / brightness_max));
                entry["brightnessRaw"] = json!(brightness_raw);
                if let Some(values) = entry.get_mut("brightnessValues").and_then(|v| v.as_array_mut()) {
                    if let Some(first) = values.first_mut() {
                        *first = json!(brightness_raw);
                    }
                }
            }
        }
        None => {
            // WMI only takes whole levels, and the bridge rejects anything else.
            // Normalization and calibration usually produce fractions.（原注释）
            let brightness = brightness.clamp(0.0, 100.0).round();
            // WMI 显示器条目（mon.type == "wmi"）同步跟踪值
            if let Some(key) = monitors.iter().find_map(|(k, m)| {
                if m.get("type").and_then(|v| v.as_str()) == Some("wmi") {
                    Some(k.clone())
                } else {
                    None
                }
            }) {
                if let Some(entry) = monitors.get_mut(&key) {
                    entry["brightness"] = json!(brightness);
                    entry["brightnessRaw"] = json!(brightness);
                }
            }
            if can_use_wmi_bridge_now() {
                // Set brightness via native WMI（原注释）
                wmibridge::set_brightness(brightness as i32);
            } else {
                // If native WMI is unavailable, fall back to old method（原注释）
                use std::process::Command;
                let _ = Command::new("powershell.exe")
                    .args([
                        "-NoProfile",
                        &format!(
                            "(Get-WmiObject -Namespace root\\\\wmi -Class WmiMonitorBrightnessMethods).wmisetbrightness(0, {})",
                            brightness as i32
                        ),
                    ])
                    .spawn();
            }
        }
    }
}

/// monitor.hwid.join("#")（原版多处使用）。
fn join_hwid(monitor: &Value) -> String {
    monitor
        .get("hwid")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|v| v.as_str().unwrap_or("").to_string())
                .collect::<Vec<_>>()
                .join("#")
        })
        .unwrap_or_default()
}

/// setVCP 等价（equiv L1884-L1905）。
/// 写成功时同步 vcpCache 与显示器特征值（原版三处副作用逐字保留）。
pub fn set_vcp_for(settings: &WorkerSettings, monitor: &str, code: u8, value: f64) -> bool {
    if is_ddc_blocked_str(settings, monitor) {
        return false;
    }
    match ddcci::set_vcp(monitor, code, value as u32) {
        Ok(()) => {
            features::update_vcp_cache_value(monitor, code as i64, value);
            true
        }
        Err(e) => {
            println!(
                "Error setting VCP code {} for {}. Reason: {}",
                vcp_str(code as i32),
                monitor,
                features::classify_ddc_error(&e)
            );
            false
        }
    }
}

/// getHighLevelBrightness 等价（equiv L1907-L1916）：返回 [current, max]。
pub fn get_high_level_brightness(settings: &WorkerSettings, monitor: &str) -> Value {
    if is_ddc_blocked_str(settings, monitor) {
        return Value::Bool(false);
    }
    match ddcci::get_high_level_brightness(monitor) {
        Ok((current, max, _min)) => json!([current, max]),
        Err(e) => {
            println!("{}", e);
            Value::Bool(false)
        }
    }
}

/// setHighLevelBrightness 等价（equiv L1918-L1928）。
pub fn set_high_level_brightness(monitor: &str, value: f64) -> bool {
    match ddcci::set_high_level_brightness(monitor, value as u32) {
        Ok(()) => true,
        Err(e) => {
            println!("{}", e);
            false
        }
    }
}

/// setStudioDisplayBrightness 等价（equiv L1050-L1083 的 USB 控制传输路径）。
/// Studio Display 经 WinUSB 直调；未接入时保留降级（返回 false 不 panic）。
pub fn set_studio_display_brightness(serial: &str, brightness: f64) -> bool {
    let nits = crate::settings_state::percent_to_studio_display_nits(brightness);
    match crate::studio_display::set_brightness(serial, nits) {
        Ok(applied) => applied,
        Err(e) => {
            println!("Couldn't set Studio Display brightness: {}", e);
            false
        }
    }
}

/// getBrightnessDDC 等价（equiv L1636-L1703）。
/// 返回 (更新后的 monitor, brightnessReadFailed)。
pub fn get_brightness_ddc(
    settings: &WorkerSettings,
    monitor_obj: &Value,
    include_features: bool,
    report_read_failure: bool,
) -> (Value, bool) {
    let mut monitor = monitor_obj.clone();
    let path = join_hwid(&monitor);

    // If brightness is not supported, or DDC/CI is blocked, stop（原注释）
    let brightness_type = monitor.get("brightnessType").cloned().unwrap_or(Value::Bool(false));
    let has_brightness_type = brightness_type.as_u64().map(|v| v > 0).unwrap_or(false);
    if !has_brightness_type || is_ddc_blocked_str(settings, &path) {
        return (monitor, false);
    }
    let code = brightness_type.as_u64().unwrap_or(0x10) as i64;

    // Determine / get brightness（原注释）：reportReadFailure 时不使用错误缓存
    let mut brightness_values = check_vcp_for(settings, &path, code, false, !report_read_failure);
    let brightness_read_failed = !brightness_values.is_array();

    // If something goes wrong and there are previous values, use those（原注释）
    if !brightness_values.is_array() {
        let hwid1 = monitor
            .get("hwid")
            .and_then(|v| v.as_array())
            .and_then(|a| a.get(1))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        println!("\x1b[41mNO BRIGHTNESS VALUES AVAILABLE FOR {}\x1b[0m", hwid1);
        let raw = monitor.get("brightnessRaw").and_then(|v| v.as_f64());
        let max = monitor.get("brightnessMax").and_then(|v| v.as_f64());
        if let (Some(raw), Some(max)) = (raw, max) {
            println!("\x1b[41mUSING PREVIOUS VALUES\x1b[0m");
            brightness_values = json!([raw, max]);
        } else if let Some(cached) = features::vcp_cache_value(&path, 0x10) {
            println!("\x1b[41mUSING VCP CACHE\x1b[0m");
            brightness_values = cached;
        } else {
            println!("CATASTROPHIC FAILURE {}", monitor);
            // Catastrophic failure. Revert to defaults.（原注释）
            brightness_values = json!([50, 100]);
        }
    }

    let values = brightness_values.as_array().cloned().unwrap_or_default();
    let raw = values.first().and_then(|v| v.as_f64()).unwrap_or(0.0);
    let max = values.get(1).and_then(|v| v.as_f64()).filter(|v| *v != 0.0).unwrap_or(100.0);
    monitor["brightness"] = json!(raw * (100.0 / max));
    monitor["brightnessMax"] = json!(max);
    monitor["brightnessRaw"] = json!(raw);

    // Get normalization info（原注释）
    let remaps = settings.get("remaps").cloned().unwrap_or(json!({}));
    monitor = apply_remap(monitor, &remaps);
    // Unnormalize brightness（原注释）
    let min = monitor.get("min").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let max_range = monitor.get("max").and_then(|v| v.as_i64()).unwrap_or(100) as i32;
    let current = monitor.get("brightness").and_then(|v| v.as_f64()).unwrap_or(0.0);
    monitor["brightness"] = json!(normalize_brightness(current, true, min, max_range));

    // Get custom DDC/CI features（原注释）
    if include_features {
        let hwid1 = monitor
            .get("hwid")
            .and_then(|v| v.as_array())
            .and_then(|a| a.get(1))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let settings_features = settings
            .get("monitorFeatures")
            .and_then(|v| v.get(&hwid1))
            .cloned();
        if let Some(features_obj) = settings_features.as_ref().and_then(|v| v.as_object()) {
            let id = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let brightness_type_num = monitor
                .get("brightnessType")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            for (vcp, enabled) in features_obj {
                if vcp == &brightness_type_num.to_string() {
                    continue; // Skip brightness（原注释）
                }
                if !enabled.as_bool().unwrap_or(false) {
                    continue;
                }
                if let Ok(code) = i64::from_str_radix(vcp.trim_start_matches("0x"), 16) {
                    let value = check_vcp_for(settings, &id, code, false, true);
                    monitor["features"][vcp_str(code as i32)] = value;
                }
            }
        }
    }

    if report_read_failure {
        monitor["brightnessReadFailed"] = json!(brightness_read_failed);
    }
    (monitor, brightness_read_failed)
}

/// readKnownBrightness 等价（equiv L644-L720）：只读取回亮度，不重新枚举/扫描。
pub fn read_known_brightness(
    monitors: &mut MonitorMap,
    settings: &WorkerSettings,
) -> HashMap<String, Value> {
    let mut result: HashMap<String, Value> = HashMap::new();
    let mut unreadable_monitor_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    // DDC 读数：仅 type==="ddcci" 且 brightnessType 为真的显示器
    let ddc_keys: Vec<String> = monitors
        .iter()
        .filter(|(_k, m)| {
            m.get("type").and_then(|v| v.as_str()) == Some("ddcci")
                && m.get("brightnessType")
                    .and_then(|v| v.as_u64())
                    .map(|v| v > 0)
                    .unwrap_or(false)
        })
        .map(|(k, _)| k.clone())
        .collect();
    for hwid2 in ddc_keys {
        let monitor = match monitors.get(&hwid2) {
            Some(m) => m.clone(),
            None => continue,
        };
        let (refreshed, read_failed) = get_brightness_ddc(settings, &monitor, false, true);
        if !read_failed {
            monitors.insert(hwid2, refreshed);
        } else if let Some(id) = monitor.get("id").and_then(|v| v.as_str()) {
            unreadable_monitor_ids.insert(id.to_string());
        }
    }

    // Internal display brightness (native WMI preferred, WMIC fallback)（原注释）
    if crate::wmi_fallback::can_use_internal_brightness() {
        let remaps = settings.get("remaps").cloned().unwrap_or(json!({}));
        match crate::wmi_fallback::get_brightness_internal(&remaps) {
            Ok(Some(wmi_brightness)) => {
                if let Some(hwid2) = wmi_brightness
                    .get("hwid")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.get(2))
                    .and_then(|v| v.as_str())
                {
                    let hwid2 = hwid2.to_string();
                    update_display(monitors, &hwid2, wmi_brightness);
                }
            }
            Ok(None) => {}
            Err(()) => println!("\x1b[41mgetKnownBrightnessInternal() failed!\x1b[0m"),
        }
    }

    // Studio Display 读数
    if !crate::wmi_fallback::APPLE_STUDIO_UNAVAILABLE.load(std::sync::atomic::Ordering::SeqCst) {
        let studio_keys: Vec<String> = monitors
            .iter()
            .filter(|(_k, m)| m.get("type").and_then(|v| v.as_str()) == Some("studio-display"))
            .map(|(k, _)| k.clone())
            .collect();
        for hwid2 in studio_keys {
            let serial = monitors
                .get(&hwid2)
                .and_then(|m| m.get("serial"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            match crate::studio_display::get_brightness(&serial) {
                Ok(nits) => {
                    let percent =
                        crate::settings_state::studio_display_nits_to_percent(nits as f64);
                    if let Some(entry) = monitors.get_mut(&hwid2) {
                        entry["brightness"] = json!(percent);
                        entry["brightnessRaw"] = json!(percent);
                        entry["brightnessMax"] = json!(100);
                    }
                }
                Err(e) => {
                    if let Some(id) = monitors
                        .get(&hwid2)
                        .and_then(|m| m.get("id"))
                        .and_then(|v| v.as_str())
                    {
                        unreadable_monitor_ids.insert(id.to_string());
                    }
                    println!("\x1b[41mgetKnownBrightnessStudioDisplay() failed!\x1b[0m {}", e);
                }
            }
        }
    }

    // HDR
    if !settings.disable_hdr() {
        let mut snapshot = monitors.clone();
        crate::wmi_fallback::get_hdr_displays(&mut snapshot);
        *monitors = snapshot;
    }

    crate::software_brightness::read_gamma_brightness(monitors, &settings.raw);
    crate::software_brightness::apply_software_brightness(monitors, &settings.raw);

    let gamma_as_main = settings
        .get("gammaAsMainSliderDisplays")
        .cloned()
        .unwrap_or(json!({}));
    for (_hwid2, monitor) in monitors.iter() {
        let id = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if id.is_empty() {
            continue;
        }
        let monitor_type = monitor.get("type").and_then(|v| v.as_str()).unwrap_or("none");
        let gamma_opt_in = gamma_as_main
            .get(_hwid2)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if monitor_type == "none" && !gamma_opt_in {
            continue;
        }
        if unreadable_monitor_ids.contains(id) {
            continue;
        }
        result.insert(
            id.to_string(),
            json!({
                "brightness": monitor.get("brightness").cloned().unwrap_or(Value::Null),
                "brightnessRaw": monitor.get("brightnessRaw").cloned().unwrap_or(Value::Null),
                "brightnessMax": monitor.get("brightnessMax").cloned().unwrap_or(Value::Null),
                "gammaBrightness": monitor.get("gammaBrightness").cloned().unwrap_or(Value::Null),
                "sdrLevel": monitor.get("sdrLevel").cloned().unwrap_or(Value::Null),
                "hdr": monitor.get("hdr").cloned().unwrap_or(Value::Null),
            }),
        );
    }
    result
}

/// 主进程 wmi-bridge-ok 消息置位（equiv L273-L274）。
pub fn set_can_use_wmi_bridge(value: bool) {
    CAN_USE_WMI_BRIDGE.store(value, std::sync::atomic::Ordering::SeqCst);
    if !value {
        WMI_FAILED.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// setGammaBrightness 的消息入口（equiv gamma 分支）。
pub fn handle_gamma_message(
    monitors: &mut MonitorMap,
    brightness: f64,
    id: Option<&str>,
) -> Option<i32> {
    let id = id?;
    set_gamma_brightness(monitors, brightness, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_hwid_matches_js() {
        assert_eq!(
            join_hwid(&json!({ "hwid": ["a", "b", "c"] })),
            "a#b#c"
        );
        assert_eq!(join_hwid(&json!({})), "");
    }

    #[test]
    fn is_ddc_blocked_matches_substring_semantics() {
        let settings = WorkerSettings::from_value(json!({
            "disableDDCDisplays": { "4&abc123&0&UID1234": true, "off": false }
        }));
        assert!(is_ddc_blocked_str(&settings, "\\\\?\\DISPLAY#X#4&abc123&0&UID1234"));
        assert!(!is_ddc_blocked_str(&settings, "\\\\?\\DISPLAY#X#other"));
        // 空字符串不阻塞（equiv if (!monitorString) return false）
        assert!(!is_ddc_blocked_str(&settings, ""));
    }
}
