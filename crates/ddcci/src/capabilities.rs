//! capabilities 字符串获取与句柄校验。
//!
//! equiv: ddcci.cc:L562-L644（getCapabilitiesString）、L646-L690（getPhysicalHandleResults）。

use crate::dlog;
use windows::Win32::Devices::Display::{
    CapabilitiesRequestAndCapabilitiesReply, GetCapabilitiesStringLength, GetVCPFeatureAndVCPFeatureReply,
};


/// getCapabilitiesString 等价（equiv cc:L562-L644）。
/// capabilities 检查非常不稳定，长度 3 次重试（100ms 间隔）、内容 5 次重试——逐字保留（原注释语义）。
pub(crate) fn get_capabilities_string(handle: crate::HWrap) -> String {
    if handle.is_invalid() {
        dlog!("No handle for capabilities string!");
        return String::new();
    }

    // Get the capabilities string length.
    // Checking the capabilities string is, apparently, very flaky.
    // So if it fails, we should try again.（原注释）
    let mut cch_string_length: u32 = 0;
    let mut b_success = false;
    let mut attempt = 0;
    while !b_success && attempt < 3 {
        if attempt > 1 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        attempt += 1;
        // SAFETY: 出参指针合法
        b_success =
            unsafe { GetCapabilitiesStringLength(handle.to_handle(), &mut cch_string_length) } != 0;
        dlog!("== cLength attempt #{}: {}", attempt, b_success as i32);
    }

    if !b_success {
        dlog!("Couldn't get capabilities length!");
        return String::new(); // Does not respond to DDC/CI
    }

    dlog!("== cLength: {}", cch_string_length);

    if cch_string_length == 0 {
        return String::new();
    }

    // Allocate the string buffer.
    let mut capabilities_buffer: Vec<u8> = vec![0u8; cch_string_length as usize];

    // Get the capabilities string.
    // We know it exists, so we'll try a few times if needed.（原注释）
    let mut attempt = 0;
    let mut b_success = false;
    while !b_success && attempt < 5 {
        if attempt > 1 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        attempt += 1;
        // SAFETY: 缓冲区长度由 GetCapabilitiesStringLength 决定
        b_success = unsafe {
            CapabilitiesRequestAndCapabilitiesReply(handle.to_handle(), &mut capabilities_buffer)
        } != 0;
        dlog!("== cString attempt #{}: {}", attempt, b_success as i32);
    }

    if !b_success {
        dlog!("Couldn't get capabilities string!");
        return String::new(); // Does not respond to DDC/CI
    }

    // std::string(capabilitiesBuffer.data()) 语义：到第一个 NUL 为止
    let end = capabilities_buffer
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(capabilities_buffer.len());
    String::from_utf8_lossy(&capabilities_buffer[..end]).into_owned()
}

/// getPhysicalHandleResults 等价（equiv cc:L646-L690）。
/// 返回 "invalid"、"ok" 或 capabilities 字符串。
pub(crate) fn get_physical_handle_results(handle: crate::HWrap, validation_method: &str) -> String {
    if validation_method == "no-validation" {
        return "ok".to_string();
    }

    // Accurate method: Check capabilities string
    if validation_method == "accurate" {
        let result = get_capabilities_string(handle);
        if result.is_empty() {
            return "invalid".to_string();
        }
        return result;
    }

    // Fast method: Check common VCP codes
    let mut current_value: u32 = 0;
    let mut max_value: u32 = 0;

    // 0x02, New Control Value
    let ok = unsafe {
        GetVCPFeatureAndVCPFeatureReply(
            handle.to_handle(),
            0x02,
            None,
            &mut current_value,
            Some(&mut max_value),
        )
    };
    if ok != 0 {
        return "ok".to_string();
    }
    // 0xDF, VCP Version
    let ok = unsafe {
        GetVCPFeatureAndVCPFeatureReply(
            handle.to_handle(),
            0xDF,
            None,
            &mut current_value,
            Some(&mut max_value),
        )
    };
    if ok != 0 {
        return "ok".to_string();
    }
    // 0x10, Brightness (usually)
    let ok = unsafe {
        GetVCPFeatureAndVCPFeatureReply(
            handle.to_handle(),
            0x10,
            None,
            &mut current_value,
            Some(&mut max_value),
        )
    };
    if ok != 0 {
        return "ok".to_string();
    }

    "invalid".to_string()
}

/// getCapabilitiesString 公开版：带缓存（equiv cc:L1160-L1210 getNAPICapabilitiesString）。
pub fn get_capabilities_string_impl(monitor_name: &str) -> Result<String, String> {
    let physical_monitor_key = crate::find_physical_monitor(monitor_name);
    let cache_key = {
        let guard = crate::PHYSICAL_MONITOR_HANDLES.lock().unwrap();
        match physical_monitor_key.as_deref().and_then(|k| guard.as_ref().and_then(|m| m.get(k))) {
            Some(pm) if !pm.device_key.is_empty() => pm.device_key.clone(),
            _ => monitor_name.to_string(),
        }
    };

    // Check if it's already saved in memory first.
    {
        let cap = crate::CAPABILITIES.lock().unwrap();
        if let Some(map) = cap.as_ref() {
            if let Some(found) = map.get(&cache_key) {
                return Ok(found.clone());
            }
        }
    }

    // Find requested monitor.
    let handle = {
        let handles = crate::HANDLES.lock().unwrap();
        let map = handles.as_ref().ok_or("Monitor not found")?;
        map.get(&cache_key)
            .or_else(|| map.get(monitor_name))
            .copied()
            .ok_or("Monitor not found")?
    };

    let return_string = get_capabilities_string(handle);
    if return_string.is_empty() {
        return Err("Monitor not responding.".to_string()); // Does not respond to DDC/CI
    }

    // A capabilities request can be performed after a fast discovery pass.
    // Persist it in both native caches（原注释语义）。
    {
        let mut pm_guard = crate::PHYSICAL_MONITOR_HANDLES.lock().unwrap();
        if let (Some(key), Some(map)) = (&physical_monitor_key, pm_guard.as_mut()) {
            if let Some(pm) = map.get_mut(key) {
                let result = return_string.clone();
                crate::apply_capabilities_result(Some(key), pm, &result);
            }
        } else if physical_monitor_key.is_none() {
            let mut cap = crate::CAPABILITIES.lock().unwrap();
            cap.get_or_insert_with(std::collections::HashMap::new)
                .insert(cache_key.clone(), return_string.clone());
        }
    }

    Ok(return_string)
}


/// parseCapabilitiesString 等价（equiv node-ddcci/index.js:L91-L166）。
/// 返回 None 等价原版 `false`（未找到 VCP 列表）；空数组等价“接受范围值或无固定值”。
pub fn parse_capabilities_string(report: &str) -> Option<std::collections::HashMap<String, Vec<u32>>> {
    // Find where VCP list starts. Some monitors report "VCP(" or "vcp (",
    // so match case-insensitively and allow whitespace before the parenthesis.（原注释）
    let start_match = regex_find_vcp(report)?;

    let mut layers: i32 = 1;
    let mut output = String::new();
    let mut position = start_match + start_of_paren(report, start_match) + 1;

    // Iterate through report string after the start of the list until we hit the end.
    // We'll check for nested parenthesis to correctly find the end of the list.（原注释）
    let bytes: Vec<char> = report.chars().collect();
    while layers > 0 && position + 1 < bytes.len() {
        let ch = bytes[position];
        if ch == '(' {
            layers += 1;
        } else if ch == ')' {
            layers -= 1;
            if layers <= 0 {
                break;
            }
        }
        output.push(ch);
        position += 1;
    }

    // Strip out unnecessary characters（原注释语义：去 \0 与空格后 trim）
    let output: String = output
        .chars()
        .filter(|c| *c != '\0' && *c != ' ')
        .collect::<String>()
        .trim()
        .to_string();

    // Iterate through the above string, alternating between parsing VCP codes
    // and accepted values as needed（原注释）
    let chars: Vec<char> = output.chars().collect();
    let mut code_list: std::collections::HashMap<String, Vec<u32>> = std::collections::HashMap::new();

    let mut pos: usize = 0;
    while pos < chars.len() {
        let cur = chars[pos];
        pos += 1;

        // Check for VCP codes
        if cur != '(' {
            if pos >= chars.len() {
                break;
            }
            let vcp_code: String = format!("{}{}", cur, chars[pos]);
            let mut vcp_values: Vec<u32> = Vec::new();
            pos += 1;

            // Check for defined values
            if pos < chars.len() && chars[pos] == '(' {
                pos += 1;
                let mut depth = 0;
                while pos < chars.len() && !(chars[pos] == ')' && depth == 0) {
                    // Check for subdata
                    // I don't know what to do with this, so we'll just read through and ignore it.（原注释）
                    if chars[pos] == '(' {
                        pos += 1;
                        depth += 1;
                        while depth > 0 && pos < chars.len() {
                            if chars[pos] == '(' {
                                depth += 1;
                            } else if chars[pos] == ')' {
                                depth -= 1;
                            }
                            pos += 1;
                        }
                    } else {
                        // Push defined value to list
                        if pos + 1 < chars.len() {
                            let hex = format!("{}{}", chars[pos], chars[pos + 1]);
                            if let Ok(v) = u32::from_str_radix(&hex, 16) {
                                vcp_values.push(v);
                            }
                        }
                        pos += 2;
                    }
                }
                pos += 1;
            }
            // Add VCP code with defined values (if found)（原注释）
            code_list.insert(format!("0x{}", vcp_code.to_uppercase()), vcp_values);
        }
    }

    Some(code_list)
}

/// 匹配 /vcp\s*\(/i 的首个位置。
fn regex_find_vcp(report: &str) -> Option<usize> {
    let lower = report.to_lowercase();
    let bytes: Vec<char> = lower.chars().collect();
    let mut i = 0;
    while i + 3 < bytes.len() {
        if bytes[i] == 'v' && bytes[i + 1] == 'c' && bytes[i + 2] == 'p' {
            // 允许空白后跟 (
            let mut j = i + 3;
            while j < bytes.len() && bytes[j].is_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == '(' {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// vcp( 的 '(' 相对 start_match 的偏移（在原字符流中定位真正的 '('）。
fn start_of_paren(report: &str, start: usize) -> usize {
    let chars: Vec<char> = report.chars().collect();
    let mut j = start + 3; // 跳过 "vcp"
    while j < chars.len() && chars[j].is_whitespace() {
        j += 1;
    }
    j.saturating_sub(start + 1) // 相对偏移
}
