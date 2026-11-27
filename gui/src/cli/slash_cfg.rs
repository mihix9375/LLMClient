use crate::api_cli::{apply_settings_and_gui, save_job_outputs_with_workspace};
use crate::cli::repl::ReplState;
use crate::cli::slash::parse_toggle;
use crate::config::{
    is_valid_effort, is_valid_model_role, is_valid_work_mode, load_gui_settings_file,
    preset_index_from_name, preset_name_from_index, preset_sampling, sampling_to_json, valid_id,
    validate_workdir, CliArgs,
};
use crate::win_util::{desktop_windows, is_current_process_elevated};
use serde_json::{json, Value};

pub fn patch_cfg(st: &mut ReplState, gui_p: Value, srv_p: Value) -> Result<Value, String> {
    let mut a = CliArgs::default();
    a.sync_gui = st.sync_gui;
    if let Some(m) = gui_p.as_object() { a.gui_patch = m.clone(); }
    if let Some(m) = srv_p.as_object() { a.server_patch = m.clone(); }
    let merged = apply_settings_and_gui(st.api.as_deref(), &st.root, &mut a)?;
    st.gui = merged["gui"].clone();
    if !merged["server"].is_null() { st.server = merged["server"].clone(); }
    Ok(merged)
}

pub fn show_json(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

fn active_job_id<'a>(st: &'a ReplState, arg: &'a str) -> Result<&'a str, String> {
    let jid = if !arg.is_empty() { arg } else if !st.last_job_id.is_empty() { &st.last_job_id } else { &st.parent_job_id };
    if !valid_id(jid) { return Err("有効な jobId を指定してください".into()); }
    Ok(jid)
}

pub fn handle_cfg_slash(st: &mut ReplState, cmd: &str, a: &str) -> Result<(), String> {
    match cmd {
        "mode" => {
            if a.is_empty() { println!("workMode={}", st.work_mode()); }
            else if is_valid_work_mode(a) {
                let mut patch = json!({ "workMode": a });
                if a == "coding" { patch["workspaceEnabled"] = json!(true); }
                else if a == "pc_control" { patch["workspaceEnabled"] = json!(true); patch["planMode"] = json!(false); }
                let _ = patch_cfg(st, patch, json!({}))?;
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
        "pwd" => println!("workdir={}", st.gui.get("localWorkdir").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("(自動)")),
        "workdir" | "reset" => {
            let arg = if cmd == "reset" { "" } else { a };
            let v = if arg.is_empty() || arg == "clear" || arg == "reset" { String::new() } else { validate_workdir(arg)?.to_string_lossy().into_owned() };
            let _ = patch_cfg(st, json!({ "localWorkdir": v }), json!({}))?;
            eprintln!("workdir={}", if v.is_empty() { "(自動)" } else { &v });
        }
        "read-dir" => {
            let mut dirs: Vec<String> = st.gui.get("readOnlyFolders").and_then(|v| v.as_array()).map(|x| x.iter().filter_map(|s| s.as_str().map(String::from)).collect()).unwrap_or_default();
            if a.is_empty() || a.eq_ignore_ascii_case("list") {
                if dirs.is_empty() { println!("読み取り専用フォルダーなし"); }
                else { for (i, d) in dirs.iter().enumerate() { println!("{:>2}) {}", i + 1, d); } }
                return Ok(());
            }
            if a == "clear" { dirs.clear(); }
            else if let Some(rest) = a.strip_prefix("remove ").or_else(|| a.strip_prefix("rm ")).or_else(|| a.strip_prefix("delete ")) {
                let clean = rest.trim().trim_start_matches('#');
                if let Ok(n) = clean.parse::<usize>() {
                    if n >= 1 && n <= dirs.len() { dirs.remove(n - 1); } else { return Err("番号が範囲外です (/read-dir list で確認)".into()); }
                } else { dirs.retain(|d| !d.eq_ignore_ascii_case(clean)); }
            } else {
                let wd = validate_workdir(a)?.to_string_lossy().into_owned();
                if !dirs.contains(&wd) { dirs.push(wd); }
            }
            let _ = patch_cfg(st, json!({ "readOnlyFolders": dirs }), json!({}))?;
            eprintln!("readOnlyFolders={}", st.gui["readOnlyFolders"]);
        }
        "workspace" | "local-cli" | "run-on-pc" | "pc" => {
            let k = match cmd { "workspace" => "workspaceEnabled", "local-cli" => "localCliEnabled", "run-on-pc" => "runOnPc", _ => "pcEnabled" };
            let next = if a.is_empty() { !(st.gui.get(k).and_then(|v| v.as_bool()).unwrap_or(false)) } else { parse_toggle(a)? };
            let _ = patch_cfg(st, json!({ k: next }), json!({}))?;
            eprintln!("{}={}", k, next);
        }
        "shared-desktop" => {
            if a.is_empty() { println!("sharedDesktop={}", st.shared_desktop); }
            else { st.shared_desktop = parse_toggle(a)?; eprintln!("sharedDesktop={}", st.shared_desktop); }
        }
        "windows" => {
            let list = desktop_windows();
            let filtered: Vec<Value> = list.into_iter()
                .filter(|w| a.is_empty() || w.title.to_lowercase().contains(&a.to_lowercase()) || w.process_name.to_lowercase().contains(&a.to_lowercase()))
                .map(|w| serde_json::to_value(w).unwrap()).collect();
            st.recent_windows = filtered.clone();
            if filtered.is_empty() { println!("(該当ウィンドウなし)"); }
            else {
                let self_elev = is_current_process_elevated();
                for (i, w) in filtered.iter().enumerate() {
                    let elev = if !self_elev && w.get("elevated").and_then(|v| v.as_bool()).unwrap_or(false) { " [管理者・観察可]" } else { "" };
                    println!("{:>2}) [{}] {}{}", i + 1, w.get("id").and_then(|v| v.as_i64()).unwrap_or(0), w.get("title").and_then(|v| v.as_str()).unwrap_or(""), elev);
                }
            }
        }
        "window" => {
            if a.is_empty() { println!("pcWindowTitle={}", st.gui.get("pcWindowTitle").and_then(|v| v.as_str()).unwrap_or("(未選択)")); return Ok(()); }
            let val = if a == "clear" { String::new() }
                else if let Ok(n) = a.trim_start_matches('#').parse::<usize>() {
                    if n >= 1 && n <= st.recent_windows.len() { st.recent_windows[n - 1].get("title").and_then(|v| v.as_str()).unwrap_or("").to_string() }
                    else { a.to_string() }
                } else { a.to_string() };
            let _ = patch_cfg(st, json!({ "pcWindowTitle": val }), json!({}))?;
            eprintln!("pcWindowTitle={}", if val.is_empty() { "(未選択)" } else { &val });
        }
        "status" => {
            let toks: Vec<&str> = a.split_whitespace().collect();
            if !toks.is_empty() && !toks[0].eq_ignore_ascii_case("full") {
                let jid = toks[0];
                let q = if toks.get(1).is_some_and(|s| s.eq_ignore_ascii_case("full")) { "?full=1" } else { "" };
                show_json(&st.require_api()?.get(&format!("/jobs/{}{}", jid, q), 15000)?);
            } else if toks.first().is_some_and(|s| s.eq_ignore_ascii_case("full")) && valid_id(active_job_id(st, "").unwrap_or("")) {
                show_json(&st.require_api()?.get(&format!("/jobs/{}?full=1", active_job_id(st, "")?), 15000)?);
            } else {
                let api = st.require_api()?;
                let h = api.get("/health", 8000)?;
                let m = api.get("/models/status", 8000).unwrap_or_else(|_| json!({}));
                let sc = api.get("/schedules", 8000).unwrap_or_else(|_| json!({}));
                show_json(&json!({ "url": api.base_url(), "workMode": st.work_mode(), "conversationId": st.conv_id, "standby": st.standby.is_active(), "health": h, "models": m, "schedules": sc }));
            }
        }
        "models" => {
            let api = st.require_api()?;
            let empty = json!({});
            let v = match if a.is_empty() { "status" } else { a } {
                "status" => api.get("/models/status", 15000)?,
                "perf" => api.get("/models/perf", 15000)?,
                "start" => api.post("/models/start", &empty, 120_000)?,
                "stop" => api.post("/models/stop", &empty, 60_000)?,
                "restart" => api.post("/models/restart", &empty, 120_000)?,
                _ => return Err("使い方: /models [status|start|stop|restart|perf]".into()),
            };
            show_json(&v);
        }
        "perf" => show_json(&st.require_api()?.get("/models/perf", 15000)?),
        "gui-settings" => show_json(&load_gui_settings_file(&st.root)),
        "settings" => {
            let srv = if let Some(api) = &st.api { api.get("/settings", 15000)? } else { st.server.clone() };
            show_json(&json!({ "gui": st.gui, "server": srv, "conversationId": st.conv_id, "parentJobId": st.parent_job_id }));
        }
        "tune-apply" => show_json(&st.require_api()?.post("/tune/apply", &json!({}), 120_000)?),
        "fetch" | "save" => {
            let toks: Vec<&str> = a.split_whitespace().collect();
            let raw_id = toks.first().copied().unwrap_or("");
            let jid = active_job_id(st, raw_id)?;
            let api = st.require_api()?;
            let job = api.get(&format!("/jobs/{}", jid), 15000)?;
            let out_dir = match toks.get(1) { Some(d) => st.root.join(d), None => st.root.join("downloads") };
            eprintln!("保存先: {}", save_job_outputs_with_workspace(api, &job, &out_dir)?.display());
        }
        "cancel" => show_json(&st.require_api()?.post(&format!("/jobs/{}/cancel", active_job_id(st, a)?), &json!({}), 15000)?),
        "interrupt" => {
            let (jid, txt) = if let Some((id_part, rest)) = a.split_once(char::is_whitespace).filter(|(p, _)| valid_id(p)) { (id_part, rest.trim()) }
                else if !a.is_empty() && valid_id(active_job_id(st, "").unwrap_or("")) { (active_job_id(st, "")?, a) }
                else { return Err("使い方: /interrupt [jobId] <text>".into()); };
            if txt.is_empty() { return Err("追加指示テキストを指定してください".into()); }
            show_json(&st.require_api()?.post(&format!("/jobs/{}/interrupt", jid), &json!({ "text": txt }), 15000)?);
        }
        _ => return Err(format!("不明なコマンドです: /{} (/help で一覧)", cmd)),
    }
    Ok(())
}