//! IPC 消息类型（行协议 JSON）。
//!
//! equiv: Monitors.js handleMonitorMessage（L175-L305）+ 各 process.send 调用点。
//! 字段名与原版 JSON 逐字对应（serde 默认 camelCase 字段名）。

use serde::{Deserialize, Serialize};

/// 主进程 -> worker 的消息（equiv handleMonitorMessage 的 data.type 分支）。
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum IncomingMessage {
    #[serde(rename = "refreshMonitors")]
    RefreshMonitors {
        #[serde(default)]
        full_refresh: bool,
        #[serde(default)]
        clear_cache: bool,
        #[serde(default)]
        ddcci_type: Option<String>,
        #[serde(default)]
        generation: Option<u64>,
    },
    #[serde(rename = "readKnownBrightness")]
    ReadKnownBrightness { #[serde(default)] request_id: Option<u64> },
    #[serde(rename = "enrichMonitorCapabilities")]
    EnrichMonitorCapabilities { #[serde(default)] generation: Option<u64> },
    #[serde(rename = "brightness")]
    Brightness { brightness: f64, #[serde(default)] id: Option<String> },
    #[serde(rename = "sdr")]
    Sdr { brightness: f64, #[serde(default)] id: Option<String> },
    #[serde(rename = "gamma")]
    Gamma { brightness: f64, #[serde(default)] id: Option<String> },
    #[serde(rename = "settings")]
    Settings { #[serde(default)] settings: Option<serde_json::Value> },
    #[serde(rename = "ddcBrightnessVCPs")]
    DdcBrightnessVCPs { #[serde(default)] ddc_brightness_vcps: Option<serde_json::Value> },
    #[serde(rename = "localization")]
    Localization { localization: LocalizationPayload },
    #[serde(rename = "vcp")]
    Vcp { monitor: String, code: u8, value: u32 },
    #[serde(rename = "flushvcp")]
    FlushVcp,
    #[serde(rename = "wmi-bridge-ok")]
    WmiBridgeOk { value: bool },
    #[serde(rename = "getVCP")]
    GetVcp { monitor: String, code: u8 },
    #[serde(rename = "getReport")]
    GetReport,
}

/// localization 消息负载（worker 只需要 GENERIC_DISPLAY_SINGLE）。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LocalizationPayload {
    #[serde(rename = "GENERIC_DISPLAY_SINGLE")]
    pub generic_display_single: String,
}

/// worker -> 主进程的消息（equiv 各 process.send）。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum OutgoingMessage {
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "refreshMonitors")]
    RefreshMonitors {
        monitors: serde_json::Value,
        generation: Option<u64>,
        can_enrich_capabilities: bool,
    },
    #[serde(rename = "knownBrightness")]
    KnownBrightness { brightness: serde_json::Value, request_id: Option<u64> },
    #[serde(rename = "monitorsEnriched")]
    MonitorsEnriched { monitors: serde_json::Value, generation: Option<u64>, failed: bool },
    #[serde(rename = "getReport")]
    GetReport { report: serde_json::Value },
    /// 动态 key: `getVCP::{monitor}::{code}`（等价原版模板字面量）。
    #[serde(rename = "getVCPResponse")]
    GetVcpResponse {
        /// 发送时替换为 `getVCP::{monitor}::{code}`
        response_key: String,
        monitor: String,
        code: u8,
        value: Option<u32>,
    },
    #[serde(rename = "wmi-bridge-test")]
    WmiBridgeTest,
}

/// 等价原版 getVCP 响应 type 的动态构造。
pub fn get_vcp_response_key(monitor: &str, code: u8) -> String {
    format!("getVCP::{}::{}", monitor, code)
}
