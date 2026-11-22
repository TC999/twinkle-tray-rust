//! 会话日志（Logger.js 等价移植）。
//!
//! equiv: origin/src/Logger.js（206 行）。
//! 主进程所有 console 输出汇入单个会话日志文件，按 MAX_KEEP=3 轮转、单文件
//! MAX_BYTES=50MB 封顶；每行格式 `[HH:MM:SS.mmm] [TAG] message\r\n`。
//!
//! 降级语义：任何一次写盘失败都只记录一次错误并继续（原版 disabled/capped 标志）。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// 保留的会话日志份数（equiv Logger.js MAX_KEEP = 3）。
pub const MAX_KEEP: usize = 3;
/// 单文件上限（equiv MAX_BYTES = 50 * 1024 * 1024）。
pub const MAX_BYTES: u64 = 50 * 1024 * 1024;

struct LoggerState {
    session_path: Option<PathBuf>,
    bytes_written: u64,
    capped: bool,
    disabled: bool,
    mirror_log: bool,
    pre_init: Vec<String>,
}

static STATE: Mutex<LoggerState> = Mutex::new(LoggerState {
    session_path: None,
    bytes_written: 0,
    capped: false,
    disabled: false,
    mirror_log: true,
    pre_init: Vec::new(),
});

/// timestamp 等价（equiv Logger.js:L37-L41）：HH:MM:SS.mmm 本地时间。
fn timestamp() -> String {
    // 无 chrono 依赖：用 SystemTime 的相对秒数换算当日时分秒（原版仅用于日志前缀）。
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let total = now.as_secs();
    let millis = now.subsec_millis();
    // 本机时区偏移（秒）：Windows 下无法轻易取，退化为 UTC 计时仍保持单调可读
    let secs_of_day = total % 86_400;
    let h = secs_of_day / 3600;
    let m = (secs_of_day % 3600) / 60;
    let s = secs_of_day % 60;
    format!("{:02}:{:02}:{:02}.{:03}", h, m, s, millis)
}

/// stripAnsi 等价（equiv L35）：去掉 ESC[...m 序列。
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // 吞掉 CSI 序列直到终止字母
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// formatLine 等价（equiv L51-L54）。
fn format_line(tag: &str, level: &str, message: &str) -> String {
    let lvl = match level {
        "warn" => ":WAR",
        "error" => ":ERR",
        _ => "",
    };
    format!("[{}] [{}{}] {}\r\n", timestamp(), tag, lvl, strip_ansi(message))
}

/// initMainLogger 等价（equiv Logger.js:L100-L140 initMainLogger）：
/// 建立会话文件、轮转旧文件、冲刷 pre-init 缓冲。
pub fn init_main_logger(dir: &Path, is_dev: bool, enabled: bool) {
    let mut state = STATE.lock().unwrap();
    state.disabled = !enabled;
    if !enabled {
        state.pre_init.clear();
        return;
    }
    if let Err(_e) = std::fs::create_dir_all(dir) {
        state.disabled = true;
        return;
    }
    // 轮转：session-<n>.log，保留 MAX_KEEP 份（equiv rotateLogs）
    rotate_logs(dir, is_dev);

    let path = dir.join(format!("session{}.log", if is_dev { "-dev" } else { "" }));
    state.session_path = Some(path.clone());
    state.bytes_written = 0;
    state.capped = false;

    let buffered: Vec<String> = state.pre_init.drain(..).collect();
    drop(state);
    for line in buffered {
        append_line(&line);
    }
}

fn rotate_logs(dir: &Path, is_dev: bool) {
    let suffix = if is_dev { "-dev" } else { "" };
    // 从最旧开始删，保留 MAX_KEEP-1 份历史
    let oldest = dir.join(format!("session-{}{}.log", MAX_KEEP - 1, suffix));
    let _ = std::fs::remove_file(oldest);
    for idx in (1..MAX_KEEP - 1).rev() {
        let from = dir.join(format!("session-{}{}.log", idx, suffix));
        let to = dir.join(format!("session-{}{}.log", idx + 1, suffix));
        if from.exists() {
            let _ = std::fs::rename(&from, &to);
        }
    }
    let current = dir.join(format!("session{}.log", suffix));
    let first = dir.join(format!("session-1{}.log", suffix));
    if current.exists() {
        let _ = std::fs::rename(&current, &first);
    }
}

/// appendLine 等价（equiv L56-L80）：未初始化时进缓冲，写满上限后 capped。
fn append_line(line: &str) {
    let mut state = STATE.lock().unwrap();
    if state.capped || state.disabled {
        return;
    }
    let path = match state.session_path.clone() {
        Some(p) => p,
        None => {
            state.pre_init.push(line.to_string());
            return;
        }
    };
    let mirror = state.mirror_log;
    let bytes = line.len() as u64;
    if state.bytes_written + bytes > MAX_BYTES {
        state.capped = true;
        drop(state);
        if mirror {
            print!("{}[CAP] session log reached its size limit\r\n", "");
        }
        return;
    }
    state.bytes_written += bytes;
    drop(state);

    match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut f) => {
            let _ = f.write_all(line.as_bytes());
        }
        Err(_) => {
            let mut state = STATE.lock().unwrap();
            state.disabled = true;
        }
    }
    if mirror {
        print!("{}", line);
    }
}

/// logAs 等价（equiv Logger.js:L150-L170）：带 tag/level 写一行。
pub fn log_as(tag: &str, level: &str, message: &str) {
    let line = format_line(tag, level, message);
    append_line(&line);
}

/// setMirrorLog 等价（equiv Logger.js patchConsole 的 mirrorLog）：
/// 非 dev 且无 --console 时只收集到文件、不输出到 stdout。
pub fn set_mirror_log(enabled: bool) {
    STATE.lock().unwrap().mirror_log = enabled;
}

/// 当前会话日志路径（equiv getSessionLogPath）。
pub fn session_log_path() -> Option<PathBuf> {
    STATE.lock().unwrap().session_path.clone()
}

/// 日志是否启用（equiv isLoggingEnabled）。
pub fn is_logging_enabled() -> bool {
    !STATE.lock().unwrap().disabled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_color_codes() {
        assert_eq!(strip_ansi("\u{1b}[41mERR\u{1b}[0m ok"), "ERR ok");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn format_line_matches_js_shape() {
        let line = format_line("MAIN", "error", "boom");
        assert!(line.starts_with('['));
        assert!(line.contains("[MAIN:ERR] boom"));
        assert!(line.ends_with("\r\n"));
        let warn = format_line("X", "warn", "w");
        assert!(warn.contains("[X:WAR] w"));
        let info = format_line("X", "info", "i");
        assert!(info.contains("[X] i"));
    }

    #[test]
    fn rotation_keeps_three_files() {
        let dir = std::env::temp_dir().join(format!("tt-logger-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..5 {
            init_main_logger(&dir, false, true);
            log_as("TEST", "info", &format!("line {}", i));
        }
        // session.log + session-1.log + session-2.log（MAX_KEEP=3）
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert!(names.len() <= MAX_KEEP, "kept {} files: {:?}", names.len(), names);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
