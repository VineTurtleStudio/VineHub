#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    // argv 最早分流：--rpc 进 sidecar 模式，跳过一切 GUI 初始化
    if std::env::args().nth(1).as_deref() == Some("--rpc") {
        return vinesync_lib::rpc::run();
    }
    vinesync_lib::run();
    ExitCode::SUCCESS
}
