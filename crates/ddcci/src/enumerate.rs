//! 物理显示器枚举。
//!
//! equiv: ddcci.cc:L272-L297（getPhysicalMonitorName）、L471-L535（getAllHandles）、
//! L536-L561（getHighLevelCapabilities）。

use crate::{dlog, plog, Monitor, MonitorHighLevel};
use windows::Win32::Devices::Display::{
    GetMonitorBrightness, GetMonitorContrast, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, PHYSICAL_MONITOR,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, MONITORINFOEXW,
};
use windows::Win32::Foundation::RECT;

/// getPhysicalMonitorName 等价（equiv cc:L272-L287）：MONITORINFOEXW.szDevice。
pub(crate) fn get_physical_monitor_name(handle: windows::Win32::Graphics::Gdi::HMONITOR) -> String {
    // SAFETY: monitorInfo.cbSize 已设置且 handle 来自 EnumDisplayMonitors
    let mut monitor_info = MONITORINFOEXW::default();
    monitor_info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    let ok = unsafe { GetMonitorInfoW(handle, &mut monitor_info as *mut _ as *mut _) };
    if !ok.as_bool() {
        return String::new();
    }
    let len = monitor_info
        .szDevice
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(monitor_info.szDevice.len());
    String::from_utf16_lossy(&monitor_info.szDevice[..len])
}

/// getAllHandles 等价（equiv cc:L471-L535）。
/// 虚拟/远程显示器可能没有物理显示器句柄——跳过而非整体失败（原注释语义）。
pub(crate) fn get_all_handles() -> Vec<Monitor> {
    let mut monitors: Vec<Monitor> = Vec::new();

    // SAFETY: lparam 传入 monitors 的裸指针，EnumProc 内仅 push
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(monitor_enum_proc
                as unsafe extern "system" fn(
                    windows::Win32::Graphics::Gdi::HMONITOR,
                    HDC,
                    *mut RECT,
                    windows::Win32::Foundation::LPARAM,
                ) -> windows::core::BOOL),
            windows::Win32::Foundation::LPARAM(&mut monitors as *mut _ as isize),
        );
    }

    for monitor in monitors.iter_mut() {
        monitor.monitor_name =
            get_physical_monitor_name(windows::Win32::Graphics::Gdi::HMONITOR(
                monitor.handle.0 as *mut core::ffi::c_void,
            ));

        let mut num_physical_monitors: u32 = 0;
        // SAFETY: 出参指针合法
        if unsafe {
            GetNumberOfPhysicalMonitorsFromHMONITOR(
                windows::Win32::Graphics::Gdi::HMONITOR(monitor.handle.0 as *mut core::ffi::c_void),
                &mut num_physical_monitors,
            )
        }
        .is_err()
        {
            let error_code = unsafe { windows::Win32::Foundation::GetLastError() };
            plog!(
                "Skipping {}: couldn't get its physical monitor count. {:?}",
                monitor.monitor_name,
                error_code
            );
            continue;
        }
        if num_physical_monitors == 0 {
            plog!("Skipping {}: no physical monitors.", monitor.monitor_name);
            continue;
        }

        let mut physical_monitors: Vec<PHYSICAL_MONITOR> =
            vec![PHYSICAL_MONITOR::default(); num_physical_monitors as usize];
        // SAFETY: 数组长度与 numPhysicalMonitors 匹配
        if unsafe {
            GetPhysicalMonitorsFromHMONITOR(
                windows::Win32::Graphics::Gdi::HMONITOR(monitor.handle.0 as *mut core::ffi::c_void),
                &mut physical_monitors,
            )
        }
        .is_err() {
            let error_code = unsafe { windows::Win32::Foundation::GetLastError() };
            plog!(
                "Skipping {}: couldn't get its physical monitors. {:?}",
                monitor.monitor_name,
                error_code
            );
            continue;
        }

        for pm in &physical_monitors {
            let pm = *pm; // packed struct: 先整体 copy 再访问字段
            monitor
                .physical_handles
                .push(crate::HWrap(pm.hPhysicalMonitor.0 as usize));
            let desc = pm.szPhysicalMonitorDescription;
            let len = desc.iter().position(|&c| c == 0).unwrap_or(desc.len());
            monitor
                .physical_descriptions
                .push(String::from_utf16_lossy(&desc[..len]));
        }
    }

    // 无句柄的显示器不持有资源，丢弃不泄漏（原注释语义）
    monitors.retain(|m| !m.physical_handles.is_empty());
    monitors
}

/// destroyPhysicalMonitorHandles 等价（equiv cc:L148-L157）。
pub(crate) fn destroy_physical_monitor_handles(monitors: &mut [Monitor]) {
    for monitor in monitors.iter_mut() {
        for handle in monitor.physical_handles.iter_mut() {
            if !handle.is_invalid() {
                // SAFETY: 句柄所有权在本集合
                unsafe {
                    let _ = windows::Win32::Devices::Display::DestroyPhysicalMonitor(handle.to_handle());
                }
                *handle = crate::HWrap(0);
            }
        }
    }
}

/// monitor_enum_proc（equiv cc:L479-L486 lambda）。
unsafe extern "system" fn monitor_enum_proc(
    hmonitor: windows::Win32::Graphics::Gdi::HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    dw_data: windows::Win32::Foundation::LPARAM,
) -> windows::core::BOOL {
    let monitors = &mut *(dw_data.0 as *mut Vec<Monitor>);
    monitors.push(Monitor {
        handle: crate::HWrap(hmonitor.0 as usize),
        ..Default::default()
    });
    windows::core::BOOL(1) // TRUE
}

/// getHighLevelCapabilities 等价（equiv cc:L536-L561）。
/// GetMonitorCapabilities 被原版注释禁用（不支持色温的显示器响应差），移植时保留该禁用状态。
pub(crate) fn get_high_level_capabilities(handle: crate::HWrap) -> MonitorHighLevel {
    let mut monitor = MonitorHighLevel::default();

    if handle.is_invalid() {
        dlog!("No handle for capabilities data!");
        return monitor;
    }

    // Brightness
    // SAFETY: 三个出参均为合法可写位置
    let mut brightness_min = 0u32;
    let mut brightness = 0u32;
    let mut brightness_max = 0u32;
    let brightness_ok = unsafe {
        GetMonitorBrightness(
            handle.to_handle(),
            &mut brightness_min,
            &mut brightness,
            &mut brightness_max,
        )
    };
    monitor.brightness_ok = brightness_ok as u32;
    monitor.brightness_min = brightness_min;
    monitor.brightness = brightness;
    monitor.brightness_max = brightness_max;
    dlog!(
        "-- -- GetMonitorBrightness: {}: {} ({}-{})",
        brightness_ok,
        brightness,
        brightness_min,
        brightness_max
    );
    if brightness_ok == 0 {
        let e = unsafe { windows::Win32::Foundation::GetLastError() };
        dlog!("-- -- Error: {:?}", e);
    }

    // Contrast
    let mut contrast_min = 0u32;
    let mut contrast = 0u32;
    let mut contrast_max = 0u32;
    let contrast_ok = unsafe {
        GetMonitorContrast(handle.to_handle(), &mut contrast_min, &mut contrast, &mut contrast_max)
    };
    monitor.contrast_ok = contrast_ok as u32;
    monitor.contrast_min = contrast_min;
    monitor.contrast = contrast;
    monitor.contrast_max = contrast_max;
    dlog!(
        "-- -- GetMonitorContrast: {}: {} ({}-{})",
        contrast_ok,
        contrast,
        contrast_min,
        contrast_max
    );
    if contrast_ok == 0 {
        let e = unsafe { windows::Win32::Foundation::GetLastError() };
        dlog!("-- -- Error: {:?}", e);
    }

    monitor
}
