//! 显示设备清单（DISPLAY_DEVICE + DisplayConfig 两条路）。
//!
//! equiv: ddcci.cc:L289-L349（getAllDisplays）、L376-L470（getDisplayConfigTargets）。

use crate::{DisplayConfigTarget, DisplayDevice};
use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::Graphics::Gdi::{EnumDisplayDevicesW, DISPLAY_DEVICEW};

/// wideToString 等价（equiv cc:L353-L374）：UTF-16 -> UTF-8。
pub(crate) fn wide_to_string(wstr: &[u16]) -> String {
    let len = wstr.iter().position(|&c| c == 0).unwrap_or(wstr.len());
    String::from_utf16_lossy(&wstr[..len])
}

/// getAllDisplays 等价（equiv cc:L289-L349）。
/// keyType: "name" -> 以 deviceName 为 key；"adapter" -> adapterName；其余 -> deviceKey。
pub(crate) fn get_all_displays(
    key_type: &str,
    mut ordered_displays: Option<&mut Vec<DisplayDevice>>,
) -> std::collections::HashMap<String, DisplayDevice> {
    let mut out = std::collections::HashMap::new();

    // Loop through adapters（原注释语义）
    let mut adapter_dev_index: u32 = 0;
    loop {
        // SAFETY: DISPLAY_DEVICEW.cbSize 先设置
        let mut adapter_dev = DISPLAY_DEVICEW::default();
        adapter_dev.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
        // 原版对 ATTACHED_TO_DESKTOP 的检查被注释掉（//continue;），保留此行为
        if unsafe { EnumDisplayDevicesW(None, adapter_dev_index, &mut adapter_dev, 0) }.as_bool() {
            adapter_dev_index += 1;
        } else {
            break;
        }

        // Loop through displays (with device ID) on each adapter（原注释语义）
        let mut display_dev_index: u32 = 0;
        loop {
            let mut display_dev = DISPLAY_DEVICEW::default();
            display_dev.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
            if !unsafe {
                EnumDisplayDevicesW(
                    windows::core::PCWSTR(adapter_dev.DeviceName.as_ptr()),
                    display_dev_index,
                    &mut display_dev,
                    windows::Win32::UI::WindowsAndMessaging::EDD_GET_DEVICE_INTERFACE_NAME, // u32
                )
            }
            .as_bool()
            {
                break;
            }
            display_dev_index += 1;

            // Check valid target
            if (display_dev.StateFlags & windows::Win32::Graphics::Gdi::DISPLAY_DEVICE_ATTACHED_TO_DESKTOP).0 == 0 {
                continue;
            }

            let adapter_name = wide_to_string(&adapter_dev.DeviceName);
            let device_name = wide_to_string(&display_dev.DeviceName);
            let device_id = wide_to_string(&display_dev.DeviceID);
            let device_key = device_id
                .split("#{")
                .next()
                .unwrap_or(&device_id)
                .to_string();

            let new_device = DisplayDevice {
                adapter_name: adapter_name.clone(),
                device_name: device_name.clone(),
                device_id: device_id.clone(),
                device_key: device_key.clone(),
            };

            if let Some(ordered) = ordered_displays.as_deref_mut() {
                ordered.push(new_device.clone());
            }

            let key = if key_type == "name" {
                device_name
            } else if key_type == "adapter" {
                adapter_name
            } else {
                device_key
            };
            out.insert(key, new_device);
        }
    }
    out
}

/// getDisplayConfigTargets 等价（equiv cc:L376-L470）。
/// 局部失败返回空集：索引偏移会把物理句柄配错显示器（原注释语义）。
pub(crate) fn get_display_config_targets() -> Vec<DisplayConfigTarget> {
    let mut targets = Vec::new();

    let mut path_count: u32 = 0;
    let mut mode_count: u32 = 0;
    let mut result: WIN32_ERROR;
    #[allow(unused_assignments)] // do-while 结构（等价 C++ L394-L411）：首次赋值在循环内
    let mut paths: Vec<windows::Win32::Devices::Display::DISPLAYCONFIG_PATH_INFO> = Vec::new();

    loop {
        // SAFETY: 出参指针合法
        result = unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count) };
        if result != ERROR_SUCCESS {
            return targets;
        }
        paths = vec![Default::default(); path_count as usize];
        let mut modes: Vec<windows::Win32::Devices::Display::DISPLAYCONFIG_MODE_INFO> =
            vec![Default::default(); mode_count as usize];
        let mut actual_paths = path_count;
        let mut actual_modes = mode_count;
        // SAFETY: 数组容量与返回值匹配
        result = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut actual_paths,
                paths.as_mut_ptr(),
                &mut actual_modes,
                modes.as_mut_ptr(),
                None,
            )
        };
        if result == WIN32_ERROR(ERROR_INSUFFICIENT_BUFFER.0) {
            continue;
        }
        break;
    }

    if result != ERROR_SUCCESS {
        return targets;
    }
    paths.truncate(path_count as usize);

    for path in &paths {
        // SAFETY: header.type/size/adapterId/id 按 SDK 要求先设置
        let mut source_name = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
        source_name.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
        source_name.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
        source_name.header.adapterId = path.sourceInfo.adapterId;
        source_name.header.id = path.sourceInfo.id;
        if unsafe { DisplayConfigGetDeviceInfo(&mut source_name.header) } != ERROR_SUCCESS.0 as i32 {
            return Vec::new();
        }

        let mut target_name = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
        target_name.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
        target_name.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
        target_name.header.adapterId = path.targetInfo.adapterId;
        target_name.header.id = path.targetInfo.id;
        if unsafe { DisplayConfigGetDeviceInfo(&mut target_name.header) } != ERROR_SUCCESS.0 as i32 {
            return Vec::new();
        }

        let gdi_device_name = wide_to_string(&source_name.viewGdiDeviceName);
        let device_path = wide_to_string(&target_name.monitorDevicePath);
        let device_key = device_path
            .split("#{")
            .next()
            .unwrap_or(&device_path)
            .to_string();
        let friendly_name = wide_to_string(&target_name.monitorFriendlyDeviceName);
        if gdi_device_name.is_empty() || device_path.is_empty() {
            return Vec::new();
        }
        targets.push(DisplayConfigTarget {
            gdi_device_name,
            device_path,
            device_key,
            friendly_name,
        });
    }
    targets
}
