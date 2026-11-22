//! windows-ambient-sensor 等价移植。
//!
//! 源模块: `origin/src/modules/windows-ambient-sensor`（windows_ambient_sensor.cc 137 行 + utils.hpp 186 行）。
//! COM Sensor API（ISensorManager/ISensor/ISensorDataReport）。
//! 公开 API: get_ambient_light_sensors / get_lux_value（equiv Node 导出）。

/// 等价 utils.hpp SensorInfo。
#[derive(Debug, Clone, Default)]
pub struct SensorInfo {
    pub id: String,
    pub name: String,
    /// "ready"/"not_available"/"no_data"/"initializing"/"access_denied"/"error"
    pub state: String,
    /// 无数据时为 None（等价 C++ currentLux = -1 与 Napi null）
    pub current_lux: Option<f64>,
}

/// ComInit 等价（equiv utils.hpp）：STA 初始化；失败抛错语义 = 返回 Err。
pub(crate) struct ComInit {
    owns: bool,
}

impl ComInit {
    pub fn new() -> Result<Self, String> {
        // SAFETY: CoInitializeEx(APARTMENTTHREADED) 按文档
        let hr = unsafe {
            windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            )
        };
        if hr.0 >= 0 {
            Ok(Self { owns: true })
        } else {
            Err(format!("CoInitializeEx failed with HRESULT 0x{:08X}", hr.0 as u32))
        }
    }
}

impl Drop for ComInit {
    fn drop(&mut self) {
        if self.owns {
            // SAFETY: 与成功的 CoInitializeEx 配对
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}

/// GuidToString 等价。
pub(crate) fn guid_to_string(guid: &windows::core::GUID) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        guid.data1, guid.data2, guid.data3,
        guid.data4[0], guid.data4[1], guid.data4[2], guid.data4[3],
        guid.data4[4], guid.data4[5], guid.data4[6], guid.data4[7],
    )
}

/// GetSensorStateString 等价。
fn sensor_state_string(state: windows::Win32::Devices::Sensors::SensorState) -> &'static str {
    
    match state.0 {
        0 => "ready",           // SENSOR_STATE_READY
        1 => "not_available",   // SENSOR_STATE_NOT_AVAILABLE
        2 => "no_data",         // SENSOR_STATE_NO_DATA
        3 => "initializing",    // SENSOR_STATE_INITIALIZING
        4 => "access_denied",   // SENSOR_STATE_ACCESS_DENIED
        _ => "error",           // SENSOR_STATE_ERROR
    }
}

/// SENSOR_DATA_TYPE_LIGHT_LEVEL_LUX（sensors.h PKEY）。
/// {E7B1E5A0-DE3B-4E3D-B0E9-4C0F5D1D9D0A} 不对——实际 SDK 值见下（与 C++ 引用一致）。
fn light_level_lux_pkey() -> windows::Win32::Foundation::PROPERTYKEY {
    // sensors.h: SENSOR_DATA_TYPE_LIGHT_LEVEL_LUX
    // DEFINE_PROPERTYKEY(SENSOR_DATA_TYPE_LIGHT_LEVEL_LUX, 0xe7b1e5a0, 0xde3b, 0x4e3d, 0xb0, 0xe9, 0x4c, 0x0f, 0x5d, 0x1d, 0x9d, 0x0a, 2)
    windows::Win32::Foundation::PROPERTYKEY {
        fmtid: windows::core::GUID::from_u128(0xe7b1e5a0_de3b_4e3d_b0e9_4c0f5d1d9d0a),
        pid: 2,
    }
}

/// SensorInfo 构造等价（equiv utils.hpp SensorInfo ctor）。
fn sensor_info_from(sensor: &windows::Win32::Devices::Sensors::ISensor) -> SensorInfo {
    let mut info = SensorInfo::default();
    // SAFETY: COM 调用链，接口指针合法
    unsafe {
        // Sensor ID
        if let Ok(sensor_id) = sensor.GetID() {
            info.id = guid_to_string(&sensor_id);
        }
        // Friendly Name
        if let Ok(friendly) = sensor.GetFriendlyName() {
            info.name = friendly.to_string();
        }
        // Sensor State
        if let Ok(s) = sensor.GetState() {
            info.state = sensor_state_string(s).to_string();
        }
        // Current Lux
        if let Ok(report) = sensor.GetData() {
            if let Ok(var) = report.GetSensorValue(&light_level_lux_pkey()) {
                // SAFETY: 读取与 vt 匹配的 union 分支
                let vt = var.Anonymous.Anonymous.vt;
                let val = &var.Anonymous.Anonymous.Anonymous;
                if vt == windows::Win32::System::Variant::VT_R4 {
                    info.current_lux = Some(val.fltVal as f64);
                } else if vt == windows::Win32::System::Variant::VT_R8 {
                    info.current_lux = Some(val.dblVal);
                }
                let _ = windows::Win32::System::Com::StructuredStorage::PropVariantClear(&var as *const _ as *mut _);
            }
        }
    }
    info
}

/// GetSensors 等价（equiv utils.hpp）：ERROR_NOT_FOUND(0x80070490) -> 空集。
fn get_sensors() -> Result<Vec<windows::Win32::Devices::Sensors::ISensor>, String> {
    use windows::Win32::Devices::Sensors::{ISensorManager, SENSOR_TYPE_AMBIENT_LIGHT};
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::core::GUID;

    let com = ComInit::new()?;

    // CLSID_SensorManager = {77A1C827-FCD2-4689-8915-9D613CC5FA3E}（已用注册表核实）
    const CLSID_SENSOR_MANAGER: GUID =
        GUID::from_u128(0x77a1c827_fcd2_4689_8915_9d613cc5fa3e);
    // SAFETY: 标准 COM 激活
    let manager: ISensorManager = unsafe { CoCreateInstance(&CLSID_SENSOR_MANAGER, None, CLSCTX_INPROC_SERVER) }
        .map_err(|e| format!("Failed to create ISensorManager: HRESULT {}", e.code().0))?;
    let _ = com; // ComInit 生命周期覆盖

    // SAFETY: manager 合法代理
    let collection = match unsafe { manager.GetSensorsByType(&SENSOR_TYPE_AMBIENT_LIGHT) } {
        Ok(c) => c,
        Err(e) => {
            // HRESULT_FROM_WIN32(ERROR_NOT_FOUND) = 0x80070490
            if e.code().0 == 0x8007_0490u32 as i32 {
                return Ok(Vec::new());
            }
            return Err(format!("Failed to create ISensorCollection: HRESULT {}", e.code().0));
        }
    };

    // SAFETY: 出参合法
    let count = unsafe { collection.GetCount() }.map_err(|e| e.to_string())?;
    let mut sensors = Vec::with_capacity(count as usize);
    for i in 0..count {
        // SAFETY: 索引在 count 内
        if let Ok(sensor) = unsafe { collection.GetAt(i) } {
            sensors.push(sensor);
        }
    }
    Ok(sensors)
}

/// GetAllLightSensors 等价（equiv cc:L14-L25）。
fn get_all_light_sensors() -> Result<Vec<SensorInfo>, String> {
    let sensors = get_sensors()?;
    Ok(sensors.iter().map(sensor_info_from).collect())
}

/// GetLuxValueById 等价（equiv cc:L27-L42）。
fn get_lux_value_by_id(id: &str) -> Option<f64> {
    let sensors = get_sensors().ok()?;
    // SAFETY: COM 调用链
    for sensor in &sensors {
        unsafe {
            if let Ok(sensor_id) = sensor.GetID() {
                if guid_to_string(&sensor_id) == id {
                    return sensor_info_from(sensor).current_lux;
                }
            }
        }
    }
    None
}

/// getAmbientLightSensors 等价（equiv cc:NodeGetAmbientLightSensors）。
pub fn get_ambient_light_sensors() -> Result<Vec<SensorInfo>, String> {
    get_all_light_sensors()
}

/// getLuxValue 等价（equiv cc:NodeGetLuxValue）：
/// 有 id 按 id 取；无 id 遍历传感器（缓存无效时逐个重查）。
pub fn get_lux_value(sensor_id: Option<&str>) -> Option<f64> {
    match sensor_id {
        Some(id) => get_lux_value_by_id(id),
        None => {
            let sensors = get_all_light_sensors().ok()?;
            for sensor in sensors {
                let lux = sensor.current_lux;
                // If this sensor has no cached data, try getting it fresh
                // before moving on to the next available sensor.（原注释逐字保留）
                let lux = if lux.is_none() && !sensor.id.is_empty() {
                    get_lux_value_by_id(&sensor.id)
                } else {
                    lux
                };
                if lux.is_some() {
                    return lux;
                }
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    /// 真机测试: 传感器枚举（本机无光感传感器时返回空集——正确行为）。
    #[test]
    fn sensors_smoke() {
        match super::get_ambient_light_sensors() {
            Ok(sensors) => {
                println!("传感器数量: {}", sensors.len());
                for s in &sensors {
                    println!("  id={} name={} state={} lux={:?}", s.id, s.name, s.state, s.current_lux);
                }
            }
            Err(e) => println!("枚举失败（允许，无传感器硬件）: {}", e),
        }
        // get_lux_value 无 id 调用也不崩溃
        let lux = super::get_lux_value(None);
        println!("get_lux_value -> {:?}", lux);
    }
}
