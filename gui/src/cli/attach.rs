use crate::attachments::{
    build_attachment_from_dib, build_attachment_from_file_with, build_attachment_from_png,
    MAX_ATTACHMENT_BYTES,
};
use crate::win_util::{clipboard_read_payload, ClipboardPayload};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};
use std::path::Path;

pub fn existing_paths_and_bytes(items: &[Value]) -> (Vec<String>, usize) {
    let mut paths = Vec::with_capacity(items.len());
    let mut used = 0usize;
    for item in items {
        if let Some(p) = item.get("path").and_then(|v| v.as_str()) {
            paths.push(p.to_string());
        }
        used += item.get("size").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    }
    (paths, used)
}

pub fn push_file_attachment(items: &mut Vec<Value>, path: &Path) -> Result<Value, String> {
    if items.len() >= 5 {
        return Err("添付は最大5件までです".into());
    }
    let (existing, used) = existing_paths_and_bytes(items);
    let remaining = MAX_ATTACHMENT_BYTES.saturating_sub(used);
    let att = build_attachment_from_file_with(path, &existing, remaining)?;
    items.push(att.clone());
    Ok(att)
}

pub fn push_clipboard_attachments(items: &mut Vec<Value>) -> Result<Vec<Value>, String> {
    if items.len() >= 5 { return Err("添付は最大5件までです".into()); }
    let (mut existing, mut used) = existing_paths_and_bytes(items);
    let mut added = Vec::new();
    match clipboard_read_payload()? {
        ClipboardPayload::Files(paths) => for p in paths {
            if items.len() >= 5 { break; }
            let att = build_attachment_from_file_with(&p, &existing, MAX_ATTACHMENT_BYTES.saturating_sub(used))?;
            if let Some(n) = att.get("path").and_then(|v| v.as_str()) { existing.push(n.to_string()); }
            used += att.get("size").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            items.push(att.clone());
            added.push(att);
        },
        ClipboardPayload::PngBytes(png) => {
            let att = build_attachment_from_png(&png, &existing, MAX_ATTACHMENT_BYTES.saturating_sub(used))?;
            items.push(att.clone());
            added.push(att);
        }
        ClipboardPayload::DibBytes(dib) => {
            let att = build_attachment_from_dib(&dib, &existing, MAX_ATTACHMENT_BYTES.saturating_sub(used))?;
            items.push(att.clone());
            added.push(att);
        }
        ClipboardPayload::Text | ClipboardPayload::Empty => return Err("クリップボードに画像またはファイルがありません".into()),
    }
    Ok(added)
}

pub fn attachments_to_request_files(items: &[Value]) -> Vec<Value> {
    items
        .iter()
        .map(|a| json!({ "path": a["path"], "base64": a["base64"] }))
        .collect()
}

pub fn load_oneshot_attachments(paths: &[String], include_clipboard: bool) -> Result<Vec<Value>, String> {
    let mut items = Vec::new();
    for p in paths {
        push_file_attachment(&mut items, Path::new(p))?;
    }
    if include_clipboard {
        push_clipboard_attachments(&mut items)?;
    }
    Ok(attachments_to_request_files(&items))
}

fn find_attach_idx(items: &[Value], query: &str) -> Result<usize, String> {
    let clean = query.trim().trim_start_matches('#');
    if let Ok(n) = clean.parse::<usize>() {
        if n >= 1 && n <= items.len() {
            return Ok(n - 1);
        }
        return Err(format!("添付番号 {} は範囲外です (/attach list で確認)", n));
    }
    let needle = clean.to_ascii_lowercase();
    items
        .iter()
        .position(|a| {
            let p = a.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let d = a.get("displayName").and_then(|v| v.as_str()).unwrap_or("");
            p.to_ascii_lowercase() == needle || d.to_ascii_lowercase() == needle
        })
        .ok_or_else(|| format!("添付が見つかりません: {}", query))
}

pub fn print_attach_list(items: &[Value]) {
    if items.is_empty() {
        println!("添付なし (0/5)");
        return;
    }
    let (_, used) = existing_paths_and_bytes(items);
    println!("添付一覧 ({}/5, {:.1} KB):", items.len(), used as f64 / 1024.0);
    for (i, a) in items.iter().enumerate() {
        let name = a.get("displayName").or_else(|| a.get("path")).and_then(|v| v.as_str()).unwrap_or("");
        let kind = a.get("kind").and_then(|v| v.as_str()).unwrap_or("file");
        let sz = a.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
        println!("  {}) {} [{}] ({} B)", i + 1, name, kind, sz);
    }
}

pub fn handle_attach_slash(items: &mut Vec<Value>, arg: &str) -> Result<(), String> {
    let a = arg.trim();
    if a.is_empty() || a.eq_ignore_ascii_case("list") {
        print_attach_list(items);
        return Ok(());
    }
    if a.eq_ignore_ascii_case("clear") {
        items.clear();
        eprintln!("添付を解除しました。");
        return Ok(());
    }
    if a.eq_ignore_ascii_case("paste") {
        let added = push_clipboard_attachments(items)?;
        for x in &added {
            let nm = x.get("displayName").or_else(|| x.get("path")).and_then(|v| v.as_str()).unwrap_or("attachment");
            eprintln!("クリップボードから添付しました ({}/5): {}", items.len(), nm);
        }
        return Ok(());
    }
    if let Some(rest) = a.strip_prefix("remove ").or_else(|| a.strip_prefix("rm ")).or_else(|| a.strip_prefix("delete ")) {
        let idx = find_attach_idx(items, rest)?;
        let removed = items.remove(idx);
        let nm = removed.get("displayName").or_else(|| removed.get("path")).and_then(|v| v.as_str()).unwrap_or("");
        eprintln!("添付を削除しました: {} (残り {}/5)", nm, items.len());
        return Ok(());
    }
    if let Some(rest) = a.strip_prefix("preview ") {
        let item = &items[find_attach_idx(items, rest)?];
        let title = item.get("displayName").or_else(|| item.get("path")).and_then(|v| v.as_str()).unwrap_or("");
        let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("file");
        let sz = item.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
        println!("=== {} [{} / {} B] ===", title, kind, sz);
        if kind == "image" {
            println!("(画像データのためプレビュー本文はありません)");
        } else if let Ok(bytes) = B64.decode(item.get("base64").and_then(|v| v.as_str()).unwrap_or("")) {
            let txt = String::from_utf8_lossy(&bytes);
            let shown: String = txt.chars().take(4000).collect();
            println!("{}", shown);
            if txt.chars().count() > 4000 { eprintln!("…(以降省略)"); }
        }
        return Ok(());
    }
    let clean = a.trim_matches('"');
    let att = push_file_attachment(items, Path::new(clean))?;
    let nm = att.get("displayName").or_else(|| att.get("path")).and_then(|v| v.as_str()).unwrap_or(clean);
    eprintln!("添付追加 ({}/5): {}", items.len(), nm);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_list_remove_and_request_mapping_work() {
        let dir = std::env::temp_dir().join(format!("qwen_cli_att_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f1 = dir.join("note.txt");
        let f2 = dir.join("code.rs");
        std::fs::write(&f1, "hello attach").unwrap();
        std::fs::write(&f2, "fn main() {}").unwrap();

        let mut items = Vec::new();
        handle_attach_slash(&mut items, f1.to_str().unwrap()).unwrap();
        handle_attach_slash(&mut items, f2.to_str().unwrap()).unwrap();
        assert_eq!(items.len(), 2);
        let req = attachments_to_request_files(&items);
        assert_eq!(req[0]["path"], "uploads/note.txt");
        handle_attach_slash(&mut items, "preview #1").unwrap();
        handle_attach_slash(&mut items, "remove 1").unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["displayName"], "code.rs");
        handle_attach_slash(&mut items, "clear").unwrap();
        assert!(items.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}