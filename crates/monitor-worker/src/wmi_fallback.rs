//! WMI 内部面板枚举/亮度读数 —— 桥优先 + WMIC 回退链。
//!
//! equiv: Monitors.js:L1216-L1294（canUseWmiBridgeNow / canUseInternalBrightness /
//! hasInternalPanel / noteBridgeFailure / getMonitorsInternal / getBrightnessInternal）、
//! L2005-L2149（getWMICPath / wmicExists / getWMIC / getMonitorsWMIC / getBrightnessWMIC）、
//! L1183-L1211（getHDRDisplays）。
//!
//! §3.2 高危区：WMIC 在 Win11 部分版本仍存在，必须保留 wmicExists() 检测与
//! BRIDGE_FAILURE_LIMIT=3 的失败计数切换，不得删除 WMIC 分支。

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::monitors::{parse_wmi_string, read_instance_name};
use crate::settings_state::{normalize_brightness, BRIDGE_FAILURE_LIMIT, INTERNAL_CONNECTORS};

/// wmiFailed（equiv L1296）。
pub static WMI_FAILED: AtomicBool = AtomicBool::new(false);
/// win32Failed（equiv L1345）。
pub static WIN32_FAILED: AtomicBool = AtomicBool::new(false);
/// wmicUnavailable（equiv L2005）。
pub static WMIC_UNAVAILABLE: AtomicBool = AtomicBool::new(false);
/// canUseWmiBridge（equiv L427，由 wmi-bridge-ok 消息置位）。
pub static CAN_USE_WMI_BRIDGE: AtomicBool = AtomicBool::new(false);
/// bridgeListingFailures（equiv L1249）。
pub static BRIDGE_LISTING_FAILURES: AtomicU32 = AtomicU32::new(0);
/// bridgeBrightnessFailures（equiv L1269）。
pub static BRIDGE_BRIGHTNESS_FAILURES: AtomicU32 = AtomicU32::new(0);
/// appleStudioUnavailable（equiv L1004，由 disableAppleStudio 覆写）。
pub static APPLE_STUDIO_UNAVAILABLE: AtomicBool = AtomicBool::new(false);

/// monitorsWin32 最近一次结果（equiv L419）。hasInternalPanel 与 hideClosedLid 依赖它。
static MONITORS_WIN32: OnceLock<Mutex<HashMap<String, Value>>> = OnceLock::new();

fn win32_state() -> &'static Mutex<HashMap<String, Value>> {
    MONITORS_WIN32.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 记录 win32 枚举结果（equiv getAllMonitors 中 monitorsWin32 = await getMonitorsWin32()）。
pub fn set_monitors_win32(monitors: &HashMap<String, Value>) {
    *win32_state().lock().unwrap() = monitors.clone();
}

/// 读取 win32 枚举快照。
pub fn monitors_win32_snapshot() -> HashMap<String, Value> {
    win32_state().lock().unwrap().clone()
}

/// getWMICPath 等价（equiv L2009-L2012）。
pub fn get_wmic_path() -> String {
    let system_root = std::env::var("SystemRoot")
        .or_else(|_| std::env::var("windir"))
        .unwrap_or_else(|_| r"C:\Windows".to_string());
    format!(r"{}\System32\Wbem\WMIC.exe", system_root)
}

/// wmicExists 等价（equiv L2014-L2016）。
pub fn wmic_exists() -> bool {
    std::path::Path::new(&get_wmic_path()).exists()
}

/// 模块加载期 WMIC 探测（equiv L2046-L2051）。
pub fn check_wmic_availability() {
    if !wmic_exists() {
        println!("\x1b[41mWARNING: WMIC unavailable! Using WMI Bridge instead.\x1b[0m");
        WMIC_UNAVAILABLE.store(true, Ordering::SeqCst);
    }
}

/// canUseWmiBridgeNow 等价（equiv L1216-L1218）。
pub fn can_use_wmi_bridge_now() -> bool {
    CAN_USE_WMI_BRIDGE.load(Ordering::SeqCst) && !WMI_FAILED.load(Ordering::SeqCst)
}

/// canUseInternalBrightness 等价（equiv L1220-L1222）。
pub fn can_use_internal_brightness() -> bool {
    can_use_wmi_bridge_now() || !WMIC_UNAVAILABLE.load(Ordering::SeqCst)
}

/// INTERNAL_CONNECTORS 匹配（equiv L1229-L1232）。
pub fn has_internal_panel() -> bool {
    let state = win32_state().lock().unwrap();
    state.values().any(|m| {
        m.get("connector")
            .and_then(|c| c.as_str())
            .map(|c| INTERNAL_CONNECTORS.contains(&c))
            .unwrap_or(false)
    })
}

/// noteBridgeFailure 等价（equiv L1239-L1244）：
/// 只在 WMIC 确实可接管时才放弃桥（wmiFailed 同时门控 setBrightness）。
fn note_bridge_failure(what: &str, failures: u32) {
    if failures >= BRIDGE_FAILURE_LIMIT
        && !WMIC_UNAVAILABLE.load(Ordering::SeqCst)
        && !WMI_FAILED.load(Ordering::SeqCst)
    {
        WMI_FAILED.store(true, Ordering::SeqCst);
        println!("{} failed {} times. Falling back to WMIC.", what, failures);
    }
}

/// WMIC CSV 输出：表头 + 数据行。
struct WmicTable {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl WmicTable {
    fn col<'a>(&self, row: &'a [String], name: &str) -> Option<&'a str> {
        let idx = self
            .headers
            .iter()
            .position(|h| h.eq_ignore_ascii_case(name))?;
        row.get(idx).map(|s| s.as_str())
    }
}

/// CSV 行切分（wmic /format:csv 语义：双引号包裹含逗号的字段）。
fn split_csv(row: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for ch in row.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    out.push(cur.trim().to_string());
    out
}

/// 执行 WMIC 查询并按表头解析（equiv wmi.query(...) 的对象化结果）。
fn run_wmic(class: &str, fields: &[&str]) -> Option<WmicTable> {
    use std::process::Command;
    let field_arg = fields.join(",");
    let output = Command::new(get_wmic_path())
        .args([
            "/namespace:\\\\root\\WMI",
            "path",
            class,
            "get",
            &field_arg,
            "/format:csv",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && l.contains(','));
    let headers = split_csv(lines.next()?);
    let rows = lines.map(split_csv).collect();
    Some(WmicTable { headers, rows })
}

/// readInstanceName + 下划线截断的组合（原版在多处重复的三行）。
fn instance_to_hwid(instance_name: &str) -> Option<Vec<String>> {
    let mut hwid = read_instance_name(Some(instance_name))?;
    if hwid.len() < 3 {
        return None;
    }
    let hwid2 = hwid[2].split('_').next().unwrap_or("").to_string();
    hwid[2] = hwid2;
    Some(hwid)
}

/// getMonitorsWMIC 等价（equiv L2054-L2097）。
/// None 表示 WMIC 不可用或查询失败（等价 resolve(false)）。
pub fn get_monitors_wmic() -> Option<HashMap<String, Value>> {
    if WMIC_UNAVAILABLE.load(Ordering::SeqCst) || !wmic_exists() {
        WMIC_UNAVAILABLE.store(true, Ordering::SeqCst);
        return None;
    }
    let table = run_wmic(
        "WmiMonitorID",
        &["InstanceName", "SerialNumberID", "UserFriendlyName"],
    )?;
    let mut found: HashMap<String, Value> = HashMap::new();
    for row in &table.rows {
        let instance_name = match table.col(row, "InstanceName") {
            Some(v) if !v.is_empty() => v,
            _ => continue,
        };
        let hwid = match instance_to_hwid(instance_name) {
            Some(h) => h,
            None => continue,
        };
        let hwid2 = hwid[2].clone();
        let serial = table
            .col(row, "SerialNumberID")
            .and_then(|v| parse_wmi_string(Some(v)));
        let friendly = table
            .col(row, "UserFriendlyName")
            .and_then(|v| parse_wmi_string(Some(v)));

        let mut wmi_info = json!({
            "id": format!("\\\\?\\{}#{}#{}", hwid[0], hwid[1], hwid[2]),
            "key": hwid2,
            "hwid": hwid,
            "serial": serial,
        });
        if let Some(name) = friendly.filter(|n| !n.is_empty()) {
            wmi_info["name"] = json!(name);
        }
        found.insert(hwid2, wmi_info);
    }
    Some(found)
}

/// getBrightnessWMIC 等价（equiv L2100-L2149）。
/// 外层 None = WMIC 不可用（resolve(false)）；内层 None = 无显示器可读。
pub fn get_brightness_wmic(remaps: &Value) -> Option<Option<Value>> {
    if WMIC_UNAVAILABLE.load(Ordering::SeqCst) || !wmic_exists() {
        WMIC_UNAVAILABLE.store(true, Ordering::SeqCst);
        return None;
    }
    let table = run_wmic("WmiMonitorBrightness", &["InstanceName", "CurrentBrightness"])?;
    for row in &table.rows {
        let instance_name = match table.col(row, "InstanceName") {
            Some(v) if !v.is_empty() => v,
            _ => continue,
        };
        let current: f64 = match table
            .col(row, "CurrentBrightness")
            .and_then(|v| v.parse::<f64>().ok())
        {
            Some(v) => v,
            None => continue,
        };
        let hwid = match instance_to_hwid(instance_name) {
            Some(h) => h,
            None => continue,
        };
        let mut wmi_info = json!({
            "id": format!("\\\\?\\{}#{}#{}", hwid[0], hwid[1], hwid[2]),
            "brightness": current,
            "hwid": hwid,
            "min": 0,
            "max": 100,
            "type": "wmi",
        });
        // Get normalization info（原注释）
        wmi_info = apply_remap(wmi_info, remaps);
        // Unnormalize brightness（原注释）
        let min = wmi_info.get("min").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let max = wmi_info.get("max").and_then(|v| v.as_i64()).unwrap_or(100) as i32;
        wmi_info["brightnessRaw"] = json!(current);
        wmi_info["brightness"] = json!(normalize_brightness(current, true, min, max));
        return Some(Some(wmi_info));
    }
    Some(None)
}

/// applyRemap 等价（equiv L1958-L1973）。
/// settings.remaps 按旧键（monitor.name）或新键（monitor.id）命中覆盖 min/max；
/// 命中新键时立即返回（原注释 Stop if using new scheme）。
pub fn apply_remap(mut monitor: Value, remaps: &Value) -> Value {
    if let Some(map) = remaps.as_object() {
        let name = monitor
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let id = monitor
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        for (remap_name, remap) in map {
            if !remap_name.is_empty() && (remap_name == &name || remap_name == &id) {
                if let Some(min) = remap.get("min") {
                    monitor["min"] = min.clone();
                }
                if let Some(max) = remap.get("max") {
                    monitor["max"] = max.clone();
                }
                if remap_name == &id {
                    return monitor;
                }
            }
        }
    }
    if monitor.get("min").map(|v| v.is_null()).unwrap_or(true) {
        monitor["min"] = json!(0);
    }
    if monitor.get("max").map(|v| v.is_null()).unwrap_or(true) {
        monitor["max"] = json!(100);
    }
    monitor
}

/// getMonitorsInternal 等价（equiv L1250-L1263）：
/// 桥优先；每次桥失败都立刻尝试 WMIC 回退，不必等到放弃桥。
pub fn get_monitors_internal() -> Option<HashMap<String, Value>> {
    if can_use_wmi_bridge_now() {
        if let Ok(monitors) = crate::monitors::get_monitors_wmi() {
            BRIDGE_LISTING_FAILURES.store(0, Ordering::SeqCst);
            return Some(monitors);
        }
        let failures = BRIDGE_LISTING_FAILURES.fetch_add(1, Ordering::SeqCst) + 1;
        note_bridge_failure("getMonitorsWMI()", failures);
    }
    if !WMIC_UNAVAILABLE.load(Ordering::SeqCst) {
        return get_monitors_wmic();
    }
    None
}

/// getBrightnessInternal 等价（equiv L1270-L1294）。
/// Ok(Some) 读到亮度；Ok(None) 无内屏可读（正常）；Err 桥彻底失败。
pub fn get_brightness_internal(remaps: &Value) -> Result<Option<Value>, ()> {
    if can_use_wmi_bridge_now() {
        match crate::monitors::get_brightness_wmi_full(remaps) {
            Ok(Some(info)) => {
                BRIDGE_BRIGHTNESS_FAILURES.store(0, Ordering::SeqCst);
                return Ok(Some(info));
            }
            // Nothing reports brightness through WMI (OEM-controlled or OLED panels,
            // desktops). Not a failure, and WMIC would query the same class.（原注释）
            Ok(None) => {
                BRIDGE_BRIGHTNESS_FAILURES.store(0, Ordering::SeqCst);
                return Ok(None);
            }
            Err(()) => {
                // No internal panel to read. Expected on desktops, so leave WMIC alone.（原注释）
                if !has_internal_panel() {
                    return Ok(None);
                }
                let failures = BRIDGE_BRIGHTNESS_FAILURES.fetch_add(1, Ordering::SeqCst) + 1;
                note_bridge_failure("getBrightnessWMI() for an internal panel", failures);
            }
        }
    }
    if !WMIC_UNAVAILABLE.load(Ordering::SeqCst) {
        return match get_brightness_wmic(remaps) {
            Some(result) => Ok(result),
            None => Err(()),
        };
    }
    Ok(None)
}

/// getHDRDisplays 等价（equiv L1183-L1211）：
/// displayconfig::hdr 结果按 hwid[2] 合并进 monitors，附带 sdrNits/sdrLevel/hdr 状态。
pub fn get_hdr_displays(monitors: &mut HashMap<String, Value>) {
    let displays = displayconfig::hdr::get_displays();
    for (_key, display) in displays {
        let hwid: Vec<String> = display.path.split('#').map(|s| s.to_string()).collect();
        let key = hwid.get(2).cloned().unwrap_or_default();
        let mut new_display = json!({
            "key": key,
            "id": display.path,
            "hwid": hwid,
            "sdrNits": display.nits,
            "sdrLevel": (display.nits - 80) / 4,
            "hdr": if display.hdr_active {
                "active"
            } else if display.hdr_enabled {
                "supported"
            } else {
                "unsupported"
            },
        });
        if !display.name.is_empty() {
            new_display["name"] = json!(display.name);
        }
        crate::monitors::update_display(monitors, &key, new_display);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wmic_path_uses_system_root() {
        assert!(get_wmic_path().ends_with(r"\System32\Wbem\WMIC.exe"));
    }

    #[test]
    fn csv_split_handles_quotes() {
        assert_eq!(split_csv("a,b,c"), vec!["a", "b", "c"]);
        assert_eq!(split_csv(r#""a,1",b"#), vec!["a,1", "b"]);
        assert_eq!(split_csv(" a , b "), vec!["a", "b"]);
    }

    #[test]
    fn apply_remap_prefers_id_and_stops() {
        let monitor = json!({ "id": "A#B#C", "name": "M", "min": 0, "max": 100 });
        let remaps = json!({
            "M": { "min": 10, "max": 90 },
            "A#B#C": { "min": 20, "max": 80 }
        });
        let out = apply_remap(monitor, &remaps);
        // name 命中先写入，随后 id 命中覆盖并立即返回（原版 Stop if using new scheme）
        assert_eq!(out["min"], json!(20));
        assert_eq!(out["max"], json!(80));
    }

    #[test]
    fn apply_remap_fills_defaults() {
        let out = apply_remap(json!({ "id": "X" }), &json!({}));
        assert_eq!(out["min"], json!(0));
        assert_eq!(out["max"], json!(100));
    }

    #[test]
    fn instance_to_hwid_truncates_suffix() {
        let hwid = instance_to_hwid(r"DISPLAY\CMN1521\5&2cf91c03&0&UID8388688_0").unwrap();
        assert_eq!(hwid[2], "5&2cf91c03&0&UID8388688");
    }
}
