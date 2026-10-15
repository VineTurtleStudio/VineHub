//! Windows 构建脚本：给 release exe 嵌入图标与版本信息。
//!
//! 仅在 Windows + release 下编译资源，避免每次 debug 构建都跑 rc.exe。
//! 无 rc.exe 时仅告警并跳过（不阻断构建）。

fn main() {
    // 仅 Windows
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // debug 构建不嵌资源（rc.exe 需要额外工具链）
    let profile = std::env::var("PROFILE").unwrap_or_default();
    if profile != "release" {
        return;
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=icons/icon.ico");

    let icon = "icons/icon.ico";
    if !std::path::Path::new(icon).exists() {
        println!("cargo:warning={icon} 不存在，跳过资源嵌入");
        return;
    }

    let mut res = winresource::WindowsResource::new();

    // 本机 Windows SDK 的 rc.exe 不在 PATH 上，且注册表 Installed Roots 读不到
    // KitsRoot1，winresource 的自动探测会失败。这里显式指定一个存在的 SDK bin
    // 目录；都不存在则留给 compile() 报错（届时仅告警，不阻断构建）。
    for p in [
        r"C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64",
        r"C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x86",
    ] {
        if std::path::Path::new(p).join("rc.exe").is_file() {
            res.set_toolkit_path(p);
            break;
        }
    }

    res.set_icon(icon);

    // FileVersion / ProductVersion / ProductName / FileDescription 已由
    // winresource::new() 从 CARGO_PKG_* 自动填充，这里只补两个 cargo 没有的字段。
    res.set("OriginalFilename", "vine-pulse.exe");
    res.set(
        "LegalCopyright",
        concat!("MIT License © VineHub ", env!("CARGO_PKG_VERSION")),
    );

    if let Err(e) = res.compile() {
        println!(
            "cargo:warning=资源编译失败（rc.exe 不可用？）跳过图标/版本信息：{e}"
        );
    }
}
