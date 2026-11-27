use crate::api_cli::build_cli_desktop_tools;
use crate::config::find_window_match;
use crate::desktop_worker::DesktopWorkerProcess;
use crate::tls_http::ApiClient;
use crate::win_util::{computer_name, desktop_windows, is_valid_window};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub enum StandbyNotice {
    Log(String),
    State(String),
    Jobs(Value),
    Conversation(Value),
    StandbyEnded { error: String },
}

pub fn desktop_action_label(tool: &str) -> &'static str {
    match tool {
        "local.run" => "このPCでPowerShellを実行しています…",
        t if t.starts_with("local.") => "このPCの作業ファイルを操作しています…",
        "desktop.observe" => "PCの操作要素を確認しています…",
        "desktop.screenshot" => "選択したウィンドウの画面を撮影しています…",
        "desktop.video" => "選択したウィンドウの動画を撮影しています…",
        "desktop.click" | "desktop.click_at" => "選択したウィンドウをクリックしています…",
        "desktop.drag" => "選択したウィンドウをドラッグしています…",
        "desktop.type" => "選択したウィンドウに入力しています…",
        "desktop.key" | "desktop.hold_key" => "選択したウィンドウにキー入力しています…",
        "desktop.sequence" => "選択したウィンドウを連続操作しています…",
        "desktop.sustain" => "選択したウィンドウの継続入力を切り替えています…",
        "desktop.release" => "選択したウィンドウの継続入力を解除しています…",
        _ => "選択したウィンドウを操作しています…",
    }
}

pub fn standby_action_label(tool: &str) -> &'static str {
    match tool {
        "local.run" => "連携待機 · コマンドを実行しています…",
        t if t.starts_with("local.") => "連携待機 · 作業ファイルを操作しています…",
        "desktop.screenshot" => "連携待機 · 選択ウィンドウを撮影しています…",
        "desktop.video" => "連携待機 · 選択ウィンドウの動画を撮影しています…",
        "desktop.click" | "desktop.click_at" => "連携待機 · クリックしています…",
        "desktop.drag" => "連携待機 · ドラッグ操作しています…",
        "desktop.type" => "連携待機 · 入力しています…",
        "desktop.key" | "desktop.hold_key" => "連携待機 · キー操作しています…",
        "desktop.sustain" => "連携待機 · 継続入力を切り替えています…",
        "desktop.release" => "連携待機 · 継続入力を解除しています…",
        _ => "連携待機 · PC操作を実行しています…",
    }
}

pub struct JobDesktopBridge {
    stop: Arc<AtomicBool>,
    poll_api: Option<Arc<ApiClient>>,
    thread: Option<JoinHandle<()>>,
    api_url: String,
    session_id: String,
    token: String,
}

impl JobDesktopBridge {
    pub fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            poll_api: None,
            thread: None,
            api_url: String::new(),
            session_id: String::new(),
            token: String::new(),
        }
    }

    pub fn open_and_start<F, K>(
        &mut self,
        api_url: &str,
        hwnd: isize,
        win_title: &str,
        workspace: PathBuf,
        read_roots: Vec<PathBuf>,
        desktop_allowed: bool,
        local_allowed: bool,
        is_keep_alive: K,
        emit_state: F,
    ) -> Result<String, String>
    where
        F: Fn(StandbyNotice) + Send + Sync + 'static,
        K: Fn() -> bool + Send + Sync + 'static,
    {
        self.stop_and_close();
        let client = ApiClient::new(api_url)?;
        let mut sbody = json!({ "tools": build_cli_desktop_tools(desktop_allowed, local_allowed) });
        if !win_title.is_empty() {
            sbody["windowTitle"] = json!(win_title);
        }
        let session = client.post("/desktop/sessions", &sbody, 15000)?;
        let sid = session
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("desktop session id missing")?
            .to_string();
        let token = session
            .get("token")
            .and_then(|v| v.as_str())
            .ok_or("desktop session token missing")?
            .to_string();

        self.api_url = api_url.to_string();
        self.session_id = sid.clone();
        self.token = token.clone();
        self.stop.store(false, Ordering::SeqCst);
        let poll_api = Arc::new(ApiClient::new(api_url)?);
        self.poll_api = Some(Arc::clone(&poll_api));

        let stop = Arc::clone(&self.stop);
        let sid_c = sid.clone();
        self.thread = Some(thread::spawn(move || {
            let mut worker = DesktopWorkerProcess::new(hwnd, workspace, read_roots);
            let mut last_err = String::new();
            while !stop.load(Ordering::SeqCst) && is_keep_alive() {
                let res = poll_api.post(
                    &format!("/desktop/{}/poll", sid_c),
                    &json!({ "token": token, "waitMs": 15000 }),
                    25000,
                );
                let poll = match res {
                    Ok(v) => {
                        last_err.clear();
                        v
                    }
                    Err(e) => {
                        if stop.load(Ordering::SeqCst) || !is_keep_alive() {
                            break;
                        }
                        if last_err != e {
                            last_err = e.clone();
                            emit_state(StandbyNotice::Log(format!(
                                "注意: PC操作ポーリングで一時的遅延: {}\r\n",
                                e
                            )));
                        }
                        for _ in 0..4 {
                            if stop.load(Ordering::SeqCst) || !is_keep_alive() {
                                break;
                            }
                            thread::sleep(Duration::from_millis(30));
                        }
                        continue;
                    }
                };
                if stop.load(Ordering::SeqCst) || !is_keep_alive() {
                    break;
                }
                let Some(action) = poll.get("action").filter(|v| !v.is_null()) else {
                    continue;
                };
                let action_id = action.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let tool = action.get("tool").and_then(|v| v.as_str()).unwrap_or("");
                emit_state(StandbyNotice::State(desktop_action_label(tool).to_string()));
                let result = if (tool.starts_with("local.") && !local_allowed)
                    || (tool.starts_with("desktop.") && !desktop_allowed)
                {
                    json!({ "error": "このPC操作は許可されていません" })
                } else {
                    match worker.perform(
                        action,
                        &stop,
                        |output| {
                            let _ = poll_api.post(
                                &format!("/desktop/{}/progress", sid_c),
                                &json!({ "token": token, "actionId": action_id, "output": output }),
                                5000,
                            );
                        },
                        &is_keep_alive,
                    ) {
                        Ok(v) => v,
                        Err(e) => json!({ "error": e }),
                    }
                };
                if !stop.load(Ordering::SeqCst) {
                    let _ = poll_api.post(
                        &format!("/desktop/{}/result", sid_c),
                        &json!({ "token": token, "actionId": action_id, "result": result }),
                        10000,
                    );
                }
            }
        }));
        Ok(sid)
    }

    pub fn cancel_io(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(ref api) = self.poll_api {
            api.cancel_io();
        }
    }

    pub fn stop_and_close(&mut self) {
        self.cancel_io();
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
        self.poll_api = None;
        if !self.session_id.is_empty() && !self.api_url.is_empty() {
            if let Ok(cleanup) = ApiClient::new(&self.api_url) {
                let _ = cleanup.post(
                    &format!("/desktop/{}/close", self.session_id),
                    &json!({ "token": self.token }),
                    2000,
                );
            }
            self.session_id.clear();
            self.token.clear();
        }
    }
}

impl Drop for JobDesktopBridge {
    fn drop(&mut self) {
        self.stop_and_close();
    }
}

#[derive(Clone, Default)]
pub struct SyncCursor {
    pub conv_id: String,
    pub job_id: String,
    pub job_status: String,
    pub busy: bool,
}

pub struct StandbyController {
    active: Arc<AtomicBool>,
    api: Option<Arc<ApiClient>>,
    thread: Option<JoinHandle<()>>,
    sync_cursor: Arc<Mutex<SyncCursor>>,
}

impl StandbyController {
    pub fn new() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(false)),
            api: None,
            thread: None,
            sync_cursor: Arc::new(Mutex::new(SyncCursor::default())),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    pub fn update_sync_cursor(&self, conv_id: &str, job_id: &str, job_status: &str, busy: bool) {
        let mut g = self.sync_cursor.lock();
        g.conv_id = conv_id.to_string();
        g.job_id = job_id.to_string();
        g.job_status = job_status.to_string();
        g.busy = busy;
    }

    pub fn start<F>(
        &mut self,
        api_url: &str,
        target_hwnd: isize,
        win_title: &str,
        workspace: PathBuf,
        read_roots: Vec<PathBuf>,
        allow_local: bool,
        emit: F,
    ) -> Result<(), String>
    where
        F: Fn(StandbyNotice) + Send + Sync + 'static,
    {
        self.stop();
        let standby_api = Arc::new(ApiClient::new(api_url)?);
        self.api = Some(Arc::clone(&standby_api));
        self.active.store(true, Ordering::SeqCst);

        let active = Arc::clone(&self.active);
        let url = api_url.to_string();
        let title = win_title.to_string();
        let comp_name = computer_name();
        let cursor = Arc::clone(&self.sync_cursor);

        self.thread = Some(thread::spawn(move || {
            let mut current_target = target_hwnd;
            let mut sid = String::new();
            let mut stoken = String::new();
            let mut err_msg = String::new();
            let mut dw: Option<DesktopWorkerProcess> = None;

            let sleep_active = |ms: u64| {
                let mut elapsed = 0u64;
                while elapsed < ms && active.load(Ordering::SeqCst) {
                    let step = (ms - elapsed).min(100);
                    thread::sleep(Duration::from_millis(step));
                    elapsed += step;
                }
            };

            let close_session = |sid: &mut String, stoken: &mut String| {
                if sid.is_empty() {
                    return;
                }
                if let Ok(cleanup) = ApiClient::new(&url) {
                    let _ = cleanup.post(
                        "/desktop/shared",
                        &json!({ "enabled": false, "id": &*sid, "token": &*stoken }),
                        2000,
                    );
                    let _ = cleanup.post(
                        &format!("/desktop/{}/close", sid),
                        &json!({ "token": &*stoken }),
                        2000,
                    );
                }
                sid.clear();
                stoken.clear();
            };

            let ensure_worker = |current_target: &mut isize,
                                 dw: &mut Option<DesktopWorkerProcess>|
             -> bool {
                if *current_target == 0 || !is_valid_window(*current_target) {
                    *current_target = 0;
                    if !title.is_empty() {
                        let list: Vec<Value> = desktop_windows()
                            .into_iter()
                            .map(|w| serde_json::to_value(w).unwrap())
                            .collect();
                        if let Some(found) = find_window_match(&list, &title) {
                            *current_target =
                                found.get("id").and_then(|v| v.as_i64()).unwrap_or(0) as isize;
                        }
                    }
                    *dw = None;
                }
                if *current_target == 0 {
                    return false;
                }
                if dw.is_none() {
                    *dw = Some(DesktopWorkerProcess::new(
                        *current_target,
                        workspace.clone(),
                        read_roots.clone(),
                    ));
                }
                true
            };

            let register_session =
                |sid: &mut String, stoken: &mut String| -> Result<(), String> {
                    let sbody = json!({
                        "tools": build_cli_desktop_tools(true, allow_local),
                        "windowTitle": title,
                    });
                    let session = standby_api.post("/desktop/sessions", &sbody, 15000)?;
                    let new_id = session
                        .get("id")
                        .and_then(|v| v.as_str())
                        .ok_or("session id missing")?
                        .to_string();
                    let new_tok = session
                        .get("token")
                        .and_then(|v| v.as_str())
                        .ok_or("session token missing")?
                        .to_string();
                    let reg = json!({
                        "enabled": true,
                        "id": new_id,
                        "token": new_tok,
                        "windowTitle": title,
                        "computerName": comp_name,
                    });
                    standby_api.post("/desktop/shared", &reg, 10000)?;
                    *sid = new_id;
                    *stoken = new_tok;
                    Ok(())
                };

            let run_res = (|| -> Result<(), String> {
                if !ensure_worker(&mut current_target, &mut dw) {
                    return Err("対象ウィンドウが見つかりません".into());
                }
                register_session(&mut sid, &mut stoken)?;
                emit(StandbyNotice::Log(format!(
                    "PC連携待機を開始しました: {}\r\n",
                    title
                )));
                emit(StandbyNotice::State(
                    "PC連携待機中 · スマホや外部からの操作を受け付けます".into(),
                ));

                let mut next_sync = Instant::now();
                let mut consecutive_errors = 0u32;
                let mut backoff_ms = 1000u64;

                while active.load(Ordering::SeqCst) {
                    if !ensure_worker(&mut current_target, &mut dw) {
                        close_session(&mut sid, &mut stoken);
                        emit(StandbyNotice::State(
                            "PC連携待機 · 対象ウィンドウの再起動を待っています…".into(),
                        ));
                        sleep_active(3000);
                        continue;
                    }
                    if sid.is_empty() {
                        match register_session(&mut sid, &mut stoken) {
                            Ok(()) => {
                                backoff_ms = 1000;
                                consecutive_errors = 0;
                                emit(StandbyNotice::Log(format!(
                                    "PC連携待機セッションを再登録しました: {}\r\n",
                                    title
                                )));
                                emit(StandbyNotice::State(
                                    "PC連携待機中 · スマホや外部からの操作を受け付けます".into(),
                                ));
                            }
                            Err(_) => {
                                if !active.load(Ordering::SeqCst) {
                                    break;
                                }
                                sleep_active(backoff_ms);
                                backoff_ms = (backoff_ms * 2).min(10_000);
                                continue;
                            }
                        }
                    }

                    let poll_res = standby_api.post(
                        &format!("/desktop/{}/poll", sid),
                        &json!({ "token": stoken, "waitMs": 15000 }),
                        25000,
                    );
                    let poll = match poll_res {
                        Ok(v) => {
                            consecutive_errors = 0;
                            backoff_ms = 1000;
                            v
                        }
                        Err(msg) => {
                            if !active.load(Ordering::SeqCst) {
                                break;
                            }
                            let session_invalid = msg.contains("PC操作セッションが無効です")
                                || msg.contains("Invalid PC session");
                            consecutive_errors += 1;
                            if session_invalid || consecutive_errors >= 3 {
                                emit(StandbyNotice::Log(format!(
                                    "注意: PC連携待機セッションを再接続します: {}\r\n",
                                    msg
                                )));
                                close_session(&mut sid, &mut stoken);
                                sleep_active(backoff_ms);
                                backoff_ms = (backoff_ms * 2).min(10_000);
                                continue;
                            }
                            sleep_active(500);
                            continue;
                        }
                    };

                    let mut did_work = false;
                    if let Some(action) = poll.get("action").filter(|v| !v.is_null()) {
                        did_work = true;
                        let aid = action.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let tool = action.get("tool").and_then(|v| v.as_str()).unwrap_or("");
                        emit(StandbyNotice::State(standby_action_label(tool).to_string()));
                        let res = if let Some(ref mut worker) = dw {
                            let stop_flag = AtomicBool::new(false);
                            match worker.perform(
                                action,
                                &stop_flag,
                                |output| {
                                    let _ = standby_api.post(
                                        &format!("/desktop/{}/progress", sid),
                                        &json!({ "token": stoken, "actionId": aid, "output": output }),
                                        5000,
                                    );
                                },
                                || active.load(Ordering::SeqCst),
                            ) {
                                Ok(v) => v,
                                Err(e) => json!({ "error": e }),
                            }
                        } else {
                            json!({ "error": "PC操作ワーカーがありません" })
                        };
                        if active.load(Ordering::SeqCst) {
                            if let Err(e) = standby_api.post(
                                &format!("/desktop/{}/result", sid),
                                &json!({ "token": stoken, "actionId": aid, "result": res }),
                                10000,
                            ) {
                                if e.contains("PC操作セッションが無効です") {
                                    close_session(&mut sid, &mut stoken);
                                }
                            }
                        }
                    }

                    if active.load(Ordering::SeqCst)
                        && (did_work || Instant::now() >= next_sync)
                    {
                        next_sync = Instant::now() + Duration::from_secs(4);
                        if let Ok(convs) = standby_api.get("/conversations", 10000) {
                            emit(StandbyNotice::Jobs(convs.clone()));
                            let cur = cursor.lock().clone();
                            if !cur.conv_id.is_empty() && !cur.busy {
                                if let Some(arr) = convs.as_array() {
                                    if let Some(row) = arr.iter().find(|r| {
                                        r.get("id").and_then(|v| v.as_str()) == Some(&cur.conv_id)
                                    }) {
                                        let latest = row
                                            .get("latestJobId")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        let st = row
                                            .get("status")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        if latest != cur.job_id || st != cur.job_status {
                                            if let Ok(conv) = standby_api.get(
                                                &format!("/conversations/{}", cur.conv_id),
                                                10000,
                                            ) {
                                                if !cursor.lock().busy
                                                    && active.load(Ordering::SeqCst)
                                                {
                                                    emit(StandbyNotice::Conversation(conv));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(())
            })();

            if let Err(e) = run_res {
                if active.load(Ordering::SeqCst) {
                    err_msg = e;
                }
            }
            drop(dw);
            close_session(&mut sid, &mut stoken);
            active.store(false, Ordering::SeqCst);
            emit(StandbyNotice::StandbyEnded { error: err_msg });
        }));
        Ok(())
    }

    pub fn stop(&mut self) {
        self.active.store(false, Ordering::SeqCst);
        if let Some(ref api) = self.api {
            api.cancel_io();
        }
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
        self.api = None;
    }
}

impl Drop for StandbyController {
    fn drop(&mut self) {
        self.stop();
    }
}
