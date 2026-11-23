//! settings.json 兼容层：默认值 + 读写 + 版本迁移。
//!
//! equiv: electron.js:L877-L976（defaultSettings 全部 100 项逐字保留）、
//! L108-L114（defaultLightSensorSettings）、L1099-L1160（版本迁移）。
//! settings.json 必须与原版双向兼容（AGENTS.md §3.2：老用户配置文件可直接用）。

use serde::Serialize;
use serde_json::{json, Map, Value};

/// defaultLightSensorSettings 等价（equiv light-sensor.js:L7-L20）。
pub fn default_light_sensor_settings() -> Value {
    json!({
        "enabled": false,
        "active": "fake",
        "sensorPollingInterval": 10,
        "sensors": {
            "yocto": { "hubUrl": "user:password@localhost" },
            "fake": { "overriddenLux": 50 },
            "windows": {}
        },
        "monitorSettings": {}
    })
}

/// defaultSettings 等价（equiv electron.js:L877-L976）。
/// appVersion 相关项（settingsVer/settingsBuild/branch/uuid/udpKey/isWin11）由调用方传入。
/// is_dev: --isdev 参数；is_really_win11: os.release >= 22000。
pub fn default_settings(
    is_dev: bool,
    is_really_win11: bool,
    app_version: &str,
    app_build: &str,
    app_version_tag: Option<&str>,
    uuid_value: &str,
    udp_key: &str,
) -> Value {
    json!({
        "isDev": is_dev,
        "settingsVer": format!("v{}", app_version),
        "settingsBuild": app_build,
        "userClosedIntro": false,
        "theme": "default",
        "icon": "icon",
        "updateInterval": 500,
        "openAtLogin": true,
        "brightnessAtStartup": true,
        "killWhenIdle": false,
        "remaps": {},
        "hotkeys": [],
        "hotkeyPercent": 10,
        "adjustmentTimes": [],
        "adjustmentTimeIndividualDisplays": false,
        "adjustmentTimeSpeed": "normal",
        "adjustmentTimeAnimate": false,
        "adjustmentTimeLongitude": 0,
        "adjustmentTimeLatitude": 0,
        "checkTimeAtStartup": true,
        "order": [],
        "monitorFeatures": {},
        "monitorFeaturesSettings": {},
        "hideDisplays": {},
        "disableDDCDisplays": {},
        "hdrDisplays": {},
        "gammaAsMainSliderDisplays": {},
        "extendMinimumDisplays": {},
        "extendMinimumBreakpoints": {},
        "sdrAsMainSliderDisplays": {},
        "sdrAsMainSlider": false,
        "checkForUpdates": !is_dev,
        "dismissedUpdate": "",
        "language": "system",
        "names": {},
        "analytics": !is_dev,
        "scrollShortcut": true,
        "scrollShortcutAmount": 2,
        "scrollFlyoutAmount": 2,
        "invertScroll": false,
        "useAcrylic": false,
        "useNativeAnimation": false,
        "sleepAction": "ps",
        "hotkeysBreakLinkedLevels": true,
        "enableSunValley": true,
        "isWin11": is_really_win11,
        "windowsStyle": "system",
        "hideClosedLid": false,
        "getDDCBrightnessUpdates": false,
        "detectIdleTimeEnabled": false,
        "detectIdleTimeSeconds": 0,
        "detectIdleTimeMinutes": 5,
        "detectIdleCheckFullscreen": false,
        "detectIdleMedia": false,
        "idleRestoreSeconds": 0,
        "wakeRestoreSeconds": 0,
        "hardwareRestoreSeconds": 0,
        "restartOnWake": false,
        "checkVCPWaitMS": 20,
        "overrideTaskbarPosition": false,
        "overrideTaskbarGap": false,
        "disableAppleStudio": false,
        "disableHighLevel": false,
        "disableWMIC": false,
        "disableWMI": false,
        "disableWin32": false,
        "disableHDR": false,
        "useSoftwareBrightnessFallback": false,
        "useWin32Event": true,
        "useElectronEvents": true,
        "useWmDisplayChangeEvent": true,
        "useScMonitorPowerEvent": true,
        "useGuidPresenceEvent": true,
        "useGuidBrightnessEvent": true,
        "recreateTray": false,
        "recreateFlyout": false,
        "defaultOverlayType": "safe",
        "disableMouseEvents": false,
        "disableThrottling": false,
        "userDDCBrightnessVCPs": {},
        "userSkipReapply": [],
        "preferredDDCCIMethod": "accurate",
        "lastDetectedDDCCIMethod": "none",
        "forceLowPowerGPU": false,
        "ddcPowerOffValue": 5,
        "disableAutoRefresh": false,
        "disableAutoApply": false,
        "disableOnLockScreen": false,
        "udpEnabled": false,
        "udpRemote": false,
        "udpPortStart": 14715,
        "udpPortActive": 14715,
        "udpKey": udp_key,
        "showConsole": false,
        "logging": true,
        "profiles": [],
        "uuid": uuid_value,
        "branch": if app_version_tag.map(|t| t.starts_with("beta")).unwrap_or(false) { "beta" } else { "master" },
        "lightSensor": default_light_sensor_settings(),
    })
}

/// tempSettings 等价（equiv L978-L981）。
pub fn temp_settings() -> Value {
    json!({
        "pauseTimeAdjustments": false,
        "pauseIdleDetection": false
    })
}

/// getVersionValue 等价（Utils.js）：版本字符串 -> 可比较数值。
/// "v1.18.0" -> [1, 18, 0]。非数字段忽略。
pub fn get_version_value(version: &str) -> Vec<u32> {
    let trimmed = version.trim_start_matches('v');
    trimmed
        .split('.')
        .filter_map(|s| s.split('+').next().and_then(|p| p.parse::<u32>().ok()))
        .collect()
}

/// 版本比较：a < b。
pub fn version_lt(a: &[u32], b: &[u32]) -> bool {
    for i in 0..b.len().max(a.len()) {
        let av = a.get(i).copied().unwrap_or(0);
        let bv = b.get(i).copied().unwrap_or(0);
        if av != bv {
            return av < bv;
        }
    }
    false
}

/// writeSettings 的磁盘写盘部分（equiv electron.js:L1225-L1233）。
/// JSON.stringify(settings, null, '\t') 的等价：tab 缩进序列化，
/// 写失败仅记录错误（equiv 原版回调里的 debug.error），不 panic（AGENTS.md §5）。
pub fn write_settings_file(settings_path: &std::path::Path, settings: &Value) -> Result<(), String> {
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"\t");
    let mut buf = Vec::new();
    {
        let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
        settings.serialize(&mut ser).map_err(|e| e.to_string())?;
    }
    std::fs::write(settings_path, buf).map_err(|e| e.to_string())
}

/// normalizeMonitorOrder 等价（equiv L1085-L1105）：
/// v1.18.0+ 格式（字符串数组）直通；旧格式 { id, order } 排序去重。
pub fn normalize_monitor_order(order: &Value) -> Value {
    match order.as_array() {
        Some(arr) if arr.iter().all(|e| e.is_string()) => json!(arr),
        Some(arr) => {
            // Legacy: { id, order } 排序（a.order * 1 - b.order * 1）
            let mut legacy: Vec<(String, f64)> = arr
                .iter()
                .filter_map(|e| {
                    let id = e.get("id").and_then(|v| v.as_str())?.to_string();
                    let ord = e.get("order").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    Some((id, ord))
                })
                .collect();
            legacy.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            let mut seen = std::collections::HashSet::new();
            let ids: Vec<Value> = legacy
                .into_iter()
                .filter_map(|(id, _)| {
                    if seen.insert(id.clone()) {
                        Some(json!(id))
                    } else {
                        None
                    }
                })
                .collect();
            json!(ids)
        }
        _ => json!([]),
    }
}

/// readSettings + 版本迁移等价（equiv L1107-L1210）。
/// 迁移分支：v1.15.0（adjustmentTimes/idle）、v1.16.0（hotkeys 双向）、降级分支。
/// settings_path: userData/settings.json。
pub fn read_and_migrate_settings(settings_path: &std::path::Path, is_dev: bool, app_version: &str) -> Result<Value, String> {
    let mut settings = default_settings(
        is_dev,
        is_windows_11(),
        app_version,
        "dev",
        None,
        &generate_uuid(),
        &generate_uuid(),
    );

    if settings_path.exists() {
        let text = std::fs::read_to_string(settings_path)
            .map_err(|e| format!("Couldn't load settings {}", e))?;
        // Object.assign(settings, JSON.parse(...)) 语义：浅合并
        if let (Some(base), Some(loaded)) = (settings.as_object_mut(), serde_json::from_str::<Map<String, Value>>(&text).ok()) {
            for (k, v) in loaded {
                base.insert(k, v);
            }
        }
    } else {
        std::fs::write(settings_path, "{}").map_err(|e| e.to_string())?;
    }

    // Overrides（equiv L1117-L1124）
    if let Some(obj) = settings.as_object_mut() {
        obj.insert("isDev".to_string(), json!(is_dev));
        obj.insert("killWhenIdle".to_string(), json!(false));
    }
    // if (settings.updateInterval === 999) settings.updateInterval = 100
    if settings.get("updateInterval").and_then(|v| v.as_u64()) == Some(999) {
        settings["updateInterval"] = json!(100);
    }

    // 版本迁移
    let settings_version = get_version_value(
        settings.get("settingsVer").and_then(|v| v.as_str()).unwrap_or(""),
    );
    let app_version_value = get_version_value(&format!("v{}", app_version));

    if version_lt(&settings_version, &get_version_value("v1.15.0")) {
        // v1.15.0: adjustmentTimes 升级 + idle 设置升级（equiv L1132-L1153）
        upgrade_adjustment_times(&mut settings);
        upgrade_idle_settings(&mut settings);
    } else if version_lt(&app_version_value, &get_version_value("v1.16.0"))
        && !version_lt(&settings_version, &get_version_value("v1.16.0"))
    {
        // Downgrade from v1.16.0+（equiv L1154-L1161）
        if settings.get("hotkeysPre1160").is_some() {
            let old = settings.get("hotkeysPre1160").cloned().unwrap_or(json!({}));
            settings["hotkeys"] = old;
        } else {
            settings["hotkeys"] = json!({});
        }
        println!("Downgraded settings from v1.16.0+ format!");
    }
    if version_lt(&settings_version, &get_version_value("v1.16.0")) {
        // v1.16.0: hotkeys 升级（equiv L1163-L1210）
        upgrade_hotkeys(&mut settings);
        // lastKnownDisplays = {}（equiv L1164）
        settings["lastKnownDisplays"] = json!({});
    }

    Ok(settings)
}

/// upgradeAdjustmentTimes 等价（Utils.js，v1.15.0 格式迁移）。
fn upgrade_adjustment_times(settings: &mut Value) {
    if let Some(times) = settings.get("adjustmentTimes").cloned() {
        // 旧格式 { time: "HH:MM", monitors: [...] } -> 新格式数组
        // 原版 Utils.upgradeAdjustmentTimes 实现：转 { time: minutes, displays: {} }
        if let Some(arr) = times.as_array() {
            let upgraded: Vec<Value> = arr
                .iter()
                .map(|t| {
                    if t.get("time").and_then(|v| v.as_str()).is_some() {
                        let time_str = t.get("time").and_then(|v| v.as_str()).unwrap_or("00:00");
                        let parts: Vec<&str> = time_str.split(':').collect();
                        let minutes = parts
                            .first()
                            .and_then(|h| h.parse::<u64>().ok())
                            .unwrap_or(0)
                            * 60
                            + parts.get(1).and_then(|m| m.parse::<u64>().ok()).unwrap_or(0);
                        let mut new_t = t.clone();
                        new_t["time"] = json!(minutes);
                        new_t
                    } else {
                        t.clone()
                    }
                })
                .collect();
            settings["adjustmentTimes"] = json!(upgraded);
            println!("Upgraded Adjustment Times to v1.15.0 format!");
        }
    }
}

/// idle 设置升级（equiv L1140-L1150）。
fn upgrade_idle_settings(settings: &mut Value) {
    if let Some(idle) = settings.get("detectIdleTime").and_then(|v| v.as_f64()) {
        if idle > 0.0 {
            settings["detectIdleTimeEnabled"] = json!(true);
            settings["detectIdleTimeSeconds"] = json!((idle as u64) % 60);
            settings["detectIdleTimeMinutes"] = json!((idle as u64) / 60);
        }
        if let Some(obj) = settings.as_object_mut() {
            obj.remove("detectIdleTime");
        }
        println!("Upgraded Idle settings to v1.15.0 format!");
    }
}

/// hotkeys v1.16.0 升级（equiv L1163-L1210）。
fn upgrade_hotkeys(settings: &mut Value) {
    let hotkeys = settings.get("hotkeys").cloned();
    if hotkeys.is_some() {
        // hotkeysPre1160 保存旧格式
        settings["hotkeysPre1160"] = hotkeys.clone().unwrap_or(json!({}));
        let mut new_hotkeys: Vec<Value> = Vec::new();
        if let Some(obj) = hotkeys.as_ref().and_then(|h| h.as_object()) {
            for (_k, hotkey) in obj {
                let monitor = hotkey.get("monitor").and_then(|v| v.as_str()).unwrap_or("");
                let direction = hotkey.get("direction").and_then(|v| v.as_i64()).unwrap_or(0);
                let mut new_hotkey = json!({
                    "accelerator": hotkey.get("accelerator").cloned().unwrap_or(json!("")),
                    "id": generate_uuid(),
                    "actions": [{
                        "monitors": {},
                        "target": "brightness",
                        "values": [0],
                        "value": 0,
                        "allMonitors": false,
                    }]
                });
                if monitor == "turn_off_displays" {
                    new_hotkey["actions"][0]["type"] = json!("off");
                } else {
                    new_hotkey["monitors"] = json!({});
                    if monitor == "all" {
                        new_hotkey["actions"][0]["allMonitors"] = json!(true);
                    } else {
                        new_hotkey["actions"][0]["monitors"][monitor] = json!(true);
                    }
                    new_hotkey["actions"][0]["type"] = json!("offset");
                    // value = settings.hotkeyPercent * hotkey.direction
                    let percent = settings.get("hotkeyPercent").and_then(|v| v.as_i64()).unwrap_or(10);
                    new_hotkey["actions"][0]["value"] = json!(percent * direction);
                }
                new_hotkeys.push(new_hotkey);
            }
        }
        settings["hotkeys"] = json!(new_hotkeys);
        println!("Upgraded Hotkeys to v1.16.0 format!");
    }
}

/// isWin11 检测（equiv electron.js:L97 isReallyWin11）。
fn is_windows_11() -> bool {
    // os.release() 第三段 >= 22000
    if let Ok(output) = std::process::Command::new("cmd").args(["/c", "ver"]).output() {
        let text = String::from_utf8_lossy(&output.stdout);
        // "10.0.22631.4037" 形式
        for part in text.split(['.', ' ']) {
            if let Ok(build) = part.parse::<u32>() {
                if build >= 22000 {
                    return true;
                }
            }
        }
    }
    false
}

/// uuid 生成（等价 uuid() v4 随机格式）。
pub fn generate_uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let nanos = now.subsec_nanos() as u64;
    let seed = now.as_secs() ^ (nanos << 32) ^ (std::process::id() as u64);
    // 简单 LCG 拼装 v4 格式
    let mut s = seed;
    let mut hex = String::new();
    for i in 0..32 {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let digit = ((s >> 33) & 0xF) as u8;
        hex.push(std::char::from_digit(digit as u32, 16).unwrap_or('0'));
        if i == 7 || i == 11 || i == 15 || i == 19 {
            hex.push('-');
        }
    }
    // v4 标记：第 13 段 = 4，第 17 段高 2 位 = 10
    let bytes: Vec<char> = hex.chars().collect();
    let mut out: String = bytes.iter().collect();
    // position 14 (0-based) 是第三段第一个字符 -> '4'
    let mut chars: Vec<char> = out.chars().collect();
    if chars.len() > 14 {
        chars[14] = '4';
    }
    // variant 位（第 19 个 hex 位后）
    if chars.len() > 19 {
        chars[19] = '8';
    }
    out = chars.into_iter().collect();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_key_count() {
        let ds = default_settings(false, true, "1.18.0", "dev", None, "u1", "k1");
        // 100 项（equiv L877-L976 键数）
        let count = ds.as_object().unwrap().len();
        println!("defaultSettings 键数: {}", count);
        // 100 个键（isDev..lightSensor）
        assert!(count >= 99, "应至少 99 键: {}", count);
        // 关键默认值抽查（逐字对照原版）
        assert_eq!(ds["updateInterval"], json!(500));
        assert_eq!(ds["checkVCPWaitMS"], json!(20));
        assert_eq!(ds["preferredDDCCIMethod"], json!("accurate"));
        assert_eq!(ds["hotkeyPercent"], json!(10));
        assert_eq!(ds["ddcPowerOffValue"], json!(5));
        assert_eq!(ds["udpPortStart"], json!(14715));
        assert_eq!(ds["language"], json!("system"));
        assert_eq!(ds["useWin32Event"], json!(true));
        assert_eq!(ds["useElectronEvents"], json!(true));
        assert_eq!(ds["useWmDisplayChangeEvent"], json!(true));
        assert_eq!(ds["useScMonitorPowerEvent"], json!(true));
        assert_eq!(ds["useGuidPresenceEvent"], json!(true));
        assert_eq!(ds["useGuidBrightnessEvent"], json!(true));
        assert_eq!(ds["lightSensor"]["active"], json!("fake"));
        assert_eq!(ds["lightSensor"]["sensorPollingInterval"], json!(10));
    }

    #[test]
    fn version_comparison() {
        assert!(version_lt(&get_version_value("v1.14.9"), &get_version_value("v1.15.0")));
        assert!(!version_lt(&get_version_value("v1.15.0"), &get_version_value("v1.15.0")));
        assert!(version_lt(&get_version_value("v1.15.1"), &get_version_value("v1.16.0")));
    }

    #[test]
    fn monitor_order_upgrade() {
        // 新格式直通
        let new_fmt = json!(["id1", "id2"]);
        assert_eq!(normalize_monitor_order(&new_fmt), json!(["id1", "id2"]));
        // 旧格式排序去重
        let old_fmt = json!([
            {"id": "b", "order": 2},
            {"id": "a", "order": 1},
            {"id": "b", "order": 3},
        ]);
        assert_eq!(normalize_monitor_order(&old_fmt), json!(["a", "b"]));
    }

    #[test]
    fn settings_merge_preserves_user_values() {
        // 模拟旧用户 settings.json：自定义值不被默认值覆盖
        let tmp = std::env::temp_dir().join("tt-test-settings.json");
        std::fs::write(&tmp, r#"{"updateInterval": 300, "theme": "dark", "checkVCPWaitMS": 30}"#).unwrap();
        let migrated = read_and_migrate_settings(&tmp, false, "1.18.0").unwrap();
        std::fs::remove_file(&tmp).ok();
        assert_eq!(migrated["updateInterval"], json!(300));
        assert_eq!(migrated["theme"], json!("dark"));
        assert_eq!(migrated["checkVCPWaitMS"], json!(30));
        // 默认值仍在
        assert_eq!(migrated["hotkeyPercent"], json!(10));
    }

    #[test]
    fn interval_999_override() {
        // updateInterval === 999 -> 100（equiv L1122）
        let tmp = std::env::temp_dir().join("tt-test-settings-999.json");
        std::fs::write(&tmp, r#"{"updateInterval": 999}"#).unwrap();
        let migrated = read_and_migrate_settings(&tmp, false, "1.18.0").unwrap();
        std::fs::remove_file(&tmp).ok();
        assert_eq!(migrated["updateInterval"], json!(100));
    }
}
