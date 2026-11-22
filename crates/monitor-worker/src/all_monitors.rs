//! getAllMonitors：WMI / Win32 / DDC / Studio / HDR / gamma / software 七段合并。
//!
//! equiv: Monitors.js:L770-L978（getAllMonitors 全文）。
//! §3.2 高危区：hwid 三源对齐（WMI/Win32/DDC 标识合并）——错位会导致控制错显示器，
//! 因此合并顺序、updateDisplay 的 Object.assign 语义、以及最后的名字/num 修复逐段保留。

use serde_json::{json, Value};
use std::collections::HashMap;

use crate::features::{check_vcp, check_vcp_if_enabled, determine_brightness_vcp_code};
use crate::monitors::{get_features_ddc, get_monitors_win32, update_display, DdcFeatureEntry};
use crate::settings_state::{normalize_brightness, vcp_str, WorkerSettings, SOFTWARE_BRIGHTNESS_MIN};
use crate::wmi_fallback::{
    apply_remap, can_use_internal_brightness, get_brightness_internal, get_hdr_displays,
    get_monitors_internal, monitors_win32_snapshot, set_monitors_win32, APPLE_STUDIO_UNAVAILABLE,
    WIN32_FAILED,
};

/// worker 的显示器模型。
pub type MonitorMap = HashMap<String, Value>;

/// 显示器枚举的全局时间戳（供日志用，等价原版 process.hrtime 计时）。
struct Timer(std::time::Instant);

impl Timer {
    fn new() -> Self {
        Timer(std::time::Instant::now())
    }
    /// 原版 `(startTime - process.hrtime.bigint()) / BigInt(-1000000)` 的等价耗时毫秒。
    fn elapsed_ms(&self) -> u128 {
        self.0.elapsed().as_millis()
    }
}

/// getNameFallback：`${localization.GENERIC_DISPLAY_SINGLE} ${idx + 1}`
fn localized_display_name(generic: &str, idx: usize) -> String {
    if generic.is_empty() {
        format!("Display {}", idx + 1)
    } else {
        format!("{} {}", generic, idx + 1)
    }
}

/// getAllMonitors 等价（equiv L770-L978）。
/// generic_display_name: localization.GENERIC_DISPLAY_SINGLE（空串时退回 "Display"）。
pub fn get_all_monitors(
    settings: &WorkerSettings,
    ddcci_method: &str,
    core_only: bool,
    generic_display_name: &str,
    ddc_brightness_vcps: &Value,
) -> MonitorMap {
    let mut found_monitors: MonitorMap = HashMap::new();
    let full_start = Timer::new();

    let blocked_keys = settings.ddc_blocked_keys();
    let check_vcp_wait_ms = settings
        .get("checkVCPWaitMS")
        .and_then(|v| v.as_u64())
        .unwrap_or(20);
    let unstable_ddc = crate::sentinel_current_unstable();
    let mon_features = settings.get("monitorFeatures").cloned();
    let disable_high_level = settings.disable_high_level();
    let force_max = settings.debug_force_brightness_max();

    // ---- 1) List via WMI (native bridge preferred, WMIC fallback)（equiv L776-L789）
    if can_use_internal_brightness() {
        let timer = Timer::new();
        match get_monitors_internal() {
            Some(monitors_wmi) => {
                for (hwid2, monitor) in monitors_wmi {
                    update_display(&mut found_monitors, &hwid2, monitor);
                }
                println!("getMonitorsInternal() Total: {}ms", timer.elapsed_ms());
            }
            None => println!("\x1b[41mgetMonitorsInternal() failed!\x1b[0m"),
        }
    } else {
        println!("getMonitorsInternal() skipped. No WMI method available.");
    }

    // ---- 2) List via Win32 (more details)（equiv L792-L807）
    if !WIN32_FAILED.load(std::sync::atomic::Ordering::SeqCst) {
        let timer = Timer::new();
        match get_monitors_win32() {
            Ok(monitors_win32) => {
                // monitorsWin32 保存供 hasInternalPanel / hideClosedLid 使用
                set_monitors_win32(&monitors_win32);
                for (hwid2, monitor) in &monitors_win32 {
                    update_display(&mut found_monitors, hwid2, monitor.clone());
                }
                println!("getMonitorsWin32() Total: {}ms", timer.elapsed_ms());
            }
            Err(e) => println!("\x1b[41mgetMonitorsWin32() failed!\x1b[0m {}", e),
        }
    } else {
        println!("getMonitorsWin32() skipped due to previous failure.");
    }

    // ---- 3) List Apple Studio displays（equiv L810-L820）
    if !APPLE_STUDIO_UNAVAILABLE.load(std::sync::atomic::Ordering::SeqCst) {
        let timer = Timer::new();
        crate::studio_display_discovery::get_studio_display(&mut found_monitors);
        println!("getStudioDisplay() Total: {}ms", timer.elapsed_ms());
    } else {
        println!("getStudioDisplay() skipped due to previous failure.");
    }

    // ---- 4) DDC/CI Brightness + Features（equiv L823-L894）
    let timer = Timer::new();
    let features_list: HashMap<String, DdcFeatureEntry> = get_features_ddc(
        ddcci_method,
        core_only,
        disable_high_level,
        &blocked_keys,
        &unstable_ddc,
        mon_features.as_ref(),
        check_vcp_wait_ms,
        force_max,
    );

    for (hwid2, monitor) in &features_list {
        let can_use_high_level_brightness =
            !disable_high_level && monitor.hl_brightness_supported;
        let is_blocked = blocked_keys.iter().any(|k| monitor.id.contains(k.as_str()));

        // A timed-out DDC scan must not trigger another DDC probe. Use the
        // standard luminance VCP as the high-level API's placeholder so
        // downstream feature and display-type handling remains valid.
        // A DDC-blocked display must not be probed for one either.（原注释）
        let brightness_type: Value = if is_blocked {
            Value::Bool(false)
        } else if monitor.feature_scan_timed_out {
            if can_use_high_level_brightness {
                json!(0x10)
            } else {
                Value::Bool(false)
            }
        } else {
            let hwid1 = monitor.hwid.get(1).cloned().unwrap_or_default();
            let has_custom = ddc_brightness_vcps
                .get(&hwid1)
                .map(|v| !v.is_null())
                .unwrap_or(false);
            if monitor.ddcci_supported || has_custom {
                match determine_brightness_vcp_code(&monitor.id, ddc_brightness_vcps, check_vcp_wait_ms) {
                    Some(code) => json!(code),
                    None => Value::Bool(false),
                }
            } else if can_use_high_level_brightness {
                // DDC/CI validation already failed for this display (e.g. an
                // internal panel), so probing brightness VCPs only produces
                // device errors. Fall back to the high-level placeholder.（原注释）
                json!(0x10)
            } else {
                Value::Bool(false)
            }
        };

        let features = monitor.features.clone();
        // brightnessType 真假判定（JS 真值语义：非 0 数字 / 非 false）
        let brightness_truthy = brightness_type
            .as_u64()
            .map(|v| v > 0)
            .unwrap_or(false);

        let brightness_values_initial = if brightness_truthy {
            features
                .get(vcp_str(brightness_type.as_u64().unwrap_or(0) as i32))
                .cloned()
                .filter(|v| v.is_array())
                .unwrap_or_else(|| json!([50, 100]))
        } else {
            json!([50, 100])
        };

        let mut ddcci_info = json!({
            "id": monitor.id,
            "key": hwid2,
            "hwid": monitor.hwid,
            "path": monitor.path,
            "ddcciSupported": monitor.ddcci_supported,
            "highLevelSupported": {
                "brightness": monitor.hl_brightness_supported,
                "contrast": monitor.hl_contrast_supported,
            },
            "features": features,
            "vcpCodes": monitor.vcp_codes,
            "featuresPending": core_only,
            "type": if (monitor.ddcci_supported || monitor.hl_brightness_supported) && brightness_truthy {
                "ddcci"
            } else {
                "none"
            },
            "min": 0,
            "max": 100,
            "brightnessType": brightness_type,
            "brightnessValues": brightness_values_initial,
        });

        // if(canUseHighLevelBrightness && !(brightnessType > 0x10))（equiv L865-L874）
        let brightness_code = brightness_type.as_u64().unwrap_or(0);
        let brightness: Value = if can_use_high_level_brightness && brightness_code <= 0x10 {
            crate::brightness_ops::get_high_level_brightness(settings, &monitor.id)
        } else {
            if is_blocked {
                Value::Bool(false)
            } else {
                check_vcp(
                    &monitor.id,
                    brightness_code as i64,
                    false,
                    true,
                    check_vcp_wait_ms,
                    force_max,
                )
            }
        };

        // Force lower max brightness for testing（原注释）
        let mut brightness = brightness;
        if let Some(arr) = brightness.as_array().cloned() {
            if arr.get(1).map(|v| !v.is_null()).unwrap_or(false) {
                if let Some(force) = force_max {
                    let mut arr = arr;
                    arr[1] = json!(force);
                    brightness = json!(arr);
                }
            }
        }

        if brightness.is_array() {
            if let Some(obj) = ddcci_info.as_object_mut() {
                obj.insert("brightnessValues".to_string(), brightness.clone());
            }
            // features[vcpStr(brightnessType)] = brightness（原注释）
            if let Some(f) = ddcci_info.get_mut("features").and_then(|v| v.as_object_mut()) {
                f.insert(vcp_str(brightness_code as i32), brightness.clone());
            }
        }

        let arr = ddcci_info
            .get("brightnessValues")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let raw = arr.first().cloned().unwrap_or(json!(50));
        let max = arr.get(1).cloned().unwrap_or(json!(100));
        ddcci_info["brightnessRaw"] = raw;
        ddcci_info["brightnessMax"] = max;

        // Get normalization info（原注释）
        let remaps = settings.get("remaps").cloned().unwrap_or(json!({}));
        ddcci_info = apply_remap(ddcci_info, &remaps);

        // Unnormalize brightness（原注释）
        let min = ddcci_info.get("min").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let max_range = ddcci_info.get("max").and_then(|v| v.as_i64()).unwrap_or(100) as i32;
        let raw_f = ddcci_info
            .get("brightnessRaw")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        ddcci_info["brightness"] = json!(normalize_brightness(raw_f, true, min, max_range));

        update_display(&mut found_monitors, hwid2, ddcci_info);
    }
    println!("getFeaturesDDC() Total: {}ms", timer.elapsed_ms());

    // ---- 5) Re-apply Studio Display control after DDC/CI（equiv L897-L905）
    if !APPLE_STUDIO_UNAVAILABLE.load(std::sync::atomic::Ordering::SeqCst) {
        let timer = Timer::new();
        crate::studio_display_discovery::get_studio_display(&mut found_monitors);
        println!("getStudioDisplay() Total: {}ms", timer.elapsed_ms());
    }

    // ---- 6) Internal display brightness（equiv L908-L927）
    if can_use_internal_brightness() {
        let timer = Timer::new();
        let remaps = settings.get("remaps").cloned().unwrap_or(json!({}));
        match get_brightness_internal(&remaps) {
            Ok(Some(wmi_brightness)) => {
                println!("getBrightnessInternal() Total: {}ms", timer.elapsed_ms());
                if let Some(hwid2) = wmi_brightness
                    .get("hwid")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.get(2))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                {
                    update_display(&mut found_monitors, &hwid2, wmi_brightness);
                    // If Win32 doesn't find the internal display, hide it.（原注释）
                    if settings.hide_closed_lid()
                        && !monitors_win32_snapshot().contains_key(&hwid2)
                    {
                        update_display(&mut found_monitors, &hwid2, json!({ "type": "none" }));
                    }
                }
            }
            Ok(None) => {}
            Err(()) => println!("\x1b[41mgetBrightnessInternal() failed!\x1b[0m"),
        }
    } else {
        println!("getBrightnessInternal() skipped. No WMI method available.");
    }

    // ---- 7) HDR（equiv L930-L938）
    if !settings.disable_hdr() {
        let timer = Timer::new();
        get_hdr_displays(&mut found_monitors);
        println!("getHDRDisplays() Total: {}ms", timer.elapsed_ms());
    }

    // ---- 8) Gamma（equiv L941-L949）
    if crate::software_brightness::gamma_features_active(&settings.raw) {
        let timer = Timer::new();
        crate::software_brightness::read_gamma_brightness(&mut found_monitors, &settings.raw);
        println!("readGammaBrightness() Total: {}ms", timer.elapsed_ms());
    }

    // ---- 9) Software brightness fallback（equiv L951）
    crate::software_brightness::apply_software_brightness(&mut found_monitors, &settings.raw);

    // ---- 10) Hide internal（equiv L954-L959）
    if settings.hide_closed_lid() {
        let wmi_key = found_monitors.iter().find_map(|(k, m)| {
            if m.get("type").and_then(|v| v.as_str()) == Some("wmi") {
                Some(k.clone())
            } else {
                None
            }
        });
        if let Some(key) = wmi_key {
            let hwid2 = found_monitors
                .get(&key)
                .and_then(|m| m.get("hwid"))
                .and_then(|v| v.as_array())
                .and_then(|a| a.get(2))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !monitors_win32_snapshot().contains_key(&hwid2) {
                update_display(&mut found_monitors, &hwid2, json!({ "type": "none" }));
            }
        }
    }

    // ---- 11) Finally, fix names/num（equiv L962-L971）
    // 原版按 Object 的插入顺序编号；Rust HashMap 无序，改按 key 排序保证确定性，
    // 编号语义（0..n 唯一且连续）不变。
    let mut keys: Vec<String> = found_monitors.keys().cloned().collect();
    keys.sort();
    for (idx, key) in keys.iter().enumerate() {
        if let Some(entry) = found_monitors.get_mut(key) {
            let name_empty = entry
                .get("name")
                .map(|v| v.is_null() || v.as_str().map(|s| s.is_empty()).unwrap_or(false))
                .unwrap_or(true);
            if name_empty {
                entry["name"] = json!(localized_display_name(generic_display_name, idx));
            }
            entry["num"] = json!(idx);
        }
    }

    println!("getAllMonitors() total: {}ms", full_start.elapsed_ms());
    println!(
        "Monitors found: {:?}",
        found_monitors.keys().collect::<Vec<_>>()
    );
    found_monitors
}

/// checkVCPIfEnabled 的便捷封装（供 features 内部与外部一致调用）。
#[allow(dead_code)]
fn check_vcp_if_enabled_with_settings(
    settings: &WorkerSettings,
    monitor: &str,
    code: i64,
    skip_cache: bool,
) -> Value {
    let wait = settings
        .get("checkVCPWaitMS")
        .and_then(|v| v.as_u64())
        .unwrap_or(20);
    check_vcp_if_enabled(
        monitor,
        code,
        skip_cache,
        settings.get("monitorFeatures"),
        wait,
        settings.debug_force_brightness_max(),
    )
}

/// SOFTWARE_BRIGHTNESS_MIN 供外部模块引用（保持常量单点定义）。
#[allow(dead_code)]
const _SOFTWARE_MIN: i32 = SOFTWARE_BRIGHTNESS_MIN;
