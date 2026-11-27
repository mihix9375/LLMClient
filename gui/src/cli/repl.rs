use crate::api_cli::{apply_settings_and_gui, connect_api_with_failover};
use crate::cli::args::CliInvocation;
use crate::cli::attach::{attachments_to_request_files, push_clipboard_attachments, push_file_attachment};
use crate::cli::runner::{extract_final_answer, prompt_question_once, submit_and_poll_job};
use crate::cli::slash::{handle_slash, SlashAction};
use crate::cli::stream::StreamPrinter;
use crate::cli::term;
use crate::config::{load_gui_settings_file, CliArgs};
use crate::standby::StandbyController;
use crate::tls_http::ApiClient;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct ReplState {
    pub root: PathBuf,
    pub api: Option<Arc<ApiClient>>,
    pub standby: StandbyController,
    pub gui: Value,
    pub server: Value,
    pub conv_id: String,
    pub parent_job_id: String,
    pub last_job_id: String,
    pub attachments: Vec<Value>,
    pub recent_convs: Vec<(String, String)>,
    pub recent_windows: Vec<Value>,
    pub recent_shares: Vec<(String, String)>,
    pub shared_desktop: bool,
    pub sync_gui: bool,
    pub stream: bool,
    pub quiet: bool,
    pub ansi: bool,
    pub last_goal: String,
    pub last_files: Vec<Value>,
    pub last_slash: Option<String>,
    pub last_answer: String,
}

impl ReplState {
    pub fn work_mode(&self) -> &str {
        self.gui.get("workMode").and_then(|v| v.as_str()).unwrap_or("standard")
    }

    pub fn require_api(&self) -> Result<&ApiClient, String> {
        self.api.as_deref().ok_or_else(|| "サーバーへ未接続です。/connect <URL> で接続してください".into())
    }

    pub fn api_arc(&self) -> Result<Arc<ApiClient>, String> {
        self.api.as_ref().map(Arc::clone).ok_or_else(|| "サーバーへ未接続です。/connect <URL> で接続してください".into())
    }

    pub fn build_cli_args(&self) -> CliArgs {
        let mut c = CliArgs::default();
        c.sync_gui = self.sync_gui;
        c.conv_id = self.conv_id.clone();
        c.parent_id = self.parent_job_id.clone();
        c.shared_desktop = self.shared_desktop;
        if let Some(obj) = self.gui.as_object() {
            for (k, v) in obj { c.gui_patch.insert(k.clone(), v.clone()); }
        }
        c
    }
}

pub enum ReadOutcome { Line(String), Interrupted, Eof }
pub struct LineReader;
impl LineReader {
    pub fn new() -> Self { Self }
    pub fn read_prompt(&mut self, prompt: &str) -> Result<ReadOutcome, String> {
        let before = term::sigint_count();
        let mut err_w = io::stderr();
        let _ = write!(err_w, "{}", prompt);
        let _ = err_w.flush();
        let mut buf = String::new();
        match io::stdin().lock().read_line(&mut buf) {
            Ok(0) => Ok(if term::sigint_count() != before { ReadOutcome::Interrupted } else { ReadOutcome::Eof }),
            Ok(_) => {
                if term::sigint_count() != before && buf.trim().is_empty() { return Ok(ReadOutcome::Interrupted); }
                while buf.ends_with('\n') || buf.ends_with('\r') { buf.pop(); }
                Ok(ReadOutcome::Line(buf.trim_start_matches('\u{FEFF}').to_string()))
            }
            Err(e) if term::is_interrupted_io(&e) || term::sigint_count() != before => Ok(ReadOutcome::Interrupted),
            Err(e) => Err(format!("入力読込エラー: {}", e)),
        }
    }
}

fn run_turn(st: &mut ReplState, goal: &str, slash_cmd: Option<&str>, override_files: Option<Vec<Value>>) -> Result<(), String> {
    let api = st.api_arc()?;
    let files_val = match override_files { Some(f) => f, None => std::mem::take(&mut st.attachments) };
    let req_files = attachments_to_request_files(&files_val);
    st.last_goal = goal.to_string();
    st.last_files = files_val;
    st.last_slash = slash_cmd.map(|s| s.to_string());

    let mut cli_args = st.build_cli_args();
    let mut printer = StreamPrinter::new(st.stream, st.quiet, st.ansi);
    let job = submit_and_poll_job(&api, &mut cli_args, &st.root, goal, &req_files, slash_cmd, &mut printer, prompt_question_once)?;
    let answer = extract_final_answer(&job);
    let (mut out_w, mut err_w) = (io::stdout(), io::stderr());
    printer.finish(&answer, &mut out_w, &mut err_w);
    if !answer.is_empty() { st.last_answer = answer; }
    if let Some(cid) = job.get("conversationId").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        st.conv_id = cid.to_string();
    }
    if let Some(jid) = job.get("id").and_then(|v| v.as_str()) {
        st.last_job_id = jid.to_string();
        if job.get("status").and_then(|v| v.as_str()) == Some("completed") && job.get("result").is_some() {
            st.parent_job_id = jid.to_string();
        }
        let jst = job.get("status").and_then(|v| v.as_str()).unwrap_or("");
        st.standby.update_sync_cursor(&st.conv_id, jid, jst, false);
    }
    printer.print_footer(&job, &mut err_w);
    Ok(())
}

pub fn run_interactive(inv: &mut CliInvocation, root: &Path) -> Result<i32, String> {
    let _sig = term::install_sigint_handler();
    let mut gui = load_gui_settings_file(root);
    let (api, server) = match connect_api_with_failover(root, &inv.cli.api_url) {
        Ok(client) => {
            let srv = if !inv.cli.server_patch.is_empty() || !inv.cli.gui_patch.is_empty() {
                let merged = apply_settings_and_gui(Some(&client), root, &mut inv.cli)?;
                gui = merged["gui"].clone();
                merged["server"].clone()
            } else { client.get("/settings", 15000).unwrap_or_else(|_| json!({})) };
            (Some(Arc::new(client)), srv)
        }
        Err(e) => {
            eprintln!("警告: {}\n(/connect <URL> で後から接続できます)", e);
            if !inv.cli.gui_patch.is_empty() {
                gui = apply_settings_and_gui(None, root, &mut inv.cli)?["gui"].clone();
            }
            (None, json!({}))
        }
    };
    let mut init_att = Vec::new();
    for p in &inv.cli.attach_files { push_file_attachment(&mut init_att, Path::new(p))?; }
    if inv.cli.attach_clipboard { let _ = push_clipboard_attachments(&mut init_att)?; }

    let mut st = ReplState {
        root: root.to_path_buf(), api, standby: StandbyController::new(), gui, server,
        conv_id: inv.cli.conv_id.clone(), parent_job_id: inv.cli.parent_id.clone(), last_job_id: String::new(),
        attachments: init_att, recent_convs: Vec::new(), recent_windows: Vec::new(), recent_shares: Vec::new(),
        shared_desktop: inv.cli.shared_desktop, sync_gui: inv.cli.sync_gui, stream: inv.stream.unwrap_or(true),
        quiet: inv.quiet, ansi: term::use_ansi_stderr(inv.no_color),
        last_goal: String::new(), last_files: Vec::new(), last_slash: None, last_answer: String::new(),
    };
    let url_lbl = st.api.as_ref().map(|a| a.base_url()).unwrap_or("(未接続)");
    let eff = st.server.get("reasoningEffort").and_then(|v| v.as_str()).unwrap_or("none");
    let ctx = st.server.get("contextWindow").and_then(|v| v.as_i64()).unwrap_or(16384);
    let tok = st.server.get("maxTokens").and_then(|v| v.as_i64()).unwrap_or(1800);
    eprintln!("Qwen27 CLI v{} — {}", env!("CARGO_PKG_VERSION"), url_lbl);
    eprintln!("mode={}  effort={}  ctx={}  tokens={}  (/help で一覧, /exit で終了)", st.work_mode(), eff, ctx, tok);

    let (mut reader, mut multiline): (LineReader, Option<Vec<String>>) = (LineReader::new(), None);
    while !term::is_exit_signal() {
        let mut tag = if st.api.is_none() { "未接続".to_string() } else { st.work_mode().to_string() };
        if st.standby.is_active() { tag.push_str(" 待機中"); }
        if !st.attachments.is_empty() { tag.push_str(&format!(" +{}添付", st.attachments.len())); }
        let prompt = if multiline.is_some() { "...> ".into() } else { format!("[{}] > ", tag) };
        let raw = match reader.read_prompt(&prompt)? {
            ReadOutcome::Eof => break,
            ReadOutcome::Interrupted => {
                if multiline.take().is_some() { eprintln!("\n複数行入力をキャンセルしました。"); continue; }
                if term::should_exit_on_idle_sigint() { eprintln!(); break; }
                eprintln!("\n(終了するにはもう一度 Ctrl+C、または /exit)"); continue;
            }
            ReadOutcome::Line(s) => { term::reset_idle_sigint(); s }
        };
        let trimmed = raw.trim_start_matches('\u{FEFF}').trim();
        let text = if let Some(ref mut buf) = multiline {
            if trimmed == "/end" { let j = buf.join("\n").trim().to_string(); multiline = None; j }
            else if trimmed == "/cancel" { multiline = None; eprintln!("複数行入力をキャンセルしました。"); continue; }
            else { buf.push(raw); continue; }
        } else if trimmed == "/multi" {
            multiline = Some(Vec::new()); eprintln!("複数行モード (/end で送信, /cancel で中止)"); continue;
        } else { trimmed.to_string() };
        if text.is_empty() { continue; }
        if text.starts_with('/') {
            match handle_slash(&mut st, &text) {
                Ok(SlashAction::Exit) => break,
                Ok(SlashAction::Handled) => {}
                Ok(SlashAction::RunTask { goal, slash_cmd, files }) => {
                    if let Err(e) = run_turn(&mut st, &goal, slash_cmd.as_deref(), files) { eprintln!("エラー: {}", e); }
                }
                Err(e) => eprintln!("エラー: {}", e),
            }
        } else if let Err(e) = run_turn(&mut st, &text, None, None) { eprintln!("エラー: {}", e); }
    }
    st.standby.stop();
    eprintln!("対話を終了しました。サーバーのサービスは起動したままです。");
    Ok(0)
}