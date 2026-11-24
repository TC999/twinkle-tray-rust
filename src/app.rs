//! 主进程启动序列（electron.js 的 app 生命周期等价移植）。
//!
//! equiv: electron.js:L1-L130（路径与版本常量）、L862-L868（配置目录）、
//! L1201-L1214（readSettings 后的日志/开关）、L4514-L4586（app ready/quit 钩子）。
//!
//! 本模块只做「启动前置」：路径解析、settings 读取与迁移、日志初始化、
//! worker 拉起。UI 与托盘由 lib.rs 负责（AGENTS.md §5 线程模型）。

use std::path::{Path, PathBuf};

/// 应用版本（equiv package.json versionBuild 的 version 段）。
pub const APP_VERSION: &str = "1.18.0";
/// 版本构建串（equiv appBuild；开发构建恒为 dev）。
pub const APP_BUILD: &str = "dev";

/// 主进程路径集合（equiv L24-L27）。
#[derive(Debug, Clone)]
pub struct AppPaths {
    /// configFilesDir（isPortable 时是 exe 同级 config/，否则 userData）
    pub config_dir: PathBuf,
    /// settings[-dev].json
    pub settings_path: PathBuf,
    /// known-displays[-dev].json
    pub known_displays_path: PathBuf,
    /// ddc-unstable.json（恒在 userData，不受 portable 影响）
    pub ddc_unstable_path: PathBuf,
    /// debug[-dev].log
    pub debug_log_path: PathBuf,
    /// userData
    pub user_data: PathBuf,
}

impl AppPaths {
    /// 解析路径（equiv L24-L27 + L113）。
    /// user_data 通常是 %APPDATA%\<app name>；is_portable 时配置目录改为 exe 同级 config/。
    pub fn resolve(is_dev: bool, is_portable: bool, user_data: &Path, exe_dir: &Path) -> Self {
        let suffix = if is_dev { "-dev" } else { "" };
        let config_dir = if is_portable {
            // path.join(__dirname, "../../config/")：exe 同级 config/
            exe_dir.join("config")
        } else {
            user_data.to_path_buf()
        };
        Self {
            settings_path: config_dir.join(format!("settings{}.json", suffix)),
            known_displays_path: config_dir.join(format!("known-displays{}.json", suffix)),
            // ddcUnstablePath = path.join(app.getPath("userData"), "ddc-unstable.json")
            ddc_unstable_path: user_data.join("ddc-unstable.json"),
            debug_log_path: config_dir.join(format!("debug{}.log", suffix)),
            config_dir,
            user_data: user_data.to_path_buf(),
        }
    }

    /// 确保配置目录存在（equiv L862-L868）。
    pub fn ensure_config_dir(&self) {
        if !self.config_dir.exists() {
            if let Err(e) = std::fs::create_dir_all(&self.config_dir) {
                eprintln!("Couldn't create config directory: {}", e);
            }
        }
    }
}

/// 启动期解析出的运行模式（equiv L6-L18 的开关组合）。
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct LaunchOptions {
    /// --dev
    pub is_dev: bool,
    /// --console（logger 镜像输出）
    pub has_console: bool,
    /// app.name == "twinkle-tray-portable"
    pub is_portable: bool,
    /// app.name == "twinkle-tray-appx"
    pub is_appx: bool,
    /// --monitor-worker（子进程自调用）
    pub is_monitor_worker: bool,
    /// --app-smoke（自检：启动主进程序列后立即退出）
    pub app_smoke: bool,
    /// --ui-smoke（自检：创建 Slint 四窗口 + 注入模型后退出）
    pub ui_smoke: bool,
    /// --ui-demo（真机渲染：显示面板 + 设置窗口供截图对拍）
    pub ui_demo: bool,
    /// --ui-demo=<page>（ui-demo 的设置窗口初始页，如 hotkeys/monitors/general）
    pub ui_demo_page: Option<String>,
    /// --brightness-smoke=<level>（亮度闭环对拍）
    pub brightness_smoke: Option<f64>,
    /// --udp-smoke（UDP 服务器真机对拍）
    pub udp_smoke: bool,
}

impl LaunchOptions {
    /// 从命令行参数解析（等价 app.commandLine.hasSwitch）。
    pub fn parse(args: &[String]) -> Self {
        let has = |name: &str| {
            args.iter()
                .any(|a| a == name || a.starts_with(&format!("{} ", name)) || a == &format!("--{}==true", &name[2..]))
        };
        Self {
            is_dev: has("--dev"),
            has_console: has("--console"),
            is_portable: args.iter().any(|a| a == "--portable"),
            is_appx: args.iter().any(|a| a == "--appx"),
            is_monitor_worker: args.iter().any(|a| a == "--monitor-worker"),
            app_smoke: args.iter().any(|a| a == "--app-smoke"),
            ui_smoke: args.iter().any(|a| a == "--ui-smoke"),
            ui_demo: args
                .iter()
                .any(|a| a == "--ui-demo" || a.starts_with("--ui-demo=")),
            ui_demo_page: args
                .iter()
                .find_map(|a| a.strip_prefix("--ui-demo="))
                .map(|v| v.to_string()),
            brightness_smoke: args
                .iter()
                .find_map(|a| a.strip_prefix("--brightness-smoke="))
                .and_then(|v| v.parse::<f64>().ok()),
            udp_smoke: args.iter().any(|a| a == "--udp-smoke"),
        }
    }
}

/// 默认 userData 目录（equiv app.getPath("userData")）：%APPDATA%\Twinkle Tray。
pub fn default_user_data_dir() -> PathBuf {
    let appdata = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("C:\\ProgramData"));
    appdata.join("Twinkle Tray")
}

/// 日志级别与镜像输出（equiv L37 patchConsole + L1206-L1207 initMainLogger）。
pub fn init_logging(paths: &AppPaths, opts: &LaunchOptions, logging_enabled: bool) {
    // 非 dev 且无 --console 时只收集到文件（原注释语义）
    crate::logger::set_mirror_log(opts.is_dev || opts.has_console);
    crate::logger::init_main_logger(&paths.config_dir, opts.is_dev, logging_enabled);
    if crate::logger::is_logging_enabled() {
        if let Some(p) = crate::logger::session_log_path() {
            crate::logger::log_as("MAIN", "info", &format!("Session log file: {:?}", p));
        }
    }
}

/// readSettings 之后、processSettings 之前的启动前置（equiv L1209-L1218）：
/// disableThrottling / forceLowPowerGPU 在 Electron 中是 Chromium 开关，
/// 原生实现下对应进程优先级与 GPU 选择，这里记录并被后续模块消费。
#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub struct StartupFlags {
    pub disable_throttling: bool,
    pub force_low_power_gpu: bool,
    pub show_console: bool,
}

pub fn read_startup_flags(settings: &serde_json::Value) -> StartupFlags {
    let flag = |k: &str| settings.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    StartupFlags {
        disable_throttling: flag("disableThrottling"),
        force_low_power_gpu: flag("forceLowPowerGPU"),
        show_console: flag("showConsole"),
    }
}

/// 删除上次会话遗留的 debug 日志（equiv L117-L123）。
pub fn remove_old_debug_log(paths: &AppPaths) {
    if paths.debug_log_path.exists() {
        if std::fs::remove_file(&paths.debug_log_path).is_err() {
            eprintln!("Couldn't delete log file");
        }
    }
}

/// 删除旧 debug 日志前的路径存在性探测（供测试）。
#[allow(dead_code)]
pub fn debug_log_exists(paths: &AppPaths) -> bool {
    paths.debug_log_path.exists()
}

/// 配置目录路径的字符串形式（等价 path.join 后的可比较值）。
#[allow(dead_code)]
pub fn path_str(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_paths_match_electron_layout() {
        let paths = AppPaths::resolve(
            false,
            false,
            Path::new(r"C:\Users\u\AppData\Roaming\Twinkle Tray"),
            Path::new(r"C:\app"),
        );
        assert!(paths.settings_path.ends_with("settings.json"));
        assert!(paths.known_displays_path.ends_with("known-displays.json"));
        assert!(paths.debug_log_path.ends_with("debug.log"));
        // ddc-unstable.json 恒在 userData 根
        assert_eq!(
            paths.ddc_unstable_path,
            Path::new(r"C:\Users\u\AppData\Roaming\Twinkle Tray\ddc-unstable.json")
        );
    }

    #[test]
    fn dev_suffix_and_portable_dir() {
        let paths = AppPaths::resolve(
            true,
            true,
            Path::new(r"C:\Users\u\AppData\Roaming\Twinkle Tray"),
            Path::new(r"C:\Portable\app"),
        );
        assert!(paths.settings_path.ends_with("settings-dev.json"));
        assert!(paths.debug_log_path.ends_with("debug-dev.log"));
        // portable：配置目录是 exe 同级 config/
        assert_eq!(paths.config_dir, Path::new(r"C:\Portable\app\config"));
        // 但 ddc-unstable.json 仍在 userData（原版常量如此）
        assert!(paths
            .ddc_unstable_path
            .starts_with(r"C:\Users\u\AppData\Roaming\Twinkle Tray"));
    }

    #[test]
    fn launch_options_parse_switches() {
        let args: Vec<String> = ["app.exe", "--dev", "--console", "--app-smoke"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let opts = LaunchOptions::parse(&args);
        assert!(opts.is_dev);
        assert!(opts.has_console);
        assert!(opts.app_smoke);
        assert!(!opts.is_monitor_worker);
        assert!(!opts.ui_smoke);
    }

    #[test]
    fn startup_flags_read_settings() {
        let flags = read_startup_flags(&serde_json::json!({
            "disableThrottling": true,
            "forceLowPowerGPU": false,
            "showConsole": true
        }));
        assert!(flags.disable_throttling);
        assert!(!flags.force_low_power_gpu);
        assert!(flags.show_console);
    }
}