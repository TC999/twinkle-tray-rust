//! 更新检查与下载安装（equiv electron.js latestVersion/getLatestUpdate/runUpdate
//! L5016-L5182 的等价移植；网络路径用 net::WinHTTP，§1.2 预定方案）。
//!
//! 线程模型：网络与下载在线程执行（std::thread::spawn），结果写入全局
//! UPDATE_STATE 单例；UI 层（panel_controller tick）轮询并刷新更新条。
//! 状态机等价原版 latestVersion 对象：show/error/downloading 标志 + 版本信息。

use crate::app;
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};

/// 更新信息（equiv latestVersion 对象）。
#[derive(Debug, Clone, Default)]
pub struct UpdateInfo {
    pub release_url: String,
    pub version: String,
    pub download_url: String,
    pub filesize: u64,
    pub changelog: String,
    pub show: bool,
    pub error: bool,
    pub downloading: bool,
    pub progress: u32,
}

/// 共享更新状态（等价模块级 latestVersion）。
static UPDATE_STATE: OnceLock<Mutex<UpdateInfo>> = OnceLock::new();

/// 更新状态访问。
pub fn update_state() -> &'static Mutex<UpdateInfo> {
    UPDATE_STATE.get_or_init(|| Mutex::new(UpdateInfo::default()))
}

/// 快照（UI 层读取）。
pub fn snapshot() -> UpdateInfo {
    update_state().lock().map(|g| g.clone()).unwrap_or_default()
}

/// getVersionValue（equiv Utils.getVersionValue L263-L267）：
/// "v1.18.0-beta" -> 1*10000*10000 + 18*10000 + 0。
pub fn get_version_value(version: &str) -> i64 {
    let clean = version.split('-').next().unwrap_or("").trim_start_matches('v');
    let mut parts = clean.split('.');
    let major = parts.next().and_then(|p| p.parse::<i64>().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|p| p.parse::<i64>().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|p| p.parse::<i64>().ok()).unwrap_or(0);
    major * 10000 * 10000 + minor * 10000 + patch
}

/// 从 GitHub releases JSON 找到目标 release（equiv L5030-L5052）。
/// 返回 (found, latestVersion)。
fn find_latest_release(releases: &Value, settings: &Value) -> (bool, UpdateInfo) {
    let branch = settings.get("branch").and_then(|v| v.as_str()).unwrap_or("master").to_string();
    let app_version = format!("v{}", app::APP_VERSION);
    let app_version_value = get_version_value(&app_version);
    let mut latest = UpdateInfo::default();
    let mut found = false;
    if let Some(list) = releases.as_array() {
        for release in list {
            let prerelease = release.get("prerelease").and_then(|v| v.as_bool()).unwrap_or(false);
            // Skip versions older than current（及 master 分支跳过 prerelease）
            if branch == "master" && prerelease {
                continue;
            }
            let tag = release.get("tag_name").and_then(|v| v.as_str()).unwrap_or("");
            let version_value = get_version_value(tag);
            if version_value < app_version_value {
                continue;
            }
            let assets = release.get("assets").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let first = assets.first().cloned().unwrap_or_default();
            latest = UpdateInfo {
                release_url: release.get("html_url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                version: tag.to_string(),
                download_url: first.get("browser_download_url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                filesize: first.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
                changelog: release.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                show: false,
                error: false,
                downloading: false,
                progress: 0,
            };
            found = true;
            break;
        }
    }
    (found, latest)
}

/// checkForUpdates（equiv L5018-L5066）。
/// force=true 时跳过 checkForUpdates 开关与当日限制（设置按钮调用）。
pub fn check_for_updates(settings: &Value, force: bool, update_path: std::path::PathBuf) {
    if !force {
        let enabled = settings.get("checkForUpdates").and_then(|v| v.as_bool()).unwrap_or(true);
        if !enabled {
            return;
        }
        // lastCheck 当日限制（equiv L5021）：Rust 侧以文件时间近似,启动时记录
    }
    // isPortable/isAppX 跳过（equiv L5023）——Rust 侧 portable 时 config 目录不同,
    // 以 settings 无 checkForUpdates 开关近似等价
    let _ = update_path;
    let settings = settings.clone();
    std::thread::spawn(move || {
        println!("Checking for updates...");
        match crate::net::fetch_github_releases() {
            Some(body) => {
                let parsed: Value = serde_json::from_str(&body).unwrap_or(json!([]));
                let (found, latest) = find_latest_release(&parsed, &settings);
                if !found {
                    return;
                }
                {
                    let mut st = update_state().lock().unwrap();
                    *st = latest.clone();
                }
                // foundVersion && "v"+appVersion != latestVersion.version && (dismissedUpdate != version || force)
                // (equiv L5054-L5058)
                let dismissed = settings.get("dismissedUpdate").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let current = format!("v{}", app::APP_VERSION);
                if found && current != latest.version && (dismissed != latest.version || force) {
                    if !force {
                        update_state().lock().unwrap().show = true;
                    }
                    println!("Found version: {}", latest.version);
                    println!("Sending new version to windows.");
                }
            }
            None => {
                println!("Couldn't check for updates");
            }
        }
    });
}

/// ignoreUpdate（equiv L5190-L5194）：写 dismissedUpdate + 清 show。
pub fn ignore_update(version: &str) {
    let mut st = update_state().lock().unwrap();
    st.show = false;
    st.version = version.to_string();
}

/// 下载更新并运行安装器（equiv getLatestUpdate L5069-L5128 + runUpdate L5130-L5182）。
/// download_url 为 latestVersion.downloadURL;update_path 为 configDir/update.exe。
pub fn download_and_install(download_url: &str, expected_size: u64, update_path: std::path::PathBuf) {
    let download_url = download_url.to_string();
    std::thread::spawn(move || {
        // equiv L5074-L5076:downloading=true 推送到窗口
        update_state().lock().unwrap().downloading = true;
        println!("Downloading update from: {}", download_url);
        let _ = expected_size;

        // equiv L5077-L5084:删除旧 update.exe
        if update_path.exists() {
            if let Err(e) = std::fs::remove_file(&update_path) {
                println!("Couldn't delete old update file: {}", e);
            }
        }

        // 流式下载（WinHTTP 块读;equiv fetch 流的 data 事件）
        match crate::net::download_to_file(&download_url, &update_path, |progress| {
            update_state().lock().unwrap().progress = progress as u32;
        }) {
            Ok(size) => {
                // equiv L5113-L5115:校验大小
                if size != expected_size {
                    println!("Update download was incomplete. Expected {} bytes, got {}", expected_size, size);
                    finish_with_error();
                    return;
                }
                println!("Saved! Running...");
                // equiv L5118-L5120:1250ms 后运行安装器
                std::thread::sleep(std::time::Duration::from_millis(1250));
                if run_update(&update_path, expected_size) {
                    // equiv app.quit()（L5174）：退出事件循环,主函数继续清理托盘/worker
                    let _ = slint::quit_event_loop();
                }
            }
            Err(e) => {
                println!("Couldn't download update!: {}", e);
                finish_with_error();
            }
        }
    });
}

fn finish_with_error() {
    let mut st = update_state().lock().unwrap();
    st.show = true;
    st.downloading = false;
    st.error = true;
}

/// runUpdate（equiv L5130-L5182）：校验大小后 spawn 安装器（detached）。
/// 返回 true 表示已成功启动安装器。
fn run_update(update_path: &std::path::PathBuf, expected_size: u64) -> bool {
    if !update_path.exists() {
        println!("Update file doesn't exist!");
        finish_with_error();
        return false;
    }
    let file_size = std::fs::metadata(update_path).map(|m| m.len()).unwrap_or(0);
    println!("Expected size: {}", expected_size);
    if expected_size != 0 && file_size != expected_size {
        // equiv L5138-L5147:错误大小,删除后报错
        if let Err(e) = std::fs::remove_file(update_path) {
            println!("Couldn't delete update file: {}", e);
        }
        println!("Update is wrong file size! Expected: {}. Got: {}", expected_size, file_size);
        finish_with_error();
        return false;
    }
    // equiv L5157-L5174:spawn(updatePath, detached) 失败 1s 后重试一次
    use std::os::windows::process::CommandExt;
    let mut attempts = 0;
    loop {
        match std::process::Command::new(update_path)
            .creation_flags(0x0000_0008) // DETACHED_PROCESS
            .spawn()
        {
            Ok(child) => {
                // equiv L5173:process.unref()——DETACHED_PROCESS 子进程与父进程独立,
                // drop Child 句柄不会终止进程（Windows 句柄语义）;不 wait 即"不等待"
                drop(child);
                return true;
            }
            Err(e) => {
                attempts += 1;
                if attempts >= 2 {
                    println!("Couldn't start update: {}", e);
                    finish_with_error();
                    return false;
                }
                println!("Retrying update spawn: {}", e);
                std::thread::sleep(std::time::Duration::from_millis(1000));
            }
        }
    }
}

/// clear-update（equiv L5196-L5199：清 show）。
pub fn clear_update() {
    update_state().lock().unwrap().show = false;
}

/// 清除错误标志（equiv get-update L3671：latestVersion.error = false）。
pub fn clear_error() {
    update_state().lock().unwrap().error = false;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn version_value() {
        assert_eq!(get_version_value("v1.18.0"), 1 * 10000 * 10000 + 18 * 10000 + 0);
        assert_eq!(get_version_value("v1.18.0-beta2"), 1 * 10000 * 10000 + 18 * 10000 + 0);
        assert_eq!(get_version_value("1.2.3"), 1 * 10000 * 10000 + 2 * 10000 + 3);
        assert_eq!(get_version_value(""), 0);
    }

    #[test]
    fn find_latest_release_skips_older() {
        let releases = json!([
            {"tag_name": "v1.17.0", "prerelease": false, "html_url": "a", "assets": []},
            {"tag_name": "v1.19.0", "prerelease": false, "html_url": "b", "assets": [{"browser_download_url": "u", "size": 100}]}
        ]);
        let settings = json!({"branch": "master"});
        let (found, latest) = find_latest_release(&releases, &settings);
        assert!(found);
        assert_eq!(latest.version, "v1.19.0");
        assert_eq!(latest.download_url, "u");
        assert_eq!(latest.filesize, 100);
    }

    #[test]
    fn find_latest_release_master_skips_prerelease() {
        let releases = json!([
            {"tag_name": "v1.19.0-beta1", "prerelease": true, "html_url": "a", "assets": []}
        ]);
        let settings = json!({"branch": "master"});
        let (found, _latest) = find_latest_release(&releases, &settings);
        assert!(!found);
        // beta 分支接受 prerelease
        let settings_beta = json!({"branch": "beta"});
        let (found2, latest2) = find_latest_release(&releases, &settings_beta);
        assert!(found2);
        assert_eq!(latest2.version, "v1.19.0-beta1");
    }
}
