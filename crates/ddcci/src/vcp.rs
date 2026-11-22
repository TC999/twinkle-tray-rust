//! VCP 读写与 High Level 亮度/对比度。
//!
//! equiv: ddcci.cc:L1262-L1312（setVCP）、L1314-L1359（getVCP）、L1361-L1490（HighLevel 系列）。

use crate::try_ddc_ci_operation;
use windows::Win32::Devices::Display::{
    GetMonitorBrightness, GetMonitorContrast, GetVCPFeatureAndVCPFeatureReply, SaveCurrentSettings,
    SetMonitorBrightness, SetMonitorContrast, SetVCPFeature,
};


/// handles 查找：等价 `handles.find(monitorName)` + "Monitor not found" 异常语义。
pub(crate) fn find_handle(monitor_name: &str) -> Result<crate::HWrap, String> {
    let handles = crate::HANDLES.lock().unwrap();
    let map = handles.as_ref().ok_or("Monitor not found")?;
    map.get(monitor_name).copied().ok_or("Monitor not found".to_string())
}

/// setVCP 等价。
pub fn set_vcp(monitor_name: &str, vcp_code: u8, new_value: u32) -> Result<(), String> {
    let handle = find_handle(monitor_name)?;

    let mut error_code: u32 = 0; // ERROR_SUCCESS
    // SAFETY: handle 来自 refresh 的合法句柄表
    let ok = try_ddc_ci_operation(
        || unsafe { SetVCPFeature(handle.to_handle(), vcp_code, new_value) } != 0,
        &mut error_code,
    );
    if !ok {
        return Err(format!("Failed to set VCP code value (win32Code: 0x{:X})", error_code));
    }
    Ok(())
}

/// getVCP 等价：返回 [current, max]。
pub fn get_vcp(monitor_name: &str, vcp_code: u8) -> Result<(u32, u32), String> {
    let handle = find_handle(monitor_name)?;

    let mut current_value: u32 = 0;
    let mut max_value: u32 = 0;
    let mut error_code: u32 = 0;
    // SAFETY: 出参均为合法可写位置
    let ok = try_ddc_ci_operation(
        || {
            (unsafe {
                GetVCPFeatureAndVCPFeatureReply(
                    handle.to_handle(),
                    vcp_code,
                    None,
                    &mut current_value,
                    Some(&mut max_value),
                )
            }) != 0
        },
        &mut error_code,
    );
    if !ok {
        return Err(format!("Failed to get VCP code value (win32Code: 0x{:X})", error_code));
    }
    Ok((current_value, max_value))
}

/// saveCurrentSettings 等价。
pub fn save_current_settings(monitor_name: &str) -> Result<bool, String> {
    let handle = find_handle(monitor_name)?;
    // SAFETY: handle 合法
    let ok = unsafe { SaveCurrentSettings(handle.to_handle()) };
    Ok(ok != 0)
}

/// getHighLevelBrightness 等价：返回 [current, max, min]。
pub fn get_high_level_brightness(monitor_name: &str) -> Result<(u32, u32, u32), String> {
    let handle = find_handle(monitor_name)?;
    let mut min_value: u32 = 0;
    let mut current_value: u32 = 0;
    let mut max_value: u32 = 0;
    let mut error_code: u32 = 0;
    // SAFETY: 出参均为合法可写位置
    let ok = try_ddc_ci_operation(
        || {
            (unsafe {
                GetMonitorBrightness(handle.to_handle(), &mut min_value, &mut current_value, &mut max_value)
            }) != 0
        },
        &mut error_code,
    );
    if !ok {
        return Err(format!("Failed to get high level brightness (win32Code: 0x{:X})", error_code));
    }
    Ok((current_value, max_value, min_value))
}

/// setHighLevelBrightness 等价。
pub fn set_high_level_brightness(monitor_name: &str, new_value: u32) -> Result<(), String> {
    let handle = find_handle(monitor_name)?;
    let mut error_code: u32 = 0;
    // SAFETY: handle 合法
    let ok = try_ddc_ci_operation(
        || unsafe { SetMonitorBrightness(handle.to_handle(), new_value) } != 0,
        &mut error_code,
    );
    if !ok {
        return Err(format!("Failed to set high level brightness (win32Code: 0x{:X})", error_code));
    }
    Ok(())
}

/// getHighLevelContrast 等价：返回 [current, max, min]。
pub fn get_high_level_contrast(monitor_name: &str) -> Result<(u32, u32, u32), String> {
    let handle = find_handle(monitor_name)?;
    let mut min_value: u32 = 0;
    let mut current_value: u32 = 0;
    let mut max_value: u32 = 0;
    let mut error_code: u32 = 0;
    // SAFETY: 出参均为合法可写位置
    let ok = try_ddc_ci_operation(
        || unsafe { GetMonitorContrast(handle.to_handle(), &mut min_value, &mut current_value, &mut max_value) } != 0,
        &mut error_code,
    );
    if !ok {
        return Err(format!("Failed to get high level contrast (win32Code: 0x{:X})", error_code));
    }
    Ok((current_value, max_value, min_value))
}

/// setHighLevelContrast 等价。
pub fn set_high_level_contrast(monitor_name: &str, new_value: u32) -> Result<(), String> {
    let handle = find_handle(monitor_name)?;
    let mut error_code: u32 = 0;
    // SAFETY: handle 合法
    let ok = try_ddc_ci_operation(
        || unsafe { SetMonitorContrast(handle.to_handle(), new_value) } != 0,
        &mut error_code,
    );
    if !ok {
        return Err(format!("Failed to set high level contrast (win32Code: 0x{:X})", error_code));
    }
    Ok(())
}

/// setLogLevel 等价。
pub fn set_log_level(level: i32) {
    crate::set_log_level_impl(level);
}
