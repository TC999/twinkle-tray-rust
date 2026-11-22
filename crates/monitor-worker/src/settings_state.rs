//! worker 侧 settings 状态与方法判定。
//!
//! equiv: Monitors.js 的 settings 消费点（L427-L520, L1005-L1010, L2023-L2053）。
//! 全部时序/数值常量逐字保留（AGENTS.md §3.2）。

use serde_json::Value;

/// 软件亮度下限（equiv L18 SOFTWARE_BRIGHTNESS_MIN = 20）。真机调校值，不得改动。
pub const SOFTWARE_BRIGHTNESS_MIN: i32 = 20;

/// 内部面板连接器类型集合（equiv L1229 INTERNAL_CONNECTORS）。
pub const INTERNAL_CONNECTORS: [&str; 5] = [
    "internal",
    "displayport_embedded",
    "udi_embedded",
    "ldvs",
    "lvds",
];

/// WMI 桥失败上限（equiv L1238 BRIDGE_FAILURE_LIMIT = 3）。
pub const BRIDGE_FAILURE_LIMIT: u32 = 3;

/// Studio Display USB 常量（equiv L1005-L1010）。逐字保留。
pub const STUDIO_DISPLAY_VENDOR_ID: u16 = 0x05ac;
pub const STUDIO_DISPLAY_PRODUCT_IDS: [u16; 1] = [0x1118];
pub const STUDIO_DISPLAY_BRIGHTNESS_INTERFACE: u32 = 0x7;
pub const STUDIO_DISPLAY_REPORT_ID: u8 = 0x01;
pub const STUDIO_DISPLAY_BRIGHTNESS_MIN: f64 = 400.0;
pub const STUDIO_DISPLAY_BRIGHTNESS_MAX: f64 = 60000.0;

/// percentToStudioDisplayNits 等价（equiv L1012-L1015）。
pub fn percent_to_studio_display_nits(percent: f64) -> f64 {
    STUDIO_DISPLAY_BRIGHTNESS_MIN
        + ((STUDIO_DISPLAY_BRIGHTNESS_MAX - STUDIO_DISPLAY_BRIGHTNESS_MIN) * (percent / 100.0))
}

/// studioDisplayNitsToPercent 等价（equiv L1016-L1019）。
pub fn studio_display_nits_to_percent(nits: f64) -> f64 {
    ((nits - STUDIO_DISPLAY_BRIGHTNESS_MIN)
        / (STUDIO_DISPLAY_BRIGHTNESS_MAX - STUDIO_DISPLAY_BRIGHTNESS_MIN))
        * 100.0
}

/// worker 的 settings 快照（等价 handleMonitorMessage settings 分支的全局 settings）。
#[derive(Debug, Clone, Default)]
pub struct WorkerSettings {
    pub raw: Value,
}

impl WorkerSettings {
    pub fn from_value(v: Value) -> Self {
        Self { raw: v }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.raw.get(key)
    }

    /// disableDDCDisplays 的启用键集合（equiv L436-L445 getDDCBlockedKeys）。
    pub fn ddc_blocked_keys(&self) -> Vec<String> {
        let disable = self.raw.get("disableDDCDisplays");
        match disable {
            Some(Value::Object(map)) => map
                .iter()
                .filter(|(_, v)| v.as_bool() == Some(true))
                .map(|(k, _)| k.clone())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// isDDCBlocked 等价（equiv L444-L452）：实例 ID 出现在任何 id 形式内即算命中。
    /// 接受 id 字符串或 hwid 数组（用 # 连接）。
    pub fn is_ddc_blocked(&self, monitor: &Value) -> bool {
        let monitor_string = match monitor {
            Value::String(s) => s.clone(),
            Value::Array(arr) => arr
                .iter()
                .map(|v| v.as_str().unwrap_or("").to_string())
                .collect::<Vec<_>>()
                .join("#"),
            Value::Object(obj) => {
                if let Some(id) = obj.get("id").and_then(|v| v.as_str()) {
                    id.to_string()
                } else if let Some(hwid) = obj.get("hwid").and_then(|v| v.as_array()) {
                    hwid
                        .iter()
                        .map(|v| v.as_str().unwrap_or("").to_string())
                        .collect::<Vec<_>>()
                        .join("#")
                } else {
                    String::new()
                }
            }
            _ => String::new(),
        };
        if monitor_string.is_empty() {
            return false;
        }
        self.ddc_blocked_keys()
            .iter()
            .any(|key| monitor_string.contains(key.as_str()))
    }

    /// determineDDCCIMethod 等价（equiv L980-L995 + L2023-L2046）。
    /// is_fast_fine: 快速校验结果；unstable_downgraded: 崩溃降级标志。
    pub fn determine_ddcci_method(&self, is_fast_fine: bool, unstable_downgraded: bool) -> String {
        let mut method = if is_fast_fine { "fast" } else { "accurate" }.to_string();

        let saved = self.raw.get("preferredDDCCIMethod").and_then(|v| v.as_str());
        let valid_values = ["fast", "accurate", "no-validation", "legacy"];
        if let Some(saved) = saved {
            if valid_values.contains(&saved) {
                // An explicit user preference wins over the crash downgrade.（原注释）
                method = saved.to_string();
            } else if unstable_downgraded && method == "accurate" {
                println!("Using \"fast\" DDC/CI validation due to crash evidence from a previous session.");
                method = "fast".to_string();
            }
        } else if unstable_downgraded && method == "accurate" {
            println!("Using \"fast\" DDC/CI validation due to crash evidence from a previous session.");
            method = "fast".to_string();
        }
        method
    }

    /// shouldEnrichCapabilities 等价（equiv L995-L1003）。
    pub fn should_enrich_capabilities(&self, unstable_downgraded: bool) -> bool {
        match self.raw.get("preferredDDCCIMethod").and_then(|v| v.as_str()) {
            Some("accurate") => true,
            // Auto keeps its crash-recovery behavior: don't retry a capabilities read
            // after a worker died while probing DDC/CI.（原注释逐字保留）
            Some("auto") => !unstable_downgraded,
            _ => false,
        }
    }

    /// disableHighLevel（多处使用）。
    pub fn disable_high_level(&self) -> bool {
        self.raw.get("disableHighLevel").and_then(|v| v.as_bool()).unwrap_or(false)
    }

    /// hideClosedLid。
    pub fn hide_closed_lid(&self) -> bool {
        self.raw.get("hideClosedLid").and_then(|v| v.as_bool()).unwrap_or(false)
    }

    /// getDDCBrightnessUpdates。
    pub fn get_ddc_brightness_updates(&self) -> bool {
        self.raw.get("getDDCBrightnessUpdates").and_then(|v| v.as_bool()).unwrap_or(false)
    }

    /// disableHDR。
    pub fn disable_hdr(&self) -> bool {
        self.raw.get("disableHDR").and_then(|v| v.as_bool()).unwrap_or(false)
    }

    /// disableSoftwareBrightnessFallback 等价（useSoftwareBrightnessFallback 反相）。
    pub fn use_software_brightness_fallback(&self) -> bool {
        self.raw.get("useSoftwareBrightnessFallback").and_then(|v| v.as_bool()).unwrap_or(false)
    }

    /// debugForceBrightnessMax。
    pub fn debug_force_brightness_max(&self) -> Option<u32> {
        self.raw.get("debugForceBrightnessMax").and_then(|v| v.as_u64()).map(|v| v as u32)
    }
}

/// normalizeBrightness 等价（equiv L1930-L1956）。
/// unnormalize=true 时把 0..100 归一值映射到 min..max。
pub fn normalize_brightness(brightness: f64, unnormalize: bool, min: i32, max: i32) -> f64 {
    if min == max {
        return min as f64;
    }
    if unnormalize {
        let b = (brightness as f64 / 100.0) * (max - min) as f64 + min as f64;
        b.round().clamp(min as f64, max as f64)
    } else {
        (((brightness - min as f64) / (max - min) as f64) * 100.0).round().clamp(0.0, 100.0)
    }
}

/// vcpStr 等价（equiv L2159-L2164）：0x10 -> "0x10"。
pub fn vcp_str(code: i32) -> String {
    format!("0x{:02x}", code)
}
