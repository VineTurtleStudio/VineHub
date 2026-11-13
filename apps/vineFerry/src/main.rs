#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use std::process::ExitCode;

fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("--rpc") {
        return vine_ferry::rpc::run();
    }
    if std::env::args().len() > 1
        && matches!(
            std::env::args().nth(1).as_deref(),
            Some("copy") | Some("move") | Some("-h") | Some("--help")
        )
    {
        return vine_ferry::cli::run();
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("vineFerry（藤渡）")
            .with_inner_size([1040.0, 660.0])
            .with_min_inner_size([760.0, 460.0]),
        ..Default::default()
    };
    match eframe::run_native(
        "vine-ferry",
        options,
        Box::new(|cc| Ok(Box::new(vine_ferry::ui::App::new(cc)))),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("启动界面失败：{e}");
            ExitCode::FAILURE
        }
    }
}
