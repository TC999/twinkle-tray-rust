//! 电源设置通知。
//!
//! equiv: windows_power_events.cc（166 行）。

use std::sync::Mutex;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Power::{
    RegisterPowerSettingNotification, UnregisterPowerSettingNotification, HPOWERNOTIFY,
    POWERBROADCAST_SETTING,
};
use windows::Win32::System::SystemServices::{
    GUID_CONSOLE_DISPLAY_STATE, GUID_LIDSWITCH_STATE_CHANGE, GUID_MONITOR_POWER_ON,
    GUID_SESSION_DISPLAY_STATUS, GUID_SESSION_USER_PRESENCE, GUID_SLEEP_IDLE_THRESHOLD,
    GUID_STANDBY_TIMEOUT, GUID_SYSTEM_AWAYMODE, GUID_VIDEO_ADAPTIVE_DISPLAY_BRIGHTNESS,
    GUID_VIDEO_ADAPTIVE_PERCENT_INCREASE, GUID_VIDEO_ADAPTIVE_POWERDOWN,
    GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS, GUID_VIDEO_DIM_TIMEOUT, GUID_VIDEO_POWERDOWN_TIMEOUT,
};

static POWER_NOTIFICATIONS: Mutex<Vec<HPOWERNOTIFY>> = Mutex::new(Vec::new());

/// GetPowerSettingName 等价：GUID -> 名字字符串。
fn get_power_setting_name(guid: &windows::core::GUID) -> &'static str {
    if *guid == GUID_CONSOLE_DISPLAY_STATE {
        "GUID_CONSOLE_DISPLAY_STATE"
    } else if *guid == GUID_MONITOR_POWER_ON {
        "GUID_MONITOR_POWER_ON"
    } else if *guid == GUID_SESSION_DISPLAY_STATUS {
        "GUID_SESSION_DISPLAY_STATUS"
    } else if *guid == GUID_SYSTEM_AWAYMODE {
        "GUID_SYSTEM_AWAYMODE"
    } else if *guid == GUID_LIDSWITCH_STATE_CHANGE {
        "GUID_LIDSWITCH_STATE_CHANGE"
    } else if *guid == GUID_SESSION_USER_PRESENCE {
        "GUID_SESSION_USER_PRESENCE"
    } else if *guid == GUID_STANDBY_TIMEOUT {
        "GUID_STANDBY_TIMEOUT"
    } else if *guid == GUID_VIDEO_ADAPTIVE_DISPLAY_BRIGHTNESS {
        "GUID_VIDEO_ADAPTIVE_DISPLAY_BRIGHTNESS"
    } else if *guid == GUID_VIDEO_ADAPTIVE_PERCENT_INCREASE {
        "GUID_VIDEO_ADAPTIVE_PERCENT_INCREASE"
    } else if *guid == GUID_VIDEO_ADAPTIVE_POWERDOWN {
        "GUID_VIDEO_ADAPTIVE_POWERDOWN"
    } else if *guid == GUID_VIDEO_DIM_TIMEOUT {
        "GUID_VIDEO_DIM_TIMEOUT"
    } else if *guid == GUID_SLEEP_IDLE_THRESHOLD {
        "GUID_SLEEP_IDLE_THRESHOLD"
    } else if *guid == GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS {
        "GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS"
    } else if *guid == GUID_VIDEO_POWERDOWN_TIMEOUT {
        "GUID_VIDEO_POWERDOWN_TIMEOUT"
    } else {
        ""
    }
}

/// ClearPowerSettingNotifications 等价。
fn clear_power_setting_notifications() {
    let mut notifications = POWER_NOTIFICATIONS.lock().unwrap();
    for notification in notifications.iter() {
        // SAFETY: HPOWERNOTIFY 来自 RegisterPowerSettingNotification
        unsafe {
            let _ = UnregisterPowerSettingNotification(*notification);
        }
    }
    notifications.clear();
}

/// RegisterPowerSettingNotifications 等价。
/// 至少一个注册成功即报成功：单个不支持的 GUID 不拖垮其余通知（原注释语义）。
pub fn register_power_setting_notifications(hwnd: HWND) -> bool {
    // 等价原版前置检查: hwnd == NULL 直接返回 false（equiv cc:RegisterPowerSettingNotifications）
    if hwnd.is_invalid() || hwnd.0.is_null() {
        return false;
    }
    clear_power_setting_notifications();
    let settings = [
        GUID_CONSOLE_DISPLAY_STATE,
        GUID_MONITOR_POWER_ON,
        GUID_SESSION_DISPLAY_STATUS,
        GUID_SYSTEM_AWAYMODE,
        GUID_LIDSWITCH_STATE_CHANGE,
        GUID_SESSION_USER_PRESENCE,
        GUID_STANDBY_TIMEOUT,
        GUID_VIDEO_ADAPTIVE_DISPLAY_BRIGHTNESS,
        GUID_VIDEO_ADAPTIVE_PERCENT_INCREASE,
        GUID_VIDEO_ADAPTIVE_POWERDOWN,
        GUID_VIDEO_DIM_TIMEOUT,
        GUID_SLEEP_IDLE_THRESHOLD,
        GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS,
        GUID_VIDEO_POWERDOWN_TIMEOUT,
    ];

    let mut notifications = POWER_NOTIFICATIONS.lock().unwrap();
    for setting in &settings {
        // SAFETY: hwnd 合法、GUID 静态
        let notification = unsafe {
            RegisterPowerSettingNotification(
                windows::Win32::Foundation::HANDLE(hwnd.0),
                setting,
                windows::Win32::UI::WindowsAndMessaging::REGISTER_NOTIFICATION_FLAGS(0),
            )
        };
        if let Ok(n) = notification {
            notifications.push(n);
        }
    }
    !notifications.is_empty()
}

/// UnregisterPowerSettingNotifications 等价。
pub fn unregister_power_setting_notifications() -> bool {
    clear_power_setting_notifications();
    true
}

/// GetPowerSetting 等价：解析 POWERBROADCAST_SETTING 指针。
/// 返回 (guid字符串, name, data)；guid 为 GUID 的字符串形式（等价 GUIDToString）。
pub fn get_power_setting(setting_ptr: usize) -> Option<(String, String, u32)> {
    if setting_ptr == 0 {
        return None;
    }
    // SAFETY: 指针来自 WM_POWERBROADCAST 消息（调用方保证生命周期）
    unsafe {
        let setting = &*(setting_ptr as *const POWERBROADCAST_SETTING);
        let guid_str = format!(
            "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
            setting.PowerSetting.data1,
            setting.PowerSetting.data2,
            setting.PowerSetting.data3,
            setting.PowerSetting.data4[0],
            setting.PowerSetting.data4[1],
            setting.PowerSetting.data4[2],
            setting.PowerSetting.data4[3],
            setting.PowerSetting.data4[4],
            setting.PowerSetting.data4[5],
            setting.PowerSetting.data4[6],
            setting.PowerSetting.data4[7],
        );
        let name = get_power_setting_name(&setting.PowerSetting).to_string();
        let copy_len = (setting.DataLength as usize).min(4);
        let data_bytes = core::slice::from_raw_parts(setting.Data.as_ptr(), copy_len);
        let mut bytes = [0u8; 4];
        bytes[..copy_len].copy_from_slice(data_bytes);
        let data = u32::from_le_bytes(bytes);
        Some((guid_str, name, data))
    }
}
