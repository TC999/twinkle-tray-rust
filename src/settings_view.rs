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
        // debug 页（equiv getSidebar L398-L403：label "Debug"，原版无本地化键，照抄硬编码）
        page("debug", "SETTINGS_SIDEBAR_DEBUG", "\u{ebe8}"),
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

    /// 原版硬编码英文标题的开关（equiv SettingsWindow.jsx L1417-L1422 的
    /// `<SettingsChild title={"Apple Studio Displays"} .../>`）。
    ///
    /// 源语料里没有这些标题的 FTL 键（原版就是字面量），所以**不能**走
    /// `tt_localize::t()`：查不到键会经 en.ftl 回退链返回空串，表现为
    /// 「开关有圆钮、没有文字」。
    fn toggle_literal(&mut self, key: &str, title: &str, settings: &Value) -> &mut Self {
        self.items.push(SettingsItem {
            kind: SettingsItemKind::Toggle,
            key: key.into(),
            title: title.into(),
            toggle_value: settings
                .get(key)
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            ..Default::default()
        });
        self
    }

    /// 下拉（equiv <select>）：options 为 (value, label) 列表。
    /// ComboBox 的 model 只接受字符串，故拆成同序的 values/labels 两个数组；
    /// selected 回调传显示文本，宿主侧按 labels 位置映射回 values。
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
        let values: Vec<&str> = options.iter().map(|(v, _)| *v).collect();
        let selected_index = compute_selection_index(&current, &values);
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
            selected_index: selected_index as i32,
            option_values: ModelRc::new(VecModel::from(
                values.iter().map(|v| (*v).into()).collect::<Vec<_>>(),
            )),
            option_labels: ModelRc::new(VecModel::from(
                options
                    .iter()
                    .map(|(_, l)| l.as_str().into())
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

/// JS 真值语义（equiv `!monitor.brightnessType` 的取反判定）。
fn json_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|v| v != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(_) => true,
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

/// 下拉选中下标：当前值在选项值中的位置；未知值回落第一项（equiv <select>
/// 无匹配 option 时浏览器保持/回落首项的表现）。
fn compute_selection_index(current: &str, values: &[&str]) -> usize {
    values.iter().position(|v| *v == current).unwrap_or(0)
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
        "debug" => debug_page(&mut b, settings),
        _ => {}
    }
    b.finish()
}

/// 构建某一页主列表**之后**的设置项（原版部分页面的 pageSection 位于热键
/// 列表/显示器卡片之后；当前仅 hotkeys 页需要，equiv L1627-L1667）。
pub fn build_post_items(page_id: &str, settings: &Value) -> Vec<SettingsItem> {
    let mut b = ItemBuilder::new();
    if page_id == "hotkeys" {
        hotkeys_page_post(&mut b, settings);
    }
    b.finish()
}

/// 调试页（equiv SettingsWindow.jsx L1722-L1798 的 pageSection debug 两段）。
/// 原版各设置项硬编码英文标题；此处走 FTL 回退链（en.ftl 提供，行为一致）。
fn debug_page(b: &mut ItemBuilder, s: &Value) {
    // ---- All Displays（equiv L1723-L1725：Refresh Monitors / Clear Cache 按钮）----
    b.section(&tt_localize::t("SETTINGS_DEBUG_ALL_DISPLAYS"));
    b.button(
        "refresh-monitors",
        "SETTINGS_DEBUG_REFRESH_MONITORS",
        "",
        "SETTINGS_DEBUG_REFRESH_MONITORS",
    );
    b.button(
        "clear-cache",
        "SETTINGS_DEBUG_CLEAR_CACHE",
        "",
        "SETTINGS_DEBUG_CLEAR_CACHE",
    );

    // ---- Save Report（equiv L1727）----
    b.button(
        "save-report",
        "SETTINGS_DEBUG_SAVE_REPORT",
        "SETTINGS_DEBUG_SAVE_REPORT_DESC",
        "SETTINGS_DEBUG_GENERATE_REPORT",
    );

    // ---- Settings 文件（equiv L1729-L1733：路径显示 + Open Settings 按钮）----
    let settings_path = std::env::var("TT_SETTINGS_PATH").unwrap_or_default();
    b.note(&format!("{}: {}", tt_localize::t("SETTINGS_DEBUG_SETTINGS"), settings_path));
    b.button(
        "open-settings-file",
        "SETTINGS_DEBUG_OPEN_SETTINGS",
        "",
        "SETTINGS_DEBUG_OPEN_SETTINGS",
    );

    // ---- Raw Monitor Data（equiv L1735-L1739：JSON 文本展示）----
    b.section(&tt_localize::t("SETTINGS_DEBUG_RAW_MONITOR_DATA"));

    // ---- Other（equiv L1742-L1798 的开关/数值/下拉区）----
    b.section(&tt_localize::t("SETTINGS_DEBUG_OTHER"));
    b.toggle("isDev", "SETTINGS_DEBUG_DEV_MODE", "", s);
    // UDP Server 组（equiv L1746-L1752）
    b.toggle("udpEnabled", "SETTINGS_DEBUG_UDP_ENABLE", "", s);
    b.toggle("udpRemote", "SETTINGS_DEBUG_UDP_REMOTE", "", s);
    b.number("udpPortStart", "SETTINGS_DEBUG_UDP_PORT", "", s);
    // Active port / UDP key 只读展示（equiv L1750-L1751 的 SettingsChild 无 input）
    let active_port = s.get("udpPortActive").map(json_to_string).unwrap_or_default();
    let udp_key = s.get("udpKey").map(json_to_string).unwrap_or_default();
    b.note(&tt_localize::t("SETTINGS_DEBUG_UDP_ACTIVE").replace("{{1}}", &active_port));
    b.note(&tt_localize::t("SETTINGS_DEBUG_UDP_KEY").replace("{{1}}", &udp_key));
    // DDC/CI Scanning Mode（equiv L1754-L1764 下拉）
    b.select(
        "preferredDDCCIMethod",
        "SETTINGS_DEBUG_DDC_SCAN",
        "",
        s,
        &[
            ("auto", tt_localize::t("SETTINGS_DEBUG_DDC_AUTO")),
            ("fast", tt_localize::t("SETTINGS_DEBUG_DDC_FAST")),
            ("accurate", tt_localize::t("SETTINGS_DEBUG_DDC_ACCURATE")),
            ("no-validation", tt_localize::t("SETTINGS_DEBUG_DDC_NO_VALIDATION")),
            ("legacy", tt_localize::t("SETTINGS_DEBUG_DDC_LEGACY")),
        ],
    );
    let last_method = s.get("lastDetectedDDCCIMethod").map(json_to_string).unwrap_or_default();
    b.note(&tt_localize::t("SETTINGS_DEBUG_DDC_SCAN_DESC").replace("{{1}}", &last_method));
    // 恢复/延迟数值（equiv L1774-L1782）
    b.number("idleRestoreSeconds", "SETTINGS_DEBUG_IDLE_RESTORE", "SETTINGS_DEBUG_IDLE_RESTORE_DESC", s);
    b.number("wakeRestoreSeconds", "SETTINGS_DEBUG_WAKE_RESTORE", "SETTINGS_DEBUG_WAKE_RESTORE_DESC", s);
    b.number("hardwareRestoreSeconds", "SETTINGS_DEBUG_HW_RESTORE", "SETTINGS_DEBUG_HW_RESTORE_DESC", s);
    b.number("checkVCPWaitMS", "SETTINGS_DEBUG_VCP_DELAY", "SETTINGS_DEBUG_VCP_DELAY_DESC", s);
    b.number("scrollFlyoutAmount", "SETTINGS_DEBUG_SCROLL_FLYOUT", "SETTINGS_DEBUG_SCROLL_FLYOUT_DESC", s);
    // 开关区（equiv L1784-L1798）
    b.toggle("disableThemeChanges", "SETTINGS_DEBUG_DISABLE_THEME", "SETTINGS_DEBUG_DISABLE_THEME_DESC", s);
    b.toggle("restartOnWake", "SETTINGS_DEBUG_RESTART_WAKE", "", s);
    b.toggle("disableAutoRefresh", "SETTINGS_DEBUG_DISABLE_AUTO_REFRESH", "SETTINGS_DEBUG_DISABLE_AUTO_REFRESH_DESC", s);
    b.toggle("useWin32Event", "SETTINGS_DEBUG_USE_WIN32", "", s);
    b.toggle("useElectronEvents", "SETTINGS_DEBUG_USE_ELECTRON", "", s);
    b.toggle("useWmDisplayChangeEvent", "SETTINGS_DEBUG_USE_WM_DISPLAYCHANGE", "", s);
    b.toggle("useScMonitorPowerEvent", "SETTINGS_DEBUG_USE_SC_MONITORPOWER", "", s);
    b.toggle("useGuidPresenceEvent", "SETTINGS_DEBUG_USE_GUID_PRESENCE", "", s);
    b.toggle("useGuidBrightnessEvent", "SETTINGS_DEBUG_USE_GUID_BRIGHTNESS", "", s);
    b.toggle("reloadTray", "SETTINGS_DEBUG_RELOAD_TRAY", "", s);
    b.toggle("reloadFlyout", "SETTINGS_DEBUG_RELOAD_FLYOUT", "", s);
    b.toggle("logging", "SETTINGS_DEBUG_LOGGING", "SETTINGS_DEBUG_LOGGING_DESC", s);
    b.toggle("showConsole", "SETTINGS_DEBUG_SHOW_CONSOLE", "", s);
    b.toggle("useTaskbarRegistry", "SETTINGS_DEBUG_TASKBAR_REGISTRY", "", s);
    b.toggle("disableMouseEvents", "SETTINGS_DEBUG_DISABLE_MOUSE", "", s);
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
    // 六项在原版 SettingsWindow.jsx L1417-L1422 是硬编码英文标题（无 FTL 键），
    // 必须用 toggle_literal 直接给字面量，否则标题会被当成缺失的键翻成空串。
    b.toggle_literal("disableAppleStudio", "Apple Studio Displays", s);
    b.toggle_literal("disableHighLevel", "DDC/CI HL", s);
    b.toggle_literal("disableHDR", "HDR", s);
    b.toggle_literal("disableWMIC", "WMIC", s);
    b.toggle_literal("disableWMI", "WMI-Bridge", s);
    b.toggle_literal("disableWin32", "Win32-DisplayConfig", s);
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
    // sleepAction（TOD）不在显示器页——原版该下拉只出现在热键页
    // 的 Turn Off Displays 区（SettingsWindow.jsx L1659-L1667），已移至 hotkeys_page_post。
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
/// 热键页（列表之前的部分，equiv SettingsWindow.jsx L1608-L1625）：
/// 标题 + 说明 + 原生亮度警告（仅当热键含 BrightnessUp/BrightnessDown 时显示，
/// equiv L1336-L1340 hasNativeBrightnessHotkey）。列表本体由 Slint hotkeys 分支渲染。
fn hotkeys_page(b: &mut ItemBuilder, s: &Value) {
    b.section(&tt_localize::t("SETTINGS_HOTKEYS_TITLE"));
    b.note(&tt_localize::t("SETTINGS_HOTKEYS_DESC"));
    if has_native_brightness_hotkey(s) {
        b.note(&format!(
            "⚠️ {}",
            tt_localize::t("SETTINGS_HOTKEYS_NATIVE_BRIGHTNESS_WARN")
        ));
        b.note(&tt_localize::t("SETTINGS_HOTKEYS_NATIVE_BRIGHTNESS_CHECK"));
    }
}

/// 热键页（列表之后的部分，equiv SettingsWindow.jsx L1627-L1667）：
/// break linked levels / 托盘滚动快捷键 / TOD（关闭显示器动作）。
fn hotkeys_page_post(b: &mut ItemBuilder, s: &Value) {
    b.toggle(
        "hotkeysBreakLinkedLevels",
        "SETTINGS_HOTKEYS_BREAK_TITLE",
        "SETTINGS_HOTKEYS_BREAK_DESC",
        s,
    );
    b.section(&tt_localize::t("SETTINGS_GENERAL_SCROLL_TITLE"));
    b.toggle(
        "scrollShortcut",
        "SETTINGS_GENERAL_SCROLL_TITLE",
        "SETTINGS_GENERAL_SCROLL_DESC",
        s,
    );
    // equiv L1649-L1656：Amount to scroll 数值输入（键 SETTINGS_HOTKEYS_SCROLL_AMOUNT）
    b.number("scrollShortcutAmount", "SETTINGS_HOTKEYS_SCROLL_AMOUNT", "", s);
    b.toggle(
        "invertScroll",
        "SETTINGS_HOTKEYS_INVERT_SCROLL_TITLE",
        "SETTINGS_HOTKEYS_INVERT_SCROLL_DESC",
        s,
    );
    // TOD（equiv L1659-L1667：Turn Off Displays action 下拉 + 注记 + DDC 警告）
    let sleep_action = s
        .get("sleepAction")
        .and_then(|v| v.as_str())
        .unwrap_or("ps");
    b.select(
        "sleepAction",
        "SETTINGS_HOTKEYS_TOD_TITLE",
        "SETTINGS_HOTKEYS_TOD_DESC",
        s,
        &[
            ("none", tt_localize::t("SETTINGS_HOTKEYS_TOD_NONE")),
            ("ps", tt_localize::t("SETTINGS_HOTKEYS_TOD_SOFT")),
            ("ddcci", tt_localize::t("SETTINGS_HOTKEYS_TOD_HARD")),
            ("ps_ddcci", tt_localize::t("SETTINGS_HOTKEYS_TOD_BOTH")),
        ],
    );
    b.note(&tt_localize::t("SETTINGS_HOTKEYS_TOD_NOTE"));
    if sleep_action == "ddcci" || sleep_action == "ps_ddcci" {
        b.note(&format!(
            "⚠️ {}",
            tt_localize::t("GENERIC_DDC_WARNING")
        ));
    }
}

/// 热键列表是否包含原生亮度键（equiv SettingsWindow.jsx L1336-L1340：
/// hotkeys.some(h => h.accelerator === "BrightnessUp" || "BrightnessDown")）。
fn has_native_brightness_hotkey(s: &Value) -> bool {
    s.get("hotkeys")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter().any(|h| {
                matches!(
                    h.get("accelerator").and_then(|v| v.as_str()),
                    Some("BrightnessUp") | Some("BrightnessDown")
                )
            })
        })
        .unwrap_or(false)
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
    // 亮度归一化 remap（equiv settings.remaps；getRemap 先按 id 再按 name 回退）
    let remaps = settings.get("remaps").cloned().unwrap_or(json!({}));

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
            // equiv getInfoMonitors L863-L881：displayLevel 只取整显示；不支持/探测失败分别为
            // "Not supported" 与 "???"（JS 真值语义判断 brightnessType）
            let brightness_type = m
                .get("brightnessType")
                .map(json_truthy)
                .unwrap_or(false);
            let not_supported = tt_localize::t("GENERIC_NOT_SUPPORTED");
            let display_level = |v: Option<&Value>| match v.and_then(|v| v.as_f64()) {
                Some(n) => format!("{}", n.round() as i64),
                None => "???".to_string(),
            };
            let brightness = match type_str {
                "none" => not_supported.clone(),
                "ddcci" if !brightness_type => "???".to_string(),
                _ => display_level(m.get("brightness")),
            };
            let maximum = match type_str {
                "ddcci" | "software" => {
                    if type_str == "ddcci" && !brightness_type {
                        "???".to_string()
                    } else {
                        display_level(m.get("brightnessMax"))
                    }
                }
                _ => not_supported.clone(),
            };
            let normalization = if type_str == "none" {
                not_supported.clone()
            } else {
                format!(
                    "{} - {}",
                    m.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0).round() as i64,
                    m.get("max").and_then(|v| v.as_f64()).unwrap_or(100.0).round() as i64
                )
            };
            let hdr = match m.get("hdr").and_then(|v| v.as_str()) {
                Some("active") => tt_localize::t("GENERIC_ACTIVE"),
                Some("supported") => tt_localize::t("GENERIC_SUPPORTED"),
                _ => tt_localize::t("GENERIC_UNSUPPORTED"),
            };
            // equiv getInfoMonitors 的 <p> 明细（Name / Internal Name / Communication /
            // Current Brightness / Max Brightness / Brightness Normalization / HDR）
            let internal_name = m
                .get("hwid")
                .and_then(|v| v.as_array())
                .and_then(|a| a.get(1))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let display_name = monitor_view::settings_monitor_name(&m, &names);
            let details = format!(
                "{}: {}\n{}: {}\n{}: {}\n{}: {}\n{}: {}\n{}: {}\n{}: {}",
                tt_localize::t("SETTINGS_MONITORS_DETAILS_NAME"),
                display_name,
                tt_localize::t("SETTINGS_MONITORS_DETAILS_INTERNAL_NAME"),
                internal_name,
                tt_localize::t("SETTINGS_MONITORS_DETAILS_COMMUNICATION"),
                connection,
                tt_localize::t("SETTINGS_MONITORS_DETAILS_BRIGHTNESS"),
                brightness,
                tt_localize::t("SETTINGS_MONITORS_DETAILS_MAX_BRIGHTNESS"),
                maximum,
                tt_localize::t("SETTINGS_MONITORS_DETAILS_BRIGHTNESS_NORMALIZATION"),
                normalization,
                tt_localize::t("SETTINGS_MONITORS_DETAILS_HDR"),
                hdr
            );
            let remap = remap_of(&remaps, &m);
            // 校准点（equiv getMonitorCalibration L509-L534）：input 为滑块级别、output 为下发级别
            let calibration = remap
                .and_then(|r| r.get("calibration"))
                .and_then(|v| v.as_array())
                .map(|points| {
                    points
                        .iter()
                        .enumerate()
                        .map(|(index, point)| SettingsCalibrationPoint {
                            index: index as i32,
                            input: point.get("input").and_then(|v| v.as_f64()).unwrap_or(0.0)
                                as f32,
                            output: point.get("output").and_then(|v| v.as_f64()).unwrap_or(100.0)
                                as f32,
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let remap_min = remap
                .and_then(|r| r.get("min"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as f32;
            let remap_max = remap
                .and_then(|r| r.get("max"))
                .and_then(|v| v.as_f64())
                .unwrap_or(100.0) as f32;
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
                title: m
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .into(),
                connection: connection.into(),
                hdr: hdr.into(),
                brightness: brightness.into(),
                maximum: maximum.into(),
                normalization: normalization.into(),
                details: details.into(),
                remap_min,
                remap_max,
                calibration: ModelRc::new(VecModel::from(calibration)),
            }
        })
        .collect()
}

/// getRemap（equiv SettingsWindow.jsx:L263-L273）：先按显示器 id，找不到再按 name 回退。
fn remap_of<'a>(remaps: &'a Value, m: &Value) -> Option<&'a Value> {
    let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
    if let Some(r) = remaps.get(id) {
        if !r.is_null() {
            return Some(r);
        }
    }
    let name = m.get("name").and_then(|v| v.as_str()).unwrap_or("");
    remaps.get(name)
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
    use slint::Model;

    #[test]
    fn pages_match_origin_sidebar() {
        let pages = build_pages();
        let ids: Vec<&str> = pages.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["general", "monitors", "features", "time", "hotkeys", "light-sensor", "updates", "debug"]
        );
    }

    #[test]
    fn select_resolves_current_index() {
        let values = vec!["default", "dark", "light"];
        assert_eq!(compute_selection_index("dark", &values), 1);
        // 未知值 -> 从第一个开始
        assert_eq!(compute_selection_index("zzz", &values), 0);
        assert_eq!(compute_selection_index("light", &values), 2);
        // 空选项不 panic
        assert_eq!(compute_selection_index("dark", &[]), 0);
    }

    #[test]
    fn general_page_covers_key_settings() {        let settings = json!({"openAtLogin": true, "theme": "dark", "useAcrylic": false});
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
    fn troubleshooting_toggles_keep_literal_titles() {
        // equiv SettingsWindow.jsx L1417-L1422：这六项标题在原版就是硬编码字面量，
        // 走 t() 会因源语料无对应键而得到空串（表现为「开关无字」）。
        let items = build_items("general", &json!({}));
        for (key, title) in [
            ("disableAppleStudio", "Apple Studio Displays"),
            ("disableHighLevel", "DDC/CI HL"),
            ("disableHDR", "HDR"),
            ("disableWMIC", "WMIC"),
            ("disableWMI", "WMI-Bridge"),
            ("disableWin32", "Win32-DisplayConfig"),
        ] {
            let item = items.iter().find(|i| i.key == key).unwrap();
            assert_eq!(item.title.as_str(), title, "{} 标题丢失", key);
        }
        // 整页任何设置项都不该渲染成无文字的空行。
        for item in &items {
            assert!(!item.title.is_empty(), "{} 标题为空", item.key);
        }
    }

    #[test]
    fn select_items_carry_aligned_options() {
        // 下拉项：values/labels 同序等长，selected-index 指向当前值
        let settings = json!({"theme": "dark", "language": "en"});
        let items = build_items("general", &settings);
        let theme = items.iter().find(|i| i.key == "theme").unwrap();
        assert_eq!(theme.option_values.row_count(), theme.option_labels.row_count());
        assert_eq!(theme.selected_index, 1);
        assert_eq!(theme.option_values.row_data(1).as_deref(), Some("dark"));
        let lang = items.iter().find(|i| i.key == "language").unwrap();
        assert_eq!(lang.option_values.row_data(1).as_deref(), Some("en"));
    }

    #[test]
    fn monitor_entry_connection_labels() {
        // equiv getInfoMonitors：ddcci 有 brightnessType 时显示读数，缺省则为 "???"
        let monitors = json!({
            "k1": {"key": "k1", "id": "\\\\.\\DISPLAY1", "name": "CMN1521", "type": "ddcci",
                   "highLevelSupported": {"brightness": true}, "brightnessType": 1,
                   "brightness": 60, "brightnessMax": 100, "min": 0, "max": 100,
                   "hwid": ["MONITOR\\CMN1521", "CMN1521", "5&1&0&UID1"]}
        });
        let entries = build_monitor_entries(&monitors, &json!({}));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].connection, "DDC/CI HL");
        assert_eq!(entries[0].title, "CMN1521");
        assert_eq!(entries[0].brightness, "60");
        assert_eq!(entries[0].maximum, "100");
        assert_eq!(entries[0].normalization, "0 - 100");
        assert!(entries[0].details.contains("CMN1521"));
        assert!(entries[0].details.contains("Current Brightness: 60"));
    }

    #[test]
    fn monitor_entry_ddcci_without_brightness_type_shows_unknown() {
        // equiv getInfoMonitors L866-L869：type=ddcci 且无 brightnessType -> "???"
        let monitors = json!({
            "k1": {"key": "k1", "id": "DISPLAY1", "name": "X", "type": "ddcci",
                   "brightness": 60, "brightnessMax": 100, "min": 0, "max": 100}
        });
        let entries = build_monitor_entries(&monitors, &json!({}));
        assert_eq!(entries[0].brightness, "???");
        assert_eq!(entries[0].maximum, "???");
    }

    #[test]
    fn monitor_entry_none_type_reports_not_supported() {
        // equiv getInfoMonitors L878-L880：type=none 的三项均为 Not supported
        let monitors = json!({
            "k1": {"key": "k1", "id": "DISPLAY1", "name": "Ghost", "type": "none"}
        });
        let entries = build_monitor_entries(&monitors, &json!({}));
        let not_supported = tt_localize::t("GENERIC_NOT_SUPPORTED");
        assert_eq!(entries[0].brightness, not_supported);
        assert_eq!(entries[0].maximum, not_supported);
        assert_eq!(entries[0].normalization, not_supported);
    }

    #[test]
    fn monitors_page_renders_all_display_items() {
        // 原版 monitors 页的全局项（updateInterval / hideClosedLid / normalize 说明）
        let items = build_items(
            "monitors",
            &json!({"updateInterval": 500, "hideClosedLid": true}),
        );
        let keys: Vec<&str> = items.iter().map(|i| i.key.as_str()).collect();
        assert!(keys.contains(&"updateInterval"));
        assert!(keys.contains(&"hideClosedLid"));
        assert!(items
            .iter()
            .any(|i| i.kind == SettingsItemKind::Note && !i.title.is_empty()));
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

    #[test]
    fn hotkeys_pre_items_and_conditional_native_warn() {
        // 无原生亮度键：仅标题 + 说明（equiv L1336-L1340 条件不满足）
        let plain = json!({"hotkeys": [{"accelerator": "Control+Shift+Up"}]});
        let pre = build_items("hotkeys", &plain);
        assert_eq!(pre.len(), 2);
        assert_eq!(pre[0].title, tt_localize::t("SETTINGS_HOTKEYS_TITLE"));
        assert_eq!(pre[1].title, tt_localize::t("SETTINGS_HOTKEYS_DESC"));

        // 含 BrightnessUp：追加两条原生亮度警告（equiv L1612-L1617）
        let native = json!({"hotkeys": [{"accelerator": "BrightnessUp"}]});
        let pre2 = build_items("hotkeys", &native);
        assert_eq!(pre2.len(), 4);
        assert!(pre2[2]
            .title
            .contains(&tt_localize::t("SETTINGS_HOTKEYS_NATIVE_BRIGHTNESS_WARN")));
        assert_eq!(
            pre2[3].title,
            tt_localize::t("SETTINGS_HOTKEYS_NATIVE_BRIGHTNESS_CHECK")
        );
    }

    #[test]
    fn hotkeys_post_items_match_origin_sections() {
        let s = json!({
            "hotkeysBreakLinkedLevels": true,
            "scrollShortcut": false,
            "scrollShortcutAmount": 10,
            "invertScroll": false,
            "sleepAction": "ddcci"
        });
        let post = build_post_items("hotkeys", &s);
        let keys: Vec<&str> = post
            .iter()
            .filter(|i| !i.key.is_empty())
            .map(|i| i.key.as_str())
            .collect();
        // 顺序与原版一致：break → scrollShortcut → amount → invert → TOD
        assert_eq!(
            keys,
            vec![
                "hotkeysBreakLinkedLevels",
                "scrollShortcut",
                "scrollShortcutAmount",
                "invertScroll",
                "sleepAction"
            ]
        );
        // 滚动量标题走正确键（SETTINGS_HOTKEYS_SCROLL_AMOUNT，非不存在的 *_TITLE）
        assert_eq!(
            post.iter().find(|i| i.key == "scrollShortcutAmount").unwrap().title,
            tt_localize::t("SETTINGS_HOTKEYS_SCROLL_AMOUNT")
        );
        // sleepAction == ddcci 时 DDC 警告出现（equiv L1665-L1667）
        assert!(post
            .iter()
            .any(|i| i.title.contains(&tt_localize::t("GENERIC_DDC_WARNING"))));
        // 其他页无列表后置项
        assert!(build_post_items("general", &s).is_empty());
    }

    #[test]
    fn monitors_page_has_no_sleep_action() {
        // 原版 sleepAction 只在热键页 TOD 区，不在显示器页
        let items = build_items("monitors", &json!({}));
        assert!(items.iter().all(|i| i.key != "sleepAction"));
    }
}