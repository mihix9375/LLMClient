use crate::cli::attach::handle_attach_slash;
use crate::cli::format::filter_conversations;
use crate::cli::help::SLASH_HELP;
use crate::cli::repl::ReplState;
use crate::cli::slash_cfg::{handle_cfg_slash, patch_cfg, show_json};
use crate::cli::slash_ops::handle_ops_slash;
use crate::config::valid_id;
use crate::wording::CONTINUE_MESSAGE;
use serde_json::json;

pub enum SlashAction {
    Handled,
    Exit,
    RunTask {
        goal: String,
        slash_cmd: Option<String>,
        files: Option<Vec<serde_json::Value>>,
    },
}

pub fn parse_slash(raw: &str) -> Option<(String, String)> {
    let rest = raw.trim_start_matches('\u{FEFF}').trim().strip_prefix('/')?;
    let (head, arg) = match rest.split_once(char::is_whitespace) {
        Some((h, a)) => (h, a.trim()),
        None => (rest, ""),
    };
    if head.is_empty() {
        return None;
    }
    Some((head.to_ascii_lowercase(), arg.to_string()))
}

pub fn parse_toggle(raw: &str) -> Result<bool, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "yes" => Ok(true),
        "off" | "false" | "0" | "no" => Ok(false),
        _ => Err("on または off を指定してください".into()),
    }
}

pub fn parse_schedule_spec(arg: &str) -> Result<(String, String), String> {
    let mut it = arg.split_whitespace();
    let kw = it.next().unwrap_or("").to_ascii_lowercase();
    let amt = it.next().unwrap_or("");
    let ok_kw = matches!(kw.as_str(), "in" | "every");
    let ok_amt = amt.len() >= 2
        && amt[..amt.len() - 1].bytes().all(|b| b.is_ascii_digit())
        && matches!(amt.as_bytes()[amt.len() - 1].to_ascii_lowercase(), b's' | b'm' | b'h');
    let rest = arg
        .find(amt)
        .and_then(|pos| (!amt.is_empty()).then(|| arg[pos + amt.len()..].trim()))
        .unwrap_or("");
    if !ok_kw || !ok_amt || rest.is_empty() {
        return Err("使い方: /schedule [list | cancel <id> | in 10m <task> | every 1h <task>]".into());
    }
    Ok((format!("{} {}", kw, amt), rest.to_string()))
}

pub fn resolve_conv_arg(arg: &str, recent: &[(String, String)]) -> Result<String, String> {
    let clean = arg.trim().trim_start_matches('#');
    if let Ok(idx) = clean.parse::<usize>() {
        if idx >= 1 && idx <= recent.len() {
            return Ok(recent[idx - 1].0.clone());
        }
        return Err(format!("会話番号 {} は範囲外です (/history で確認)", idx));
    }
    if valid_id(clean) {
        return Ok(clean.to_string());
    }
    Err("有効な会話 UUID または /history の番号を指定してください".into())
}

pub fn handle_slash(st: &mut ReplState, raw: &str) -> Result<SlashAction, String> {
    let Some((cmd, arg)) = parse_slash(raw) else {
        return Err("不明なコマンドです。/help で一覧を表示します。".into());
    };
    let a = arg.as_str();
    match cmd.as_str() {
        "exit" | "quit" => return Ok(SlashAction::Exit),
        "help" => println!("{}", SLASH_HELP),
        "new" | "clear" => {
            st.conv_id.clear();
            st.parent_job_id.clear();
            st.last_job_id.clear();
            st.attachments.clear();
            eprintln!("新しい会話を開始します。");
        }
        "history" => {
            let (mut limit, mut filter, mut q_parts) = (20usize, String::new(), Vec::new());
            for tok in a.split_whitespace() {
                if let Ok(n) = tok.parse::<usize>() { limit = n.clamp(1, 50); }
                else if matches!(tok.to_ascii_lowercase().as_str(), "all" | "active" | "done") { filter = tok.to_ascii_lowercase(); }
                else { q_parts.push(tok); }
            }
            let list = st.require_api()?.get("/conversations", 15000)?;
            st.recent_convs.clear();
            if let Some(arr) = list.as_array() {
                for (i, c) in filter_conversations(arr, &filter, &q_parts.join(" ")).iter().rev().take(limit).enumerate() {
                    let id = c.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let title = c.get("title").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("(無題)").to_string();
                    let st_s = c.get("status").and_then(|v| v.as_str()).unwrap_or("ready");
                    let jcnt = c.get("jobCount").and_then(|v| v.as_u64()).unwrap_or(0);
                    println!("{:>2}) {} [{}·{}手] {}", i + 1, id.chars().take(8).collect::<String>(), st_s, jcnt, title);
                    st.recent_convs.push((id, title));
                }
                if st.recent_convs.is_empty() { eprintln!("(該当する会話履歴はありません)"); }
            }
        }
        "conv" => {
            if a.is_empty() { println!("conversationId={}", st.conv_id); }
            else if a == "clear" { st.conv_id.clear(); eprintln!("会話IDを解除しました。"); }
            else {
                st.conv_id = resolve_conv_arg(a, &st.recent_convs)?;
                st.parent_job_id.clear();
                eprintln!("会話を切り替えました: {}", st.conv_id);
            }
        }
        "resume" => {
            if !valid_id(a) { return Err("使い方: /resume <jobId>".into()); }
            let job = st.require_api()?.get(&format!("/jobs/{}", a), 15000)?;
            if job.get("status").and_then(|v| v.as_str()) != Some("completed") || job.get("result").is_none() {
                return Err("完了済みのジョブを指定してください。".into());
            }
            st.parent_job_id = a.to_string();
            st.last_job_id = a.to_string();
            if let Some(cid) = job.get("conversationId").and_then(|v| v.as_str()) { st.conv_id = cid.to_string(); }
            eprintln!("ジョブから再開します: {}", a);
        }
        "continue" => return Ok(SlashAction::RunTask { goal: CONTINUE_MESSAGE.into(), slash_cmd: None, files: None }),
        "retry" | "resend" => {
            if st.last_goal.is_empty() { return Err("再送信できる直前の依頼がありません".into()); }
            return Ok(SlashAction::RunTask { goal: st.last_goal.clone(), slash_cmd: st.last_slash.clone(), files: Some(st.last_files.clone()) });
        }
        "attach" => handle_attach_slash(&mut st.attachments, a)?,
        "paste" => handle_attach_slash(&mut st.attachments, "paste")?,
        "plan" if a.is_empty() || parse_toggle(a).is_ok() => {
            let cur = st.gui.get("planMode").and_then(|v| v.as_bool()).unwrap_or(false);
            let next = if a.is_empty() { !cur } else { parse_toggle(a)? };
            let _ = patch_cfg(st, json!({ "planMode": next }), json!({}))?;
            eprintln!("planMode={}", next);
        }
        "plan" | "goal" | "grill-me" | "boost" | "teamwork" | "browser" => {
            if a.is_empty() { return Err(format!("使い方: /{} <指示内容>", cmd)); }
            return Ok(SlashAction::RunTask { goal: a.to_string(), slash_cmd: Some(cmd), files: None });
        }
        "compact" => {
            let cid = if valid_id(&st.conv_id) { st.conv_id.clone() }
                else if valid_id(&st.parent_job_id) {
                    st.require_api()?.get(&format!("/jobs/{}", st.parent_job_id), 15000)?.get("conversationId").and_then(|v| v.as_str()).unwrap_or("").to_string()
                } else { String::new() };
            if !valid_id(&cid) { return Err("圧縮対象の会話がありません".into()); }
            let body = if a.is_empty() { json!({}) } else { json!({ "focus": a }) };
            show_json(&st.require_api()?.post(&format!("/conversations/{}/compact", cid), &body, 30_000)?);
        }
        "learn" => {
            let api = st.api_arc()?;
            if a.is_empty() || a == "list" { show_json(&api.get("/memory", 15000)?); }
            else if let Some(id) = a.strip_prefix("delete ").or_else(|| a.strip_prefix("remove ")).or_else(|| a.strip_prefix("rm ")) {
                show_json(&api.delete(&format!("/memory/{}", id.trim()), 15000)?);
            } else { show_json(&api.post("/memory", &json!({ "text": a }), 15000)?); }
        }
        "schedule" => {
            let api = st.api_arc()?;
            if a.is_empty() || a == "list" { show_json(&api.get("/schedules", 15000)?); }
            else if let Some(id) = a.strip_prefix("cancel ").or_else(|| a.strip_prefix("delete ")).or_else(|| a.strip_prefix("rm ")) {
                show_json(&api.delete(&format!("/schedules/{}", id.trim()), 15000)?);
            } else {
                let (spec, goal) = parse_schedule_spec(a)?;
                let mut body = json!({ "spec": spec, "goal": goal });
                if valid_id(&st.conv_id) { body["conversationId"] = json!(st.conv_id); }
                show_json(&api.post("/schedules", &body, 15000)?);
            }
        }
        _ => if !handle_ops_slash(st, &cmd, a)? { handle_cfg_slash(st, &cmd, a)?; },
    }
    Ok(SlashAction::Handled)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slash_parsers_and_conv_resolution() {
        assert_eq!(parse_slash("/MODE chat"), Some(("mode".into(), "chat".into())));
        assert_eq!(parse_slash("\u{FEFF}/help"), Some(("help".into(), String::new())));
        assert_eq!(parse_slash("/exit"), Some(("exit".into(), String::new())));
        assert_eq!(parse_slash("plain"), None);
        assert!(parse_toggle("on").unwrap());
        assert!(!parse_toggle("OFF").unwrap());
        assert!(parse_toggle("maybe").is_err());
        let (sp, g) = parse_schedule_spec("in 10m テストを実行").unwrap();
        assert_eq!((sp.as_str(), g.as_str()), ("in 10m", "テストを実行"));
        assert!(parse_schedule_spec("later 5m x").is_err());
        let recent = vec![("12345678-1234-1234-1234-1234567890ab".into(), "t1".into())];
        assert_eq!(resolve_conv_arg("1", &recent).unwrap(), recent[0].0);
        assert_eq!(resolve_conv_arg("#1", &recent).unwrap(), recent[0].0);
        assert!(resolve_conv_arg("2", &recent).is_err());
    }
}