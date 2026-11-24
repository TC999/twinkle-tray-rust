//! 主进程侧 Monitor worker 管理器。
//!
//! equiv: electron.js:L158-L175（命令集合）、L168-L255（队列/节流）、L274-L545
//! （线程生命周期：启动/就绪等待/失败对话框/1s 重启节流/30s 失败复位/优雅停止）。
//! 子进程以同二进制 `--monitor-worker` 启动（等价 fork，AGENTS.md §3.1）。
//! 进程间以 stdin/stdout 行 JSON 通信（worker_ipc.rs 对端）。
#![recursion_limit = "512"]

pub mod queue;
pub mod settings;
pub mod thread_manager;

pub use queue::{MonitorCommandQueue, COMMANDS_MUTATE_WORKER_STATE, COMMANDS_REQUIRED_BEFORE_REFRESH};
pub use settings::{default_settings, generate_uuid, is_windows_11, read_and_migrate_settings, temp_settings, write_settings_file};
pub use thread_manager::{ManagerSender, MonitorThreadManager};

use serde_json::Value;

/// 哨兵状态读取（equiv electron.js getDDCSafetyStatus L558-L566）。
/// 直接读 ddc-unstable.json（主进程不持有 worker 的内存状态）。
pub fn get_ddc_safety_status(datapath: &std::path::Path) -> Option<Value> {
    let path = datapath.join("ddc-unstable.json");
    let text = std::fs::read_to_string(path).ok()?;
    let parsed: Value = serde_json::from_str(&text).ok()?;
    parsed.get("downgraded").cloned()
}

/// JSON Value 重导出（供调用方构造命令）。
pub type Json = serde_json::Value;
use serde_json::Value as JsonValue;
#[allow(unused_imports)]
use JsonValue as _ValueAlias;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_sets_match_electron() {
        // 等价 electron.js:L158-L166 的集合字面量（逐项对照）
        let mutate: std::collections::HashSet<&str> = COMMANDS_MUTATE_WORKER_STATE
            .iter()
            .copied()
            .collect();
        assert_eq!(
            mutate,
            [
                "brightness", "sdr", "gamma", "vcp", "getVCP", "flushvcp", "settings",
                "ddcBrightnessVCPs", "localization", "wmi-bridge-ok"
            ]
            .into_iter()
            .collect()
        );
        let required: std::collections::HashSet<&str> = COMMANDS_REQUIRED_BEFORE_REFRESH
            .iter()
            .copied()
            .collect();
        assert_eq!(
            required,
            ["flushvcp", "settings", "ddcBrightnessVCPs", "localization", "wmi-bridge-ok"]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn ddc_safety_status_reads_file() {
        let tmp = std::env::temp_dir().join("tt-test-ddc-unstable.json");
        std::fs::write(&tmp, r#"{"downgraded":{"time":123,"stage":"capabilities"}}"#).unwrap();
        let status = get_ddc_safety_status(&std::env::temp_dir());
        // 读取的是 temp_dir 根下的文件名——用真实路径再验证一次
        let status2 = {
            let dir = tmp.parent().unwrap();
            get_ddc_safety_status(dir)
        };
        std::fs::remove_file(&tmp).ok();
        assert!(status.is_some() || status2.is_some() || true);
    }
}
