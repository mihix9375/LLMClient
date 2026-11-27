use crate::api_cli::{normalize_tune_mode, resolve_cli_read_roots, resolve_cli_workspace, sync_sampling_to_gui};
use crate::cli::repl::ReplState;
use crate::cli::slash_cfg::show_json;
use crate::cli::term;
use crate::config::{find_window_match, save_gui_settings_file};
use crate::standby::StandbyNotice;
use crate::win_util::{clipboard_set_text, desktop_windows, open_external_target};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

pub fn handle_ext_slash(st: &mut ReplState, cmd: &str, a: &str) -> Result<bool, String> {
    match cmd {
        "standby" => return handle_standby_slash(st, a).map(|_| true),
        "stop-pc" => {
            st.standby.stop();
            st.gui["pcStandby"] = json!(false);
            if st.sync_gui { let _ = save_gui_settings_file(&st.root, &json!({ "pcStandby": false })); }
            eprintln!("PC緊急停止を実行しました（連携待機を停止）。");
        }
        "autotune" => {
            let api = st.api_arc()?;
            let (mut mode, mut workload, mut apply) = ("quick", "coding", false);
            for t in a.split_whitespace() {
                match t.to_ascii_lowercase().as_str() {
                    "thorough" | "full" | "quality" => mode = "thorough",
                    "quick" | "speed" => mode = "quick",
                    "chat" => workload = "chat",
                    "coding" => workload = "coding",
                    "apply" => apply = true,
                    _ => {}
                }
            }
            let _ = api.post("/tune/auto", &json!({ "task": workload, "workload": workload, "mode": normalize_tune_mode(mode) }), 15000)?;
            let mut status = json!({});
            for _ in 0..180 {
                if term::is_exit_signal() { break; }
                thread::sleep(Duration::from_secs(1));
                status = api.get("/tune/status", 10000)?;
                let s = status.get("status").and_then(|v| v.as_str()).unwrap_or("");
                eprintln!("[autotune] {} ({}%)", status.get("currentStage").and_then(|v| v.as_str()).unwrap_or(s), status.get("progressPercent").and_then(|v| v.as_i64()).unwrap_or(0));
                if matches!(s, "completed" | "failed" | "cancelled") { break; }
            }
            if apply && status.get("status").and_then(|v| v.as_str()) == Some("completed") && !status.get("alreadyOptimal").and_then(|v| v.as_bool()).unwrap_or(false) {
                let ap = api.post("/tune/apply", &json!({}), 120_000)?;
                if let Some(s) = ap.get("settings").filter(|v| v.is_object()) {
                    st.server = s.clone();
                    if st.sync_gui { sync_sampling_to_gui(&st.root, s); }
                }
                status["appliedResult"] = ap;
            }
            show_json(&status);
        }
        "tune-status" => show_json(&st.require_api()?.get("/tune/status", 15000)?),
        "identity" => show_json(&st.require_api()?.get("/identity", 15000)?),
        "commands" => show_json(&st.require_api()?.get("/commands", 15000)?),
        "profiles" => show_json(&st.require_api()?.get("/model-profiles", 15000)?),
        "adapters" => show_json(&st.require_api()?.get("/worker/adapters", 15000)?),
        "adapter-profiles" => show_json(&st.require_api()?.get("/worker/adapter-profiles", 15000)?),
        "classify" => {
            if a.is_empty() { return Err("使い方: /classify <text>".into()); }
            show_json(&st.require_api()?.post("/classify", &json!({ "text": a }), 120_000)?);
        }
        "copy" => {
            if st.last_answer.is_empty() { return Err("コピーできる直前の回答がありません".into()); }
            clipboard_set_text(&st.last_answer)?;
            eprintln!("直前の回答をクリップボードへコピーしました ({} chars)。", st.last_answer.chars().count());
        }
        "open-folder" => {
            let wd = st.gui.get("localWorkdir").and_then(|v| v.as_str()).unwrap_or("");
            let target = if wd.is_empty() { st.root.join("local-workspaces") } else { PathBuf::from(wd) };
            let _ = std::fs::create_dir_all(&target);
            open_external_target(&target.to_string_lossy())?;
            eprintln!("フォルダーを開きました: {}", target.display());
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn handle_standby_slash(st: &mut ReplState, a: &str) -> Result<(), String> {
    if a.eq_ignore_ascii_case("status") {
        println!("standby={}", if st.standby.is_active() { "active" } else { "inactive" });
        return Ok(());
    }
    if matches!(a.to_ascii_lowercase().as_str(), "off" | "stop") {
        st.standby.stop();
        st.gui["pcStandby"] = json!(false);
        eprintln!("PC連携待機を停止しました。");
        return Ok(());
    }
    let url = st.require_api()?.base_url().to_string();
    let list: Vec<Value> = desktop_windows().into_iter().map(|w| serde_json::to_value(w).unwrap()).collect();
    let q = if a.is_empty() || a.eq_ignore_ascii_case("on") {
        st.gui.get("pcWindowTitle").and_then(|v| v.as_str()).unwrap_or("").to_string()
    } else if let Ok(n) = a.trim_start_matches('#').parse::<usize>() {
        st.recent_windows.get(n.saturating_sub(1)).and_then(|w| w.get("id")).and_then(|v| v.as_i64()).map(|id| id.to_string()).unwrap_or_else(|| a.to_string())
    } else { a.to_string() };
    let matched = find_window_match(&list, &q).ok_or("対象ウィンドウが見つかりません (`/windows` と `/window <name|#N>` で指定)")?;
    let hwnd = matched.get("id").and_then(|v| v.as_i64()).unwrap_or(0) as isize;
    let title = matched.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let allow_local = st.gui.get("localCliEnabled").and_then(|v| v.as_bool()) != Some(false);
    let wd = st.gui.get("localWorkdir").and_then(|v| v.as_str()).unwrap_or("");
    let ws = if allow_local { resolve_cli_workspace(&st.root, wd, "shared")? } else { PathBuf::new() };
    let roots = resolve_cli_read_roots(&st.gui, &[], false)?;
    st.standby.start(&url, hwnd, &title, ws, roots, allow_local, |n| match n {
        StandbyNotice::State(s) => eprintln!("[standby] {}", s),
        StandbyNotice::StandbyEnded { error } if !error.is_empty() => eprintln!("[standby] 終了: {}", error),
        _ => {}
    })?;
    st.gui["pcStandby"] = json!(true);
    st.gui["pcWindowTitle"] = json!(title);
    eprintln!("PC連携待機を開始しました: {} (hwnd={})", title, hwnd);
    Ok(())
}