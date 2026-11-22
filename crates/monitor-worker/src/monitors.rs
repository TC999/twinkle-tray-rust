//! 显示器三源枚举合并 + 特征快照。
//!
//! equiv: Monitors.js getAllMonitors（L766-L980）、updateDisplay（L1705-L1723）、
//! getMonitorsWMI（L1300-L1336）、getMonitorsWin32（L1346-L1390）、readInstanceName（L1976-L1978）、
//! featureSnapshotKey/saveFeatureSnapshots/applyFeatureSnapshots/invalidateFeatureSnapshots
//! （L453-L520）、changedFeatureMonitorIds（L503-L520）。
//! hwid 三源对齐（WMI/Win32/DDC 标识合并）是 §3.2 高危区：错位会导致控制错显示器。

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// updateDisplay 等价（equiv L1705-L1723）：默认字段骨架 + Object.assign 语义。
pub fn update_display(monitors: &mut HashMap<String, Value>, hwid2: &str, info: Value) {
    let entry = monitors.entry(hwid2.to_string()).or_insert_with(|| {
        json!({
            "id": null,
            "key": null,
            "num": null,
            "brightness": 50,
            "brightnessMax": 100,
            "brightnessRaw": 50,
            "type": "none",
            "connector": "unknown",
            "min": 0,
            "max": 100,
            "hwid": [],
            "name": null,
            "serial": null
        })
    });
    if let (Value::Object(base), Value::Object(patch)) = (entry, &info) {
        for (k, v) in patch {
            base.insert(k.clone(), v.clone());
        }
    }
}

/// featureSnapshotKey 等价（equiv L453-L455）：hwid 数组 # 连接。
pub fn feature_snapshot_key(monitor: &Value) -> Option<String> {
    monitor
        .get("hwid")
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter()
                .map(|v| v.as_str().unwrap_or("").to_string())
                .collect::<Vec<_>>()
                .join("#")
        })
}

/// saveFeatureSnapshots 等价（equiv L465-L478）。
/// 保存 features/vcpCodes 到快照（acceptInvalidatedSnapshots: 失效快照是否仍接受）。
pub fn save_feature_snapshots(
    monitors: &HashMap<String, Value>,
    snapshots: &mut HashMap<String, Value>,
    invalidated: &HashSet<String>,
    accept_invalidated_snapshots: bool,
) {
    for (_key, monitor) in monitors {
        let snapshot_key = match feature_snapshot_key(monitor) {
            Some(k) => k,
            None => continue,
        };
        if !accept_invalidated_snapshots && invalidated.contains(&snapshot_key) {
            continue;
        }
        if let (Some(features), Some(vcp_codes)) =
            (monitor.get("features"), monitor.get("vcpCodes"))
        {
            if features.is_object() && !features.as_object().unwrap().is_empty() {
                snapshots.insert(
                    snapshot_key,
                    json!({
                        "features": features,
                        "vcpCodes": vcp_codes,
                    }),
                );
            }
        }
    }
}

/// applyFeatureSnapshots 等价（equiv L479-L496）。
pub fn apply_feature_snapshots(
    monitors: &mut HashMap<String, Value>,
    snapshots: &HashMap<String, Value>,
) {
    for (_key, monitor) in monitors.iter_mut() {
        let snapshot_key = match feature_snapshot_key(monitor) {
            Some(k) => k,
            None => continue,
        };
        if let Some(snapshot) = snapshots.get(&snapshot_key) {
            if let Some(features) = snapshot.get("features") {
                monitor["features"] = features.clone();
            }
            if let Some(vcp_codes) = snapshot.get("vcpCodes") {
                monitor["vcpCodes"] = vcp_codes.clone();
            }
        }
    }
}

/// changedFeatureMonitorIds 等价（equiv L503-L520）：
/// 比较 settings 中会改变特征的键。原版比较对象引用/JSON——这里按 JSON 值比较。
pub fn changed_feature_monitor_ids(previous: &Value, next: &Value) -> HashSet<String> {
    let mut changed = HashSet::new();

    let changed_feature_keys = [
        "disableHighLevel",
        "disableHDR",
        "disableDDCDisplays",
        "hideClosedLid",
        "monitorFeatures",
        "disableSoftwareBrightnessFallback",
        "gammaAsMainSliderDisplays",
        "disableAppleStudio",
        "disableWMIC",
        "disableWMI",
        "disableWin32",
    ];

    for key in changed_feature_keys {
        let prev = previous.get(key);
        let next_v = next.get(key);
        if prev != next_v {
            // 对 monitorFeatures：收集涉及的 hwid[1] 键
            if key == "monitorFeatures" {
                if let (Some(p), Some(n)) = (prev.and_then(|v| v.as_object()), next_v.and_then(|v| v.as_object())) {
                    for k in p.keys().chain(n.keys()) {
                        changed.insert(k.clone());
                    }
                }
            } else {
                // 全局开关变化影响所有显示器：原版对这类键的响应在 handleMonitorMessage
                // 的 invalidateFeatureSnapshots(changedMonitors) 中处理
                changed.insert("*".to_string());
            }
        }
    }
    changed
}

/// invalidateFeatureSnapshots 等价（equiv L522-L528）。
pub fn invalidate_feature_snapshots(
    monitor_ids: &HashSet<String>,
    invalidated: &mut HashSet<String>,
) {
    for id in monitor_ids {
        if id == "*" {
            // 全局变化：由调用方 clear 整个集合（等价原版对全局键的处理）
            continue;
        }
        invalidated.insert(id.clone());
    }
}

/// readInstanceName 等价（equiv L1976-L1978）：
/// &amp; HTML 实体还原 + 反斜杠切分。
pub fn read_instance_name(ins_name: Option<&str>) -> Option<Vec<String>> {
    ins_name.map(|name| {
        name.replace("&amp;", "&")
            .split('\\')
            .map(|s| s.to_string())
            .collect()
    })
}

/// parseWMIString 等价（equiv L1980-L1990）：
/// WMIC 的 {49;32;67;...} 十进制字节串解码。null 直通。
pub fn parse_wmi_string(s: Option<&str>) -> Option<String> {
    let s = s?;
    let hexed = s.replace('{', "").replace('}', "").replace(";0", ";32");
    let mut decoded = String::new();
    for part in hexed.split(';') {
        if let Ok(code) = part.parse::<u32>() {
            if let Some(ch) = char::from_u32(code) {
                decoded.push(ch);
            }
        }
    }
    Some(decoded.trim().to_string())
}

/// getMonitorsWin32 等价（equiv L1346-L1390）：extractDisplayConfig 的 inUse 过滤与 hwid 切分。
pub fn get_monitors_win32() -> Result<HashMap<String, Value>, String> {
    let mut found_displays = HashMap::new();
    let display_config = displayconfig::extract_display_config()
        .map_err(|e| format!("getMonitorsWin32: Failed to get all monitors. {:?}", e))?;

    for display in display_config {
        // Must be an active display（原注释）
        if !display.in_use {
            continue;
        }
        let parts: Vec<&str> = display.device_path.split('#').collect();
        if parts.len() < 3 {
            continue;
        }
        let hwid0 = parts[0].to_string();
        let hwid1 = parts[1].to_string();
        let hwid2_full = parts[2];
        let hwid2 = hwid2_full.split('_').next().unwrap_or(hwid2_full).to_string();

        let mut win32_info = json!({
            "id": format!("{}#{}#{}", hwid0, hwid1, hwid2),
            "key": hwid2,
            "connector": display.output_technology,
            "hwid": [hwid0, hwid1, hwid2],
            "sourceID": display.source_config_id.id,
            "scaling": display.scaling,
            "bounds": {
                "width": display.source_mode.width,
                "height": display.source_mode.height,
                "position": {
                    "x": display.source_mode.position_x,
                    "y": display.source_mode.position_y,
                },
                "pixelFormat": display.source_mode.pixel_format,
            },
        });
        if !display.display_name.is_empty() {
            win32_info["name"] = json!(display.display_name);
        }

        found_displays.insert(hwid2, win32_info);
    }
    Ok(found_displays)
}

/// getMonitorsWMI 等价（equiv L1300-L1336）。
pub fn get_monitors_wmi() -> Result<HashMap<String, Value>, ()> {
    let mut found_monitors: HashMap<String, Value> = HashMap::new();

    match wmibridge::get_monitors() {
        Err(_) => {
            println!("\x1b[41mRecieved FAILED response from getMonitors()\x1b[0m");
            Err(())
        }
        Ok(wmi_monitors) => {
            for monitor in wmi_monitors {
                let instance_name = monitor.instance_name.clone();
                if instance_name.is_empty() {
                    continue;
                }
                let hwid = match read_instance_name(Some(&instance_name)) {
                    Some(h) => h,
                    None => continue,
                };
                if hwid.len() < 3 {
                    continue;
                }
                let hwid2_full = hwid[2].clone();
                let hwid2 = hwid2_full.split('_').next().unwrap_or(&hwid2_full).to_string();

                let mut wmi_info = json!({
                    "id": format!("\\\\?\\{}#{}#{}", hwid[0], hwid[1], hwid2),
                    "key": hwid2,
                    "hwid": hwid,
                });
                if !monitor.user_friendly_name.as_deref().unwrap_or("").is_empty() {
                    wmi_info["name"] = json!(monitor.user_friendly_name.clone().unwrap_or_default());
                }
                found_monitors.insert(hwid2, wmi_info);
            }
            Ok(found_monitors)
        }
    }
}

/// getBrightnessWMI 等价（equiv L1595-L1635）。
pub fn get_brightness_wmi() -> Result<Option<Value>, ()> {
    let result = wmibridge::get_brightness();
    if result.failed {
        if result.unsupported {
            return Ok(None);
        }
        return Err(());
    }
    let instance_name = result.instance_name.clone().unwrap_or_default();
    let hwid = match read_instance_name(Some(&instance_name)) {
        Some(h) => h,
        None => return Err(()),
    };
    if hwid.len() < 3 {
        return Err(());
    }
    let hwid2_full = hwid[2].clone();
    let hwid2 = hwid2_full.split('_').next().unwrap_or(&hwid2_full).to_string();
    let brightness = result.brightness.unwrap_or(0);

    let mut wmi_info = json!({
        "id": format!("\\\\?\\{}#{}#{}", hwid[0], hwid[1], hwid2),
        "brightness": brightness,
        "hwid": hwid,
        "min": 0,
        "max": 100,
        "type": "wmi",
    });
    let brightness_raw = brightness;
    let normalized = crate::settings_state::normalize_brightness(brightness_raw as f64, true, 0, 100);
    wmi_info["brightnessRaw"] = json!(brightness_raw);
    wmi_info["brightness"] = json!(normalized);
    Ok(Some(wmi_info))
}

/// DDC 侧单显示器特征记录（equiv L1445-L1460 的 monitorFeatures 元素）。
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DdcFeatureEntry {
    pub id: String,
    pub hwid: Vec<String>,
    pub features: Value,
    pub feature_scan_timed_out: bool,
    pub ddcci_supported: bool,
    pub hl_brightness_supported: bool,
    pub hl_contrast_supported: bool,
    pub path: String,
    pub vcp_codes: Value,
}

/// getFeaturesDDC 完整版（equiv L1392-L1460）：
/// DDC 枚举 -> 崩溃哨兵包裹 -> 逐显示器特征扫描（capabilities 获取 + 默认特征 VCP 读取）。
/// 全局 80s / 单显示器 15s 超时由调用方以时间戳实现（等价 setTimeout 语义）。
pub fn get_features_ddc(
    ddcci_method: &str,
    core_only: bool,
    disable_high_level: bool,
    blocked_keys: &[String],
    unstable_ddc: &std::collections::HashMap<String, Value>,
    settings_monitor_features: Option<&Value>,
    check_vcp_wait_ms: u64,
    debug_force_brightness_max: Option<u32>,
) -> HashMap<String, DdcFeatureEntry> {
    let mut monitor_features = HashMap::new();

    let _ = ddcci::refresh(ddcci_method, true, !disable_high_level, blocked_keys);
    let ddcci_monitors = ddcci::get_all_monitors();

    // 全局 80s 超时（equiv L1398-L1404 setTimeout 80000）
    let global_start = std::time::Instant::now();
    const GLOBAL_TIMEOUT_MS: u128 = 80_000;
    const PER_MONITOR_TIMEOUT_MS: u128 = 15_000;

    for monitor in &ddcci_monitors {
        // timedOut -> break（equiv L1442）
        if global_start.elapsed().as_millis() >= GLOBAL_TIMEOUT_MS {
            println!("getFeaturesDDC Timed out. Returning partial results.");
            break;
        }
        let id = monitor.device_key.clone();
        if id.is_empty() {
            continue;
        }
        let monitor_start = std::time::Instant::now();
        let hwid: Vec<String> = id.split('#').map(|s| s.to_string()).collect();
        if hwid.len() < 3 {
            continue;
        }
        let hwid2 = hwid[2].clone();

        // shouldAbortFeatureScan 等价（equiv L1448）：全局超时 || 单显示器超时
        let should_abort = || {
            global_start.elapsed().as_millis() >= GLOBAL_TIMEOUT_MS
                || monitor_start.elapsed().as_millis() >= PER_MONITOR_TIMEOUT_MS
        };

        // capabilities 报告（受哨兵保护）
        let features = if monitor.ddcci_supported && !core_only && !should_abort() {
            let unstable_has = unstable_ddc.contains_key(&id);
            let _ = std::thread::sleep(std::time::Duration::from_millis(10));
            crate::features::check_monitor_features(
                &id,
                false,
                ddcci_method,
                unstable_has,
                settings_monitor_features,
                check_vcp_wait_ms,
                debug_force_brightness_max,
                &should_abort,
            )
        } else {
            json!({})
        };

        // capabilities 报告保存到 monitorReports（equiv L1440-L1443）
        let monitor_reports = crate::features::monitor_reports_snapshot();
        let vcp_codes = monitor_reports
            .get(&id)
            .cloned()
            .unwrap_or_else(|| json!({}));

        monitor_features.insert(
            hwid2.clone(),
            DdcFeatureEntry {
                id: format!("{}#{}#{}", hwid[0], hwid[1], hwid[2]),
                hwid,
                features,
                feature_scan_timed_out: monitor_start.elapsed().as_millis() >= PER_MONITOR_TIMEOUT_MS,
                ddcci_supported: monitor.ddcci_supported,
                hl_brightness_supported: monitor.hl_brightness_supported,
                hl_contrast_supported: monitor.hl_contrast_supported,
                path: monitor.full_name.clone(),
                vcp_codes,
            },
        );
    }
    monitor_features
}

/// monitorReports 快照读取（features.rs 内部状态的只读视图）。
pub fn monitor_reports_snapshot() -> HashMap<String, Value> {
    let reports = crate::features::MONITOR_REPORTS.lock().unwrap();
    reports.as_ref().cloned().unwrap_or_default()
}

/// determineBrightnessVCPCode 等价（equiv L1563-L1600）。
pub fn determine_brightness_vcp_code(
    monitor: &str,
    ddc_brightness_vcps: &Value,
    check_vcp_wait_ms: u64,
) -> Option<u8> {
    crate::features::determine_brightness_vcp_code(monitor, ddc_brightness_vcps, check_vcp_wait_ms)
}
