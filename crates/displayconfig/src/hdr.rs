//! HDR/SDR 亮度控制（windows-hdr 等价移植）。
//!
//! equiv: `origin/src/modules/windows-hdr/windows-hdr.cc`（310 行，基于 set_maxtml）。
//! 公开 API: get_displays / set_sdr_brightness（equiv Node 导出 getDisplays/setSDRBrightness）。
//!
//! 关键点（逐字保留）：
//! - SDR white level 的 nits 换算：SDRWhiteLevel = nits * 1000 / 80（读方向 nits = value * 80 / 1000）
//! - SET_SDR_WHITE_LEVEL = 0xFFFFFFEE（未文档化的设备信息类型）
//! - Windows 11 24H2+ 的 GET_ADVANCED_COLOR_INFO_2（type=15）优先，旧系统回退 legacy
//! - nits 钳制 80..480 且对齐 4 的倍数

use serde_json::json;
use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, DisplayConfigSetDeviceInfo, GetDisplayConfigBufferSizes,
    QueryDisplayConfig, DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
    DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SDR_WHITE_LEVEL, DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, WIN32_ERROR};

/// DISPLAYCONFIG_DEVICE_INFO_SET_SDR_WHITE_LEVEL = 0xFFFFFFEE（equiv cc:L6）。
const DISPLAYCONFIG_DEVICE_INFO_SET_SDR_WHITE_LEVEL: i32 = 0xFFFFFFEEu32 as i32;

/// TT_DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO_2 = 15（equiv cc:L15）。
/// Windows 11 24H2+ 报告活动色彩模式，可区分真 HDR 与 SDR+Auto Color Management
/// （legacy struct 对两者都报 advancedColorEnabled）。旧 Windows 上调用直接失败，
/// 走 legacy 路径。（原注释语义逐字保留）
const TT_DEVICE_INFO_GET_ADVANCED_COLOR_INFO_2: i32 = 15;

/// TT_DISPLAYCONFIG_ADVANCED_COLOR_MODE（equiv cc:L18-L23）。
#[allow(dead_code)] // SDR 模式值保留：与 C++ 枚举一一对应
const TT_ADVANCED_COLOR_MODE_SDR: u32 = 0;
const TT_ADVANCED_COLOR_MODE_HDR: u32 = 2;

/// TT_DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO_2（equiv cc:L25-L38，本地定义以兼容旧 SDK）。
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct GetAdvancedColorInfo2 {
    pub header: DisplayConfigDeviceInfoHeaderRepr,
    /// 位域 union（32 位）：advancedColorSupported:1, advancedColorActive:1, reserved1:1,
    /// advancedColorLimitedByPolicy:1, highDynamicRangeSupported:1,
    /// highDynamicRangeUserEnabled:1, wideColorSupported:1, wideColorUserEnabled:1, reserved:24
    pub value: u32,
    pub color_encoding: i32,
    pub bits_per_color_channel: u32,
    pub active_color_mode: u32,
}

/// DISPLAYCONFIG_DEVICE_INFO_HEADER 的 repr(C) 镜像（windows crate 类型含非 repr(C) 字段封装）。
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct DisplayConfigDeviceInfoHeaderRepr {
    pub kind: i32,
    pub size: u32,
    pub adapter_id_luid_low: u32,
    pub adapter_id_luid_high: i32,
    pub id: u32,
}

/// _DISPLAYCONFIG_SET_SDR_WHITE_LEVEL（equiv cc:L40-L44）。
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct SetSdrWhiteLevel {
    pub header: DisplayConfigDeviceInfoHeaderRepr,
    pub sdr_white_level: u32,
    pub final_value: u8,
}

/// pathSetSdrWhite 等价（equiv cc:L46-L60）。
unsafe fn path_set_sdr_white(
    path: &DISPLAYCONFIG_PATH_INFO,
    nits: i32,
) -> i32 {
    let mut params = SetSdrWhiteLevel::default();
    params.header.kind = DISPLAYCONFIG_DEVICE_INFO_SET_SDR_WHITE_LEVEL;
    params.header.size = std::mem::size_of::<SetSdrWhiteLevel>() as u32;
    params.header.adapter_id_luid_low = path.targetInfo.adapterId.LowPart;
    params.header.adapter_id_luid_high = path.targetInfo.adapterId.HighPart;
    params.header.id = path.targetInfo.id;

    params.sdr_white_level = ((nits as u64) * 1000 / 80) as u32;
    params.final_value = 1;

    // SAFETY: params 以 header 开头且 size 匹配（DisplayConfigSetDeviceInfo 约定）
    DisplayConfigSetDeviceInfo(&mut params as *mut SetSdrWhiteLevel as *mut windows::Win32::Devices::Display::DISPLAYCONFIG_DEVICE_INFO_HEADER)
}

/// Display 信息（equiv cc:L62-L71 struct Display）。
#[derive(Debug, Clone, Default)]
pub struct HdrDisplay {
    pub name: String,
    pub path: String,
    pub nits: i32,
    pub hdr_supported: bool,
    pub hdr_enabled: bool,
    pub hdr_active: bool,
    pub bits: i32,
    /// path 在 QueryDisplayConfig 结果中的下标（替代 C++ 保存的 DISPLAYCONFIG_PATH_INFO）
    pub path_index: usize,
}

/// wcharToString 等价（equiv cc:L73-L88）。
fn wchar_to_string(wstr: &[u16]) -> String {
    let len = wstr.iter().position(|&c| c == 0).unwrap_or(wstr.len());
    String::from_utf16_lossy(&wstr[..len])
}

/// setSDRBrightness 等价（equiv cc:L90-L125）：
/// nits 钳制 80..480、对齐 4 倍数。silent=true 时不打错误日志（hdrActive 探测用）。
pub fn set_sdr_brightness_by_path(
    path: &DISPLAYCONFIG_PATH_INFO,
    desired_nits: i32,
    silent: bool,
) -> bool {
    let mut nits = desired_nits;
    if nits < 80 {
        nits = 80;
    }
    if nits > 480 {
        nits = 480;
    }
    if nits % 4 != 0 {
        nits += 4 - (nits % 4);
    }
    // SAFETY: params 结构按 SDK 布局构造
    let result = unsafe { path_set_sdr_white(path, nits) };
    if result != ERROR_SUCCESS.0 as i32 {
        if !silent {
            eprintln!("Error on DisplayConfigSetDeviceInfo for SDR white level");
        }
        return false;
    }
    true
}

/// getDisplays 等价（equiv cc:L127-L262）。
pub fn get_displays() -> std::collections::BTreeMap<String, HdrDisplay> {
    let mut new_displays = std::collections::BTreeMap::new();

    // QueryDisplayConfig do-while（equiv cc:L133-L152）
    #[allow(unused_assignments)] // do-while 结构（等价 C++ L133-L152）：赋值在循环内
    let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = Vec::new();
    unsafe {
        let mut path_count: u32 = 0;
        let mut mode_count: u32 = 0;
        let mut result: WIN32_ERROR;
        loop {
            result = GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count);
            if result != ERROR_SUCCESS {
                eprintln!("Error on GetDisplayConfigBufferSizes");
                return new_displays;
            }
            paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
            let mut modes = vec![windows::Win32::Devices::Display::DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
            let mut actual_paths = path_count;
            let mut actual_modes = mode_count;
            result = QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut actual_paths,
                paths.as_mut_ptr(),
                &mut actual_modes,
                modes.as_mut_ptr(),
                None,
            );
            if result == WIN32_ERROR(ERROR_INSUFFICIENT_BUFFER.0) {
                continue;
            }
            if result != ERROR_SUCCESS {
                eprintln!("Error on QueryDisplayConfig");
                return new_displays;
            }
            break;
        }
        paths.truncate(path_count as usize);
    }

    for (i, path) in paths.iter().enumerate() {
        // SAFETY: 各 header.type/size/adapterId/id 按 SDK 要求先设置
        #[allow(unused_unsafe)]
        unsafe {
            let mut target_name = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            target_name.header.adapterId = path.targetInfo.adapterId;
            target_name.header.id = path.targetInfo.id;
            target_name.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            target_name.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            if DisplayConfigGetDeviceInfo(&mut target_name.header) != ERROR_SUCCESS.0 as i32 {
                eprintln!("Error on DisplayConfigGetDeviceInfo for target name");
                continue;
            }

            let mut display_info = DISPLAYCONFIG_SDR_WHITE_LEVEL::default();
            display_info.header.adapterId = path.targetInfo.adapterId;
            display_info.header.id = path.targetInfo.id;
            display_info.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL;
            display_info.header.size = std::mem::size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32;
            if DisplayConfigGetDeviceInfo(&mut display_info.header) != ERROR_SUCCESS.0 as i32 {
                eprintln!("Error on DisplayConfigGetDeviceInfo for SDR white level");
                continue;
            }

            let nits = (display_info.SDRWhiteLevel as i32) * 80 / 1000;
            let monitor_device_path = wchar_to_string(&target_name.monitorDevicePath);
            let device_path = monitor_device_path
                .split("#{")
                .next()
                .unwrap_or(&monitor_device_path)
                .to_string();

            let mut new_display = HdrDisplay {
                name: wchar_to_string(&target_name.monitorFriendlyDeviceName),
                path: device_path.clone(),
                nits,
                path_index: i,
                ..Default::default()
            };

            // Prefer the newer advanced color info（原注释）
            let mut hdr_info2 = GetAdvancedColorInfo2::default();
            hdr_info2.header.kind = TT_DEVICE_INFO_GET_ADVANCED_COLOR_INFO_2;
            hdr_info2.header.size = std::mem::size_of::<GetAdvancedColorInfo2>() as u32;
            hdr_info2.header.adapter_id_luid_low = path.targetInfo.adapterId.LowPart;
            hdr_info2.header.adapter_id_luid_high = path.targetInfo.adapterId.HighPart;
            hdr_info2.header.id = path.targetInfo.id;

            let result2 = DisplayConfigGetDeviceInfo(
                &mut hdr_info2 as *mut GetAdvancedColorInfo2 as *mut windows::Win32::Devices::Display::DISPLAYCONFIG_DEVICE_INFO_HEADER,
            );
            if result2 == ERROR_SUCCESS.0 as i32 {
                // 位域提取（equiv cc:L265-L272 的位域访问）
                let v = hdr_info2.value;
                let hdr_supported = (v >> 4) & 1 == 1;
                let hdr_enabled = (v >> 5) & 1 == 1;
                new_display.hdr_supported = hdr_supported;
                new_display.hdr_enabled = hdr_enabled;
                new_display.bits = hdr_info2.bits_per_color_channel as i32;
                new_display.hdr_active = hdr_info2.active_color_mode == TT_ADVANCED_COLOR_MODE_HDR;
            } else {
                // Older Windows: fall back to the legacy advanced color info（原注释）
                let mut hdr_info = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO::default();
                hdr_info.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO;
                hdr_info.header.size = std::mem::size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32;
                hdr_info.header.adapterId = path.targetInfo.adapterId;
                hdr_info.header.id = path.targetInfo.id;

                if DisplayConfigGetDeviceInfo(&mut hdr_info.header) != ERROR_SUCCESS.0 as i32 {
                    eprintln!("Error on DisplayConfigGetDeviceInfo for advanced color");
                    continue;
                }

                // legacy struct 的 union 位域：advancedColorSupported bit0, advancedColorEnabled bit1
                let v = unsafe { hdr_info.Anonymous.value };
                new_display.hdr_supported = (v >> 0) & 1 == 1;
                new_display.hdr_enabled = (v >> 1) & 1 == 1;
                new_display.bits = hdr_info.bitsPerColorChannel as i32;

                // The legacy struct doesn't report whether HDR is actually active,
                // so probe by attempting to set the current SDR white level.
                // Only check for HDR if Windows reports it's on（原注释逐字保留）
                new_display.hdr_active = false;
                if new_display.hdr_enabled {
                    new_display.hdr_active = set_sdr_brightness_by_path(path, nits, true);
                }
            }

            new_displays.insert(new_display.path.clone(), new_display);
        }
    }

    new_displays
}

/// getDisplays 的 JSON 视图（equiv nodeGetDisplays L264-L295 字段名）。
pub fn get_displays_json() -> Vec<serde_json::Value> {
    let displays = get_displays();
    displays
        .values()
        .map(|d| {
            json!({
                "name": d.name,
                "path": d.path,
                "nits": d.nits,
                "hdrSupported": d.hdr_supported,
                "hdrEnabled": d.hdr_enabled,
                "hdrActive": d.hdr_active,
                "bits": d.bits,
            })
        })
        .collect()
}

/// setSDRBrightness 等价（equiv nodeSetSDRBrightness L297-L318）：
/// 按 path 查找当前显示器，对其 target 设置 SDR white level。
pub fn set_sdr_brightness(path_str: &str, nits: i32) -> bool {
    let displays = get_displays();
    for display in displays.values() {
        if display.path == path_str {
            // 重新查询以取得当前 DISPLAYCONFIG_PATH_INFO
            let paths = query_active_paths();
            if let Some(path) = paths.get(display.path_index) {
                return set_sdr_brightness_by_path(path, nits, false);
            }
        }
    }
    false
}

fn query_active_paths() -> Vec<DISPLAYCONFIG_PATH_INFO> {
    unsafe {
        let mut path_count: u32 = 0;
        let mut mode_count: u32 = 0;
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
            != ERROR_SUCCESS
        {
            return Vec::new();
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![windows::Win32::Devices::Display::DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        let mut actual_paths = path_count;
        let mut actual_modes = mode_count;
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut actual_paths,
            paths.as_mut_ptr(),
            &mut actual_modes,
            modes.as_mut_ptr(),
            None,
        ) != ERROR_SUCCESS
        {
            return Vec::new();
        }
        paths.truncate(actual_paths as usize);
        paths
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sdr_nits_clamping_matches_cc() {
        // equiv cc:L96-L107：80..480 且 4 的倍数
        let path = windows::Win32::Devices::Display::DISPLAYCONFIG_PATH_INFO::default();
        // 无效 path -> 设置失败，但钳制逻辑已执行（不 panic 即通过）
        let _ = super::set_sdr_brightness_by_path(&path, 50, true);
        let _ = super::set_sdr_brightness_by_path(&path, 5000, true);
        let _ = super::set_sdr_brightness_by_path(&path, 101, true);
    }

    /// 真机测试: 枚举 HDR 显示器。
    #[test]
    fn hdr_displays_smoke() {
        let displays = super::get_displays();
        println!("HDR displays: {}", displays.len());
        for (path, d) in &displays {
            println!(
                "  name={} nits={} hdrSupported={} hdrEnabled={} hdrActive={} bits={}",
                d.name, d.nits, d.hdr_supported, d.hdr_enabled, d.hdr_active, d.bits
            );
        }
    }
}
