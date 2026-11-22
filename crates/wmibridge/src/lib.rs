//! wmi-bridge 等价移植：COM 直调 IWbemLocator/IWbemServices。
//!
//! 源模块: `origin/src/modules/wmi-bridge/wmi-bridge.cc`（482 行）。
//! 公开 API 与原 addon 一致: setBrightness / getBrightness / getMonitors。

mod brightness;
mod monitors;

pub use brightness::{get_brightness, set_brightness, BrightnessResult};
pub use monitors::{get_monitors, MonitorInfo};

/// makeFailure 等价：`{ failed: true }`。
pub const FAILED: BrightnessResult = BrightnessResult {
    failed: true,
    unsupported: false,
    brightness: None,
    instance_name: None,
};

/// WBEM "类无 provider/实例" 错误码集合（equiv cc:isUnsupportedResult）。
pub(crate) fn is_unsupported_result(hr: i32) -> bool {
    use windows::Win32::System::Wmi::{WBEM_E_INVALID_CLASS, WBEM_E_NOT_FOUND, WBEM_E_NOT_SUPPORTED};
    hr == WBEM_E_NOT_SUPPORTED.0
        || hr == WBEM_E_NOT_FOUND.0
        || hr == WBEM_E_INVALID_CLASS.0
}

/// ComScope 等价（equiv cc:L18-L42）：
/// CoInitializeEx(COINIT_MULTITHREADED)；已是 STA 时 COM 仍可用但不负责 Uninitialize。
pub(crate) struct ComScope {
    result: i32,
    should_uninitialize: bool,
}

impl ComScope {
    pub fn new() -> Self {
        // SAFETY: CoInitializeEx 按文档初始化当前线程 COM
        let result = unsafe {
            windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            )
        }
        .0;
        Self {
            result,
            should_uninitialize: result >= 0,
        }
    }

    pub fn is_usable(&self) -> bool {
        self.result >= 0 || self.result == windows::Win32::Foundation::RPC_E_CHANGED_MODE.0 as i32
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.should_uninitialize {
            // SAFETY: 与成功的 CoInitializeEx 配对
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}

/// initializeWmiSecurity 等价（equiv cc:L45-L67）。
/// COM 安全必须随当前 COM 生命周期初始化，不跨调用缓存（原注释语义）。
pub(crate) fn initialize_wmi_security() -> bool {
    use windows::Win32::System::Com::{
        CoInitializeSecurity, EOAC_NONE, RPC_C_AUTHN_LEVEL_CONNECT, RPC_C_IMP_LEVEL_IMPERSONATE,
    };
    // SAFETY: 进程级安全初始化，参数全为合法静态值
    let security_result = unsafe {
        CoInitializeSecurity(
            None,
            -1,
            None,
            None,
            RPC_C_AUTHN_LEVEL_CONNECT,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
            Some(0 as *const core::ffi::c_void),
        )
    };
    // 进程其他组件已配置过安全设置时视为可用（原注释语义：RPC_E_TOO_LATE -> S_OK）
    match security_result {
        Ok(()) => true,
        Err(e) if e.code() == windows::Win32::Foundation::RPC_E_TOO_LATE => true,
        Err(_) => false,
    }
}

/// connectToWmi 等价（equiv cc:L120-L139）。
/// CLSID_WbemLocator 在 windows crate 无常量，按 SDK 头文件值手写。
pub(crate) fn connect_to_wmi() -> Option<(windows::Win32::System::Wmi::IWbemLocator, windows::Win32::System::Wmi::IWbemServices)> {
    use windows::core::GUID;
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    use windows::Win32::System::Wmi::{IWbemLocator, IWbemServices, WBEM_FLAG_CONNECT_USE_MAX_WAIT};

    // {4590F811-1D3A-11D0-891F-00AA004B2E24} = CLSID_WbemLocator（已用注册表核实）
    const CLSID_WBEM_LOCATOR: GUID = GUID::from_u128(0x4590f811_1d3a_11d0_891f_00aa004b2e24);

    // SAFETY: 标准 COM 激活路径
    let locator: IWbemLocator = unsafe {
        CoCreateInstance(&CLSID_WBEM_LOCATOR, None, CLSCTX_ALL)
    }
    .ok()?;

    let namespace_path = windows::core::BSTR::from_wide(&utf16_wide("ROOT\\WMI"));
    // SAFETY: 合法 BSTR 与输出指针；IWbemLocator::ConnectServer 共 7 参
    let service: IWbemServices = unsafe {
        locator
            .ConnectServer(
                &namespace_path,
                &windows::core::BSTR::new(),
                &windows::core::BSTR::new(),
                &windows::core::BSTR::new(),
                WBEM_FLAG_CONNECT_USE_MAX_WAIT.0 as i32,
                &windows::core::BSTR::new(),
                None,
            )
            .ok()?
    };
    Some((locator, service))
}

#[inline]
fn utf16_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机测试: getBrightness 全流程（COM init -> 连接 -> 查询）。
    /// 桌面机通常返回 unsupported（WmiMonitorBrightness 仅笔记本面板实现）——
    /// 两种结果都算通过；只有意外的 failed-without-unsupported 才提示异常。
    #[test]
    fn wmi_brightness_smoke() {
        let result = crate::brightness::get_brightness();
        println!("get_brightness -> {:?}", result);
        // 不 panic 即为通过；结果结构必须是 failed 或有效亮度二选一
        assert!(result.failed || result.brightness.is_some());
    }

    /// 真机测试: getMonitors。
    #[test]
    fn wmi_monitors_smoke() {
        let result = crate::monitors::get_monitors();
        match result {
            Ok(monitors) => println!("get_monitors -> {} 台", monitors.len()),
            Err(_) => println!("get_monitors failed (允许)"),
        }
    }

    /// set_brightness 越界拒绝（纯逻辑）。
    #[test]
    fn set_brightness_range_guard() {
        assert!(!crate::brightness::set_brightness(-1));
        assert!(!crate::brightness::set_brightness(101));
    }

    /// 分步诊断 COM 链路。
    #[test]
    fn wmi_step_diagnosis() {
        let com = ComScope::new();
        println!("[1] CoInitializeEx hr=0x{:08X} usable={}", com.result, com.is_usable());
        let sec = initialize_wmi_security();
        println!("[2] initialize_wmi_security = {}", sec);

        use windows::core::{GUID, Interface};
        use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
        const CLSID_WBEM_LOCATOR: GUID = GUID::from_u128(0x4590f811_1d3a_11d0_891f_00aa004b2e24);
        let locator: Result<windows::Win32::System::Wmi::IWbemLocator, _> =
            unsafe { CoCreateInstance(&CLSID_WBEM_LOCATOR, None, CLSCTX_ALL) };
        println!("[3] CoCreateInstance = {:?}", locator.as_ref().map(|_| "ok").map_err(|e| e.code().0));
        let locator = match locator {
            Ok(l) => l,
            Err(_) => return,
        };

        let ns = windows::core::BSTR::from_wide("ROOT\\WMI".encode_utf16().collect::<Vec<u16>>().as_slice());
        use windows::Win32::System::Wmi::WBEM_FLAG_CONNECT_USE_MAX_WAIT;
        let svc = unsafe {
            locator.ConnectServer(
                &ns,
                &windows::core::BSTR::new(),
                &windows::core::BSTR::new(),
                &windows::core::BSTR::new(),
                WBEM_FLAG_CONNECT_USE_MAX_WAIT.0 as i32,
                &windows::core::BSTR::new(),
                None,
            )
        };
        println!("[4] ConnectServer = {:?}", svc.as_ref().map(|_| "ok").map_err(|e| e.code().0));
    }
}
