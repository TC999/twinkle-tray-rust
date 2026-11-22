//! extractDisplayConfig：把低层查询结果聚合成每显示器一条的高层记录。
//!
//! equiv: index.js:L194-L293 extractDisplayConfig。

use crate::query::{ModeValue, QueryResults};
use crate::{query_display_config, ConfigId, Rational};

/// 等价 index.js 的 `ExtractedDisplayConfig`。
/// path/mode buffer 的逐字节还原需求由 topology.rs 直接持有 QueryResults 满足，
/// 这里只携带聚合语义所需字段。
#[derive(Debug, Clone)]
pub struct ExtractedDisplayConfig {
    pub display_name: String,
    pub device_path: String,
    pub source_config_id: ConfigId,
    pub target_config_id: ConfigId,
    pub in_use: bool,
    pub output_technology: String,
    pub rotation: i32,
    pub scaling: String,
    pub source_mode: SourceMode,
    pub target_video_signal_info: Option<TargetVideoSignalInfo>,
    /// 该记录对应的 path 在 QueryResults.paths 中的下标（替代原版 pathBuffer）。
    pub path_index: usize,
    /// source mode 在 QueryResults.modes 中的下标（替代原版 sourceModeBuffer）。
    pub source_mode_index: usize,
    /// target mode 下标；显示器关闭时为 None（等价原版 targetModeBuffer undefined）。
    pub target_mode_index: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct SourceMode {
    pub width: u32,
    pub height: u32,
    pub pixel_format: i32,
    pub position_x: i32,
    pub position_y: i32,
}

#[derive(Debug, Clone, Default)]
pub struct TargetVideoSignalInfo {
    pub pixel_rate: u64,
    pub h_sync_freq: Rational,
    pub v_sync_freq: Rational,
    pub active_size_cx: u32,
    pub active_size_cy: u32,
    pub total_size_cx: u32,
    pub total_size_cy: u32,
    pub video_standard: u32,
    pub scanline_ordering: String,
}

/// equiv index.js:L196-L293。
/// 注意保留两个原版语义：
/// 1. `inUse = value.flags & (1 === 1)` 实为 `flags & 1`；
/// 2. 有 source 无 target 时 `inUse = false`（显示器关闭）。
pub fn extract_display_config() -> Result<Vec<ExtractedDisplayConfig>, crate::Win32Error> {
    let config: QueryResults = query_display_config()?;
    let mut ret = Vec::new();

    for (pi, _p) in config.paths.iter().enumerate() {
        let value = config.path_value(pi);
        let mut in_use = (value.flags & 1) == 1;

        let source_config_id = ConfigId {
            adapter_id: value.source_adapter_id,
            id: value.source_id,
        };
        let target_config_id = ConfigId {
            adapter_id: value.target_adapter_id,
            id: value.target_id,
        };

        // nameArray 匹配: adapterId + id + outputTechnology 非空 + monitorDevicePath 非空
        let display_entry = config.names.iter().find(|n| {
            n.adapter_id == value.target_adapter_id
                && n.id == value.target_id
                && !n.output_technology.is_empty()
                && !n.monitor_device_path.is_empty()
        });
        let display_entry = match display_entry {
            Some(e) => e,
            None => continue,
        };

        let source_mode_idx = value.source_mode_info_idx as usize;
        let target_mode_idx = value.target_mode_info_idx;
        let source_mode = config.modes.get(source_mode_idx);
        let target_mode = config.modes.get(target_mode_idx as usize);
        if source_mode.is_none() {
            continue;
        }
        if target_mode.is_none() {
            // 有 source 无 target => 显示器关闭
            in_use = false;
        }

        let source_mode_value = config.mode_value(source_mode_idx);
        let source_mode_value = match source_mode_value {
            Some(ModeValue::Source(sm)) => sm,
            _ => continue,
        };

        ret.push(ExtractedDisplayConfig {
            display_name: display_entry.monitor_friendly_device_name.clone(),
            device_path: display_entry.monitor_device_path.clone(),
            source_config_id,
            target_config_id,
            in_use,
            output_technology: value.output_technology.clone(),
            rotation: value.rotation,
            scaling: value.scaling.clone(),
            source_mode: SourceMode {
                width: source_mode_value.width,
                height: source_mode_value.height,
                pixel_format: source_mode_value.pixel_format,
                position_x: source_mode_value.position_x,
                position_y: source_mode_value.position_y,
            },
            target_video_signal_info: target_mode.and_then(|_| {
                match config.mode_value(target_mode_idx as usize) {
                    Some(ModeValue::Target(tm)) => Some(TargetVideoSignalInfo {
                        pixel_rate: tm.pixel_rate,
                        h_sync_freq: tm.h_sync_freq,
                        v_sync_freq: tm.v_sync_freq,
                        active_size_cx: tm.active_size_cx,
                        active_size_cy: tm.active_size_cy,
                        total_size_cx: tm.total_size_cx,
                        total_size_cy: tm.total_size_cy,
                        video_standard: tm.video_standard,
                        scanline_ordering: tm.scanline_ordering,
                    }),
                    _ => None,
                }
            }),
            path_index: pi,
            source_mode_index: source_mode_idx,
            target_mode_index: if target_mode.is_some() {
                Some(target_mode_idx as usize)
            } else {
                None
            },
        });
    }
    Ok(ret)
}
