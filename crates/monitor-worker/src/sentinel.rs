//! DDC/CI 崩溃哨兵。
//!
//! equiv: Monitors.js:L307-L455。
//! 哨兵文件在每次高危 DDC/CI 操作前写入、成功后删除。若文件在下次启动时仍存在，
//! 说明上个会话在操作中途死亡，DDC/CI 校验降级为 "fast"（不再请求 capabilities 字符串），
//! 避免每次启动都卡死。（原注释语义逐字保留）
//!
//! 这是原生调用可致整个 worker 崩溃的唯一自愈机制（AGENTS.md §3.2），不得简化。

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// 哨兵状态（等价模块级 unstableDDC 变量）。
pub struct DdcSentinel {
    /// ddc-probe.lock
    pub sentinel_path: PathBuf,
    /// ddc-unstable.json
    pub unstable_ddc_path: PathBuf,
    /// unstableDDC 内容
    pub unstable_ddc: Mutex<HashMap<String, Value>>,
}

impl DdcSentinel {
    /// 等价 L322-L327 的参数解析: --datapath= 前缀去掉 11 字符，缺省 tmpdir。
    pub fn from_args(datapath: Option<&str>) -> Self {
        let dir = match datapath {
            Some(p) => PathBuf::from(p),
            None => std::env::temp_dir(),
        };
        Self {
            sentinel_path: dir.join("ddc-probe.lock"),
            unstable_ddc_path: dir.join("ddc-unstable.json"),
            unstable_ddc: Mutex::new(HashMap::new()),
        }
    }

    /// writeUnstableDDC 等价（equiv L329-L335）。
    pub fn write_unstable_ddc(&self) {
        let map = self.unstable_ddc.lock().unwrap();
        match serde_json::to_string(&*map) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&self.unstable_ddc_path, json) {
                    println!("Couldn't save unstable DDC list {:?}", e);
                }
            }
            Err(e) => println!("Couldn't save unstable DDC list {:?}", e),
        }
    }

    /// checkForDDCCrashEvidence 等价（equiv L337-L365）。
    /// 启动时调用：读 unstableDDC、检测哨兵残留、降级记录。
    pub fn check_for_ddc_crash_evidence(&self) {
        // 读 unstableDDC 缓存
        {
            let mut map = self.unstable_ddc.lock().unwrap();
            match std::fs::read_to_string(&self.unstable_ddc_path) {
                Ok(text) => {
                    *map = serde_json::from_str(&text).unwrap_or_default();
                }
                Err(_) => {
                    map.clear();
                }
            }
        }

        if !self.sentinel_path.exists() {
            return;
        }

        let evidence: Value = std::fs::read_to_string(&self.sentinel_path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        let _ = std::fs::remove_file(&self.sentinel_path);

        println!(
            "\x1b[41mA previous session died during a DDC/CI operation. Downgrading DDC/CI validation to \"fast\".\x1b[0m {:?}",
            evidence
        );

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let stage = evidence.get("stage").cloned().unwrap_or(Value::Null);
        let monitor = evidence.get("monitor").cloned().unwrap_or(Value::Null);

        let mut map = self.unstable_ddc.lock().unwrap();
        map.insert(
            "downgraded".to_string(),
            serde_json::json!({
                "time": now_ms,
                "stage": stage,
                "monitor": monitor,
            }),
        );
        if !monitor.is_null() {
            map.insert(
                monitor.as_str().unwrap_or("").to_string(),
                serde_json::json!({ "time": now_ms, "stage": stage }),
            );
        }
        drop(map);
        self.write_unstable_ddc();
    }

    /// getDDCSafetyReport 等价（equiv L367-L387）。
    /// effective_validation_method 由调用方传入（determineDDCCIMethod 的结果）。
    pub fn get_ddc_safety_report(&self, effective_method: &str) -> Value {
        let map = self.unstable_ddc.lock().unwrap();
        let fallback = map.get("downgraded").cloned().unwrap_or(Value::Bool(false));
        let is_fallback = !fallback.is_boolean();
        let affected_displays: Vec<Value> = map
            .iter()
            .filter(|(id, _)| id.as_str() != "downgraded")
            .map(|(id, evidence)| {
                serde_json::json!({
                    "id": id,
                    "stage": evidence.get("stage").cloned().unwrap_or(Value::Bool(false)),
                    "time": evidence.get("time").cloned().unwrap_or(Value::Bool(false)),
                })
            })
            .collect();

        serde_json::json!({
            "active": is_fallback,
            "fallbackInEffect": is_fallback && effective_method == "fast",
            "effectiveValidationMethod": effective_method,
            "fallbackValidationMethod": if is_fallback { json!("fast") } else { Value::Bool(false) },
            "detectedAt": fallback.get("time").cloned().unwrap_or(Value::Bool(false)),
            "stage": fallback.get("stage").cloned().unwrap_or(Value::Bool(false)),
            "affectedDisplays": affected_displays,
        })
    }

    /// ddcSentinelStart 等价（equiv L389-L394）。
    pub fn sentinel_start(&self, stage: &str, monitor: Option<&str>) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let payload = serde_json::json!({
            "stage": stage,
            "monitor": monitor,
            "time": now_ms,
        });
        if let Err(_e) = std::fs::write(&self.sentinel_path, payload.to_string()) {
            // 原版 catch(e) {} 静默
        }
    }

    /// ddcSentinelEnd 等价（equiv L395-L400）。
    pub fn sentinel_end(&self) {
        let _ = std::fs::remove_file(&self.sentinel_path);
    }

    /// withDDCSentinel 等价（equiv L401-L410）。
    /// 原生崩溃不会走到 sentinel_end（留下证据）；普通 Err 会走到（不算崩溃）。
    /// 原注释语义逐字保留。
    pub fn with_ddc_sentinel<T, F>(&self, stage: &str, monitor: Option<&str>, operation: F) -> T
    where
        F: FnOnce() -> T,
    {
        self.sentinel_start(stage, monitor);
        let result = operation();
        self.sentinel_end();
        result
    }
}

/// 等价 L412: 优雅退出时清哨兵（process.on('exit')）。注册为进程退出钩子。
pub fn register_exit_cleanup(sentinel: &'static DdcSentinel) {
    // Rust 无 process.on('exit')；等价实现：Drop guard 由 main 持有 +
    // panic hook 兜底。这里提供显式调用入口。
    // 注意: 原生崩溃（segfault）不会触发任何清理——这正是哨兵的设计意图。
    sentinel.sentinel_end();
}

/// 等价模块级单例检查（Monitors.js 在模块加载时立即执行 checkForDDCCrashEvidence）。
pub fn check_for_ddc_crash_evidence(sentinel: &DdcSentinel) {
    sentinel.check_for_ddc_crash_evidence();
}
