use crate::api_cli::{connect_api_with_failover, sync_sampling_to_gui};
use crate::cli::attach::push_file_attachment;
use crate::cli::format::{print_conversation_summary, print_file_or_list, print_share_list};
use crate::cli::repl::ReplState;
use crate::cli::runner::extract_final_answer;
use crate::cli::slash::{parse_toggle, resolve_conv_arg};
use crate::cli::slash_cfg::{patch_cfg, show_json};
use crate::cli::slash_ext::handle_ext_slash;
use crate::config::{save_gui_settings_file, valid_id, validate_workdir, SamplingValues};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn split_cli_tokens(raw: &str) -> Vec<String> {
    let (mut out, mut cur, mut in_q) = (Vec::new(), String::new(), false);
    for ch in raw.chars() {
        match ch {
            '"' => in_q = !in_q,
            c if c.is_whitespace() && !in_q => if !cur.is_empty() { out.push(std::mem::take(&mut cur)); },
            c => cur.push(c),
        }
    }
    if !cur.is_empty() { out.push(cur); }
    out
}

fn require_conv_id(st: &ReplState) -> Result<String, String> {
    if valid_id(&st.conv_id) { Ok(st.conv_id.clone()) }
    else { Err("会話が選択されていません。/open <#番号|ID> で開くか、先にメッセージを送信してください".into()) }
}

pub fn handle_ops_slash(st: &mut ReplState, cmd: &str, a: &str) -> Result<bool, String> {
    match cmd {
        "connect" | "url" => {
            let client = connect_api_with_failover(&st.root, a)?;
            let health = client.get("/health", 15000)?;
            if health.get("service").and_then(|v| v.as_str()) != Some("qwen27-framework") {
                return Err("接続先が Qwen27 Framework API ではありません".into());
            }
            let url = client.base_url().to_string();
            st.server = client.get("/settings", 15000).unwrap_or_else(|_| json!({}));
            st.gui["api"] = json!(url);
            if st.sync_gui { let _ = save_gui_settings_file(&st.root, &json!({ "api": url })); }
            st.api = Some(std::sync::Arc::new(client));
            let ip = health.get("clientIp").and_then(|v| v.as_str()).unwrap_or("");
            eprintln!("サーバーに接続しました: {} ({})", url, if ip.is_empty() { "ok" } else { ip });
        }
        "auto-connect" => {
            if a.is_empty() { println!("autoConnect={}", st.gui.get("autoConnect").and_then(|v| v.as_bool()) != Some(false)); }
            else { let b = parse_toggle(a)?; let _ = patch_cfg(st, json!({ "autoConnect": b }), json!({}))?; eprintln!("autoConnect={}", b); }
        }
        "theme" => {
            let cur = st.gui.get("darkTheme").and_then(|v| v.as_bool()).unwrap_or(true);
            let next = match a.to_ascii_lowercase().as_str() {
                "dark" | "on" => true, "light" | "off" => false, "toggle" | "" => !cur,
                _ => return Err("使い方: /theme [dark|light|toggle]".into()),
            };
            st.gui["darkTheme"] = json!(next);
            if st.sync_gui { let _ = save_gui_settings_file(&st.root, &json!({ "darkTheme": next })); }
            eprintln!("darkTheme={}", next);
        }
        "reset-sampling" => {
            let mut samp = json!({});
            crate::config::sampling_to_json(&SamplingValues::default(), &mut samp);
            let mut gp = samp.clone();
            gp["samplingPreset"] = json!("code");
            if st.sync_gui { sync_sampling_to_gui(&st.root, &samp); }
            show_json(&patch_cfg(st, gp, samp)?);
        }
        "open" | "show" => {
            let cid = if a.is_empty() { require_conv_id(st)? } else { resolve_conv_arg(a, &st.recent_convs)? };
            let conv = st.require_api()?.get(&format!("/conversations/{}", cid), 15000)?;
            print_conversation_summary(&conv);
            st.conv_id = cid.clone();
            st.parent_job_id.clear();
            st.last_job_id.clear();
            if let Some(last) = conv.get("jobs").and_then(|v| v.as_array()).and_then(|arr| arr.last()) {
                if let Some(jid) = last.get("id").and_then(|v| v.as_str()) {
                    st.last_job_id = jid.to_string();
                    if last.get("status").and_then(|v| v.as_str()) == Some("completed") && last.get("result").is_some() {
                        st.parent_job_id = jid.to_string();
                    }
                }
                st.last_answer = extract_final_answer(last);
            }
            if let Some(wd) = st.gui.get("localWorkdirs").and_then(|m| m.get(&cid)).and_then(|v| v.as_str()) {
                if validate_workdir(wd).is_ok() { st.gui["localWorkdir"] = json!(wd); }
            }
        }
        "files" => {
            let cid = require_conv_id(st)?;
            let api = st.api_arc()?;
            if a.is_empty() || a.eq_ignore_ascii_case("list") {
                print_file_or_list(&api.get(&format!("/conversations/{}/files", cid), 15000)?, None)?;
            } else if let Some(rest) = a.strip_prefix("save ") {
                let toks = split_cli_tokens(rest);
                let remote = toks.first().ok_or("使い方: /files save <remote> [dest]")?;
                let enc = utf8_percent_encode(remote, NON_ALPHANUMERIC).to_string();
                let val = api.get(&format!("/conversations/{}/files?path={}", cid, enc), 30000)?;
                let dest = toks.get(1).map(PathBuf::from).unwrap_or_else(|| {
                    st.root.join("downloads").join(&cid).join(Path::new(remote).file_name().unwrap_or_default())
                });
                print_file_or_list(&val, Some(&dest))?;
            } else {
                let enc = utf8_percent_encode(a, NON_ALPHANUMERIC).to_string();
                print_file_or_list(&api.get(&format!("/conversations/{}/files?path={}", cid, enc), 30000)?, None)?;
            }
        }
        "upload" => {
            let cid = require_conv_id(st)?;
            let mut tmp = Vec::new();
            let toks = if Path::new(a.trim_matches('"')).is_file() { vec![a.trim_matches('"').to_string()] } else { split_cli_tokens(a) };
            for p in toks { push_file_attachment(&mut tmp, Path::new(&p))?; }
            if tmp.is_empty() { return Err("使い方: /upload <file...>".into()); }
            let req: Vec<Value> = tmp.iter().map(|x| json!({ "path": x["path"], "base64": x["base64"] })).collect();
            show_json(&st.require_api()?.post(&format!("/conversations/{}/files", cid), &json!({ "files": req }), 60_000)?);
        }
        "share" => {
            let cid = require_conv_id(st)?;
            let api = st.api_arc()?;
            let toks = split_cli_tokens(a);
            let sub = toks.first().map(|s| s.as_str()).unwrap_or("list");
            if sub.eq_ignore_ascii_case("list") {
                let val = api.get(&format!("/conversations/{}/access", cid), 15000).or_else(|_| api.get(&format!("/conversations/{}", cid), 15000))?;
                st.recent_shares = val.get("allowedIps").and_then(|v| v.as_array()).map(|arr| {
                    arr.iter().map(|e| (e.get("ip").and_then(|v| v.as_str()).unwrap_or("").into(), e.get("mode").and_then(|v| v.as_str()).unwrap_or("content").into())).collect()
                }).unwrap_or_default();
                print_share_list(&val);
            } else if matches!(sub, "remove" | "delete" | "rm") {
                let target = toks.get(1).ok_or("使い方: /share remove <ip|#番号>")?;
                let (ip, mode) = if let Ok(n) = target.trim_start_matches('#').parse::<usize>() {
                    st.recent_shares.get(n.saturating_sub(1)).cloned().ok_or("共有番号が範囲外です (/share list で確認)")?
                } else { (target.clone(), "content".into()) };
                print_share_list(&api.post(&format!("/conversations/{}/access", cid), &json!({ "ip": ip, "mode": mode, "remove": true }), 15000)?);
            } else {
                let (ip, mode) = if sub == "add" {
                    (toks.get(1).ok_or("使い方: /share add <ip> [content|member]")?.clone(), toks.get(2).cloned().unwrap_or_else(|| "content".into()))
                } else { (toks[0].clone(), toks.get(1).cloned().unwrap_or_else(|| "content".into())) };
                print_share_list(&api.post(&format!("/conversations/{}/access", cid), &json!({ "ip": ip, "mode": mode, "remove": false }), 15000)?);
            }
        }
        _ => return handle_ext_slash(st, cmd, a),
    }
    Ok(true)
}