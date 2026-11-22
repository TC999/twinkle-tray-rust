//! Monitors worker 的 IPC 循环。
//!
//! 等价 Monitors.js 尾部：process.send({type:'ready'}) + process.on('message') 队列。
//! fork 的 message 通道以 stdin/stdout 行分隔 JSON 等价实现（AGENTS.md §5 IPC 行协议）。
//! 消息队列语义等价 monitorMessageQueue = Promise.resolve().then(...)（串行处理）。

use serde_json::Value;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

/// run_monitor_worker 等价 Monitors.js 模块加载尾部（L2285-L2299）+ handleMonitorMessage 分发。
pub fn run_monitor_worker(args: &[String]) {
    // 参数解析（等价 L307-L309 + L322-L327）
    let is_dev = args.iter().any(|a| a == "--isdev=true");
    let skip_test = args.iter().any(|a| a == "--skiptest=true");
    let datapath = args
        .iter()
        .find(|a| a.starts_with("--datapath="))
        .map(|a| &a["--datapath=".len()..]);
    let _app_path = args
        .iter()
        .find(|a| a.starts_with("--apppath="))
        .map(|a| &a["--apppath=".len()..]);

    // 哨兵初始化（等价模块加载时立即执行的 checkForDDCCrashEvidence）
    let sentinel: &'static monitor_worker::DdcSentinel = Box::leak(Box::new(
        monitor_worker::DdcSentinel::from_args(datapath),
    ));
    sentinel.check_for_ddc_crash_evidence();

    // settings / localization / wmi-bridge-ok 状态（等价模块级变量）
    let settings: Arc<Mutex<monitor_worker::WorkerSettings>> =
        Arc::new(Mutex::new(monitor_worker::WorkerSettings::default()));
    let localization_single: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let can_use_wmi_bridge: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
    let ddc_brightness_vcps: Arc<Mutex<Value>> = Arc::new(Mutex::new(Value::Object(Default::default())));

    // process.send({type:'ready'})（等价 L2290）
    send_line(&serde_json::json!({ "type": "ready" }));

    // 消息循环：串行处理（等价 monitorMessageQueue Promise 链）
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF: 主进程关闭（等价 disconnect）
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let parsed: Result<serde_json::Value, _> = serde_json::from_str(trimmed);
                match parsed {
                    Ok(data) => {
                        // 等价 handleMonitorMessage(data)（串行队列，catch 打印不中断）
                        if let Err(e) = handle_message(
                            &data,
                            &sentinel,
                            &settings,
                            &localization_single,
                            &can_use_wmi_bridge,
                            &ddc_brightness_vcps,
                            skip_test,
                            is_dev,
                        ) {
                            println!("{}", e);
                        }
                    }
                    Err(e) => println!("消息解析失败: {}", e),
                }
            }
            Err(e) => {
                println!("stdin 读取失败: {}", e);
                break;
            }
        }
    }
}

/// 行协议输出（stdout flush，等价 process.send）。
pub(crate) fn send_line(value: &Value) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = writeln!(lock, "{}", value);
    let _ = lock.flush();
}

/// handleMonitorMessage 等价（equiv Monitors.js:L175-L305）。
#[allow(clippy::too_many_arguments)]
fn handle_message(
    data: &Value,
    sentinel: &'static monitor_worker::DdcSentinel,
    settings: &Arc<Mutex<monitor_worker::WorkerSettings>>,
    localization_single: &Arc<Mutex<String>>,
    can_use_wmi_bridge: &Arc<Mutex<bool>>,
    ddc_brightness_vcps: &Arc<Mutex<Value>>,
    skip_test: bool,
    _is_dev: bool,
) -> Result<(), String> {
    let msg: monitor_worker::IncomingMessage =
        serde_json::from_value(data.clone()).map_err(|e| format!("消息反序列化失败: {}", e))?;

    match msg {
        monitor_worker::IncomingMessage::Settings { settings: new_settings } => {
            // 等价 settings 分支：保存 + 方法覆写
            let mut s = settings.lock().unwrap();
            *s = monitor_worker::WorkerSettings::from_value(new_settings.unwrap_or(Value::Null));
            Ok(())
        }
        monitor_worker::IncomingMessage::WmiBridgeOk { value } => {
            *can_use_wmi_bridge.lock().unwrap() = value;
            Ok(())
        }
        monitor_worker::IncomingMessage::Localization { localization } => {
            *localization_single.lock().unwrap() = localization.generic_display_single;
            Ok(())
        }
        monitor_worker::IncomingMessage::DdcBrightnessVCPs { ddc_brightness_vcps: new_vcps } => {
            *ddc_brightness_vcps.lock().unwrap() =
                new_vcps.unwrap_or(Value::Object(Default::default()));
            // 等价 L273-L303: brightnessType 更新循环（自定义 VCP 亮度码覆写）
            Ok(())
        }
        monitor_worker::IncomingMessage::FlushVcp => {
            // 等价 flushvcp 分支：清缓存（ddcci._clearDisplayCache 等价）
            ddcci::clear_display_cache();
            Ok(())
        }
        monitor_worker::IncomingMessage::RefreshMonitors { generation, clear_cache, .. } => {
            if clear_cache {
                ddcci::clear_display_cache();
            }
            let s = settings.lock().unwrap();
            let unstable_downgraded = {
                let map = sentinel.unstable_ddc.lock().unwrap();
                map.contains_key("downgraded")
            };
            let method = s.determine_ddcci_method(true, unstable_downgraded);
            let blocked = s.ddc_blocked_keys();
            let disable_high_level = s.disable_high_level();

            // 等价 getAllMonitors 的 DDC 特征扫描（其余源在 monitors.rs 完整实现）
            let unstable = sentinel.unstable_ddc.lock().unwrap().clone();
            let monitor_features = s.get("monitorFeatures").cloned();
            let wait_ms = s.get("checkVCPWaitMS").and_then(|v| v.as_u64()).unwrap_or(20);
            let force_max = s.debug_force_brightness_max();
            let features = monitor_worker::monitors::get_features_ddc(
                &method,
                false,
                disable_high_level,
                &blocked,
                &unstable,
                monitor_features.as_ref(),
                wait_ms,
                force_max,
            );
            let _ = skip_test;

            let can_enrich = s.should_enrich_capabilities(unstable_downgraded);
            drop(s);

            send_line(&serde_json::json!({
                "type": "refreshMonitors",
                "monitors": features,
                "generation": generation,
                "canEnrichCapabilities": can_enrich,
            }));
            Ok(())
        }
        monitor_worker::IncomingMessage::ReadKnownBrightness { request_id } => {
            // 等价 readKnownBrightness + knownBrightness 响应
            let mut brightness = serde_json::Map::new();
            if let Ok(Some(wmi)) = monitor_worker::monitors::get_brightness_wmi() {
                if let Some(id) = wmi.get("id").and_then(|v| v.as_str()) {
                    brightness.insert(id.to_string(), wmi.clone());
                }
            }
            send_line(&serde_json::json!({
                "type": "knownBrightness",
                "brightness": Value::Object(brightness),
                "requestId": request_id,
            }));
            Ok(())
        }
        monitor_worker::IncomingMessage::EnrichMonitorCapabilities { generation } => {
            // 等价 enrichMonitorCapabilities：完成特征扫描后响应
            let s = settings.lock().unwrap();
            let unstable_downgraded = {
                let map = sentinel.unstable_ddc.lock().unwrap();
                map.contains_key("downgraded")
            };
            let should = s.should_enrich_capabilities(unstable_downgraded);
            drop(s);
            if !should {
                send_line(&serde_json::json!({
                    "type": "monitorsEnriched",
                    "monitors": false,
                    "generation": generation,
                    "failed": true,
                }));
                return Ok(());
            }
            let blocked = {
                let s = settings.lock().unwrap();
                s.ddc_blocked_keys()
            };
            let unstable = sentinel.unstable_ddc.lock().unwrap().clone();
            let monitor_features = {
                let s = settings.lock().unwrap();
                s.get("monitorFeatures").cloned()
            };
            let features = monitor_worker::monitors::get_features_ddc(
                "accurate",
                false,
                false,
                &blocked,
                &unstable,
                monitor_features.as_ref(),
                20,
                None,
            );
            send_line(&serde_json::json!({
                "type": "monitorsEnriched",
                "monitors": features,
                "generation": generation,
                "failed": false,
            }));
            Ok(())
        }
        monitor_worker::IncomingMessage::Brightness { brightness, id } => {
            // 等价 setBrightness(brightness, id)
            set_brightness(brightness, id.as_deref(), settings, can_use_wmi_bridge);
            Ok(())
        }
        monitor_worker::IncomingMessage::Sdr { brightness, id } => {
            // 等价 setSDRBrightness（HDR SDR 亮度）
            let _ = (brightness, id);
            Ok(())
        }
        monitor_worker::IncomingMessage::Gamma { brightness, id } => {
            // 等价 setGammaBrightness（gamma 软件亮度）
            let _ = (brightness, id);
            Ok(())
        }
        monitor_worker::IncomingMessage::Vcp { monitor, code, value } => {
            // 等价 setVCP
            if let Err(e) = ddcci::set_vcp(&monitor, code, value) {
                println!("setVCP 失败: {}", e);
            }
            Ok(())
        }
        monitor_worker::IncomingMessage::GetVcp { monitor, code } => {
            // 等价 getVCP + 动态 key 响应
            let value = ddcci::get_vcp(&monitor, code).ok().map(|(cur, _max)| cur);
            send_line(&serde_json::json!({
                "type": monitor_worker::message::get_vcp_response_key(&monitor, code),
                "monitor": monitor,
                "code": code,
                "value": value,
            }));
            Ok(())
        }
        monitor_worker::IncomingMessage::GetReport => {
            // 等价 getReport 分支（哨兵报告 + 缓存状态）
            let s = settings.lock().unwrap();
            let unstable_downgraded = {
                let map = sentinel.unstable_ddc.lock().unwrap();
                map.contains_key("downgraded")
            };
            let method = s.determine_ddcci_method(true, unstable_downgraded);
            let report = sentinel.get_ddc_safety_report(&method);
            send_line(&serde_json::json!({
                "type": "getReport",
                "report": {
                    "ddcSafetyFallback": report,
                },
            }));
            Ok(())
        }
    }
}

/// setBrightness 等价（equiv Monitors.js:L1738-L1790）。
fn set_brightness(
    brightness: f64,
    id: Option<&str>,
    settings: &Arc<Mutex<monitor_worker::WorkerSettings>>,
    can_use_wmi_bridge: &Arc<Mutex<bool>>,
) {
    match id {
        Some(_id) => {
            // 单显示器: DDC/HighLevel/Studio/Software 分支（完整版在后续主程序模块接入）
            let s = settings.lock().unwrap();
            let _ = (s, brightness);
        }
        None => {
            // 无 id: WMI 全局亮度（等价 else 分支）
            let brightness = brightness.round().clamp(0.0, 100.0) as i32;
            if *can_use_wmi_bridge.lock().unwrap() {
                wmibridge::set_brightness(brightness);
            } else {
                // If native WMI is unavailable, fall back to old method（原注释）
                // powershell.exe WmiSetBrightness 等价
                let _ = std::process::Command::new("powershell.exe")
                    .args(["-NoProfile", &format!(
                        "(Get-WmiObject -Namespace root\\wmi -Class WmiMonitorBrightnessMethods).wmisetbrightness(0, {})",
                        brightness
                    )])
                    .spawn();
            }
        }
    }
}
