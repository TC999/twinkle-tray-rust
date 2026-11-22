//! tt-windows-utils 等价移植。
//!
//! 源模块: `origin/src/modules/tt-windows-utils`（index.js 58 行 + 6 个 .cc 共 1166 行）。
//! 六个子模块与原 addon 的 bindings 一一对应。

/// 等价 wait_for(timeout) 的轮询实现（windows-future 0.3 无原生超时等待）。
/// Completed -> true；Started 超时 -> false；Error/其它 -> false。
pub(crate) fn wait_with_timeout<T: windows::core::RuntimeType + 'static>(
    operation: &windows_future::IAsyncOperation<T>,
    timeout: std::time::Duration,
) -> bool {
    let start = std::time::Instant::now();
    loop {
        match operation.Status() {
            Ok(windows_future::AsyncStatus::Completed) => return true,
            Ok(windows_future::AsyncStatus::Started) => {
                if start.elapsed() >= timeout {
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            _ => return false,
        }
    }
}

pub mod app_startup;
pub mod brightness_keys;
pub mod display_brightness;
pub mod media_status;
pub mod power_events;
pub mod window_material;
pub mod window_utils;

#[cfg(test)]
mod tests {
    use windows::Win32::Foundation::HWND;

    /// 真机测试: gamma ramp 软件亮度全流程（读 -> 设 -> 读回）。
    #[test]
    fn gamma_brightness_roundtrip() {
        let display = "\\\\.\\DISPLAY1";
        let original = crate::display_brightness::get_brightness(display);
        println!("get_brightness({}) -> {}", display, original);
        // 内屏必有 gamma 通道：读数应在 20-100 或 -1（无 DC）
        if original >= 0 {
            assert!((20..=100).contains(&original), "亮度应钳制在 20..100: {}", original);
            let ok = crate::display_brightness::set_brightness(display, 50.0);
            println!("set_brightness(50) -> {}", ok);
            if ok {
                let after = crate::display_brightness::get_brightness(display);
                println!("get_brightness after set -> {}", after);
                assert!((45..=55).contains(&after), "读回应接近 50: {}", after);
            }
        }
    }

    /// 真机测试: 电源设置注册（NULL 窗口 -> false，等价原版 hwnd==NULL 分支）。
    #[test]
    fn power_events_register_invalid() {
        assert!(!crate::power_events::register_power_setting_notifications(HWND::default()));
    }

    /// 真机测试: 亮度键注册（message-only 窗口为 NULL -> false）。
    #[test]
    fn brightness_keys_invalid() {
        assert!(!crate::brightness_keys::register(HWND::default()));
    }

    /// getPowerSetting 空指针 -> None（等价原版 early return）。
    #[test]
    fn power_setting_null_pointer() {
        assert!(crate::power_events::get_power_setting(0).is_none());
    }
}
