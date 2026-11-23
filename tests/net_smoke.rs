//! WinHTTP 网络路径真机验证（真实网络调用；常规测试集默认不跑，手动 --ignored）。
use twinkle_tray_rust::net;

#[test]
#[ignore = "真机网络测试，手动执行"]
fn net_real_requests() {
    match net::fetch_github_releases() {
        Some(json) => {
            println!("github releases OK, len={}", json.len());
            assert!(json.contains("tag_name"));
        }
        None => println!("github releases FAILED (网络不可用或超时)"),
    }
    match net::fetch_geo_coordinates() {
        Some((lat, long)) => println!("geo OK: lat={} long={}", lat, long),
        None => println!("geo FAILED (网络不可用)"),
    }
    println!("os_release: {}", net::os_release());
}
