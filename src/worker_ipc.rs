//! Monitors worker 的 IPC 循环与消息分发。
//!
//! equiv: Monitors.js:L175-L305（handleMonitorMessage）+ L2285-L2299（模块尾部的
//! process.send({type:'ready'}) 与串行消息队列）。
//!
//! AGENTS.md §5：worker 子进程不依赖主进程状态；IPC 用 JSON 行协议，
//! 消息类型与原版一一对应（ready/settings/ddcBrightnessVCPs/wmi-bridge-ok/...）。
//! 串行语义等价 `monitorMessageQueue = monitorMessageQueue.then(() => handleMonitorMessage(data))`。

use monitor_worker::all_monitors::get_all_monitors;
use monitor_worker::brightness_ops::{
    is_ddc_blocked_str, read_known_brightness, set_brightness, set_sdr_brightness, set_vcp_for,
};
use monitor_worker::feature_snapshots::FeatureSnapshots;
use monitor_worker::features;
use monitor_worker::monitors::update_display;
use monitor_worker::settings_state::WorkerSettings;
use monitor_worker::studio_display_discovery::set_apple_studio_unavailable;
use monitor_worker::wmi_fallback::{self, set_monitors_win32, BRIDGE_LISTING_FAILURES};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

/// worker 的常驻状态（等价 Monitors.js 模块级 let 变量集合）。
pub struct WorkerState {
    /// `let monitors = false`（未刷新时为 None）
    pub monitors: Option<HashMap<String, Value>>,
    /// `let settings = { order: [] }`
    pub settings: WorkerSettings,
    /// `let localization = {}`
    pub localization: Value,
    /// `let ddcBrightnessVCPs = {}`
    pub ddc_brightness_vcps: Value,
    /// `let monitorsAppleStudio = {}`
    pub monitors_apple_studio: HashMap<String, Value>,
    /// `let lastRefresh = {}`（getReport 使用）
    pub last_refresh: Value,
    /// `let lastDDCCIList = []`（getReport 使用）
    pub last_ddcci_list: Value,
    /// `let lastWMI / lastWin32 / lastHDR`（getReport 使用）
    pub last_wmi: Value,
    pub last_win32: Value,
    pub last_hdr: Value,
    /// `let busyLevel = 0`
    pub busy_level: u32,
    /// `let isFastFine = true`
    pub is_fast_fine: bool,
    /// `let userDisabledMethods = new Set()`
    pub user_disabled_methods: std::collections::HashSet<String>,
    /// `let monitorFeatureSnapshots = {}` + `invalidatedFeatureSnapshotMonitorIds`
    pub snapshots: FeatureSnapshots,
}

impl Default for WorkerState {
    fn default() -> Self {
        Self {
            monitors: None,
            // equiv `let settings = { order: [] }`
            settings: WorkerSettings::from_value(json!({ "order": [] })),
            localization: json!({}),
            ddc_brightness_vcps: json!({}),
            monitors_apple_studio: HashMap::new(),
            last_refresh: json!({}),
            last_ddcci_list: json!([]),
            last_wmi: json!({}),
            last_win32: json!({}),
            last_hdr: json!({}),
            busy_level: 0,
            is_fast_fine: true,
            user_disabled_methods: std::collections::HashSet::new(),
            snapshots: FeatureSnapshots::default(),
        }
    }
}

impl WorkerState {
    /// localization.GENERIC_DISPLAY_SINGLE（等价 L965 的兜底名字前缀）。
    pub fn generic_display_name(&self) -> String {
        self.localization
            .get("GENERIC_DISPLAY_SINGLE")
            .or_else(|| self.localization.get("GENERIC_DISPLAY_SINGLE".to_ascii_lowercase().as_str()))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }

    /// determineDDCCIMethod 等价（equiv L980-L993）：以 isFastFine + 崩溃降级为输入。
    fn determine_ddcci_method(&self, unstable_downgraded: bool) -> String {
        self.settings
            .determine_ddcci_method(self.is_fast_fine, unstable_downgraded)
    }

    /// shouldEnrichCapabilities 等价（equiv L995-L1003）。
    fn should_enrich_capabilities(&self, unstable_downgraded: bool) -> bool {
        self.settings.should_enrich_capabilities(unstable_downgraded)
    }

    /// clearFeatureSnapshots 等价（flushvcp / clearCache 分支）。
    fn clear_feature_snapshots(&mut self) {
        self.snapshots.clear();
    }

    /// saveFeatureSnapshots 委托（需要 &mut 才能把快照写回状态）。
    fn save_feature_snapshots(&mut self, monitors: &HashMap<String, Value>, accept_invalidated: bool) {
        self.snapshots.save_all(monitors, accept_invalidated);
    }

    /// invalidateFeatureSnapshots 委托。
    fn invalidate_feature_snapshots(&mut self, ids: &std::collections::HashSet<String>) {
        self.snapshots.invalidate(ids);
    }

    /// applyFeatureSnapshots 委托。
    fn apply_feature_snapshots(&self, found: &mut HashMap<String, Value>) {
        self.snapshots.apply(found);
    }
}

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

    // ddcci 日志级别（equiv L1997: isDev ? 2 : 1）
    ddcci::set_log_level(if is_dev { 2 } else { 1 });

    // WMIC 可用性在首次查询前先探测（equiv L2046-L2051）
    wmi_fallback::check_wmic_availability();

    // 哨兵初始化（equiv 模块加载时立即执行的 checkForDDCCrashEvidence）
    let sentinel: &'static monitor_worker::DdcSentinel = Box::leak(Box::new(
        monitor_worker::DdcSentinel::from_args(datapath),
    ));
    monitor_worker::sentinel::install_sentinel(sentinel);
    sentinel.check_for_ddc_crash_evidence();

    let state = Arc::new(Mutex::new(WorkerState::default()));

    // process.send({type:'ready'})（等价 L2286-L2288）
    send_line(&json!({ "type": "ready" }));
    let _ = (skip_test, is_dev);

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
                match serde_json::from_str::<Value>(trimmed) {
                    Ok(data) => {
                        // 等价 handleMonitorMessage 的 try/catch：异常只记录，队列继续
                        if let Err(e) = handle_message(&data, &state) {
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

/// handleMonitorMessage 等价（equiv L175-L305）。
pub fn handle_message(data: &Value, state: &Arc<Mutex<WorkerState>>) -> Result<(), String> {
    let msg_type = data.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match msg_type {
        "refreshMonitors" => {
            let full_refresh = data.get("fullRefresh").and_then(|v| v.as_bool()).unwrap_or(false);
            let clear_cache = data.get("clearCache").and_then(|v| v.as_bool()).unwrap_or(false);
            let generation = data.get("generation").cloned().unwrap_or(Value::Null);

            if clear_cache {
                // 等价 L178-L184：清 vcpCache / monitorReports / monitorReportsRaw /
                // monitorFeatureSnapshots / invalidatedFeatureSnapshotMonitorIds
                features::clear_vcp_cache();
                state.lock().unwrap().clear_feature_snapshots();
            }

            let results = refresh_monitors(state, full_refresh);
            let can_enrich = full_refresh && {
                let s = state.lock().unwrap();
                let downgraded = sentinel_downgraded();
                s.should_enrich_capabilities(downgraded)
            };
            {
                let mut s = state.lock().unwrap();
                s.last_refresh = results.clone();
            }
            send_line(&json!({
                "type": "refreshMonitors",
                "monitors": results,
                "generation": generation,
                "canEnrichCapabilities": can_enrich,
            }));
            Ok(())
        }
        "readKnownBrightness" => {
            let request_id = data.get("requestId").cloned().unwrap_or(Value::Null);
            let brightness = {
                let mut s = state.lock().unwrap();
                let settings = s.settings.clone();
                let mut monitors = s.monitors.take();
                let result = match monitors.as_mut() {
                    Some(m) => read_known_brightness(m, &settings),
                    None => HashMap::new(),
                };
                s.monitors = monitors;
                result
            };
            send_line(&json!({
                "type": "knownBrightness",
                "brightness": brightness,
                "requestId": request_id,
            }));
            Ok(())
        }
        "enrichMonitorCapabilities" => {
            let generation = data.get("generation").cloned().unwrap_or(Value::Null);
            let results = enrich_monitor_capabilities(state);
            let failed = results.is_none();
            if let Some(v) = results.as_ref() {
                let mut s = state.lock().unwrap();
                s.last_refresh = v.clone();
            }
            send_line(&json!({
                "type": "monitorsEnriched",
                "monitors": results.unwrap_or(Value::Bool(false)),
                "generation": generation,
                "failed": failed,
            }));
            Ok(())
        }
        "brightness" => {
            let brightness = data.get("brightness").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let id = data.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
            let mut s = state.lock().unwrap();
            let settings = s.settings.clone();
            let vcps = s.ddc_brightness_vcps.clone();
            if let Some(mut monitors) = s.monitors.take() {
                set_brightness(&mut monitors, &settings, brightness, id.as_deref(), &vcps);
                s.monitors = Some(monitors);
            }
            Ok(())
        }
        "sdr" => {
            let brightness = data.get("brightness").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let id = data.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let s = state.lock().unwrap();
            set_sdr_brightness(&s.settings, brightness, &id);
            Ok(())
        }
        "gamma" => {
            let brightness = data.get("brightness").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let id = data.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let mut s = state.lock().unwrap();
            if let Some(monitors) = s.monitors.as_mut() {
                monitor_worker::software_brightness::set_gamma_brightness(monitors, brightness, &id);
            }
            Ok(())
        }
        "settings" => {
            let new_settings = data.get("settings").cloned().unwrap_or(Value::Null);
            handle_settings_update(state, new_settings);
            Ok(())
        }
        "ddcBrightnessVCPs" => {
            let new_vcps = data
                .get("ddcBrightnessVCPs")
                .cloned()
                .unwrap_or(json!({}));
            handle_ddc_brightness_vcps(state, new_vcps);
            Ok(())
        }
        "localization" => {
            let localization = data.get("localization").cloned().unwrap_or(json!({}));
            state.lock().unwrap().localization = localization;
            Ok(())
        }
        "vcp" => {
            let monitor = data.get("monitor").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let code = data.get("code").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
            let value = data.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let s = state.lock().unwrap();
            set_vcp_for(&s.settings, &monitor, code, value);
            Ok(())
        }
        "flushvcp" => {
            // 等价 L266-L272：清全部 worker 缓存 + ddcci._clearDisplayCache()
            features::clear_vcp_cache();
            {
                let mut s = state.lock().unwrap();
                s.clear_feature_snapshots();
            }
            ddcci::clear_display_cache();
            Ok(())
        }
        "wmi-bridge-ok" => {
            let value = data.get("value").and_then(|v| v.as_bool()).unwrap_or(false);
            monitor_worker::brightness_ops::set_can_use_wmi_bridge(value);
            Ok(())
        }
        "getVCP" => {
            let monitor = data.get("monitor").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let code = data.get("code").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
            let s = state.lock().unwrap();
            let value = if is_ddc_blocked_str(&s.settings, &monitor) {
                Value::Bool(false)
            } else {
                let wait = s.settings.get("checkVCPWaitMS").and_then(|v| v.as_u64()).unwrap_or(20);
                features::check_vcp(
                    &monitor,
                    code as i64,
                    false,
                    true,
                    wait,
                    s.settings.debug_force_brightness_max(),
                )
            };
            send_line(&json!({
                "type": format!("getVCP::{}::{}", monitor, code),
                "monitor": monitor,
                "code": code,
                "value": value,
            }));
            Ok(())
        }
        "getReport" => {
            let report = build_report(state);
            send_line(&json!({ "type": "getReport", "report": report }));
            Ok(())
        }
        _ => {
            // 原版对未知 type 静默（if/else 链末尾无 else）
            Ok(())
        }
    }
}

/// sentinel 的 downgraded 标志（等价 `unstableDDC.downgraded` 真值）。
fn sentinel_downgraded() -> bool {
    monitor_worker::sentinel_current_unstable().contains_key("downgraded")
}

/// refreshMonitors 等价（equiv L539-L638）。
fn refresh_monitors(state: &Arc<Mutex<WorkerState>>, full_refresh: bool) -> Value {
    {
        let mut s = state.lock().unwrap();
        // `if ((busyLevel > 0 && !fullRefresh) || (busyLevel > 0 && fullRefresh))`
        // —— 原式恒等价于 busyLevel > 0，逐字保留其语义：忙碌时直接取消。
        if s.busy_level > 0 {
            println!("Thread busy. Cancelling refresh.");
            return match s.monitors.as_ref() {
                Some(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
                None => Value::Bool(false),
            };
        }
        s.busy_level = if full_refresh { 2 } else { 1 };
    }

    let result = refresh_monitors_inner(state, full_refresh);

    {
        let mut s = state.lock().unwrap();
        s.busy_level = 0;
    }
    result
}

fn refresh_monitors_inner(state: &Arc<Mutex<WorkerState>>, full_refresh: bool) -> Value {
    let needs_full = {
        let s = state.lock().unwrap();
        s.monitors.is_none() || full_refresh
    };

    if needs_full {
        // if (fullRefresh) saveFeatureSnapshots(monitors)
        if full_refresh {
            let mut s = state.lock().unwrap();
            let monitors = s.monitors.clone();
            if let Some(monitors) = monitors.as_ref() {
                s.save_feature_snapshots(monitors, false);
            }
        }

        let (ddcci_method, core_only, settings, generic, vcps) = {
            let s = state.lock().unwrap();
            let downgraded = sentinel_downgraded();
            let use_enrichment = s.should_enrich_capabilities(downgraded);
            let method = if use_enrichment {
                "fast".to_string()
            } else {
                s.determine_ddcci_method(downgraded)
            };
            (
                method,
                use_enrichment,
                s.settings.clone(),
                s.generic_display_name(),
                s.ddc_brightness_vcps.clone(),
            )
        };

        let mut found = get_all_monitors(&settings, &ddcci_method, core_only, &generic, &vcps);
        if core_only {
            let s = state.lock().unwrap();
            s.apply_feature_snapshots(&mut found);
        }

        {
            let mut s = state.lock().unwrap();
            s.monitors = Some(found.clone());
            s.last_ddcci_list = json!(monitor_worker::monitors::get_all_monitors_report());
            s.last_win32 = json!(wmi_fallback::monitors_win32_snapshot());
        }
        return json!(found);
    }

    refresh_monitors_incremental(state)
}

/// refreshMonitors 的增量分支（equiv L557-L633）：只刷新读数，不重新枚举。
fn refresh_monitors_incremental(state: &Arc<Mutex<WorkerState>>) -> Value {
    let (settings, get_updates) = {
        let s = state.lock().unwrap();
        (s.settings.clone(), s.settings.get_ddc_brightness_updates())
    };

    {
        let mut s = state.lock().unwrap();
        let is_fast_fine = s.is_fast_fine;
        let Some(mut monitors) = s.monitors.take() else {
            return Value::Bool(false);
        };

        // DDC/CI 读数（equiv L560-L581）
        if get_updates {
            let blocked = settings.ddc_blocked_keys();
            let unstable = sentinel_downgraded();
            let method = settings.determine_ddcci_method(is_fast_fine, unstable);
            let _ = ddcci::refresh(
                &method,
                true,
                !settings.disable_high_level(),
                &blocked,
            );
            let keys: Vec<String> = monitors.keys().cloned().collect();
            for hwid2 in keys {
                let is_ddcci = monitors
                    .get(&hwid2)
                    .map(|m| m.get("type").and_then(|v| v.as_str()) == Some("ddcci"))
                    .unwrap_or(false);
                if !is_ddcci {
                    continue;
                }
                let monitor = monitors.get(&hwid2).cloned().unwrap_or(json!({}));
                let (refreshed, _failed) =
                    monitor_worker::brightness_ops::get_brightness_ddc(&settings, &monitor, true, false);
                monitors.insert(hwid2, refreshed);
            }
        }

        // Internal display brightness（equiv L584-L601）
        if wmi_fallback::can_use_internal_brightness() {
            let remaps = settings.get("remaps").cloned().unwrap_or(json!({}));
            if let Ok(Some(wmi_brightness)) = wmi_fallback::get_brightness_internal(&remaps) {
                if let Some(hwid2) = wmi_brightness
                    .get("hwid")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.get(2))
                    .and_then(|v| v.as_str())
                    .map(|x| x.to_string())
                {
                    update_display(&mut monitors, &hwid2, wmi_brightness);
                    if settings.hide_closed_lid()
                        && !wmi_fallback::monitors_win32_snapshot().contains_key(&hwid2)
                    {
                        update_display(&mut monitors, &hwid2, json!({ "type": "none" }));
                    }
                }
            }
        }

        // Apple Studio displays（equiv L604-L612）
        if !monitor_worker::wmi_fallback::APPLE_STUDIO_UNAVAILABLE
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            monitor_worker::studio_display_discovery::get_studio_display(&mut monitors);
        }

        // HDR（equiv L615-L623）
        if !settings.disable_hdr() {
            wmi_fallback::get_hdr_displays(&mut monitors);
        }

        // Hide internal（equiv L626-L631）
        if settings.hide_closed_lid() {
            let wmi_key = monitors.iter().find_map(|(k, m)| {
                if m.get("type").and_then(|v| v.as_str()) == Some("wmi") {
                    Some(k.clone())
                } else {
                    None
                }
            });
            if let Some(key) = wmi_key {
                let hwid2 = monitors
                    .get(&key)
                    .and_then(|m| m.get("hwid"))
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.get(2))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !wmi_fallback::monitors_win32_snapshot().contains_key(&hwid2) {
                    update_display(&mut monitors, &hwid2, json!({ "type": "none" }));
                }
            }
        }

        s.monitors = Some(monitors);
        json!(s.monitors.as_ref().unwrap())
    }
}

/// enrichMonitorCapabilities 等价（equiv L724-L767）。
/// 返回 None 等价原版 false（跳过/失败）。
fn enrich_monitor_capabilities(state: &Arc<Mutex<WorkerState>>) -> Option<Value> {
    {
        let s = state.lock().unwrap();
        let downgraded = sentinel_downgraded();
        if s.busy_level > 0 || s.monitors.is_none() || !s.should_enrich_capabilities(downgraded) {
            println!("Skipping capabilities enrichment.");
            return None;
        }
    }
    {
        let mut s = state.lock().unwrap();
        s.busy_level = 2;
    }

    let result = (|| {
        let (settings, generic, vcps, list) = {
            let s = state.lock().unwrap();
            (
                s.settings.clone(),
                s.generic_display_name(),
                s.ddc_brightness_vcps.clone(),
                s.last_ddcci_list.clone(),
            )
        };

        // 逐台补 capabilities（原版只补尚未有报告的显示器）
        if let Some(items) = list.as_array() {
            for item in items {
                let id = item.get("deviceKey").and_then(|v| v.as_str()).unwrap_or("");
                if id.is_empty() || is_ddc_blocked_str(&settings, id) {
                    continue;
                }
                let unstable = sentinel_downgraded();
                let _ = unstable;
                match ddcci::get_capabilities_string_impl(id) {
                    Ok(raw) if !raw.is_empty() => {
                        if let Some(report) = ddcci::parse_capabilities_string(&raw) {
                            if !report.is_empty() {
                                if let Ok(value) = serde_json::to_value(&report) {
                                    features::set_monitor_report(id, value);
                                }
                            }
                        }
                    }
                    _ => println!("Couldn't get capabilities report for monitor {}", id),
                }
            }
        }

        // monitors = await getAllMonitors("fast", false)（原注释：不重复请求 capabilities）
        let mut found = get_all_monitors(&settings, "fast", false, &generic, &vcps);
        {
            let mut s = state.lock().unwrap();
            s.save_feature_snapshots(&found, true);
        }
        {
            let mut s = state.lock().unwrap();
            s.monitors = Some(found.clone());
        }
        let _ = &mut found;
        Some(json!(found))
    })();

    {
        let mut s = state.lock().unwrap();
        s.busy_level = 0;
    }
    if result.is_none() {
        println!("Couldn't enrich monitor capabilities");
    }
    result
}

/// settings 消息分支（equiv L215-L227）：失效快照 + 软件亮度恢复 + 方法覆写。
fn handle_settings_update(state: &Arc<Mutex<WorkerState>>, new_settings: Value) {
    let mut s = state.lock().unwrap();
    let previous = s.settings.raw.clone();
    let changed = monitor_worker::monitors::changed_feature_monitor_ids(&previous, &new_settings);
    let had_software_brightness = s.settings.use_software_brightness_fallback();
    s.settings = WorkerSettings::from_value(new_settings);
    s.invalidate_feature_snapshots(&changed);

    if had_software_brightness && !s.settings.use_software_brightness_fallback() {
        if let Some(monitors) = s.monitors.as_mut() {
            monitor_worker::software_brightness::restore_software_brightness(monitors);
        }
    }

    // applyMethodOverride（equiv L224-L227）：只有设置本身置位时才清除标志
    apply_method_override(&mut s, "disableAppleStudio");
    apply_method_override(&mut s, "disableWMIC");
    apply_method_override(&mut s, "disableWMI");
    apply_method_override(&mut s, "disableWin32");
}

/// applyMethodOverride 等价（equiv L502-L509）。
fn apply_method_override(state: &mut WorkerState, setting: &str) {
    let enabled = state
        .settings
        .get(setting)
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if enabled {
        state.user_disabled_methods.insert(setting.to_string());
        set_method_disabled(setting, true);
    } else if state.user_disabled_methods.remove(setting) {
        set_method_disabled(setting, false);
    }
}

fn set_method_disabled(setting: &str, disabled: bool) {
    use std::sync::atomic::Ordering;
    match setting {
        "disableAppleStudio" => set_apple_studio_unavailable(disabled),
        // applyMethodOverride("disableWMIC", value => wmicUnavailable = (value || !wmicExists()))
        "disableWMIC" => wmi_fallback::WMIC_UNAVAILABLE
            .store(disabled || !wmi_fallback::wmic_exists(), Ordering::SeqCst),
        "disableWMI" => wmi_fallback::WMI_FAILED.store(disabled, Ordering::SeqCst),
        "disableWin32" => wmi_fallback::WIN32_FAILED.store(disabled, Ordering::SeqCst),
        _ => {}
    }
}

/// ddcBrightnessVCPs 消息分支（equiv L229-L261）：失效快照 + 覆写 brightnessType。
fn handle_ddc_brightness_vcps(state: &Arc<Mutex<WorkerState>>, new_vcps: Value) {
    let mut s = state.lock().unwrap();
    let previous = json!({ "userDDCBrightnessVCPs": s.ddc_brightness_vcps });
    let next = json!({ "userDDCBrightnessVCPs": new_vcps });
    let changed = monitor_worker::monitors::changed_feature_monitor_ids(&previous, &next);
    s.ddc_brightness_vcps = new_vcps;
    s.invalidate_feature_snapshots(&changed);

    if changed.is_empty() {
        return;
    }
    let vcps = s.ddc_brightness_vcps.clone();
    if let Some(monitors) = s.monitors.as_mut() {
        for (_hwid2, monitor) in monitors.iter_mut() {
            if monitor.get("type").and_then(|v| v.as_str()) != Some("ddcci") {
                continue;
            }
            let hwid1 = monitor
                .get("hwid")
                .and_then(|v| v.as_array())
                .and_then(|a| a.get(1))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !changed.contains(&hwid1) {
                continue;
            }
            match vcps.get(&hwid1).and_then(|v| v.as_i64()) {
                Some(code) if (0..=0xFF).contains(&code) => {
                    // Custom VCP code set - use it（原注释）
                    monitor["brightnessType"] = json!(code);
                }
                Some(_) => {
                    // Invalid VCP code, fall back to default（原注释）
                    monitor["brightnessType"] = json!(0x10);
                }
                None => {
                    // Override removed - reset to default (0x10 = 16)
                    // until the refresh that follows detects it again（原注释）
                    monitor["brightnessType"] = json!(0x10);
                }
            }
        }
    }
}

/// getReport 响应体（equiv L284-L300）。
fn build_report(state: &Arc<Mutex<WorkerState>>) -> Value {
    let s = state.lock().unwrap();
    let unstable = monitor_worker::sentinel_current_unstable();
    let downgraded = unstable.contains_key("downgraded");
    let method = s.determine_ddcci_method(downgraded);
    let safety = match monitor_worker::sentinel::sentinel() {
        Some(sentinel) => sentinel.get_ddc_safety_report(&method),
        None => Value::Bool(false),
    };
    json!({
        "lastDDCCIList": s.last_ddcci_list,
        "lastWMI": s.last_wmi,
        "lastWin32": s.last_win32,
        "lastHDR": s.last_hdr,
        "monitorsAppleStudio": s.monitors_apple_studio,
        "monitorReports": features::monitor_reports_snapshot(),
        "monitorReportsRaw": features::monitor_reports_raw_snapshot(),
        "lastRefresh": s.last_refresh,
        "settings": s.settings.raw,
        "unstableDDC": unstable,
        "ddcSafetyFallback": safety,
    })
}

/// 未使用的导入占位（BRIDGE_LISTING_FAILURES 由 wmi_fallback 内部维护）。
#[allow(dead_code)]
fn _touch() {
    let _ = BRIDGE_LISTING_FAILURES.load(std::sync::atomic::Ordering::SeqCst);
    let _ = set_monitors_win32(&HashMap::new());
}

