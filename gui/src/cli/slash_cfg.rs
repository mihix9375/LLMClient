use crate::api_cli::{apply_settings_and_gui, save_job_outputs};
use crate::attachments::build_attachment_from_file;
use crate::cli::repl::ReplState;
use crate::cli::slash::parse_toggle;
use crate::config::{
    is_valid_effort, is_valid_model_role, is_valid_work_mode, load_gui_settings_file,
    preset_index_from_name, preset_name_from_index, preset_sampling, sampling_to_json, valid_id,
    validate_workdir, CliArgs,
};
use crate::win_util::desktop_windows;
use serde_json::{json, Value};
use std::path::Path;

pub const SLASH_HELP: &str = "\
依頼を入力すると実行します（同一セッションで会話履歴を引き継ぎ）。
/new  /clear                     新しい会話を作成
/history [N]                     最近の会話一覧を表示
/conv [ID|#番号|clear]           会話IDを表示/切替/解除
/resume <jobId>                  完了ジョブから再開
/plan <task>  /goal <task>       計画作成 / 完遂自律ループ (Direct 27B)
/grill-me <topic>  /boost <task> 設計インタビュー / 推論強化
/teamwork <task>  /browser <task> 工程分離完遂 / Web・画面調査優先
/compact [focus]                 現在の会話を要約・圧縮
/learn [list|delete <id>|<note>] 永続学習メモリの閲覧・追加・削除
/schedule [list|cancel <id>|in 10m <task>|every 1h <task>]
/mode [standard|chat|coding|pc_control]  /effort [none|low|medium|xhigh]
/role [planner|worker]  /plan [on|off]   /preset <code|balanced|creative>
/budget <N>  /context <N>  /tokens <N>   /temp /top-p /top-k /min-p /repeat-penalty ...
/attach [path|clear]             添付ファイル追加・一覧・解除
/workdir [path|clear]  /pwd  /read-dir [path|clear]
/workspace /local-cli /run-on-pc /pc [on|off]  /windows [q]  /window <name|clear>
/status [id]  /models [status|start|stop|restart|perf]  /perf
/settings  /gui-settings  /fetch <id>  /cancel [id]  /interrupt <id> <text>  /tune-apply
/multi (/end で送信)             /exit  /quit";

pub fn patch_cfg(st: &mut ReplState, gui_p: Value, srv_p: Value) -> Result<Value, String> {
    let mut a = CliArgs::default();
    a.sync_gui = st.sync_gui;
    if let Some(m) = gui_p.as_object() { a.gui_patch = m.clone(); }
    if let Some(m) = srv_p.as_object() { a.server_patch = m.clone(); }
    let merged = apply_settings_and_gui(Some(&st.api), &st.root, &mut a)?;
    st.gui = merged["gui"].clone();
    if !merged["server"].is_null() { st.server = merged["server"].clone(); }
    Ok(merged)
}

pub fn show_json(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

pub fn handle_cfg_slash(st: &mut ReplState, cmd: &str, a: &str) -> Result<(), String> {
    match cmd {
        "mode" => {
            if a.is_empty() { println!("workMode={}", st.work_mode()); }
            else if is_valid_work_mode(a) {
                let _ = patch_cfg(st, json!({ "workMode": a }), json!({}))?;
                eprintln!("動作モードを変更しました: {}", a);
            } else { return Err("使い方: /mode <standard|chat|coding|pc_control>".into()); }
        }
        "effort" => {
            if a.is_empty() { println!("reasoningEffort={}", st.server.get("reasoningEffort").and_then(|v| v.as_str()).unwrap_or("none")); }
            else if is_valid_effort(a) {
                let _ = patch_cfg(st, json!({ "reasoningEffort": a }), json!({ "reasoningEffort": a }))?;
                eprintln!("reasoningEffort={}", a);
            } else { return Err("使い方: /effort <none|low|medium|xhigh>".into()); }
        }
        "role" => {
            if !is_valid_model_role(a) { return Err("使い方: /role <planner|worker>".into()); }
            let _ = patch_cfg(st, json!({ "modelRole": a }), json!({}))?;
            eprintln!("modelRole={}", a);
        }
        "preset" => {
            let idx = preset_index_from_name(a);
            if !(0..=2).contains(&idx) { return Err("使い方: /preset <code|balanced|creative>".into()); }
            let mut sp = json!({});
            sampling_to_json(&preset_sampling(idx), &mut sp);
            let mut gp = sp.clone();
            gp["samplingPreset"] = json!(preset_name_from_index(idx));
            let _ = patch_cfg(st, gp, sp)?;
            eprintln!("preset={}", preset_name_from_index(idx));
        }
        "budget" | "context" | "tokens" => {
            let v = a.parse::<i32>().map_err(|_| "整数で指定してください".to_string())?;
            let k = match cmd { "budget" => "reasoningBudgetTokens", "context" => "contextWindow", _ => "maxTokens" };
            show_json(&patch_cfg(st, json!({ k: v }), json!({ k: v }))?);
        }
        "temp" | "top-p" | "top-k" | "min-p" | "repeat-penalty" | "presence-penalty" | "frequency-penalty" => {
            let k = match cmd {
                "temp" => "temperature", "top-p" => "top_p", "top-k" => "top_k", "min-p" => "min_p",
                "repeat-penalty" => "repeat_penalty", "presence-penalty" => "presence_penalty", _ => "frequency_penalty",
            };
            let val: Value = if k == "top_k" { json!(a.parse::<i32>().map_err(|_| "整数で指定してください")?) } else { json!(a.parse::<f64>().map_err(|_| "数値で指定してください")?) };
            show_json(&patch_cfg(st, json!({ k: val }), json!({ k: val }))?);
        }
        "attach" => {
            if a == "clear" { st.attachments.clear(); eprintln!("添付を解除しました。"); }
            else if a.is_empty() || a == "list" { println!("添付 ({}件): {:?}", st.attachments.len(), st.attachments); }
            else {
                if st.attachments.len() >= 5 { return Err("添付は最大5件までです".into()); }
                let _ = build_attachment_from_file(Path::new(a))?;
                st.attachments.push(a.to_string());
                eprintln!("添付追加 ({}件): {}", st.attachments.len(), a);
            }
        }
        "pwd" => println!("workdir={}", st.gui.get("localWorkdir").and_then(|v| v.as_str()).unwrap_or("(自動)")),
        "workdir" => {
            let v = if a.is_empty() || a == "clear" || a == "reset" { String::new() } else { validate_workdir(a)?.to_string_lossy().into_owned() };
            let _ = patch_cfg(st, json!({ "localWorkdir": v }), json!({}))?;
            eprintln!("workdir={}", if v.is_empty() { "(自動)" } else { &v });
        }
        "read-dir" => {
            let mut dirs: Vec<String> = if a == "clear" { Vec::new() } else {
                st.gui.get("readOnlyFolders").and_then(|v| v.as_array()).map(|x| x.iter().filter_map(|s| s.as_str().map(String::from)).collect()).unwrap_or_default()
            };
            if !a.is_empty() && a != "clear" {
                let wd = validate_workdir(a)?.to_string_lossy().into_owned();
                if !dirs.contains(&wd) { dirs.push(wd); }
            }
            let _ = patch_cfg(st, json!({ "readOnlyFolders": dirs }), json!({}))?;
        }
        "workspace" | "local-cli" | "run-on-pc" | "pc" => {
            let k = match cmd { "workspace" => "workspaceEnabled", "local-cli" => "localCliEnabled", "run-on-pc" => "runOnPc", _ => "pcEnabled" };
            let _ = patch_cfg(st, json!({ k: parse_toggle(a)? }), json!({}))?;
        }
        "windows" => {
            let list = desktop_windows();
            let filtered: Vec<_> = if a.is_empty() { list } else { list.into_iter().filter(|w| w.title.to_lowercase().contains(&a.to_lowercase())).collect() };
            show_json(&serde_json::to_value(filtered).unwrap_or(json!([])));
        }
        "window" => { let _ = patch_cfg(st, json!({ "pcWindowTitle": if a.is_empty() || a == "clear" { "" } else { a } }), json!({}))?; }
        "status" => {
            if !a.is_empty() { show_json(&st.api.get(&format!("/jobs/{}", a), 15000)?); }
            else {
                let h = st.api.get("/health", 8000)?;
                let m = st.api.get("/models/status", 8000).unwrap_or_else(|_| json!({}));
                let sc = st.api.get("/schedules", 8000).unwrap_or_else(|_| json!({}));
                show_json(&json!({ "url": st.api.base_url(), "workMode": st.work_mode(), "conversationId": st.conv_id, "health": h, "models": m, "schedules": sc }));
            }
        }
        "models" => {
            let empty = json!({});
            let v = match if a.is_empty() { "status" } else { a } {
                "status" => st.api.get("/models/status", 15000)?,
                "perf" => st.api.get("/models/perf", 15000)?,
                "start" => st.api.post("/models/start", &empty, 120_000)?,
                "stop" => st.api.post("/models/stop", &empty, 60_000)?,
                "restart" => st.api.post("/models/restart", &empty, 120_000)?,
                _ => return Err("使い方: /models [status|start|stop|restart|perf]".into()),
            };
            show_json(&v);
        }
        "perf" => show_json(&st.api.get("/models/perf", 15000)?),
        "gui-settings" => show_json(&load_gui_settings_file(&st.root)),
        "settings" => show_json(&json!({ "gui": st.gui, "server": st.api.get("/settings", 15000)?, "conversationId": st.conv_id, "parentJobId": st.parent_job_id })),
        "tune-apply" => show_json(&st.api.post("/tune/apply", &json!({}), 120_000)?),
        "fetch" => {
            if !valid_id(a) { return Err("使い方: /fetch <jobId>".into()); }
            let job = st.api.get(&format!("/jobs/{}", a), 15000)?;
            eprintln!("保存先: {}", save_job_outputs(&job, &st.root.join("downloads"))?.display());
        }
        "cancel" => {
            let jid = if !a.is_empty() { a } else { &st.parent_job_id };
            if !valid_id(jid) { return Err("使い方: /cancel <jobId>".into()); }
            show_json(&st.api.post(&format!("/jobs/{}/cancel", jid), &json!({}), 15000)?);
        }
        "interrupt" => {
            let Some((jid, txt)) = a.split_once(char::is_whitespace) else { return Err("使い方: /interrupt <jobId> <text>".into()); };
            if !valid_id(jid) || txt.trim().is_empty() { return Err("使い方: /interrupt <jobId> <text>".into()); }
            show_json(&st.api.post(&format!("/jobs/{}/interrupt", jid), &json!({ "text": txt.trim() }), 15000)?);
        }
        _ => return Err(format!("不明なコマンドです: /{} (/help で一覧)", cmd)),
    }
    Ok(())
}
