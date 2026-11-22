//! worker 侧特征快照表（equiv Monitors.js:L422-L423 + L453-L537）。
//!
//! `monitorFeatureSnapshots` 与 `invalidatedFeatureSnapshotMonitorIds` 是 worker 的会话级缓存：
//! Fast 扫描结果先发布，capabilities 补全后用快照把已验证的特征集补回来（原注释语义）。
//! 失效名单用于阻止「用户刚改过该显示器特征」时旧快照被写回。

use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// 特征快照表（等价 `let monitorFeatureSnapshots = {}`）。
#[derive(Debug, Clone, Default)]
pub struct FeatureSnapshots {
    /// key = monitor.hwid.join("#")
    pub snapshots: HashMap<String, Value>,
    /// 失效的显示器 hwid[1] 集合（等价 invalidatedFeatureSnapshotMonitorIds）
    pub invalidated: HashSet<String>,
}

impl FeatureSnapshots {
    /// featureSnapshotKey 等价（equiv L453-L455）：非数组返回 None（等价 false）。
    fn key(monitor: &Value) -> Option<String> {
        let arr = monitor.get("hwid")?.as_array()?;
        Some(
            arr.iter()
                .map(|v| v.as_str().unwrap_or("").to_string())
                .collect::<Vec<_>>()
                .join("#"),
        )
    }

    /// saveFeatureSnapshot 等价（equiv L457-L471）。
    pub fn save(&mut self, monitor: &Value, accept_invalidated: bool) -> bool {
        let ddcci_supported = monitor
            .get("ddcciSupported")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let features_pending = monitor
            .get("featuresPending")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let features_refreshing = monitor
            .get("featuresRefreshing")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !ddcci_supported || features_pending || features_refreshing {
            return false;
        }

        let monitor_id = monitor
            .get("hwid")
            .and_then(|v| v.as_array())
            .and_then(|a| a.get(1))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if self.invalidated.contains(&monitor_id) && !accept_invalidated {
            return false;
        }

        let key = match Self::key(monitor) {
            Some(k) => k,
            None => return false,
        };

        // deepCopy({ features: monitor.features || {}, vcpCodes: monitor.vcpCodes || {} })
        let snapshot = serde_json::json!({
            "features": monitor.get("features").cloned().unwrap_or(serde_json::json!({})),
            "vcpCodes": monitor.get("vcpCodes").cloned().unwrap_or(serde_json::json!({})),
        });
        self.snapshots.insert(key, snapshot);
        if accept_invalidated {
            self.invalidated.remove(&monitor_id);
        }
        true
    }

    /// saveFeatureSnapshots 等价（equiv L473-L477）。
    pub fn save_all(&mut self, monitors: &HashMap<String, Value>, accept_invalidated: bool) {
        for monitor in monitors.values() {
            self.save(monitor, accept_invalidated);
        }
    }

    /// applyFeatureSnapshots 等价（equiv L479-L497）。
    pub fn apply(&self, found_monitors: &mut HashMap<String, Value>) {
        for monitor in found_monitors.values_mut() {
            let ddcci_supported = monitor
                .get("ddcciSupported")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !ddcci_supported
                || monitor.get("type").and_then(|v| v.as_str()) != Some("ddcci")
                || !monitor
                    .get("featuresPending")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            {
                continue;
            }
            let key = match Self::key(monitor) {
                Some(k) => k,
                None => continue,
            };
            let snapshot = match self.snapshots.get(&key) {
                Some(s) => s,
                None => continue,
            };

            // Current Fast readings take precedence (especially brightness), while
            // the last verified feature set keeps controls available during refresh.
            // Keep the session cache isolated from the live model.（原注释）
            let snapshot_features = snapshot
                .get("features")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            let snapshot_vcp_codes = snapshot
                .get("vcpCodes")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();

            let mut merged_features = snapshot_features;
            if let Some(current) = monitor.get("features").and_then(|v| v.as_object()) {
                for (k, v) in current {
                    merged_features.insert(k.clone(), v.clone());
                }
            }
            let mut merged_vcp_codes = snapshot_vcp_codes;
            if let Some(current) = monitor.get("vcpCodes").and_then(|v| v.as_object()) {
                for (k, v) in current {
                    merged_vcp_codes.insert(k.clone(), v.clone());
                }
            }

            monitor["features"] = Value::Object(merged_features);
            monitor["vcpCodes"] = Value::Object(merged_vcp_codes);
            monitor["featuresPending"] = serde_json::json!(false);
            monitor["featuresRefreshing"] = serde_json::json!(true);
        }
    }

    /// invalidateFeatureSnapshots 等价（equiv L531-L537）。
    pub fn invalidate(&mut self, monitor_ids: &HashSet<String>) {
        if monitor_ids.is_empty() {
            return;
        }
        for id in monitor_ids {
            self.invalidated.insert(id.clone());
        }
        let keys: Vec<String> = self.snapshots.keys().cloned().collect();
        for key in keys {
            // key.split("#")[1]
            let second = key.split('#').nth(1).unwrap_or("");
            if monitor_ids.contains(second) {
                self.snapshots.remove(&key);
            }
        }
    }

    /// clear 等价（flushvcp / refreshMonitors 的 clearCache 分支）。
    pub fn clear(&mut self) {
        self.snapshots.clear();
        self.invalidated.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn monitor(hwid2: &str, features: Value) -> Value {
        json!({
            "hwid": ["DISPLAY", "HW1", hwid2],
            "ddcciSupported": true,
            "type": "ddcci",
            "featuresPending": true,
            "features": features,
        })
    }

    #[test]
    fn save_skips_pending_and_refreshing() {
        let mut snapshots = FeatureSnapshots::default();
        // featuresPending=true -> 不保存（equiv L458）
        assert!(!snapshots.save(&monitor("k", json!({ "0x10": [50, 100] })), false));
        let mut ready = monitor("k", json!({ "0x10": [50, 100] }));
        ready["featuresPending"] = json!(false);
        assert!(snapshots.save(&ready, false));
        ready["featuresRefreshing"] = json!(true);
        assert!(!snapshots.save(&ready, false));
    }

    #[test]
    fn apply_merges_current_over_snapshot() {
        let mut snapshots = FeatureSnapshots::default();
        let mut first = monitor("k", json!({ "0x10": [10, 100], "0x12": [20, 100] }));
        first["featuresPending"] = json!(false);
        snapshots.save(&first, false);

        let mut found: HashMap<String, Value> = HashMap::new();
        // 当前 Fast 读数只带 0x10，快照带 0x12 -> 合并后两者都在，且 0x10 取当前值
        found.insert("k".to_string(), monitor("k", json!({ "0x10": [70, 100] })));
        snapshots.apply(&mut found);
        let merged = &found["k"];
        assert_eq!(merged["features"]["0x10"], json!([70, 100]));
        assert_eq!(merged["features"]["0x12"], json!([20, 100]));
        assert_eq!(merged["featuresPending"], json!(false));
        assert_eq!(merged["featuresRefreshing"], json!(true));
    }

    #[test]
    fn invalidate_marks_and_drops() {
        let mut snapshots = FeatureSnapshots::default();
        let mut monitor_v = monitor("k", json!({ "0x10": [10, 100] }));
        monitor_v["featuresPending"] = json!(false);
        snapshots.save(&monitor_v, false);
        assert_eq!(snapshots.snapshots.len(), 1);

        let mut ids = HashSet::new();
        ids.insert("HW1".to_string());
        snapshots.invalidate(&ids);
        assert!(snapshots.snapshots.is_empty());
        assert!(snapshots.invalidated.contains("HW1"));

        // 失效后普通保存被拒，accept_invalidated 才放行并清除失效标记
        assert!(!snapshots.save(&monitor_v, false));
        assert!(snapshots.save(&monitor_v, true));
        assert!(!snapshots.invalidated.contains("HW1"));
    }
}
