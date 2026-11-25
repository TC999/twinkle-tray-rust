//! 构建脚本：Slint UI 编译 + Windows exe 资源（图标 + 版本信息）。
//!
//! equiv: electron-builder build.win.icon = src/assets/logo.ico —— 原版构建把
//! logo.ico 嵌为 exe 图标与文件元数据（origin/package.json build.win.icon）。
//! 这里用 SDK rc.exe 编译动态生成的资源脚本，以 COFF 静态库形式链接进 exe。
//! rc.exe 缺失或编译失败时打印 cargo:warning 并跳过（资源只影响外观，
//! 不阻塞构建——符合"出错则降级，不是崩溃"的移植规范）。

use std::path::PathBuf;
use std::process::Command;

fn main() {
    slint_build::compile("ui/all.slint").unwrap();
    build_windows_resources();
}

/// exe 资源：图标（资源 id 1，也是窗口图标的运行时降级来源）+ VERSIONINFO。
fn build_windows_resources() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let Some(rc_exe) = find_rc_exe() else {
        println!("cargo:warning=rc.exe not found; exe icon/version resources skipped");
        return;
    };
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");

    // 资源脚本保持纯 ASCII（rc.exe 以活动代码页解码无 BOM 文件）；图标路径相对
    // manifest 目录，编译时 cwd=manifest_dir。版本号取自 Cargo.toml，避免双源。
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
            // .res 是资源表格式而非 COFF 档案，rustc 的 native lib 解析不接受。
            // 链路：rc.exe -> .res -> cvtres 转 COFF .obj -> 直接作为链接输入。
            let Some(cvtres) = find_msvc_bin("cvtres.exe") else {
                println!("cargo:warning=cvtres.exe not found; exe icon/version resources skipped");
                return;
            };
            // cvtres 在 out_dir 内以 ASCII 相对文件名工作（工程路径含非 ASCII
            // 字符时绝对路径参数在工具链 ANSI 代码页下不可靠）。
            let obj_file = out_dir.join("app.res.obj");
            let cvtres_status = Command::new(&cvtres)
                .current_dir(&out_dir)
                .args(["/MACHINE:X64", "/NOLOGO"])
                .arg("/OUT:app.res.obj")
                .arg("app.res")
                .status();
            match cvtres_status {
                Ok(s2) if s2.success() && obj_file.exists() => {
                    // 直接把 COFF obj 传给最终二进制链接：显式 obj 输入不走档案成员
                    // 拉取规则（无符号引用也会链接），.rsrc 段必然合并进 exe。
                    // （此前 .res 复制为 .lib 的做法失败：无符号档案成员不被拉入。）
                    println!("cargo:rustc-link-arg-bins={}", obj_file.display());
                }
                Ok(s2) => println!(
                    "cargo:warning=cvtres failed (exit {:?}); exe icon/version resources skipped",
                    s2.code()
                ),
                Err(e) => println!("cargo:warning=cvtres spawn failed: {e}; resources skipped"),
            }
        }
        Ok(s) => println!(
            "cargo:warning=rc.exe failed (exit {:?}); exe icon/version resources skipped",
            s.code()
        ),
        Err(e) => println!("cargo:warning=rc.exe spawn failed: {e}; resources skipped"),
    }
}

/// 定位 MSVC 工具（cvtres.exe/lib.exe）：VCToolsInstallDir 优先，
/// 其次仓库既定的本机 MSVC 安装位置（tools/build-env.cmd 同源）。
fn find_msvc_bin(exe: &str) -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("VCToolsInstallDir") {
        let p = PathBuf::from(dir).join("bin").join("Hostx64").join("x64").join(exe);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(root) = std::env::var("MSVC") {
        let p = PathBuf::from(root).join("bin").join("Hostx64").join("x64").join(exe);
        if p.exists() {
            return Some(p);
        }
    }
    // tools/build-env.cmd 的固定安装位置（本仓库既定构建环境）。
    let p = PathBuf::from(r"D:\Software\Visual Studio\VC\Tools\MSVC\14.41.34120")
        .join("bin")
        .join("Hostx64")
        .join("x64")
        .join(exe);
    if p.exists() {
        return Some(p);
    }
    None
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
