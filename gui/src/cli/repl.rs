use crate::api_cli::{apply_settings_and_gui, connect_api_with_failover};
use crate::cli::args::CliInvocation;
use crate::cli::runner::{extract_final_answer, load_attachments, prompt_question_once, submit_and_poll_job};
use crate::cli::slash::{handle_slash, SlashAction};
use crate::cli::stream::StreamPrinter;
use crate::cli::term;
use crate::config::{load_gui_settings_file, CliArgs};
use crate::tls_http::ApiClient;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

pub struct ReplState {
    pub root: PathBuf,
    pub api: ApiClient,
    pub gui: Value,
    pub server: Value,
    pub conv_id: String,
    pub parent_job_id: String,
    pub attachments: Vec<String>,
    pub recent_convs: Vec<(String, String)>,
    pub sync_gui: bool,
    pub stream: bool,
    pub quiet: bool,
    pub ansi: bool,
}

impl ReplState {
    pub fn work_mode(&self) -> &str {
        self.gui.get("workMode").and_then(|v| v.as_str()).unwrap_or("standard")
    }

    pub fn build_cli_args(&self) -> CliArgs {
        let mut c = CliArgs::default();
        c.sync_gui = self.sync_gui;
        c.conv_id = self.conv_id.clone();
        c.parent_id = self.parent_job_id.clone();
        c.attach_files = self.attachments.clone();
        if let Some(obj) = self.gui.as_object() {
            for (k, v) in obj {
                c.gui_patch.insert(k.clone(), v.clone());
            }
        }
        c
    }
}

pub enum ReadOutcome {
    Line(String),
    Interrupted,
    Eof,
}

pub struct LineReader;

impl LineReader {
    pub fn new() -> Self {
        Self
    }

    pub fn read_prompt(&mut self, prompt: &str) -> Result<ReadOutcome, String> {
        let before = term::sigint_count();
        let mut err_w = io::stderr();
        let _ = write!(err_w, "{}", prompt);
        let _ = err_w.flush();
        let mut buf = String::new();
        match io::stdin().lock().read_line(&mut buf) {
            Ok(0) => Ok(if term::sigint_count() != before { ReadOutcome::Interrupted } else { ReadOutcome::Eof }),
            Ok(_) => {
                if term::sigint_count() != before && buf.trim().is_empty() {
                    return Ok(ReadOutcome::Interrupted);
                }
                while buf.ends_with('\n') || buf.ends_with('\r') { buf.pop(); }
                let clean = buf.trim_start_matches('\u{FEFF}').to_string();
                Ok(ReadOutcome::Line(clean))
            }
            Err(e) if term::is_interrupted_io(&e) || term::sigint_count() != before => Ok(ReadOutcome::Interrupted),
            Err(e) => Err(format!("入力読込エラー: {}", e)),
        }
    }
}

fn run_turn(st: &mut ReplState, goal: &str, slash_cmd: Option<&str>) -> Result<(), String> {
    let attached = load_attachments(&st.attachments)?;
    let mut cli_args = st.build_cli_args();
    let mut printer = StreamPrinter::new(st.stream, st.quiet, st.ansi);
    let job = submit_and_poll_job(&st.api, &mut cli_args, &st.root, goal, &attached, slash_cmd, &mut printer, prompt_question_once)?;
    st.attachments.clear();

    let (mut out_w, mut err_w) = (io::stdout(), io::stderr());
    printer.finish(&extract_final_answer(&job), &mut out_w, &mut err_w);
    if let Some(cid) = job.get("conversationId").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        st.conv_id = cid.to_string();
    }
    if job.get("status").and_then(|v| v.as_str()) == Some("completed") && job.get("result").is_some() {
        if let Some(jid) = job.get("id").and_then(|v| v.as_str()) {
            st.parent_job_id = jid.to_string();
        }
    }
    printer.print_footer(&job, &mut err_w);
    Ok(())
}

pub fn run_interactive(inv: &mut CliInvocation, root: &Path) -> Result<i32, String> {
    let _sig = term::install_sigint_handler();
    let api = connect_api_with_failover(root, &inv.cli.api_url)?;
    let mut gui = load_gui_settings_file(root);
    let server = if !inv.cli.server_patch.is_empty() || !inv.cli.gui_patch.is_empty() {
        let merged = apply_settings_and_gui(Some(&api), root, &mut inv.cli)?;
        gui = merged["gui"].clone();
        merged["server"].clone()
    } else {
        api.get("/settings", 15000).unwrap_or_else(|_| json!({}))
    };

    let mut st = ReplState {
        root: root.to_path_buf(),
        api,
        gui,
        server,
        conv_id: inv.cli.conv_id.clone(),
        parent_job_id: inv.cli.parent_id.clone(),
        attachments: inv.cli.attach_files.clone(),
        recent_convs: Vec::new(),
        sync_gui: inv.cli.sync_gui,
        stream: inv.stream.unwrap_or(true),
        quiet: inv.quiet,
        ansi: term::use_ansi_stderr(inv.no_color),
    };

    let eff = st.server.get("reasoningEffort").and_then(|v| v.as_str()).unwrap_or("none");
    let ctx = st.server.get("contextWindow").and_then(|v| v.as_i64()).unwrap_or(16384);
    let tok = st.server.get("maxTokens").and_then(|v| v.as_i64()).unwrap_or(1800);
    eprintln!("Qwen27 CLI v{} — {}", env!("CARGO_PKG_VERSION"), st.api.base_url());
    eprintln!("mode={}  effort={}  ctx={}  tokens={}  (/help で一覧, /exit で終了)", st.work_mode(), eff, ctx, tok);

    let (mut reader, mut multiline): (LineReader, Option<Vec<String>>) = (LineReader::new(), None);
    while !term::is_exit_signal() {
        let prompt = if multiline.is_some() {
            "...> ".into()
        } else if st.attachments.is_empty() {
            format!("[{}] > ", st.work_mode())
        } else {
            format!("[{} +{}添付] > ", st.work_mode(), st.attachments.len())
        };
        let raw = match reader.read_prompt(&prompt)? {
            ReadOutcome::Eof => break,
            ReadOutcome::Interrupted => {
                if multiline.take().is_some() { eprintln!("\n複数行入力をキャンセルしました。"); continue; }
                if term::should_exit_on_idle_sigint() { eprintln!(); break; }
                eprintln!("\n(終了するにはもう一度 Ctrl+C、または /exit)");
                continue;
            }
            ReadOutcome::Line(s) => { term::reset_idle_sigint(); s }
        };

        let trimmed = raw.trim_start_matches('\u{FEFF}').trim();
        let text = if let Some(ref mut buf) = multiline {
            if trimmed == "/end" {
                let joined = buf.join("\n").trim().to_string();
                multiline = None;
                joined
            } else if trimmed == "/cancel" {
                multiline = None;
                eprintln!("複数行入力をキャンセルしました。");
                continue;
            } else {
                buf.push(raw);
                continue;
            }
        } else if trimmed == "/multi" {
            multiline = Some(Vec::new());
            eprintln!("複数行モード (/end で送信, /cancel で中止)");
            continue;
        } else {
            trimmed.to_string()
        };
        if text.is_empty() { continue; }
        if text.starts_with('/') {
            match handle_slash(&mut st, &text) {
                Ok(SlashAction::Exit) => break,
                Ok(SlashAction::Handled) => {}
                Ok(SlashAction::RunTask { goal, slash_cmd }) => {
                    if let Err(e) = run_turn(&mut st, &goal, Some(&slash_cmd)) { eprintln!("エラー: {}", e); }
                }
                Err(e) => eprintln!("エラー: {}", e),
            }
        } else if let Err(e) = run_turn(&mut st, &text, None) {
            eprintln!("エラー: {}", e);
        }
    }
    eprintln!("対話を終了しました。サーバーのサービスは起動したままです。");
    Ok(0)
}
