use crate::api_cli::{build_job_request, connect_api_with_failover, save_job_outputs_with_workspace};
use crate::cli::args::CliInvocation;
use crate::cli::attach::load_oneshot_attachments;
use crate::cli::poll::{poll_job, PollOutcome};
use crate::cli::stream::StreamPrinter;
use crate::cli::term::{self, StdinKind};
use crate::config::CliArgs;
use crate::tls_http::ApiClient;
use crate::wording::job_end_notice;
use serde_json::{json, Value};
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

const MAX_STDIN_BYTES: usize = 4 * 1024 * 1024;

pub fn extract_final_answer(job: &Value) -> String {
    job.get("result")
        .and_then(|r| r.get("review"))
        .and_then(|rv| rv.get("final_answer"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| job.get("error").and_then(|v| v.as_str()).filter(|s| !s.is_empty()))
        .or_else(|| job.get("partialAnswer").and_then(|v| v.as_str()).filter(|s| !s.is_empty()))
        .or_else(|| job.get("status").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string()
}

pub fn job_exit_code(job: &Value) -> i32 {
    let status = job.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if status == "waiting_user" {
        return 3;
    }
    if status != "completed"
        || job.get("result").and_then(|r| r.get("achieved")).and_then(|v| v.as_bool()) == Some(false)
    {
        return 2;
    }
    0
}

pub fn submit_and_poll_job<F>(
    api: &ApiClient,
    args: &mut CliArgs,
    root: &Path,
    goal: &str,
    attached: &[Value],
    slash_cmd: Option<&str>,
    printer: &mut StreamPrinter,
    mut on_question: F,
) -> Result<Value, String>
where
    F: FnMut(&Value) -> Result<Option<String>, String>,
{
    if goal.encode_utf16().count() > 6000 {
        return Err("入力は 1〜6000 文字以内で指定してください".into());
    }
    let mut prep = build_job_request(api, args, root, goal, attached, slash_cmd)?;
    let (work_mode, job_id) = (prep.work_mode.clone(), prep.job_id.clone());
    let final_job = loop {
        match poll_job(api, &job_id, &work_mode, printer)? {
            PollOutcome::Done(job) | PollOutcome::Cancelled(job) => break job,
            PollOutcome::WaitingUser(job) => {
                let q = job.get("question").cloned().unwrap_or_else(|| json!({}));
                match on_question(&q)? {
                    Some(answer) => {
                        let qid = q.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let _ = api.post(&format!("/jobs/{}/answer", job_id), &json!({ "questionId": qid, "answer": answer }), 15000)?;
                    }
                    None => break job,
                }
            }
        }
    };
    prep.bridge.stop();
    Ok(final_job)
}

fn resolve_oneshot_goal(inv: &CliInvocation) -> Result<String, String> {
    let base = if let Some(ref p) = inv.prompt {
        p.clone()
    } else if !inv.cli.goal_file.is_empty() {
        fs::read_to_string(&inv.cli.goal_file).map_err(|_| format!("ファイルを開けません: {}", inv.cli.goal_file))?
    } else {
        inv.cli.positionals.join(" ")
    };
    let should_read_stdin = inv.stdin_kind == StdinKind::Redirected
        && (base.trim().is_empty() || term::stdin_has_available_data(50));
    let stdin_part = if should_read_stdin {
        let mut buf = Vec::new();
        io::stdin().take((MAX_STDIN_BYTES + 1) as u64).read_to_end(&mut buf).map_err(|e| format!("標準入力読込エラー: {}", e))?;
        if buf.len() > MAX_STDIN_BYTES { return Err("標準入力が大きすぎます (最大 4 MiB)".into()); }
        let s = String::from_utf8_lossy(&buf);
        s.strip_prefix('\u{FEFF}').unwrap_or(&s).trim().to_string()
    } else {
        String::new()
    };
    match (base.trim().is_empty(), stdin_part.is_empty()) {
        (false, false) => Ok(format!("{}\n\n{}", base.trim(), stdin_part)),
        (false, true) => Ok(base.trim().to_string()),
        (true, false) => Ok(stdin_part),
        (true, true) if !inv.cli.attach_files.is_empty() || inv.cli.attach_clipboard => {
            Ok("添付したファイルを確認してください。".into())
        }
        _ => Err("プロンプトを指定してください (-p <text> / --file <path> / stdin)".into()),
    }
}

pub fn run_one_shot(inv: &mut CliInvocation, root: &Path) -> Result<i32, String> {
    let _sig = term::install_sigint_handler();
    let goal = resolve_oneshot_goal(inv)?;
    let attached = load_oneshot_attachments(&inv.cli.attach_files, inv.cli.attach_clipboard)?;
    let api = connect_api_with_failover(root, &inv.cli.api_url)?;

    let json_out = inv.cli.json_out;
    let stream_on = !json_out && inv.stream.unwrap_or(true);
    let mut printer = StreamPrinter::new(stream_on, inv.quiet || json_out, term::use_ansi_stderr(inv.no_color));
    let (auto_ans, ans_txt, can_prompt) = (inv.cli.auto_answer, inv.cli.answer_text.clone(), inv.stdin_kind == StdinKind::Tty && !json_out);

    let job = submit_and_poll_job(&api, &mut inv.cli, root, &goal, &attached, None, &mut printer, |q| {
        if auto_ans { Ok(Some(ans_txt.clone())) } else if can_prompt { prompt_question_once(q) } else { Ok(None) }
    })?;
    let saved_dir = if inv.save_outputs && job.get("status").and_then(|v| v.as_str()) != Some("waiting_user") {
        let out_dir = if inv.cli.output_dir.is_empty() { root.join("downloads") } else { root.join(&inv.cli.output_dir) };
        Some(save_job_outputs_with_workspace(&api, &job, &out_dir)?.to_string_lossy().into_owned())
    } else {
        None
    };

    let (answer, code) = (extract_final_answer(&job), job_exit_code(&job));
    if json_out {
        let mut obj = json!({
            "id": job.get("id").cloned().unwrap_or(Value::Null),
            "conversationId": job.get("conversationId").cloned().unwrap_or(Value::Null),
            "status": job.get("status").cloned().unwrap_or(Value::Null),
            "achieved": job.get("result").and_then(|r| r.get("achieved")).cloned().unwrap_or(Value::Null),
            "answer": answer,
            "job": job,
        });
        if let Some(dir) = saved_dir { obj["savedDir"] = json!(dir); }
        println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
        return Ok(code);
    }

    let (mut out_w, mut err_w) = (io::stdout(), io::stderr());
    if code == 3 {
        let q = job.get("question").cloned().unwrap_or_else(|| json!({}));
        let _ = writeln!(err_w, "[回答待ち] {}\njobId={} questionId={}", q.get("text").and_then(|v| v.as_str()).unwrap_or(""), job.get("id").and_then(|v| v.as_str()).unwrap_or(""), q.get("id").and_then(|v| v.as_str()).unwrap_or(""));
        return Ok(3);
    }
    printer.finish(&answer, &mut out_w, &mut err_w);
    if code != 0 && !inv.quiet {
        let res = job.get("result").cloned().unwrap_or_else(|| json!({}));
        let ach = if res.get("achieved").and_then(|v| v.as_bool()) == Some(true) { 1 } else { 0 };
        if let Some(n) = job_end_notice(job.get("status").and_then(|v| v.as_str()).unwrap_or(""), ach, job.get("error").and_then(|v| v.as_str()).unwrap_or(""), res.get("stopReason").and_then(|v| v.as_str()).unwrap_or(""), res.get("detail").and_then(|v| v.as_str()).unwrap_or("")) {
            let _ = writeln!(err_w, "注意: {}", n.label);
        }
    }
    if let Some(dir) = saved_dir { if !inv.quiet { let _ = writeln!(err_w, "保存先: {}", dir); } }
    Ok(code)
}

pub fn prompt_question_once(q: &Value) -> Result<Option<String>, String> {
    let mut err_w = io::stderr();
    let opts: Vec<String> = q.get("options").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let _ = writeln!(err_w, "\n[質問] {}", q.get("text").and_then(|v| v.as_str()).unwrap_or("質問"));
    for (i, o) in opts.iter().enumerate() { let _ = writeln!(err_w, "  {}) {}", i + 1, o); }
    let _ = write!(err_w, "回答 (番号または自由記述)> ");
    let _ = err_w.flush();
    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(0) => Ok(None),
        Ok(_) => {
            let t = line.trim();
            if let Ok(idx) = t.parse::<usize>() { if idx >= 1 && idx <= opts.len() { return Ok(Some(opts[idx - 1].clone())); } }
            Ok(Some(t.to_string()))
        }
        Err(e) if term::is_interrupted_io(&e) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
