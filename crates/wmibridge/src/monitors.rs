//! WMI 显示器清单。
//!
//! equiv: wmi-bridge.cc:L252-L303（getWMIMonitors）。

use crate::{connect_to_wmi, initialize_wmi_security, ComScope};

/// 等价 Napi 返回: { [InstanceName]: { InstanceName, UserFriendlyName? } }
#[derive(Debug, Clone, Default)]
pub struct MonitorInfo {
    pub instance_name: String,
    pub user_friendly_name: Option<String>,
}

/// getMonitors 等价。
pub fn get_monitors() -> Result<Vec<MonitorInfo>, ()> {
    let com = ComScope::new();
    if !com.is_usable() || !initialize_wmi_security() {
        return Err(());
    }
    let (_locator, service) = connect_to_wmi().ok_or(())?;

    use windows::Win32::System::Wmi::{
        IWbemClassObject, WBEM_GENERIC_FLAG_TYPE, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY,
    };
    use windows::core::BSTR;

    // SAFETY: service 合法代理，查询语句静态
    let enumerator = unsafe {
        service.ExecQuery(
            &BSTR::from_wide(&utf16("WQL")),
            &BSTR::from_wide(&utf16("SELECT * FROM WmiMonitorID")),
            WBEM_GENERIC_FLAG_TYPE(
                (WBEM_FLAG_FORWARD_ONLY.0 | WBEM_FLAG_RETURN_IMMEDIATELY.0) as i32,
            ),
            None,
        )
    }
    .map_err(|_| ())?;

    let mut monitors = Vec::new();
    loop {
        let mut cls_obj: Option<IWbemClassObject> = None;
        let mut returned: u32 = 0;
        // SAFETY: 输出数组长度 1
        let hr = unsafe { enumerator.Next(500, std::slice::from_mut(&mut cls_obj), &mut returned) };
        if hr.is_err() || returned == 0 {
            break;
        }
        let obj = match cls_obj {
            Some(o) => o,
            None => break,
        };

        let instance = match get_bstr_property(&obj, "InstanceName") {
            Some(v) if !v.is_empty() => v,
            _ => continue,
        };

        // UserFriendlyName: UINT16 数组 -> ANSI 字符串（equiv getWMIClassUINTString）
        let friendly = get_uint16_array_string(&obj, "UserFriendlyName");

        monitors.push(MonitorInfo {
            instance_name: instance,
            user_friendly_name: friendly,
        });
    }
    Ok(monitors)
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// bstr_to_str 等价（equiv cc:L82-L100）。
pub(crate) fn get_bstr_property(obj: &windows::Win32::System::Wmi::IWbemClassObject, name: &str) -> Option<String> {
    use windows::Win32::System::Variant::{VariantClear, VARIANT, VT_BSTR};
    let mut value = VARIANT::default();
    // SAFETY: VARIANT 生命周期由本函数管理
    let hr = unsafe { obj.Get(&windows::core::HSTRING::from(name), 0, &mut value, None, None) };
    let vt = unsafe { value.Anonymous.Anonymous.vt };
    if hr.is_err() || vt != VT_BSTR {
        unsafe { let _ = VariantClear(&mut value); };
        return None;
    }
    // SAFETY: vt == VT_BSTR
    let s = unsafe {
        let bstr = &*value.Anonymous.Anonymous.Anonymous.bstrVal;
        if bstr.is_empty() {
            String::new()
        } else {
            let wide = std::slice::from_raw_parts(bstr.as_ptr(), bstr.len());
            bstr_to_ansi(wide)
        }
    };
    unsafe { let _ = VariantClear(&mut value); };
    Some(s)
}

/// getWMIClassUINTString 等价（equiv cc:L103-L118）：
/// UINT16 SafeArray 逐元素收集非零值，按字节窄字符拼接。
pub(crate) fn get_uint16_array_string(obj: &windows::Win32::System::Wmi::IWbemClassObject, name: &str) -> Option<String> {
    use windows::Win32::System::Ole::{
        SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound,
    };
    use windows::Win32::System::Variant::{VariantClear, VARIANT, VT_ARRAY};

    let mut value = VARIANT::default(); // VARIANT::default() == VariantInit（全零）
    // SAFETY: VARIANT 生命周期由本函数管理
    let hr = unsafe { obj.Get(&windows::core::HSTRING::from(name), 0, &mut value, None, None) };
    let mut output = String::new();
    let mut ok = false;
    // SAFETY: 检查 vt & VT_ARRAY 后访问 parray（parray 在 VARIANT_0_0_0 内）
    unsafe {
        let vt = value.Anonymous.Anonymous.vt;
        let parray = value.Anonymous.Anonymous.Anonymous.parray;
        if hr.is_ok() && (vt.0 & VT_ARRAY.0) != 0 && !parray.is_null() {
            let lower = SafeArrayGetLBound(parray, 1);
            let upper = SafeArrayGetUBound(parray, 1);
            if let (Ok(lower), Ok(upper)) = (lower, upper) {
                ok = true;
                for i in lower..=upper {
                    let mut element: u16 = 0;
                    if SafeArrayGetElement(parray, &i, &mut element as *mut u16 as *mut _).is_ok()
                        && element != 0
                    {
                        output.push(element as u8 as char);
                    }
                }
            }
        }
        let _ = VariantClear(&mut value);
    }
    if ok { Some(output) } else { None }
}

/// 等价 bstr_to_str 的 ANSI 转换（与 brightness.rs 相同语义）。
pub(crate) fn bstr_to_ansi(wide: &[u16]) -> String {
    if wide.is_empty() {
        return String::new();
    }
    use windows::Win32::Globalization::{WideCharToMultiByte, CP_ACP};
    // SAFETY: 两步转换，长度匹配
    let len = unsafe { WideCharToMultiByte(CP_ACP, 0, wide, None, windows::core::PCSTR::null(), None) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u8; len as usize];
    let written = unsafe {
        WideCharToMultiByte(CP_ACP, 0, wide, Some(&mut buf), windows::core::PCSTR::null(), None)
    };
    if written == 0 {
        return String::new();
    }
    buf.truncate(written as usize);
    buf.iter().map(|&b| b as char).collect()
}
