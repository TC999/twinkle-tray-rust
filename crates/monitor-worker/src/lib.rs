//! Monitors worker：`--monitor-worker` 模式的核心逻辑。
//!
//! equiv: `origin/src/Monitors.js`（2299 行）。IPC 行协议消息与原版一一对应
//! （ready/settings/ddcBrightnessVCPs/wmi-bridge-ok/refreshMonitors/...）。

pub mod features;
pub mod message;
pub mod sentinel;
pub mod monitors;
pub mod settings_state;

pub use message::{IncomingMessage, OutgoingMessage};
pub use sentinel::{DdcSentinel, check_for_ddc_crash_evidence};
pub use settings_state::WorkerSettings;
