use crate::config::diag_line;
use crate::image_codec::{
    compress_image_bytes_to_budget, dib_bytes_to_png, make_thumbnail_data_url,
};
use crate::tls_http::ApiClient;
use crate::win_util::no_links;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chrono::Local;
use parking_lot::{Condvar, Mutex};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const MAX_ATTACHMENT_BYTES: usize = 2_097_152;
pub const MAX_READABLE_IMAGE_FILE_BYTES: u64 = 33_554_432;
const STAGED_UPLOAD_WAIT_MS: u64 = 30_000;
const STAGED_DELETE_TIMEOUT_MS: u64 = 5_000;

static CLIPBOARD_SEQ: AtomicUsize = AtomicUsize::new(1);

pub fn is_image_filename(filename: &str) -> bool {
    let lower = filename.to_ascii_lowercase();
    lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".bmp")
        || lower.ends_with(".gif")
        || lower.ends_with(".webp")
}

pub fn replace_filename_ext(filename: &str, new_ext: &str) -> String {
    if new_ext.is_empty() {
        return filename.to_string();
    }
    match filename.rfind('.') {
        Some(dot) => format!("{}{}", &filename[..dot], new_ext),
        None => format!("{}{}", filename, new_ext),
    }
}

pub fn sanitize_upload_path(orig_name: &str, is_image: bool, existing_paths: &[String]) -> String {
    let raw_base = orig_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim();
    let (raw_stem, ext_with_dot) = match raw_base.rfind('.') {
        Some(dot) if dot > 0 => {
            let ext = &raw_base[dot + 1..];
            if !ext.is_empty() && ext.len() <= 10 && ext.chars().all(|c| c.is_ascii_alphanumeric())
            {
                (&raw_base[..dot], format!(".{}", ext.to_ascii_lowercase()))
            } else {
                (raw_base, String::new())
            }
        }
        _ => (raw_base, String::new()),
    };

    let mut clean = String::new();
    let mut prev_us = false;
    for ch in raw_stem.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' {
            clean.push(ch);
            prev_us = false;
        } else if ch == '_' {
            if !prev_us && !clean.is_empty() {
                clean.push('_');
                prev_us = true;
            }
        } else if !prev_us && !clean.is_empty() {
            clean.push('_');
            prev_us = true;
        }
    }
    let clean = clean.trim_matches(['_', '-', '.']);
    let mut stem = if clean.is_empty() || !clean.as_bytes()[0].is_ascii_alphanumeric() {
        let seq = CLIPBOARD_SEQ.fetch_add(1, Ordering::Relaxed);
        let stamp = Local::now().format("%Y%m%d-%H%M%S");
        format!("{}-{}-{}", if is_image { "image" } else { "file" }, stamp, seq)
    } else {
        clean.to_string()
    };

    let max_stem = 100usize.saturating_sub(ext_with_dot.len()).max(8);
    if stem.len() > max_stem {
        stem.truncate(max_stem);
        let trimmed = stem.trim_end_matches(['_', '-']).to_string();
        if !trimmed.is_empty() {
            stem = trimmed;
        }
    }

    let taken = |cand: &str| {
        existing_paths
            .iter()
            .any(|p| p.eq_ignore_ascii_case(cand))
    };
    let primary = format!("uploads/{}{}", stem, ext_with_dot);
    if !taken(&primary) {
        return primary;
    }
    for n in 2..=999usize {
        let suffix = format!("-{}", n);
        let base_cap = 100usize.saturating_sub(ext_with_dot.len() + suffix.len()).max(4);
        let base_part = if stem.len() > base_cap {
            stem[..base_cap].trim_end_matches(['_', '-'])
        } else {
            stem.as_str()
        };
        let cand = format!("uploads/{}{}{}", base_part, suffix, ext_with_dot);
        if !taken(&cand) {
            return cand;
        }
    }
    format!(
        "uploads/{}-{}{}",
        if is_image { "image" } else { "file" },
        CLIPBOARD_SEQ.fetch_add(1, Ordering::Relaxed),
        ext_with_dot
    )
}

#[allow(dead_code)]
pub fn build_attachment_from_file(path: &Path) -> Result<Value, String> {
    build_attachment_from_file_with(path, &[], MAX_ATTACHMENT_BYTES)
}

pub fn build_attachment_from_file_with(
    path: &Path,
    existing_paths: &[String],
    budget_bytes: usize,
) -> Result<Value, String> {
    no_links(path)?;
    let meta = fs::metadata(path).map_err(|_| {
        format!(
            "添付ファイルを読めません: {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        )
    })?;
    if !meta.is_file() {
        return Err("フォルダーではなくファイルを選択してください".into());
    }
    let orig_name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "attachment.bin".into());
    let effective_budget = budget_bytes.min(MAX_ATTACHMENT_BYTES);
    if effective_budget == 0 {
        return Err("添付ファイルの合計は 2 MB までです".into());
    }
    let is_image = is_image_filename(&orig_name);
    let max_read = if is_image {
        MAX_READABLE_IMAGE_FILE_BYTES
    } else {
        effective_budget as u64
    };
    if meta.len() > max_read {
        return Err(if is_image {
            format!("画像ファイル {} が大きすぎます (最大 32 MB)", orig_name)
        } else {
            format!(
                "ファイル {} が大きすぎます (残り容量 {} KB)",
                orig_name,
                (effective_budget / 1024).max(1)
            )
        });
    }
    let raw = fs::read(path).map_err(|_| format!("添付ファイルを読めません: {}", orig_name))?;
    if !is_image {
        let upload_path = sanitize_upload_path(&orig_name, false, existing_paths);
        return Ok(json!({
            "path": upload_path,
            "displayName": orig_name,
            "base64": B64.encode(&raw),
            "size": raw.len(),
            "kind": "file",
            "sourcePath": path.to_string_lossy(),
        }));
    }

    let (final_bytes, final_name) = if raw.len() <= effective_budget
        && !orig_name.to_ascii_lowercase().ends_with(".bmp")
    {
        (raw, orig_name.clone())
    } else if let Some((compressed, ext)) = compress_image_bytes_to_budget(&raw, effective_budget) {
        let name = replace_filename_ext(&orig_name, ext);
        (compressed, name)
    } else if raw.len() <= effective_budget {
        (raw, orig_name.clone())
    } else {
        return Err(format!("画像 {} を 2 MB 以内に圧縮できませんでした", orig_name));
    };
    let upload_path = sanitize_upload_path(&final_name, true, existing_paths);
    let preview = make_thumbnail_data_url(&final_bytes, 220);
    Ok(json!({
        "path": upload_path,
        "displayName": orig_name,
        "base64": B64.encode(&final_bytes),
        "size": final_bytes.len(),
        "kind": "image",
        "previewUrl": preview.unwrap_or_default(),
        "sourcePath": path.to_string_lossy(),
    }))
}

pub fn build_attachment_from_png(
    png: &[u8],
    existing_paths: &[String],
    budget_bytes: usize,
) -> Result<Value, String> {
    let effective_budget = budget_bytes.min(MAX_ATTACHMENT_BYTES);
    if effective_budget == 0 {
        return Err("添付ファイルの合計は 2 MB までです".into());
    }
    let (final_bytes, ext) = if png.len() <= effective_budget {
        (png.to_vec(), ".png")
    } else {
        compress_image_bytes_to_budget(png, effective_budget)
            .ok_or_else(|| "クリップボード画像を 2 MB 以内に圧縮できませんでした".to_string())?
    };
    let seq = CLIPBOARD_SEQ.fetch_add(1, Ordering::Relaxed);
    let stamp = Local::now().format("%Y%m%d-%H%M%S");
    let filename = format!("clipboard-{}-{}{}", stamp, seq, ext);
    let upload_path = sanitize_upload_path(&filename, true, existing_paths);
    let preview = make_thumbnail_data_url(&final_bytes, 220);
    Ok(json!({
        "path": upload_path,
        "displayName": filename,
        "base64": B64.encode(&final_bytes),
        "size": final_bytes.len(),
        "kind": "image",
        "previewUrl": preview.unwrap_or_default(),
    }))
}

pub fn build_attachment_from_dib(
    dib: &[u8],
    existing_paths: &[String],
    budget_bytes: usize,
) -> Result<Value, String> {
    let png = dib_bytes_to_png(dib)?;
    build_attachment_from_png(&png, existing_paths, budget_bytes)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadStageState {
    Idle,
    Uploading,
    Uploaded,
    Failed,
}

impl UploadStageState {
    pub fn as_str(self) -> &'static str {
        match self {
            UploadStageState::Idle => "idle",
            UploadStageState::Uploading => "uploading",
            UploadStageState::Uploaded => "uploaded",
            UploadStageState::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug)]
struct UploadEntry {
    upload_id: String,
    state: UploadStageState,
    removed: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ResolvedSendAttachments {
    pub upload_ids: Vec<String>,
    pub inline_files: Vec<Value>,
}

type TestTransport = Arc<
    dyn Fn(&str, &str, Option<&Value>, u64, &str) -> Result<Value, String> + Send + Sync,
>;

struct StagedShared {
    mutex: Mutex<StagedInner>,
    cv: Condvar,
    active: AtomicUsize,
    closing: AtomicBool,
}

struct StagedInner {
    entries: HashMap<String, UploadEntry>,
    inflight: HashSet<usize>,
    clients: HashMap<usize, Arc<ApiClient>>,
    next_id: usize,
    transport: Option<TestTransport>,
}

#[derive(Clone)]
pub struct StagedUploadManager {
    shared: Arc<StagedShared>,
}

impl Default for StagedUploadManager {
    fn default() -> Self {
        Self::new()
    }
}

impl StagedUploadManager {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(StagedShared {
                mutex: Mutex::new(StagedInner {
                    entries: HashMap::new(),
                    inflight: HashSet::new(),
                    clients: HashMap::new(),
                    next_id: 1,
                    transport: None,
                }),
                cv: Condvar::new(),
                active: AtomicUsize::new(0),
                closing: AtomicBool::new(false),
            }),
        }
    }

    #[cfg(test)]
    pub fn set_test_transport(&self, transport: TestTransport) {
        self.shared.mutex.lock().transport = Some(transport);
    }

    pub fn stage_status_map(&self) -> HashMap<String, &'static str> {
        self.shared
            .mutex
            .lock()
            .entries
            .iter()
            .filter(|(_, e)| !e.removed)
            .map(|(k, e)| (k.clone(), e.state.as_str()))
            .collect()
    }

    fn perform(
        shared: &Arc<StagedShared>,
        url: &str,
        path: &str,
        body: Option<&Value>,
        timeout_ms: u64,
        method: &str,
    ) -> Result<Value, String> {
        let transport_opt = { shared.mutex.lock().transport.clone() };
        if let Some(t) = transport_opt {
            return t(url, path, body, timeout_ms, method);
        }
        let client = Arc::new(ApiClient::new(url)?);
        let id = {
            let mut guard = shared.mutex.lock();
            if shared.closing.load(Ordering::SeqCst) {
                return Err("closing".into());
            }
            let id = guard.next_id;
            guard.next_id += 1;
            guard.inflight.insert(id);
            guard.clients.insert(id, Arc::clone(&client));
            id
        };
        let res = client.request(path, body, timeout_ms, Some(method));
        let mut guard = shared.mutex.lock();
        guard.inflight.remove(&id);
        guard.clients.remove(&id);
        res
    }

    fn fire_delete(shared: &Arc<StagedShared>, api_url: String, upload_id: String) {
        if shared.closing.load(Ordering::SeqCst) || api_url.is_empty() || upload_id.is_empty() {
            return;
        }
        shared.active.fetch_add(1, Ordering::SeqCst);
        let s = Arc::clone(shared);
        std::thread::spawn(move || {
            let path = format!("/uploads/{}", upload_id);
            let _ = Self::perform(&s, &api_url, &path, None, STAGED_DELETE_TIMEOUT_MS, "DELETE");
            if s.active.fetch_sub(1, Ordering::SeqCst) == 1 {
                s.cv.notify_all();
            }
        });
    }

    pub fn start_uploads<F>(&self, attached_files: &[Value], api_url: &str, on_done: Option<F>)
    where
        F: Fn() + Send + Sync + 'static,
    {
        if self.shared.closing.load(Ordering::SeqCst) || api_url.is_empty() {
            return;
        }
        let mut todo: Vec<(String, String)> = Vec::new();
        {
            let mut guard = self.shared.mutex.lock();
            for f in attached_files {
                let path = f.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let b64 = f.get("base64").and_then(|v| v.as_str()).unwrap_or("");
                if path.is_empty() || b64.is_empty() {
                    continue;
                }
                let entry = guard.entries.entry(path.to_string()).or_insert(UploadEntry {
                    upload_id: String::new(),
                    state: UploadStageState::Idle,
                    removed: false,
                });
                if entry.state == UploadStageState::Idle
                    || entry.removed
                    || entry.state == UploadStageState::Failed
                {
                    *entry = UploadEntry {
                        upload_id: String::new(),
                        state: UploadStageState::Uploading,
                        removed: false,
                    };
                    todo.push((path.to_string(), b64.to_string()));
                }
            }
        }
        if todo.is_empty() {
            return;
        }
        let cb: Option<Arc<dyn Fn() + Send + Sync>> = on_done.map(|f| Arc::new(f) as _);
        for (path, b64) in todo {
            let s = Arc::clone(&self.shared);
            let url = api_url.to_string();
            let cb_clone = cb.clone();
            s.active.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || {
                let body = json!({ "files": [{ "path": path, "base64": b64 }] });
                let res = Self::perform(&s, &url, "/uploads", Some(&body), STAGED_UPLOAD_WAIT_MS, "POST");
                let mut orphan_id: Option<String> = None;
                match res {
                    Ok(reply) => {
                        let id = reply
                            .get("uploaded")
                            .and_then(|v| v.as_array())
                            .and_then(|a| a.first())
                            .and_then(|x| x.get("id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let mut guard = s.mutex.lock();
                        if s.closing.load(Ordering::SeqCst) || id.is_empty() {
                            if !id.is_empty() {
                                orphan_id = Some(id);
                            }
                            if let Some(entry) = guard.entries.get_mut(&path) {
                                if entry.removed {
                                    guard.entries.remove(&path);
                                } else {
                                    entry.state = UploadStageState::Failed;
                                }
                            }
                        } else if let Some(entry) = guard.entries.get_mut(&path) {
                            if entry.removed {
                                orphan_id = Some(id);
                                guard.entries.remove(&path);
                            } else {
                                *entry = UploadEntry {
                                    upload_id: id,
                                    state: UploadStageState::Uploaded,
                                    removed: false,
                                };
                            }
                        } else {
                            orphan_id = Some(id);
                        }
                    }
                    Err(err) => {
                        diag_line(&format!("staged upload failed ({}): {}", path, err));
                        let mut guard = s.mutex.lock();
                        if let Some(entry) = guard.entries.get_mut(&path) {
                            if entry.removed {
                                guard.entries.remove(&path);
                            } else {
                                entry.state = UploadStageState::Failed;
                            }
                        }
                    }
                }
                if let Some(id) = orphan_id {
                    Self::fire_delete(&s, url, id);
                }
                s.cv.notify_all();
                if !s.closing.load(Ordering::SeqCst) {
                    if let Some(f) = cb_clone {
                        f();
                    }
                }
                if s.active.fetch_sub(1, Ordering::SeqCst) == 1 {
                    s.cv.notify_all();
                }
            });
        }
    }

    pub fn remove_path(&self, path: &str, api_url: &str) {
        if path.is_empty() {
            return;
        }
        let mut delete_id = None;
        {
            let mut guard = self.shared.mutex.lock();
            if let Some(entry) = guard.entries.get_mut(path) {
                if entry.state == UploadStageState::Uploaded && !entry.upload_id.is_empty() {
                    delete_id = Some(entry.upload_id.clone());
                    guard.entries.remove(path);
                } else if entry.state == UploadStageState::Uploading {
                    entry.removed = true;
                } else {
                    guard.entries.remove(path);
                }
            }
        }
        self.shared.cv.notify_all();
        if let Some(id) = delete_id {
            Self::fire_delete(&self.shared, api_url.to_string(), id);
        }
    }

    pub fn clear_all(&self, api_url: &str) {
        let mut delete_ids = Vec::new();
        {
            let mut guard = self.shared.mutex.lock();
            let keys: Vec<String> = guard.entries.keys().cloned().collect();
            for k in keys {
                if let Some(entry) = guard.entries.get_mut(&k) {
                    if entry.state == UploadStageState::Uploaded && !entry.upload_id.is_empty() {
                        delete_ids.push(entry.upload_id.clone());
                        guard.entries.remove(&k);
                    } else if entry.state == UploadStageState::Uploading {
                        entry.removed = true;
                    } else {
                        guard.entries.remove(&k);
                    }
                }
            }
        }
        self.shared.cv.notify_all();
        for id in delete_ids {
            Self::fire_delete(&self.shared, api_url.to_string(), id);
        }
    }

    pub fn resolve_for_send(&self, files: &[Value], wait_ms: u64) -> ResolvedSendAttachments {
        let mut out = ResolvedSendAttachments::default();
        if files.is_empty() {
            return out;
        }
        let deadline = Instant::now() + Duration::from_millis(wait_ms);
        let mut guard = self.shared.mutex.lock();
        while !self.shared.closing.load(Ordering::SeqCst) {
            let still_uploading = files.iter().any(|f| {
                let path = f.get("path").and_then(|v| v.as_str()).unwrap_or("");
                guard
                    .entries
                    .get(path)
                    .map(|e| e.state == UploadStageState::Uploading)
                    .unwrap_or(false)
            });
            if !still_uploading {
                break;
            }
            if self.shared.cv.wait_until(&mut guard, deadline).timed_out() {
                break;
            }
        }
        for f in files {
            let path = f.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let mut used_id = None;
            if let Some(entry) = guard.entries.get_mut(&path) {
                if entry.state == UploadStageState::Uploaded && !entry.upload_id.is_empty() {
                    used_id = Some(entry.upload_id.clone());
                } else if entry.state == UploadStageState::Uploading {
                    entry.removed = true;
                }
            }
            if let Some(id) = used_id {
                guard.entries.remove(&path);
                out.upload_ids.push(id);
            } else {
                if guard
                    .entries
                    .get(&path)
                    .map(|e| e.state != UploadStageState::Uploading)
                    .unwrap_or(false)
                {
                    guard.entries.remove(&path);
                }
                let b64 = f.get("base64").and_then(|v| v.as_str()).unwrap_or("");
                out.inline_files.push(json!({ "path": path, "base64": b64 }));
            }
        }
        out
    }

    pub fn shutdown(&self) {
        self.shared.closing.store(true, Ordering::SeqCst);
        let clients: Vec<Arc<ApiClient>> = {
            let guard = self.shared.mutex.lock();
            guard.clients.values().cloned().collect()
        };
        for c in clients {
            c.cancel_io();
        }
        self.shared.cv.notify_all();
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut guard = self.shared.mutex.lock();
        while self.shared.active.load(Ordering::SeqCst) > 0 {
            if self.shared.cv.wait_until(&mut guard, deadline).timed_out() {
                break;
            }
        }
    }
}

impl Drop for StagedUploadManager {
    fn drop(&mut self) {
        if Arc::strong_count(&self.shared) == 1 {
            self.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staged_upload_resolves_uploaded_ids_and_cleans_orphan() {
        let mgr = StagedUploadManager::new();
        let calls = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let calls2 = Arc::clone(&calls);
        mgr.set_test_transport(Arc::new(move |_url, path, _body, _t, method| {
            calls2.lock().push((method.to_string(), path.to_string()));
            if method == "POST" {
                Ok(json!({ "uploaded": [{ "id": "u-123" }] }))
            } else {
                Ok(json!({ "ok": true }))
            }
        }));
        let files = vec![json!({ "path": "a.png", "base64": "AAAA" })];
        mgr.start_uploads::<fn()>(&files, "http://127.0.0.1:8090", None);
        let res = mgr.resolve_for_send(&files, 2000);
        assert_eq!(res.upload_ids, vec!["u-123"]);
        assert!(res.inline_files.is_empty());
        assert_eq!(calls.lock().len(), 1);
    }

    #[test]
    fn remove_while_uploading_deletes_orphan_on_completion() {
        let mgr = StagedUploadManager::new();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let deleted = Arc::new((Mutex::new(Vec::<String>::new()), Condvar::new()));
        let gate2 = Arc::clone(&gate);
        let del2 = Arc::clone(&deleted);
        mgr.set_test_transport(Arc::new(move |_url, path, _body, _t, method| {
            if method == "POST" {
                let (m, cv) = &*gate2;
                let mut g = m.lock();
                while !*g {
                    cv.wait(&mut g);
                }
                Ok(json!({ "uploaded": [{ "id": "u-orphan" }] }))
            } else {
                del2.0.lock().push(path.to_string());
                del2.1.notify_all();
                Ok(json!({ "ok": true }))
            }
        }));
        let files = vec![json!({ "path": "b.png", "base64": "BBBB" })];
        mgr.start_uploads::<fn()>(&files, "http://127.0.0.1:8090", None);
        std::thread::sleep(Duration::from_millis(30));
        mgr.remove_path("b.png", "http://127.0.0.1:8090");
        {
            let (m, cv) = &*gate;
            *m.lock() = true;
            cv.notify_all();
        }
        let (dm, dcv) = &*deleted;
        let mut dg = dm.lock();
        let dl = Instant::now() + Duration::from_secs(2);
        while dg.is_empty() && !dcv.wait_until(&mut dg, dl).timed_out() {}
        assert_eq!(*dg, vec!["/uploads/u-orphan".to_string()]);
    }

    #[test]
    fn sanitize_upload_path_matches_server_rule_and_dedups() {
        let valid_server_path = |p: &str| {
            let Some(rest) = p.strip_prefix("uploads/") else { return false };
            !rest.is_empty()
                && rest.len() <= 121
                && rest.as_bytes()[0].is_ascii_alphanumeric()
                && rest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        };

        let p1 = sanitize_upload_path("スクリーンショット 2026-11-23.png", true, &[]);
        assert!(valid_server_path(&p1), "invalid path: {}", p1);
        assert!(p1.ends_with(".png"));

        let p2 = sanitize_upload_path("スクリーンショット.png", true, &[]);
        assert!(valid_server_path(&p2), "invalid fallback path: {}", p2);
        assert!(p2.starts_with("uploads/image-"));

        let p3 = sanitize_upload_path("my photo (final).JPEG", true, &[]);
        assert_eq!(p3, "uploads/my_photo_final.jpeg");
        assert!(valid_server_path(&p3));

        let p4 = sanitize_upload_path(
            "my photo (final).JPEG",
            true,
            &["uploads/my_photo_final.jpeg".to_string()],
        );
        assert_eq!(p4, "uploads/my_photo_final-2.jpeg");
        assert!(valid_server_path(&p4));
    }

    #[test]
    fn build_attachment_from_png_sets_uploads_prefix_and_enforces_budget() {
        let minimal_png = [
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H',
            b'D', b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, b'I', b'D', b'A', b'T', 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82,
        ];
        let att = build_attachment_from_png(&minimal_png, &[], MAX_ATTACHMENT_BYTES).unwrap();
        let path = att["path"].as_str().unwrap();
        assert!(path.starts_with("uploads/clipboard-"));
        assert!(att["displayName"].as_str().unwrap().starts_with("clipboard-"));
        assert_eq!(att["kind"].as_str(), Some("image"));
        assert!(build_attachment_from_png(&minimal_png, &[], 0).is_err());
    }
}
