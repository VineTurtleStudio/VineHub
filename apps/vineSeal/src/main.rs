#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

#[cfg(windows)]
fn enable_utf8_console() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetConsoleOutputCP(code_page: u32) -> i32;
    }
    unsafe {
        SetConsoleOutputCP(65001);
    }
}

fn main() -> ExitCode {
    let Some(first) = std::env::args().nth(1) else {
        return vine_seal::ui::run();
    };
    if first == "--rpc" {
        return vine_seal::rpc::run();
    }
    #[cfg(windows)]
    enable_utf8_console();
    match first.as_str() {
        "copy" | "move" | "verify" | "hash" | "-h" | "--help" | "--version" => {
            vine_seal::cli::run()
        }
        other => {
            eprintln!("错误：未知命令 {other}");
            eprintln!("无参数启动图形界面；copy / move / verify / hash 走命令行；--rpc 为宿主模式");
            eprintln!("使用 --help 查看用法");
            ExitCode::from(2)
        }
    }
}
