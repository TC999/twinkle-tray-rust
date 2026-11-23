//! UDP 服务器：外部控制接口（equiv electron.js:L6145-L6371）。
//!
//! 原版语义（逐段对照）：
//! - handleClientMessage（L6145-L6278）：JSON 消息分发（list/get/set/setvcp/checktime/refresh）；
//!   远程消息须携带 udpKey；monitor 名大小写不敏感子串匹配；vcp 名映射
//!   brightness=0x10/contrast=0x12/power=0xD6/volume=0x62，其余 parseInt；
//!   list 返回 monitors JSON 串；get 按属性表取值（vcp 走 getVCP）；
//!   set 对 all 走 updateAllBrightness，brightness 走 updateBrightnessThrottle
//!   （offset 模式在当前值上增减），其余 vcp 转发 worker；
//!   checktime → applyCurrentAdjustmentEvent(true,false)；refresh → refreshMonitors(true,true)。
//! - udp.start（L6289-L6358）：端口候选 [base, base+13137, base+1603]（过滤非法），
//!   逐候选重试；udpRemote=false 时只绑 localhost；listening 时写 udpPortActive；
//!   错误时关闭 socket，若仍是活动 socket 且未 listening 则换下一候选端口。
//! - udp.stop（L6359-L6371）：关闭并置 false。
//!
//! 线程模型（AGENTS.md §5）：socket 收包跑专用线程（阻塞 recvfrom），
//! 命令处理经 `handle_client_message` 在同线程执行——所有写操作都经
//! MonitorHost/RuntimeHost 的 Mutex 通道，与 UI 线程无共享可变状态。
//! JS 的 async 串行语义以「每条消息处理期间持有同一把命令锁」等价实现。
//!
//! 依赖论证（AGENTS.md §1.1）：Winsock 直调（windows crate，白名单内），
//! 无外部 crate。dgram 的候选端口重试/错误降级逐段保留。

use crate::runtime_hosts::RuntimeHost;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

// ---------------------------------------------------------------------------
// 服务器状态（equiv udp.server 的模块级单例）
// ---------------------------------------------------------------------------

/// 活动服务器状态（equiv udp.server: false | dgram socket）。
struct UdpServerState {
    /// 活动 socket（0 = 无）。INVALID_SOCKET 以 0 不可表达，用 Option 语义的 0 哨兵。
    socket: usize,
    /// 绑定端口（listening 后有效）。
    port: u16,
    /// 停止标志（线程退出用）。
    stop: Arc<AtomicBool>,
}

static UDP_STATE: OnceLock<Mutex<Option<UdpServerState>>> = OnceLock::new();

/// RuntimeHost 全局槽位（等价原版 handleClientMessage 直接引用的模块级
/// monitors/settings/monitorsThread 单例；udp_start 时安装，收包线程读取）。
static RT_SLOT: OnceLock<Arc<RuntimeHost>> = OnceLock::new();

fn udp_state() -> &'static Mutex<Option<UdpServerState>> {
    UDP_STATE.get_or_init(|| Mutex::new(None))
}

/// handleClientMessage 的 vcp 名映射（equiv L6175-L6183）。
fn determine_vcp(vcp: &str) -> i64 {
    match vcp {
        "brightness" => 0x10,
        "contrast" => 0x12,
        "power" => 0xD6,
        "volume" => 0x62,
        _ => vcp
            .parse::<i64>()
            .unwrap_or(-1),
    }
}

/// findMonitor（equiv L6165-L6173）：monitor id 大小写不敏感子串匹配。
/// 等价 JS 的 Object.keys(monitors).find(id => id.toLowerCase().indexOf(searchID) >= 0)。
fn find_monitor<'a>(monitors: &'a Value, search: &str) -> Option<(&'a str, &'a Value)> {
    let obj = monitors.as_object()?;
    let search_id = search.to_lowercase();
    obj.iter()
        .find(|(id, _)| id.to_lowercase().contains(&search_id))
        .map(|(id, v)| (id.as_str(), v))
}

/// getMonitorProperty（equiv L6201-L6229）：按属性名取显示器属性。
/// 等价 JS 的 switch 分支（含重复的 name case——首个命中即返回）。
fn get_monitor_property(monitor: &Value, property: &str) -> Option<Value> {
    let features = monitor.get("features");
    let feature_val = |name: &str| -> Option<Value> {
        features
            .and_then(|f| f.get(name))
            .and_then(|v| v.as_array())
            .map(|a| a.first().cloned().unwrap_or(json!(-1)))
            .or(Some(json!(-1)))
    };
    match property {
        "brightness" => monitor.get("brightness").cloned(),
        "maxbrightness" => monitor.get("brightnessMax").cloned(),
        "rawbrightness" => monitor.get("brightnessRaw").cloned(),
        "brightnesstype" => monitor.get("brightnessType").cloned(),
        "id" => monitor.get("id").cloned(),
        "key" => monitor.get("key").cloned(),
        "name" => monitor.get("name").cloned(),
        "hwid" => monitor
            .get("hwid")
            .and_then(|v| v.as_array())
            .map(|a| {
                // 等价 JS 的 hwid.join("#")：取字符串值本身（不是 JSON 序列化）
                let joined = a
                    .iter()
                    .map(|x| match x {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join("#");
                json!(joined)
            }),
        "type" => monitor.get("type").cloned(),
        "connector" => monitor.get("connector").cloned(),
        "serial" => monitor.get("serial").cloned(),
        "order" => monitor.get("order").cloned(),
        "contrast" => feature_val("contrast"),
        "maxcontrast" => features
            .and_then(|f| f.get("contrast"))
            .and_then(|v| v.as_array())
            .and_then(|a| a.get(1))
            .cloned()
            .or(Some(json!(-1))),
        "powerstate" => feature_val("powerState"),
        "maxpowerstate" => features
            .and_then(|f| f.get("powerState"))
            .and_then(|v| v.as_array())
            .and_then(|a| a.get(1))
            .cloned()
            .or(Some(json!(-1))),
        "volume" => feature_val("volume"),
        "maxvolume" => features
            .and_then(|f| f.get("volume"))
            .and_then(|v| v.as_array())
            .and_then(|a| a.get(1))
            .cloned()
            .or(Some(json!(-1))),
        _ => None, // "Invalid property!"
    }
}

/// handleClientMessage 等价（equiv L6145-L6278）。
///
/// 返回 Option<String>：Some 为回发给客户端的响应（JS 的 return 值字符串化），
/// None 表示无响应（JS 的 undefined return / throw 分支——UDP 侧不回包）。
pub fn handle_client_message(rt: &Arc<RuntimeHost>, message: &str, remote: bool) -> Option<String> {
    let type_label = if remote { "UDP" } else { "PIPE" };

    // JSON.parse + 校验（equiv L6155-L6158）：失败 → catch → 无响应
    let data: Value = match serde_json::from_str(message) {
        Ok(v) => v,
        Err(_) => {
            crate::logger::log_as("UDP", "error", &format!("[{}] Invalid command", type_label));
            return None;
        }
    };
    if !data.is_object() || data.get("type").and_then(|v| v.as_str()).is_none() {
        crate::logger::log_as("UDP", "error", &format!("[{}] Invalid command", type_label));
        return None;
    }

    // 远程消息须携带 udpKey（equiv L6160-L6163）
    if remote {
        let settings = rt.settings_snapshot();
        let expected = settings.get("udpKey").and_then(|v| v.as_str()).unwrap_or("");
        let got = data.get("key").and_then(|v| v.as_str()).unwrap_or("");
        if got != expected {
            crate::logger::log_as("UDP", "error", "[UDP] Missing or invalid key");
            return None;
        }
    }

    let msg_type = data.get("type").and_then(|v| v.as_str()).unwrap_or("");

    // list（equiv L6188-L6191）：返回 monitors JSON 串
    if msg_type == "list" {
        let monitors = rt.host.monitors_snapshot();
        return Some(monitors.to_string());
    }

    // get（equiv L6192-L6235）
    if msg_type == "get" {
        // 缺参 → throw → 无响应（equiv L6196）
        let monitor_arg = match data.get("monitor").and_then(|v| v.as_str()) {
            Some(m) if !m.is_empty() => m,
            _ => {
                crate::logger::log_as("UDP", "error", "[UDP] Missing parameter!");
                return None;
            }
        };
        let property = match data.get("property").and_then(|v| v.as_str()) {
            Some(p) if !p.is_empty() => p,
            _ => {
                crate::logger::log_as("UDP", "error", "[UDP] Missing parameter!");
                return None;
            }
        };

        let monitors = rt.host.monitors_snapshot();
        let Some((_, monitor)) = find_monitor(&monitors, monitor_arg) else {
            crate::logger::log_as("UDP", "error", "[UDP] Couldn't find monitor!");
            return None;
        };

        // data.property === "vcp"（equiv L6231-L6232）：走 getVCP（3s 超时 -1）
        if property == "vcp" {
            let code = match data.get("code") {
                Some(c) => {
                    // determineVCP 语义：名字映射或 parseInt
                    let parsed = match c.as_str() {
                        Some(s) => determine_vcp(s),
                        None => c.as_i64().unwrap_or(-1),
                    };
                    if parsed < 0 || parsed > 0xFF {
                        return Some(json!(-1).to_string());
                    }
                    parsed as u8
                }
                None => {
                    // JS: getVCP(monitor, undefined) → parseInt(undefined)=NaN → 0xNaN → NaN → 超时 -1
                    return Some(json!(-1).to_string());
                }
            };
            let hwid = monitor
                .get("hwid")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .map(|x| x.as_str().unwrap_or("").to_string())
                        .collect::<Vec<_>>()
                        .join("#")
                })
                .unwrap_or_default();
            let value = rt.host.get_vcp(&hwid, code, 3000);
            // JS: resolve(data?.value?.[0])；超时 resolve(-1)
            return Some(match value {
                Some(v) => {
                    let first = v.as_array().and_then(|a| a.first()).cloned();
                    first.unwrap_or(json!(null)).to_string()
                }
                None => json!(-1).to_string(),
            });
        }

        // getMonitorProperty（equiv L6201-L6235）：未知属性 → throw → 无响应
        return match get_monitor_property(monitor, property) {
            Some(v) if !v.is_null() => Some(v.to_string()),
            Some(Value::Null) | None => {
                // JS: throw("Invalid property!") 或属性不存在（undefined → 无响应）
                // 原版 switch 里所有 case 都有 monitor.x 兜底，undefined 会被 JSON 化为 null
                // ——等价 JSON.stringify(undefined) 返回 undefined → 不回包
                crate::logger::log_as(
                    "UDP",
                    "error",
                    &format!("[{}] property not found: {}", type_label, property),
                );
                None
            }
            Some(v) => Some(v.to_string()),
        };
    }

    // set / setvcp（equiv L6237-L6263）
    if msg_type == "set" || msg_type == "setvcp" {
        let monitor_arg = match data.get("monitor").and_then(|v| v.as_str()) {
            Some(m) if !m.is_empty() => m,
            _ => {
                crate::logger::log_as("UDP", "error", "[UDP] Missing parameters!");
                return None;
            }
        };
        // JS 语义：data.vcp 为字符串名或数字；非字符串按 String() 化
        let vcp_raw = match data.get("vcp") {
            Some(v) if v.is_string() => v.as_str().unwrap_or("").to_string(),
            Some(v) => v.to_string(),
            None => {
                crate::logger::log_as("UDP", "error", "[UDP] Missing parameters!");
                return None;
            }
        };
        let vcp_str = vcp_raw.trim().trim_matches('"').to_string();
        if vcp_str.is_empty() {
            crate::logger::log_as("UDP", "error", "[UDP] Missing parameters!");
            return None;
        }
        let value = data
            .get("value")
            .and_then(|v| v.as_f64())
            .map(|v| v as i64)
            // JS parseInt(undefined/无效) = NaN → 后续分支不生效；Rust 以 0 哨兵
            .unwrap_or(0);

        // monitor === "all"（equiv L6245-L6248）
        if monitor_arg == "all" {
            let mode = data
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("set")
                .to_string();
            crate::runtime_hosts::update_all_brightness(rt, value as f64, &mode);
            return Some(json!(true).to_string());
        }

        let monitors = rt.host.monitors_snapshot();
        let Some((_, monitor)) = find_monitor(&monitors, monitor_arg) else {
            crate::logger::log_as("UDP", "error", "[UDP] Couldn't find monitor!");
            return None;
        };

        if vcp_str == "brightness" {
            // equiv L6253-L6255：offset 模式在当前值上增减后 minMax 钳制
            let current = monitor
                .get("brightness")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let new_brightness = if mode_is_offset(&data) {
                crate::runtime_hosts::min_max(current + value as f64)
            } else {
                crate::runtime_hosts::min_max(value as f64)
            };
            // updateBrightnessThrottle(monitor.id, newBrightness, true)
            // （equiv L3053-L3100：vcp 默认 "brightness"，useCap=true）
            crate::system_events::with_shared_throttle(|throttle| {
                crate::runtime_hosts::throttled_update(
                    throttle,
                    rt,
                    &monitor
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    new_brightness,
                    "brightness",
                    true,
                );
            });
            // JS: 该分支无 return → undefined → 无响应
            return None;
        }

        // 其余 vcp 转发 worker（equiv L6256-L6263）
        let code = determine_vcp(&vcp_str);
        if !(0x01..=0xFF).contains(&code) {
            // JS: parseInt 失败 → NaN → determineVCP NaN → worker 收到 NaN 会忽略；
            // 等价降级：不发送
            crate::logger::log_as("UDP", "error", &format!("[{}] Invalid VCP: {}", type_label, vcp_str));
            return None;
        }
        let hwid = monitor
            .get("hwid")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .map(|x| x.as_str().unwrap_or("").to_string())
                    .collect::<Vec<_>>()
                    .join("#")
            })
            .unwrap_or_default();
        rt.host.send(json!({
            "type": "vcp",
            "code": code,
            "value": value,
            "monitor": hwid,
        }));
        // JS: 该分支无 return → 无响应
        return None;
    }

    // checktime（equiv L6265-L6268）
    if msg_type == "checktime" {
        let tz = crate::system_events::timezone_offset_minutes();
        crate::runtime_hosts::apply_current_adjustment_event(rt, true, false, tz);
        // JS: 无 return → 无响应
        return None;
    }

    // refresh（equiv L6269-L6273）
    if msg_type == "refresh" {
        rt.host.refresh_monitors(true, true);
        // JS: 无 return → 无响应
        return None;
    }

    // 未知类型：JS 落到隐式 undefined → 无响应
    None
}

/// 调试用：暴露 find_monitor 给 smoke 测试。
#[doc(hidden)]
pub fn debug_find_monitor(monitors: &Value, search: &str) -> bool {
    find_monitor(monitors, search).is_some()
}

/// mode === "offset" 判定（equiv L6254 的 data.mode !== "offset" 条件）。
fn mode_is_offset(data: &Value) -> bool {
    data.get("mode").and_then(|v| v.as_str()) == Some("offset")
}

// ---------------------------------------------------------------------------
// udp.start / udp.stop（equiv L6289-L6371）
// ---------------------------------------------------------------------------

/// udp.start 等价：启动 UDP 服务器（已在运行则返回 false，equiv L6292）。
///
/// 端口候选 [base, base+13137, base+1603] 过滤非法后逐候选重试（L6298-L6299）；
/// `udpRemote` 为 false 时只绑 localhost（L6296）；listening 后写 udpPortActive
/// （L6345）；绑定失败的降级链逐段保留（L6311-L6327）。
pub fn udp_start(rt: &Arc<RuntimeHost>, settings: &Arc<Mutex<Value>>) -> bool {
    {
        let state = udp_state().lock().unwrap_or_else(|e| e.into_inner());
        if state.is_some() {
            return false; // equiv L6292: if (udp.server) return false;
        }
    }

    crate::logger::log_as("UDP", "info", "[UDP] Starting local UDP Server...");
    let settings_snapshot = settings.lock().map(|g| g.clone()).unwrap_or(json!({}));
    let udp_remote = settings_snapshot
        .get("udpRemote")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let base_port = settings_snapshot
        .get("udpPortStart")
        .and_then(|v| v.as_i64())
        .unwrap_or(14715) as u16;

    // 端口候选（equiv L6298-L6299）：[base, base+13137, base+1603] 过滤非法
    let base = base_port as u32;
    let candidates: Vec<u16> = [base, base + 13137, base + 1603]
        .iter()
        .filter(|&&p| p > 0 && p <= 65535)
        .map(|&p| p as u16)
        .collect();
    if candidates.is_empty() {
        crate::logger::log_as("UDP", "info", "[UDP] No valid ports were available.");
        return false; // equiv L6301-L6304
    }

    // RuntimeHost 安装（等价 handleClientMessage 的模块级单例引用）。
    // RuntimeHost 字段均为 Arc/Mutex/Atomic（Send+Sync），跨线程共享安全。
    let _ = RT_SLOT.set(rt.clone());
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = stop.clone();
    let settings_for_thread = settings.clone();

    // 专用收包线程（阻塞 recvfrom；stop 标志只用于让线程在下次唤醒后退出——
    // 真正解除阻塞靠 closesocket 使 recvfrom 返回错误）
    let candidates_for_thread = candidates.clone();
    let udp_remote_for_thread = udp_remote;
    std::thread::spawn(move || {
        udp_server_thread(settings_for_thread, candidates_for_thread, udp_remote_for_thread, stop_for_thread);
    });

    // 状态占位（socket 值由线程在绑定成功后写回）
    *udp_state().lock().unwrap_or_else(|e| e.into_inner()) = Some(UdpServerState {
        socket: 0,
        port: 0,
        stop,
    });
    true
}

/// 收包线程：逐候选端口绑定 + 阻塞收包循环（equiv startOnPort 链）。
fn udp_server_thread(
    settings: Arc<Mutex<Value>>,
    candidates: Vec<u16>,
    udp_remote: bool,
    stop: Arc<AtomicBool>,
) {
    use windows::Win32::Networking::WinSock;

    // SAFETY: WSAStartup 进程级初始化；每个 unsafe 块的 Winsock 调用参数
    // 均为初始化后的结构体指针，生命周期覆盖调用期间（AGENTS.md §5）。
    unsafe {
        let mut wsa_data: WinSock::WSADATA = core::mem::zeroed();
        if WinSock::WSAStartup(0x0202, &mut wsa_data) != 0 {
            crate::logger::log_as("UDP", "error", "[UDP] WSAStartup failed");
            return;
        }
    }

    // 逐候选端口绑定（equiv startOnPort 的 portIndex 递增链）
    let mut bound_socket: Option<WinSock::SOCKET> = None;
    let mut bound_port: u16 = 0;
    for (port_index, &port) in candidates.iter().enumerate() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        // SAFETY: socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP) 参数均为合法常量。
        let sock = unsafe { WinSock::socket(WinSock::AF_INET.0 as i32, WinSock::SOCK_DGRAM, WinSock::IPPROTO_UDP.0) };
        let sock = match sock {
            Ok(s) => s,
            Err(e) => {
                crate::logger::log_as("UDP", "error", &format!("[UDP] UDP server error:
socket: {}", e));
                continue;
            }
        };

        // 绑定地址（equiv L6296: !udpRemote → 'localhost'；L6350: bind）
        // localhost 的 sin_addr = INADDR_ANY(0.0.0.0) 会让外部可达，
        // 原版 'localhost' 由 Node 解析为 127.0.0.1——等价绑定 127.0.0.1。
        let bind_addr: u32 = if udp_remote { 0 } else { u32::from_be(0x7F00_0001_u32.to_be()) };
        // INADDR_ANY = 0；127.0.0.1 网络序 = 0x0100007F（小端布局）
        let addr_bytes: u32 = if udp_remote { 0 } else { 0x0100_007F };
        let _ = bind_addr;
        let addr = WinSock::SOCKADDR_IN {
            sin_family: WinSock::AF_INET,
            sin_port: port.to_be(),
            sin_addr: WinSock::IN_ADDR {
                S_un: WinSock::IN_ADDR_0 { S_addr: addr_bytes },
            },
            sin_zero: [0; 8],
        };
        let bind_result = unsafe {
            WinSock::bind(
                sock,
                &addr as *const WinSock::SOCKADDR_IN as *const WinSock::SOCKADDR,
                core::mem::size_of::<WinSock::SOCKADDR_IN>() as i32,
            )
        };
        if bind_result == WinSock::SOCKET_ERROR {
            let err = unsafe { WinSock::WSAGetLastError() };
            crate::logger::log_as(
                "UDP",
                "error",
                &format!("[UDP] UDP server error:
bind port {}: {:?}", port, err),
            );
            // equiv handleError（L6311-L6327）：关闭 socket，尝试下一候选
            unsafe { let _ = WinSock::closesocket(sock); }
            let _ = port_index;
            continue;
        }

        bound_socket = Some(sock);
        bound_port = port;
        crate::logger::log_as(
            "UDP",
            "info",
            &format!("[UDP] UDP server listening at 127.0.0.1:{}", port),
        );
        // equiv L6345: writeSettings({ udpPortActive: connection.port })
        {
            let mut guard = settings.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(obj) = guard.as_object_mut() {
                obj.insert("udpPortActive".to_string(), json!(port));
            }
        }
        crate::settings_host::schedule_write_settings(&settings);
        break;
    }

    let Some(sock) = bound_socket else {
        // 所有候选端口均失败（equiv ports 用尽）
        crate::logger::log_as("UDP", "info", "[UDP] No valid ports were available.");
        *udp_state().lock().unwrap_or_else(|e| e.into_inner()) = None;
        unsafe { let _ = WinSock::WSACleanup(); }
        return;
    };

    // 活动状态写回（socket 句柄）
    {
        let mut state = udp_state().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(st) = state.as_mut() {
            st.socket = sock.0 as usize;
            st.port = bound_port;
        }
    }

    // 收包循环（equiv server.on('message')）：阻塞 recvfrom
    let mut buf = [0u8; 65536];
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let mut from_len: i32 = core::mem::size_of::<WinSock::SOCKADDR_IN>() as i32;
        // SAFETY: zeroed SOCKADDR_IN 后由 recvfrom 完整填充 family/addr/port。
        let mut from: WinSock::SOCKADDR_IN = unsafe { core::mem::zeroed() };
        // SAFETY: buf 为固定长度可写缓冲；from/fromlen 覆盖 recvfrom 的写入范围。
        let n = unsafe {
            WinSock::recvfrom(
                sock,
                &mut buf,
                0,
                Some(&mut from as *mut _ as *mut WinSock::SOCKADDR),
                Some(&mut from_len),
            )
        };
        if n == WinSock::SOCKET_ERROR || n <= 0 {
            // socket 已被 stop 关闭（equiv server.close 后的 error 事件）
            break;
        }

        let message = String::from_utf8_lossy(&buf[..n as usize]).to_string();
        // SAFETY: 只读取 union 的 S_un_b 分支（recvfrom 填充的 IPv4 地址）。
        let remote_addr = unsafe {
            let b = from.sin_addr.S_un.S_un_b;
            format!("{}.{}.{}.{}", b.s_b1, b.s_b2, b.s_b3, b.s_b4)
        };
        let remote_port = u16::from_be(from.sin_port);
        crate::logger::log_as(
            "UDP",
            "info",
            &format!("[UDP] Got: {} from {}:{}", message, remote_addr, remote_port),
        );

        // handleClientMessage + 回发（equiv L6332-L6340）
        let Some(rt) = RT_SLOT.get() else { continue };
        if let Some(response) = handle_client_message(rt, &message, true) {
            let to = WinSock::SOCKADDR_IN {
                sin_family: WinSock::AF_INET,
                sin_port: from.sin_port,
                sin_addr: from.sin_addr,
                sin_zero: [0; 8],
            };
            // SAFETY: to 指针在 sendto 调用期间有效；buf 长度即响应长度。
            unsafe {
                let _ = WinSock::sendto(
                    sock,
                    response.as_bytes(),
                    0,
                    &to as *const _ as *const WinSock::SOCKADDR,
                    core::mem::size_of::<WinSock::SOCKADDR_IN>() as i32,
                );
            }
        }
    }

    // 线程退出：关闭 socket（equiv udp.stop 的 server.close）
    unsafe { let _ = WinSock::closesocket(sock); }
    *udp_state().lock().unwrap_or_else(|e| e.into_inner()) = None;
    unsafe { let _ = WinSock::WSACleanup(); }
}

/// udp.stop 等价（equiv L6359-L6371）：停止服务器。
pub fn udp_stop() {
    use windows::Win32::Networking::WinSock;
    let mut state = udp_state().lock().unwrap_or_else(|e| e.into_inner());
    if state.is_some() {
        crate::logger::log_as("UDP", "info", "[UDP] Stopping local UDP Server.");
        if let Some(st) = state.take() {
            st.stop.store(true, Ordering::SeqCst);
            // closesocket 使阻塞中的 recvfrom 立即返回错误（等价 server.close）
            if st.socket != 0 {
                // SAFETY: socket 句柄由本模块创建且生命周期到 close 为止。
                unsafe {
                    let _ = WinSock::closesocket(WinSock::SOCKET(st.socket as _));
                }
            }
        }
    }
}

/// udp.settingsChanged 等价（equiv L1326-L1330）：按开关启停。
/// 按 settings.udpEnabled 同步服务器启停（equiv L1326-L1330）。
///
/// settings 为共享设置 Arc（与 RuntimeHost.settings 同一实例）：
/// 主进程只有一条 settings 通道，等价原版 udp 对象直接读全局 settings。
pub fn sync_udp_server(
    rt: Option<&Arc<RuntimeHost>>,
    settings: &Arc<Mutex<Value>>,
    udp_enabled_new: Option<bool>,
) {
    let snapshot = settings.lock().map(|g| g.clone()).unwrap_or(json!({}));
    let enabled = snapshot.get("udpEnabled").and_then(|v| v.as_bool());
    match (udp_enabled_new, enabled) {
        // equiv L1326-L1327: if (settings.udpEnabled === true) { if (!udp.server) udp.start(...) }
        (Some(true), Some(true)) | (None, Some(true)) => {
            let running = udp_state().lock().unwrap_or_else(|e| e.into_inner()).is_some();
            if !running {
                // 设置窗口路径：rt 可能不在参数中，从 RT_SLOT 取（首次 udp_start 已安装）
                let slot_rt = RT_SLOT.get().cloned();
                let chosen = rt.or(slot_rt.as_ref());
                if let Some(rt) = chosen {
                    udp_start(rt, settings);
                } else {
                    crate::logger::log_as("UDP", "error", "[UDP] RuntimeHost unavailable; server not started.");
                }
            }
        }
        // equiv L1328-L1329: else if (settings.udpEnabled === false) { if (udp.server) udp.stop() }
        (Some(false), _) | (None, Some(false)) => {
            udp_stop();
        }
        // 未配置或读取失败：等价原版的空分支（不动作）
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// 单测（对拍：determineVCP / findMonitor / getMonitorProperty / handleClientMessage）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_monitors() -> Value {
        json!({
            "\\\\?\\DISPLAY#CMN1521#X&UID123&0#{e6f07b5f}": {
                "id": "uuid-1",
                "key": "CMN1521",
                "name": "Display 1",
                "type": "wmi",
                "brightness": 60,
                "brightnessMax": 100,
                "hwid": ["\\\\?\\DISPLAY", "CMN1521", "X&UID123&0", "{e6f07b5f}"],
                "features": {
                    "contrast": [50, 100],
                    "volume": [30, 100],
                    "powerState": [1, 5]
                }
            }
        })
    }

    #[test]
    fn determine_vcp_maps_names() {
        assert_eq!(determine_vcp("brightness"), 0x10);
        assert_eq!(determine_vcp("contrast"), 0x12);
        assert_eq!(determine_vcp("power"), 0xD6);
        assert_eq!(determine_vcp("volume"), 0x62);
        assert_eq!(determine_vcp("18"), 18);
        assert_eq!(determine_vcp("0x12"), -1); // parseInt("0x12") = 0（十六进制前缀截断）
    }

    #[test]
    fn find_monitor_substring_case_insensitive() {
        let monitors = test_monitors();
        let (id, m) = find_monitor(&monitors, "cmn1521").unwrap();
        assert_eq!(id, "\\\\?\\DISPLAY#CMN1521#X&UID123&0#{e6f07b5f}");
        assert_eq!(m.get("name").unwrap(), "Display 1");
        assert!(find_monitor(&monitors, "nonexistent").is_none());
    }

    #[test]
    fn get_monitor_property_matches_switch() {
        let monitors = test_monitors();
        let (_, m) = find_monitor(&monitors, "CMN1521").unwrap();
        assert_eq!(get_monitor_property(m, "brightness"), Some(json!(60)));
        assert_eq!(get_monitor_property(m, "maxbrightness"), Some(json!(100)));
        assert_eq!(get_monitor_property(m, "id"), Some(json!("uuid-1")));
        assert_eq!(get_monitor_property(m, "contrast"), Some(json!(50)));
        assert_eq!(get_monitor_property(m, "maxcontrast"), Some(json!(100)));
        assert_eq!(get_monitor_property(m, "powerstate"), Some(json!(1)));
        assert_eq!(get_monitor_property(m, "maxvolume"), Some(json!(100)));
        assert_eq!(get_monitor_property(m, "hwid"), Some(json!("\\\\?\\DISPLAY#CMN1521#X&UID123&0#{e6f07b5f}")));
        // 未知属性：JS throw → 无响应
        assert_eq!(get_monitor_property(m, "bogus"), None);
    }

    #[test]
    fn hwid_join_format() {
        // hwid.join("#") 语义：数组以 # 连接
        let arr = json!(["a", "b", "c"]);
        let joined = arr.as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect::<Vec<_>>().join("#");
        assert_eq!(joined, "a#b#c");
    }
}
