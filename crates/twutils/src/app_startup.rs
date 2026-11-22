//! StartupTask（AppX 开机自启）。
//!
//! equiv: windows_app_startup.cc（87 行）。仅 AppX 打包模式下有任务可取。
//! 原版 wait_for 超时语义以 status 轮询等价实现（超时返回 false）。

use windows::ApplicationModel::StartupTask;
use windows_future::AsyncStatus;

/// 等价 wait_for(timeout)：轮询 Status 直到 Completed/Error 或超时。
fn wait_with_timeout_local<T: windows::core::RuntimeType + 'static>(
    operation: &windows_future::IAsyncOperation<T>,
    timeout: std::time::Duration,
) -> bool {
    let start = std::time::Instant::now();
    loop {
        match operation.Status() {
            Ok(AsyncStatus::Completed) => return true,
            Ok(AsyncStatus::Started) => {
                if start.elapsed() >= timeout {
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            _ => return false,
        }
    }
}

/// getStartupTask 等价：取第一个 StartupTask，失败/超时返回 None。
fn get_startup_task() -> Option<StartupTask> {
    let package_async = StartupTask::GetForCurrentPackageAsync().ok()?;
    if !wait_with_timeout_local(&package_async, std::time::Duration::from_secs(1)) {
        return None;
    }
    let tasks = package_async.GetResults().ok()?;
    // 取第一个任务（原版 for task in tasks: return task）
    let size = tasks.Size().unwrap_or(0);
    (0..size).find_map(|i| tasks.GetAt(i).ok())
}

/// enable 等价：RequestEnableAsync 超时 5 秒（原注释：首次 AppX 注册可能耗时数秒）。
pub fn enable() -> bool {
    let task = match get_startup_task() {
        Some(t) => t,
        None => return false,
    };

    let enable_async = match task.RequestEnableAsync() {
        Ok(a) => a,
        Err(_) => return false,
    };
    if !wait_with_timeout_local(&enable_async, std::time::Duration::from_secs(5)) {
        return false;
    }
    matches!(
        enable_async.GetResults(),
        Ok(windows::ApplicationModel::StartupTaskState::Enabled)
    )
}

/// disable 等价。
pub fn disable() -> bool {
    let task = match get_startup_task() {
        Some(t) => t,
        None => return false,
    };
    if task.Disable().is_err() {
        return false;
    }
    task.State()
        .map(|s| s != windows::ApplicationModel::StartupTaskState::Enabled)
        .unwrap_or(false)
}
