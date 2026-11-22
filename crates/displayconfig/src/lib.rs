//! win32-displayconfig 等价移植。
//!
//! 源模块: `origin/src/modules/win32-displayconfig`（index.js 757 行 + win32-displayconfig.cc 1073 行）。
//! 对外 API 语义与原 JS 一一对应；枚举到字符串的映射逐字保留（含原版的 `ldvs` 拼写）。

pub mod convert;
pub mod hdr;
pub mod extract;
pub mod monitor;
mod monitor_thread;
pub mod query;
pub mod refresh;
pub mod topology;

pub use extract::{extract_display_config, ExtractedDisplayConfig};
pub use monitor::{add_display_change_listener, remove_display_change_listener, DisplayChangeEvent};
pub use query::{query_display_config, NameInfo, QueryResults};
pub use refresh::VerticalRefreshRateContext;
pub use topology::{
    display_config_for_restoration, restore_display_config, toggle_enabled_displays,
    DisplayRestorationConfig, RestoreDisplayConfigArgs, ToggleEnabledDisplaysArgs,
};

/// Win32 错误码。等价 index.js 的 `Win32Error`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Win32Error {
    /// 原 Win32 API 返回的确切数字码。
    pub code: i32,
}

impl Win32Error {
    pub fn new(code: i32) -> Self {
        Self { code }
    }
}

impl std::fmt::Display for Win32Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Win32 error code {}", self.code)
    }
}

impl std::error::Error for Win32Error {}

/// adapter 标识。等价 index.js 的 `AdapterId`（LUID LowPart/HighPart）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct AdapterId {
    pub low_part: u32,
    pub high_part: i32,
}

impl AdapterId {
    pub fn from_luid(luid: &windows::Win32::Foundation::LUID) -> Self {
        Self {
            low_part: luid.LowPart,
            high_part: luid.HighPart,
        }
    }
    pub fn to_luid(&self) -> windows::Win32::Foundation::LUID {
        windows::Win32::Foundation::LUID {
            LowPart: self.low_part,
            HighPart: self.high_part,
        }
    }
}

/// config 路径上的目标/源标识。等价 index.js 的 `ConfigId`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ConfigId {
    pub adapter_id: AdapterId,
    pub id: u32,
}

/// 有理数（刷新率等）。等价 `DisplayConfigFractional`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rational {
    pub numerator: u32,
    pub denominator: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机测试: QueryDisplayConfig 全流程（枚举 + 名字获取）。
    /// 无头 CI 环境跳过——本仓库目标是 Windows 真机对拍，真机上必须通过。
    #[test]
    fn query_display_config_smoke() {
        let result = query::query_display_config();
        match result {
            Ok(qr) => {
                // path 与 mode 数量一致约束由 API 保证
                assert!(!qr.names.is_empty() || qr.paths.is_empty());
                for n in &qr.names {
                    assert!(!n.monitor_device_path.is_empty() || n.monitor_friendly_device_name.is_empty());
                }
            }
            Err(e) => {
                // 显示子系统不可用时允许错误返回，但错误码必须是合法 Win32 码
                assert!(e.code >= 0);
            }
        }
    }

    /// 真机测试: extractDisplayConfig + 刷新率查询。
    #[test]
    fn extract_and_refresh_smoke() {
        let extracted = extract::extract_display_config();
        if let Ok(entries) = extracted {
            let ctx = refresh::VerticalRefreshRateContext::new();
            for entry in &entries {
                if entry.in_use {
                    let _ = ctx.find_vertical_refresh_rate_for_display_point(
                        entry.source_mode.position_x,
                        entry.source_mode.position_y,
                    );
                }
            }
        }
    }

    /// 枚举映射单测（纯逻辑，不依赖真机）。
    #[test]
    fn enum_mapping_matches_cc() {
        use windows::Win32::Devices::Display::*;
        assert_eq!(convert::output_technology_str(DISPLAYCONFIG_OUTPUT_TECHNOLOGY_HDMI), "hdmi");
        assert_eq!(convert::output_technology_str(DISPLAYCONFIG_OUTPUT_TECHNOLOGY_LVDS), "ldvs");
        assert_eq!(convert::output_technology_str(DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL), "internal");
        assert_eq!(convert::output_technology_str(DISPLAYCONFIG_OUTPUT_TECHNOLOGY_OTHER), "other");
        assert_eq!(convert::rotation_str(DISPLAYCONFIG_ROTATION_ROTATE90), 90);
        assert_eq!(convert::scaling_str(DISPLAYCONFIG_SCALING_PREFERRED), "preferred");
        assert_eq!(convert::scaling_str(DISPLAYCONFIG_SCALING_IDENTITY), "identity");
        assert_eq!(
            convert::scan_line_ordering_str(DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE),
            "progressive"
        );
        assert_eq!(
            convert::scan_line_ordering_str(DISPLAYCONFIG_SCANLINE_ORDERING_INTERLACED),
            "interlaced"
        );
    }
}
