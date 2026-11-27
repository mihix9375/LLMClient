use crate::api_cli::{apply_settings_and_gui, connect_api_with_failover, run_standby_for_args, save_job_outputs};
use crate::attachments::build_attachment_from_file;
use crate::cli::args::CliInvocation;
use crate::config::{
    find_window_match, infer_preset_index, preset_name_from_index, sampling_from_json,
    sampling_to_json, save_gui_settings_file, valid_id, SamplingValues,
};
use crate::win_util::desktop_windows;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use std::path::Path;
use std::thread;
use std::time::Duration;

fn print_json(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

fn format_human(cmd: &str, v: &Value) {
    match cmd {
        "health" => println!(
            "status={} service={} apiVersion={}",
            if v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false) { "ok" } else { "error" },
            v.get("service").and_then(|x| x.as_str()).unwrap_or("unknown"),
            v.get("apiVersion").and_then(|x| x.as_i64()).unwrap_or(0)
        ),
        "perf" => {
            let tps = v.get("tokensPerSec").or_else(|| v.get("lastTokensPerSec")).and_then(|x| x.as_f64());
            let pp = v.get("promptTokensPerSec").and_then(|x| x.as_f64());
            if tps.is_some() || pp.is_some() { println!("gen={:.1} tok/s  prompt={:.1} tok/s", tps.unwrap_or(0.0), pp.unwrap_or(0.0)); }
            else { print_json(v); }
        }
        "conversations" => match v.as_array() {
            Some(arr) if arr.is_empty() => println!("(会話履歴はありません)"),
            Some(arr) => for (i, c) in arr.iter().rev().take(20).enumerate() {
                println!("{:>2}) {}  {}", i + 1, c.get("id").and_then(|x| x.as_str()).unwrap_or(""), c.get("title").and_then(|x| x.as_str()).unwrap_or("(無題)"));
            },
            None => print_json(v),
        },
        _ => print_json(v),
    }
}

fn need_id(pos: &[String], idx: usize) -> Result<String, String> {
    let s = pos.get(idx).ok_or_else(|| "有効な UUID を指定してください".to_string())?;
    if !valid_id(s) { return Err("有効な UUID を指定してください".into()); }
    Ok(s.clone())
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

    let api = connect_api_with_failover(root, &args.api_url)?;
    let empty = json!({});

    let val: Value = match cmd {
        "settings" | "configure" | "preset" => apply_settings_and_gui(if args.local_only { None } else { Some(&api) }, root, args)?,
        "health" => api.get("/health", 15000)?,
        "commands" => api.get("/commands", 15000)?,
        "identity" => api.get("/identity", 15000)?,
        "profiles" => api.get("/model-profiles", 15000)?,
        "adapters" => api.get("/worker/adapters", 15000)?,
        "adapter-profiles" => api.get("/worker/adapter-profiles", 15000)?,
        "perf" => api.get("/models/perf", 15000)?,
        "tune-status" => api.get("/tune/status", 15000)?,
        "conversations" => api.get("/conversations", 15000)?,
        "conversation" => api.get(&format!("/conversations/{}", need_id(&args.positionals, 0)?), 15000)?,
        "standby" => return run_standby_for_args(&api, args, root),
        "tune-apply" => {
            let res = api.post("/tune/apply", &empty, 120_000)?;
            if args.sync_gui {
                if let Some(s_obj) = res.get("settings").filter(|v| v.is_object()) {
                    let mut s = SamplingValues::default();
                    sampling_from_json(s_obj, &mut s);
                    let mut patch = json!({ "samplingPreset": preset_name_from_index(infer_preset_index(&s)) });
                    sampling_to_json(&s, &mut patch);
                    let _ = save_gui_settings_file(root, &patch);
                }
            }
            res
        }
        "autotune" => {
            let _ = api.post("/tune/auto", &json!({ "task": args.task, "mode": args.tune_mode }), 15000)?;
            let mut st = json!({});
            for _ in 0..180 {
                thread::sleep(Duration::from_secs(1));
                st = api.get("/tune/status", 10000)?;
                let s = st.get("status").and_then(|v| v.as_str()).unwrap_or("");
                if !inv.quiet && !json_out { eprintln!("[autotune] {} ({}%)", s, st.get("progressPercent").and_then(|v| v.as_i64()).unwrap_or(0)); }
                if matches!(s, "completed" | "failed" | "cancelled") { break; }
            }
            if args.apply_tune && st.get("status").and_then(|v| v.as_str()) == Some("completed") && !st.get("alreadyOptimal").and_then(|v| v.as_bool()).unwrap_or(false) {
                st["appliedResult"] = api.post("/tune/apply", &empty, 120_000)?;
            }
            let ok = st.get("status").and_then(|v| v.as_str()) == Some("completed");
            show(&st);
            return Ok(if ok { 0 } else { 2 });
        }
        "models" => match args.positionals.first().map(|s| s.as_str()).unwrap_or("status") {
            "status" => api.get("/models/status", 15000)?,
            "perf" => api.get("/models/perf", 15000)?,
            "start" => api.post("/models/start", &empty, 120_000)?,
            "stop" => api.post("/models/stop", &empty, 60_000)?,
            "restart" => api.post("/models/restart", &empty, 120_000)?,
            _ => return Err("使い方: models [status|start|stop|restart|perf]".into()),
        },
        "compact" => {
            let cid = need_id(&args.positionals, 0)?;
            let focus = args.positionals.get(1..).map(|s| s.join(" ")).unwrap_or_default();
            api.post(&format!("/conversations/{}/compact", cid), &if focus.is_empty() { empty } else { json!({ "focus": focus }) }, 30_000)?
        }
        "memory" => match args.positionals.first().map(|s| s.as_str()).unwrap_or("list") {
            "list" => api.get("/memory", 15000)?,
            "add" => {
                let txt = args.positionals.get(1..).map(|s| s.join(" ")).unwrap_or_default();
                if txt.trim().is_empty() { return Err("使い方: memory add <text>".into()); }
                api.post("/memory", &json!({ "text": txt }), 15000)?
            }
            "delete" | "remove" => api.delete(&format!("/memory/{}", args.positionals.get(1).ok_or("使い方: memory delete <id>")?), 15000)?,
            _ => return Err("使い方: memory [list|add <text>|delete <id>]".into()),
        },
        "schedules" => match args.positionals.first().map(|s| s.as_str()).unwrap_or("list") {
            "list" => api.get("/schedules", 15000)?,
            "add" => {
                if args.positionals.len() < 3 { return Err("使い方: schedules add \"<in 10m|every 1h>\" <goal>".into()); }
                let mut body = json!({ "spec": args.positionals[1], "goal": args.positionals[2..].join(" ") });
                if !args.conv_id.is_empty() { body["conversationId"] = json!(args.conv_id); }
                api.post("/schedules", &body, 15000)?
            }
            "delete" | "remove" | "cancel" => api.delete(&format!("/schedules/{}", args.positionals.get(1).ok_or("使い方: schedules delete <id>")?), 15000)?,
            _ => return Err("使い方: schedules [list|add <spec> <goal>|delete <id>]".into()),
        },
        "share" => {
            let cid = need_id(&args.positionals, 0)?;
            let ip = if !args.ip.is_empty() { args.ip.clone() } else { args.positionals.get(1).cloned().ok_or("使い方: share <cid> <ip>")? };
            let mode = if !args.mode_arg.is_empty() { args.mode_arg.clone() } else { args.positionals.get(2).cloned().unwrap_or_else(|| "content".into()) };
            api.post(&format!("/conversations/{}/access", cid), &json!({ "ip": ip, "mode": mode, "remove": args.remove }), 15000)?
        }
        "files" => {
            let cid = need_id(&args.positionals, 0)?;
            let q = if !args.path_arg.is_empty() { args.path_arg.clone() } else { args.positionals.get(1).cloned().unwrap_or_default() };
            let p = if q.is_empty() { format!("/conversations/{}/files", cid) } else { format!("/conversations/{}/files?path={}", cid, utf8_percent_encode(&q, NON_ALPHANUMERIC)) };
            api.get(&p, 15000)?
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
        "status" => api.get(&if args.positionals.is_empty() { "/jobs".into() } else { format!("/jobs/{}", need_id(&args.positionals, 0)?) }, 15000)?,
        "fetch" | "save" => {
            let job = api.get(&format!("/jobs/{}", need_id(&args.positionals, 0)?), 15000)?;
            let out_dir = if args.output_dir.is_empty() { root.join("downloads") } else { root.join(&args.output_dir) };
            json!({ "savedDir": save_job_outputs(&job, &out_dir)?.to_string_lossy(), "job": job })
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
