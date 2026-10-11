use std::process::ExitCode;

fn main() -> ExitCode {
    // argv 最早分流：--rpc 进 sidecar 模式，跳过一切 GUI 初始化
    if std::env::args().nth(1).as_deref() == Some("--rpc") {
        return vine_press_lib::rpc::run();
    }
    vine_press_lib::run();
    ExitCode::SUCCESS
}
