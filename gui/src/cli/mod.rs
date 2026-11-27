pub mod args;
pub mod attach;
mod commands;
pub mod format;
mod help;
mod poll;
mod repl;
mod runner;
pub mod slash;
mod slash_cfg;
mod slash_ext;
mod slash_ops;
mod stream;
pub mod term;

use args::{classify_invocation, InvocationMode};
use serde_json::json;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn help_text() -> String {
    help::cli_help_text(VERSION)
}

fn print_help(json_out: bool) {
    if json_out {
        let v = json!({
            "name": "Qwen27",
            "version": VERSION,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "modes": ["gui", "interactive", "one-shot", "subcommand", "legacy (--api)"],
            "usage": help_text(),
        });
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
    } else {
        println!("{}", help_text());
    }
}

fn print_version(json_out: bool) {
    if json_out {
        println!(
            "{}",
            json!({
                "name": "Qwen27",
                "version": VERSION,
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
            })
        );
    } else {
        println!("Qwen27 {} ({}/{})", VERSION, std::env::consts::OS, std::env::consts::ARCH);
    }
}

pub fn try_run_cli(raw_args: &[String]) -> Option<i32> {
    let stdin_kind = term::detect_stdin_kind();
    let has_display = term::has_gui_display();
    let mut inv = match classify_invocation(raw_args, stdin_kind, has_display) {
        Ok(i) => i,
        Err(msg) => {
            let session = term::ConsoleSession::attach(false);
            eprintln!("エラー: {}", msg);
            drop(session);
            return Some(1);
        }
    };
    if inv.mode == InvocationMode::Gui {
        return None;
    }

    let session = term::ConsoleSession::attach(inv.mode == InvocationMode::Interactive);
    let root = crate::config::config_root_dir();

    let res = match inv.mode.clone() {
        InvocationMode::Help => {
            print_help(inv.cli.json_out);
            Ok(0)
        }
        InvocationMode::Version => {
            print_version(inv.cli.json_out);
            Ok(0)
        }
        InvocationMode::Interactive => repl::run_interactive(&mut inv, &root),
        InvocationMode::OneShotRun => runner::run_one_shot(&mut inv, &root),
        InvocationMode::Subcommand(cmd) => commands::run_direct_subcommand(&cmd, &mut inv, &root),
        InvocationMode::Gui => Ok(0),
    };

    let code = match res {
        Ok(c) => c,
        Err(e) => {
            if inv.cli.json_out {
                eprintln!("{}", json!({ "error": e }));
            } else {
                eprintln!("エラー: {}", e);
            }
            1
        }
    };
    drop(session);
    Some(code)
}
