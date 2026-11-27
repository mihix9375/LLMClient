use crate::api_cli::{
    apply_settings_and_gui, connect_api_with_failover, normalize_tune_mode, run_standby_for_args,
    save_job_outputs_with_workspace, sync_sampling_to_gui,
};
use crate::attachments::build_attachment_from_file;
use crate::cli::args::CliInvocation;
use crate::cli::format::{filter_conversations, format_human, print_file_or_list, print_json};
use crate::cli::slash::parse_schedule_spec;
use crate::config::{find_window_match, sampling_to_json, save_gui_settings_file, valid_id, CliArgs, SamplingValues};
use crate::tls_http::ApiClient;
use crate::win_util::desktop_windows;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use std::path::Path;
use std::thread;
use std::time::Duration;

fn need_id(pos: &[String], idx: usize) -> Result<String, String> {
    let s = pos.get(idx).ok_or_else(|| "有効な UUID を指定してください".to_string())?;
    if !valid_id(s) { return Err("有効な UUID を指定してください".into()); }
    Ok(s.clone())
}

fn run_share_cmd(api: &ApiClient, args: &CliArgs) -> Result<Value, String> {
    let cid = need_id(&args.positionals, 0)?;
    let sub = args.positionals.get(1).map(|s| s.as_str()).unwrap_or("");
    if args.ip.is_empty() && (sub.is_empty() || sub == "list") {
        return api.get(&format!("/conversations/{}/access", cid), 15000).or_else(|_| api.get(&format!("/conversations/{}", cid), 15000));
    }
    let is_verb = matches!(sub, "add" | "remove" | "delete" | "rm");
    let (ip_idx, mode_idx, rem) = if is_verb { (2, 3, sub != "add") } else { (1, 2, args.remove) };
    let ip = if !args.ip.is_empty() { args.ip.clone() } else { args.positionals.get(ip_idx).cloned().ok_or("使い方: share <cid> [list|add <ip>|remove <ip>]")? };
    let mode = if !args.mode_arg.is_empty() { args.mode_arg.clone() } else { args.positionals.get(mode_idx).cloned().unwrap_or_else(|| "content".into()) };
    api.post(&format!("/conversations/{}/access", cid), &json!({ "ip": ip, "mode": mode, "remove": rem }), 15000)
}

fn run_autotune_cmd(api: &ApiClient, args: &CliArgs, root: &Path, quiet: bool, json_out: bool) -> Result<(bool, Value), String> {
    let mode = normalize_tune_mode(&args.tune_mode);
    let wl = if args.task == "chat" { "chat" } else { "coding" };
    let _ = api.post("/tune/auto", &json!({ "task": wl, "workload": wl, "mode": mode }), 15000)?;
    let mut st = json!({});
    for _ in 0..180 {
        thread::sleep(Duration::from_secs(1));
        st = api.get("/tune/status", 10000)?;
        let s = st.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if !quiet && !json_out { eprintln!("[autotune] {} ({}%)", s, st.get("progressPercent").and_then(|v| v.as_i64()).unwrap_or(0)); }
        if matches!(s, "completed" | "failed" | "cancelled") { break; }
    }
    if args.apply_tune && st.get("status").and_then(|v| v.as_str()) == Some("completed") && !st.get("alreadyOptimal").and_then(|v| v.as_bool()).unwrap_or(false) {
        let ap = api.post("/tune/apply", &json!({}), 120_000)?;
        if args.sync_gui { if let Some(s) = ap.get("settings").filter(|v| v.is_object()) { sync_sampling_to_gui(root, s); } }
        st["appliedResult"] = ap;
    }
    Ok((st.get("status").and_then(|v| v.as_str()) == Some("completed"), st))
}

pub fn run_direct_subcommand(cmd: &str, inv: &mut CliInvocation, root: &Path) -> Result<i32, String> {
    let args = &mut inv.cli;
    let json_out = args.json_out;
    let show = |v: &Value| if json_out { print_json(v); } else { format_human(cmd, v); };

    if cmd == "windows" {
        let list: Vec<Value> = desktop_windows().into_iter().map(|w| serde_json::to_value(w).unwrap()).collect();
        if !args.window_query.is_empty() {
            let matched = find_window_match(&list, &args.window_query);
            let ok = matched.is_some();
            show(&json!({ "windows": list, "matched": matched }));
            return Ok(if ok { 0 } else { 1 });
        }
        show(&Value::Array(list));
        return Ok(0);
    }
    if cmd == "gui-settings" { show(&apply_settings_and_gui(None, root, args)?["gui"]); return Ok(0); }
    if cmd == "connect" {
        let raw = if !args.api_url.is_empty() { args.api_url.clone() } else { args.positionals.first().cloned().unwrap_or_default() };
        let api = connect_api_with_failover(root, &raw)?;
        let health = api.get("/health", 15000)?;
        if health.get("service").and_then(|v| v.as_str()) != Some("qwen27-framework") { return Err("接続先が Qwen27 Framework API ではありません".into()); }
        let url = api.base_url().to_string();
        if args.sync_gui { let _ = save_gui_settings_file(root, &json!({ "api": url })); }
        show(&json!({ "url": url, "health": health }));
        return Ok(0);
    }
    let api = connect_api_with_failover(root, &args.api_url)?;
    let empty = json!({});
    let val: Value = match cmd {
        "settings" | "configure" | "preset" => apply_settings_and_gui(if args.local_only { None } else { Some(&api) }, root, args)?,
        "reset-sampling" => {
            let (mut sp, mut gp) = (Value::Object(std::mem::take(&mut args.server_patch)), Value::Object(std::mem::take(&mut args.gui_patch)));
            sampling_to_json(&SamplingValues::default(), &mut sp);
            sampling_to_json(&SamplingValues::default(), &mut gp);
            gp["samplingPreset"] = json!("code");
            args.server_patch = sp.as_object().cloned().unwrap_or_default();
            args.gui_patch = gp.as_object().cloned().unwrap_or_default();
            apply_settings_and_gui(if args.local_only { None } else { Some(&api) }, root, args)?
        }
        "health" => api.get("/health", 15000)?,
        "commands" => api.get("/commands", 15000)?,
        "identity" => api.get("/identity", 15000)?,
        "profiles" => api.get("/model-profiles", 15000)?,
        "adapters" => api.get("/worker/adapters", 15000)?,
        "adapter-profiles" => api.get("/worker/adapter-profiles", 15000)?,
        "perf" => api.get("/models/perf", 15000)?,
        "tune-status" => api.get("/tune/status", 15000)?,
        "conversations" => {
            let list = api.get("/conversations", 15000)?;
            if args.history_filter.is_empty() && args.history_query.is_empty() { list }
            else if let Some(arr) = list.as_array() { Value::Array(filter_conversations(arr, &args.history_filter, &args.history_query)) }
            else { list }
        }
        "conversation" => api.get(&format!("/conversations/{}", need_id(&args.positionals, 0)?), 15000)?,
        "standby" => return run_standby_for_args(&api, args, root),
        "tune-apply" => {
            let res = api.post("/tune/apply", &empty, 120_000)?;
            if args.sync_gui { if let Some(s) = res.get("settings").filter(|v| v.is_object()) { sync_sampling_to_gui(root, s); } }
            res
        }
        "autotune" => { let (ok, st) = run_autotune_cmd(&api, args, root, inv.quiet, json_out)?; show(&st); return Ok(if ok { 0 } else { 2 }); }
        "models" => match args.positionals.first().map(|s| s.as_str()).unwrap_or("status") {
            "status" => api.get("/models/status", 15000)?, "perf" => api.get("/models/perf", 15000)?,
            "start" => api.post("/models/start", &empty, 120_000)?, "stop" => api.post("/models/stop", &empty, 60_000)?,
            "restart" => api.post("/models/restart", &empty, 120_000)?, _ => return Err("使い方: models [status|start|stop|restart|perf]".into()),
        },
        "compact" => {
            let cid = need_id(&args.positionals, 0)?;
            let focus = args.positionals.get(1..).map(|s| s.join(" ")).unwrap_or_default();
            api.post(&format!("/conversations/{}/compact", cid), &if focus.is_empty() { empty } else { json!({ "focus": focus }) }, 30_000)?
        }
        "memory" => match args.positionals.first().map(|s| s.as_str()).unwrap_or("list") {
            "list" => api.get("/memory", 15000)?,
            "add" => { let t = args.positionals.get(1..).map(|s| s.join(" ")).unwrap_or_default(); if t.trim().is_empty() { return Err("使い方: memory add <text>".into()); } api.post("/memory", &json!({ "text": t }), 15000)? }
            "delete" | "remove" => api.delete(&format!("/memory/{}", args.positionals.get(1).ok_or("使い方: memory delete <id>")?), 15000)?,
            _ => return Err("使い方: memory [list|add <text>|delete <id>]".into()),
        },
        "schedules" => match args.positionals.first().map(|s| s.as_str()).unwrap_or("list") {
            "list" => api.get("/schedules", 15000)?,
            "add" => {
                let (spec, goal) = parse_schedule_spec(&args.positionals[1..].join(" "))?;
                let mut body = json!({ "spec": spec, "goal": goal });
                if !args.conv_id.is_empty() { body["conversationId"] = json!(args.conv_id); }
                api.post("/schedules", &body, 15000)?
            }
            "delete" | "remove" | "cancel" => api.delete(&format!("/schedules/{}", args.positionals.get(1).ok_or("使い方: schedules delete <id>")?), 15000)?,
            _ => return Err("使い方: schedules [list|add <spec> <goal>|delete <id>]".into()),
        },
        "share" => run_share_cmd(&api, args)?,
        "files" => {
            let cid = need_id(&args.positionals, 0)?;
            let q = if !args.path_arg.is_empty() { args.path_arg.clone() } else { args.positionals.get(1).cloned().unwrap_or_default() };
            let p = if q.is_empty() { format!("/conversations/{}/files", cid) } else { format!("/conversations/{}/files?path={}", cid, utf8_percent_encode(&q, NON_ALPHANUMERIC)) };
            let val = api.get(&p, 15000)?;
            if !json_out && !q.is_empty() { print_file_or_list(&val, (!args.output_dir.is_empty()).then(|| Path::new(&args.output_dir)))?; return Ok(0); }
            val
        }
        "upload" => {
            let cid = need_id(&args.positionals, 0)?;
            let mut files = Vec::new();
            for p in args.positionals.iter().skip(1).chain(args.attach_files.iter()) {
                let att = build_attachment_from_file(Path::new(p))?;
                files.push(json!({ "path": att["path"], "base64": att["base64"] }));
            }
            if files.is_empty() { return Err("アップロードするファイルを指定してください".into()); }
            api.post(&format!("/conversations/{}/files", cid), &json!({ "files": files }), 60_000)?
        }
        "status" => if args.positionals.is_empty() { api.get("/jobs", 15000)? } else { api.get(&format!("/jobs/{}{}", need_id(&args.positionals, 0)?, if args.full_job { "?full=1" } else { "" }), 15000)? },
        "fetch" | "save" => {
            let job = api.get(&format!("/jobs/{}", need_id(&args.positionals, 0)?), 15000)?;
            let out_dir = if args.output_dir.is_empty() { root.join("downloads") } else { root.join(&args.output_dir) };
            json!({ "savedDir": save_job_outputs_with_workspace(&api, &job, &out_dir)?.to_string_lossy(), "job": job })
        }
        "cancel" => api.post(&format!("/jobs/{}/cancel", need_id(&args.positionals, 0)?), &empty, 15000)?,
        "answer" => {
            let jid = need_id(&args.positionals, 0)?;
            let qid = if !args.question_id.is_empty() { args.question_id.clone() } else { args.positionals.get(1).cloned().ok_or("questionId を指定してください")? };
            let ans = if !args.answer_text.is_empty() { args.answer_text.clone() } else { args.positionals.get(2..).map(|s| s.join(" ")).unwrap_or_default() };
            if ans.is_empty() { return Err("回答テキストを指定してください".into()); }
            api.post(&format!("/jobs/{}/answer", jid), &json!({ "questionId": qid, "answer": ans }), 15000)?
        }
        "interrupt" => {
            let jid = need_id(&args.positionals, 0)?;
            let txt = args.positionals.get(1..).map(|s| s.join(" ")).unwrap_or_default();
            if txt.is_empty() { return Err("追加指示テキストを指定してください".into()); }
            api.post(&format!("/jobs/{}/interrupt", jid), &json!({ "text": txt }), 15000)?
        }
        "classify" => {
            let txt = args.positionals.join(" ");
            if txt.is_empty() { return Err("分類対象テキストを指定してください".into()); }
            api.post("/classify", &json!({ "text": txt }), 120_000)?
        }
        other => return Err(format!("不明なコマンドです: {}", other)),
    };
    show(&val);
    Ok(0)
}