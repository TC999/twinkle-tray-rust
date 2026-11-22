//! node-ddcci 等价移植。
//!
//! 源模块: `origin/src/modules/node-ddcci/ddcci.cc`（1666 行）+ index.js（167 行）。
//! 所有函数保留原 JS/C++ 命名，便于真机对拍逐行对照（AGENTS.md §5）。

pub mod capabilities;
pub mod displays;
pub mod enumerate;
pub mod handles;
pub mod vcp;

pub use capabilities::{get_capabilities_string_impl, parse_capabilities_string};
pub use handles::{get_all_monitors, get_monitor_inputs, get_monitor_list, refresh};
pub use vcp::{
    get_vcp, set_vcp, get_high_level_brightness, set_high_level_brightness,
    get_high_level_contrast, set_high_level_contrast, save_current_settings, set_log_level,
};

use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Mutex;

/// equiv cc:L22-L27 struct Monitor。
#[derive(Debug, Default, Clone)]
pub(crate) struct Monitor {
    pub handle: crate::HWrap,
    pub monitor_name: String,
    pub physical_handles: Vec<crate::HWrap>,
    pub physical_descriptions: Vec<String>,
}

/// equiv cc:L29-L41 struct MonitorHighLevel。
/// capabilities 字段对应 C++ 的 GetMonitorCapabilities（原版已注释禁用，字段保留等价）。
#[derive(Debug, Default, Clone, Copy)]
#[allow(dead_code)]
pub(crate) struct MonitorHighLevel {
    pub capabilities: u32,
    pub capabilities_ok: u32,
    pub supported_temperatures: u32,
    pub brightness: u32,
    pub brightness_min: u32,
    pub brightness_max: u32,
    pub brightness_ok: u32,
    pub contrast: u32,
    pub contrast_min: u32,
    pub contrast_max: u32,
    pub contrast_ok: u32,
}

/// equiv cc:L43-L54 struct PhysicalMonitor。
#[derive(Debug, Default, Clone)]
pub(crate) struct PhysicalMonitor {
    pub handle: crate::HWrap,
    pub handle_is_valid: bool,
    pub ddcci_supported: bool,
    pub name: String,
    pub full_name: String,
    pub physical_name: String,
    pub result: String,
    pub device_key: String,
    pub device_id: String,
    pub hl_capabilities: MonitorHighLevel,
}

/// equiv cc:L56-L61 struct DisplayDevice。
#[derive(Debug, Default, Clone)]
pub(crate) struct DisplayDevice {
    pub adapter_name: String,
    pub device_name: String,
    pub device_id: String,
    pub device_key: String,
}

/// equiv cc:L370-L375 struct DisplayConfigTarget。
#[derive(Debug, Default, Clone)]
pub(crate) struct DisplayConfigTarget {
    /// e.g. "\\\.\\DISPLAY2"
    pub gdi_device_name: String,
    /// e.g. "\\\\?\\DISPLAY#ABC1234#5&..#{GUID}"
    pub device_path: String,
    /// devicePath up to "#{"
    pub device_key: String,
    pub friendly_name: String,
}

/// equiv cc:L102 logLevel。
static LOG_LEVEL: AtomicI32 = AtomicI32::new(0);

/// setLogLevel 等价（equiv cc:L1479-L1490）。
pub fn set_log_level_impl(level: i32) {
    LOG_LEVEL.store(level, Ordering::SeqCst);
}

pub(crate) fn log_level() -> i32 {
    LOG_LEVEL.load(Ordering::SeqCst)
}

/// Info（equiv cc:L103-L109 p()）。
#[macro_export]
macro_rules! plog {
    ($($arg:tt)*) => {
        if $crate::log_level() >= 1 {
            println!("[node-ddcci] {}", format!($($arg)*));
        }
    };
}

/// Debug（equiv cc:L111-L117 d()）。
#[macro_export]
macro_rules! dlog {
    ($($arg:tt)*) => {
        if $crate::log_level() >= 2 {
            println!("[node-ddcci] {}", format!($($arg)*));
        }
    };
}

/// 全局句柄表。equiv cc:L96 `std::map<std::string, HANDLE> handles;`
/// 不变量: `handles` 拥有每个物理显示器句柄；`physical_monitor_handles` 只存元数据，绝不销毁其副本（原注释语义）。
pub(crate) static HANDLES: Mutex<Option<HashMap<String, crate::HWrap>>> = Mutex::new(None);
pub(crate) static PHYSICAL_MONITOR_HANDLES: Mutex<Option<HashMap<String, PhysicalMonitor>>> = Mutex::new(None);
pub(crate) static CAPABILITIES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// findPhysicalMonitor 等价（equiv cc:L112-L131）。
pub(crate) fn find_physical_monitor(monitor_name: &str) -> Option<String> {
    let guard = PHYSICAL_MONITOR_HANDLES.lock().unwrap();
    let map = guard.as_ref()?;
    for (key, monitor) in map {
        if key == monitor_name
            || monitor.name == monitor_name
            || monitor.full_name == monitor_name
            || monitor.physical_name == monitor_name
            || monitor.device_key == monitor_name
            || monitor.device_id == monitor_name
        {
            return Some(key.clone());
        }
    }
    None
}

/// applyCapabilitiesResult 等价（equiv cc:L134-L146）。
pub(crate) fn apply_capabilities_result(monitor_key: Option<&str>, monitor: &mut PhysicalMonitor, result: &str) {
    if result.is_empty() {
        return;
    }
    monitor.result = result.to_string();
    monitor.ddcci_supported = true;
    if !monitor.device_key.is_empty() {
        let mut cap = CAPABILITIES.lock().unwrap();
        cap.get_or_insert_with(HashMap::new)
            .insert(monitor.device_key.clone(), result.to_string());
    }
    let _ = monitor_key;
}

/// isTransientDdcError 等价（equiv cc:L180-L196）。
/// DDC/CI 乱码或 I2C 总线毛刺值得重试；不支持的 VCP 码或显示器消失等永久性错误不重试（原注释语义）。
pub(crate) fn is_transient_ddc_error(error_code: u32) -> bool {
    use windows::Win32::Foundation::{
        ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_CHECKSUM, ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_COMMAND,
        ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_LENGTH,
        ERROR_GRAPHICS_DDCCI_MONITOR_RETURNED_INVALID_TIMING_STATUS_BYTE,
        ERROR_GRAPHICS_I2C_ERROR_RECEIVING_DATA, ERROR_GRAPHICS_I2C_ERROR_TRANSMITTING_DATA,
    };
    error_code == ERROR_GRAPHICS_I2C_ERROR_TRANSMITTING_DATA.0 as u32
        || error_code == ERROR_GRAPHICS_I2C_ERROR_RECEIVING_DATA.0 as u32
        || error_code == ERROR_GRAPHICS_DDCCI_MONITOR_RETURNED_INVALID_TIMING_STATUS_BYTE.0 as u32
        || error_code == ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_COMMAND.0 as u32
        || error_code == ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_LENGTH.0 as u32
        || error_code == ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_CHECKSUM.0 as u32
}

/// tryDdcCiOperation 等价（equiv cc:L203-L226）：瞬态错误重试，最多 3 次，间隔 50ms。
pub(crate) fn try_ddc_ci_operation<F>(mut operation: F, error_code: &mut u32) -> bool
where
    F: FnMut() -> bool,
{
    const MAX_ATTEMPTS: i32 = 3;
    for attempt in 1..=MAX_ATTEMPTS {
        if attempt > 1 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if operation() {
            return true;
        }
        // SAFETY: GetLastError 仅在失败后调用
        *error_code = unsafe { windows::Win32::Foundation::GetLastError() }.0;
        if !is_transient_ddc_error(*error_code) {
            return false;
        }
        dlog!("Transient DDC/CI error. Attempt #{} failed with 0x{:X}", attempt, *error_code);
    }
    false
}

/// isExcludedDeviceKey 等价（equiv cc:L233-L247）：子串匹配。
pub(crate) fn is_excluded_device_key(device_key: &str, excluded_keys: &[String]) -> bool {
    if device_key.is_empty() {
        return false;
    }
    excluded_keys
        .iter()
        .any(|key| !key.is_empty() && device_key.contains(key.as_str()))
}

/// clearMonitorData 等价（equiv cc:L119-L132）。
pub(crate) fn clear_monitor_data() {
    let mut handles = HANDLES.lock().unwrap();
    if let Some(map) = handles.as_ref() {
        for handle in map.values() {
            if !handle.is_invalid() {
                // SAFETY: handles 拥有全部物理显示器句柄
                unsafe {
                    let _ = windows::Win32::Devices::Display::DestroyPhysicalMonitor(handle.to_handle());
                }
            }
        }
    }
    *handles = None;
    *PHYSICAL_MONITOR_HANDLES.lock().unwrap() = None;
    *CAPABILITIES.lock().unwrap() = None;
}

/// clearDisplayCache 等价（equiv cc:L1146-L1155）：只清元数据与 capabilities，不动句柄。
pub fn clear_display_cache() {
    *PHYSICAL_MONITOR_HANDLES.lock().unwrap() = None;
    *CAPABILITIES.lock().unwrap() = None;
}

/// HANDLE 的可跨线程包装（HANDLE 本质是 usize 句柄值）。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct HWrap(pub usize);

impl HWrap {
    #[allow(dead_code)]
    pub fn from_handle(h: windows::Win32::Foundation::HANDLE) -> Self {
        Self(h.0 as usize)
    }
    pub fn to_handle(&self) -> windows::Win32::Foundation::HANDLE {
        windows::Win32::Foundation::HANDLE(self.0 as *mut core::ffi::c_void)
    }
    pub fn is_invalid(&self) -> bool {
        // NULL 与 INVALID_HANDLE_VALUE(-1) 都视为无效（与 C++ HANDLE==NULL/-1 检查一致）
        self.0 == 0 || self.0 == usize::MAX
    }
}

/// getAllMonitors 的条目结构（等价原版 Napi::Object 字段）。
#[derive(Debug, Clone, Default)]
pub struct MonitorsReportEntry {
    pub ddcci_supported: bool,
    pub hl_brightness_supported: bool,
    pub hl_contrast_supported: bool,
    pub handle_is_valid: bool,
    pub name: String,
    pub full_name: String,
    pub physical_name: String,
    pub result: String,
    pub device_key: String,
    pub device_id: String,
    /// std::map 的 key（原版遍历 physicalMonitorHandles 的键）。
    pub map_key: String,
}

#[cfg(test)]
mod tests {
    #[test]
    fn exclusion_substring_match() {
        assert!(super::is_excluded_device_key(
            "\\\\?\\DISPLAY#CMN1521#5&2cf91c03&0&UID8388688#{e6f5-...}",
            &["UID8388688".to_string()]
        ));
        assert!(!super::is_excluded_device_key("", &["X".to_string()]));
        assert!(!super::is_excluded_device_key("DISPLAY#OTHER", &["".to_string()]));
    }

    #[test]
    fn transient_error_classification() {
        // I2C 发送错误（0xC0262583）值得重试
        assert!(super::is_transient_ddc_error(0xC026_2583));
        // 不存在的错误码不算瞬态
        assert!(!super::is_transient_ddc_error(0xC026_1000 | 0x1000));
    }
}

#[cfg(test)]
mod real_machine_tests {
    /// 真机测试: refresh(fast) + getMonitorList。
    /// 内屏（LVDS/内部面板）通常不响应 DDC/CI，列表可能为空——这是正确行为；
    /// 关键验证点是枚举链路不 panic、句柄管理正常。
    #[test]
    fn ddcci_refresh_smoke() {
        crate::set_log_level_impl(2);
        let result = crate::refresh("fast", false, false, &[]);
        println!("refresh -> {:?}", result);
        let list = crate::get_monitor_list();
        println!("get_monitor_list -> {:?}", list);
        let all = crate::get_all_monitors();
        for m in &all {
            println!(
                "monitor: name={} full={} ddcci={} hl_bright={} result={}",
                m.name, m.full_name, m.ddcci_supported, m.hl_brightness_supported, m.result
            );
        }
    }

    /// parseCapabilitiesString 等价性测试（用样例 capabilities 字符串）。
    #[test]
    fn parse_capabilities_matches_js() {
        // 典型 capabilities 字符串
        let report = "(prot(monitor)type(lcd)model(ART2965)edid(256 165 )),vcp(04 05 10 12(01 05) 60(11 12 0f 04 01) ac b6 )";
        let parsed = crate::parse_capabilities_string(report);
        let map = parsed.expect("应解析成功");
        // 0x10 亮度码存在且无固定值列表
        assert!(map.contains_key("0x10"));
        // 0x60 输入源码有定义值列表
        let inputs = map.get("0x60").expect("0x60 应存在");
        assert_eq!(inputs, &vec![0x11, 0x12, 0x0f, 0x04, 0x01]);
        // 12(01 05) 有两个定义值
        assert_eq!(map.get("0x12").unwrap(), &vec![1, 5]);
        // 小写 vcp( 也能匹配
        let lower = crate::parse_capabilities_string("(type(lcd)),vcp ( 10 12 )");
        assert!(lower.is_some());
        // 无 VCP 列表 -> false/None
        assert!(crate::parse_capabilities_string("(prot(monitor)type(lcd))").is_none());
    }
}
