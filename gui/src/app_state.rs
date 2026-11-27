use crate::api_cli::{resolve_api_url, resolve_cli_workspace, save_job_outputs};
use crate::attachments::{
    build_attachment_from_dib, build_attachment_from_file_with, build_attachment_from_png,
    is_image_filename, StagedUploadManager, MAX_ATTACHMENT_BYTES,
};
use crate::image_codec::make_thumbnail_data_url;
use crate::config::{
    diag_install, diag_line, infer_preset_index, is_valid_effort, is_valid_model_role,
    is_valid_work_mode, load_gui_settings_file, parse_workdir_command, preset_index_from_name,
    preset_name_from_index, preset_sampling, resolve_api_candidates, safe_artifact,
    sampling_from_json, sampling_to_json, save_gui_settings_file, strip_unc_prefix, valid_id,
    validate_workdir, SamplingValues, WorkdirCommand,
};
use crate::live_status::{
    flashes_taskbar, last_update_ago, liveness, live_tail, result_card, LiveClock, LiveTail,
};
use crate::standby::{JobDesktopBridge, StandbyController, StandbyNotice};
use crate::tls_http::{parse_api_url, ApiClient};
use crate::win_util::{
    clipboard_read_payload, clipboard_set_text, computer_name, desktop_windows, executable_dir,
    is_current_process_elevated, open_external_target, pick_files_dialog, pick_folder_dialog,
    write_file_atomic, ClipboardPayload, DesktopWindowEntry,
};
use crate::wording::{
    input_notice, job_end_notice, plain_notice, stage_log_line, CONTINUE_MESSAGE,
    EMPTY_GOAL_MESSAGE, NOT_CONNECTED_MESSAGE, READ_ONLY_MESSAGE,
};
use crate::worker_panels::{collect_worker_panels, execution_stage_label, planner_activity, WorkerPanel};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use parking_lot::Mutex;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Clone, Debug)]
pub enum UiAction {
    PushEvent(Value),
    ApplyTheme(bool),
    FlashTaskbar,
}

type UiSink = Arc<dyn Fn(UiAction) + Send + Sync>;

struct MutableState {
    gui: Value,
    server_settings: Value,
    capabilities: Value,
    health: Value,
    worker_profile: Value,
    perf: Value,
    conversations: Vec<Value>,
    conversation: Option<Value>,
    active_job: Option<Value>,
    active_job_pending: bool,
    connected: bool,
    connecting: bool,
    busy: bool,
    read_only: bool,
    history_access: bool,
    client_ip: String,
    attachments: Vec<Value>,
    sent_attachments_cache: HashMap<String, Value>,
    sent_attachments_order: VecDeque<String>,
    pc_windows: Vec<DesktopWindowEntry>,
    selected_window_id: isize,
    selected_window_title: String,
    local_workdir: String,
    local_workdirs: Map<String, Value>,
    read_roots: Vec<PathBuf>,
    logs: Vec<Value>,
    last_submitted_goal: String,
    last_submitted_files: Vec<Value>,
    last_job_id: String,
    conversation_id: String,
    question_id: String,
    pending_answer_job: String,
    full_watch_jobs: HashSet<String>,
    cli_override: bool,
    startup_server_patch: Map<String, Value>,
    tuning_open: bool,
    live_clock: LiveClock,
    live_newest_ms: i64,
    live_step_ms: i64,
    live_touched_ms: i64,
    live_pc_control: bool,
}

fn epoch_ms_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub struct AppController {
    root: PathBuf,
    tx: UiSink,
    hwnd: AtomicIsize,
    closing: AtomicBool,
    cancel_req: AtomicBool,
    staged: StagedUploadManager,
    active_api: Mutex<Option<Arc<ApiClient>>>,
    job_bridge: Arc<Mutex<JobDesktopBridge>>,
    standby: Arc<Mutex<StandbyController>>,
    st: Mutex<MutableState>,
}

impl AppController {
    pub fn new(cli_args: &[String], tx: UiSink) -> Arc<Self> {
        diag_install();
        let root = executable_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut gui = load_gui_settings_file(&root);
        let mut cli_override = false;
        let mut startup_server_patch = Map::new();

        // Check https-server.json auto promotion when api is still loopback
        if let Ok(text) = fs::read_to_string(root.join("https-server.json")) {
            if let Ok(trust) = serde_json::from_str::<Value>(&text) {
                if let Some(trust_url) = trust.get("url").and_then(|v| v.as_str()) {
                    let cur_api = gui.get("api").and_then(|v| v.as_str()).unwrap_or("");
                    if let Ok(p) = parse_api_url(cur_api) {
                        let is_loopback = p.host == "127.0.0.1" || p.host == "localhost";
                        let trust_is_loopback = parse_api_url(trust_url)
                            .map(|t| t.host == "127.0.0.1" || t.host == "localhost")
                            .unwrap_or(true);
                        if is_loopback && (!p.secure || !trust_is_loopback) {
                            gui["api"] = json!(trust_url);
                        }
                    }
                }
            }
        }

        // Apply GUI startup flags
        let mut i = 1usize;
        while i < cli_args.len() {
            let a = cli_args[i].as_str();
            let mut next_val = || -> Option<String> {
                if i + 1 < cli_args.len() {
                    i += 1;
                    Some(cli_args[i].clone())
                } else {
                    None
                }
            };
            match a {
                "--api" | "--url" | "--api-url" => {
                    if let Some(u) = next_val() {
                        gui["api"] = json!(u);
                    }
                    if a == "--api" {
                        cli_override = true;
                    }
                }
                "--no-sync-gui" => cli_override = true,
                "--no-tunnel" => {
                    if cli_args.len() <= 3 {
                        cli_override = true;
                    }
                }
                "--ssh-target" => {
                    let _ = next_val();
                }
                "--connect" | "--auto-connect" => gui["autoConnect"] = json!(true),
                "--no-connect" => gui["autoConnect"] = json!(false),
                "--plan" => {
                    gui["planMode"] = json!(true);
                    if cli_args.len() <= 2 {
                        cli_override = true;
                    }
                }
                "--no-plan" | "--direct" => gui["planMode"] = json!(false),
                "--work-mode" | "--mode" => {
                    if let Some(v) = next_val() {
                        if is_valid_work_mode(&v) {
                            gui["workMode"] = json!(v);
                        }
                    }
                }
                "--role" | "--model-role" => {
                    if let Some(v) = next_val() {
                        if is_valid_model_role(&v) {
                            gui["modelRole"] = json!(v);
                        }
                    }
                }
                "--workspace" => gui["workspaceEnabled"] = json!(true),
                "--no-workspace" => gui["workspaceEnabled"] = json!(false),
                "--local-cli" => gui["localCliEnabled"] = json!(true),
                "--no-local-cli" => gui["localCliEnabled"] = json!(false),
                "--run-on-pc" => gui["runOnPc"] = json!(true),
                "--server-windows" => gui["runOnPc"] = json!(false),
                "--workdir" => {
                    if let Some(v) = next_val() {
                        if let Ok(wd) = validate_workdir(&v) {
                            gui["localWorkdir"] = json!(wd.to_string_lossy());
                        }
                    }
                }
                "--window" => {
                    if let Some(v) = next_val() {
                        gui["pcWindowTitle"] = json!(v);
                    }
                }
                "--pc" => gui["pcEnabled"] = json!(true),
                "--no-pc" => gui["pcEnabled"] = json!(false),
                "--standby" => gui["pcStandby"] = json!(true),
                "--no-standby" => gui["pcStandby"] = json!(false),
                "--context" => {
                    if let Some(v) = next_val().and_then(|s| s.parse::<i32>().ok()) {
                        let c = v.clamp(2048, 32768);
                        gui["contextWindow"] = json!(c);
                        startup_server_patch.insert("contextWindow".into(), json!(c));
                    }
                }
                "--tokens" | "--max-tokens" => {
                    if let Some(v) = next_val().and_then(|s| s.parse::<i32>().ok()) {
                        let t = v.clamp(128, 8192);
                        gui["maxTokens"] = json!(t);
                        startup_server_patch.insert("maxTokens".into(), json!(t));
                    }
                }
                "--effort" | "--reasoning-effort" => {
                    if let Some(v) = next_val() {
                        if is_valid_effort(&v) {
                            gui["reasoningEffort"] = json!(v);
                            startup_server_patch.insert("reasoningEffort".into(), json!(v));
                        }
                    }
                }
                "--budget" | "--reasoning-budget" => {
                    if let Some(v) = next_val().and_then(|s| s.parse::<i32>().ok()) {
                        let b = v.clamp(-1, 8192);
                        gui["reasoningBudgetTokens"] = json!(b);
                        startup_server_patch.insert("reasoningBudgetTokens".into(), json!(b));
                    }
                }
                "--preset" => {
                    if let Some(v) = next_val() {
                        let idx = preset_index_from_name(&v);
                        if (0..=2).contains(&idx) {
                            let s = preset_sampling(idx);
                            sampling_to_json(&s, &mut gui);
                            gui["samplingPreset"] = json!(preset_name_from_index(idx));
                            let mut tmp = Value::Object(std::mem::take(&mut startup_server_patch));
                            sampling_to_json(&s, &mut tmp);
                            if let Value::Object(m) = tmp {
                                startup_server_patch = m;
                            }
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }

        let selected_title = gui
            .get("pcWindowTitle")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let local_workdir = gui
            .get("localWorkdir")
            .and_then(|v| v.as_str())
            .and_then(|s| if s.is_empty() { None } else { validate_workdir(s).ok() })
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let local_workdirs = gui
            .get("localWorkdirs")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let mut read_roots = Vec::new();
        if let Some(arr) = gui.get("readOnlyFolders").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(s) = item.as_str() {
                    if Path::new(s).is_dir() {
                        if let Ok(canon) = fs::canonicalize(s) {
                            read_roots.push(strip_unc_prefix(canon));
                        }
                    }
                }
            }
        }
        let conv_id = gui
            .get("conversationId")
            .and_then(|v| v.as_str())
            .filter(|s| valid_id(s))
            .unwrap_or("")
            .to_string();
        let last_job = gui
            .get("lastJob")
            .and_then(|v| v.as_str())
            .filter(|s| valid_id(s))
            .unwrap_or("")
            .to_string();

        Arc::new(Self {
            root: root.clone(),
            tx,
            hwnd: AtomicIsize::new(0),
            closing: AtomicBool::new(false),
            cancel_req: AtomicBool::new(false),
            staged: StagedUploadManager::new(),
            active_api: Mutex::new(None),
            job_bridge: Arc::new(Mutex::new(JobDesktopBridge::new())),
            standby: Arc::new(Mutex::new(StandbyController::new())),
            st: Mutex::new(MutableState {
                gui,
                server_settings: json!({}),
                capabilities: json!({}),
                health: json!({}),
                worker_profile: json!({}),
                perf: json!({}),
                conversations: Vec::new(),
                conversation: None,
                active_job: None,
                active_job_pending: false,
                connected: false,
                connecting: false,
                busy: false,
                read_only: false,
                history_access: false,
                client_ip: String::new(),
                attachments: Vec::new(),
                sent_attachments_cache: HashMap::new(),
                sent_attachments_order: VecDeque::new(),
                pc_windows: Vec::new(),
                selected_window_id: 0,
                selected_window_title: selected_title,
                local_workdir,
                local_workdirs,
                read_roots,
                logs: Vec::new(),
                last_submitted_goal: String::new(),
                last_submitted_files: Vec::new(),
                last_job_id: last_job,
                conversation_id: conv_id,
                question_id: String::new(),
                pending_answer_job: String::new(),
                full_watch_jobs: HashSet::new(),
                cli_override,
                startup_server_patch,
                tuning_open: false,
                live_clock: LiveClock::default(),
                live_newest_ms: 0,
                live_step_ms: 0,
                live_touched_ms: 0,
                live_pc_control: false,
            }),
        })
    }

    pub fn set_hwnd(&self, hwnd: isize) {
        self.hwnd.store(hwnd, Ordering::SeqCst);
    }

    pub fn saved_window_geometry(&self) -> (Option<i32>, Option<i32>, i32, i32, bool) {
        let g = self.st.lock();
        let x = g.gui.get("windowX").and_then(|v| v.as_i64()).map(|v| v as i32);
        let y = g.gui.get("windowY").and_then(|v| v.as_i64()).map(|v| v as i32);
        let w = g.gui.get("windowWidth").and_then(|v| v.as_i64()).unwrap_or(1320) as i32;
        let h = g.gui.get("windowHeight").and_then(|v| v.as_i64()).unwrap_or(860) as i32;
        let m = g.gui.get("windowMaximized").and_then(|v| v.as_bool()).unwrap_or(false);
        (x, y, w.clamp(1000, 3840), h.clamp(660, 2160), m)
    }

    pub fn initial_ui_snapshot(&self) -> (Value, String, String, Vec<String>, String) {
        let g = self.st.lock();
        let api = g
            .gui
            .get("api")
            .and_then(|v| v.as_str())
            .unwrap_or("https://localhost:8443")
            .to_string();
        let read_roots = g
            .read_roots
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        (
            g.gui.clone(),
            api,
            g.local_workdir.clone(),
            read_roots,
            g.selected_window_title.clone(),
        )
    }

    fn emit_event(&self, ev: Value) {
        if !self.closing.load(Ordering::SeqCst) {
            (self.tx)(UiAction::PushEvent(ev));
        }
    }

    fn push_log(&self, line: &str, level: &str) {
        let row = json!({ "line": line, "level": level });
        {
            let mut g = self.st.lock();
            g.logs.push(row.clone());
            if g.logs.len() > 600 {
                g.logs.remove(0);
            }
        }
        diag_line(line.trim());
        self.emit_event(json!({ "type": "log", "line": line, "level": level }));
    }

    fn toast(&self, level: &str, message: &str) {
        self.emit_event(json!({ "type": "toast", "level": level, "message": message }));
    }

    fn push_connection(&self) {
        let g = self.st.lock();
        let standby = self.standby.lock().is_active();
        let url = g.gui.get("api").and_then(|v| v.as_str()).unwrap_or("").to_string();
        self.emit_event(json!({
            "type": "connection",
            "connected": g.connected,
            "busy": g.busy,
            "connecting": g.connecting,
            "standby": standby,
            "url": url,
        }));
    }

    fn push_attachments(&self) {
        let items = self.st.lock().attachments.clone();
        let stages = self.staged.stage_status_map();
        self.emit_event(json!({
            "type": "attachments",
            "items": items,
            "stages": stages,
        }));
    }

    fn persist_gui(&self) {
        let patch_val = {
            let mut g = self.st.lock();
            if g.cli_override {
                return;
            }
            let folders: Vec<Value> = g
                .read_roots
                .iter()
                .map(|p| Value::String(p.to_string_lossy().into_owned()))
                .collect();
            g.gui["localWorkdirs"] = Value::Object(g.local_workdirs.clone());
            g.gui["localWorkdir"] = json!(g.local_workdir);
            g.gui["readOnlyFolders"] = Value::Array(folders);
            g.gui["pcWindowTitle"] = json!(g.selected_window_title);
            g.gui["lastJob"] = json!(g.last_job_id);
            g.gui["conversationId"] = json!(g.conversation_id);
            g.gui.clone()
        };
        let _ = save_gui_settings_file(&self.root, &patch_val);
    }

    fn start_live_clock(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_millis(1000));
                let Some(app) = weak.upgrade() else { break };
                if app.closing.load(Ordering::SeqCst) {
                    break;
                }
                let snap = {
                    let g = app.st.lock();
                    if !g.active_job_pending || g.active_job.is_none() {
                        continue;
                    }
                    (
                        g.active_job.clone().unwrap(),
                        g.read_only,
                        g.live_clock,
                        g.live_newest_ms,
                        g.live_step_ms,
                        g.live_touched_ms,
                        g.live_pc_control,
                    )
                };
                let (job, ro, clock, newest_ms, step_ms, touched_ms, pc_ctrl) = snap;
                let now = epoch_ms_now();
                let seen_ago = if touched_ms > 0 { (now - touched_ms).max(0) } else { -1 };
                let ago_ms = last_update_ago(ro, &clock, newest_ms, now, seen_ago);
                let step_elapsed = if step_ms > 0 { (now - step_ms).max(0) } else { -1 };
                let tail: LiveTail = live_tail(step_elapsed, ago_ms, pc_ctrl);
                let tail_text = if tail.step.is_empty() {
                    tail.fresh.clone()
                } else if tail.fresh.is_empty() {
                    tail.step.clone()
                } else {
                    format!("{} · {}", tail.step, tail.fresh)
                };
                let stage_text = execution_stage_label(&job);
                app.emit_event(json!({
                    "type": "job_update",
                    "liveText": stage_text,
                    "liveTail": tail_text,
                    "isStale": tail.stale,
                }));
            }
        });
    }

    pub fn handle_ipc(self: &Arc<Self>, msg: Value) {
        let cmd = msg.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
        match cmd {
            "init" => {
                self.start_live_clock();
                let snap = {
                    let mut g = self.st.lock();
                    if !g.cli_override {
                        if g.gui.get("modelRole").and_then(|v| v.as_str()) == Some("worker") {
                            g.gui["modelRole"] = json!("planner");
                        }
                        if g.gui.get("planMode").and_then(|v| v.as_bool()) == Some(true) {
                            g.gui["planMode"] = json!(false);
                        }
                    }
                    let read_roots: Vec<String> = g
                        .read_roots
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect();
                    let wins: Vec<Value> = g
                        .pc_windows
                        .iter()
                        .map(|w| serde_json::to_value(w).unwrap())
                        .collect();
                    json!({
                        "type": "bootstrap",
                        "guiSettings": g.gui,
                        "computerName": computer_name(),
                        "elevated": is_current_process_elevated(),
                        "logs": g.logs,
                        "attached": g.attachments,
                        "uploadStages": self.staged.stage_status_map(),
                        "pcWindows": wins,
                        "selectedWindowId": g.selected_window_id,
                        "selectedWindowTitle": g.selected_window_title,
                        "localWorkdir": g.local_workdir,
                        "readRoots": read_roots,
                    })
                };
                self.emit_event(snap);
                self.push_connection();
                let app_win = Arc::clone(self);
                thread::spawn(move || {
                    app_win.refresh_windows_internal();
                    app_win.emit_pc_windows_and_tools();
                });
                if self
                    .st
                    .lock()
                    .gui
                    .get("autoConnect")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true)
                {
                    let app = Arc::clone(self);
                    thread::spawn(move || app.do_connect());
                }
            }
            "connect" => {
                self.apply_conn_fields(&msg);
                let app = Arc::clone(self);
                thread::spawn(move || app.do_connect());
            }
            "start_server" => {
                self.apply_conn_fields(&msg);
                let app = Arc::clone(self);
                thread::spawn(move || app.do_start_server());
            }
            "stop_server" => {
                self.apply_conn_fields(&msg);
                let app = Arc::clone(self);
                thread::spawn(move || app.do_stop_server());
            }
            "restart_server" => {
                self.apply_conn_fields(&msg);
                let app = Arc::clone(self);
                thread::spawn(move || app.do_restart_server());
            }
            "refresh_history" => {
                let app = Arc::clone(self);
                thread::spawn(move || app.do_refresh_history());
            }
            "refresh_perf" => {
                let app = Arc::clone(self);
                thread::spawn(move || app.do_refresh_perf());
            }
            "theme_toggle" => {
                let next_dark = {
                    let mut g = self.st.lock();
                    let cur = g.gui.get("darkTheme").and_then(|v| v.as_bool()).unwrap_or(true);
                    g.gui["darkTheme"] = json!(!cur);
                    !cur
                };
                self.persist_gui();
                (self.tx)(UiAction::ApplyTheme(next_dark));
                self.emit_settings_sync();
            }
            "switch_work_mode" => {
                if let Some(m) = msg.get("mode").and_then(|v| v.as_str()) {
                    if is_valid_work_mode(m) {
                        {
                            let mut g = self.st.lock();
                            g.gui["workMode"] = json!(m);
                            if m == "coding" {
                                g.gui["workspaceEnabled"] = json!(true);
                            } else if m == "pc_control" {
                                g.gui["workspaceEnabled"] = json!(true);
                                g.gui["planMode"] = json!(false);
                            }
                        }
                        self.persist_gui();
                        self.emit_settings_sync();
                    }
                }
            }
            "switch_model_role" => {
                {
                    let mut g = self.st.lock();
                    g.gui["modelRole"] = json!("planner");
                }
                self.persist_gui();
                self.emit_settings_sync();
            }
            "toggle_plan_mode" => {
                {
                    let mut g = self.st.lock();
                    g.gui["planMode"] = json!(false);
                }
                self.persist_gui();
                self.emit_settings_sync();
            }
            "set_tuning_open" => {
                let open = msg.get("open").and_then(|v| v.as_bool()).unwrap_or(false);
                self.st.lock().tuning_open = open;
            }
            "update_pref" => {
                self.apply_conn_fields(&msg);
                self.persist_gui();
                self.emit_settings_sync();
            }
            "apply_settings" => {
                self.apply_conn_fields(&msg);
                let app = Arc::clone(self);
                thread::spawn(move || app.do_apply_settings(msg));
            }
            "reset_sampling" => {
                let s = SamplingValues::default();
                let mut patch = json!({});
                sampling_to_json(&s, &mut patch);
                patch["samplingPreset"] = json!("code");
                let app = Arc::clone(self);
                thread::spawn(move || app.do_apply_settings(patch));
            }
            "start_autotune" => {
                let mode = msg
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("speed")
                    .to_string();
                let app = Arc::clone(self);
                thread::spawn(move || app.do_autotune(&mode));
            }
            "send_message" => {
                let text = msg.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                self.handle_send_message(text);
            }
            "cancel_job" => {
                self.handle_cancel_job();
            }
            "answer_question" => {
                let qid = msg.get("questionId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let ans = msg.get("answer").and_then(|v| v.as_str()).unwrap_or("").to_string();
                self.handle_answer_question(qid, ans);
            }
            "continue_chat" => {
                self.handle_send_message(CONTINUE_MESSAGE.to_string());
            }
            "resend_request" => {
                let (goal, files, cid) = {
                    let g = self.st.lock();
                    (g.last_submitted_goal.clone(), g.last_submitted_files.clone(), g.conversation_id.clone())
                };
                if !goal.is_empty() {
                    self.cache_sent_attachments(&cid, &files);
                    let pending_id = format!("pending-{}", uuid::Uuid::new_v4());
                    self.emit_optimistic_job(&pending_id, &goal, &files);
                    let app = Arc::clone(self);
                    thread::spawn(move || app.submit_and_poll_job(goal, files, None, None, pending_id));
                }
            }
            "new_chat" => {
                self.clear_all_attachments_internal();
                {
                    let mut g = self.st.lock();
                    g.conversation = None;
                    g.conversation_id.clear();
                    g.active_job = None;
                    g.active_job_pending = false;
                    g.question_id.clear();
                    g.read_only = false;
                }
                self.persist_gui();
                self.emit_event(json!({
                    "type": "conversation",
                    "conversation": null,
                    "readOnly": false,
                    "localWorkdir": self.st.lock().local_workdir,
                }));
                self.emit_event(json!({ "type": "notice", "clear": true }));
                self.emit_event(json!({ "type": "workers", "panels": [], "summaryLabel": "" }));
                self.emit_event(json!({ "type": "command", "action": "focus_input" }));
            }
            "open_conversation" => {
                if let Some(id) = msg.get("id").and_then(|v| v.as_str()) {
                    let cid = id.to_string();
                    let app = Arc::clone(self);
                    thread::spawn(move || app.do_open_conversation(&cid));
                }
            }
            "load_full_job" => {
                if let Some(id) = msg.get("jobId").and_then(|v| v.as_str()) {
                    let jid = id.to_string();
                    self.st.lock().full_watch_jobs.insert(jid.clone());
                    let app = Arc::clone(self);
                    thread::spawn(move || app.do_load_full_job(&jid));
                }
            }
            "pick_attachments" => {
                let owner = self.hwnd.load(Ordering::SeqCst);
                let paths = pick_files_dialog(owner, "添付ファイルを選択（最大5件）");
                if !paths.is_empty() {
                    self.attach_local_paths(&paths);
                }
            }
            "add_dropped_files" => {
                if let Some(arr) = msg.get("paths").and_then(|v| v.as_array()) {
                    let paths: Vec<PathBuf> = arr
                        .iter()
                        .filter_map(|v| v.as_str())
                        .map(PathBuf::from)
                        .collect();
                    if !paths.is_empty() {
                        self.attach_local_paths(&paths);
                    }
                }
            }
            "paste_clipboard" => {
                self.handle_clipboard_paste();
            }
            "remove_attachment" => {
                let path = msg.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let api_url = self.current_api_url();
                self.staged.remove_path(path, &api_url);
                self.st
                    .lock()
                    .attachments
                    .retain(|a| a.get("path").and_then(|v| v.as_str()) != Some(path));
                self.push_attachments();
            }
            "clear_attachments" => {
                self.clear_all_attachments_internal();
            }
            "preview_attachment" => {
                let idx = msg.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let item_opt = self.st.lock().attachments.get(idx).cloned();
                if let Some(item) = item_opt {
                    self.preview_attachment_item(&item);
                }
            }
            "pick_workdir" => {
                let owner = self.hwnd.load(Ordering::SeqCst);
                if let Some(dir) = pick_folder_dialog(owner, "作業フォルダーを選択") {
                    match validate_workdir(&dir.to_string_lossy()) {
                        Ok(canon) => {
                            self.set_current_workdir(&canon.to_string_lossy());
                            self.toast("ok", &format!("作業フォルダーを設定しました: {}", canon.display()));
                        }
                        Err(e) => {
                            self.emit_error_notice(&e);
                        }
                    }
                }
            }
            "reset_workdir" => {
                self.set_current_workdir("");
                self.toast("ok", "作業フォルダーをジョブごとの自動設定へ戻しました");
            }
            "open_local_folder" => {
                let target = {
                    let g = self.st.lock();
                    if !g.local_workdir.is_empty() {
                        PathBuf::from(&g.local_workdir)
                    } else {
                        self.root.join("local-workspaces")
                    }
                };
                let _ = fs::create_dir_all(&target);
                let _ = open_external_target(&target.to_string_lossy());
            }
            "add_read_folder" => {
                let owner = self.hwnd.load(Ordering::SeqCst);
                if let Some(dir) = pick_folder_dialog(owner, "読み取り専用の資料フォルダーを追加") {
                    match validate_workdir(&dir.to_string_lossy()) {
                        Ok(canon) => {
                            {
                                let mut g = self.st.lock();
                                if !g.read_roots.contains(&canon) {
                                    g.read_roots.push(canon);
                                }
                            }
                            self.persist_gui();
                            self.emit_pc_windows_and_tools();
                        }
                        Err(e) => self.emit_error_notice(&e),
                    }
                }
            }
            "clear_read_folders" => {
                self.st.lock().read_roots.clear();
                self.persist_gui();
                self.emit_pc_windows_and_tools();
            }
            "remove_read_folder" => {
                let idx = msg.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                {
                    let mut g = self.st.lock();
                    if idx < g.read_roots.len() {
                        g.read_roots.remove(idx);
                    }
                }
                self.persist_gui();
                self.emit_pc_windows_and_tools();
            }
            "set_workspace_enabled" => {
                let v = msg.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true);
                self.st.lock().gui["workspaceEnabled"] = json!(v);
                self.persist_gui();
                self.emit_settings_sync();
            }
            "set_local_cli" => {
                let v = msg.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true);
                self.st.lock().gui["localCliEnabled"] = json!(v);
                self.persist_gui();
                self.emit_settings_sync();
            }
            "set_run_on_pc" => {
                let v = msg.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false);
                self.st.lock().gui["runOnPc"] = json!(v);
                self.persist_gui();
                self.emit_settings_sync();
            }
            "set_pc_enabled" => {
                let v = msg.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false);
                self.st.lock().gui["pcEnabled"] = json!(v);
                self.persist_gui();
                self.emit_settings_sync();
            }
            "set_pc_standby" => {
                let want = msg.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false);
                if want {
                    self.start_standby();
                } else {
                    self.stop_standby();
                }
            }
            "refresh_pc_windows" => {
                self.refresh_windows_internal();
                self.emit_pc_windows_and_tools();
            }
            "select_pc_window" => {
                let id = msg.get("id").and_then(|v| v.as_i64()).unwrap_or(0) as isize;
                {
                    let mut g = self.st.lock();
                    let found = g
                        .pc_windows
                        .iter()
                        .find(|w| w.id == id)
                        .map(|w| (w.id, w.title.clone()));
                    if let Some((wid, wtitle)) = found {
                        g.selected_window_id = wid;
                        g.selected_window_title = wtitle;
                    }
                }
                self.persist_gui();
                self.emit_pc_windows_and_tools();
            }
            "stop_pc_all" => {
                self.trigger_emergency_pc_stop();
            }
            "files_load" => {
                let app = Arc::clone(self);
                thread::spawn(move || app.do_load_files());
            }
            "files_preview" => {
                let path = msg.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let app = Arc::clone(self);
                thread::spawn(move || app.do_preview_remote_file(&path));
            }
            "preview_sent_attachment" => {
                let cid = msg.get("conversationId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let path = msg.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let display_name = msg.get("displayName").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let app = Arc::clone(self);
                thread::spawn(move || app.do_preview_sent_attachment(&cid, &path, &display_name));
            }
            "save_downloads" => {
                let job_id = msg.get("jobId").and_then(|v| v.as_str()).map(|s| s.to_string());
                let app = Arc::clone(self);
                thread::spawn(move || app.do_save_downloads(job_id));
            }
            "share_load" => {
                let app = Arc::clone(self);
                thread::spawn(move || app.do_share_load());
            }
            "share_add" => {
                let ip = msg.get("ip").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let mode = msg.get("mode").and_then(|v| v.as_str()).unwrap_or("content").to_string();
                let app = Arc::clone(self);
                thread::spawn(move || app.do_share_update(ip, mode, false));
            }
            "share_remove" => {
                let idx = msg.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let app = Arc::clone(self);
                thread::spawn(move || app.do_share_remove(idx));
            }
            "open_external" => {
                if let Some(t) = msg.get("target").and_then(|v| v.as_str()) {
                    if t.starts_with("http://") || t.starts_with("https://") {
                        let _ = open_external_target(t);
                    }
                }
            }
            "copy_text" => {
                if let Some(t) = msg.get("text").and_then(|v| v.as_str()) {
                    let _ = clipboard_set_text(t);
                    self.toast("ok", "コピーしました");
                }
            }
            _ => {}
        }
    }

    pub fn trigger_emergency_pc_stop(&self) {
        self.push_log("緊急停止 (Ctrl+Shift+F12 / 停止ボタン) を実行しました\r\n", "warn");
        self.stop_standby();
        self.job_bridge.lock().cancel_io();
        self.toast("warn", "PC操作を停止しました");
        self.push_connection();
    }

    pub fn on_window_closing(&self, placement: Option<(i32, i32, i32, i32, bool)>) {
        self.closing.store(true, Ordering::SeqCst);
        if let Some((x, y, w, h, max)) = placement {
            let mut g = self.st.lock();
            g.gui["windowX"] = json!(x);
            g.gui["windowY"] = json!(y);
            g.gui["windowWidth"] = json!(w);
            g.gui["windowHeight"] = json!(h);
            g.gui["windowMaximized"] = json!(max);
        }
        self.persist_gui();
        self.staged.shutdown();
        if let Some(api) = self.active_api.lock().take() {
            api.cancel_io();
        }
        self.standby.lock().stop();
        self.job_bridge.lock().stop_and_close();
    }

    fn current_api_url(&self) -> String {
        let g = self.st.lock();
        g.gui
            .get("api")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| resolve_api_url(&self.root, ""))
    }

    fn apply_conn_fields(&self, msg: &Value) {
        let mut g = self.st.lock();
        for key in ["api", "autoConnect"] {
            if let Some(v) = msg.get(key) {
                g.gui[key] = v.clone();
            }
        }
    }

    fn connect_first_healthy(&self) -> Result<(String, ApiClient, Value), String> {
        let preferred = self.current_api_url();
        let candidates = resolve_api_candidates(&self.root, &preferred);
        let mut errs: Vec<String> = Vec::new();
        for url in &candidates {
            match ApiClient::new(url).and_then(|c| c.get("/health", 2500).map(|h| (c, h))) {
                Ok((client, health)) => return Ok((url.clone(), client, health)),
                Err(e) => errs.push(format!("{} ({})", url, e)),
            }
        }
        Err(if errs.is_empty() {
            "HTTPS API へ接続できません。サーバーと証明書を確認してください。".into()
        } else {
            format!("HTTPS API へ接続できません: {}", errs.join(" / "))
        })
    }

    fn emit_settings_sync(&self) {
        let g = self.st.lock();
        self.emit_event(json!({
            "type": "settings_sync",
            "gui": g.gui,
            "server": g.server_settings,
            "health": g.health,
            "workerProfile": g.worker_profile,
            "capabilities": g.capabilities,
            "readOnly": g.read_only,
            "clientIp": g.client_ip,
            "historyAccess": g.history_access,
        }));
    }

    fn emit_pc_windows_and_tools(&self) {
        let g = self.st.lock();
        let wins: Vec<Value> = g
            .pc_windows
            .iter()
            .map(|w| serde_json::to_value(w).unwrap())
            .collect();
        self.emit_event(json!({
            "type": "pc_windows",
            "windows": wins,
            "selectedId": g.selected_window_id,
            "selectedTitle": g.selected_window_title,
        }));
        let read_roots: Vec<String> = g
            .read_roots
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        self.emit_event(json!({
            "type": "bootstrap",
            "guiSettings": g.gui,
            "computerName": computer_name(),
            "elevated": is_current_process_elevated(),
            "pcWindows": wins,
            "selectedWindowId": g.selected_window_id,
            "selectedWindowTitle": g.selected_window_title,
            "localWorkdir": g.local_workdir,
            "readRoots": read_roots,
            "attached": g.attachments,
            "uploadStages": self.staged.stage_status_map(),
            "logs": g.logs,
        }));
    }

    fn emit_error_notice(&self, raw: &str) {
        let n = input_notice(raw);
        self.emit_event(json!({
            "type": "notice",
            "level": "error",
            "label": n.label,
            "hint": n.hint,
            "raw": n.raw,
            "retry": n.retry,
        }));
        self.push_log(&format!("エラー: {}\r\n", raw), "error");
    }

    fn refresh_windows_internal(&self) {
        let wins = desktop_windows();
        let mut g = self.st.lock();
        let cur_id = g.selected_window_id;
        let cur_title = g.selected_window_title.clone();
        if cur_id != 0 {
            if let Some(m) = wins.iter().find(|w| w.id == cur_id) {
                g.selected_window_title = m.title.clone();
            } else if !cur_title.is_empty() {
                if let Some(m) = wins.iter().find(|w| w.title == cur_title) {
                    g.selected_window_id = m.id;
                } else if let Some(m) = wins.iter().find(|w| w.title.to_lowercase().contains(&cur_title.to_lowercase())) {
                    g.selected_window_id = m.id;
                    g.selected_window_title = m.title.clone();
                } else {
                    g.selected_window_id = 0;
                }
            } else {
                g.selected_window_id = 0;
            }
        } else if !cur_title.is_empty() {
            if let Some(m) = wins.iter().find(|w| w.title == cur_title).or_else(|| {
                wins.iter()
                    .find(|w| w.title.to_lowercase().contains(&cur_title.to_lowercase()))
            }) {
                g.selected_window_id = m.id;
                g.selected_window_title = m.title.clone();
            }
        }
        g.pc_windows = wins;
    }

    fn set_current_workdir(&self, dir: &str) {
        let mut g = self.st.lock();
        g.local_workdir = dir.to_string();
        if valid_id(&g.conversation_id) {
            let cid = g.conversation_id.clone();
            if dir.is_empty() {
                g.local_workdirs.remove(&cid);
            } else {
                g.local_workdirs.insert(cid, json!(dir));
            }
        }
        g.gui["localWorkdir"] = json!(dir);
        drop(g);
        self.persist_gui();
        self.emit_pc_windows_and_tools();
    }

    fn clear_all_attachments_internal(&self) {
        let url = self.current_api_url();
        self.staged.clear_all(&url);
        self.st.lock().attachments.clear();
        self.push_attachments();
    }

    fn attachment_context(&self) -> (Vec<String>, usize) {
        let g = self.st.lock();
        let mut paths = Vec::new();
        let mut used = 0usize;
        for a in &g.attachments {
            if let Some(p) = a.get("path").and_then(|v| v.as_str()) {
                paths.push(p.to_string());
            }
            used += a.get("size").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        }
        if let Some(jobs) = g
            .conversation
            .as_ref()
            .and_then(|c| c.get("jobs"))
            .and_then(|v| v.as_array())
        {
            for j in jobs {
                for key in ["attachments", "files"] {
                    if let Some(arr) = j.get(key).and_then(|v| v.as_array()) {
                        for a in arr {
                            if let Some(p) = a.get("path").and_then(|v| v.as_str()) {
                                paths.push(p.to_string());
                            }
                        }
                    }
                }
            }
        }
        (paths, used)
    }

    fn attach_local_paths(self: &Arc<Self>, paths: &[PathBuf]) {
        let api_url = self.current_api_url();
        let connected = self.st.lock().connected;
        let (mut existing, mut used) = self.attachment_context();
        for p in paths {
            let cur_count = self.st.lock().attachments.len();
            if cur_count >= 5 {
                self.toast("warn", "添付は最大5件までです");
                break;
            }
            let remaining = MAX_ATTACHMENT_BYTES.saturating_sub(used);
            match build_attachment_from_file_with(p, &existing, remaining) {
                Ok(att) => {
                    if let Some(name) = att.get("path").and_then(|v| v.as_str()) {
                        existing.push(name.to_string());
                    }
                    used += att.get("size").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    self.st.lock().attachments.push(att);
                }
                Err(e) => {
                    self.emit_error_notice(&e);
                }
            }
        }
        self.push_attachments();
        if connected && !api_url.is_empty() {
            let all = self.st.lock().attachments.clone();
            let app = Arc::clone(self);
            self.staged.start_uploads(&all, &api_url, Some(move || app.push_attachments()));
        }
    }

    fn handle_clipboard_paste(self: &Arc<Self>) {
        if self.st.lock().attachments.len() >= 5 {
            match clipboard_read_payload() {
                Ok(ClipboardPayload::Files(_))
                | Ok(ClipboardPayload::PngBytes(_))
                | Ok(ClipboardPayload::DibBytes(_)) => {
                    self.toast("warn", "添付は最大5件までです");
                }
                Ok(ClipboardPayload::Text) | Ok(ClipboardPayload::Empty) => {}
                Err(e) => self.emit_error_notice(&e),
            }
            return;
        }
        let (existing, used) = self.attachment_context();
        let remaining = MAX_ATTACHMENT_BYTES.saturating_sub(used);
        match clipboard_read_payload() {
            Ok(ClipboardPayload::Files(paths)) => self.attach_local_paths(&paths),
            Ok(ClipboardPayload::PngBytes(png)) => {
                match build_attachment_from_png(&png, &existing, remaining) {
                    Ok(att) => self.finish_clipboard_image_attachment(att),
                    Err(e) => self.emit_error_notice(&e),
                }
            }
            Ok(ClipboardPayload::DibBytes(dib)) => {
                match build_attachment_from_dib(&dib, &existing, remaining) {
                    Ok(att) => self.finish_clipboard_image_attachment(att),
                    Err(e) => self.emit_error_notice(&e),
                }
            }
            Ok(ClipboardPayload::Text) | Ok(ClipboardPayload::Empty) => {}
            Err(e) => self.emit_error_notice(&e),
        }
    }

    fn finish_clipboard_image_attachment(self: &Arc<Self>, att: Value) {
        {
            let mut g = self.st.lock();
            if g.attachments.len() >= 5 {
                self.toast("warn", "添付は最大5件までです");
                return;
            }
            g.attachments.push(att);
        }
        self.push_attachments();
        let api_url = self.current_api_url();
        if self.st.lock().connected {
            let all = self.st.lock().attachments.clone();
            let app = Arc::clone(self);
            self.staged.start_uploads(&all, &api_url, Some(move || app.push_attachments()));
        }
        self.toast("ok", "クリップボード画像を添付しました");
    }

    fn image_mime_for_path(path: &str) -> Option<&'static str> {
        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".png") {
            Some("image/png")
        } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
            Some("image/jpeg")
        } else if lower.ends_with(".webp") {
            Some("image/webp")
        } else if lower.ends_with(".gif") {
            Some("image/gif")
        } else if lower.ends_with(".bmp") {
            Some("image/bmp")
        } else {
            None
        }
    }

    fn cache_sent_attachment(&self, cid: &str, item: &Value) {
        let path = item.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if path.is_empty() {
            return;
        }
        let mut g = self.st.lock();
        let insert_key = |st: &mut MutableState, key: String, val: Value| {
            if !st.sent_attachments_cache.contains_key(&key) {
                st.sent_attachments_order.push_back(key.clone());
            }
            st.sent_attachments_cache.insert(key, val);
            while st.sent_attachments_order.len() > 96 {
                if let Some(old) = st.sent_attachments_order.pop_front() {
                    st.sent_attachments_cache.remove(&old);
                }
            }
        };
        insert_key(&mut g, path.to_string(), item.clone());
        if valid_id(cid) {
            insert_key(&mut g, format!("{}:{}", cid, path), item.clone());
        }
    }

    fn emit_sent_thumbs(&self, cid: &str, items: &[Value]) {
        let mut out = Vec::new();
        for item in items {
            let path = item.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let preview = item.get("previewUrl").and_then(|v| v.as_str()).unwrap_or("");
            if path.is_empty() || preview.is_empty() {
                continue;
            }
            out.push(json!({ "key": path, "path": path, "previewUrl": preview }));
            if valid_id(cid) {
                out.push(json!({ "key": format!("{}:{}", cid, path), "path": path, "previewUrl": preview }));
            }
        }
        if !out.is_empty() {
            self.emit_event(json!({ "type": "sent_attach_thumbs", "items": out }));
        }
    }

    fn cache_sent_attachments(&self, cid: &str, items: &[Value]) {
        if items.is_empty() {
            return;
        }
        for item in items {
            self.cache_sent_attachment(cid, item);
        }
        self.emit_sent_thumbs(cid, items);
    }

    fn preview_attachment_item(&self, item: &Value) {
        let path = item.get("path").and_then(|v| v.as_str()).unwrap_or("attachment");
        let title = item
            .get("displayName")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| path.strip_prefix("uploads/").unwrap_or(path));
        let b64 = item.get("base64").and_then(|v| v.as_str()).unwrap_or("");
        let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("file");
        if kind == "image" || Self::image_mime_for_path(path).is_some() {
            let mime = Self::image_mime_for_path(path).unwrap_or("image/png");
            let data_url = if mime == "image/bmp" {
                B64.decode(b64)
                    .ok()
                    .and_then(|bytes| make_thumbnail_data_url(&bytes, 1920))
                    .unwrap_or_else(|| format!("data:{};base64,{}", mime, b64))
            } else {
                format!("data:{};base64,{}", mime, b64)
            };
            self.emit_event(json!({
                "type": "attachment_preview",
                "title": title,
                "kind": "image",
                "dataUrl": data_url,
            }));
        } else {
            let text = B64
                .decode(b64)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            self.emit_event(json!({
                "type": "attachment_preview",
                "title": title,
                "kind": "text",
                "text": text,
            }));
        }
    }

    fn enrich_conversation(&self, mut conv: Value) -> Value {
        let mut active_job: Option<Value> = None;
        if let Some(jobs) = conv.get_mut("jobs").and_then(|v| v.as_array_mut()) {
            for j in jobs.iter_mut() {
                let pc_ctrl = j
                    .get("options")
                    .and_then(|o| o.get("workMode"))
                    .and_then(|v| v.as_str())
                    == Some("pc_control");
                let rc = result_card(j, pc_ctrl);
                j["_resultCard"] = json!({ "line": rc.line, "badge": rc.badge });
                active_job = Some(j.clone());
            }
        }
        if let Some(ref j) = active_job {
            let mut panels: Vec<WorkerPanel> = Vec::new();
            let jid = j.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let jst = j.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let act_rows = j
                .get("activity")
                .and_then(|v| v.as_array())
                .map(|a| a.as_slice())
                .unwrap_or(&[]);
            collect_worker_panels(&mut panels, jid, act_rows, jst);
            let p_rows = conv
                .get("jobs")
                .and_then(|v| v.as_array())
                .and_then(|a| a.last())
                .and_then(|j| j.get("activity"))
                .and_then(|v| v.as_array())
                .map(|a| planner_activity(a))
                .unwrap_or_default();
            self.emit_event(json!({
                "type": "workers",
                "panels": panels,
                "plannerActivity": p_rows,
            }));
        }
        conv
    }

    fn do_connect(self: &Arc<Self>) {
        {
            let mut g = self.st.lock();
            if g.connecting {
                return;
            }
            g.connecting = true;
        }
        self.push_connection();

        let res = (|| -> Result<Value, String> {
            let (resolved_url, client, health) = self.connect_first_healthy()?;
            if health.get("service").and_then(|v| v.as_str()) != Some("qwen27-framework") {
                return Err("接続先が Qwen27 Framework API ではありません".into());
            }
            if resolved_url != self.current_api_url() {
                {
                    let mut g = self.st.lock();
                    g.gui["api"] = json!(resolved_url);
                }
                self.persist_gui();
            }

            let patch = std::mem::take(&mut self.st.lock().startup_server_patch);
            if !patch.is_empty() {
                let _ = client.post("/settings", &Value::Object(patch), 420_000);
            }

            let mut settings = client.get("/settings", 15000)?;
            let caps = health.get("capabilities").cloned().unwrap_or_else(|| json!({}));
            let hist_acc = health.get("historyAccess").and_then(|v| v.as_bool()).unwrap_or(false);
            let client_ip = health.get("clientIp").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if hist_acc {
                if let Ok(id) = client.get("/identity", 8000) {
                    settings["identity"] = id;
                }
            }
            let wp = if health.get("apiVersion").and_then(|v| v.as_i64()).unwrap_or(0) >= 6 {
                client
                    .get("/model-profiles", 8000)
                    .ok()
                    .and_then(|p| p.get("worker").cloned())
                    .unwrap_or_else(|| json!({}))
            } else {
                json!({})
            };
            let conversations_val = client.get("/conversations", 15000)?;
            let conversations = conversations_val.as_array().cloned().unwrap_or_default();
            let perf = client.get("/models/perf", 8000).unwrap_or_else(|_| json!({}));

            let restore_conv_id = {
                let g = self.st.lock();
                if valid_id(&g.conversation_id) {
                    Some(g.conversation_id.clone())
                } else if valid_id(&g.last_job_id) {
                    client
                        .get(&format!("/jobs/{}", g.last_job_id), 8000)
                        .ok()
                        .and_then(|j| j.get("conversationId").and_then(|v| v.as_str()).map(|s| s.to_string()))
                } else {
                    conversations
                        .last()
                        .and_then(|c| c.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
                }
            };
            let restored = restore_conv_id
                .and_then(|cid| client.get(&format!("/conversations/{}", cid), 15000).ok());

            Ok(json!({
                "health": health,
                "settings": settings,
                "capabilities": caps,
                "workerProfile": wp,
                "conversations": conversations,
                "perf": perf,
                "historyAccess": hist_acc,
                "clientIp": client_ip,
                "restored": restored,
            }))
        })();

        match res {
            Ok(data) => {
                let want_standby = {
                    let mut g = self.st.lock();
                    g.connecting = false;
                    g.connected = true;
                    g.health = data["health"].clone();
                    g.server_settings = data["settings"].clone();
                    g.capabilities = data["capabilities"].clone();
                    g.worker_profile = data["workerProfile"].clone();
                    g.perf = data["perf"].clone();
                    g.history_access = data["historyAccess"].as_bool().unwrap_or(false);
                    g.client_ip = data["clientIp"].as_str().unwrap_or("").to_string();
                    g.conversations = data["conversations"].as_array().cloned().unwrap_or_default();
                    for k in [
                        "contextWindow",
                        "maxTokens",
                        "reasoningEffort",
                        "reasoningBudgetTokens",
                        "temperature",
                        "top_p",
                        "top_k",
                        "min_p",
                        "repeat_penalty",
                        "presence_penalty",
                        "frequency_penalty",
                    ] {
                        if let Some(v) = g.server_settings.get(k) {
                            g.gui[k] = v.clone();
                        }
                    }
                    let mut samp = SamplingValues::default();
                    sampling_from_json(&g.gui, &mut samp);
                    g.gui["samplingPreset"] = json!(preset_name_from_index(infer_preset_index(&samp)));
                    g.gui.get("pcStandby").and_then(|v| v.as_bool()).unwrap_or(false)
                };
                self.push_connection();
                self.emit_settings_sync();
                self.emit_event(json!({
                    "type": "perf",
                    "data": self.st.lock().perf,
                }));
                self.emit_event(json!({
                    "type": "jobs",
                    "conversations": self.st.lock().conversations,
                }));
                if let Some(conv) = data["restored"].as_object().map(|_| data["restored"].clone()) {
                    self.apply_loaded_conversation(conv, false);
                }
                self.push_log("サーバーに接続しました\r\n", "info");
                self.toast("ok", "サーバーに接続しました");
                if want_standby {
                    self.start_standby();
                }
            }
            Err(e) => {
                {
                    let mut g = self.st.lock();
                    g.connecting = false;
                    g.connected = false;
                }
                self.push_connection();
                self.emit_error_notice(&e);
            }
        }
    }

    fn apply_loaded_conversation(self: &Arc<Self>, raw_conv: Value, track_latest: bool) {
        let enriched = self.enrich_conversation(raw_conv);
        let cid = enriched.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let ro = enriched.get("readOnly").and_then(|v| v.as_bool()).unwrap_or(false);
        let latest_job = enriched
            .get("jobs")
            .and_then(|v| v.as_array())
            .and_then(|a| a.last())
            .cloned();

        let local_wd = {
            let mut g = self.st.lock();
            g.conversation_id = cid.clone();
            g.read_only = ro;
            if let Some(saved) = g.local_workdirs.get(&cid).and_then(|v| v.as_str()) {
                if validate_workdir(saved).is_ok() {
                    g.local_workdir = saved.to_string();
                }
            }
            g.conversation = Some(enriched.clone());
            if let Some(ref lj) = latest_job {
                g.active_job = Some(lj.clone());
                g.last_job_id = lj.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let st = lj.get("status").and_then(|v| v.as_str()).unwrap_or("");
                if st == "waiting_user" {
                    g.question_id = lj
                        .get("question")
                        .and_then(|q| q.get("id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                } else {
                    g.question_id.clear();
                }
            } else {
                g.active_job = None;
                g.question_id.clear();
            }
            g.local_workdir.clone()
        };
        self.persist_gui();
        {
            let lj_id = latest_job
                .as_ref()
                .and_then(|j| j.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let lj_st = latest_job
                .as_ref()
                .and_then(|j| j.get("status"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let busy = self.st.lock().busy;
            self.standby.lock().update_sync_cursor(&cid, lj_id, lj_st, busy);
        }
        self.emit_event(json!({
            "type": "conversation",
            "conversation": enriched,
            "readOnly": ro,
            "localWorkdir": local_wd,
        }));
        if valid_id(&cid) {
            let is_pending = latest_job
                .as_ref()
                .and_then(|j| j.get("status"))
                .and_then(|v| v.as_str())
                .is_some_and(|st| st == "running" || st == "queued");
            let mut cached_emit = Vec::new();
            let mut missing_paths = Vec::new();
            {
                let g = self.st.lock();
                if let Some(jobs) = enriched.get("jobs").and_then(|v| v.as_array()) {
                    for j in jobs.iter().rev() {
                        for key in ["attachments", "files"] {
                            if let Some(arr) = j.get(key).and_then(|v| v.as_array()) {
                                for a in arr {
                                    let p = a.get("path").and_then(|v| v.as_str()).unwrap_or("");
                                    if p.is_empty() {
                                        continue;
                                    }
                                    if let Some(cached) = g
                                        .sent_attachments_cache
                                        .get(&format!("{}:{}", cid, p))
                                        .or_else(|| g.sent_attachments_cache.get(p))
                                    {
                                        cached_emit.push(cached.clone());
                                    } else if !is_pending
                                        && is_image_filename(p)
                                        && missing_paths.len() < 12
                                        && !missing_paths.iter().any(|x| x == p)
                                    {
                                        missing_paths.push(p.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if !cached_emit.is_empty() {
                self.emit_sent_thumbs(&cid, &cached_emit);
            }
            if !missing_paths.is_empty() {
                let app = Arc::clone(self);
                let cid_bg = cid.clone();
                thread::spawn(move || app.fetch_conversation_attachment_thumbs(cid_bg, missing_paths));
            }
        }
        if track_latest {
            if let Some(lj) = latest_job {
                let st = lj.get("status").and_then(|v| v.as_str()).unwrap_or("");
                if st == "running" || st == "queued" {
                    if let Some(jid) = lj.get("id").and_then(|v| v.as_str()) {
                        self.poll_existing_job(jid.to_string());
                    }
                }
            }
        }
    }

    fn do_open_conversation(self: &Arc<Self>, cid: &str) {
        if !valid_id(cid) {
            return;
        }
        self.emit_event(json!({ "type": "loading", "scope": "conversation", "active": true }));
        let url = self.current_api_url();
        let res = ApiClient::new(&url).and_then(|c| c.get(&format!("/conversations/{}", cid), 15000));
        self.emit_event(json!({ "type": "loading", "scope": "conversation", "active": false }));
        match res {
            Ok(conv) => self.apply_loaded_conversation(conv, true),
            Err(e) => self.emit_error_notice(&e),
        }
    }

    fn do_load_full_job(self: &Arc<Self>, jid: &str) {
        if !valid_id(jid) {
            return;
        }
        let url = self.current_api_url();
        if let Ok(job) = ApiClient::new(&url).and_then(|c| c.get(&format!("/jobs/{}?full=1", jid), 20000)) {
            let cid = job.get("conversationId").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !cid.is_empty() {
                if let Ok(conv) = ApiClient::new(&url).and_then(|c| c.get(&format!("/conversations/{}", cid), 15000)) {
                    let mut c_obj = conv;
                    if let Some(jobs) = c_obj.get_mut("jobs").and_then(|v| v.as_array_mut()) {
                        for row in jobs.iter_mut() {
                            if row.get("id").and_then(|v| v.as_str()) == Some(jid) {
                                *row = job.clone();
                            }
                        }
                    }
                    self.apply_loaded_conversation(c_obj, false);
                }
            }
        }
    }

    fn do_refresh_history(&self) {
        self.emit_event(json!({ "type": "loading", "scope": "history", "active": true }));
        let url = self.current_api_url();
        let res = ApiClient::new(&url).and_then(|c| c.get("/conversations", 15000));
        self.emit_event(json!({ "type": "loading", "scope": "history", "active": false }));
        match res {
            Ok(convs) => {
                let list = convs.as_array().cloned().unwrap_or_default();
                self.st.lock().conversations = list.clone();
                self.emit_event(json!({ "type": "jobs", "conversations": list }));
            }
            Err(e) => self.emit_error_notice(&e),
        }
    }

    fn do_refresh_perf(&self) {
        let url = self.current_api_url();
        if let Ok(perf) = ApiClient::new(&url).and_then(|c| c.get("/models/perf", 10000)) {
            self.st.lock().perf = perf.clone();
            self.emit_event(json!({ "type": "perf", "data": perf }));
        }
    }

    fn do_start_server(self: &Arc<Self>) {
        let res = self
            .connect_first_healthy()
            .and_then(|(_, c, _)| c.post("/models/start", &json!({}), 120_000));
        match res {
            Ok(_) => {
                self.push_log("モデルを起動しました\r\n", "info");
                self.toast("ok", "モデルを起動しました");
                self.do_connect();
            }
            Err(e) => self.emit_error_notice(&e),
        }
    }

    fn do_stop_server(&self) {
        let res = self
            .connect_first_healthy()
            .and_then(|(_, c, _)| c.post("/models/stop", &json!({}), 60_000));
        match res {
            Ok(_) => {
                self.toast("ok", "モデルを停止しました");
                self.push_log("モデルを停止しました\r\n", "info");
            }
            Err(e) => self.emit_error_notice(&e),
        }
    }

    fn do_restart_server(self: &Arc<Self>) {
        let res = self
            .connect_first_healthy()
            .and_then(|(_, c, _)| c.post("/models/restart", &json!({}), 120_000));
        match res {
            Ok(_) => {
                self.push_log("モデルを再起動しました\r\n", "info");
                self.toast("ok", "モデルを再起動しました");
                self.do_connect();
            }
            Err(e) => self.emit_error_notice(&e),
        }
    }

    fn do_apply_settings(&self, payload: Value) {
        let mut srv_patch = Map::new();
        for k in [
            "contextWindow",
            "maxTokens",
            "reasoningEffort",
            "reasoningBudgetTokens",
            "temperature",
            "top_p",
            "top_k",
            "min_p",
            "repeat_penalty",
            "presence_penalty",
            "frequency_penalty",
        ] {
            if let Some(v) = payload.get(k) {
                srv_patch.insert(k.to_string(), v.clone());
            }
        }
        if let Some(preset) = payload.get("samplingPreset") {
            self.st.lock().gui["samplingPreset"] = preset.clone();
        }
        let connected = self.st.lock().connected;
        if connected && !srv_patch.is_empty() {
            let url = self.current_api_url();
            match ApiClient::new(&url).and_then(|c| c.post("/settings", &Value::Object(srv_patch), 420_000)) {
                Ok(updated) => {
                    {
                        let mut g = self.st.lock();
                        g.server_settings = updated.clone();
                        for (k, v) in updated.as_object().into_iter().flatten() {
                            g.gui[k] = v.clone();
                        }
                        let mut samp = SamplingValues::default();
                        sampling_from_json(&g.gui, &mut samp);
                        g.gui["samplingPreset"] = json!(preset_name_from_index(infer_preset_index(&samp)));
                    }
                    self.persist_gui();
                    self.emit_settings_sync();
                    self.toast("ok", "設定をサーバーへ適用しました");
                }
                Err(e) => self.emit_error_notice(&e),
            }
        } else {
            {
                let mut g = self.st.lock();
                for (k, v) in srv_patch {
                    g.gui[k] = v;
                }
                let mut samp = SamplingValues::default();
                sampling_from_json(&g.gui, &mut samp);
                g.gui["samplingPreset"] = json!(preset_name_from_index(infer_preset_index(&samp)));
            }
            self.persist_gui();
            self.emit_settings_sync();
            self.toast("ok", "ローカル設定を保存しました");
        }
    }

    fn do_autotune(&self, mode: &str) {
        let url = self.current_api_url();
        let Ok(client) = ApiClient::new(&url) else {
            self.emit_event(json!({ "type": "autotune", "running": false, "error": "URL 不正" }));
            return;
        };
        if let Err(e) = client.post("/tune/auto", &json!({ "mode": mode }), 15000) {
            self.emit_event(json!({ "type": "autotune", "running": false, "error": e }));
            return;
        }
        self.emit_event(json!({ "type": "autotune", "running": true, "stage": "開始中…", "progressPercent": 0 }));
        for _ in 0..180 {
            if self.closing.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(Duration::from_secs(1));
            if let Ok(st) = client.get("/tune/status", 10000) {
                let s = st.get("status").and_then(|v| v.as_str()).unwrap_or("");
                let stage = st.get("currentStage").and_then(|v| v.as_str()).unwrap_or("");
                let pct = st.get("progressPercent").and_then(|v| v.as_i64()).unwrap_or(0);
                let done = matches!(s, "completed" | "failed" | "cancelled");
                self.emit_event(json!({
                    "type": "autotune",
                    "running": !done,
                    "stage": stage,
                    "status": s,
                    "progressPercent": pct,
                    "result": st,
                }));
                if done {
                    if s == "completed" && !st.get("alreadyOptimal").and_then(|v| v.as_bool()).unwrap_or(false) {
                        if let Ok(applied) = client.post("/tune/apply", &json!({}), 120_000) {
                            if let Some(srv) = applied.get("settings").filter(|v| v.is_object()) {
                                let mut g = self.st.lock();
                                g.server_settings = srv.clone();
                                for (k, v) in srv.as_object().into_iter().flatten() {
                                    g.gui[k] = v.clone();
                                }
                            }
                            self.persist_gui();
                            self.emit_settings_sync();
                        }
                    }
                    break;
                }
            }
        }
    }

    fn show_text_modal(&self, title: &str, text: &str) {
        self.emit_event(json!({
            "type": "attachment_preview",
            "title": title,
            "kind": "text",
            "text": text,
        }));
    }

    fn emit_optimistic_job(&self, pending_id: &str, goal: &str, files: &[Value]) {
        let cid = self.st.lock().conversation_id.clone();
        self.emit_event(json!({
            "type": "job_update",
            "liveText": "依頼を送信しています…",
            "job": {
                "id": pending_id,
                "conversationId": cid,
                "status": "queued",
                "stage": "queued",
                "goal": goal,
                "files": files,
                "attachments": files,
                "activity": [],
                "_optimistic": true,
            }
        }));
    }

    fn handle_send_message(self: &Arc<Self>, raw_text: String) {
        let trimmed = raw_text.trim();
        if let Ok(cmd) = parse_workdir_command(trimmed) {
            match cmd {
                WorkdirCommand::Other => {}
                WorkdirCommand::Show => {
                    let wd = self.st.lock().local_workdir.clone();
                    let label = if wd.is_empty() {
                        "現在の作業フォルダー: ジョブごとの自動ワークスペース".to_string()
                    } else {
                        format!("現在の作業フォルダー: {}", wd)
                    };
                    self.toast("ok", &label);
                    return;
                }
                WorkdirCommand::Choose => {
                    self.handle_ipc(json!({ "cmd": "pick_workdir" }));
                    return;
                }
                WorkdirCommand::Reset => {
                    self.set_current_workdir("");
                    self.toast("ok", "作業フォルダーを標準に戻しました");
                    return;
                }
                WorkdirCommand::Set(p) => {
                    match validate_workdir(&p) {
                        Ok(wd) => {
                            self.set_current_workdir(&wd.to_string_lossy());
                            self.toast("ok", &format!("作業フォルダー: {}", wd.display()));
                        }
                        Err(e) => self.emit_error_notice(&e),
                    }
                    return;
                }
            }
        }

        if let Some(after) = trimmed.strip_prefix('/') {
            let (head, arg) = match after.split_once(char::is_whitespace) {
                Some((c, rest)) => (c, rest.trim()),
                None => (after, ""),
            };
            let cmd = head.to_ascii_lowercase();
            match cmd.as_str() {
                "clear" | "new" => {
                    self.handle_ipc(json!({ "cmd": "new_chat" }));
                    self.toast("ok", "新しい会話を開始しました");
                    return;
                }
                "reset" => {
                    self.handle_ipc(json!({ "cmd": "reset_workdir" }));
                    return;
                }
                "help" => {
                    let guide = "\
[agy スラッシュコマンド (Direct 27B)]\n\
/plan <task>         事前調査・変更手順・検証計画を作成\n\
/goal <task>         ビルド・検証完了まで自律ループで徹底遂行\n\
/compact [focus]     現在の会話を圧縮してコンテキスト負荷を軽減\n\
/grill-me <topic>    実装前に仕様の曖昧さ・選択論点を質問で明確化\n\
/boost <task>        推論強化・境界点検を徹底して高品質に解決\n\
/teamwork <task>     設計→実装→検証を工程分離して自己レビュー完遂\n\
/learn <note|list>   知見の保存・一覧表示 (/learn delete <id> で削除)\n\
/browser <task>      Web検索・画面状態確認を優先して調査・実行\n\
/schedule <...>      予定登録・一覧 (/schedule list | cancel <id> | in 10m <task> | every 1h <task>)\n\
/mode <mode>         動作モード切替 (standard|chat|coding|pc_control)\n\
/effort <level>      推論負荷変更 (none|low|medium|xhigh)\n\
/models <subcmd>     モデル制御 (status|start|stop|restart|perf)\n\
/status              接続・モデル・スケジュールの現在状態を表示\n\
/clear               新しい会話を開始\n\n\
[ローカル / パネルコマンド]\n\
/workdir [path]      作業フォルダーの表示・変更 (/reset で解除)\n\
/tools /settings /connect";
                    self.show_text_modal("スラッシュコマンド一覧 (/help)", guide);
                    return;
                }
                "mode" => {
                    if is_valid_work_mode(arg) {
                        self.handle_ipc(json!({ "cmd": "switch_work_mode", "mode": arg }));
                        self.toast("ok", &format!("動作モードを変更しました: {}", arg));
                    } else {
                        self.toast("warn", "使い方: /mode <standard|chat|coding|pc_control>");
                    }
                    return;
                }
                "effort" => {
                    if is_valid_effort(arg) {
                        let eff = arg.to_string();
                        let app = Arc::clone(self);
                        thread::spawn(move || app.do_apply_settings(json!({ "reasoningEffort": eff })));
                    } else {
                        self.toast("warn", "使い方: /effort <none|low|medium|xhigh>");
                    }
                    return;
                }
                "status" => {
                    let app = Arc::clone(self);
                    thread::spawn(move || {
                        let res = app.connect_first_healthy().map(|(url, c, health)| {
                            let ms = c.get("/models/status", 8000).unwrap_or_else(|_| json!({}));
                            let sch = c.get("/schedules", 8000).unwrap_or_else(|_| json!({}));
                            json!({ "url": url, "health": health, "models": ms, "schedules": sch })
                        });
                        match res {
                            Ok(v) => app.show_text_modal("サーバー状態 (/status)", &serde_json::to_string_pretty(&v).unwrap_or_default()),
                            Err(e) => app.emit_error_notice(&e),
                        }
                    });
                    return;
                }
                "models" => {
                    let sub = if arg.is_empty() { "status" } else { arg }.to_ascii_lowercase();
                    let app = Arc::clone(self);
                    match sub.as_str() {
                        "start" => { thread::spawn(move || app.do_start_server()); }
                        "stop" => { thread::spawn(move || app.do_stop_server()); }
                        "restart" => { thread::spawn(move || app.do_restart_server()); }
                        "status" | "perf" => {
                            let path = if sub == "perf" { "/models/perf" } else { "/models/status" };
                            thread::spawn(move || match app.connect_first_healthy().and_then(|(_, c, _)| c.get(path, 15000)) {
                                Ok(v) => app.show_text_modal(&format!("モデル情報 (/models {})", sub), &serde_json::to_string_pretty(&v).unwrap_or_default()),
                                Err(e) => app.emit_error_notice(&e),
                            });
                        }
                        _ => self.toast("warn", "使い方: /models [status|start|stop|restart|perf]"),
                    }
                    return;
                }
                "compact" => {
                    let (connected, cid) = {
                        let g = self.st.lock();
                        (g.connected, g.conversation_id.clone())
                    };
                    if !connected {
                        self.emit_error_notice("サーバーへ未接続です");
                        return;
                    }
                    if !valid_id(&cid) {
                        self.toast("warn", "圧縮対象の会話がまだありません");
                        return;
                    }
                    let focus = arg.to_string();
                    let app = Arc::clone(self);
                    thread::spawn(move || {
                        let body = if focus.is_empty() { json!({}) } else { json!({ "focus": focus }) };
                        match app.connect_first_healthy().and_then(|(_, c, _)| c.post(&format!("/conversations/{}/compact", cid), &body, 30_000)) {
                            Ok(res) => {
                                app.toast("ok", "会話を圧縮しました (/compact)");
                                let summary = res.get("summary").and_then(|v| v.as_str()).unwrap_or("");
                                app.show_text_modal("会話サマリー (/compact)", if summary.is_empty() { "圧縮完了" } else { summary });
                            }
                            Err(e) => app.emit_error_notice(&e),
                        }
                    });
                    return;
                }
                "learn" => {
                    let arg_s = arg.to_string();
                    let app = Arc::clone(self);
                    thread::spawn(move || {
                        let res = app.connect_first_healthy().and_then(|(_, c, _)| {
                            let trimmed_arg = arg_s.trim();
                            if trimmed_arg.is_empty() || trimmed_arg.eq_ignore_ascii_case("list") {
                                c.get("/memory", 15000).map(|v| ("list", v))
                            } else if let Some(rest) = trimmed_arg
                                .strip_prefix("delete ")
                                .or_else(|| trimmed_arg.strip_prefix("remove "))
                                .or_else(|| trimmed_arg.strip_prefix("rm "))
                            {
                                c.delete(&format!("/memory/{}", rest.trim()), 15000).map(|v| ("delete", v))
                            } else {
                                c.post("/memory", &json!({ "text": trimmed_arg }), 15000).map(|v| ("add", v))
                            }
                        });
                        match res {
                            Ok(("add", v)) => {
                                let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
                                app.toast("ok", &format!("学習メモリに保存しました ({})", id));
                            }
                            Ok(("delete", v)) => {
                                let id = v.get("deleted").and_then(|x| x.as_str()).unwrap_or("");
                                app.toast("ok", &format!("学習メモリを削除しました ({})", id));
                            }
                            Ok((_, v)) => {
                                app.show_text_modal("学習メモリ一覧 (/learn)", &serde_json::to_string_pretty(&v).unwrap_or_default());
                            }
                            Err(e) => app.emit_error_notice(&e),
                        }
                    });
                    return;
                }
                "schedule" => {
                    let arg_s = arg.to_string();
                    let cid = self.st.lock().conversation_id.clone();
                    let app = Arc::clone(self);
                    thread::spawn(move || {
                        let trimmed_arg = arg_s.trim();
                        if trimmed_arg.is_empty() || trimmed_arg.eq_ignore_ascii_case("list") {
                            match app.connect_first_healthy().and_then(|(_, c, _)| c.get("/schedules", 15000)) {
                                Ok(v) => app.show_text_modal("スケジュール一覧 (/schedule)", &serde_json::to_string_pretty(&v).unwrap_or_default()),
                                Err(e) => app.emit_error_notice(&e),
                            }
                            return;
                        }
                        if let Some(rest) = trimmed_arg
                            .strip_prefix("cancel ")
                            .or_else(|| trimmed_arg.strip_prefix("delete "))
                            .or_else(|| trimmed_arg.strip_prefix("remove "))
                            .or_else(|| trimmed_arg.strip_prefix("rm "))
                        {
                            match app.connect_first_healthy().and_then(|(_, c, _)| c.delete(&format!("/schedules/{}", rest.trim()), 15000)) {
                                Ok(_) => app.toast("ok", "スケジュールをキャンセルしました"),
                                Err(e) => app.emit_error_notice(&e),
                            }
                            return;
                        }
                        let mut parts = trimmed_arg.split_whitespace();
                        let kw = parts.next().unwrap_or("").to_ascii_lowercase();
                        let amt = parts.next().unwrap_or("");
                        let rest_goal = trimmed_arg
                            .find(amt)
                            .and_then(|pos| (!amt.is_empty()).then(|| trimmed_arg[pos + amt.len()..].trim()))
                            .unwrap_or("");
                        if rest_goal.is_empty() || !matches!(kw.as_str(), "in" | "every") {
                            app.toast("warn", "使い方: /schedule [list | cancel <id> | in 10m <task> | every 1h <task>]");
                            return;
                        }
                        let spec = format!("{} {}", kw, amt);
                        let goal = rest_goal;
                        let mut body = json!({ "spec": spec, "goal": goal });
                        if valid_id(&cid) {
                            body["conversationId"] = json!(cid);
                        }
                        match app.connect_first_healthy().and_then(|(_, c, _)| c.post("/schedules", &body, 15000)) {
                            Ok(v) => {
                                let sid = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
                                app.toast("ok", &format!("スケジュール登録: {} ({})", sid, spec));
                            }
                            Err(e) => app.emit_error_notice(&e),
                        }
                    });
                    return;
                }
                "plan" | "goal" | "grill-me" | "boost" | "teamwork" | "browser" => {
                    let (connected, read_only, busy) = {
                        let g = self.st.lock();
                        (g.connected, g.read_only, g.busy)
                    };
                    if !connected {
                        let n = plain_notice(NOT_CONNECTED_MESSAGE);
                        self.emit_event(json!({ "type": "notice", "level": "warn", "label": n.label }));
                        return;
                    }
                    if read_only {
                        let n = plain_notice(READ_ONLY_MESSAGE);
                        self.emit_event(json!({ "type": "notice", "level": "warn", "label": n.label }));
                        return;
                    }
                    if busy {
                        self.toast("warn", "別の依頼が実行中です。完了後または /clear 後に実行してください");
                        return;
                    }
                    if arg.is_empty() {
                        self.toast("warn", &format!("使い方: /{} <指示内容>", cmd));
                        return;
                    }
                    let goal = arg.to_string();
                    let sc = cmd.clone();
                    let (files, cid) = {
                        let mut g = self.st.lock();
                        g.last_submitted_goal = trimmed.to_string();
                        g.last_submitted_files = g.attachments.clone();
                        (std::mem::take(&mut g.attachments), g.conversation_id.clone())
                    };
                    self.push_attachments();
                    self.cache_sent_attachments(&cid, &files);
                    let pending_id = format!("pending-{}", uuid::Uuid::new_v4());
                    let display_goal = format!("/{} {}", sc, goal);
                    self.emit_optimistic_job(&pending_id, &display_goal, &files);
                    let app = Arc::clone(self);
                    thread::spawn(move || app.submit_and_poll_job(goal, files, None, Some(sc), pending_id));
                    return;
                }
                _ => {}
            }
        }

        let (connected, read_only, busy, qid, has_attachments) = {
            let g = self.st.lock();
            (g.connected, g.read_only, g.busy, g.question_id.clone(), !g.attachments.is_empty())
        };
        if !connected {
            let n = plain_notice(NOT_CONNECTED_MESSAGE);
            self.emit_event(json!({ "type": "notice", "level": "warn", "label": n.label }));
            return;
        }
        if read_only {
            let n = plain_notice(READ_ONLY_MESSAGE);
            self.emit_event(json!({ "type": "notice", "level": "warn", "label": n.label }));
            return;
        }
        if trimmed.is_empty() && !has_attachments {
            let n = plain_notice(EMPTY_GOAL_MESSAGE);
            self.emit_event(json!({ "type": "notice", "level": "info", "label": n.label }));
            return;
        }
        if !qid.is_empty() && !busy {
            self.handle_answer_question(qid, trimmed.to_string());
            return;
        }
        if busy {
            let jid = self.st.lock().last_job_id.clone();
            if valid_id(&jid) {
                let url = self.current_api_url();
                let txt = trimmed.to_string();
                let app = Arc::clone(self);
                thread::spawn(move || {
                    if let Err(e) = ApiClient::new(&url)
                        .and_then(|c| c.post(&format!("/jobs/{}/interrupt", jid), &json!({ "text": txt }), 15000))
                    {
                        app.emit_error_notice(&e);
                    } else {
                        app.toast("ok", "追加指示を送りました");
                    }
                });
            }
            return;
        }

        let goal = if trimmed.is_empty() {
            "添付したファイルを確認してください。".to_string()
        } else {
            trimmed.to_string()
        };
        let (files, cid) = {
            let mut g = self.st.lock();
            g.last_submitted_goal = goal.clone();
            g.last_submitted_files = g.attachments.clone();
            (std::mem::take(&mut g.attachments), g.conversation_id.clone())
        };
        self.push_attachments();
        self.cache_sent_attachments(&cid, &files);
        let pending_id = format!("pending-{}", uuid::Uuid::new_v4());
        self.emit_optimistic_job(&pending_id, &goal, &files);
        let app = Arc::clone(self);
        thread::spawn(move || app.submit_and_poll_job(goal, files, None, None, pending_id));
    }

    fn handle_cancel_job(self: &Arc<Self>) {
        let (busy, qid, last_job) = {
            let g = self.st.lock();
            (g.busy, g.question_id.clone(), g.last_job_id.clone())
        };
        if !busy && !qid.is_empty() && valid_id(&last_job) {
            let url = self.current_api_url();
            let app = Arc::clone(self);
            thread::spawn(move || {
                let _ = ApiClient::new(&url).and_then(|c| c.post(&format!("/jobs/{}/cancel", last_job), &json!({}), 15000));
                app.do_refresh_history();
            });
            return;
        }
        self.cancel_req.store(true, Ordering::SeqCst);
        self.job_bridge.lock().cancel_io();
        self.push_log("中断を要求しています…\r\n", "warn");
        self.toast("warn", "ジョブの中断を要求しました");
    }

    fn handle_answer_question(self: &Arc<Self>, qid: String, answer: String) {
        let (jid, url) = {
            let mut g = self.st.lock();
            g.pending_answer_job = g.last_job_id.clone();
            g.busy = true;
            g.question_id.clear();
            (
                g.last_job_id.clone(),
                g.gui.get("api").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            )
        };
        self.push_connection();
        let app = Arc::clone(self);
        thread::spawn(move || {
            let body = json!({ "questionId": qid, "answer": answer });
            match ApiClient::new(&url).and_then(|c| c.post(&format!("/jobs/{}/answer", jid), &body, 15000)) {
                Ok(_) => app.poll_existing_job(jid),
                Err(e) => {
                    app.st.lock().busy = false;
                    app.push_connection();
                    app.emit_error_notice(&e);
                }
            }
        });
    }

    fn submit_and_poll_job(self: &Arc<Self>, goal: String, files: Vec<Value>, _parent: Option<String>, slash_cmd: Option<String>, pending_id: String) {
        if goal.encode_utf16().count() > 6000 {
            self.emit_event(json!({ "type": "job_removed", "jobId": pending_id }));
            self.emit_error_notice("入力が長すぎます（6000文字まで）");
            return;
        }
        let url = self.current_api_url();
        self.cancel_req.store(false, Ordering::SeqCst);
        {
            let mut g = self.st.lock();
            g.busy = true;
            g.active_job_pending = true;
        }
        self.push_connection();

        let resolved = self.staged.resolve_for_send(&files, 3000);
        let setup_res = (|| -> Result<(String, Value), String> {
            let (
                gui,
                srv,
                caps,
                conv_id,
                pc_win_id,
                pc_win_title,
                local_wd,
                read_roots,
                cli_override,
            ) = {
                let g = self.st.lock();
                (
                    g.gui.clone(),
                    g.server_settings.clone(),
                    g.capabilities.clone(),
                    g.conversation_id.clone(),
                    g.selected_window_id,
                    g.selected_window_title.clone(),
                    g.local_workdir.clone(),
                    g.read_roots.clone(),
                    g.cli_override,
                )
            };
            let work_mode = gui.get("workMode").and_then(|v| v.as_str()).unwrap_or("standard");
            let model_role = if cli_override {
                gui.get("modelRole").and_then(|v| v.as_str()).unwrap_or("planner")
            } else {
                "planner"
            };
            let plan_mode = if cli_override {
                gui.get("planMode").and_then(|v| v.as_bool()).unwrap_or(false)
            } else {
                false
            };
            let server_ws_cap = caps.get("workspace").and_then(|v| v.as_bool()).unwrap_or(false);
            let use_ws = gui.get("workspaceEnabled").and_then(|v| v.as_bool()).unwrap_or(true)
                && server_ws_cap;
            let desktop_allowed = gui.get("pcEnabled").and_then(|v| v.as_bool()).unwrap_or(false);
            let local_allowed = gui.get("localCliEnabled").and_then(|v| v.as_bool()).unwrap_or(true);
            let run_on_pc = gui.get("runOnPc").and_then(|v| v.as_bool()).unwrap_or(false);
            let server_win_avail = caps
                .get("serverWindows")
                .and_then(|s| s.get("available"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let local_cap = caps.get("localCli").and_then(|v| v.as_bool()).unwrap_or(false);

            let mut options = json!({
                "contextWindow": srv.get("contextWindow").and_then(|v| v.as_i64()).or_else(|| gui.get("contextWindow").and_then(|v| v.as_i64())).unwrap_or(16384),
                "maxIterations": 0,
                "modelRole": model_role,
            });
            if work_mode != "chat" && work_mode != "pc_control" {
                options["maxTokens"] = json!(srv
                    .get("maxTokens")
                    .and_then(|v| v.as_i64())
                    .or_else(|| gui.get("maxTokens").and_then(|v| v.as_i64()))
                    .unwrap_or(1800));
                if let Some(re) = srv.get("reasoningEffort") {
                    options["reasoningEffort"] = re.clone();
                }
                if let Some(rb) = srv.get("reasoningBudgetTokens") {
                    options["reasoningBudgetTokens"] = rb.clone();
                }
                let mut samp = SamplingValues::default();
                sampling_from_json(&srv, &mut samp);
                sampling_to_json(&samp, &mut options);
            }
            if work_mode != "standard" {
                options["workMode"] = json!(work_mode);
            }
            if model_role == "planner" {
                options["planMode"] = json!(if slash_cmd.is_some() || work_mode == "chat" || work_mode == "pc_control" { false } else { plan_mode });
            }
            if let Some(ref sc) = slash_cmd {
                options["slashCommand"] = json!(sc);
            }
            let want_ws = (use_ws && work_mode != "chat")
                || ((!files.is_empty() || slash_cmd.is_some()) && server_ws_cap);
            if want_ws {
                options["execution"] = json!("workspace");
            }
            let mut machine = String::new();
            if server_win_avail && want_ws {
                machine = if run_on_pc || desktop_allowed {
                    "user-pc"
                } else {
                    "server-windows"
                }
                .into();
            }
            if !machine.is_empty() {
                options["machine"] = json!(machine);
            }
            let use_local = (work_mode != "pc_control")
                && (work_mode != "chat")
                && want_ws
                && local_allowed
                && machine != "server-windows"
                && local_cap;
            if valid_id(&conv_id) {
                options["conversationId"] = json!(conv_id);
            }

            let open_desktop = desktop_allowed && pc_win_id != 0 && work_mode != "chat";
            if open_desktop || use_local {
                let ws_path = if use_local {
                    resolve_cli_workspace(&self.root, &local_wd, "gui")?
                } else {
                    PathBuf::new()
                };
                let app_alive = Arc::clone(self);
                let app_emit = Arc::clone(self);
                let sid = self.job_bridge.lock().open_and_start(
                    &url,
                    if open_desktop { pc_win_id } else { 0 },
                    if open_desktop { &pc_win_title } else { "" },
                    ws_path,
                    read_roots,
                    open_desktop,
                    use_local,
                    move || !app_alive.closing.load(Ordering::SeqCst) && app_alive.st.lock().active_job_pending,
                    move |notice| match notice {
                        StandbyNotice::Log(l) => app_emit.push_log(&l, "info"),
                        StandbyNotice::State(s) => app_emit.emit_event(json!({ "type": "job_update", "liveText": s })),
                        _ => {}
                    },
                )?;
                options["desktopSession"] = json!(sid);
            }

            let mut body = json!({ "goal": goal, "options": options });
            if !resolved.upload_ids.is_empty() {
                body["uploadIds"] = json!(resolved.upload_ids);
            }
            if !resolved.inline_files.is_empty() {
                body["files"] = Value::Array(resolved.inline_files);
            }
            let client = ApiClient::new(&url)?;
            let created = client.post("/jobs", &body, 15000)?;
            let jid = created
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or("job id がありません")?
                .to_string();
            Ok((jid, created))
        })();

        match setup_res {
            Ok((jid, created)) => {
                let cid = created
                    .get("conversationId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if valid_id(&cid) {
                    self.st.lock().conversation_id = cid.clone();
                }
                self.cache_sent_attachments(&cid, &files);
                let mut created_ui = created;
                if !files.is_empty() && created_ui.get("attachments").is_none() {
                    created_ui["attachments"] = Value::Array(files);
                }
                self.emit_event(json!({
                    "type": "job_update",
                    "replaceJobId": pending_id,
                    "liveText": "モデルへ接続しています…",
                    "job": created_ui,
                }));
                let app_hist = Arc::clone(self);
                thread::spawn(move || app_hist.do_refresh_history());
                self.poll_existing_job(jid);
            }
            Err(e) => {
                self.job_bridge.lock().stop_and_close();
                {
                    let mut g = self.st.lock();
                    g.active_job_pending = false;
                    g.busy = false;
                }
                self.emit_event(json!({ "type": "job_removed", "jobId": pending_id }));
                self.push_connection();
                self.emit_error_notice(&e);
            }
        }
    }

    fn poll_existing_job(self: &Arc<Self>, jid: String) {
        let url = self.current_api_url();
        let Ok(client) = ApiClient::new(&url).map(Arc::new) else {
            {
                let mut g = self.st.lock();
                g.active_job_pending = false;
                g.busy = false;
            }
            self.push_connection();
            return;
        };
        *self.active_api.lock() = Some(Arc::clone(&client));
        {
            let mut g = self.st.lock();
            g.active_job_pending = true;
            g.last_job_id = jid.clone();
            g.busy = true;
        }
        self.push_connection();

        let mut sent_cancel = false;
        let mut prev_stage = String::new();
        let mut consec_err = 0u32;

        loop {
            if self.closing.load(Ordering::SeqCst) {
                break;
            }
            if self.cancel_req.load(Ordering::SeqCst) && !sent_cancel {
                let _ = client.post(&format!("/jobs/{}/cancel", jid), &json!({}), 10000);
                sent_cancel = true;
            }
            let full_flag = self.st.lock().full_watch_jobs.contains(&jid);
            let path = if full_flag {
                format!("/jobs/{}?full=1", jid)
            } else {
                format!("/jobs/{}", jid)
            };
            let job = match client.get(&path, 15000) {
                Ok(j) => {
                    consec_err = 0;
                    j
                }
                Err(e) => {
                    consec_err += 1;
                    if consec_err > 20 {
                        self.emit_error_notice(&e);
                        break;
                    }
                    thread::sleep(Duration::from_millis(400));
                    continue;
                }
            };

            let status = job.get("status").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let stage = job.get("stage").and_then(|v| v.as_str()).unwrap_or(&status).to_string();
            let pending = status == "running" || status == "queued";
            let lv = liveness(&job);
            let work_mode_str = job
                .get("options")
                .and_then(|o| o.get("workMode"))
                .and_then(|v| v.as_str());
            let pc_ctrl = work_mode_str == Some("pc_control");
            let chat_mode = work_mode_str == Some("chat");

            {
                let mut g = self.st.lock();
                let now = epoch_ms_now();
                g.active_job = Some(job.clone());
                g.active_job_pending = pending;
                g.live_clock.observe(lv.newest_ms, now);
                g.live_newest_ms = lv.newest_ms;
                g.live_step_ms = lv.step_start_ms;
                g.live_touched_ms = now;
                g.live_pc_control = pc_ctrl;
                if status == "waiting_user" {
                    g.question_id = job
                        .get("question")
                        .and_then(|q| q.get("id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                } else {
                    g.question_id.clear();
                }
            }

            if stage != prev_stage {
                let log_line = stage_log_line(&stage);
                if !log_line.is_empty() {
                    self.push_log(&format!("{}\r\n", log_line), "info");
                }
                prev_stage = stage.clone();
            }

            let mut enriched_job = job.clone();
            let has_job_tps = enriched_job
                .get("perf")
                .and_then(|p| p.get("tokensPerSec"))
                .and_then(|v| v.as_f64())
                .is_some_and(|v| v.is_finite() && v > 0.0);
            if !pending && !has_job_tps {
                if let Ok(perf) = client.get("/models/perf", 5000) {
                    if let Some(tps) = perf
                        .get("tokensPerSec")
                        .or_else(|| perf.get("lastTokensPerSec"))
                        .and_then(|v| v.as_f64())
                        .filter(|v| v.is_finite() && *v > 0.0)
                    {
                        enriched_job["_perf"] = json!({ "tokensPerSec": tps });
                    }
                    self.st.lock().perf = perf.clone();
                    self.emit_event(json!({ "type": "perf", "data": perf }));
                }
            }
            let rc = result_card(&job, pc_ctrl);
            enriched_job["_resultCard"] = json!({ "line": rc.line, "badge": rc.badge });

            let mut panels: Vec<WorkerPanel> = Vec::new();
            let act_rows = job
                .get("activity")
                .and_then(|v| v.as_array())
                .map(|a| a.as_slice())
                .unwrap_or(&[]);
            collect_worker_panels(&mut panels, &jid, act_rows, &status);
            self.emit_event(json!({ "type": "workers", "panels": panels }));

            let live_text = execution_stage_label(&job);
            let end_notice = if !pending && status != "waiting_user" {
                let res_obj = job.get("result").cloned().unwrap_or_else(|| json!({}));
                let achieved = if res_obj.get("achieved").and_then(|v| v.as_bool()) == Some(true) {
                    1
                } else {
                    0
                };
                let err_txt = job.get("error").and_then(|v| v.as_str()).unwrap_or("");
                let stop_r = res_obj.get("stopReason").and_then(|v| v.as_str()).unwrap_or("");
                let detail = res_obj.get("detail").and_then(|v| v.as_str()).unwrap_or("");
                job_end_notice(&status, achieved, err_txt, stop_r, detail)
            } else {
                None
            };

            let notice_json = end_notice.map(|n| {
                json!({
                    "level": if status == "completed" { "warn" } else { "error" },
                    "label": n.label,
                    "hint": n.hint,
                    "raw": n.raw,
                    "retry": n.retry,
                })
            });

            self.emit_event(json!({
                "type": "job_update",
                "job": enriched_job,
                "liveText": live_text,
                "resultCard": { "line": rc.line, "badge": rc.badge },
                "notice": notice_json,
            }));

            if !pending {
                break;
            }
            thread::sleep(Duration::from_millis(if chat_mode {
                80
            } else if pc_ctrl {
                150
            } else {
                400
            }));
        }

        self.job_bridge.lock().stop_and_close();
        *self.active_api.lock() = None;
        {
            let mut g = self.st.lock();
            g.active_job_pending = false;
            g.busy = false;
            g.pending_answer_job.clear();
        }
        self.push_connection();
        let should_flash = self
            .st
            .lock()
            .active_job
            .as_ref()
            .and_then(|j| j.get("status"))
            .and_then(|v| v.as_str())
            .map(flashes_taskbar)
            .unwrap_or(false);
        if should_flash {
            (self.tx)(UiAction::FlashTaskbar);
        }
        self.do_refresh_history();
        self.do_refresh_perf();
        let cid = self.st.lock().conversation_id.clone();
        if valid_id(&cid) {
            if let Ok(mut conv) = ApiClient::new(&url).and_then(|c| c.get(&format!("/conversations/{}", cid), 15000)) {
                let perf_snap = self.st.lock().perf.clone();
                let cached_tps = perf_snap
                    .get("tokensPerSec")
                    .or_else(|| perf_snap.get("lastTokensPerSec"))
                    .and_then(|v| v.as_f64())
                    .filter(|v| v.is_finite() && *v > 0.0);
                if let (Some(tps), Some(jobs)) = (cached_tps, conv.get_mut("jobs").and_then(|v| v.as_array_mut())) {
                    for row in jobs.iter_mut() {
                        if row.get("id").and_then(|v| v.as_str()) == Some(jid.as_str()) {
                            let has_tps = row
                                .get("perf")
                                .and_then(|p| p.get("tokensPerSec"))
                                .and_then(|v| v.as_f64())
                                .is_some_and(|v| v.is_finite() && v > 0.0);
                            if !has_tps {
                                row["_perf"] = json!({ "tokensPerSec": tps });
                            }
                        }
                    }
                }
                self.apply_loaded_conversation(conv, false);
            }
        }
    }

    fn start_standby(self: &Arc<Self>) {
        let (connected, busy, sel_id, sel_title, allow_local, local_wd, read_roots, url) = {
            let g = self.st.lock();
            (
                g.connected,
                g.busy,
                g.selected_window_id,
                g.selected_window_title.clone(),
                g.gui.get("localCliEnabled").and_then(|v| v.as_bool()).unwrap_or(true),
                g.local_workdir.clone(),
                g.read_roots.clone(),
                g.gui.get("api").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            )
        };
        if !connected || busy || sel_id == 0 {
            self.toast("warn", "連携待機の前にウィンドウを選択して接続してください");
            self.push_connection();
            return;
        }
        let ws = if allow_local {
            resolve_cli_workspace(&self.root, &local_wd, "shared").unwrap_or_default()
        } else {
            PathBuf::new()
        };
        let app = Arc::clone(self);
        let res = self.standby.lock().start(
            &url,
            sel_id,
            &sel_title,
            ws,
            read_roots,
            allow_local,
            move |notice| match notice {
                StandbyNotice::Log(l) => app.push_log(&l, "info"),
                StandbyNotice::State(s) => app.emit_event(json!({ "type": "job_update", "liveText": s })),
                StandbyNotice::Jobs(convs) => {
                    let list = convs.as_array().cloned().unwrap_or_default();
                    app.st.lock().conversations = list.clone();
                    app.emit_event(json!({ "type": "jobs", "conversations": list }));
                }
                StandbyNotice::Conversation(c) => app.apply_loaded_conversation(c, false),
                StandbyNotice::StandbyEnded { error } => {
                    if !error.is_empty() {
                        app.emit_error_notice(&error);
                    }
                    app.push_connection();
                }
            },
        );
        if let Err(e) = res {
            self.emit_error_notice(&e);
        } else {
            self.st.lock().gui["pcStandby"] = json!(true);
            self.persist_gui();
        }
        self.push_connection();
    }

    fn stop_standby(&self) {
        self.standby.lock().stop();
        self.st.lock().gui["pcStandby"] = json!(false);
        self.persist_gui();
        self.push_connection();
    }

    fn do_load_files(&self) {
        let cid = self.st.lock().conversation_id.clone();
        if !valid_id(&cid) {
            return;
        }
        self.emit_event(json!({ "type": "files", "loading": true, "files": [] }));
        let url = self.current_api_url();
        match ApiClient::new(&url).and_then(|c| c.get(&format!("/conversations/{}/files", cid), 15000)) {
            Ok(v) => {
                let items = v.get("files").and_then(|f| f.as_array()).cloned().unwrap_or_default();
                self.emit_event(json!({ "type": "files", "loading": false, "files": items, "conversationId": cid }));
            }
            Err(e) => {
                self.emit_event(json!({ "type": "files", "loading": false, "files": [] }));
                self.emit_error_notice(&e);
            }
        }
    }

    fn fetch_conversation_attachment_thumbs(self: &Arc<Self>, cid: String, paths: Vec<String>) {
        if !valid_id(&cid) {
            return;
        }
        let url = self.current_api_url();
        let Ok(client) = ApiClient::new(&url) else {
            return;
        };
        for rel in paths {
            if self.closing.load(Ordering::SeqCst) || self.st.lock().conversation_id != cid {
                break;
            }
            let enc = utf8_percent_encode(&rel, NON_ALPHANUMERIC).to_string();
            let Ok(res) = client.get(&format!("/conversations/{}/files?path={}", cid, enc), 15000) else {
                break;
            };
            let b64 = res.get("base64").and_then(|v| v.as_str()).unwrap_or("");
            let Ok(bytes) = B64.decode(b64) else {
                continue;
            };
            let preview = make_thumbnail_data_url(&bytes, 220).unwrap_or_default();
            let val = json!({
                "path": rel,
                "displayName": rel.strip_prefix("uploads/").unwrap_or(&rel),
                "base64": b64,
                "size": bytes.len(),
                "kind": "image",
                "previewUrl": preview,
            });
            self.cache_sent_attachments(&cid, &[val]);
        }
    }

    fn do_preview_sent_attachment(&self, cid: &str, rel: &str, display_name: &str) {
        if rel.is_empty() {
            return;
        }
        let resolved_cid = if valid_id(cid) {
            cid.to_string()
        } else {
            self.st.lock().conversation_id.clone()
        };
        let cached = {
            let g = self.st.lock();
            (!resolved_cid.is_empty())
                .then(|| g.sent_attachments_cache.get(&format!("{}:{}", resolved_cid, rel)))
                .flatten()
                .or_else(|| g.sent_attachments_cache.get(rel))
                .cloned()
        };
        if let Some(mut item) = cached {
            if !display_name.is_empty()
                && item
                    .get("displayName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .is_empty()
            {
                item["displayName"] = json!(display_name);
            }
            self.preview_attachment_item(&item);
            return;
        }
        self.do_preview_remote_file_in(&resolved_cid, rel, display_name);
    }

    fn do_preview_remote_file(&self, rel: &str) {
        let cid = self.st.lock().conversation_id.clone();
        self.do_preview_remote_file_in(&cid, rel, rel);
    }

    fn do_preview_remote_file_in(&self, cid: &str, rel: &str, display_name: &str) {
        if !valid_id(cid) || rel.is_empty() {
            return;
        }
        let title = if !display_name.is_empty() {
            display_name
        } else {
            rel.strip_prefix("uploads/").unwrap_or(rel)
        };
        self.emit_event(json!({ "type": "loading", "scope": "preview", "active": true }));
        let url = self.current_api_url();
        let enc = utf8_percent_encode(rel, NON_ALPHANUMERIC).to_string();
        let res = ApiClient::new(&url).and_then(|c| c.get(&format!("/conversations/{}/files?path={}", cid, enc), 30000));
        self.emit_event(json!({ "type": "loading", "scope": "preview", "active": false }));
        match res {
            Ok(res) => {
                let b64 = res.get("base64").and_then(|v| v.as_str()).unwrap_or("");
                let bytes = B64.decode(b64).unwrap_or_default();
                if let Some(mime) = Self::image_mime_for_path(rel) {
                    let preview = make_thumbnail_data_url(&bytes, 220).unwrap_or_default();
                    let val = json!({
                        "path": rel,
                        "displayName": title,
                        "base64": b64,
                        "size": bytes.len(),
                        "kind": "image",
                        "previewUrl": preview,
                    });
                    self.cache_sent_attachments(cid, &[val]);
                    let data_url = if mime == "image/bmp" {
                        make_thumbnail_data_url(&bytes, 1920)
                            .unwrap_or_else(|| format!("data:{};base64,{}", mime, b64))
                    } else {
                        format!("data:{};base64,{}", mime, b64)
                    };
                    self.emit_event(json!({
                        "type": "attachment_preview",
                        "title": title,
                        "kind": "image",
                        "dataUrl": data_url,
                    }));
                } else {
                    let val = json!({
                        "path": rel,
                        "displayName": title,
                        "base64": b64,
                        "size": bytes.len(),
                        "kind": "file",
                    });
                    self.cache_sent_attachment(cid, &val);
                    self.emit_event(json!({
                        "type": "attachment_preview",
                        "title": title,
                        "kind": "text",
                        "text": String::from_utf8_lossy(&bytes),
                    }));
                }
            }
            Err(e) => {
                if e.contains("busy") || e.contains("409") {
                    self.toast("warn", "処理完了後に再度開いてください");
                } else {
                    self.emit_error_notice(&e);
                }
            }
        }
    }

    fn do_save_downloads(&self, target_job_id: Option<String>) {
        let (job_opt, cid, url) = {
            let g = self.st.lock();
            let job = match target_job_id {
                Some(ref jid) => g
                    .conversation
                    .as_ref()
                    .and_then(|c| c.get("jobs"))
                    .and_then(|a| a.as_array())
                    .and_then(|arr| arr.iter().find(|j| j.get("id").and_then(|v| v.as_str()) == Some(jid)))
                    .cloned()
                    .or_else(|| g.active_job.clone()),
                None => g.active_job.clone(),
            };
            (job, g.conversation_id.clone(), self.current_api_url())
        };
        let Some(job) = job_opt else {
            self.toast("warn", "保存できる完了ジョブがありません");
            return;
        };
        let downloads = self.root.join("downloads");
        match save_job_outputs(&job, &downloads) {
            Ok(folder) => {
                if valid_id(&cid) {
                    if let Ok(c) = ApiClient::new(&url) {
                        if let Ok(list) = c.get(&format!("/conversations/{}/files", cid), 15000) {
                            for f in list.get("files").and_then(|v| v.as_array()).into_iter().flatten() {
                                let name = f.get("path").and_then(|v| v.as_str()).unwrap_or("");
                                let sz = f.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0);
                                if !safe_artifact(name) || sz > 2_097_152 {
                                    continue;
                                }
                                let enc = utf8_percent_encode(name, NON_ALPHANUMERIC).to_string();
                                if let Ok(data) = c.get(&format!("/conversations/{}/files?path={}", cid, enc), 30000) {
                                    if let Some(b64) = data.get("base64").and_then(|v| v.as_str()) {
                                        if let Ok(bytes) = B64.decode(b64) {
                                            let dest = folder.join("workspace").join(name);
                                            let _ = write_file_atomic(&dest, &bytes);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                let _ = open_external_target(&folder.to_string_lossy());
                self.toast("ok", &format!("保存しました: {}", folder.display()));
                self.push_log(&format!("成果物を保存しました: {}\r\n", folder.display()), "info");
            }
            Err(e) => self.emit_error_notice(&e),
        }
    }

    fn do_share_load(&self) {
        let cid = self.st.lock().conversation_id.clone();
        if !valid_id(&cid) {
            self.emit_event(json!({ "type": "sharing", "entries": [] }));
            return;
        }
        let url = self.current_api_url();
        if let Ok(conv) = ApiClient::new(&url).and_then(|c| c.get(&format!("/conversations/{}", cid), 15000)) {
            let list = conv.get("allowedIps").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            self.emit_event(json!({ "type": "sharing", "conversationId": cid, "entries": list }));
        }
    }

    fn do_share_update(&self, ip: String, mode: String, remove: bool) {
        let cid = self.st.lock().conversation_id.clone();
        if !valid_id(&cid) || ip.is_empty() {
            return;
        }
        let url = self.current_api_url();
        let body = json!({ "ip": ip, "mode": mode, "remove": remove });
        match ApiClient::new(&url).and_then(|c| c.post(&format!("/conversations/{}/access", cid), &body, 15000)) {
            Ok(res) => {
                let list = res.get("allowedIps").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                self.emit_event(json!({ "type": "sharing", "conversationId": cid, "entries": list }));
                self.toast("ok", "共有設定を更新しました");
            }
            Err(e) => self.emit_error_notice(&e),
        }
    }

    fn do_share_remove(&self, idx: usize) {
        let cid = self.st.lock().conversation_id.clone();
        if !valid_id(&cid) {
            return;
        }
        let url = self.current_api_url();
        if let Ok(conv) = ApiClient::new(&url).and_then(|c| c.get(&format!("/conversations/{}", cid), 15000)) {
            let list = conv.get("allowedIps").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            if let Some(item) = list.get(idx) {
                let ip = item.get("ip").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let mode = item.get("mode").and_then(|v| v.as_str()).unwrap_or("content").to_string();
                if !ip.is_empty() {
                    self.do_share_update(ip, mode, true);
                }
            }
        }
    }
}
