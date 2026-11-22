//! Apple Studio Display USB 亮度控制（WinUSB 直调）。
//!
//! equiv: Monitors.js:L1005-L1019（nits 换算常量）、L1020-L1025（makeStudioDisplayRequestData）、
//! L1067-L1077（USB controlTransfer 读/写）、L1027-L1083（getFallbackStudioDisplays）。
//!
//! AGENTS.md §1.2：不引入 nusb，用 Win32_Devices_Usb 的
//! SetupDiGetClassDevsW + WinUsb_Initialize + WinUsb_ControlTransfer 等价移植。
//! 常量（vendor 0x05ac / product 0x1118 / interface 7 / report id 0x01 / nits 400-60000）逐字保留。
//!
//! 降级语义：任何一层失败都返回 Err，由调用方记录日志并继续（绝不 panic）。

use crate::settings_state::{
    percent_to_studio_display_nits, STUDIO_DISPLAY_BRIGHTNESS_INTERFACE, STUDIO_DISPLAY_PRODUCT_IDS,
    STUDIO_DISPLAY_REPORT_ID, STUDIO_DISPLAY_VENDOR_ID,
};

/// makeStudioDisplayRequestData 等价（equiv L1020-L1025）：
/// 7 字节报文，byte0 = report id，byte1..3 = nits 的 UInt16LE。
pub fn make_request_data(percent: f64) -> [u8; 7] {
    let nits = percent_to_studio_display_nits(percent).round();
    let nits = nits.clamp(0.0, 65535.0) as u16;
    let mut bytes = [0u8; 7];
    bytes[0] = STUDIO_DISPLAY_REPORT_ID;
    let le = nits.to_le_bytes();
    bytes[1] = le[0];
    bytes[2] = le[1];
    bytes
}

/// 解析响应报文（equiv `response.readUInt16LE(1)`）。
pub fn parse_response_nits(response: &[u8]) -> Option<u16> {
    if response.len() < 3 {
        return None;
    }
    Some(u16::from_le_bytes([response[1], response[2]]))
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use windows::core::PCWSTR;
    use windows::Win32::Devices::Usb::{
        WinUsb_ControlTransfer, WinUsb_Free, WinUsb_Initialize, WINUSB_INTERFACE_HANDLE,
        WINUSB_SETUP_PACKET,
    };
    // 注：windows 0.62 的 WinUsb_ControlTransfer 签名以 Option<&mut [u8]> 传缓冲区，
    // 由绑定层推导 bufferlength；与 C 的 (buffer, length) 双参一一对应。
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
        SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
        SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
    };
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    /// 请求类型常量（equiv L1034-L1035）。
    const REQUEST_TYPE: u8 = 0x20 | 0x01;
    const REPORT_VALUE: u16 = 0x0300 | STUDIO_DISPLAY_REPORT_ID as u16;

    /// device interface GUID：USB 设备接口 {A5DCBF10-6530-11D2-901F-00C04FB951ED}。
    const GUID_DEVINTERFACE_USB_DEVICE: windows::core::GUID =
        windows::core::GUID::from_u128(0xa5dcbf10_6530_11d2_901f_00c04fb951ed);

    /// 打开的显示器 USB 句柄组（等价 withBrightnessInterface 的 open/claim/finally 语义）。
    struct StudioHandle {
        file: HANDLE,
        winusb: WINUSB_INTERFACE_HANDLE,
    }

    impl Drop for StudioHandle {
        fn drop(&mut self) {
            // SAFETY: winusb/file 均由成功初始化路径产出，配对释放
            unsafe {
                if !self.winusb.is_invalid() {
                    let _ = WinUsb_Free(self.winusb);
                }
                let _ = CloseHandle(self.file);
            }
        }
    }

    /// 枚举匹配 vendor/product 的 USB 设备路径（equiv usb.getDeviceList().filter(...)）。
    fn enumerate_device_paths() -> Vec<String> {
        let mut paths = Vec::new();
        // SAFETY: SetupAPI 生命周期在本函数内闭合
        unsafe {
            let dev_info = match SetupDiGetClassDevsW(
                Some(&GUID_DEVINTERFACE_USB_DEVICE),
                PCWSTR::null(),
                None,
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            ) {
                Ok(h) => h,
                Err(_) => return paths,
            };
            let mut index = 0u32;
            loop {
                let mut iface = SP_DEVICE_INTERFACE_DATA {
                    cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                    ..Default::default()
                };
                if SetupDiEnumDeviceInterfaces(dev_info, None, &GUID_DEVINTERFACE_USB_DEVICE, index, &mut iface).is_err() {
                    break;
                }
                index += 1;

                let mut required: u32 = 0;
                let _ = SetupDiGetDeviceInterfaceDetailW(dev_info, &iface, None, 0, Some(&mut required), None);
                if required == 0 {
                    continue;
                }
                let mut buffer = vec![0u8; required as usize];
                let detail = buffer.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
                (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
                if SetupDiGetDeviceInterfaceDetailW(dev_info, &iface, Some(detail), required, None, None).is_err() {
                    continue;
                }
                let path_ptr = (*detail).DevicePath.as_ptr();
                let mut len = 0usize;
                while *path_ptr.add(len) != 0 {
                    len += 1;
                }
                let path = String::from_utf16_lossy(std::slice::from_raw_parts(path_ptr, len));
                paths.push(path);
            }
            let _ = SetupDiDestroyDeviceInfoList(dev_info);
        }
        paths
    }

    /// 打开并初始化 WinUSB 接口（equiv dev.open() + interface(7).claim()）。
    fn open_handle(device_path: &str) -> Option<StudioHandle> {
        let wide: Vec<u16> = device_path.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: 句柄成对关闭（Drop）
        unsafe {
            let file = CreateFileW(
                PCWSTR::from_raw(wide.as_ptr()),
                0x80000000 | 0x40000000, // GENERIC_READ | GENERIC_WRITE
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
            .ok()?;
            let mut winusb = WINUSB_INTERFACE_HANDLE::default();
            if WinUsb_Initialize(file, &mut winusb).is_err() {
                let _ = CloseHandle(file);
                return None;
            }
            Some(StudioHandle { file, winusb })
        }
    }

    /// USB control transfer（equiv controlTransfer.call(display, ...)）。
    fn control_transfer(
        handle: &StudioHandle,
        request_type: u8,
        request: u8,
        buffer: &mut [u8],
    ) -> Option<usize> {
        let packet = WINUSB_SETUP_PACKET {
            RequestType: request_type,
            Request: request,
            Value: REPORT_VALUE,
            Index: STUDIO_DISPLAY_BRIGHTNESS_INTERFACE as u16,
            Length: buffer.len() as u16,
        };
        let mut transferred: u32 = 0;
        // SAFETY: buffer 与 handle 均有效；transferred 为合法出参
        let result = unsafe {
            WinUsb_ControlTransfer(
                handle.winusb,
                packet,
                Some(buffer),
                Some(&mut transferred),
                None,
            )
        };
        if result.is_err() {
            return None;
        }
        Some(transferred as usize)
    }

    /// 匹配 Studio Display 的设备路径（equiv getDeviceList().filter(vid/pid)）。
    /// 设备接口路径形如 \\?\usb#vid_05ac&pid_1118#<serial>#{guid}。
    pub fn enumerate_matching_device_paths() -> Vec<String> {
        enumerate_device_paths()
            .into_iter()
            .filter(|path| {
                let upper = path.to_uppercase();
                let vid_ok = upper.contains(&format!("VID_{:04X}", STUDIO_DISPLAY_VENDOR_ID));
                let pid_ok = STUDIO_DISPLAY_PRODUCT_IDS
                    .iter()
                    .any(|pid| upper.contains(&format!("PID_{:04X}", pid)));
                vid_ok && pid_ok
            })
            .collect()
    }

    /// 从设备路径提取序列号（equiv getStringDescriptor(iSerialNumber)）。
    /// USB 设备接口路径的第三段即序列号；取不到时返回空串（原版 undefined）。
    pub fn read_serial(device_path: &str) -> Option<String> {
        // \\?\usb#vid_xxxx&pid_xxxx#<serial>#{interface-guid}
        let trimmed = device_path.trim_start_matches("\\\\?\\").trim_start_matches('\\');
        let parts: Vec<&str> = trimmed.split('#').collect();
        if parts.len() < 3 {
            return None;
        }
        let serial = parts[2];
        if serial.is_empty() {
            return None;
        }
        Some(serial.to_string())
    }

    /// 通过序列号定位对应设备（serial 为空时取第一台匹配设备）。
    fn find_handle(serial: &str) -> Option<StudioHandle> {
        for path in enumerate_matching_device_paths() {
            if !serial.is_empty() && !path.contains(serial) {
                continue;
            }
            if let Some(handle) = open_handle(&path) {
                return Some(handle);
            }
        }
        None
    }

    /// 读当前亮度（nits）。equiv L1067-L1072：
    /// controlTransfer(0x80 | requestType, 1, reportValue, interface, 7)。
    pub fn get_brightness_nits(serial: &str) -> Result<i32, String> {
        let handle = find_handle(serial).ok_or("Studio Display is not in the known display cache.")?;
        let mut buffer = [0u8; 7];
        let transferred = control_transfer(&handle, 0x80 | REQUEST_TYPE, 1, &mut buffer)
            .ok_or("Studio Display brightness read failed")?;
        if transferred == 0 {
            return Err("Studio Display brightness read returned no data".to_string());
        }
        let nits = parse_response_nits(&buffer).ok_or("Studio Display response too short")?;
        Ok(nits as i32)
    }

    /// 写亮度。equiv L1073-L1077：controlTransfer(requestType, 9, reportValue, interface, data)。
    pub fn set_brightness_nits(serial: &str, percent: f64) -> Result<bool, String> {
        let handle = find_handle(serial).ok_or("Studio Display is not in the known display cache.")?;
        let mut payload = make_request_data(percent);
        control_transfer(&handle, REQUEST_TYPE, 9, &mut payload)
            .map(|_| true)
            .ok_or_else(|| "Studio Display brightness write failed".to_string())
    }
}

#[cfg(target_os = "windows")]
pub use platform::{get_brightness_nits as get_brightness, set_brightness_nits as set_brightness};

#[cfg(not(target_os = "windows"))]
pub fn get_brightness(_serial: &str) -> Result<i32, String> {
    Err("Studio Display control is Windows-only".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn set_brightness(_serial: &str, _percent: f64) -> Result<bool, String> {
    Err("Studio Display control is Windows-only".to_string())
}


/// 发现的 Studio Display 设备（equiv getFallbackStudioDisplays 的元素）。
#[derive(Debug, Clone, Default)]
pub struct StudioDisplayDevice {
    /// 序列号（equiv getSerialNumber()）
    pub serial: String,
    /// 设备路径（打开 WinUSB 句柄时使用）
    pub device_path: String,
}

/// 枚举 Studio Display 设备（equiv L1027-L1083 getFallbackStudioDisplays）。
/// existing_serials: 已存在的序列号集合，命中的设备按原版语义返回空序列号（调用方跳过）。
/// 无匹配设备时返回空 Vec；任何失败返回 Err 由调用方记录日志后继续。
pub fn discover(existing_serials: &[String]) -> Result<Vec<StudioDisplayDevice>, String> {
    #[cfg(target_os = "windows")]
    {
        let mut out = Vec::new();
        for path in platform::enumerate_matching_device_paths() {
            let serial = platform::read_serial(&path).unwrap_or_default();
            // getSerialNumber(): 已存在则返回 undefined（原版语义）
            let serial = if existing_serials.iter().any(|s| s == &serial) {
                String::new()
            } else {
                serial
            };
            out.push(StudioDisplayDevice {
                serial,
                device_path: path,
            });
        }
        Ok(out)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = existing_serials;
        Err("Studio Display discovery is Windows-only".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_data_matches_js() {
        // percent 0 -> nits 400 -> 0x0190 LE = [0x90, 0x01]
        let bytes = make_request_data(0.0);
        assert_eq!(bytes[0], STUDIO_DISPLAY_REPORT_ID);
        assert_eq!(u16::from_le_bytes([bytes[1], bytes[2]]), 400);
        // percent 100 -> nits 60000 -> 0xEA60 LE = [0x60, 0xEA]
        let bytes = make_request_data(100.0);
        assert_eq!(u16::from_le_bytes([bytes[1], bytes[2]]), 60000);
        // 尾随字节保持 0
        assert_eq!(&bytes[3..], &[0, 0, 0, 0]);
    }

    #[test]
    fn response_parse_reads_le_u16_at_offset_one() {
        let mut response = [0u8; 7];
        response[1] = 0x60;
        response[2] = 0xEA;
        assert_eq!(parse_response_nits(&response), Some(60000));
        assert_eq!(parse_response_nits(&[0, 0]), None);
    }
}
