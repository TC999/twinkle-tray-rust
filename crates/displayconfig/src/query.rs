//! QueryDisplayConfig 结果类型与值转换。
//!
//! equiv: index.js:L42-L183（typedef 们）、win32-displayconfig.cc:L620-L800（ConvertPathInfo 等）。

use crate::convert::{output_technology_str, rational_from, rotation_str, scaling_str, scan_line_ordering_str};
use crate::{AdapterId, Rational};
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_MODE_INFO_TYPE, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_MODE, DISPLAYCONFIG_TARGET_MODE,
};

/// equiv cc:L800-L815 ConvertNameInfo / cc:L14-L28 Win32DeviceNameInfo。
#[derive(Debug, Clone, Default)]
pub struct NameInfo {
    pub adapter_id: AdapterId,
    pub id: u32,
    pub output_technology: String,
    pub edid_manufacture_id: u16,
    pub edid_product_code_id: u16,
    pub connector_instance: u32,
    pub monitor_friendly_device_name: String,
    pub monitor_device_path: String,
}

/// 单条 path 的值视图。等价 index.js 的 `PathInfoValue` 关键字段。
#[derive(Debug, Clone, Default)]
pub struct PathValue {
    pub flags: u32,
    pub source_adapter_id: AdapterId,
    pub source_id: u32,
    pub source_status_flags: u32,
    pub source_mode_info_idx: u32,
    pub target_adapter_id: AdapterId,
    pub target_id: u32,
    pub target_status_flags: u32,
    pub target_mode_info_idx: u32,
    pub output_technology: String,
    pub rotation: i32,
    pub scaling: String,
    pub refresh_rate: Rational,
    pub scan_line_ordering: String,
    pub target_available: bool,
}

/// 单条 mode 的值视图（只保留原版实际用到的 source/target 形态）。
#[derive(Debug, Clone)]
pub enum ModeValue {
    Source(SourceModeInfo),
    Target(TargetModeInfo),
}

#[derive(Debug, Clone, Default)]
pub struct SourceModeInfo {
    pub adapter_id: AdapterId,
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub pixel_format: i32,
    pub position_x: i32,
    pub position_y: i32,
}

#[derive(Debug, Clone, Default)]
pub struct TargetModeInfo {
    pub adapter_id: AdapterId,
    pub id: u32,
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

/// QueryDisplayConfig 全量结果。保留原始 struct 以支持 restore 场景的逐字节序列化。
#[derive(Clone)]
pub struct QueryResults {
    pub paths: Vec<DISPLAYCONFIG_PATH_INFO>,
    pub modes: Vec<DISPLAYCONFIG_MODE_INFO>,
    pub names: Vec<NameInfo>,
}

impl std::fmt::Debug for QueryResults {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryResults")
            .field("paths", &self.paths.len())
            .field("modes", &self.modes.len())
            .field("names", &self.names.len())
            .finish()
    }
}

impl Default for QueryResults {
    fn default() -> Self {
        Self {
            paths: Vec::new(),
            modes: Vec::new(),
            names: Vec::new(),
        }
    }
}

impl QueryResults {
    pub fn from_raw(
        paths: Vec<DISPLAYCONFIG_PATH_INFO>,
        modes: Vec<DISPLAYCONFIG_MODE_INFO>,
        names: Vec<NameInfo>,
    ) -> Self {
        Self { paths, modes, names }
    }

    /// equiv cc:L654-L676 ConvertPathInfo。
    pub fn path_value(&self, idx: usize) -> PathValue {
        let p = &self.paths[idx];
        let supports_virtual = p.flags & 8 /* DISPLAYCONFIG_PATH_SUPPORT_VIRTUAL_MODE */ == 8;
        // 0.62 绑定: source/target 的 modeInfoIdx 是 _0_0._bitfield 的低 24 位（同 SDK 宏语义）。
        // supports_virtual 时原版读 sourceModeInfoIdx/targetModeInfoIdx 字段，本绑定无此独立字段，
        // 但 union 布局一致：modeInfoIdx 分支在非 virtual 时即所需值；virtual 时 _bitfield 低 24 位。
        let source_mode_idx = if supports_virtual {
            unsafe { p.sourceInfo.Anonymous.Anonymous._bitfield & 0x00FF_FFFF }
        } else {
            unsafe { p.sourceInfo.Anonymous.modeInfoIdx }
        };
        let target_mode_idx = if supports_virtual {
            unsafe { p.targetInfo.Anonymous.Anonymous._bitfield & 0x00FF_FFFF }
        } else {
            unsafe { p.targetInfo.Anonymous.modeInfoIdx }
        };
        PathValue {
            flags: p.flags,
            source_adapter_id: AdapterId::from_luid(&p.sourceInfo.adapterId),
            source_id: p.sourceInfo.id,
            source_status_flags: p.sourceInfo.statusFlags,
            source_mode_info_idx: source_mode_idx,
            target_adapter_id: AdapterId::from_luid(&p.targetInfo.adapterId),
            target_id: p.targetInfo.id,
            target_status_flags: p.targetInfo.statusFlags,
            target_mode_info_idx: target_mode_idx,
            output_technology: output_technology_str(p.targetInfo.outputTechnology).to_string(),
            rotation: rotation_str(p.targetInfo.rotation),
            scaling: scaling_str(p.targetInfo.scaling).to_string(),
            refresh_rate: rational_from(&p.targetInfo.refreshRate),
            scan_line_ordering: scan_line_ordering_str(p.targetInfo.scanLineOrdering).to_string(),
            target_available: p.targetInfo.targetAvailable.as_bool(),
        }
    }

    /// equiv cc:L780-L798 ConvertModeInfo（只实现 source/target 两形态，与 index.js 用法一致）。
    pub fn mode_value(&self, idx: usize) -> Option<ModeValue> {
        let m = self.modes.get(idx)?;
        match m.infoType {
            DISPLAYCONFIG_MODE_INFO_TYPE(x) if x == 1 => {
                // SAFETY: infoType == SOURCE 时 union 的 sourceMode 分支有效
                let sm: &DISPLAYCONFIG_SOURCE_MODE = unsafe { &m.Anonymous.sourceMode };
                Some(ModeValue::Source(SourceModeInfo {
                    adapter_id: AdapterId::from_luid(&m.adapterId),
                    id: m.id,
                    width: sm.width,
                    height: sm.height,
                    pixel_format: sm.pixelFormat.0,
                    position_x: sm.position.x,
                    position_y: sm.position.y,
                }))
            }
            DISPLAYCONFIG_MODE_INFO_TYPE(x) if x == 2 => {
                // SAFETY: infoType == TARGET 时 union 的 targetMode 分支有效
                let tm: &DISPLAYCONFIG_TARGET_MODE = unsafe { &m.Anonymous.targetMode };
                let v = &tm.targetVideoSignalInfo;
                Some(ModeValue::Target(TargetModeInfo {
                    adapter_id: AdapterId::from_luid(&m.adapterId),
                    id: m.id,
                    pixel_rate: v.pixelRate,
                    h_sync_freq: rational_from(&v.hSyncFreq),
                    v_sync_freq: rational_from(&v.vSyncFreq),
                    active_size_cx: v.activeSize.cx,
                    active_size_cy: v.activeSize.cy,
                    total_size_cx: v.totalSize.cx,
                    total_size_cy: v.totalSize.cy,
                    video_standard: unsafe { v.Anonymous.videoStandard },
                    scanline_ordering: scan_line_ordering_str(v.scanLineOrdering).to_string(),
                }))
            }
            _ => None,
        }
    }
}

/// equiv index.js:L137-L150 queryDisplayConfig()。
pub fn query_display_config() -> Result<QueryResults, crate::Win32Error> {
    crate::convert::do_query_display_config()
}

/// 供 convert.rs 引用的再导出（避免循环依赖）。
pub use windows::Win32::Devices::Display::DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY as _VideoOutputTechnology;
