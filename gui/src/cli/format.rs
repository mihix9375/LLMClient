use crate::win_util::write_file_atomic;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::Value;
use std::path::Path;

pub fn print_json(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

pub fn filter_conversations(arr: &[Value], filter: &str, query: &str) -> Vec<Value> {
    let f = filter.trim().to_ascii_lowercase();
    let q = query.trim().to_ascii_lowercase();
    arr.iter()
        .filter(|c| {
            let st = c.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let is_run = matches!(st, "running" | "queued" | "waiting_user");
            match f.as_str() {
                "active" | "running" if !is_run => return false,
                "done" | "completed" if is_run => return false,
                _ => {}
            }
            if !q.is_empty() {
                let title = c.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let cid = c.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if !title.to_ascii_lowercase().contains(&q) && !cid.to_ascii_lowercase().contains(&q) {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect()
}

pub fn print_conversation_summary(conv: &Value) {
    let cid = conv.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let title = conv.get("title").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("(無題の会話)");
    let ro = if conv.get("readOnly").and_then(|v| v.as_bool()).unwrap_or(false) { " [閲覧専用]" } else { "" };
    println!("会話: {}{}\nID: {}", title, ro, cid);
    let jobs = conv.get("jobs").and_then(|v| v.as_array()).map(|a| a.as_slice()).unwrap_or(&[]);
    if jobs.is_empty() {
        println!("(メッセージなし)");
        return;
    }
    for (idx, j) in jobs.iter().enumerate() {
        let jid = j.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let st = j.get("status").and_then(|v| v.as_str()).unwrap_or("");
        let goal = j.get("goal").and_then(|v| v.as_str()).unwrap_or("");
        println!("\n--- [{}] ({}) id={} ---", idx + 1, st, jid);
        println!("依頼> {}", goal);
        let ans = j.get("result").and_then(|r| r.get("review")).and_then(|rv| rv.get("final_answer")).and_then(|v| v.as_str()).filter(|s| !s.is_empty())
            .or_else(|| j.get("error").and_then(|v| v.as_str()).filter(|s| !s.is_empty()))
            .or_else(|| j.get("partialAnswer").and_then(|v| v.as_str()).filter(|s| !s.is_empty()));
        if let Some(txt) = ans {
            let clipped: String = txt.chars().take(1200).collect();
            println!("応答> {}", clipped);
            if txt.chars().count() > 1200 { eprintln!("…(詳細は `status <jobId> --full` で確認)"); }
        }
    }
}

pub fn print_file_or_list(val: &Value, save_to: Option<&Path>) -> Result<(), String> {
    if let Some(arr) = val.get("files").and_then(|v| v.as_array()) {
        if arr.is_empty() {
            println!("(ファイルなし)");
        } else {
            for (i, item) in arr.iter().enumerate() {
                let p = item.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let b = item.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0);
                println!("{:>2}) {} ({} B)", i + 1, p, b);
            }
        }
        return Ok(());
    }
    if let Some(b64) = val.get("base64").and_then(|v| v.as_str()) {
        let bytes = B64.decode(b64).map_err(|e| format!("base64 デコード失敗: {}", e))?;
        if let Some(dest) = save_to {
            write_file_atomic(dest, &bytes)?;
            println!("保存先: {} ({} B)", dest.display(), bytes.len());
            return Ok(());
        }
        if let Some(saved) = val.get("savedPath").and_then(|v| v.as_str()) {
            println!("保存先: {} ({} B)", saved, bytes.len());
            return Ok(());
        }
        let name = val.get("path").and_then(|v| v.as_str()).unwrap_or("file").to_ascii_lowercase();
        let is_img = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp"].iter().any(|ext| name.ends_with(ext));
        if !is_img {
            if let Ok(txt) = std::str::from_utf8(&bytes) {
                let clipped: String = txt.chars().take(8000).collect();
                println!("{}", clipped);
                if txt.chars().count() > 8000 { eprintln!("…(省略: `files <cid> <path> --output <file>` で全量保存)"); }
                return Ok(());
            }
        }
        println!("バイナリファイル ({} B)。`--output <保存先>` で保存できます。", bytes.len());
        return Ok(());
    }
    print_json(val);
    Ok(())
}

pub fn print_share_list(val: &Value) {
    let entries = val.get("allowedIps").and_then(|v| v.as_array()).map(|a| a.as_slice()).unwrap_or(&[]);
    if entries.is_empty() {
        println!("(共有設定なし)");
        return;
    }
    for (i, e) in entries.iter().enumerate() {
        let ip = e.get("ip").and_then(|v| v.as_str()).unwrap_or("");
        let mode = e.get("mode").and_then(|v| v.as_str()).unwrap_or("content");
        println!("{:>2}) {} ({})", i + 1, ip, mode);
    }
}

pub fn format_health_line(v: &Value) -> String {
    let status = v.get("status").and_then(|x| x.as_str()).unwrap_or_else(|| {
        if v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false) { "ok" } else { "error" }
    });
    let svc = v.get("service").and_then(|x| x.as_str()).unwrap_or("unknown");
    let ver = v.get("apiVersion").and_then(|x| x.as_i64()).unwrap_or(0);
    let ip = v.get("clientIp").and_then(|x| x.as_str()).unwrap_or("");
    if ip.is_empty() {
        format!("status={} service={} apiVersion={}", status, svc, ver)
    } else {
        format!("status={} service={} apiVersion={} clientIp={}", status, svc, ver, ip)
    }
}

pub fn format_human(cmd: &str, v: &Value) {
    match cmd {
        "health" => println!("{}", format_health_line(v)),
        "connect" => {
            let url = v.get("url").and_then(|x| x.as_str()).unwrap_or("");
            let hl = v.get("health").map(format_health_line).unwrap_or_default();
            println!("connected={} {}", url, hl);
        }
        "perf" => {
            let tps = v.get("tokensPerSec").or_else(|| v.get("lastTokensPerSec")).and_then(|x| x.as_f64());
            let pp = v.get("promptTokensPerSec").and_then(|x| x.as_f64());
            if tps.is_some() || pp.is_some() { println!("gen={:.1} tok/s  prompt={:.1} tok/s", tps.unwrap_or(0.0), pp.unwrap_or(0.0)); }
            else { print_json(v); }
        }
        "conversations" => match v.as_array() {
            Some(arr) if arr.is_empty() => println!("(会話履歴はありません)"),
            Some(arr) => for (i, c) in arr.iter().rev().take(30).enumerate() {
                let st = c.get("status").and_then(|x| x.as_str()).unwrap_or("ready");
                println!("{:>2}) {} [{}] {}", i + 1, c.get("id").and_then(|x| x.as_str()).unwrap_or(""), st, c.get("title").and_then(|x| x.as_str()).unwrap_or("(無題)"));
            },
            None => print_json(v),
        },
        "conversation" => print_conversation_summary(v),
        "files" => { let _ = print_file_or_list(v, None); }
        "share" => if v.get("allowedIps").is_some() { print_share_list(v); } else { print_json(v); },
        _ => print_json(v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn health_status_and_conversation_filtering_work() {
        let h = json!({ "status": "ok", "service": "qwen27-framework", "apiVersion": 8, "clientIp": "192.0.2.5" });
        assert_eq!(format_health_line(&h), "status=ok service=qwen27-framework apiVersion=8 clientIp=192.0.2.5");

        let arr = vec![
            json!({ "id": "aaa", "title": "Build fix", "status": "running" }),
            json!({ "id": "bbb", "title": "Release v4", "status": "completed" }),
        ];
        assert_eq!(filter_conversations(&arr, "active", "").len(), 1);
        assert_eq!(filter_conversations(&arr, "done", "release").len(), 1);
        assert_eq!(filter_conversations(&arr, "", "aaa").len(), 1);
    }
}