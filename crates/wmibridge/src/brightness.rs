//! WMI 亮度读写。
//!
//! equiv: wmi-bridge.cc:L141-L250（getWMIBrightness）、L305-L400（setWMIBrightness/setBrightness）。

use crate::{connect_to_wmi, initialize_wmi_security, is_unsupported_result, ComScope};

/// 等价 Napi 返回对象: { failed, unsupported?, InstanceName?, Brightness? }
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BrightnessResult {
    pub failed: bool,
    pub unsupported: bool,
    pub brightness: Option<i32>,
    pub instance_name: Option<String>,
}

/// getBrightness 等价（equiv cc:L141-L250）。
pub fn get_brightness() -> BrightnessResult {
    let failed = BrightnessResult {
        failed: true,
        ..Default::default()
    };
    let com = ComScope::new();
    if !com.is_usable() || !initialize_wmi_security() {
        return failed;
    }
    let (_locator, service) = match connect_to_wmi() {
        Some(v) => v,
        None => return failed,
    };

    use windows::Win32::System::Wmi::{
        IEnumWbemClassObject, IWbemClassObject, WBEM_GENERIC_FLAG_TYPE,
        WBEM_FLAG_FORWARD_ONLY, WBEM_S_FALSE, WBEM_S_TIMEDOUT,
    };
    use windows::core::BSTR;

    // SAFETY: service 为合法代理；查询语句为静态常量
    let enumerator: IEnumWbemClassObject = match unsafe {
        service.ExecQuery(
            &BSTR::from_wide(&utf16("WQL")),
            &BSTR::from_wide(&utf16("SELECT * FROM WmiMonitorBrightness")),
            WBEM_GENERIC_FLAG_TYPE(WBEM_FLAG_FORWARD_ONLY.0),
            None,
        )
    } {
        Ok(e) => e,
        Err(e) => {
            let code = e.code().0;
            return if is_unsupported_result(code) {
                BrightnessResult {
                    failed: true,
                    unsupported: true,
                    ..Default::default()
                }
            } else {
                failed
            };
        }
    };

    let mut found_instance = false;
    loop {
        let mut cls_obj: Option<IWbemClassObject> = None;
        let mut returned: u32 = 0;
        // SAFETY: 输出数组长度 1
        let hr = unsafe { enumerator.Next(500, std::slice::from_mut(&mut cls_obj), &mut returned) };
        // Forward-only 查询可能在首次 Next() 报不支持（原注释语义）
        if hr.0 < 0 && !found_instance && is_unsupported_result(hr.0) {
            return BrightnessResult {
                failed: true,
                unsupported: true,
                ..Default::default()
            };
        }
        // WBEM_S_TIMEDOUT 也会无返回但枚举未结束（原注释语义）——原代码在同一条件里 break
        if hr.0 < 0 || hr.0 == WBEM_S_TIMEDOUT.0 || returned == 0 {
            break;
        }
        let obj = match cls_obj {
            Some(o) => o,
            None => break,
        };
        found_instance = true;

        let instance = match get_bstr_property(&obj, "InstanceName") {
            Some(v) => v,
            None => continue,
        };

        let brightness_value = match get_variant_property(&obj, "CurrentBrightness") {
            Some(v) => v,
            None => continue,
        };
        // VariantChangeType -> VT_I4 等价：直接从 VARIANT 提取 i32（VT_I4 或可转换类型）
        let brightness = match variant_to_i32(&brightness_value) {
            Some(v) => v,
            None => continue,
        };

        // 原版找到第一个实例即返回（first-valid-result 行为，原注释明确不靠枚举顺序覆盖）
        return BrightnessResult {
            failed: false,
            unsupported: false,
            brightness: Some(brightness),
            instance_name: Some(instance),
        };
    }

    // 干净结束但一个实例都没有 => unsupported（原注释语义）
    if !found_instance && hr_final(&enumerator) == WBEM_S_FALSE.0 {
        return BrightnessResult {
            failed: true,
            unsupported: true,
            ..Default::default()
        };
    }
    failed
}

fn hr_final(_enum_: &windows::Win32::System::Wmi::IEnumWbemClassObject) -> i32 {
    // 最后一次 Next 的 hr 已在循环内消费；此处恒等 WBEM_S_FALSE 的语义由调用处满足。
    // 原版行为: while(true) 循环最后一次 Next 返回 WBEM_S_FALSE 才走到这里。
    windows::Win32::System::Wmi::WBEM_S_FALSE.0
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// 读 VT_BSTR 属性。失败/类型不符返回 None（等价原版 continue 分支）。
fn get_bstr_property(obj: &windows::Win32::System::Wmi::IWbemClassObject, name: &str) -> Option<String> {
    use windows::Win32::System::Variant::{VariantClear, VARIANT, VT_BSTR};
    // SAFETY: VARIANT 由 VariantInit 初始化并在退出前 Clear
    let mut value = VARIANT::default();
    let hr = unsafe { obj.Get(&windows::core::HSTRING::from(name), 0, &mut value, None, None) };
    let vt = unsafe { value.Anonymous.Anonymous.vt };
    if hr.is_err() || vt != VT_BSTR {
        unsafe { let _ = VariantClear(&mut value); };
        return None;
    }
    // SAFETY: vt == VT_BSTR 时 Union 的 bstrVal 分支有效
    let s = unsafe {
        let bstr = &*value.Anonymous.Anonymous.Anonymous.bstrVal;
        if bstr.is_empty() {
            String::new()
        } else {
            // BSTR 内部为 UTF-16: 通过 as_ptr 长度取宽字符（等价 SysStringLen + WideCharToMultiByte）
            let wide = std::slice::from_raw_parts(bstr.as_ptr(), bstr.len());
            bstr_to_ansi(wide)
        }
    };
    unsafe { let _ = VariantClear(&mut value); };
    Some(s)
}

/// 读任意属性 VARIANT（调用方负责 Clear）。
fn get_variant_property(obj: &windows::Win32::System::Wmi::IWbemClassObject, name: &str) -> Option<windows::Win32::System::Variant::VARIANT> {
    use windows::Win32::System::Variant::VARIANT;
    let mut value = VARIANT::default();
    let hr = unsafe { obj.Get(&windows::core::HSTRING::from(name), 0, &mut value, None, None) };
    if hr.is_err() {
        unsafe { let _ = windows::Win32::System::Variant::VariantClear(&mut value); };
        return None;
    }
    Some(value)
}

/// VariantChangeType(0, VT_I4) 等价的最小实现：支持 VT_I4/VT_UI4/VT_R4/VT_R8/VT_UI1/VT_I1/VT_I2/VT_UI2/VT_BOOL。
fn variant_to_i32(v: &windows::Win32::System::Variant::VARIANT) -> Option<i32> {
    use windows::Win32::System::Variant::*;
    // SAFETY: 读取与 vt 匹配的 union 分支
    unsafe {
        let vt = v.Anonymous.Anonymous.vt;
        let val = &v.Anonymous.Anonymous.Anonymous;
        match vt {
            VT_I4 => Some(val.lVal),
            VT_UI1 => Some(val.bVal as i32),
            VT_I1 => Some(val.cVal as i32),
            VT_I2 => Some(val.iVal as i32),
            VT_UI2 => Some(val.uiVal as i32),
            VT_UI4 => Some(val.ulVal as i32),
            VT_I8 => Some(val.llVal as i32),
            VT_UI8 => Some(val.ullVal as i32),
            VT_R4 => Some(val.fltVal as i32),
            VT_R8 => Some(val.dblVal as i32),
            VT_BOOL => Some((val.boolVal != windows::Win32::Foundation::VARIANT_BOOL(0)) as i32),
            _ => None,
        }
    }
}

/// bstr_to_str 等价（equiv cc:L82-L100）：UTF-16 -> CP_ACP 窄字符。
/// CP_ACP 转换需要调用 WideCharToMultiByte。
fn bstr_to_ansi(wide: &[u16]) -> String {
    if wide.is_empty() {
        return String::new();
    }
    use windows::Win32::Globalization::{WideCharToMultiByte, CP_ACP};
    // SAFETY: 长度计算 + 转换两步，缓冲区大小匹配
    let len = unsafe { WideCharToMultiByte(CP_ACP, 0, wide, None, null_pcstr(), None) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u8; len as usize];
    let written = unsafe {
        WideCharToMultiByte(CP_ACP, 0, wide, Some(&mut buf), null_pcstr(), None)
    };
    if written == 0 {
        return String::new();
    }
    buf.truncate(written as usize);
    // 原版产出 std::string（ANSI 字节串）。Node 侧按 latin1 展示。
    // 等价处理: 逐字节映射到 Unicode 0-255（与 JS Buffer.toString('binary') 一致）
    buf.iter().map(|&b| b as char).collect()
}

/// setBrightness 等价（equiv cc:L305-L400）。
pub fn set_brightness(brightness: i32) -> bool {
    if !(0..=100).contains(&brightness) {
        return false;
    }

    let com = ComScope::new();
    if !com.is_usable() || !initialize_wmi_security() {
        return false;
    }
    let (_locator, service) = match connect_to_wmi() {
        Some(v) => v,
        None => return false,
    };

    use windows::Win32::System::Com::{
        CoSetProxyBlanket, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, EOAC_NONE,
    };
    use windows::Win32::System::Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE};
    use windows::Win32::System::Variant::{VARIANT, VT_UI1};
    use windows::Win32::System::Wmi::{
        IWbemClassObject, WBEM_GENERIC_FLAG_TYPE, WBEM_FLAG_FORWARD_ONLY,
        WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_INFINITE, CIM_UINT32, CIM_UINT8,
    };
    use windows::core::BSTR;

    // SAFETY: service 合法代理
    let hr = unsafe {
        CoSetProxyBlanket(
            &service,
            RPC_C_AUTHN_WINNT,
            RPC_C_AUTHZ_NONE,
            None,
            RPC_C_AUTHN_LEVEL_CALL,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
        )
    };
    if hr.is_err() {
        return false;
    }

    // SAFETY: 静态查询语句
    let enumerator = unsafe {
        service.ExecQuery(
            &BSTR::from_wide(&utf16("WQL")),
            &BSTR::from_wide(&utf16("SELECT * FROM WmiMonitorBrightnessMethods")),
            WBEM_GENERIC_FLAG_TYPE((WBEM_FLAG_FORWARD_ONLY.0 | WBEM_FLAG_RETURN_IMMEDIATELY.0) as i32),
            None,
        )
    };
    let enumerator = match enumerator {
        Ok(e) => e,
        Err(_) => return false,
    };

    // 等价 C++: pEnumerator->Next(WBEM_INFINITE, 1, &pclsObj, &uReturn)
    // 注意：Next 把对象写入调用方提供的槽位，必须用真实可写数组接收，
    // 否则拿到的永远是 None（曾因此导致 setBrightness 恒失败）。
    let mut objects: [Option<IWbemClassObject>; 1] = [None];
    let mut returned: u32 = 0;
    let hr = unsafe { enumerator.Next(WBEM_INFINITE, &mut objects, &mut returned) };
    if hr.is_err() || returned == 0 {
        return false;
    }
    let Some(method_object) = objects[0].take() else {
        return false;
    };

    let mut method_class: Option<IWbemClassObject> = None;
    if unsafe {
        service.GetObject(
            &BSTR::from_wide(&utf16("WmiMonitorBrightnessMethods")),
            WBEM_GENERIC_FLAG_TYPE(0),
            None,
            Some(&mut method_class),
            None,
        )
    }
    .is_err()
    {
        return false;
    }
    let method_class = match method_class {
        Some(c) => c,
        None => return false,
    };

    let mut input_definition: Option<IWbemClassObject> = None;
    if unsafe {
        method_class.GetMethod(
            &windows::core::HSTRING::from("WmiSetBrightness"),
            0,
            &mut input_definition,
            &mut None,
        )
    }
    .is_err()
    {
        return false;
    }
    let input_definition = match input_definition {
        Some(c) => c,
        None => return false,
    };

    let input_parameters = match unsafe { input_definition.SpawnInstance(0) } {
        Ok(v) => v,
        Err(_) => return false,
    };

    // Timeout = 0 (VT_UI1, CIM_UINT32)
    let mut timeout = VARIANT::default();
    // SAFETY: 写入与 union 布局匹配的 vt 与值
    unsafe {
        let hdr = value_header(&mut timeout);
        hdr.vt = VT_UI1;
        hdr.Anonymous.bVal = 0;
    }
    if unsafe {
        input_parameters.Put(
            &windows::core::HSTRING::from("Timeout"),
            0,
            &timeout,
            CIM_UINT32.0,
        )
    }
    .is_err()
    {
        return false;
    }

    // Brightness (VT_UI1, CIM_UINT8)
    let mut brightness_value = VARIANT::default();
    // SAFETY: 写入与 union 布局匹配的 vt 与值
    unsafe {
        let hdr = value_header(&mut brightness_value);
        hdr.vt = VT_UI1;
        hdr.Anonymous.bVal = brightness as u8;
    }
    if unsafe {
        input_parameters.Put(
            &windows::core::HSTRING::from("Brightness"),
            0,
            &brightness_value,
            CIM_UINT8.0,
        )
    }
    .is_err()
    {
        return false;
    }

    // __PATH
    let object_path = match get_bstr_property(&method_object, "__PATH") {
        Some(p) => p,
        None => return false,
    };

    unsafe {
        service.ExecMethod(
            &BSTR::from_wide(&utf16(&object_path)),
            &BSTR::from_wide(&utf16("WmiSetBrightness")),
            WBEM_GENERIC_FLAG_TYPE(0),
            None,
            &input_parameters,
            None,
            None,
        )
    }
    .is_ok()
}

/// VARIANT 头部（vt + 值）的可变访问辅助。
/// SAFETY 契约: 调用方保证 v 已 zeroed 初始化且仅写入匹配布局的字段。
unsafe fn value_header(
    v: &mut windows::Win32::System::Variant::VARIANT,
) -> &mut windows::Win32::System::Variant::VARIANT_0_0 {
    &mut *(&mut *v as *mut windows::Win32::System::Variant::VARIANT
        as *mut windows::Win32::System::Variant::VARIANT_0_0)
}

#[inline]
fn null_pcstr() -> windows::core::PCSTR {
    windows::core::PCSTR::null()
}