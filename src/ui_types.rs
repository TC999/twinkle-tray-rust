//! Slint 生成类型再导出（供业务层引用而不直接依赖 slint 内部模块名）。

pub use crate::{
    DisplayType, FeatureKind, PanelFeature, PanelInput, PanelMonitor, SettingsChoice,
    SettingsHotkey, SettingsItem, SettingsItemKind, SettingsMonitorEntry, SettingsPage,
};
