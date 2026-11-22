//! Twinkle Tray Rust 主入口。
//!
//! 等价 electron.js 的进程模型：同二进制 `--monitor-worker` 子命令自调用
//! 替代 child_process.fork（AGENTS.md §3.1 等价替换）。

mod worker_ipc;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // fork 参数: ["--isdev=...", "--apppath=...", "--skiptest=...", "--datapath=..."]
    // 等价 electron.js:L351 fork(Monitors.js, [...])
    let is_monitor_worker = args.iter().any(|a| a == "--monitor-worker");
    if is_monitor_worker {
        worker_ipc::run_monitor_worker(&args);
        return;
    }

    // 主进程（托盘/UI）。主线程 = Slint 事件循环（AGENTS.md §5 线程模型）。
    if let Err(e) = twinkle_tray_rust::run_panel() {
        eprintln!("面板启动失败: {}", e);
    }
}
