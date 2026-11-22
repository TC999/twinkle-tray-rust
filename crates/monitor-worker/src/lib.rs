//! Monitors worker（等价 Monitors.js）的模块集合。
//!
//! 每个子模块对应 Monitors.js 的一段逻辑域：
//! - `sentinel`：DDC/CI 崩溃哨兵（§3.2 高危区）
//! - `monitors`：三源枚举 + 特征快照 + DDC 特征扫描
//! - `wmi_fallback`：WMI 桥 / WMIC 回退链 + HDR 合并
//! - `features`：VCP 缓存与 capabilities 解析消费
//! - `software_brightness`：gamma ramp 软件亮度
//! - `brightness_ops`：setBrightness / setVCP / getBrightnessDDC / readKnownBrightness
//! - `studio_display`：Apple Studio Display USB 亮度（WinUSB 直调）
//! - `settings_state`：settings 常驻状态与判定

pub mod all_monitors;
pub mod brightness_ops;
pub mod feature_snapshots;
pub mod features;
pub mod message;
pub mod monitors;
pub mod sentinel;
pub mod settings_state;
pub mod software_brightness;
pub mod studio_display;
pub mod studio_display_discovery;
pub mod wmi_fallback;

pub use message::{IncomingMessage, OutgoingMessage};
pub use sentinel::{check_for_ddc_crash_evidence, DdcSentinel};
pub use settings_state::WorkerSettings;

/// unstableDDC 快照的便捷入口（等价模块级 `let unstableDDC = {}`）。
pub fn sentinel_current_unstable() -> std::collections::HashMap<String, serde_json::Value> {
    sentinel::unstable_ddc_snapshot()
}
