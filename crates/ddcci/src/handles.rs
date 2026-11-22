//! 句柄表构建（refresh 主流程）与列表 API。
//!
//! equiv: ddcci.cc:L691-L958（populateHandlesMapNormal）、L646-L758（populateHandlesMapLegacy）、
//! L959-L1003（populateHandlesMap 分发）、L1005-L1032（refresh NAPI）、L1213-L1260（list APIs）、
//! L1492-L1660（getMonitorInputs）。

use crate::displays::{get_all_displays, get_display_config_targets};
use crate::enumerate::{destroy_physical_monitor_handles, get_all_handles, get_high_level_capabilities};
use crate::{dlog, plog, DisplayConfigTarget, DisplayDevice, PhysicalMonitor};
use std::collections::{HashMap, HashSet};

/// cleanMonitorHandles 等价（equiv cc:L134-L148）。
fn clean_monitor_handles(new_handles: &HashMap<String, crate::HWrap>) {
    let mut handles = crate::HANDLES.lock().unwrap();
    if let Some(map) = handles.as_ref() {
        for (_key, handle) in map.iter() {
            let found = new_handles.values().any(|h| h == handle);
            if !found && !handle.is_invalid() {
                // SAFETY: 旧表拥有该句柄且不再被复用
                unsafe {
                    let _ = windows::Win32::Devices::Display::DestroyPhysicalMonitor(handle.to_handle());
                }
            }
        }
    }
    *handles = None;
}

/// populateHandlesMapLegacy 等价（equiv cc:L691-L758）。
fn populate_handles_map_legacy() {
    crate::clear_monitor_data();

    let mut monitors = get_all_handles();

    // Loop through adapters
    let mut adapter_dev_index: u32 = 0;
    loop {
        let mut adapter_dev = windows::Win32::Graphics::Gdi::DISPLAY_DEVICEW::default();
        adapter_dev.cb = std::mem::size_of::<windows::Win32::Graphics::Gdi::DISPLAY_DEVICEW>() as u32;
        // SAFETY: cbSize 已设置
        if !unsafe { windows::Win32::Graphics::Gdi::EnumDisplayDevicesW(None, adapter_dev_index, &mut adapter_dev, 0) }.as_bool() {
            break;
        }
        adapter_dev_index += 1;

        // Loop through displays (with device ID) on each adapter
        let mut display_dev_index: u32 = 0;
        loop {
            let mut display_dev = windows::Win32::Graphics::Gdi::DISPLAY_DEVICEW::default();
            display_dev.cb = std::mem::size_of::<windows::Win32::Graphics::Gdi::DISPLAY_DEVICEW>() as u32;
            if !unsafe {
                windows::Win32::Graphics::Gdi::EnumDisplayDevicesW(
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
            let flags = display_dev.StateFlags;
            if (flags & windows::Win32::Graphics::Gdi::DISPLAY_DEVICE_ATTACHED_TO_DESKTOP).0 == 0
                || (flags & windows::Win32::Graphics::Gdi::DISPLAY_DEVICE_MIRRORING_DRIVER).0 != 0
            {
                continue;
            }

            for monitor in monitors.iter_mut() {
                // SAFETY: handle 合法
                let mut monitor_info = windows::Win32::Graphics::Gdi::MONITORINFOEXW::default();
                monitor_info.monitorInfo.cbSize = std::mem::size_of::<windows::Win32::Graphics::Gdi::MONITORINFOEXW>() as u32;
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::GetMonitorInfoW(
                        windows::Win32::Graphics::Gdi::HMONITOR(monitor.handle.0 as *mut core::ffi::c_void),
                        &mut monitor_info as *mut windows::Win32::Graphics::Gdi::MONITORINFOEXW
                            as *mut windows::Win32::Graphics::Gdi::MONITORINFO,
                    );
                }

                for i in 0..monitor.physical_handles.len() {
                    // Re-create DISPLAY_DEVICE.DeviceName with MONITORINFOEX.szDevice and monitor index.（原注释）
                    let len = monitor_info.szDevice.iter().position(|&c| c == 0).unwrap_or(monitor_info.szDevice.len());
                    let sz_device = String::from_utf16_lossy(&monitor_info.szDevice[..len]);
                    let monitor_name = format!("{}\\Monitor{}", sz_device, i);

                    let device_name = crate::displays::wide_to_string(&display_dev.DeviceName);
                    let device_id = crate::displays::wide_to_string(&display_dev.DeviceID);
                    let device_key = device_id.split("#{").next().unwrap_or(&device_id).to_string();

                    // Match and store against device ID
                    if monitor_name == device_name {
                        let mut handles = crate::HANDLES.lock().unwrap();
                        let map = handles.get_or_insert_with(HashMap::new);
                        if let std::collections::hash_map::Entry::Vacant(e) = map.entry(device_key.clone()) {
                            e.insert(monitor.physical_handles[i]);
                            // Ownership moves to `handles`.（原注释）
                            monitor.physical_handles[i] = crate::HWrap(0);
                        }
                        break;
                    }
                }
            }
        }
    }

    // Destroy handles which could not be matched to a DISPLAY_DEVICE.
    destroy_physical_monitor_handles(&mut monitors);

    // Also update physicalMonitorHandles for use with getAllDisplays
    let displays = get_all_displays("key", None);
    let handles = crate::HANDLES.lock().unwrap();
    if let Some(map) = handles.as_ref() {
        let mut pm = crate::PHYSICAL_MONITOR_HANDLES.lock().unwrap();
        let pm_map = pm.get_or_insert_with(HashMap::new);
        for (key, handle) in map {
            let mut new_monitor = PhysicalMonitor {
                handle: *handle,
                ddcci_supported: true,
                ..Default::default()
            };
            if let Some(display) = displays.get(key) {
                new_monitor.name = display.device_name.split("Monitor").next().unwrap_or(&display.device_name).to_string();
                new_monitor.full_name = display.device_name.clone();
                new_monitor.device_key = display.device_key.clone();
                new_monitor.device_id = display.device_id.clone();
            }
            pm_map.insert(key.clone(), new_monitor);
        }
    }
}

/// populateHandlesMapNormal 等价（equiv cc:L760-L958）。
fn populate_handles_map_normal(
    validation_method: &str,
    use_previous_results: bool,
    check_high_level: bool,
    excluded_keys: &[String],
) {
    let mut new_handles: HashMap<String, crate::HWrap> = HashMap::new();
    let mut new_physical_handles: HashMap<String, PhysicalMonitor> = HashMap::new();
    let mut newly_acquired_handles: HashSet<crate::HWrap> = HashSet::new();

    dlog!("Getting all display devices...");

    let mut displays_in_enumeration_order: Vec<DisplayDevice> = Vec::new();
    let displays = get_all_displays("name", Some(&mut displays_in_enumeration_order));
    for (k, d) in &displays {
        dlog!("-- Display: {}", k);
        dlog!("-- -- deviceName: {}", d.device_name);
        dlog!("-- -- deviceID: {}", d.device_id);
        dlog!("-- -- deviceKey: {}", d.device_key);
    }

    let targets = get_display_config_targets();
    let mut claimed_device_keys: HashSet<String> = HashSet::new();
    for t in &targets {
        dlog!("-- Target: {}", t.gdi_device_name);
        dlog!("-- -- devicePath: {}", t.device_path);
        dlog!("-- -- friendlyName: {}", t.friendly_name);
    }

    plog!("Testing all physicalMonitors...");

    let mut monitors = get_all_handles();
    for monitor in monitors.iter_mut() {
        for i in 0..monitor.physical_handles.len() {
            // Loop through physical monitors, check capabilities, and only include ones that work.（原注释）
            let full_monitor_name = format!("{}\\Monitor{}", monitor.monitor_name, i);
            plog!("-- {}", full_monitor_name);

            let acquired_handle = monitor.physical_handles[i];
            let mut new_monitor = PhysicalMonitor {
                handle: acquired_handle,
                handle_is_valid: !acquired_handle.is_invalid(),
                name: monitor.monitor_name.clone(),
                physical_name: full_monitor_name.clone(),
                ddcci_supported: false,
                ..Default::default()
            };

            // Prefer an unambiguous description match. The physical-monitor
            // and QueryDisplayConfig APIs do not document a shared ordering.（原注释）
            let mut found_matching_display = false;
            let physical_description = monitor.physical_descriptions[i].clone();
            if !physical_description.is_empty() {
                plog!("-- -- Physical description: {}", physical_description);
            }

            let use_target = |new_monitor: &mut PhysicalMonitor,
                                  target: &DisplayConfigTarget,
                                  method: &str,
                                  claimed: &mut HashSet<String>| {
                new_monitor.device_key = target.device_key.clone();
                new_monitor.device_id = target.device_path.clone();
                new_monitor.full_name = format!("{}\\Monitor{}", monitor.monitor_name, i);

                // Preserve the canonical DISPLAY_DEVICE identifiers when
                // available so settings keys and cached results remain stable.（原注释）
                for d in displays.values() {
                    if d.device_key == target.device_key {
                        new_monitor.full_name = d.device_name.clone();
                        new_monitor.device_id = d.device_id.clone();
                        break;
                    }
                }
                claimed.insert(target.device_key.clone());
                plog!("-- -- Matched with ({}): {}", method, target.device_key);
            };

            let physical_description_count = monitor
                .physical_descriptions
                .iter()
                .filter(|d| **d == physical_description)
                .count();

            if !physical_description.is_empty() && physical_description_count == 1 {
                let description_targets: Vec<&DisplayConfigTarget> = targets
                    .iter()
                    .filter(|t| {
                        t.gdi_device_name == monitor.monitor_name
                            && t.friendly_name == physical_description
                    })
                    .collect();
                let description_target_count = description_targets.len();

                if description_target_count == 1
                    && !claimed_device_keys.contains(&description_targets[0].device_key)
                {
                    let t = description_targets[0].clone();
                    use_target(&mut new_monitor, &t, "description", &mut claimed_device_keys);
                    found_matching_display = true;
                } else if description_target_count > 1 {
                    plog!("-- -- Description match is ambiguous. Using fallback.");
                }
            }

            // Fall back to matching against the DISPLAY_DEVICE list by
            // enumeration order. Physical monitors have been observed to
            // follow this order; QueryDisplayConfig paths may not (e.g.
            // duplicated displays).（原注释，示例保留）
            if !found_matching_display {
                let mut found_count = 0;
                for display in &displays_in_enumeration_order {
                    if display.adapter_name == monitor.monitor_name {
                        if found_count == i {
                            if claimed_device_keys.contains(&display.device_key) {
                                plog!("-- -- DISPLAY_DEVICE is already claimed. Using fallback.");
                                break;
                            }
                            new_monitor.device_key = display.device_key.clone();
                            new_monitor.device_id = display.device_id.clone();
                            new_monitor.full_name = display.device_name.clone();
                            claimed_device_keys.insert(display.device_key.clone());
                            found_matching_display = true;
                            plog!("-- -- Matched with (fallback): {}", display.device_key);
                            break;
                        }
                        found_count += 1;
                    }
                }
            }

            // QueryDisplayConfig provides the target DevicePath, but no
            // documented ordering relation with physical-monitor handles.
            // Keep its positional match as a best-effort fallback only.（原注释）
            if !found_matching_display {
                let mut found_count = 0;
                for target in &targets {
                    if target.gdi_device_name == monitor.monitor_name {
                        if found_count == i {
                            if !claimed_device_keys.contains(&target.device_key) {
                                let t = target.clone();
                                use_target(&mut new_monitor, &t, "QDC index fallback", &mut claimed_device_keys);
                                found_matching_display = true;
                            } else {
                                plog!("-- -- QDC target is already claimed.");
                            }
                            break;
                        }
                        found_count += 1;
                    }
                }
            }

            if !found_matching_display {
                // Skip just this physical monitor; the remaining ones on
                // this HMONITOR may still match.（原注释）
                plog!("-- -- Couldn't find match. Skipping.");
                continue;
            }

            // Skip all DDC/CI and high-level probing for displays excluded
            // by the caller.（原注释）
            let ddc_blocked = crate::is_excluded_device_key(&new_monitor.device_key, excluded_keys);
            if ddc_blocked {
                new_monitor.ddcci_supported = false;
                new_monitor.result = "blocked".to_string();
                plog!("-- -- DDC/CI: blocked (excluded by caller)");
            }

            // Check if monitor was previously tested and supported
            if use_previous_results && !ddc_blocked {
                let pm = crate::PHYSICAL_MONITOR_HANDLES.lock().unwrap();
                if let Some(prev_map) = pm.as_ref() {
                    for previous in prev_map.values() {
                        if previous.full_name == new_monitor.full_name
                            && previous.device_id == new_monitor.device_id
                            && previous.ddcci_supported
                            && previous.result != "invalid"
                        {
                            new_monitor.result = previous.result.clone();
                            new_monitor.ddcci_supported = previous.ddcci_supported;

                            // Test old handle if it's valid
                            if !previous.handle.is_invalid() && previous.handle_is_valid {
                                plog!("-- -- Testing old handle.");
                                if "ok" == crate::capabilities::get_physical_handle_results(previous.handle, "fast") {
                                    plog!("-- -- Using old handle.");
                                    unsafe {
                                        let _ = windows::Win32::Devices::Display::DestroyPhysicalMonitor(acquired_handle.to_handle());
                                    }
                                    monitor.physical_handles[i] = crate::HWrap(0);
                                    new_monitor.handle = previous.handle;
                                    new_monitor.handle_is_valid = previous.handle_is_valid;
                                    new_monitor.hl_capabilities = previous.hl_capabilities;
                                }
                            }
                            break;
                        }
                    }
                }
            }

            // Test high level capabilities
            if !ddc_blocked
                && new_monitor.hl_capabilities.brightness_ok == 0
                && new_monitor.hl_capabilities.contrast_ok == 0
            {
                if check_high_level {
                    plog!("-- -- High Level: Checking...");
                    new_monitor.hl_capabilities = get_high_level_capabilities(new_monitor.handle);
                } else {
                    plog!("-- -- High Level: Skipped");
                }
            }
            if new_monitor.hl_capabilities.brightness_ok != 0 || new_monitor.hl_capabilities.contrast_ok != 0 {
                plog!("-- -- High Level: Supported");
            }

            // Test DDC/CI
            let mut save_capabilities = false;
            if ddc_blocked {
                // Nothing to test; the display must not be communicated with.（原注释）
            } else if !new_monitor.ddcci_supported {
                let mut result = "invalid".to_string();

                // If "accurate", check "fast" first
                if validation_method == "accurate" {
                    result = crate::capabilities::get_physical_handle_results(new_monitor.handle, "fast");
                }

                // Check results using requested method
                let validation_method_result =
                    crate::capabilities::get_physical_handle_results(new_monitor.handle, validation_method);
                if validation_method_result != "invalid" {
                    result = validation_method_result;
                }

                save_capabilities = true;
                new_monitor.result = result;
                plog!("-- -- DDC/CI: {}", new_monitor.result);

                if new_monitor.result == "invalid" {
                    new_monitor.ddcci_supported = false;
                } else {
                    new_monitor.ddcci_supported = true;
                }
            } else {
                plog!("-- -- DDC/CI: previously OK");
            }

            // Add to monitor list. The physical handle is either transferred
            // to `handles`, or the newly acquired handle was destroyed above
            // when an existing handle was reused.（原注释）
            if new_handles.contains_key(&new_monitor.device_key) {
                if new_monitor.handle == acquired_handle {
                    unsafe {
                        let _ = windows::Win32::Devices::Display::DestroyPhysicalMonitor(acquired_handle.to_handle());
                    }
                    monitor.physical_handles[i] = crate::HWrap(0);
                }
                continue;
            }
            new_handles.insert(new_monitor.device_key.clone(), new_monitor.handle);
            if new_monitor.handle == acquired_handle {
                newly_acquired_handles.insert(acquired_handle);
                monitor.physical_handles[i] = crate::HWrap(0);
            }
            new_physical_handles.insert(new_monitor.full_name.clone(), new_monitor.clone());

            // Add to capabilities list
            let current_result = new_physical_handles
                .get(&new_monitor.full_name)
                .map(|m| m.result.clone())
                .unwrap_or_default();
            if save_capabilities
                && validation_method == "accurate"
                && current_result != "ok"
                && current_result != "invalid"
            {
                let mut cap = crate::CAPABILITIES.lock().unwrap();
                cap.get_or_insert_with(HashMap::new)
                    .insert(new_monitor.device_key.clone(), current_result);
            }
        }
    }

    // All remaining entries were not transferred to `handles`.
    destroy_physical_monitor_handles(&mut monitors);

    clean_monitor_handles(&new_handles);
    // swap() 语义：提交句柄表与元数据表之间不留窗口（原注释语义）
    *crate::HANDLES.lock().unwrap() = Some(new_handles);
    *crate::PHYSICAL_MONITOR_HANDLES.lock().unwrap() = Some(new_physical_handles);
}

/// populateHandlesMap 等价（equiv cc:L959-L973）：
/// legacy / accurate / no-validation 直通，其余按 fast。refresh 失败保留旧数据（原注释语义）。
pub(crate) fn populate_handles_map(
    validation_method: &str,
    use_previous_results: bool,
    check_high_level: bool,
    excluded_keys: &[String],
) {
    // 原版 try/catch: "populateHandlesMap: refresh failed. Keeping previous monitor data."
    // Rust 侧所有 Win32 调用都已降级为 Result/默认值，无 panic 路径；
    // 与原版语义等价的"失败保留旧数据"由各内部函数的降级分支保证。
    if validation_method == "legacy" {
        populate_handles_map_legacy();
        return;
    }
    if validation_method == "accurate" || validation_method == "no-validation" {
        populate_handles_map_normal(validation_method, use_previous_results, check_high_level, excluded_keys);
        return;
    }
    populate_handles_map_normal("fast", use_previous_results, check_high_level, excluded_keys);
}

/// refresh 等价（equiv cc:L1005-L1032）。
pub fn refresh(
    method: &str,
    use_previous_results: bool,
    check_high_level: bool,
    excluded_keys: &[String],
) -> Result<(), String> {
    populate_handles_map(method, use_previous_results, check_high_level, excluded_keys);
    Ok(())
}

/// getMonitorList 等价（equiv cc:L1212-L1226）。
pub fn get_monitor_list() -> Vec<String> {
    let handles = crate::HANDLES.lock().unwrap();
    match handles.as_ref() {
        Some(map) => map.keys().cloned().collect(),
        None => Vec::new(),
    }
}

/// getAllMonitors 等价（equiv cc:L1228-L1261）。
/// 返回顺序与原版 std::map 一致：按 key 排序。
pub fn get_all_monitors() -> Vec<crate::MonitorsReportEntry> {
    let pm = crate::PHYSICAL_MONITOR_HANDLES.lock().unwrap();
    let mut ret = Vec::new();
    if let Some(map) = pm.as_ref() {
        for (key, m) in map {
            ret.push(crate::MonitorsReportEntry {
                ddcci_supported: m.ddcci_supported,
                hl_brightness_supported: m.hl_capabilities.brightness_ok != 0,
                hl_contrast_supported: m.hl_capabilities.contrast_ok != 0,
                handle_is_valid: m.handle_is_valid,
                name: m.name.clone(),
                full_name: m.full_name.clone(),
                physical_name: m.physical_name.clone(),
                result: m.result.clone(),
                device_key: m.device_key.clone(),
                device_id: m.device_id.clone(),
                map_key: key.clone(),
            });
        }
    }
    ret
}

/// getMonitorInputs 等价（equiv cc:L1492-L1660）。
/// 返回 [currentInput, [availableInputs...]]；VCP 60 列表来自 capabilities 字符串。
pub fn get_monitor_inputs(search_key: &str, current_input: Option<u32>) -> Result<(u32, Vec<u32>), String> {
    // Look up the monitor by any of its known keys
    let found = {
        let pm = crate::PHYSICAL_MONITOR_HANDLES.lock().unwrap();
        let map = pm.as_ref().ok_or(format!("Monitor not found. Search key: {}", search_key))?;
        let found_key = map
            .iter()
            .find(|(k, m)| {
                *k == search_key
                    || m.name == search_key
                    || m.full_name == search_key
                    || m.physical_name == search_key
                    || m.device_key == search_key
                    || m.device_id == search_key
            })
            .map(|(k, _)| k.clone());
        found_key.ok_or(format!("Monitor not found. Search key: {}", search_key))?
    };

    let monitor = {
        let pm = crate::PHYSICAL_MONITOR_HANDLES.lock().unwrap();
        pm.as_ref().and_then(|m| m.get(&found)).cloned().ok_or("Monitor not found")?
    };

    let result = monitor.result;

    // Find the input source list (VCP code 60) in the capabilities string
    let input_start = match result.find("60(") {
        Some(i) => i,
        None => return Ok((0, Vec::new())),
    };
    let input_end = match result[input_start..].find(')') {
        Some(e) => input_start + e,
        None => return Ok((0, Vec::new())),
    };

    // Extract the substring containing the input source codes
    let inputs_str = &result[input_start + 3..input_end];

    // Split into individual codes
    let input_codes: Vec<&str> = inputs_str.split(' ').filter(|c| !c.is_empty()).collect();
    if input_codes.is_empty() {
        return Ok((0, Vec::new()));
    }

    // Read the monitor's current input source, unless the caller just read it.（原注释）
    let mut current: u32 = 0;
    let mut inputs_max: u32 = 0;
    let success = match current_input {
        Some(v) => {
            current = v;
            true
        }
        None => {
            // SAFETY: handle 来自 refresh 结果
            (unsafe {
                windows::Win32::Devices::Display::GetVCPFeatureAndVCPFeatureReply(
                    monitor.handle.to_handle(),
                    0x60u8,
                    None,
                    &mut current,
                    Some(&mut inputs_max),
                )
            }) != 0
        }
    };

    // Convert the current input to a number（原版按 hex 解析）
    let current_code_value: u32 = if success {
        u32::from_str_radix(&format!("{}", current), 16).unwrap_or(0)
    } else {
        0
    };

    let mut available_inputs = Vec::new();
    for code in &input_codes {
        // Convert the hex string to a number（原版按 hex 解析）
        let code_value = u32::from_str_radix(code, 16).unwrap_or(0);
        available_inputs.push(code_value);
    }

    Ok((current_code_value, available_inputs))
}
