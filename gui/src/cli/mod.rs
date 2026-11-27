pub mod args;
mod commands;
mod poll;
mod repl;
mod runner;
mod slash;
mod slash_cfg;
mod stream;
pub mod term;

use args::{classify_invocation, InvocationMode};
use serde_json::json;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn help_text() -> String {
    format!(
        "Qwen27 {v} — 統合 CLI / GUI クライアント\n\n\
使い方:\n\
  Qwen27 [オプション]                       GUI 起動 (Linux ヘッドレス TTY では自動で対話 CLI)\n\
  Qwen27 --cli | --chat | -i | chat         対話ターミナルモード (REPL)\n\
  Qwen27 -p <プロンプト> [オプション]       非対話ワンショット実行 (stdout に回答本文)\n\
  Qwen27 run <プロンプト> [オプション]      非対話ワンショット実行\n\
  echo \"質問\" | Qwen27 [オプション]         標準入力パイプから実行\n\
  Qwen27 <サブコマンド> [引数] [オプション] API サブコマンド実行\n\
  Qwen27 --api <サブコマンド> ...           互換 NDJSON モード (qwen.ps1 互換)\n\n\
主なサブコマンド:\n\
  health, status [id], conversations, conversation <id>, settings, configure,\n\
  preset <code|balanced|creative>, perf, models [status|start|stop|restart|perf],\n\
  commands, memory [list|add|delete], schedules [list|add|delete], compact <cid>,\n\
  cancel <id>, answer <id> <qid> <text>, interrupt <id> <text>, files <cid>,\n\
  upload <cid> <file...>, classify <text>, fetch|save <id>, windows, standby, help\n\n\
主なオプション:\n\
  -p, --prompt <text>         実行プロンプト          --file <path>   ファイルから読み込み\n\
  --work-mode <mode>          standard|chat|coding|pc_control\n\
  --effort <level>            none|low|medium|xhigh   --url <url>     API 接続先 URL\n\
  --conv <id> / --parent <id> 会話ID / 親ジョブID     --attach <path> 添付ファイル (最大5件)\n\
  --json                      構造化 JSON 出力        --stream / --no-stream\n\
  -q, --quiet                 進捗表示を抑制          --save / --output <dir>\n\
  --no-sync-gui / --sync-gui  gui-settings.json 同期  --no-color      ANSI 装飾を無効化\n\
  -h, --help / -V, --version  ヘルプ / バージョン表示",
        v = VERSION
    )
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
