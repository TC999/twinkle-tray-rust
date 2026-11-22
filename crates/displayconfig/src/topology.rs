//! 显示器启用/禁用与配置恢复。
//!
//! equiv: win32-displayconfig.cc:L305-L415（ToggleEnabled）、L417-L475（RestoreDeviceConfig）、
//! index.js:L296-L460（toggleEnabledDisplays/displayConfigForRestoration/restoreDisplayConfig）。

use crate::extract::extract_display_config;
use crate::{query_display_config, ConfigId, Win32Error};
use windows::Win32::Devices::Display::{
    SetDisplayConfig, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SCALING_PREFERRED,
    SDC_ALLOW_PATH_ORDER_CHANGES, SDC_APPLY, SDC_SAVE_TO_DATABASE, SDC_TOPOLOGY_SUPPLIED,
    SDC_USE_SUPPLIED_DISPLAY_CONFIG, SET_DISPLAY_CONFIG_FLAGS,
};
use windows::Win32::Graphics::Gdi::{DISPLAYCONFIG_PATH_ACTIVE, DISPLAYCONFIG_PATH_MODE_IDX_INVALID};
use windows::Win32::Foundation::ERROR_SUCCESS;

/// equiv index.js ToggleEnabledDisplaysArgs。
#[derive(Debug, Clone, Default)]
pub struct ToggleEnabledDisplaysArgs {
    /// 要启用的显示器 NT 设备路径。
    pub enable: Vec<String>,
    /// 要禁用的显示器 NT 设备路径。
    pub disable: Vec<String>,
    /// 是否保存为默认配置。
    pub persistent: bool,
}

/// 恢复配置条目。等价 `DisplayRestorationConfigurationEntry`；
/// buffer 以原始字节保存（对外 base64 由调用方处理）。
#[derive(Debug, Clone)]
pub struct DisplayRestorationConfig {
    pub device_path: String,
    pub path_buffer: Vec<u8>,
    pub source_mode_buffer: Vec<u8>,
    pub target_mode_buffer: Vec<u8>,
}

/// 恢复参数。等价 `RestoreDisplayConfigArgs`。
#[derive(Debug, Clone)]
pub struct RestoreDisplayConfigArgs {
    pub config: Vec<DisplayRestorationConfig>,
    pub persistent: bool,
}

fn as_err(code: i32) -> Win32Error {
    Win32Error::new(code)
}

/// 把 struct 以逐字节形式拷出（等价原版 Napi::Buffer::Copy 的语义）。
fn struct_bytes<T>(v: &T) -> Vec<u8> {
    // SAFETY: T 为 repr(C) 的 POD 结构体，按原始字节读取
    unsafe {
        std::slice::from_raw_parts(v as *const T as *const u8, std::mem::size_of::<T>()).to_vec()
    }
}

/// 从逐字节还原 struct（长度必须精确匹配，等价原版 memcpy_s 前的长度校验）。
fn struct_from_bytes<T>(bytes: &[u8]) -> Option<T> {
    if bytes.len() != std::mem::size_of::<T>() {
        return None;
    }
    // SAFETY: T 为 repr(C) POD，字节数精确匹配
    Some(unsafe { std::ptr::read(bytes.as_ptr() as *const T) })
}

fn toggle_enabled_impl(
    enable: &[ConfigId],
    disable: &[ConfigId],
    persistent: bool,
) -> Result<(), Win32Error> {
    let initial = query_display_config()?;
    let mut preserve: Vec<DISPLAYCONFIG_PATH_INFO> = Vec::new();
    let mut already_enabled: Vec<ConfigId> = Vec::new();

    let contains = |v: &[ConfigId], c: &ConfigId| v.iter().any(|x| x == c);

    // 第一遍：已启用的设备先保底（不清模式索引）。
    for (i, p) in initial.paths.iter().enumerate() {
        let cid = ConfigId {
            adapter_id: crate::AdapterId::from_luid(&p.targetInfo.adapterId),
            id: p.targetInfo.id,
        };
        let v = initial.path_value(i);
        if v.source_mode_info_idx == DISPLAYCONFIG_PATH_MODE_IDX_INVALID
            || v.target_mode_info_idx == DISPLAYCONFIG_PATH_MODE_IDX_INVALID
        {
            continue;
        }
        if (p.flags & DISPLAYCONFIG_PATH_ACTIVE) == DISPLAYCONFIG_PATH_ACTIVE {
            let mut copied = *p;
            copied.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
            copied.sourceInfo.statusFlags = 0;
            copied.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
            copied.targetInfo.statusFlags = 0;
            preserve.push(copied);
            already_enabled.push(cid);
        }
    }

    // 第二遍：启用尚未启用的目标设备。
    for (i, p) in initial.paths.iter().enumerate() {
        let cid = ConfigId {
            adapter_id: crate::AdapterId::from_luid(&p.targetInfo.adapterId),
            id: p.targetInfo.id,
        };
        let v = initial.path_value(i);
        if v.source_mode_info_idx == DISPLAYCONFIG_PATH_MODE_IDX_INVALID {
            continue;
        }
        if contains(enable, &cid) && !contains(&already_enabled, &cid) {
            let mut copied = *p;
            copied.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
            copied.sourceInfo.statusFlags = 0;
            copied.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
            copied.targetInfo.statusFlags = 0;
            copied.targetInfo.scaling = DISPLAYCONFIG_SCALING_PREFERRED;
            copied.flags = DISPLAYCONFIG_PATH_ACTIVE;
            preserve.push(copied);
            already_enabled.push(cid);
        }
    }

    // 先全开（Windows 必须先 on 再关）。
    // SAFETY: preserve 中的 path 均来自 QueryDisplayConfig 的合法返回
    let error = unsafe {
        SetDisplayConfig(
            Some(&preserve),
            None,
            SDC_APPLY | SDC_TOPOLOGY_SUPPLIED | SDC_ALLOW_PATH_ORDER_CHANGES,
        )
    };
    if error != ERROR_SUCCESS.0 as i32 {
        return Err(as_err(error));
    }

    let all_on = query_display_config()?;
    preserve.clear();

    // 最后关掉不需要的。
    for (i, p) in all_on.paths.iter().enumerate() {
        let cid = ConfigId {
            adapter_id: crate::AdapterId::from_luid(&p.targetInfo.adapterId),
            id: p.targetInfo.id,
        };
        let v = all_on.path_value(i);
        if v.source_mode_info_idx == DISPLAYCONFIG_PATH_MODE_IDX_INVALID
            || v.target_mode_info_idx == DISPLAYCONFIG_PATH_MODE_IDX_INVALID
        {
            continue;
        }
        if (p.flags & DISPLAYCONFIG_PATH_ACTIVE) == DISPLAYCONFIG_PATH_ACTIVE
            && !contains(disable, &cid)
        {
            let mut copied = *p;
            copied.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
            copied.sourceInfo.statusFlags = 0;
            copied.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
            copied.targetInfo.statusFlags = 0;
            preserve.push(copied);
        }
    }

    let error = unsafe {
        SetDisplayConfig(
            Some(&preserve),
            None,
            SDC_APPLY | SDC_TOPOLOGY_SUPPLIED | SDC_ALLOW_PATH_ORDER_CHANGES,
        )
    };
    if error != ERROR_SUCCESS.0 as i32 {
        return Err(as_err(error));
    }

    if !persistent {
        return Ok(());
    }

    // persistent: 把 Windows 给出的配置原样还回去保存。
    let persisted = query_display_config()?;
    let error = unsafe {
        SetDisplayConfig(
            Some(&persisted.paths),
            Some(&persisted.modes),
            SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_SAVE_TO_DATABASE,
        )
    };
    if error != ERROR_SUCCESS.0 as i32 {
        Err(as_err(error))
    } else {
        Ok(())
    }
}

fn restore_device_config(
    configs: &[(ConfigId, ConfigId, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_MODE_INFO)],
    persistent: bool,
) -> Result<(), Win32Error> {
    let mut rg_path_info: Vec<DISPLAYCONFIG_PATH_INFO> = Vec::new();
    let mut rg_mode_info: Vec<DISPLAYCONFIG_MODE_INFO> = Vec::new();
    let mut dw_mode_info_offset: u32 = 0;

    for (_i, (source_id, target_id, path_info, source_mode, target_mode)) in configs.iter().enumerate() {
        let mut path_copy = *path_info;
        let mut source_copy = *source_mode;
        let mut target_copy = *target_mode;

        // 源 id 不改（跨重启持久，且承载主/副屏语义）；adapterId 与目标 id 需修正。
        path_copy.sourceInfo.adapterId = source_id.adapter_id.to_luid();
        path_copy.targetInfo.adapterId = target_id.adapter_id.to_luid();
        path_copy.targetInfo.id = target_id.id;

        path_copy.sourceInfo.Anonymous.modeInfoIdx = dw_mode_info_offset;
        dw_mode_info_offset += 1;
        path_copy.targetInfo.Anonymous.modeInfoIdx = dw_mode_info_offset;
        dw_mode_info_offset += 1;

        source_copy.adapterId = source_id.adapter_id.to_luid();
        target_copy.adapterId = target_id.adapter_id.to_luid();
        target_copy.id = target_id.id;

        rg_path_info.push(path_copy);
        rg_mode_info.push(source_copy);
        rg_mode_info.push(target_copy);
    }

    let persist_flag = if persistent { SDC_SAVE_TO_DATABASE } else { SET_DISPLAY_CONFIG_FLAGS(0) };

    // SAFETY: path/mode 数组自洽（modeInfoIdx 已重排）
    let error = unsafe {
        SetDisplayConfig(
            Some(&rg_path_info),
            Some(&rg_mode_info),
            SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | persist_flag,
        )
    };
    if error != ERROR_SUCCESS.0 as i32 {
        Err(as_err(error))
    } else {
        Ok(())
    }
}

/// equiv index.js:L296-L335 toggleEnabledDisplays。
pub fn toggle_enabled_displays(args: &ToggleEnabledDisplaysArgs) -> Result<(), Win32Error> {
    let display_config = extract_display_config()?;
    let mut enable: Vec<ConfigId> = Vec::new();
    let mut disable: Vec<ConfigId> = Vec::new();

    for entry in &display_config {
        if args.enable.iter().any(|p| p == &entry.device_path) {
            enable.push(entry.target_config_id);
        }
        if args.disable.iter().any(|p| p == &entry.device_path) {
            disable.push(entry.target_config_id);
        }
    }

    toggle_enabled_impl(&enable, &disable, args.persistent)
}

/// equiv index.js:L389-L417 displayConfigForRestoration。
/// 需要 QueryResults 的原始 struct；为避免二次查询，这里独立取一次。
pub fn display_config_for_restoration() -> Result<Vec<DisplayRestorationConfig>, Win32Error> {
    let current = crate::convert::do_query_display_config()?;
    let mut ret = Vec::new();

    for (i, p) in current.paths.iter().enumerate() {
        let v = current.path_value(i);
        if (p.flags & DISPLAYCONFIG_PATH_ACTIVE) != DISPLAYCONFIG_PATH_ACTIVE {
            continue;
        }
        let target_idx = v.target_mode_info_idx as usize;
        if current.modes.get(target_idx).is_none() {
            continue;
        }
        let source_idx = v.source_mode_info_idx as usize;
        let source_mode = match current.modes.get(source_idx) {
            Some(m) => m,
            None => continue,
        };
        let target_mode = current.modes[target_idx];

        // 找到对应名字（devicePath）
        let entry = current.names.iter().find(|n| {
            n.adapter_id == v.target_adapter_id
                && n.id == v.target_id
                && !n.output_technology.is_empty()
                && !n.monitor_device_path.is_empty()
        });
        let device_path = match entry {
            Some(e) => e.monitor_device_path.clone(),
            None => continue,
        };

        ret.push(DisplayRestorationConfig {
            device_path,
            path_buffer: struct_bytes(p),
            source_mode_buffer: struct_bytes(source_mode),
            target_mode_buffer: struct_bytes(&target_mode),
        });
    }
    Ok(ret)
}

/// equiv index.js:L437-L460 + L462-L540 restoreDisplayConfig 的完整分支逻辑。
pub fn restore_display_config(args: &RestoreDisplayConfigArgs) -> Result<(), Win32Error> {
    if args.config.is_empty() {
        // 原版: throw TypeError；对拍语义等价为 Win32Error(87)（ERROR_INVALID_PARAMETER）
        return Err(Win32Error::new(87));
    }

    let device_path_names: Vec<&str> = args
        .config
        .iter()
        .filter(|c| !c.target_mode_buffer.is_empty() || !c.target_mode_buffer.is_empty())
        .map(|c| c.device_path.as_str())
        .collect();
    let current_config = extract_display_config()?;

    let given_as_set: std::collections::HashSet<&str> =
        current_config.iter().map(|e| e.device_path.as_str()).collect();
    let expected_enabled_as_set: std::collections::HashSet<&str> =
        device_path_names.iter().copied().collect();

    let missing_enabled: Vec<&str> = expected_enabled_as_set
        .difference(&given_as_set)
        .copied()
        .collect();

    if missing_enabled.is_empty() {
        // 分支一：期望启用的显示器当前都存在 => 用当前 configId + 保存的 buffer 直接应用。
        let mut coerced: Vec<(ConfigId, ConfigId, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_MODE_INFO)> = Vec::new();
        for entry in &args.config {
            let current_entry = current_config
                .iter()
                .find(|e| e.device_path == entry.device_path);
            let current_entry = match current_entry {
                Some(e) => e,
                None => continue,
            };
            let path_info: DISPLAYCONFIG_PATH_INFO =
                match struct_from_bytes(&entry.path_buffer) {
                    Some(v) => v,
                    None => return Err(Win32Error::new(87)),
                };
            let source_mode: DISPLAYCONFIG_MODE_INFO =
                match struct_from_bytes(&entry.source_mode_buffer) {
                    Some(v) => v,
                    None => return Err(Win32Error::new(87)),
                };
            let target_mode: DISPLAYCONFIG_MODE_INFO =
                match struct_from_bytes(&entry.target_mode_buffer) {
                    Some(v) => v,
                    None => return Err(Win32Error::new(87)),
                };
            coerced.push((
                current_entry.source_config_id,
                current_entry.target_config_id,
                path_info,
                source_mode,
                target_mode,
            ));
        }
        if coerced.is_empty() {
            return Err(Win32Error::new(87));
        }
        restore_device_config(&coerced, args.persistent)
    } else {
        // 分支二：有期望启用但当前不存在的显示器 => 走 enable/disable 开关路径。
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut enable: Vec<String> = Vec::new();
        let mut disable: Vec<String> = Vec::new();

        let not_in_use: Vec<&str> = args
            .config
            .iter()
            .filter(|c| c.target_mode_buffer.is_empty())
            .map(|c| c.device_path.as_str())
            .collect();

        for name in &device_path_names {
            if !seen.contains(name) && given_as_set.contains(name) {
                enable.push(name.to_string());
                seen.insert(name);
            }
        }
        for name in not_in_use {
            if !seen.contains(name) && given_as_set.contains(name) {
                disable.push(name.to_string());
                seen.insert(name);
            }
        }

        toggle_enabled_displays(&ToggleEnabledDisplaysArgs {
            enable,
            disable,
            persistent: args.persistent,
        })
    }
}
