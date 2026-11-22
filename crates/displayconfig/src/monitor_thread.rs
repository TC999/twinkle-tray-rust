//! 监听线程启动入口。
//!
//! monitor.rs 的 start_listener_thread 引用此模块；实际线程体在 monitor.rs 内定义。
//! 此文件仅持有启动门闩，避免循环依赖。

#[allow(dead_code)]
pub fn start() {
    // 线程体在 monitor.rs::start_listener_thread 内联实现（CreateThread）。
    // 保留此模块以满足 lib.rs 的模块图；不重复启动。
}
