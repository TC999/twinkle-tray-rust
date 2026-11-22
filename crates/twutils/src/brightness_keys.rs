//! Raw Input 亮度键捕获。
//!
//! equiv: windows_brightness_keys.cc（198 行）。

use windows::Win32::Devices::HumanInterfaceDevice::{
    HidP_GetCaps, HidP_GetUsages, HidP_MaxUsageListLength, HIDP_CAPS, HIDP_STATUS_SUCCESS,
    PHIDP_PREPARSED_DATA,
};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::UI::Input::{
    GetRawInputData, GetRawInputDeviceInfoW, RegisterRawInputDevices, RAWINPUT, RAWINPUTDEVICE,
    RAWINPUTHEADER, RIDI_PREPARSEDDATA, RID_INPUT, RIDEV_INPUTSINK, RIDEV_REMOVE, RIM_TYPEHID,
};
use windows::Win32::Foundation::HWND;

const CONSUMER_USAGE_PAGE: u16 = 0x0C;
const CONSUMER_CONTROL_USAGE: u16 = 0x01;
const BRIGHTNESS_INCREMENT_USAGE: u16 = 0x006F;
const BRIGHTNESS_DECREMENT_USAGE: u16 = 0x0070;

/// 最后发送亮度键的设备。只有它的报告能释放该键（原注释语义）。
static HELD_KEY_DEVICE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// register 等价。
pub fn register(hwnd: HWND) -> bool {
    let device = RAWINPUTDEVICE {
        usUsagePage: CONSUMER_USAGE_PAGE,
        usUsage: CONSUMER_CONTROL_USAGE,
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: hwnd,
    };
    // SAFETY: device 数组与 size 匹配
    unsafe { RegisterRawInputDevices(&[device], std::mem::size_of::<RAWINPUTDEVICE>() as u32) }.is_ok()
}

/// unregister 等价。
pub fn unregister() -> bool {
    let device = RAWINPUTDEVICE {
        usUsagePage: CONSUMER_USAGE_PAGE,
        usUsage: CONSUMER_CONTROL_USAGE,
        dwFlags: RIDEV_REMOVE,
        hwndTarget: HWND::default(),
    };
    // SAFETY: device 数组与 size 匹配
    unsafe { RegisterRawInputDevices(&[device], std::mem::size_of::<RAWINPUTDEVICE>() as u32) }.is_ok()
}

/// ReadBrightnessKey 等价：返回 "up"/"down"/"release"/"other"/""。
pub fn read_brightness_key(input_handle: HANDLE) -> String {
    // SAFETY: GetRawInputData 两步调用；缓冲区按返回大小分配
    unsafe {
        let hrawinput = windows::Win32::UI::Input::HRAWINPUT(input_handle.0);
        let mut input_size: u32 = 0;
        if GetRawInputData(
            hrawinput,
            RID_INPUT,
            None,
            &mut input_size,
            std::mem::size_of::<RAWINPUTHEADER>() as u32,
        ) == u32::MAX
            || (input_size as usize) < std::mem::size_of::<RAWINPUTHEADER>()
        {
            return String::new();
        }

        let mut input_buffer: Vec<u8> = vec![0u8; input_size as usize];
        let result = GetRawInputData(
            hrawinput,
            RID_INPUT,
            Some(input_buffer.as_mut_ptr() as *mut core::ffi::c_void),
            &mut input_size,
            std::mem::size_of::<RAWINPUTHEADER>() as u32,
        );
        if result == u32::MAX {
            return String::new();
        }

        let input = &*(input_buffer.as_ptr() as *const RAWINPUT);
        if input.header.dwType != RIM_TYPEHID.0 || input.header.hDevice.is_invalid() {
            return String::new();
        }

        let mut preparsed_size: u32 = 0;
        if GetRawInputDeviceInfoW(
            Some(input.header.hDevice),
            RIDI_PREPARSEDDATA,
            None,
            &mut preparsed_size,
        ) == u32::MAX
            || preparsed_size == 0
        {
            return String::new();
        }

        let mut preparsed_buffer: Vec<u8> = vec![0u8; preparsed_size as usize];
        if GetRawInputDeviceInfoW(
            Some(input.header.hDevice),
            RIDI_PREPARSEDDATA,
            Some(preparsed_buffer.as_mut_ptr() as *mut core::ffi::c_void),
            &mut preparsed_size,
        ) == u32::MAX
        {
            return String::new();
        }

        let preparsed_data = PHIDP_PREPARSED_DATA(preparsed_buffer.as_ptr() as isize);
        let mut capabilities: HIDP_CAPS = HIDP_CAPS::default();
        if HidP_GetCaps(preparsed_data, &mut capabilities) != HIDP_STATUS_SUCCESS
            || capabilities.UsagePage != CONSUMER_USAGE_PAGE
            || capabilities.Usage != CONSUMER_CONTROL_USAGE
        {
            return String::new();
        }

        let max_usage_count = HidP_MaxUsageListLength(
            windows::Win32::Devices::HumanInterfaceDevice::HidP_Input,
            Some(CONSUMER_USAGE_PAGE),
            preparsed_data,
        );
        if max_usage_count == 0 {
            return String::new();
        }

        let mut usages: Vec<u16> = vec![0; max_usage_count as usize];

        // RAWINPUT 的 data.hid 区段: 与 C++ 不同，windows crate 的 RAWINPUT 把 bRawData
        // 表示为可变长尾部。头长度 = RAWINPUTHEADER + RAWHID 头之前的部分。
        let header_size = std::mem::size_of::<RAWINPUTHEADER>();
        // RAWHID 结构: dwSizeHid + dwCount + bRawData
        let hid_header_size = std::mem::size_of::<u32>() * 2;
        let data_offset = header_size + hid_header_size;
        if (input_size as usize) < data_offset {
            return String::new();
        }
        let dw_size_hid = *(input_buffer.as_ptr().add(header_size) as *const u32);
        let dw_count = *(input_buffer.as_ptr().add(header_size + 4) as *const u32);
        let raw_data_size = dw_size_hid as usize * dw_count as usize;
        if dw_size_hid == 0 || dw_count == 0 || data_offset > input_buffer.len()
            || raw_data_size > input_buffer.len() - data_offset
        {
            return String::new();
        }

        for report_index in 0..dw_count {
            let mut usage_count = max_usage_count;
            let report_slice = std::slice::from_raw_parts_mut(
                input_buffer.as_mut_ptr().add(data_offset + (report_index as usize * dw_size_hid as usize)),
                dw_size_hid as usize,
            );
            let usage_status = HidP_GetUsages(
                windows::Win32::Devices::HumanInterfaceDevice::HidP_Input,
                CONSUMER_USAGE_PAGE,
                Some(0),
                usages.as_mut_ptr(),
                &mut usage_count,
                preparsed_data,
                report_slice,
            );
            if usage_status != HIDP_STATUS_SUCCESS {
                continue;
            }
            for usage_index in 0..usage_count as usize {
                if usages[usage_index] == BRIGHTNESS_INCREMENT_USAGE
                    || usages[usage_index] == BRIGHTNESS_DECREMENT_USAGE
                {
                    HELD_KEY_DEVICE.store(
                        input.header.hDevice.0 as usize,
                        std::sync::atomic::Ordering::SeqCst,
                    );
                    return if usages[usage_index] == BRIGHTNESS_INCREMENT_USAGE {
                        "up".to_string()
                    } else {
                        "down".to_string()
                    };
                }
            }
        }

        // A report from the same device without the key means it was let go:
        // either the zero-usage release report, or another key on that device
        // replacing it. Anything else is unrelated to the held key.（原注释）
        let held = HELD_KEY_DEVICE.load(std::sync::atomic::Ordering::SeqCst);
        if held != 0 && input.header.hDevice.0 as usize == held {
            HELD_KEY_DEVICE.store(0, std::sync::atomic::Ordering::SeqCst);
            return "release".to_string();
        }
        "other".to_string()
    }
}
