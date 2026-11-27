#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod api_cli;
mod app_state;
mod attachments;
mod cli;
mod config;
mod desktop_keys;
mod desktop_worker;
mod gui_window;
mod image_codec;
mod live_status;
mod local_ws;
mod standby;
mod text_edit;
mod tls_http;
mod ui;
mod win_util;
mod wording;
mod worker_panels;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 {
        let a1 = args[1].as_str();
        if a1 == "--self-test" || a1 == "--target" {
            std::process::exit(desktop_worker::run_desktop_worker(&args));
        }
        if a1 == "--diag-selftest" {
            std::process::exit(api_cli::run_diag_selftest());
        }
        if a1 == "--api" || a1 == "-api" || a1 == "api" {
            std::process::exit(api_cli::run_api_cli(&args));
        }
    }
    if let Some(code) = cli::try_run_cli(&args) {
        std::process::exit(code);
    }
    std::process::exit(gui_window::run_gui(&args));
}
