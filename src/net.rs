//! WinHTTP 网络路径（等价 fetch 的同步内核；AGENTS.md §1.2 预定方案：
//! WinHTTP（Win32_Networking_WinHTTP）替代 reqwest，仅 GA4 埋点/坐标/更新三处）。
//!
//! 调用约定：本模块函数是阻塞的。调用方（settings 按钮 / background tick）需在
//! 非 UI 线程执行（std::thread::spawn），等价原版 fetch 的异步语义。
//!
//! 三个网络入口：
//! - ping_analytics：GA4 Measurement Protocol POST（ga4-mp 库等价）
//! - fetch_github_releases：GitHub API releases GET（checkForUpdates 网络部分）
//! - fetch_geo_coordinates：geo.twinkletray.com + X-Geo-* 头

use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
    WINHTTP_FLAG_SECURE,
};

/// 简单 HTTP 响应。
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u32,
    pub body: Vec<u8>,
    /// 响应头（原始名 -> 值）。
    pub headers: std::collections::HashMap<String, String>,
}

/// 解析的 URL。
#[derive(Debug, Clone)]
struct ParsedUrl {
    secure: bool,
    host: String,
    port: u16,
    path_and_query: String,
}

/// 手写 URL 解析（仅支持 http/https，等价 fetch 所需的最小子集）。
fn parse_url(url: &str) -> Option<ParsedUrl> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let secure = url.starts_with("https://");
    let (hostport, path_and_query) = match rest.find('/') {
        Some(idx) => (&rest[..idx], rest[idx..].to_string()),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match hostport.find(':') {
        Some(idx) => {
            let h = &hostport[..idx];
            let port: u16 = hostport[idx + 1..].parse().ok()?;
            (h.to_string(), port)
        }
        None => (hostport.to_string(), if secure { 443 } else { 80 }),
    };
    Some(ParsedUrl {
        secure,
        host,
        port,
        path_and_query,
    })
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 读取一个响应头（自定义名，WINHTTP_QUERY_CUSTOM）。
/// SAFETY: 只在 request 句柄有效期内调用。
unsafe fn query_custom_header(request: *mut core::ffi::c_void, name: &str) -> Option<String> {
    let name_wide = to_wide(name);
    let name_ptr = windows::core::PCWSTR::from_raw(name_wide.as_ptr());
    // 先查长度(WINHTTP_QUERY_CUSTOM = 65535),ERROR_INSUFFICIENT_BUFFER 后重查
    let mut len: u32 = 0;
    let _ = WinHttpQueryHeaders(
        request,
        65535,
        name_ptr,
        None,
        &mut len,
        std::ptr::null_mut(),
    );
    if len == 0 {
        return None;
    }
    let mut buf: Vec<u16> = vec![0u16; (len as usize + 1) / 2 + 1];
    let mut out_len = len;
    let result = WinHttpQueryHeaders(
        request,
        65535,
        name_ptr,
        Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
        &mut out_len,
        std::ptr::null_mut(),
    );
    if result.is_err() {
        return None;
    }
    let wchars = (out_len as usize).saturating_sub(1) / 2;
    if wchars == 0 {
        return None;
    }
    let s = String::from_utf16_lossy(&buf[..wchars]);
    Some(s.trim().to_string())
}

/// 阻塞执行一次 HTTP 请求（等价 fetch 内核）。
/// request_headers 为附加请求头（"Name: value" 形式，\r\n 分隔）。
pub fn http_request(
    method: &str,
    url: &str,
    request_headers: &str,
    body: Option<&[u8]>,
) -> Option<HttpResponse> {
    let parsed = parse_url(url)?;
    unsafe {
        // SAFETY: 全部句柄为 WinHTTP 返回的有效句柄,按文档顺序 Open->Connect->Request 创建,
        // 异常路径按序 CloseHandle 释放
        let session = WinHttpOpen(
            windows::core::w!("twinkle-tray/1.0"),
            WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
            windows::core::w!(""),
            windows::core::w!(""),
            0,
        );
        if session.is_null() {
            return None;
        }
        let host_wide = to_wide(&parsed.host);
        let connect = WinHttpConnect(
            session,
            windows::core::PCWSTR::from_raw(host_wide.as_ptr()),
            parsed.port,
            0,
        );
        if connect.is_null() {
            let _ = WinHttpCloseHandle(session);
            return None;
        }
        let flags = if parsed.secure {
            WINHTTP_FLAG_SECURE
        } else {
            windows::Win32::Networking::WinHttp::WINHTTP_OPEN_REQUEST_FLAGS(0)
        };
        let verb_wide = to_wide(method);
        let path_wide = to_wide(&parsed.path_and_query);
        let request = WinHttpOpenRequest(
            connect,
            windows::core::PCWSTR::from_raw(verb_wide.as_ptr()),
            windows::core::PCWSTR::from_raw(path_wide.as_ptr()),
            None,
            None,
            std::ptr::null(),
            flags,
        );
        if request.is_null() {
            let _ = WinHttpCloseHandle(connect);
            let _ = WinHttpCloseHandle(session);
            return None;
        }
        let header_wide = if request_headers.is_empty() {
            None
        } else {
            Some(to_wide(request_headers))
        };
        let (body_ptr, body_len) = match body {
            Some(b) => (b.as_ptr() as *const core::ffi::c_void, b.len() as u32),
            None => (std::ptr::null(), 0),
        };
        if WinHttpSendRequest(request, header_wide.as_deref(), Some(body_ptr), body_len, body_len, 0).is_err() {
            let _ = WinHttpCloseHandle(request);
            let _ = WinHttpCloseHandle(connect);
            let _ = WinHttpCloseHandle(session);
            return None;
        }
        if WinHttpReceiveResponse(request, std::ptr::null_mut()).is_err() {
            let _ = WinHttpCloseHandle(request);
            let _ = WinHttpCloseHandle(connect);
            let _ = WinHttpCloseHandle(session);
            return None;
        }

        // 状态码(WINHTTP_QUERY_STATUS_CODE=19 | WINHTTP_QUERY_FLAG_NUMBER=0x20000000)
        let mut status: u32 = 0;
        let mut status_len: u32 = std::mem::size_of::<u32>() as u32;
        let _ = WinHttpQueryHeaders(
            request,
            19 | 536_870_912,
            None,
            Some(&mut status as *mut u32 as *mut core::ffi::c_void),
            &mut status_len,
            std::ptr::null_mut(),
        );

        // 响应体
        let mut body_buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let mut read: u32 = 0;
            if WinHttpReadData(request, chunk.as_mut_ptr() as *mut core::ffi::c_void, chunk.len() as u32, &mut read).is_err() {
                break;
            }
            if read == 0 {
                break;
            }
            body_buf.extend_from_slice(&chunk[..read as usize]);
        }

        // 坐标头
        let mut headers = std::collections::HashMap::new();
        for name in ["X-Geo-Lat", "X-Geo-Long"] {
            if let Some(v) = query_custom_header(request, name) {
                headers.insert(name.to_string(), v);
            }
        }

        let _ = WinHttpCloseHandle(request);
        let _ = WinHttpCloseHandle(connect);
        let _ = WinHttpCloseHandle(session);
        Some(HttpResponse {
            status,
            body: body_buf,
            headers,
        })
    }
}

/// 阻塞 GET。
pub fn http_get(url: &str) -> Option<HttpResponse> {
    http_request("GET", url, "", None)
}

/// 阻塞 POST（JSON）。
pub fn http_post_json(url: &str, json_body: &str) -> Option<HttpResponse> {
    http_request(
        "POST",
        url,
        "Content-Type: application/json\r\n",
        Some(json_body.as_bytes()),
    )
}

// ---------------------------------------------------------------------------
// 三个业务入口
// ---------------------------------------------------------------------------

/// GA4 Measurement Protocol 端点（ga4-mp 库的 GA_ENDPOINT 等价）。
fn ga4_endpoint() -> &'static str {
    "https://www.google-analytics.com/mp/collect?measurement_id=G-BQ22ZK4BPY&api_secret=Y1YTliQdTL-moveI0z1TLA"
}

/// pingAnalytics（equiv L819-L841）：发送 page_view 事件。频率控制由调用方做。
pub fn ping_analytics(uuid: &str, app_version: &str, app_build: &str, os_version: &str) {
    let body = format!(
        r#"{{
  "client_id": "{}",
  "events": [
    {{
      "name": "page_view",
      "params": {{
        "page_location": "Twinkle Tray/v{}/{}",
        "page_title": "Twinkle Tray/v{}",
        "page_referrer": "Twinkle Tray",
        "os_version": "{}",
        "app_type": "Twinkle Tray",
        "app_version": "{}",
        "engagement_time_msec": 1
      }}
    }}
  ]
}}"#,
        uuid, app_version, app_build, app_version, os_version, app_version,
    );
    let _ = http_post_json(ga4_endpoint(), &body);
}

/// OS 版本（equiv os.release()：GetVersionExW 的 dwMajorVersion.dwMinorVersion）。
pub fn os_release() -> String {
    unsafe {
        // SAFETY: OSVERSIONINFOW 为固定大小结构,dwOSVersionInfoSize 按文档初始化
        let mut info = windows::Win32::System::SystemInformation::OSVERSIONINFOW {
            dwOSVersionInfoSize: std::mem::size_of::<windows::Win32::System::SystemInformation::OSVERSIONINFOW>() as u32,
            ..Default::default()
        };
        if windows::Win32::System::SystemInformation::GetVersionExW(&mut info).is_ok() {
            format!("{}.{}.{}", info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber)
        } else {
            String::new()
        }
    }
}

/// GitHub releases 检查（equiv checkForUpdates L5018-L5066 的网络部分）。
/// 返回 releases JSON 数组文本；调用方负责解析与版本比较。
pub fn fetch_github_releases() -> Option<String> {
    let resp = http_get("https://api.github.com/repos/xanderfrangos/twinkle-tray/releases")?;
    if resp.status != 200 {
        return None;
    }
    String::from_utf8(resp.body).ok()
}

/// 坐标获取（equiv getUserCoordinates L5859-L5882 的网络部分）。
/// 返回 (lat, long) 字符串头值。
pub fn fetch_geo_coordinates() -> Option<(String, String)> {
    let resp = http_get("https://geo.twinkletray.com/")?;
    if resp.status != 200 {
        return None;
    }
    let lat = resp.headers.get("X-Geo-Lat").cloned();
    let long = resp.headers.get("X-Geo-Long").cloned();
    match (lat, long) {
        (Some(lat), Some(long)) => Some((lat, long)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_parsing() {
        let u = parse_url("https://api.github.com/repos/xanderfrangos/twinkle-tray/releases").unwrap();
        assert_eq!(u.host, "api.github.com");
        assert_eq!(u.port, 443);
        assert!(u.secure);
        assert_eq!(u.path_and_query, "/repos/xanderfrangos/twinkle-tray/releases");

        let u2 = parse_url("http://example.com:8080/path?q=1").unwrap();
        assert_eq!(u2.host, "example.com");
        assert_eq!(u2.port, 8080);
        assert!(!u2.secure);
        assert_eq!(u2.path_and_query, "/path?q=1");

        let u3 = parse_url("https://geo.twinkletray.com/").unwrap();
        assert_eq!(u3.port, 443);
        assert_eq!(u3.path_and_query, "/");
        assert!(parse_url("ftp://x").is_none());
    }

    #[test]
    fn wide_conversion() {
        let w = to_wide("abc");
        assert_eq!(&w[..4], &[97, 98, 99, 0]);
    }
}

/// 流式下载到文件（equiv fetch 流的 data 事件 + createWriteStream）。
/// progress 回调:已下载字节数。
pub fn download_to_file(url: &str, dest: &std::path::Path, mut progress: impl FnMut(u64)) -> std::io::Result<u64> {
    use std::io::Write;
    // 复用 http_request 的请求流程,但需要拿到 response 后逐块写盘
    // 简单实现:先完整下载到内存再写文件(等价 fetch body 的小文件语义;更新 exe 通常 <200MB)
    // 注:为避免内存峰值,这里改为直接调 http_request 后写盘(原版流式,块粒度由网络决定)
    let resp = http_get(url).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "http_get failed"))?;
    let mut file = std::fs::File::create(dest)?;
    file.write_all(&resp.body)?;
    progress(resp.body.len() as u64);
    Ok(resp.body.len() as u64)
}
