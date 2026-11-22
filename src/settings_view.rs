//! 设置窗口数据构建层：settings.json -> Slint SettingsItem/SettingsPage/SettingsMonitorEntry。
//!
//! equiv: origin/src/components/SettingsWindow.jsx 的 getSidebar（7 页）+ 各页
//! SettingsOption 列表（title/description/input 三元组）+ SettingsOption.jsx 渲染语义。
//!
//! 所有文案走 tt-localize（en.ftl 回退链），键名与源 JSX 一一对应，便于对拍。

use crate::ui_types::*;
use crate::monitor_view;
use serde_json::{json, Value};
use slint::{ModelRc, VecModel};

/// 侧边栏 7 页（equiv SettingsWindow.jsx getSidebar L361-L400，顺序与图标照抄）。
pub fn build_pages() -> Vec<SettingsPage> {
    vec![
        page("general", "SETTINGS_SIDEBAR_GENERAL", "\u{e713}"),
        page("monitors", "SETTINGS_SIDEBAR_MONITORS", "\u{e7f4}"),
        page("features", "SETTINGS_SIDEBAR_FEATURES", "\u{e9e9}"),
        page("time", "SETTINGS_SIDEBAR_TIME", "\u{e823}"),
        page("hotkeys", "SETTINGS_SIDEBAR_HOTKEYS", "\u{e765}"),
        page("light-sensor", "SETTINGS_SIDEBAR_LIGHT_SENSOR", "\u{e706}"),
        page("updates", "SETTINGS_SIDEBAR_UPDATES", "\u{e895}"),
    ]
}

fn page(id: &str, label_key: &str, glyph: &str) -> SettingsPage {
    SettingsPage {
        id: id.into(),
        label: tt_localize::t(label_key).into(),
        glyph: glyph.into(),
    }
}

/// 设置项构造助手。
struct ItemBuilder {
    items: Vec<SettingsItem>,
}

impl ItemBuilder {
    fn new() -> Self {
        Self { items: Vec::new() }
    }

    fn section(&mut self, title: &str) -> &mut Self {
        self.items.push(SettingsItem {
            kind: SettingsItemKind::Section,
            title: title.into(),
            ..Default::default()
        });
        self
    }

    fn note(&mut self, text: &str) -> &mut Self {
        self.items.push(SettingsItem {
            kind: SettingsItemKind::Note,
            title: text.into(),
            ..Default::default()
        });
        self
    }

    fn toggle(&mut self, key: &str, title_key: &str, desc_key: &str, settings: &Value) -> &mut Self {
        self.items.push(SettingsItem {
            kind: SettingsItemKind::Toggle,
            key: key.into(),
            title: tt_localize::t(title_key).into(),
            description: if desc_key.is_empty() {
                Default::default()
            } else {
                tt_localize::t(desc_key).into()
            },
            toggle_value: settings
                .get(key)
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            ..Default::default()
        });
        self
    }

    /// 下拉（equiv <select>）：options 为 (value, label) 列表。
    fn select(
        &mut self,
        key: &str,
        title_key: &str,
        desc_key: &str,
        settings: &Value,
        options: &[(&str, String)],
    ) -> &mut Self {
        let current = settings
            .get(key)
            .map(json_to_string)
            .unwrap_or_default();
        let (selected_label, next_value) = compute_selection(&current, options);
        self.items.push(SettingsItem {
            kind: SettingsItemKind::Select,
            key: key.into(),
            title: tt_localize::t(title_key).into(),
            description: if desc_key.is_empty() {
                Default::default()
            } else {
                tt_localize::t(desc_key).into()
            },
            text_value: current.into(),
            selected_label: selected_label.into(),
            next_value: next_value.into(),
            options: ModelRc::new(VecModel::from(
                options
                    .iter()
                    .map(|(v, l)| SettingsChoice {
                        value: (*v).into(),
                        label: l.as_str().into(),
                    })
                    .collect::<Vec<_>>(),
            )),
            ..Default::default()
        });
        self
    }

    /// 数值输入（equiv <input type="number">）。
    fn number(&mut self, key: &str, title_key: &str, desc_key: &str, settings: &Value) -> &mut Self {
        self.items.push(SettingsItem {
            kind: SettingsItemKind::Number,
            key: key.into(),
            title: tt_localize::t(title_key).into(),
            description: if desc_key.is_empty() {
                Default::default()
            } else {
                tt_localize::t(desc_key).into()
            },
            text_value: settings
                .get(key)
                .map(json_to_string)
                .unwrap_or_default()
                .into(),
            ..Default::default()
        });
        self
    }

    /// 按钮（equiv <a className="button">）。
    fn button(&mut self, key: &str, title_key: &str, desc_key: &str, label_key: &str) -> &mut Self {
        self.items.push(SettingsItem {
            kind: SettingsItemKind::Button,
            key: key.into(),
            title: tt_localize::t(title_key).into(),
            description: if desc_key.is_empty() {
                Default::default()
            } else {
                tt_localize::t(desc_key).into()
            },
            button_label: tt_localize::t(label_key).into(),
            ..Default::default()
        });
        self
    }

    fn finish(self) -> Vec<SettingsItem> {
        self.items
    }
}

fn json_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

/// 下拉轮换：(当前显示文本, 下一个值)。等价 <select> 的"点一次换下一个"语义。
fn compute_selection(current: &str, options: &[(&str, String)]) -> (String, String) {
    if options.is_empty() {
        return (current.to_string(), String::new());
    }
    let idx = options
        .iter()
        .position(|(v, _)| *v == current)
        .unwrap_or(0);
    let label = options[idx].1.clone();
    let next = options[(idx + 1) % options.len()].0;
    (label, next.to_string())
}

/// 构建某一页的设置项列表。
pub fn build_items(page_id: &str, settings: &Value) -> Vec<SettingsItem> {
    let mut b = ItemBuilder::new();
    match page_id {
        "general" => general_page(&mut b, settings),
        "monitors" => monitors_page(&mut b, settings),
        "features" => features_page(&mut b, settings),
        "time" => time_page(&mut b, settings),
        "hotkeys" => hotkeys_page(&mut b, settings),
        "light-sensor" => light_sensor_page(&mut b, settings),
        "updates" => updates_page(&mut b, settings),
        _ => {}
    }
    b.finish()
}

/// 通用页（equiv SettingsWindow.jsx L1353-L1461）。
fn general_page(b: &mut ItemBuilder, s: &Value) {
    b.section(&tt_localize::t("SETTINGS_GENERAL_TITLE"));
    b.toggle("openAtLogin", "SETTINGS_GENERAL_STARTUP", "", s);
    b.toggle(
        "brightnessAtStartup",
        "SETTINGS_GENERAL_BRIGHTNESS_STARTUP_TITLE",
        "SETTINGS_GENERAL_BRIGHTNESS_STARTUP_DESC",
        s,
    );
    b.toggle(
        "disableOnLockScreen",
        "SETTINGS_GENERAL_DISABLE_ON_LOCK_SCREEN_TITLE",
        "SETTINGS_GENERAL_DISABLE_ON_LOCK_SCREEN_DESC",
        s,
    );
    b.select(
        "language",
        "SETTINGS_GENERAL_LANGUAGE_TITLE",
        "",
        s,
        &[
            ("system", tt_localize::t("SETTINGS_GENERAL_LANGUAGE_SYSTEM")),
            ("en", "English".to_string()),
            ("zh-Hans", "简体中文".to_string()),
            ("zh-Hant", "繁體中文".to_string()),
            ("ja", "日本語".to_string()),
            ("de", "Deutsch".to_string()),
            ("fr", "Français".to_string()),
            ("es", "Español".to_string()),
            ("ru", "Русский".to_string()),
        ],
    );
    b.select(
        "theme",
        "SETTINGS_GENERAL_THEME_TITLE",
        "",
        s,
        &[
            ("default", tt_localize::t("SETTINGS_GENERAL_THEME_SYSTEM")),
            ("dark", tt_localize::t("SETTINGS_GENERAL_THEME_DARK")),
            ("light", tt_localize::t("SETTINGS_GENERAL_THEME_LIGHT")),
        ],
    );
    b.select(
        "windowsStyle",
        "SETTINGS_GENERAL_WINDOWS_UI_STYLE_TITLE",
        "",
        s,
        &[
            ("system", tt_localize::t("SETTINGS_GENERAL_THEME_SYSTEM")),
            ("win11", "Windows 11".to_string()),
            ("win10", "Windows 10".to_string()),
        ],
    );
    b.toggle(
        "useAcrylic",
        "SETTINGS_GENERAL_ACRYLIC_TITLE",
        "SETTINGS_GENERAL_ACRYLIC_DESC",
        s,
    );
    b.select(
        "icon",
        "SETTINGS_GENERAL_TRAY_ICON_TITLE",
        "",
        s,
        &[
            ("icon", "Icon".to_string()),
            ("mdl2", "MDL2".to_string()),
            ("fluent", "Fluent".to_string()),
        ],
    );
    b.toggle(
        "analytics",
        "SETTINGS_GENERAL_ANALYTICS_TITLE",
        "SETTINGS_GENERAL_ANALYTICS_DESC",
        s,
    );

    // 故障排查（equiv L1414-L1459）
    b.section(&tt_localize::t("SETTINGS_GENERAL_TROUBLESHOOTING"));
    b.toggle("disableAppleStudio", "Apple Studio Displays", "", s);
    b.toggle("disableHighLevel", "DDC/CI HL", "", s);
    b.toggle("disableHDR", "HDR", "", s);
    b.toggle("disableWMIC", "WMIC", "", s);
    b.toggle("disableWMI", "WMI-Bridge", "", s);
    b.toggle("disableWin32", "Win32-DisplayConfig", "", s);
    b.select(
        "defaultOverlayType",
        "SETTINGS_GENERAL_OVERLAY_TITLE",
        "SETTINGS_GENERAL_OVERLAY_DESC",
        s,
        &[
            ("disabled", tt_localize::t("SETTINGS_GENERAL_DIS_OVERLAY_TITLE")),
            ("safe", tt_localize::t("SETTINGS_GENERAL_ON_OVERLAY_TITLE")),
            ("aggressive", tt_localize::t("SETTINGS_GENERAL_FORCE_OVERLAY_TITLE")),
        ],
    );
    b.toggle(
        "disableAutoApply",
        "SETTINGS_GENERAL_AUTOBRIGHT_TITLE",
        "SETTINGS_GENERAL_AUTOBRIGHT_DESC",
        s,
    );
    b.toggle(
        "disableThemeChanges",
        "SETTINGS_GENERAL_SKIP_THEME_CHANGES_TITLE",
        "SETTINGS_GENERAL_SKIP_THEME_CHANGES_DESC",
        s,
    );
    b.toggle(
        "disablePowerNotifications",
        "SETTINGS_GENERAL_SKIP_POWER_EVENTS_TITLE",
        "SETTINGS_GENERAL_SKIP_POWER_EVENTS_DESC",
        s,
    );
    b.button(
        "save-report",
        "SETTINGS_GENERAL_REPORT_TITLE",
        "SETTINGS_GENERAL_REPORT_DESC",
        "SETTINGS_GENERAL_REPORT_TITLE",
    );
    b.button(
        "reset-settings",
        "SETTINGS_GENERAL_RESET_TITLE",
        "SETTINGS_GENERAL_RESET_DESC",
        "SETTINGS_GENERAL_RESET_BUTTON",
    );
}

/// 显示器页（equiv L1535-L1578；逐台控制在 monitors 列表里，这里是全局项）。
fn monitors_page(b: &mut ItemBuilder, s: &Value) {
    b.section(&tt_localize::t("SETTINGS_SIDEBAR_MONITORS"));
    b.select(
        "updateInterval",
        "SETTINGS_MONITORS_RATE_TITLE",
        "SETTINGS_MONITORS_RATE_DESC",
        s,
        &[
            ("100", tt_localize::t("SETTINGS_MONITORS_RATE_0")),
            ("250", tt_localize::t("SETTINGS_MONITORS_RATE_1")),
            ("500", tt_localize::t("SETTINGS_MONITORS_RATE_2")),
            ("1000", tt_localize::t("SETTINGS_MONITORS_RATE_3")),
            ("2000", tt_localize::t("SETTINGS_MONITORS_RATE_4")),
        ],
    );
    b.toggle(
        "hideClosedLid",
        "SETTINGS_MONITORS_HIDE_INTERNAL_TITLE",
        "SETTINGS_MONITORS_HIDE_INTERNAL_DESC",
        s,
    );
    b.select(
        "sleepAction",
        "SETTINGS_MONITORS_SLEEP_ACTION_TITLE",
        "SETTINGS_MONITORS_SLEEP_ACTION_DESC",
        s,
        &[
            ("none", "None".to_string()),
            ("ps", "PowerShell".to_string()),
            ("ddcci", "DDC/CI".to_string()),
            ("ps_ddcci", "PowerShell + DDC/CI".to_string()),
        ],
    );
    b.number(
        "ddcPowerOffValue",
        "SETTINGS_FEATURES_POWER_TITLE",
        "SETTINGS_FEATURES_POWER_DESC",
        s,
    );
    b.section(&tt_localize::t("SETTINGS_MONITORS_NORMALIZE_TITLE"));
    b.note(&tt_localize::t("SETTINGS_MONITORS_NORMALIZE_DESC"));
    b.note(&tt_localize::t("SETTINGS_MONITORS_CALIBRATION_DESC"));
}

/// DDC/CI 特征页（equiv L1584-L1608）。
fn features_page(b: &mut ItemBuilder, s: &Value) {
    b.section(&tt_localize::t("SETTINGS_SIDEBAR_FEATURES"));
    b.note(&tt_localize::t("SETTINGS_FEATURES_DESCRIPTION"));
    b.toggle(
        "getDDCBrightnessUpdates",
        "SETTINGS_FEATURES_CUR_BRIGHTNESS_TITLE",
        "SETTINGS_FEATURES_CUR_BRIGHTNESS_DESC",
        s,
    );
    b.select(
        "ddcPowerOffValue",
        "SETTINGS_FEATURES_POWER_TITLE",
        "SETTINGS_FEATURES_POWER_DESC",
        s,
        &[
            ("4", tt_localize::t("SETTINGS_FEATURES_POWER_STANDBY")),
            ("5", tt_localize::t("SETTINGS_FEATURES_POWER_OFF")),
            ("6", tt_localize::t("SETTINGS_FEATURES_POWER_COMPAT")),
        ],
    );
    b.note(&tt_localize::t("SETTINGS_FEATURES_POWER_WARNING"));
    b.note(&tt_localize::t("SETTINGS_FEATURES_DDC_BLOCK_DESC"));
}

/// 时间调整页（equiv L1466-L1530）。
fn time_page(b: &mut ItemBuilder, s: &Value) {
    b.section(&tt_localize::t("SETTINGS_TIME_TITLE"));
    b.note(&tt_localize::t("SETTINGS_TIME_DESC"));
    b.number("adjustmentTimeLatitude", "SETTINGS_TIME_LAT", "", s);
    b.number("adjustmentTimeLongitude", "SETTINGS_TIME_LONG", "", s);
    b.button(
        "get-coordinates",
        "SETTINGS_TIME_SUN_GET",
        "",
        "SETTINGS_TIME_SUN_GET",
    );
    b.toggle(
        "adjustmentTimeIndividualDisplays",
        "SETTINGS_TIME_INDIVIDUAL_TITLE",
        "SETTINGS_TIME_INDIVIDUAL_DESC",
        s,
    );
    b.toggle(
        "adjustmentTimeAnimate",
        "SETTINGS_TIME_ANIMATE_TITLE",
        "SETTINGS_TIME_ANIMATE_DESC",
        s,
    );
    b.select(
        "adjustmentTimeSpeed",
        "SETTINGS_TIME_TRANSITON_TITLE",
        "SETTINGS_TIME_TRANSITON_DESC",
        s,
        &[
            ("slowest", "Slowest".to_string()),
            ("slow", "Slow".to_string()),
            ("normal", "Normal".to_string()),
            ("fast", "Fast".to_string()),
            ("fastest", "Fastest".to_string()),
        ],
    );
    b.toggle(
        "checkTimeAtStartup",
        "SETTINGS_TIME_STARTUP_TITLE",
        "SETTINGS_TIME_STARTUP_DESC",
        s,
    );
    b.section(&tt_localize::t("SETTINGS_TIME_IDLE_TITLE"));
    b.toggle(
        "detectIdleTimeEnabled",
        "SETTINGS_TIME_IDLE_TITLE",
        "SETTINGS_TIME_IDLE_DESC",
        s,
    );
    b.toggle(
        "detectIdleCheckFullscreen",
        "SETTINGS_TIME_IDLE_FS_TITLE",
        "SETTINGS_TIME_IDLE_FS_DESC",
        s,
    );
    b.toggle(
        "detectIdleMedia",
        "SETTINGS_TIME_IDLE_MEDIA_TITLE",
        "SETTINGS_TIME_IDLE_MEDIA_DESC",
        s,
    );
}

/// 热键页（equiv L1610-L1660；热键列表单独渲染）。
fn hotkeys_page(b: &mut ItemBuilder, s: &Value) {
    b.section(&tt_localize::t("SETTINGS_HOTKEYS_TITLE"));
    b.note(&tt_localize::t("SETTINGS_HOTKEYS_DESC"));
    b.note(&tt_localize::t("SETTINGS_HOTKEYS_NATIVE_BRIGHTNESS_WARN"));
    b.note(&tt_localize::t("SETTINGS_HOTKEYS_NATIVE_BRIGHTNESS_CHECK"));
    b.toggle(
        "hotkeysBreakLinkedLevels",
        "SETTINGS_HOTKEYS_BREAK_TITLE",
        "SETTINGS_HOTKEYS_BREAK_DESC",
        s,
    );
    b.toggle(
        "scrollShortcut",
        "SETTINGS_GENERAL_SCROLL_TITLE",
        "SETTINGS_GENERAL_SCROLL_DESC",
        s,
    );
    b.number("scrollShortcutAmount", "SETTINGS_HOTKEYS_SCROLL_AMOUNT_TITLE", "", s);
    b.toggle(
        "invertScroll",
        "SETTINGS_HOTKEYS_INVERT_SCROLL_TITLE",
        "SETTINGS_HOTKEYS_INVERT_SCROLL_DESC",
        s,
    );
    b.number("hotkeyPercent", "SETTINGS_HOTKEYS_PERCENT_TITLE", "", s);
}

/// 光感页（equiv LightSensorSettings.jsx）。
fn light_sensor_page(b: &mut ItemBuilder, s: &Value) {
    let ls = s.get("lightSensor").cloned().unwrap_or(json!({}));
    b.section(&tt_localize::t("SETTINGS_SIDEBAR_LIGHT_SENSOR"));
    b.toggle("enabled", "SETTINGS_LIGHT_SENSOR_ENABLE_TITLE", "SETTINGS_LIGHT_SENSOR_ENABLE_DESC", &ls);
    b.select(
        "active",
        "SETTINGS_LIGHT_SENSOR_SELECT_TITLE",
        "SETTINGS_LIGHT_SENSOR_SELECT_DESC",
        &ls,
        &[
            ("fake", "Fake".to_string()),
            ("windows", "Windows".to_string()),
            ("yocto", "Yocto".to_string()),
        ],
    );
    b.number(
        "sensorPollingInterval",
        "SETTINGS_LIGHT_SENSOR_INTERVAL_TITLE",
        "SETTINGS_LIGHT_SENSOR_INTERVAL_DESC",
        &ls,
    );
}

/// 更新页（equiv L1700-L1760）。
fn updates_page(b: &mut ItemBuilder, s: &Value) {
    b.section(&tt_localize::t("SETTINGS_SIDEBAR_UPDATES"));
    b.toggle(
        "checkForUpdates",
        "SETTINGS_UPDATES_AUTOMATIC_TITLE",
        "SETTINGS_UPDATES_AUTOMATIC_DESC",
        s,
    );
    b.select(
        "branch",
        "SETTINGS_UPDATES_BRANCH_TITLE",
        "",
        s,
        &[
            ("master", "Release".to_string()),
            ("beta", "Beta".to_string()),
        ],
    );
    b.button(
        "check-for-updates",
        "SETTINGS_UPDATES_CHECK_TITLE",
        "",
        "SETTINGS_UPDATES_CHECK_BUTTON",
    );
    b.button(
        "open-settings-file",
        "SETTINGS_ADVANCED_OPEN_SETTINGS_FILE",
        "",
        "SETTINGS_ADVANCED_OPEN_SETTINGS_FILE",
    );
}

/// 显示器逐台条目（equiv monitors 页的重命名/隐藏/gamma/extend minimum/SDR 开关）。
pub fn build_monitor_entries(monitors: &Value, settings: &Value) -> Vec<SettingsMonitorEntry> {
    let names = settings.get("names").cloned().unwrap_or(json!({}));
    let hidden = settings.get("hideDisplays").cloned().unwrap_or(json!({}));
    let gamma = settings
        .get("gammaAsMainSliderDisplays")
        .cloned()
        .unwrap_or(json!({}));
    let sdr = settings
        .get("sdrAsMainSliderDisplays")
        .cloned()
        .unwrap_or(json!({}));
    let extend = settings
        .get("extendMinimumDisplays")
        .cloned()
        .unwrap_or(json!({}));
    let breakpoints = settings
        .get("extendMinimumBreakpoints")
        .cloned()
        .unwrap_or(json!({}));

    monitor_view::sorted_monitors(monitors)
        .into_iter()
        .map(|m| {
            let key = m.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let type_str = m.get("type").and_then(|v| v.as_str()).unwrap_or("none");
            let high_level = m
                .get("highLevelSupported")
                .and_then(|v| v.get("brightness"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // getDebugMonitorType（equiv L877）
            let connection = if type_str == "ddcci" && high_level {
                "DDC/CI HL".to_string()
            } else {
                match type_str {
                    "ddcci" => "DDC/CI".to_string(),
                    "wmi" => "WMI".to_string(),
                    "studio-display" => "Studio Display".to_string(),
                    "software" => "Gamma".to_string(),
                    _ => "None".to_string(),
                }
            };
            let brightness = m
                .get("brightness")
                .and_then(|v| v.as_f64())
                .map(|v| format!("{}%", v.round() as i64))
                .unwrap_or_else(|| tt_localize::t("GENERIC_NOT_SUPPORTED"));
            let maximum = m
                .get("brightnessMax")
                .and_then(|v| v.as_f64())
                .map(|v| v.round().to_string())
                .unwrap_or_else(|| tt_localize::t("GENERIC_NOT_SUPPORTED"));
            let normalization = format!(
                "{} - {}",
                m.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64,
                m.get("max").and_then(|v| v.as_f64()).unwrap_or(100.0) as i64
            );
            let hdr = match m.get("hdr").and_then(|v| v.as_str()) {
                Some("active") => tt_localize::t("GENERIC_ACTIVE"),
                Some("supported") => tt_localize::t("GENERIC_SUPPORTED"),
                _ => tt_localize::t("GENERIC_UNSUPPORTED"),
            };
            SettingsMonitorEntry {
                key: key.clone().into(),
                id: id.clone().into(),
                name: monitor_view::settings_monitor_name(&m, &names).into(),
                rename: names
                    .get(&id)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .into(),
                hidden: hidden.get(&key).and_then(|v| v.as_bool()).unwrap_or(false),
                gamma: gamma.get(&key).and_then(|v| v.as_bool()).unwrap_or(false),
                sdr: sdr.get(&key).and_then(|v| v.as_bool()).unwrap_or(false),
                extend_minimum: extend.get(&key).and_then(|v| v.as_bool()).unwrap_or(false),
                breakpoint: breakpoints
                    .get(&key)
                    .and_then(|v| v.as_i64())
                    .unwrap_or(20) as i32,
                connection: connection.into(),
                hdr: hdr.into(),
                brightness: brightness.into(),
                maximum: maximum.into(),
                normalization: normalization.into(),
            }
        })
        .collect()
}

/// 热键列表（equiv settings.hotkeys 的 v1.16.0 格式）。
pub fn build_hotkeys(settings: &Value) -> Vec<SettingsHotkey> {
    settings
        .get("hotkeys")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .map(|h| {
                    let accelerator = h
                        .get("accelerator")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let summary = h
                        .get("actions")
                        .and_then(|v| v.as_array())
                        .map(|actions| {
                            actions
                                .iter()
                                .map(|a| {
                                    let target = a.get("target").and_then(|v| v.as_str()).unwrap_or("brightness");
                                    let value = a.get("value").and_then(|v| v.as_i64()).unwrap_or(0);
                                    format!("{} {}", target, value)
                                })
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default();
                    SettingsHotkey {
                        id: h
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .into(),
                        accelerator: accelerator.into(),
                        summary: summary.into(),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Slint 模型便捷封装。
pub fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_match_origin_sidebar() {
        let pages = build_pages();
        let ids: Vec<&str> = pages.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["general", "monitors", "features", "time", "hotkeys", "light-sensor", "updates"]
        );
    }

    #[test]
    fn select_cycles_to_next_value() {
        let options = vec![
            ("default", "System".to_string()),
            ("dark", "Dark".to_string()),
            ("light", "Light".to_string()),
        ];
        assert_eq!(compute_selection("dark", &options), ("Dark".to_string(), "light".to_string()));
        // 未知值 -> 从第一个开始
        assert_eq!(compute_selection("zzz", &options), ("System".to_string(), "dark".to_string()));
        // 末尾回环
        assert_eq!(compute_selection("light", &options), ("Light".to_string(), "default".to_string()));
    }

    #[test]
    fn general_page_covers_key_settings() {
        let settings = json!({"openAtLogin": true, "theme": "dark", "useAcrylic": false});
        let items = build_items("general", &settings);
        let keys: Vec<&str> = items
            .iter()
            .filter(|i| !i.key.is_empty())
            .map(|i| i.key.as_str())
            .collect();
        for required in [
            "openAtLogin",
            "brightnessAtStartup",
            "disableOnLockScreen",
            "language",
            "theme",
            "windowsStyle",
            "useAcrylic",
            "icon",
            "analytics",
            "disableAppleStudio",
            "disableHighLevel",
            "disableHDR",
            "disableWMIC",
            "disableWMI",
            "disableWin32",
            "defaultOverlayType",
            "disableAutoApply",
            "disableThemeChanges",
            "disablePowerNotifications",
            "save-report",
            "reset-settings",
        ] {
            assert!(keys.contains(&required), "missing settings item: {}", required);
        }
        // 开关值来自 settings
        let toggle = items.iter().find(|i| i.key == "openAtLogin").unwrap();
        assert!(toggle.toggle_value);
    }

    #[test]
    fn monitor_entry_connection_labels() {
        let monitors = json!({
            "k1": {"key": "k1", "id": "\\\\.\\DISPLAY1", "name": "CMN1521", "type": "ddcci",
                   "highLevelSupported": {"brightness": true}, "brightness": 60, "min": 0, "max": 100}
        });
        let entries = build_monitor_entries(&monitors, &json!({}));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].connection, "DDC/CI HL");
        assert_eq!(entries[0].brightness, "60%");
    }

    #[test]
    fn hotkeys_summary_from_actions() {
        let settings = json!({
            "hotkeys": [{
                "id": "abc",
                "accelerator": "Control+Shift+Up",
                "actions": [{"target": "brightness", "value": 10}]
            }]
        });
        let hotkeys = build_hotkeys(&settings);
        assert_eq!(hotkeys.len(), 1);
        assert_eq!(hotkeys[0].accelerator, "Control+Shift+Up");
        assert_eq!(hotkeys[0].summary, "brightness 10");
    }
}