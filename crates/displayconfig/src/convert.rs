//! 枚举到字符串映射与底层查询。
//!
//! equiv: win32-displayconfig.cc:L476-L800（Convert* 系列）、L262-L303（DoQueryDisplayConfig）、
//! L222-L260（AcquireDeviceNames）。

use crate::{AdapterId, NameInfo, QueryResults, Rational};
use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_TARGET_DEVICE_NAME,
    DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY, QDC_ALL_PATHS,
};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, WIN32_ERROR};

const ERROR_SUCCESS_U32: u32 = ERROR_SUCCESS.0;
const ERROR_INSUFFICIENT_BUFFER_U32: u32 = ERROR_INSUFFICIENT_BUFFER.0;

/// 64 来自 DISPLAYCONFIG_TARGET_DEVICE_NAME.monitorFriendlyDeviceName。
const DEVICE_NAME_SIZE: usize = 64;
/// 128 来自 DISPLAYCONFIG_TARGET_DEVICE_NAME.monitorDevicePath。
const DEVICE_PATH_SIZE: usize = 128;

/// equiv cc:L497-L547 ConvertVideoOutputTechnology。字符串逐字保留（含原版 "ldvs" 拼写）。
pub fn output_technology_str(tech: DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY) -> &'static str {
    match tech.0 {
        0 => "hd15",                     // HD15
        1 => "svideo",                   // SVIDEO
        2 => "composite",                // COMPOSITE_VIDEO
        3 => "component",                // COMPONENT_VIDEO
        4 => "dvi",                      // DVI
        5 => "hdmi",                     // HDMI
        6 => "ldvs",                     // LVDS（原实现即拼作 ldvs）
        7 => "d_jpn",                    // D_JPN
        8 => "sdi", // SDI —— 注意：SDK 中 SDI=9, D_JPN=8；原 C++ switch 顺序如下
        9 => "displayport_external",
        10 => "displayport_embedded",
        11 => "udi_external",
        12 => "udi_embedded",
        13 => "sdtvdongle",
        14 => "miracast",
        15 => "indirect_wired",
        16 => "indirect_virtual",
        -2147483648 => "internal", // DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL (0x80000000)
        _ => "other",
    }
}

/// equiv cc:L550-L563 ConvertRotation。
pub fn rotation_str(rotation: windows::Win32::Devices::Display::DISPLAYCONFIG_ROTATION) -> i32 {
    match rotation.0 {
        2 => 90,   // ROTATE90
        3 => 180,  // ROTATE180
        4 => 270,  // ROTATE270
        _ => 0,    // IDENTITY
    }
}

/// equiv cc:L565-L580 ConvertScaling。
pub fn scaling_str(scaling: windows::Win32::Devices::Display::DISPLAYCONFIG_SCALING) -> &'static str {
    match scaling.0 {
        2 => "scaling",                 // DISPLAYCONFIG_SCALING_CENTERED
        3 => "stretched",               // DISPLAYCONFIG_SCALING_STRETCHED
        4 => "aspectratiocenteredmax",  // DISPLAYCONFIG_SCALING_ASPECTRATIOCENTEREDMAX
        5 => "custom",                  // DISPLAYCONFIG_SCALING_CUSTOM
        128 => "preferred",             // DISPLAYCONFIG_SCALING_PREFERRED
        _ => "identity",                // 含 DISPLAYCONFIG_SCALING_IDENTITY(1)
    }
}

/// equiv cc:L592-L604 ConvertScanLineOrdering。
pub fn scan_line_ordering_str(
    ordering: windows::Win32::Devices::Display::DISPLAYCONFIG_SCANLINE_ORDERING,
) -> &'static str {
    match ordering.0 {
        1 => "progressive",             // DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE
        2 => "interlaced",              // DISPLAYCONFIG_SCANLINE_ORDERING_INTERLACED
        3 => "interlaced_lowerfieldfirst", // DISPLAYCONFIG_SCANLINE_ORDERING_INTERLACED_LOWERFIELDFIRST
        _ => "unspecified",
    }
}

/// equiv cc:L222-L260 AcquireDeviceNames。
/// 失败静默跳过（原注释：不暴露给 Node，这里同样不暴露）。
fn acquire_device_names(paths: &[DISPLAYCONFIG_PATH_INFO], names: &mut Vec<NameInfo>) {
    for path in paths {
        let adapter_id = AdapterId::from_luid(&path.targetInfo.adapterId);
        let id = path.targetInfo.id;
        if names
            .iter()
            .any(|n| n.adapter_id == adapter_id && n.id == id)
        {
            continue;
        }

        // 不变量: request 由 DisplayConfigGetDeviceInfo 填充，header.size 必须先设为结构体大小
        let mut request = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
        request.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
        request.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
        request.header.adapterId = path.targetInfo.adapterId;
        request.header.id = id;

        // SAFETY: request 指向合法结构体且 header.size 与之匹配
        let error = unsafe { DisplayConfigGetDeviceInfo(&mut request.header) };
        if error != ERROR_SUCCESS_U32 as i32 {
            continue;
        }

        let friendly = string_from_utf16(&request.monitorFriendlyDeviceName, DEVICE_NAME_SIZE);
        let device_path = string_from_utf16(&request.monitorDevicePath, DEVICE_PATH_SIZE);

        names.push(NameInfo {
            adapter_id,
            id,
            output_technology: output_technology_str(request.outputTechnology).to_string(),
            edid_manufacture_id: request.edidManufactureId,
            edid_product_code_id: request.edidProductCodeId,
            connector_instance: request.connectorInstance,
            monitor_friendly_device_name: friendly,
            monitor_device_path: device_path,
        });
    }
}

fn string_from_utf16(buf: &[u16], max: usize) -> String {
    let len = buf.iter().take(max).position(|&c| c == 0).unwrap_or(max);
    String::from_utf16_lossy(&buf[..len])
}

/// equiv cc:L262-L303 DoQueryDisplayConfig：带 cPathInfoMax/cModeInfoMax 高水位重试循环。
pub fn do_query_display_config() -> Result<QueryResults, crate::Win32Error> {
    let mut c_path_info_max: u32 = 0;
    let mut c_mode_info_max: u32 = 0;
    loop {
        let mut c_path_info: u32 = 0;
        let mut c_mode_info: u32 = 0;
        // SAFETY: 两个出参指针均为合法可写位置
        let error = unsafe {
            GetDisplayConfigBufferSizes(QDC_ALL_PATHS, &mut c_path_info, &mut c_mode_info)
        };
        if error != ERROR_SUCCESS {
            c_path_info = 0;
            c_mode_info = 0;
        } else {
            if c_path_info > c_path_info_max {
                c_path_info_max = c_path_info;
            }
            c_path_info = c_path_info_max;
            if c_mode_info > c_mode_info_max {
                c_mode_info_max = c_mode_info;
            }
            c_mode_info = c_mode_info_max;
        }

        let mut path_info: Vec<DISPLAYCONFIG_PATH_INFO> =
            vec![DISPLAYCONFIG_PATH_INFO::default(); c_path_info as usize];
        let mut mode_info: Vec<DISPLAYCONFIG_MODE_INFO> =
            vec![DISPLAYCONFIG_MODE_INFO::default(); c_mode_info as usize];

        if error != ERROR_SUCCESS {
            return Err(crate::Win32Error::new(error.0 as i32));
        }

        let mut actual_paths = c_path_info;
        let mut actual_modes = c_mode_info;
        // SAFETY: 数组容量由 GetDisplayConfigBufferSizes 决定，指针与长度匹配
        let qerror = unsafe {
            QueryDisplayConfig(
                QDC_ALL_PATHS,
                &mut actual_paths,
                path_info.as_mut_ptr(),
                &mut actual_modes,
                mode_info.as_mut_ptr(),
                None,
            )
        };
        match qerror {
            WIN32_ERROR(e) if e == ERROR_SUCCESS_U32 => {
                path_info.truncate(actual_paths as usize);
                mode_info.truncate(actual_modes as usize);
                let mut names = Vec::new();
                acquire_device_names(&path_info, &mut names);
                return Ok(convert_results(&path_info, &mode_info, names));
            }
            WIN32_ERROR(e) if e == ERROR_INSUFFICIENT_BUFFER_U32 => {
                continue;
            }
            WIN32_ERROR(e) => {
                return Err(crate::Win32Error::new(e as i32));
            }
        }
    }
}

fn convert_results(
    paths: &[DISPLAYCONFIG_PATH_INFO],
    modes: &[DISPLAYCONFIG_MODE_INFO],
    names: Vec<NameInfo>,
) -> QueryResults {
    // path_value/mode_value 的转换在 query.rs 完成；此处先携带原始数组
    QueryResults::from_raw(paths.to_vec(), modes.to_vec(), names)
}

/// 供 query.rs 复用的 rational 转换（equiv cc:L582-L590）。
pub fn rational_from(r: &windows::Win32::Devices::Display::DISPLAYCONFIG_RATIONAL) -> Rational {
    Rational {
        numerator: r.Numerator,
        denominator: r.Denominator,
    }
}
