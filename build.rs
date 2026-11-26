//! 构建脚本：Slint UI 编译 + Windows exe 资源（图标 + 版本信息）。
//!
//! equiv: electron-builder build.win.icon = src/assets/logo.ico —— 原版构建把
//! logo.ico 嵌为 exe 图标与文件元数据（origin/package.json build.win.icon）。
//! 这里用 SDK rc.exe 编译动态生成的资源脚本，产出的 .res 直接作为链接输入
//! （MSVC link.exe 原生接受 .res 资源文件输入，会合并进 exe 的 .rsrc 段）。
//! rc.exe 缺失或编译失败时打印 cargo:warning 并跳过（资源只影响外观，
//! 不阻塞构建——符合"出错则降级，不是崩溃"的移植规范）。

use std::path::PathBuf;
use std::process::Command;

fn main() {
    slint_build::compile("ui/all.slint").unwrap();
    build_windows_resources();
    copy_tray_icons_to_target();
}

/// exe 资源：图标（资源 id 1，也是窗口图标的运行时降级来源）+ VERSIONINFO。
fn build_windows_resources() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // 资源脚本唯一的外部输入是图标文件，必须显式声明依赖：
    // slint-build 会打印自己那批 rerun-if-changed（ui/*.slint + assets/logo.png），
    // 而 cargo 一旦收到任何 rerun-if-changed 就只跟踪这批路径——不补这一行，
    // 改 assets/logo.ico 后 build script 不会重跑，exe 里嵌的仍是旧图标。
    println!("cargo:rerun-if-changed=assets/logo.ico");
    let Some(rc_exe) = find_rc_exe() else {
        println!("cargo:warning=rc.exe not found; exe icon/version resources skipped");
        return;
    };
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");

    // 资源脚本保持纯 ASCII（rc.exe 以活动代码页解码无 BOM 文件）；图标路径相对
    // manifest 目录，编译时 cwd=manifest_dir。版本号取自 Cargo.toml，避免双源。
    // 资源用**数字 id**（1 ICON / 1 VERSIONINFO）而非字符串名：rc.exe 默认生成
    // ANSI 资源名，只有带 BOM 的 .rc 才生成 Unicode 名，字符串名在运行期要用
    // MAKEINTRESOURCEA 命中、极易踩坑；数字 id 用 MAKEINTRESOURCEW(1) 无歧义。
    let (major, minor, patch) = match version.split('.').collect::<Vec<_>>()[..] {
        [a, b, c] => (a.to_string(), b.to_string(), c.to_string()),
        _ => ("0".into(), "1".into(), "0".into()),
    };
    let rc = format!(
        "1 ICON \"assets\\logo.ico\"\r\n\r\n1 VERSIONINFO\r\nFILEVERSION {major},{minor},{patch},0\r\nPRODUCTVERSION {major},{minor},{patch},0\r\nFILEOS 0x40004\r\nFILETYPE 0x1\r\nBEGIN\r\n  BLOCK \"StringFileInfo\"\r\n  BEGIN\r\n    BLOCK \"040904B0\"\r\n    BEGIN\r\n      VALUE \"CompanyName\", \"Xander Frangos\"\r\n      VALUE \"FileDescription\", \"Twinkle Tray\"\r\n      VALUE \"FileVersion\", \"{version}\"\r\n      VALUE \"InternalName\", \"twinkle-tray-rust\"\r\n      VALUE \"LegalTrademarks\", \"Xander Frangos\"\r\n      VALUE \"OriginalFilename\", \"twinkle-tray-rust.exe\"\r\n      VALUE \"ProductName\", \"Twinkle Tray\"\r\n      VALUE \"ProductVersion\", \"{version}\"\r\n    END\r\n  END\r\n  BLOCK \"VarFileInfo\"\r\n  BEGIN\r\n    VALUE \"Translation\", 0x409, 0x04B0\r\n  END\r\nEND\r\n"
    );
    let rc_file = out_dir.join("app.rc");
    let res_file = out_dir.join("app.res");
    std::fs::write(&rc_file, rc).expect("write app.rc");

    let status = Command::new(&rc_exe)
        .current_dir(&manifest_dir)
        .args(["/nologo", "/fo"])
        .arg(&res_file)
        .arg(&rc_file)
        .status();
    match status {
        Ok(s) if s.success() && res_file.exists() => {
            // .res 直接作为链接输入（equiv embed-resource crate 的 MSVC 路径）：
            // link.exe 原生接受 .res 资源文件输入，显式 obj/文件输入不走档案成员
            // 拉取规则，.rsrc 段必然合并进 exe。
            // （此前"先转 COFF .obj 再链接"的链路已删除：cvtres 是 ANSI 工具，
            //  工程路径含非 ASCII 字符时无法打开目标文件报 CVT1101；.res 直连
            //  由 link.exe 处理，不存在该问题，也不再需要硬编码 MSVC 安装路径。）
            println!("cargo:rustc-link-arg-bins={}", res_file.display());
        }
        Ok(s) => println!(
            "cargo:warning=rc.exe failed (exit {:?}); exe icon/version resources skipped",
            s.code()
        ),
        Err(e) => println!("cargo:warning=rc.exe spawn failed: {e}; resources skipped"),
    }
}

/// 把托盘图标资产复制到 target/<profile>/assets/tray-icons（equiv 原版
/// electron-builder 打包应用内 assets 的布局）。运行时 start_tray 按 exe
/// 目录搜索链加载图标（src/main.rs），复制后 target/release 下的 exe 自带
/// 同级 assets：本地 dev 直接运行、CI 直接打包上传，都不再需要单独布资产。
/// 复制失败只降级警告不阻塞构建（托盘搜索链仍可命中仓库 assets 兜底）。
fn copy_tray_icons_to_target() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // 显式声明资产目录依赖：同 logo.ico 的理由，改动托盘图标后要重新复制。
    println!("cargo:rerun-if-changed=assets/tray-icons");
    let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    let Ok(out_dir) = std::env::var("OUT_DIR") else {
        return;
    };
    // OUT_DIR = target/<profile>/build/<pkg>-<hash>/out，上溯三级即 target/<profile>。
    let out_path = PathBuf::from(out_dir);
    let Some(profile_dir) = out_path.ancestors().nth(3) else {
        return;
    };
    let src = PathBuf::from(manifest_dir).join("assets").join("tray-icons");
    let dst = profile_dir.join("assets").join("tray-icons");
    if !src.is_dir() {
        println!("cargo:warning=assets/tray-icons missing; tray icon assets not copied to target");
        return;
    }
    match copy_dir_recursive(&src, &dst) {
        Ok(n) if n > 0 => println!("cargo:warning=tray icon assets copied: {}", dst.display()),
        Ok(_) => {}
        Err(e) => println!("cargo:warning=copy tray icon assets failed: {e}; skipped"),
    }
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(dst)?;
    let mut count = 0usize;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            count += copy_dir_recursive(&from, &to)?;
        } else {
            // 仅在内容变化时覆盖，避免无谓的 mtime 抖动
            let needs_copy = match std::fs::metadata(&to) {
                Ok(old) => {
                    let new = entry.metadata()?;
                    old.len() != new.len() || old.modified().ok() != new.modified().ok()
                }
                Err(_) => true,
            };
            if needs_copy {
                std::fs::copy(&from, &to)?;
                count += 1;
            }
        }
    }
    Ok(count)
}

/// 定位 Windows SDK rc.exe：优先 SDK 环境变量，其次枚举默认安装位置取最新版本。
fn find_rc_exe() -> Option<PathBuf> {
    if let (Ok(dir), Ok(ver)) = (
        std::env::var("WindowsSdkDir"),
        std::env::var("WindowsSDKVersion"),
    ) {
        let p = PathBuf::from(dir).join(&ver).join("x64").join("rc.exe");
        if p.exists() {
            return Some(p);
        }
    }
    let mut best: Option<PathBuf> = None;
    let root = PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return None;
    };
    for entry in entries.flatten() {
        let p = entry.path().join("x64").join("rc.exe");
        if p.exists() && best.as_ref().map_or(true, |b| p > *b) {
            best = Some(p);
        }
    }
    best
}
