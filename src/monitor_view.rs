//! 显示器模型 -> Slint UI 模型的映射层。
//!
//! equiv: origin/src/components/BrightnessPanel.jsx 的渲染分支判定
//! （numMonitors / isAdjustableDisplay / usesGammaSlider / useFeatures / hasFeatures /
//! showPowerButton / isHDROnlySDR）+ components/DDCCISliders.jsx 的特征行构建
//! + components/utilts/monitor.util.js + volume-utils.js 的音量换算。
//!
//! 关键：UI 只吃 worker 回包的 monitors 对象，任何判定都在这里按原 JS 语义重算，
//! 不在 Slint 侧做业务判断（保持单一事实来源）。

use crate::ui_types::{
    DisplayType, FeatureKind, PanelFeature, PanelInput, PanelMonitor,
};
use serde_json::{json, Value};
use slint::{ModelRc, VecModel};
use std::rc::Rc;

/// 输入源名称表（equiv DDCCISliders.jsx inputsData，逐项照抄）。
pub const INPUTS_DATA: &[(i32, &str)] = &[
    (1, "VGA-1"),
    (2, "VGA-2"),
    (3, "DVI-1"),
    (4, "DVI-2"),
    (5, "Composite video 1"),
    (6, "Composite video 2"),
    (7, "S-Video-1"),
    (8, "S-Video-2"),
    (9, "Tuner-1"),
    (10, "Tuner-2"),
    (11, "Tuner-3"),
    (12, "Component video (YPrPb/YCrCb) 1"),
    (13, "Component video (YPrPb/YCrCb) 2"),
    (14, "Component video (YPrPb/YCrCb) 3"),
    (15, "DisplayPort-1"),
    (16, "DisplayPort-2"),
    (17, "HDMI-1"),
    (18, "HDMI-2"),
];

/// getMonitorName（equiv components/utilts/monitor.util.js:L1-L7）。
/// 注意：面板用的是 utils/BrightnessPanel/getMonitorName.js（不带 " (name)" 后缀），
/// 设置页用的是 utilts 版本（带后缀）——两者原版就不一致，这里分开保留。
pub fn panel_monitor_name(monitor: &Value, renames: &Value) -> String {
    let id = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("");
    if let Some(renamed) = renames.get(id).and_then(|v| v.as_str()) {
        if !renamed.is_empty() {
            return renamed.to_string();
        }
    }
    monitor
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// getMonitorName 设置页版本（equiv components/utilts/monitor.util.js:L1-L7）。
pub fn settings_monitor_name(monitor: &Value, renames: &Value) -> String {
    let id = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let name = monitor.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if let Some(renamed) = renames.get(id).and_then(|v| v.as_str()) {
        if !renamed.is_empty() {
            return format!("{} ({})", renamed, name);
        }
    }
    name.to_string()
}

/// usesGammaSlider（equiv monitor.util.js:L19-L23）。
/// gammaAsMainSliderDisplays[key] 且 hdr != "active" 且 gammaBrightness >= 0。
pub fn uses_gamma_slider(monitor: &Value, settings: &Value) -> bool {
    let key = monitor.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let opted = settings
        .get("gammaAsMainSliderDisplays")
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !opted {
        return false;
    }
    if monitor.get("hdr").and_then(|v| v.as_str()) == Some("active") {
        return false;
    }
    monitor
        .get("gammaBrightness")
        .and_then(|v| v.as_f64())
        .map(|v| v >= 0.0)
        .unwrap_or(false)
}

/// isAdjustableDisplay（equiv monitor.util.js:L27-L29）。
pub fn is_adjustable_display(monitor: &Value, settings: &Value) -> bool {
    monitor.get("type").and_then(|v| v.as_str()) != Some("none") || uses_gamma_slider(monitor, settings)
}

/// 显示器是否可见（equiv numMonitors 与渲染分支的 hideDisplays 判定）。
pub fn is_visible_monitor(monitor: &Value, settings: &Value) -> bool {
    let key = monitor.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let hidden = settings
        .get("hideDisplays")
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if hidden {
        return false;
    }
    let hdr_active = monitor.get("hdr").and_then(|v| v.as_str()) == Some("active");
    (is_adjustable_display(monitor, settings) || hdr_active) && !hidden
}

/// getVolumeMaximum（equiv volume-utils.js:L1-L14）。
fn get_volume_maximum(feature: &Value, settings: &Value) -> f64 {
    if let Some(override_v) = settings.get("maxRawValue").and_then(|v| v.as_f64()) {
        if override_v >= 1.0 && override_v <= 65535.0 {
            return override_v;
        }
    }
    let reported = feature.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
    let current = feature.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
    // 有些显示器报 65535 但只接受 0-100；当前值 <= 100 说明是这种
    if reported == 65535.0 && current <= 100.0 {
        return 100.0;
    }
    if reported > 0.0 {
        reported
    } else {
        100.0
    }
}

/// getVolumeRange（equiv volume-utils.js:L17-L22）。
fn get_volume_range(settings: &Value) -> (f64, f64) {
    let min = settings.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let max = settings.get("max").and_then(|v| v.as_f64()).unwrap_or(100.0);
    if min >= 0.0 && max <= 100.0 && min <= max {
        (min, max)
    } else {
        (0.0, 100.0)
    }
}

/// unscaleVolume（equiv volume-utils.js:L30-L35）：原始 DDC 值 -> 0..100 百分比。
pub fn unscale_volume(value: f64, feature: &Value, settings: &Value) -> f64 {
    let (min, max) = get_volume_range(settings);
    if min == max {
        return 0.0;
    }
    let percent = value * 100.0 / get_volume_maximum(feature, settings);
    ((percent - min) * 100.0 / (max - min)).clamp(0.0, 100.0)
}

/// scaleVolume（equiv volume-utils.js:L24-L28）：0..100 百分比 -> 原始 DDC 值。
pub fn scale_volume(value: f64, feature: &Value, settings: &Value) -> f64 {
    let (min, max) = get_volume_range(settings);
    let percent = value.clamp(0.0, 100.0);
    ((min + percent * (max - min) / 100.0) * get_volume_maximum(feature, settings) / 100.0).round()
}

/// getCalibratedValue 等价（equiv Utils.js:L302-L371）：
/// calibrationPoints 为 [{input, output}]；normalize=true 表示"从 worker 读回"（反向映射）。
/// 缺失的 0/100 端点按原版规则补齐，点按 input 升序排序。
pub fn get_calibrated_value(
    value: f64,
    calibration_points: &[(f64, f64)],
    reverse: bool,
) -> f64 {
    let value = value.clamp(0.0, 100.0);
    let mut points: Vec<(f64, f64)> = calibration_points
        .iter()
        .map(|(i, o)| (i.clamp(0.0, 100.0), *o))
        .collect();
    let has_min = points.iter().any(|(i, _)| *i == 0.0);
    let has_max = points.iter().any(|(i, _)| *i == 100.0);
    if !has_min {
        points.insert(0, (0.0, 0.0));
    }
    if !has_max {
        points.push((100.0, 100.0));
    }
    points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    if reverse {
        for pair in points.windows(2) {
            let (p1, p2) = (pair[0], pair[1]);
            let min_out = p1.1.min(p2.1);
            let max_out = p1.1.max(p2.1);
            if value >= min_out && value <= max_out {
                if p2.1 == p1.1 {
                    return (p1.0 + p2.0) / 2.0;
                }
                let ratio = (value - p1.1) / (p2.1 - p1.1);
                return p1.0 + ratio * (p2.0 - p1.0);
            }
        }
        value
    } else {
        if value == 0.0 {
            if let Some(first) = points.first() {
                if first.0 == 0.0 {
                    return first.1;
                }
            }
        }
        for pair in points.windows(2) {
            let (p1, p2) = (pair[0], pair[1]);
            if value >= p1.0 && value <= p2.0 {
                let ratio = (value - p1.0) / (p2.0 - p1.0);
                return p1.1 + ratio * (p2.1 - p1.1);
            }
        }
        value
    }
}

/// normalizeBrightness 等价（equiv electron.js:L3394-L3423）。
/// normalize=true 表示从 worker 读回（反向）；false 表示下发（正向）。
pub fn normalize_brightness(
    brightness: f64,
    normalize: bool,
    min: f64,
    max: f64,
    calibration_points: &[(f64, f64)],
) -> f64 {
    let mut points: Vec<(f64, f64)> = calibration_points.to_vec();
    if min > 0.0 {
        points.push((0.0, min));
    }
    if max < 100.0 {
        points.push((100.0, max));
    }
    get_calibrated_value(brightness, &points, normalize)
}

/// 从 monitor 取 calibration 点（[{input, output}]）。
pub fn calibration_of(monitor: &Value) -> Vec<(f64, f64)> {
    monitor
        .get("calibration")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    Some((
                        p.get("input")?.as_f64()?,
                        p.get("output")?.as_f64()?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// usesExtendedMinimum（equiv electron.js:L1500-L1506）。
pub fn uses_extended_minimum(monitor: &Value, settings: &Value) -> bool {
    let key = monitor.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let opted = settings
        .get("extendMinimumDisplays")
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !opted {
        return false;
    }
    // canUseGammaRamp：HDR 激活时不可用
    if monitor.get("hdr").and_then(|v| v.as_str()) == Some("active") {
        return false;
    }
    if uses_gamma_slider(monitor, settings) {
        return false;
    }
    if !monitor
        .get("gammaBrightness")
        .and_then(|v| v.as_f64())
        .map(|v| v >= 0.0)
        .unwrap_or(false)
    {
        return false;
    }
    matches!(
        monitor.get("type").and_then(|v| v.as_str()),
        Some("ddcci") | Some("wmi") | Some("studio-display")
    )
}

/// EXTENDED_MINIMUM_BREAKPOINT_*（equiv electron.js:L116-L118，逐字保留）。
pub const EXTENDED_MINIMUM_BREAKPOINT_DEFAULT: i64 = 20;
pub const EXTENDED_MINIMUM_BREAKPOINT_MIN: i64 = 5;
pub const EXTENDED_MINIMUM_BREAKPOINT_MAX: i64 = 90;
/// GAMMA_BRIGHTNESS_MIN（equiv electron.js 常量，逐字保留）。
pub const GAMMA_BRIGHTNESS_MIN: f64 = 20.0;

/// getExtendedMinimumBreakpoint（equiv electron.js:L1509-L1513）。
pub fn get_extended_minimum_breakpoint(monitor: &Value, settings: &Value) -> i64 {
    let key = monitor.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let breakpoint = settings
        .get("extendMinimumBreakpoints")
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if breakpoint <= 0 || breakpoint > EXTENDED_MINIMUM_BREAKPOINT_MAX {
        EXTENDED_MINIMUM_BREAKPOINT_DEFAULT
    } else {
        breakpoint
    }
}

/// sliderLevelToGammaLevel（equiv electron.js:L1483-L1485）。
pub fn slider_level_to_gamma_level(level: f64) -> f64 {
    let v = GAMMA_BRIGHTNESS_MIN + (level * (100.0 - GAMMA_BRIGHTNESS_MIN) / 100.0);
    v.clamp(GAMMA_BRIGHTNESS_MIN, 100.0).round()
}

/// 取 monitor.features 里某个 VCP 的值（数组形式 [当前, 最大] 或原始值）。
fn feature_array(monitor: &Value, vcp: &str) -> Option<Vec<Value>> {
    monitor
        .get("features")
        .and_then(|f| f.get(vcp))
        .and_then(|v| v.as_array())
        .cloned()
}

/// hasEnabledLinkedFeaturesForMonitor（equiv electron.js:L3107-L3113）。
pub fn has_enabled_linked_features(monitor: &Value, settings: &Value) -> bool {
    let hwid1 = monitor
        .get("hwid")
        .and_then(|v| v.as_array())
        .and_then(|a| a.get(1))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if hwid1.is_empty() {
        return false;
    }
    let feature_settings = settings
        .get("monitorFeaturesSettings")
        .and_then(|v| v.get(hwid1))
        .and_then(|v| v.as_object());
    let enabled = settings
        .get("monitorFeatures")
        .and_then(|v| v.get(hwid1))
        .and_then(|v| v.as_object());
    let (Some(fs), Some(en)) = (feature_settings, enabled) else {
        return false;
    };
    fs.iter().any(|(vcp, s)| {
        s.get("linked").and_then(|v| v.as_bool()).unwrap_or(false)
            && en.get(vcp).map(|v| v.is_truthy()).unwrap_or(false)
    })
}

trait Truthy {
    fn is_truthy(&self) -> bool;
}
impl Truthy for Value {
    /// JS 真值语义：false / 0 / "" / null / undefined 为假
    fn is_truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_f64().map(|v| v != 0.0).unwrap_or(false),
            Value::String(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Object(_) => true,
        }
    }
}

/// isHDROnlySDR（equiv BrightnessPanel.jsx:L321）。
fn is_hdr_only_sdr(monitor: &Value, settings: &Value) -> bool {
    let hdr = monitor.get("hdr").and_then(|v| v.as_str()).unwrap_or("");
    (hdr == "active" || hdr == "supported")
        && monitor.get("type").and_then(|v| v.as_str()) == Some("none")
        && !uses_gamma_slider(monitor, settings)
}

/// useFeatures 判定（equiv BrightnessPanel.jsx:L258-L277）：
/// 任一显示器有非 brightness/power 的特征被启用且未 linked，则全列表用扩展布局。
pub fn use_features(sorted: &[Value], settings: &Value) -> bool {
    for monitor in sorted {
        let hwid1 = monitor
            .get("hwid")
            .and_then(|v| v.as_array())
            .and_then(|a| a.get(1))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let Some(features) = settings
            .get("monitorFeatures")
            .and_then(|v| v.get(hwid1))
            .and_then(|v| v.as_object())
        else {
            continue;
        };
        for (vcp, feature) in features {
            if vcp == "0x10" || vcp == "0x13" || vcp == "0xD6" {
                continue; // 跳过亮度与电源状态
            }
            if feature.is_truthy() {
                let linked = settings
                    .get("monitorFeaturesSettings")
                    .and_then(|v| v.get(hwid1))
                    .and_then(|v| v.get(vcp))
                    .and_then(|v| v.get("linked"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !linked {
                    return true;
                }
            }
        }
    }
    false
}

/// 特征行构建（equiv DDCCISliders.jsx:L51-L113 + HDRSliders.jsx）。
/// 返回 (features, has_features)。
fn build_features(
    monitor: &Value,
    settings: &Value,
    show_sdr: bool,
) -> (Vec<PanelFeature>, bool) {
    let mut out = Vec::new();
    let mut has_features = false;

    let hwid1 = monitor
        .get("hwid")
        .and_then(|v| v.as_array())
        .and_then(|a| a.get(1))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let monitor_features = settings.get("monitorFeatures").and_then(|v| v.get(hwid1));
    let feature_settings = settings
        .get("monitorFeaturesSettings")
        .and_then(|v| v.get(hwid1));

    if let Some(features) = monitor.get("features").and_then(|v| v.as_object()) {
        for (vcp, feature) in features {
            if vcp == "0x10" || vcp == "0x13" || vcp == "0xD6" {
                continue; // 跳过亮度与电源状态
            }
            let enabled = monitor_features
                .and_then(|v| v.get(vcp))
                .map(|v| v.is_truthy())
                .unwrap_or(false);
            let linked = feature_settings
                .and_then(|v| v.get(vcp))
                .and_then(|v| v.get("linked"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !enabled || linked {
                continue;
            }
            let Some(arr) = feature.as_array() else { continue };
            let current = arr.first().and_then(|v| v.as_f64()).unwrap_or(0.0);
            let max = arr.get(1).and_then(|v| v.as_f64()).unwrap_or(100.0);
            let per_feature_settings = feature_settings
                .and_then(|v| v.get(vcp))
                .cloned()
                .unwrap_or(json!({}));
            let code = parse_vcp_code(vcp);

            let kind = match vcp.as_str() {
                "0x12" => FeatureKind::Contrast,
                "0x62" => FeatureKind::Volume,
                "0x60" => FeatureKind::Inputs,
                _ => FeatureKind::Custom,
            };

            // 特征显示值：0x62 走音量反算，0x60 用当前输入源，其余用原始值
            let level = if vcp == "0x62" {
                unscale_volume(current, feature, &per_feature_settings)
            } else {
                current
            };

            // 图标（equiv DDCCISliders.jsx:L73/L83/L92 与 custom 的默认 E897）
            let icon_type = per_feature_settings
                .get("iconType")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let default_icon = match vcp.as_str() {
                "0x12" => "\u{e793}", // Contrast
                "0x62" => "\u{e767}", // Volume
                "0x60" => "\u{e839}", // Inputs
                _ => "\u{e897}",      // Custom 默认
            };
            let (icon, icon_text) = if icon_type == "windows" {
                let code_str = per_feature_settings
                    .get("icon")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                (
                    if code_str.is_empty() {
                        String::new()
                    } else {
                        char::from_u32(u32::from_str_radix(code_str, 16).unwrap_or(0xe897))
                            .map(|c| c.to_string())
                            .unwrap_or_default()
                    },
                    String::new(),
                )
            } else if icon_type == "text" {
                (
                    String::new(),
                    per_feature_settings
                        .get("iconText")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
            } else {
                (default_icon.to_string(), String::new())
            };

            // 输入源按钮（equiv inputsData）
            let inputs: Vec<PanelInput> = if vcp == "0x60" {
                arr.get(1)
                    .and_then(|v| v.as_array())
                    .map(|list| {
                        list.iter()
                            .filter_map(|v| v.as_i64())
                            .filter_map(|v| {
                                INPUTS_DATA
                                    .iter()
                                    .find(|(code, _)| *code == v as i32)
                                    .map(|(code, label)| PanelInput {
                                        value: *code,
                                        label: label.to_string().into(),
                                    })
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };

            has_features = true;
            out.push(PanelFeature {
                code,
                kind,
                level: level as f32,
                max: max as i32,
                icon: icon.into(),
                icon_text: icon_text.into(),
                inputs: ModelRc::new(VecModel::from(inputs)),
                current_input: current as i32,
            });
        }
    }

    // HDR/SDR 行（equiv HDRSliders.jsx）
    if show_sdr {
        let sdr = monitor.get("sdrLevel").and_then(|v| v.as_f64()).unwrap_or(0.0);
        has_features = true;
        out.push(PanelFeature {
            code: 0,
            kind: FeatureKind::Sdr,
            level: sdr as f32,
            max: 100,
            icon: "\u{e706}".into(),
            icon_text: "SDR".into(),
            inputs: ModelRc::new(VecModel::from(Vec::new())),
            current_input: 0,
        });
    }

    (out, has_features)
}

/// "0x62" -> 0x62
pub fn parse_vcp_code(vcp: &str) -> i32 {
    if let Some(hex) = vcp.strip_prefix("0x").or_else(|| vcp.strip_prefix("0X")) {
        i32::from_str_radix(hex, 16).unwrap_or(0)
    } else {
        vcp.parse::<i32>().unwrap_or(0)
    }
}

/// 构建单台显示器的 UI 模型。
pub fn build_panel_monitor(
    monitor: &Value,
    settings: &Value,
    use_features: bool,
) -> PanelMonitor {
    let key = monitor.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let id = monitor.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let type_str = monitor.get("type").and_then(|v| v.as_str()).unwrap_or("none");
    let display_type = match type_str {
        "wmi" => DisplayType::Wmi,
        "ddcci" => DisplayType::Ddcci,
        "studio-display" => DisplayType::Studio,
        "software" => DisplayType::Software,
        _ => DisplayType::None,
    };

    let hwid1 = monitor
        .get("hwid")
        .and_then(|v| v.as_array())
        .and_then(|a| a.get(1))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // showPowerButton（equiv BrightnessPanel.jsx:L313-L318）
    let power_feature_enabled = settings
        .get("monitorFeatures")
        .and_then(|v| v.get(hwid1))
        .and_then(|v| v.get("0xD6"))
        .map(|v| v.is_truthy())
        .unwrap_or(false);
    let custom_power_enabled = settings
        .get("monitorFeaturesSettings")
        .and_then(|v| v.get(hwid1))
        .and_then(|v| v.get("0xD6"))
        .map(|v| v.is_truthy())
        .unwrap_or(false);
    let power_feature = feature_array(monitor, "0xD6");
    let show_power = power_feature_enabled && (power_feature.is_some() || custom_power_enabled);
    let power_value = power_feature
        .as_ref()
        .and_then(|a| a.first())
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let power_on = power_value >= 4.0;

    let hdr_only_sdr = is_hdr_only_sdr(monitor, settings);
    // showHDRSliders（equiv L302-L308）
    let hdr_sliders = {
        let hdr_active = monitor.get("hdr").and_then(|v| v.as_str()) == Some("active");
        let opted = settings
            .get("hdrDisplays")
            .and_then(|v| v.get(key))
            .map(|v| v.is_truthy())
            .unwrap_or(false);
        let sdr_main = settings
            .get("sdrAsMainSliderDisplays")
            .and_then(|v| v.get(key))
            .map(|v| v.is_truthy())
            .unwrap_or(false);
        (hdr_active || opted) && !sdr_main
    };

    let adjustable = is_adjustable_display(monitor, settings);
    let (features, has_features) = build_features(monitor, settings, hdr_sliders);
    let extended = use_features && (has_features || hdr_sliders);

    PanelMonitor {
        key: key.into(),
        id: id.into(),
        name: panel_monitor_name(monitor, settings.get("names").unwrap_or(&json!({}))).into(),
        display_type,
        brightness: monitor
            .get("brightness")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32,
        minimum: monitor.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
        maximum: monitor.get("max").and_then(|v| v.as_f64()).unwrap_or(100.0) as f32,
        num: monitor.get("num").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        adjustable,
        show_power,
        power_on,
        hdr_only_sdr,
        extended,
        features: ModelRc::new(VecModel::from(features)),
    }
}

/// 排序（equiv BrightnessPanel.jsx:L253-L257）：order 缺省 999，升序。
pub fn sorted_monitors(monitors: &Value) -> Vec<Value> {
    let mut list: Vec<Value> = monitors
        .as_object()
        .map(|o| o.values().cloned().collect())
        .unwrap_or_default();
    list.sort_by(|a, b| {
        let ao = a.get("order").and_then(|v| v.as_f64()).unwrap_or(999.0);
        let bo = b.get("order").and_then(|v| v.as_f64()).unwrap_or(999.0);
        ao.partial_cmp(&bo).unwrap_or(std::cmp::Ordering::Equal)
    });
    list
}

/// 联动模式下选中的代表显示器（equiv BrightnessPanel.jsx:L237-L243）：
/// 最后一个满足"可调"条件的显示器。
pub fn linked_representative<'a>(monitors: &'a [Value], settings: &Value) -> Option<&'a Value> {
    let mut last = None;
    for monitor in monitors {
        let type_str = monitor.get("type").and_then(|v| v.as_str()).unwrap_or("none");
        let hdr_active = monitor.get("hdr").and_then(|v| v.as_str()) == Some("active");
        let brightness_type_ok = monitor
            .get("brightnessType")
            .map(|v| v.is_truthy())
            .unwrap_or(false);
        if type_str == "wmi"
            || type_str == "studio-display"
            || type_str == "software"
            || (type_str == "ddcci" && brightness_type_ok)
            || hdr_active
            || uses_gamma_slider(monitor, settings)
        {
            last = Some(monitor);
        }
    }
    last
}

/// 构建面板完整模型：(monitors, visible_count)。
pub fn build_panel_model(
    monitors: &Value,
    settings: &Value,
    linked_levels: bool,
) -> (Vec<PanelMonitor>, i32) {
    let sorted = sorted_monitors(monitors);
    let visible: Vec<&Value> = sorted
        .iter()
        .filter(|m| is_visible_monitor(m, settings))
        .collect();
    let visible_count = visible.len() as i32;
    let use_features = use_features(&sorted, settings);

    if linked_levels {
        // 联动模式：只渲染代表显示器一条滑块
        if let Some(rep) = linked_representative(&sorted, settings) {
            let mut m = build_panel_monitor(rep, settings, use_features);
            m.adjustable = true;
            m.extended = false;
            m.hdr_only_sdr = false;
            return (vec![m], visible_count);
        }
        return (Vec::new(), visible_count);
    }

    let list = sorted
        .iter()
        .filter(|m| {
            let hdr_active = m.get("hdr").and_then(|v| v.as_str()) == Some("active");
            (is_adjustable_display(m, settings) || hdr_active) && is_visible_monitor(m, settings)
        })
        .map(|m| build_panel_monitor(m, settings, use_features))
        .collect();
    (list, visible_count)
}

/// 用于单元测试的 Rc 便捷构造。
pub fn rc_model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

/// Rc 类型别名（Slint 模型非 Send，UI 层只在主线程使用）。
pub type UiModel<T> = Rc<VecModel<T>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vcp_code_parsing() {
        assert_eq!(parse_vcp_code("0x62"), 0x62);
        assert_eq!(parse_vcp_code("0x12"), 0x12);
        assert_eq!(parse_vcp_code("18"), 18);
    }

    #[test]
    fn volume_scaling_matches_js() {
        // 报 65535 但当前值 <= 100 -> 最大值按 100 算（volume-utils.js:L10-L12 的实测特例）
        assert_eq!(scale_volume(50.0, &json!([0, 65535]), &json!({})), 50.0);
        // 当前值 > 100 时按真实最大值换算：50% * 65535 / 100 = 32768
        let feature = json!([200, 65535]);
        assert_eq!(scale_volume(50.0, &feature, &json!({})), 32768.0);
        // 往返一致
        assert!((unscale_volume(32768.0, &feature, &json!({})) - 50.0).abs() < 0.01);
        // maxRawValue 覆盖（equiv settings.maxRawValue）
        assert_eq!(
            scale_volume(50.0, &json!([0, 65535]), &json!({"maxRawValue": 200})),
            100.0
        );
    }

    #[test]
    fn gamma_slider_requires_opt_in_and_non_hdr() {
        let mut settings = json!({});
        let monitor = json!({"key": "k1", "gammaBrightness": 80, "hdr": "inactive"});
        assert!(!uses_gamma_slider(&monitor, &settings));
        settings["gammaAsMainSliderDisplays"] = json!({"k1": true});
        assert!(uses_gamma_slider(&monitor, &settings));
        let hdr = json!({"key": "k1", "gammaBrightness": 80, "hdr": "active"});
        assert!(!uses_gamma_slider(&hdr, &settings));
        let no_gamma = json!({"key": "k1", "gammaBrightness": -1, "hdr": "inactive"});
        assert!(!uses_gamma_slider(&no_gamma, &settings));
    }

    #[test]
    fn adjustable_display_none_type_with_gamma() {
        let settings = json!({"gammaAsMainSliderDisplays": {"k1": true}});
        let monitor = json!({"key": "k1", "type": "none", "gammaBrightness": 90});
        assert!(is_adjustable_display(&monitor, &settings));
        let monitor2 = json!({"key": "k2", "type": "none", "gammaBrightness": 90});
        assert!(!is_adjustable_display(&monitor2, &settings));
    }

    #[test]
    fn monitor_sort_defaults_to_999() {
        let monitors = json!({
            "a": {"key": "a", "order": 2},
            "b": {"key": "b"},
            "c": {"key": "c", "order": 0}
        });
        let sorted = sorted_monitors(&monitors);
        let keys: Vec<&str> = sorted
            .iter()
            .map(|m| m.get("key").and_then(|v| v.as_str()).unwrap_or(""))
            .collect();
        assert_eq!(keys, vec!["c", "a", "b"]);
    }

    #[test]
    fn panel_name_prefers_rename_without_suffix() {
        let monitor = json!({"id": "\\\\.\\DISPLAY1", "name": "CMN1521"});
        let renames = json!({"\\\\.\\DISPLAY1": "Laptop"});
        assert_eq!(panel_monitor_name(&monitor, &renames), "Laptop");
        assert_eq!(settings_monitor_name(&monitor, &renames), "Laptop (CMN1521)");
        assert_eq!(panel_monitor_name(&monitor, &json!({})), "CMN1521");
    }
}