//! VCP 特征扫描与缓存。
//!
//! equiv: Monitors.js:L1497-L1561（checkMonitorFeatures）、L1788-L1836（checkVCPIfEnabled +
//! checkIfVCPSupported）、L1837-L1870（checkVCP）、L2159-L2164（vcpStr）、L2166-L2222
//! （DDC_ERROR_REASONS + classifyDDCError）、L1563-L1600（determineBrightnessVCPCode）。
//! 80s 全局超时 / 15s 单显示器超时由调用方（scan 循环）计时控制。

use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Mutex;

/// vcpCache（equiv L179 let vcpCache = {}）。
/// 结构: monitor -> { "vcp_0x10": [current, max], ... }
pub(crate) static VCP_CACHE: Mutex<Option<HashMap<String, HashMap<String, Value>>>> = Mutex::new(None);

/// monitorReports（equiv L181）：解析后的 capabilities 报告。
pub(crate) static MONITOR_REPORTS: Mutex<Option<HashMap<String, Value>>> = Mutex::new(None);

/// monitorReportsRaw（equiv L182）：原始 capabilities 字符串。
pub(crate) static MONITOR_REPORTS_RAW: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// vcpCache 全清（equiv flushvcp 分支）。
pub fn clear_vcp_cache() {
    *VCP_CACHE.lock().unwrap() = None;
    *MONITOR_REPORTS.lock().unwrap() = None;
    *MONITOR_REPORTS_RAW.lock().unwrap() = None;
}

/// vcpStr 等价（equiv L2159-L2161）：
/// parseInt 后再 toString(16).toUpperCase()——十进制输入 "96" -> "0x60"。
pub fn vcp_str(code: i64) -> String {
    format!("0x{:X}", code)
}

/// DDC_ERROR_REASONS 等价（equiv L2166-L2182）。winerror.h 错误码 -> 描述。
pub fn ddc_error_reason(code: u32) -> Option<&'static str> {
    match code {
        0xC026_2580 => Some("I2C not supported"),
        0xC026_2581 => Some("I2C device does not exist"),
        0xC026_2582 => Some("I2C bus error (transmit)"),
        0xC026_2583 => Some("I2C bus error (receive)"),
        0xC026_2584 => Some("VCP code unsupported"),
        0xC026_2585 => Some("invalid DDC/CI data"),
        0xC026_2586 => Some("invalid timing status byte"),
        0xC026_2587 => Some("invalid capabilities string"),
        0xC026_2589 => Some("invalid DDC/CI message command"),
        0xC026_258A => Some("invalid DDC/CI message length"),
        0xC026_258B => Some("invalid DDC/CI message checksum"),
        0xC026_258C => Some("invalid physical monitor handle"),
        0xC026_258D => Some("monitor no longer exists"),
        _ => None,
    }
}

/// classifyDDCError 等价（equiv L2184-L2200）。
/// ddcci 错误消息格式: "... (win32Code: 0xXXXX)"。
pub fn classify_ddc_error(err_msg: &str) -> String {
    // 从错误消息中提取 win32Code
    if let Some(pos) = err_msg.find("win32Code: 0x") {
        let hex = &err_msg[pos + "win32Code: 0x".len()..];
        let hex_end = hex
            .find(|c: char| !c.is_ascii_hexdigit())
            .unwrap_or(hex.len());
        if let Ok(code) = u32::from_str_radix(&hex[..hex_end], 16) {
            if let Some(reason) = ddc_error_reason(code) {
                return format!("{} (0x{:X})", reason, code);
            }
        }
    }
    // Fallback for errors without an attached code（原注释）
    if err_msg.contains("the I2C bus") {
        return "I2C bus error".to_string();
    }
    if err_msg.contains("does not support") {
        return "VCP code unsupported".to_string();
    }
    err_msg.to_string()
}

/// checkVCP 等价（equiv L1837-L1870）。
/// settings 参数: checkVCPWaitMS（默认 20，§3.2 常量）、debugForceBrightnessMax。
pub fn check_vcp(
    monitor: &str,
    code: i64,
    skip_cache_write: bool,
    use_cached_on_error: bool,
    check_vcp_wait_ms: u64,
    debug_force_brightness_max: Option<u32>,
) -> Value {
    let vcp_string = vcp_str(code);
    if code == 0 {
        return Value::Bool(false);
    }
    // SAFETY: ddcci 内部有降级分支，错误不 panic
    let result = ddcci::get_vcp(monitor, code as u8);
    match result {
        Ok((current, max)) => {
            let mut result_arr = json!([current, max]);
            if !skip_cache_write {
                let mut cache = VCP_CACHE.lock().unwrap();
                let map = cache.get_or_insert_with(HashMap::new);
                let entry = map.entry(monitor.to_string()).or_insert_with(HashMap::new);
                entry.insert(format!("vcp_{}", vcp_string), result_arr.clone());
            }
            if let Some(force_max) = debug_force_brightness_max {
                if vcp_string == "0x10" {
                    result_arr[1] = json!(force_max);
                }
            }
            // checkVCPWaitMS（默认 20ms，§3.2 时序常量）
            let wait = check_vcp_wait_ms;
            if wait > 0 {
                std::thread::sleep(std::time::Duration::from_millis(wait));
            }
            // code === 96 (0x60) 输入源特殊处理（equiv L1845）
            if code == 96 {
                match ddcci::get_monitor_inputs(monitor, Some(current)) {
                    Ok((cur, inputs)) => {
                        let mut arr = vec![json!(cur)];
                        arr.extend(inputs.into_iter().map(|v| json!(v)));
                        return Value::Array(arr);
                    }
                    Err(_) => {}
                }
            }
            result_arr
        }
        Err(e) => {
            println!(
                "Error reading VCP code {} for {}. Reason: {}",
                vcp_string,
                monitor,
                classify_ddc_error(&e)
            );
            // Since it failed, let's check for an existing value first（原注释）
            if use_cached_on_error {
                let cache = VCP_CACHE.lock().unwrap();
                if let Some(map) = cache.as_ref() {
                    if let Some(entry) = map.get(monitor) {
                        if let Some(cached) = entry.get(&format!("vcp_{}", vcp_string)) {
                            return cached.clone();
                        }
                    }
                }
            }
            Value::Bool(false)
        }
    }
}

/// checkVCPIfEnabled 等价（equiv L1788-L1816）。
pub fn check_vcp_if_enabled(
    monitor: &str,
    code: i64,
    skip_cache: bool,
    settings_monitor_features: Option<&Value>,
    check_vcp_wait_ms: u64,
    debug_force_brightness_max: Option<u32>,
) -> Value {
    let vcp_string = vcp_str(code);
    if code == 0 {
        return Value::Bool(false);
    }

    let reports = MONITOR_REPORTS.lock().unwrap();
    let report = reports.as_ref().and_then(|m| m.get(monitor)).cloned();
    drop(reports);

    let user_enabled_feature = settings_monitor_features
        .and_then(|f| f.get(&vcp_string))
        .map(|v| v.as_bool().unwrap_or(false));

    let is_in_report = report
        .as_ref()
        .and_then(|r| r.get(&vcp_string))
        .map(|v| !v.is_null())
        .unwrap_or(false);
    let has_report = report
        .as_ref()
        .and_then(|r| r.as_object())
        .map(|o| !o.is_empty())
        .unwrap_or(false);

    if has_report && !is_in_report {
        return Value::Bool(false);
    }

    // If we previously saw that a feature was supported, we shouldn't have to check again.（原注释）
    if (!skip_cache || !user_enabled_feature.unwrap_or(false)) && vcp_cache_has(monitor, &vcp_string) {
        return vcp_cache_get(monitor, &vcp_string);
    }

    check_vcp(
        monitor,
        code,
        false,
        true,
        check_vcp_wait_ms,
        debug_force_brightness_max,
    )
}

/// checkIfVCPSupported 等价（equiv L1818-L1836）。
pub fn check_if_vcp_supported(monitor: &str, code: i64, check_vcp_wait_ms: u64) -> bool {
    let vcp_string = vcp_str(code);
    if code == 0 {
        return false;
    }

    let reports = MONITOR_REPORTS.lock().unwrap();
    let report = reports.as_ref().and_then(|m| m.get(monitor)).cloned();
    drop(reports);

    let is_in_report = report
        .as_ref()
        .and_then(|r| r.get(&vcp_string))
        .map(|v| !v.is_null())
        .unwrap_or(false);
    let has_report = report
        .as_ref()
        .and_then(|r| r.as_object())
        .map(|o| !o.is_empty())
        .unwrap_or(false);

    if has_report && !is_in_report {
        return false;
    }
    if vcp_cache_has(monitor, &vcp_string) {
        return true;
    }

    let result = check_vcp_if_enabled(monitor, code, false, None, check_vcp_wait_ms, None);
    !result.is_boolean() // 非 false 即支持（等价 vcpResult ? true : false）
}

/// vcpCache 查询辅助。
fn vcp_cache_has(monitor: &str, vcp_string: &str) -> bool {
    let cache = VCP_CACHE.lock().unwrap();
    cache
        .as_ref()
        .and_then(|m| m.get(monitor))
        .map(|e| e.contains_key(&format!("vcp_{}", vcp_string)))
        .unwrap_or(false)
}

fn vcp_cache_get(monitor: &str, vcp_string: &str) -> Value {
    let cache = VCP_CACHE.lock().unwrap();
    cache
        .as_ref()
        .and_then(|m| m.get(monitor))
        .and_then(|e| e.get(&format!("vcp_{}", vcp_string)))
        .cloned()
        .unwrap_or(Value::Bool(false))
}

/// checkMonitorFeatures 等价（equiv L1497-L1561）。
/// settings 参数: monitorFeatures（按 hwid[1] 索引）、checkVCPWaitMS、debugForceBrightnessMax。
/// should_abort: 80s 全局 / 15s 单显示器超时的联合谓词（调用方闭包）。
#[allow(clippy::too_many_arguments)]
pub fn check_monitor_features(
    monitor: &str,
    skip_cache: bool,
    ddcci_method: &str,
    unstable_ddc_has: bool,
    settings_monitor_features: Option<&Value>,
    check_vcp_wait_ms: u64,
    debug_force_brightness_max: Option<u32>,
    should_abort: &dyn Fn() -> bool,
) -> Value {
    let mut features = Map::new();
    // isDDCBlocked 由调用方前置判断（此处 monitor 是 id 字符串）
    let hwid: Vec<&str> = monitor.split('#').collect();

    // Detect valid VCP codes for display if not already available（原注释）
    if should_abort() {
        return Value::Object(Map::new());
    }
    if unstable_ddc_has {
        println!(
            "Skipping capabilities report for {} due to crash evidence from a previous session.",
            monitor
        );
    } else if ddcci_method == "accurate" && monitor_reports_has(monitor) == false {
        // withDDCSentinel("capabilities", monitor, ...) 等价：
        // getCapabilitiesRaw 失败仅记录（native crash 才留哨兵证据）
        match ddcci::get_capabilities_string_impl(monitor) {
            Ok(report_raw) => {
                if should_abort() {
                    return Value::Object(Map::new());
                }
                if !report_raw.is_empty() {
                    let mut raw = MONITOR_REPORTS_RAW.lock().unwrap();
                    raw.get_or_insert_with(HashMap::new)
                        .insert(monitor.to_string(), report_raw.clone());
                    drop(raw);
                    if let Some(report) = ddcci::parse_capabilities_string(&report_raw) {
                        if !report.is_empty() {
                            let mut reports = MONITOR_REPORTS.lock().unwrap();
                            reports.get_or_insert_with(HashMap::new).insert(
                                monitor.to_string(),
                                serde_json::to_value(&report).unwrap_or(Value::Null),
                            );
                        }
                    }
                }
            }
            Err(_) => {
                println!("Couldn't get capabilities report for monitor {}", monitor);
            }
        }
    }

    if should_abort() {
        return Value::Object(Map::new());
    }

    // 原版 getAllValues 常量为 false（L1526 let getAllValues = false），走 else 分支
    // Get custom DDC/CI features
    let settings_features = settings_monitor_features
        .and_then(|f| f.get(hwid.get(1).copied().unwrap_or("")))
        .cloned();

    if let Some(sf) = &settings_features {
        if let Some(sf_obj) = sf.as_object() {
            for (vcp, enabled) in sf_obj {
                if should_abort() {
                    return Value::Object(Map::new());
                }
                // 跳过自定义亮度码由调用方传入 ddc_brightness_vcps 判定（见下）
                if enabled.as_bool().unwrap_or(false) {
                    if let Ok(code) = i64::from_str_radix(vcp.trim_start_matches("0x"), 16) {
                        let result = check_vcp_if_enabled(
                            monitor,
                            code,
                            skip_cache,
                            settings_monitor_features,
                            check_vcp_wait_ms,
                            debug_force_brightness_max,
                        );
                        features.insert(vcp_str(code), result);
                    }
                }
            }
        }
    }

    // Capabilities report allows us to skip this for unsupported codes, generally（原注释）
    const DEFAULT_FEATURES: [(i64, &str); 6] = [
        (0x10, "luminance"),
        (0x13, "brightness"),
        (0x12, "contrast"),
        (0xD6, "powerState"),
        (0x60, "inputControls"),
        (0x62, "volume"),
    ];
    for (code, _setting) in DEFAULT_FEATURES {
        if should_abort() {
            return Value::Object(Map::new());
        }
        let result = check_vcp_if_enabled(
            monitor,
            code,
            skip_cache,
            settings_monitor_features,
            check_vcp_wait_ms,
            debug_force_brightness_max,
        );
        features.insert(vcp_str(code), result);
    }

    if should_abort() {
        return Value::Object(Map::new());
    }
    Value::Object(features)
}

fn monitor_reports_has(monitor: &str) -> bool {
    let reports = MONITOR_REPORTS.lock().unwrap();
    reports
        .as_ref()
        .map(|m| m.contains_key(monitor))
        .unwrap_or(false)
}

/// determineBrightnessVCPCode 等价（equiv L1563-L1600）。
pub fn determine_brightness_vcp_code(
    monitor: &str,
    ddc_brightness_vcps: &Value,
    check_vcp_wait_ms: u64,
) -> Option<u8> {
    let hwid: Vec<&str> = monitor.split('#').collect();
    let hwid1 = hwid.get(1).copied().unwrap_or("");

    // 用户自定义亮度 VCP 优先
    if let Some(custom) = ddc_brightness_vcps.get(hwid1) {
        if let Some(v) = custom.as_u64() {
            return Some(v as u8);
        }
        if let Some(v) = custom.as_str().and_then(|s| s.parse::<u64>().ok()) {
            return Some(v as u8);
        }
    }

    const CANDIDATES: [(i64, u8); 4] = [
        (0x10, 0x10), // luminance
        (0x13, 0x13), // brightness
        (0x6B, 0x6b), // backlight level white
        (0x12, 0x12), // contrast
    ];
    for (code, _) in CANDIDATES {
        if check_if_vcp_supported(monitor, code, check_vcp_wait_ms) {
            return Some(code as u8);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vcp_str_matches_js() {
        // parseInt("96").toString(16).toUpperCase() = "60"
        assert_eq!(vcp_str(96), "0x60");
        assert_eq!(vcp_str(0x10), "0x10");
        assert_eq!(vcp_str(0xD6), "0xD6");
    }

    #[test]
    fn ddc_error_classification() {
        assert_eq!(
            classify_ddc_error("Failed to get VCP code value (win32Code: 0xC0262584)"),
            "VCP code unsupported (0xC0262584)"
        );
        assert_eq!(classify_ddc_error("the I2C bus is busy"), "I2C bus error");
        assert_eq!(classify_ddc_error("generic failure"), "generic failure");
    }

    #[test]
    fn cache_roundtrip() {
        clear_vcp_cache();
        let mut cache = VCP_CACHE.lock().unwrap();
        let map = cache.get_or_insert_with(HashMap::new);
        let entry = map.entry("monitor1".to_string()).or_insert_with(HashMap::new);
        entry.insert("vcp_0x10".to_string(), json!([50, 100]));
        drop(cache);
        assert!(vcp_cache_has("monitor1", "0x10"));
        assert_eq!(vcp_cache_get("monitor1", "0x10"), json!([50, 100]));
        assert!(!vcp_cache_has("monitor1", "0x12"));
        clear_vcp_cache();
    }
}


/// monitorReports 快照（供 monitors.rs 读取）。
pub fn monitor_reports_snapshot() -> HashMap<String, Value> {
    let reports = MONITOR_REPORTS.lock().unwrap();
    reports.as_ref().cloned().unwrap_or_default()
}
