use crate::app_state::{AppController, UiAction};
use crate::ui::anim::UiAnimations;
use crate::ui::chat_view::{filtered_slash_items, render_main_workspace};
use crate::ui::drawers::{render_right_drawer, render_sidebar};
use crate::ui::markdown_view::{self, MdBlock};
use crate::ui::overlays::{palette_items, render_overlays};
use crate::ui::selection_scope::SelectionScope;
use crate::ui::theme::{self, UI_FONT};
use crate::win_util::{
    apply_dwm_appearance, capture_window_placement, clipboard_read_payload, clipboard_set_text,
    flash_taskbar, set_window_cloaked, show_window_initial, ClipboardPayload,
};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use iced::keyboard;
use iced::widget::{container, image as img_w, operation, row, stack, text_editor, Id};
use iced::{clipboard, event, mouse, window, Element, Length, Point, Size, Subscription, Task, Theme};
use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
#[cfg(windows)]
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
#[cfg(windows)]
use windows::Win32::Foundation::{LPARAM, WPARAM};
#[cfg(windows)]
use windows::Win32::System::Threading::GetCurrentThreadId;
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_CONTROL, MOD_SHIFT, VK_F12,
};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, PostThreadMessageW, MSG, WM_HOTKEY, WM_QUIT};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawerPane {
    Sharing,
    Tools,
    Files,
    Logs,
    Workers,
    Settings,
}

impl DrawerPane {
    pub fn title(self) -> &'static str {
        match self {
            Self::Workers => "8B 担当パネル",
            Self::Tools => "作業環境・PC操作",
            Self::Settings => "設定・チューニング",
            Self::Files => "ワークスペースファイル",
            Self::Sharing => "共有アクセス管理",
            Self::Logs => "診断ログ",
        }
    }

    pub fn is_wide(self) -> bool {
        matches!(self, Self::Workers | Self::Settings)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryFilter {
    All,
    Active,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkModeOpt {
    Standard,
    Chat,
    Coding,
    PcControl,
}

impl WorkModeOpt {
    pub const ALL: [Self; 4] = [Self::Standard, Self::Chat, Self::Coding, Self::PcControl];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Chat => "chat",
            Self::Coding => "coding",
            Self::PcControl => "pc_control",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "chat" => Self::Chat,
            "coding" => Self::Coding,
            "pc_control" => Self::PcControl,
            _ => Self::Standard,
        }
    }
}

impl fmt::Display for WorkModeOpt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Standard => "標準モード",
            Self::Chat => "チャット専用",
            Self::Coding => "コーディング用",
            Self::PcControl => "ゲーム・PC操作",
        })
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelRoleOpt {
    Planner,
    Worker,
}

#[allow(dead_code)]
impl ModelRoleOpt {
    pub const ALL: [Self; 2] = [Self::Planner, Self::Worker];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planner => "planner",
            Self::Worker => "worker",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "worker" => Self::Worker,
            _ => Self::Planner,
        }
    }
}

impl fmt::Display for ModelRoleOpt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Planner => "27B",
            Self::Worker => "8B + LoRA",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffortOpt {
    None,
    Low,
    Medium,
    XHigh,
}

impl EffortOpt {
    pub const ALL: [Self; 4] = [Self::None, Self::Low, Self::Medium, Self::XHigh];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::XHigh => "xhigh",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "low" => Self::Low,
            "medium" => Self::Medium,
            "xhigh" => Self::XHigh,
            _ => Self::None,
        }
    }
}

impl fmt::Display for EffortOpt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::None => "none (オフ)",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::XHigh => "xhigh",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuneModeOpt {
    Speed,
    Balanced,
    Quality,
}

impl TuneModeOpt {
    pub const ALL: [Self; 3] = [Self::Speed, Self::Balanced, Self::Quality];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Speed => "speed",
            Self::Balanced => "balanced",
            Self::Quality => "quality",
        }
    }
}

impl fmt::Display for TuneModeOpt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Speed => "速度優先 (speed)",
            Self::Balanced => "バランス (balanced)",
            Self::Quality => "品質優先 (quality)",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareModeOpt {
    Content,
    Member,
}

impl ShareModeOpt {
    pub const ALL: [Self; 2] = [Self::Content, Self::Member];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::Member => "member",
        }
    }
}

impl fmt::Display for ShareModeOpt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Content => "閲覧+送信",
            Self::Member => "閲覧のみ",
        })
    }
}

#[derive(Debug, Clone)]
pub enum ModalKind {
    Palette,
    Shortcuts,
    Lightbox {
        title: String,
        img: Option<img_w::Handle>,
        text: String,
    },
}

#[derive(Debug, Clone)]
pub struct PaletteAction {
    pub title: &'static str,
    pub hint: &'static str,
    pub kind: &'static str,
    pub disabled: bool,
}

#[derive(Clone, Debug)]
pub struct SettingsDraft {
    pub api: String,
    pub auto_connect: bool,
    pub ctx: String,
    pub tok: String,
    pub effort: EffortOpt,
    pub budget: String,
    pub temperature: f64,
    pub top_p: f64,
    pub top_k: i32,
    pub min_p: f64,
    pub repeat_penalty: f64,
    pub presence_penalty: f64,
    pub frequency_penalty: f64,
    pub sampling_preset: String,
    pub tune_mode: TuneModeOpt,
    pub share_ip: String,
    pub share_mode: ShareModeOpt,
}

impl Default for SettingsDraft {
    fn default() -> Self {
        Self {
            api: "https://localhost:8443".into(),
            auto_connect: true,
            ctx: "16384".into(),
            tok: "1800".into(),
            effort: EffortOpt::None,
            budget: "-1".into(),
            temperature: 0.05,
            top_p: 0.95,
            top_k: 40,
            min_p: 0.05,
            repeat_penalty: 1.10,
            presence_penalty: 0.0,
            frequency_penalty: 0.0,
            sampling_preset: "code".into(),
            tune_mode: TuneModeOpt::Speed,
            share_ip: String::new(),
            share_mode: ShareModeOpt::Content,
        }
    }
}

impl SettingsDraft {
    fn sync_from(&mut self, gui: &Value, srv: &Value) {
        let pick = |k: &str| srv.get(k).or_else(|| gui.get(k));
        if let Some(s) = gui.get("api").and_then(|v| v.as_str()) {
            self.api = s.to_string();
        }
        self.auto_connect = gui.get("autoConnect").and_then(|v| v.as_bool()) != Some(false);
        if let Some(c) = pick("contextWindow").and_then(|v| v.as_i64()) {
            self.ctx = c.to_string();
        }
        if let Some(t) = pick("maxTokens").and_then(|v| v.as_i64()) {
            self.tok = t.to_string();
        }
        if let Some(e) = pick("reasoningEffort").and_then(|v| v.as_str()) {
            self.effort = EffortOpt::from_str(e);
        }
        if let Some(b) = pick("reasoningBudgetTokens").and_then(|v| v.as_i64()) {
            self.budget = b.to_string();
        }
        if let Some(v) = pick("temperature").and_then(|v| v.as_f64()) {
            self.temperature = v;
        }
        if let Some(v) = pick("top_p").and_then(|v| v.as_f64()) {
            self.top_p = v;
        }
        if let Some(v) = pick("top_k").and_then(|v| v.as_i64()) {
            self.top_k = v as i32;
        }
        if let Some(v) = pick("min_p").and_then(|v| v.as_f64()) {
            self.min_p = v;
        }
        if let Some(v) = pick("repeat_penalty").and_then(|v| v.as_f64()) {
            self.repeat_penalty = v;
        }
        if let Some(v) = pick("presence_penalty").and_then(|v| v.as_f64()) {
            self.presence_penalty = v;
        }
        if let Some(v) = pick("frequency_penalty").and_then(|v| v.as_f64()) {
            self.frequency_penalty = v;
        }
        if let Some(s) = gui.get("samplingPreset").and_then(|v| v.as_str()) {
            self.sampling_preset = s.to_string();
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    BackendAction(UiAction),
    WinEvent(window::Id, window::Event),
    GotHwnd(isize),
    RevealWindow,
    AnimFrame(Instant),
    KeyEvent(keyboard::Event),
    ClipboardTextPaste(Option<String>),
    ClearCopyFeedback(String),
    ToggleTheme,
    OpenModal(ModalKind),
    CloseModal,
    NewChat,
    RefreshHistory,
    HistorySearchChanged(String),
    SetHistoryFilter(HistoryFilter),
    OpenConversation(String),
    CopyText(String, Option<String>),
    SelectionCopied(String),
    SelectionMouseUp,
    SelectionClear,
    CopyActiveSelection,
    ConnectNow,
    ToggleSidebar,
    TogglePane(DrawerPane),
    CloseDrawer,
    SetTuningOpen(bool),
    ToggleFind,
    FindQueryChanged(String),
    StepFind(i32),
    ToggleAccordion(String, bool),
    ToggleTaskCollapse,
    EditorAction(text_editor::Action),
    InsertStarter(String),
    SendPrompt,
    ComposerCtrlV,
    GlobalPasteCheck,
    SlashMove(i32),
    SlashAccept,
    SlashPick(usize),
    CancelJob,
    AnswerQuestion(String, String),
    ContinueChat,
    CloseNotice,
    LoadFullJob(String),
    SaveDownloads(Option<String>),
    OpenExternal(String),
    PickAttachments,
    ClearAttachments,
    RemoveAttachment(String),
    PreviewAttachment(usize),
    PreviewSentAttachment {
        conv_id: String,
        path: String,
        display_name: String,
    },
    OpenLightboxImage(String, img_w::Handle),
    #[allow(dead_code)]
    SelectModelRole(ModelRoleOpt),
    SelectWorkMode(WorkModeOpt),
    #[allow(dead_code)]
    TogglePlanMode,
    OpenTuningQuick,
    ChatScrolled {
        abs_y: f32,
        dist_from_bottom: f32,
        content_h: f32,
        viewport_h: f32,
    },
    ScrollToBottom,
    SetWorkspaceEnabled(bool),
    SetLocalCli(bool),
    SetRunOnPc(bool),
    SetPcEnabled(bool),
    SetPcStandby(bool),
    PickWorkdir,
    OpenLocalFolder,
    ResetWorkdir,
    AddReadFolder,
    ClearReadFolders,
    RemoveReadFolder(usize),
    StopPcAll,
    WindowFilterChanged(String),
    RefreshPcWindows,
    SelectPcWindow(isize),
    DraftApi(String),
    DraftAutoConnect(bool),
    SaveAndConnect,
    StartServer,
    RestartServer,
    StopServer,
    DraftCtx(String),
    DraftTok(String),
    DraftEffort(EffortOpt),
    DraftBudget(String),
    ApplyServerSettings,
    SelectPreset(String),
    ResetSampling,
    SliderTemp(f64),
    SliderTopP(f64),
    SliderTopK(i32),
    SliderMinP(f64),
    SliderRepeat(f64),
    SliderPresence(f64),
    SliderFrequency(f64),
    ApplySampling,
    RefreshPerf,
    SelectTuneMode(TuneModeOpt),
    StartAutoTune,
    FilesRefresh,
    FilesPreview(String),
    DraftShareIp(String),
    DraftShareMode(ShareModeOpt),
    ShareAdd,
    ShareRemove(usize),
    LogFilterChanged(String),
    CopyVisibleLogs,
    PaletteQueryChanged(String),
    PaletteRun(usize),
    Noop,
}

#[derive(Clone)]
struct RxBridge(u64, Arc<Mutex<Option<mpsc::UnboundedReceiver<UiAction>>>>);

impl Hash for RxBridge {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

fn backend_stream(src: &RxBridge) -> impl iced::futures::Stream<Item = Message> {
    let slot = Arc::clone(&src.1);
    iced::stream::channel(64, async move |mut output| {
        use iced::futures::sink::SinkExt;
        let maybe_rx = slot.lock().take();
        if let Some(mut rx) = maybe_rx {
            while let Some(act) = rx.recv().await {
                let _ = output.send(Message::BackendAction(act)).await;
            }
        }
        std::future::pending::<()>().await;
    })
}

pub struct UiState {
    pub controller: Arc<AppController>,
    rx_slot: Arc<Mutex<Option<mpsc::UnboundedReceiver<UiAction>>>>,
    pub window_id: Option<window::Id>,
    pub hwnd: isize,
    #[cfg(windows)]
    hotkey_tid: Arc<AtomicU32>,
    pub connected: bool,
    pub connecting: bool,
    pub busy: bool,
    pub standby: bool,
    pub elevated: bool,
    pub computer_name: String,
    pub api_url: String,
    pub loading_history: bool,
    pub loading_conversation: bool,
    pub loading_preview: bool,
    pub gui: Value,
    pub server: Value,
    pub capabilities: Value,
    pub perf: Value,
    pub autotune: Value,
    pub conversations: Vec<Value>,
    pub conversation: Option<Value>,
    pub read_only: bool,
    pub attachments: Vec<Value>,
    pub attachment_thumbs: HashMap<String, img_w::Handle>,
    pub upload_stages: Map<String, Value>,
    pub pc_windows: Vec<Value>,
    pub selected_window_id: isize,
    pub selected_window_title: String,
    pub local_workdir: String,
    pub read_roots: Vec<String>,
    pub workers_panels: Vec<Value>,
    pub files_items: Vec<Value>,
    pub files_loading: bool,
    pub sharing_entries: Vec<Value>,
    pub logs: Vec<(String, String)>,
    pub notice: Option<Value>,
    pub status_main: String,
    pub status_tail: String,
    pub status_stale: bool,
    pub sidebar_open: bool,
    pub active_pane: Option<DrawerPane>,
    pub displayed_pane: Option<DrawerPane>,
    pub tuning_open: bool,
    pub history_filter: HistoryFilter,
    pub history_query: String,
    pub window_filter: String,
    pub log_filter: String,
    pub find_open: bool,
    pub find_query: String,
    pub find_match_idx: usize,
    pub task_strip_collapsed: bool,
    pub active_modal: Option<ModalKind>,
    pub displayed_modal: Option<ModalKind>,
    pub palette_query: String,
    pub palette_idx: usize,
    pub slash_idx: usize,
    pub settings_draft: SettingsDraft,
    pub composer_content: text_editor::Content,
    pub copied_key: Option<String>,
    pub scroll_id: Id,
    pub at_bottom: bool,
    pub last_scroll_abs_y: f32,
    pub last_scroll_content_h: f32,
    pub last_scroll_viewport_h: f32,
    pub pending_scroll_bottom: bool,
    pub hover_drop_count: usize,
    pub parsed_answers: HashMap<String, (String, Vec<MdBlock>)>,
    pub decoded_activity_imgs: HashMap<String, img_w::Handle>,
    pub sent_attach_thumbs: HashMap<String, img_w::Handle>,
    pub selection_scope: SelectionScope,
    pub anim: UiAnimations,
    pub pending_reveal_frames: u8,
}

impl UiState {
    pub fn is_dark(&self) -> bool {
        self.gui.get("darkTheme").and_then(|v| v.as_bool()).unwrap_or(true)
    }

    pub fn is_any_loading_or_active(&self) -> bool {
        self.connecting
            || self.loading_history
            || self.loading_conversation
            || self.loading_preview
            || self.files_loading
            || self.busy
    }

    pub fn find_match_jobs(&self) -> Vec<String> {
        let q = self.find_query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let Some(jobs) = self
            .conversation
            .as_ref()
            .and_then(|c| c.get("jobs"))
            .and_then(|v| v.as_array())
        else {
            return Vec::new();
        };
        let mut ids = Vec::new();
        for j in jobs {
            let jid = j.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let goal = j.get("goal").and_then(|v| v.as_str()).unwrap_or("");
            let ans = j
                .get("result")
                .and_then(|r| r.get("review"))
                .and_then(|rv| rv.get("final_answer"))
                .and_then(|v| v.as_str())
                .or_else(|| j.get("partialAnswer").and_then(|v| v.as_str()))
                .unwrap_or("");
            if goal.to_ascii_lowercase().contains(&q) || ans.to_ascii_lowercase().contains(&q) {
                ids.push(jid.to_string());
            }
        }
        ids
    }

    fn send_ipc(&self, cmd: Value) {
        self.controller.handle_ipc(cmd);
    }

    fn send_ipc_bg(&self, cmd: Value) {
        let ctrl = Arc::clone(&self.controller);
        std::thread::spawn(move || ctrl.handle_ipc(cmd));
    }

    fn refresh_loading_bar_target(&mut self) {
        let target = if self.is_any_loading_or_active() { 1.0 } else { 0.0 };
        self.anim.loading_bar.set_target(target, 180.0);
    }

    fn cache_job_assets(&mut self, job: &Value) {
        let jid = job.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let ans = job
            .get("result")
            .and_then(|r| r.get("review"))
            .and_then(|rv| rv.get("final_answer"))
            .and_then(|v| v.as_str())
            .or_else(|| job.get("partialAnswer").and_then(|v| v.as_str()))
            .unwrap_or("");
        if !jid.is_empty() && !ans.is_empty() {
            let needs = self
                .parsed_answers
                .get(&jid)
                .map(|(old, _)| old != ans)
                .unwrap_or(true);
            if needs {
                let parsed = markdown_view::parse_markdown(ans);
                self.parsed_answers.insert(jid.clone(), (ans.to_string(), parsed));
            }
        }
        if let Some(act) = job.get("activity").and_then(|v| v.as_array()) {
            self.cache_activity_images(&jid, act, 0);
        }
        let cid = job.get("conversationId").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(arr) = job
            .get("attachments")
            .or_else(|| job.get("files"))
            .and_then(|v| v.as_array())
        {
            for a in arr {
                let p = a.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let du = a.get("previewUrl").and_then(|v| v.as_str()).unwrap_or("");
                if !p.is_empty() && !du.is_empty() {
                    if let Some((_, b64)) = du.split_once("base64,") {
                        if let Ok(bytes) = B64.decode(b64) {
                            let h = img_w::Handle::from_bytes(bytes);
                            self.sent_attach_thumbs.insert(p.to_string(), h.clone());
                            if !cid.is_empty() {
                                self.sent_attach_thumbs.insert(format!("{}:{}", cid, p), h);
                            }
                        }
                    }
                }
            }
        }
    }

    fn cache_activity_images(&mut self, jid: &str, rows: &[Value], depth: usize) {
        for (idx, r) in rows.iter().enumerate() {
            let key = format!("actimg:{}:{}:{}", jid, depth, idx);
            if !self.decoded_activity_imgs.contains_key(&key) {
                if let Some(b64) = r
                    .get("imageBase64")
                    .or_else(|| r.get("base64"))
                    .and_then(|v| v.as_str())
                {
                    if let Ok(bytes) = B64.decode(b64) {
                        self.decoded_activity_imgs.insert(key, img_w::Handle::from_bytes(bytes));
                    }
                }
            }
            if depth < 2 {
                if let Some(sub) = r.get("workerActivity").and_then(|v| v.as_array()) {
                    self.cache_activity_images(jid, sub, depth + 1);
                }
            }
        }
    }

    fn cache_conversation_assets(&mut self, conv: &Value) {
        let jobs = conv
            .get("jobs")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for j in &jobs {
            self.cache_job_assets(j);
        }
    }

    fn sync_question_and_task_anims(&mut self) {
        let latest = self
            .conversation
            .as_ref()
            .and_then(|c| c.get("jobs"))
            .and_then(|a| a.as_array())
            .and_then(|arr| arr.last());
        let has_q = latest
            .is_some_and(|j| j.get("status").and_then(|v| v.as_str()) == Some("waiting_user") && j.get("question").is_some());
        let has_tasks = latest.is_some_and(|j| {
            j.get("result")
                .and_then(|r| r.get("batches"))
                .or_else(|| j.get("batches"))
                .and_then(|b| b.as_array())
                .is_some_and(|bs| {
                    bs.iter()
                        .any(|b| b.get("tasks").and_then(|t| t.as_array()).is_some_and(|ts| !ts.is_empty()))
                })
        });
        self.anim.question.set_target(if has_q { 1.0 } else { 0.0 }, 200.0);
        self.anim.task_strip.set_target(if has_tasks { 1.0 } else { 0.0 }, 200.0);
    }

    fn apply_backend_event(&mut self, ev: Value) -> Task<Message> {
        let kind = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let mut out_task = Task::none();
        match kind {
            "bootstrap" => {
                if let Some(g) = ev.get("guiSettings") {
                    self.gui = g.clone();
                }
                self.computer_name = ev.get("computerName").and_then(|v| v.as_str()).unwrap_or("").into();
                self.elevated = ev.get("elevated").and_then(|v| v.as_bool()).unwrap_or(false);
                if let Some(arr) = ev.get("logs").and_then(|v| v.as_array()) {
                    self.logs = arr
                        .iter()
                        .map(|r| {
                            (
                                r.get("line").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                r.get("level").and_then(|v| v.as_str()).unwrap_or("info").to_string(),
                            )
                        })
                        .collect();
                }
                if let Some(arr) = ev.get("attached").and_then(|v| v.as_array()) {
                    self.attachments = arr.clone();
                    self.rebuild_attachment_thumbs();
                }
                if let Some(m) = ev.get("uploadStages").and_then(|v| v.as_object()) {
                    self.upload_stages = m.clone();
                }
                if let Some(arr) = ev.get("pcWindows").and_then(|v| v.as_array()) {
                    self.pc_windows = arr.clone();
                }
                self.selected_window_id = ev.get("selectedWindowId").and_then(|v| v.as_i64()).unwrap_or(0) as isize;
                self.selected_window_title = ev.get("selectedWindowTitle").and_then(|v| v.as_str()).unwrap_or("").into();
                self.local_workdir = ev.get("localWorkdir").and_then(|v| v.as_str()).unwrap_or("").into();
                if let Some(arr) = ev.get("readRoots").and_then(|v| v.as_array()) {
                    self.read_roots = arr.iter().filter_map(|v| v.as_str().map(String::from)).collect();
                }
                self.settings_draft.sync_from(&self.gui, &self.server);
            }
            "connection" => {
                self.connected = ev.get("connected").and_then(|v| v.as_bool()).unwrap_or(false);
                self.busy = ev.get("busy").and_then(|v| v.as_bool()).unwrap_or(false);
                self.connecting = ev.get("connecting").and_then(|v| v.as_bool()).unwrap_or(false);
                self.standby = ev.get("standby").and_then(|v| v.as_bool()).unwrap_or(false);
                if let Some(u) = ev.get("url").and_then(|v| v.as_str()) {
                    self.api_url = u.to_string();
                }
            }
            "loading" => {
                let scope = ev.get("scope").and_then(|v| v.as_str()).unwrap_or("");
                let act = ev.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
                match scope {
                    "history" => self.loading_history = act,
                    "conversation" => self.loading_conversation = act,
                    "preview" => self.loading_preview = act,
                    _ => {}
                }
            }
            "settings_sync" => {
                if let Some(g) = ev.get("gui") {
                    self.gui = g.clone();
                }
                if let Some(s) = ev.get("server") {
                    self.server = s.clone();
                }
                if let Some(c) = ev.get("capabilities") {
                    self.capabilities = c.clone();
                }
                self.settings_draft.sync_from(&self.gui, &self.server);
            }
            "perf" => {
                if let Some(d) = ev.get("data") {
                    self.perf = d.clone();
                }
            }
            "jobs" => {
                self.conversations = ev
                    .get("conversations")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
            }
            "conversation" => {
                let prev_id = self
                    .conversation
                    .as_ref()
                    .and_then(|c| c.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let prev_job_ids: Vec<String> = self
                    .conversation
                    .as_ref()
                    .and_then(|c| c.get("jobs"))
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|j| j.get("id").and_then(|v| v.as_str()).map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                self.conversation = ev.get("conversation").filter(|v| !v.is_null()).cloned();
                let new_id = self
                    .conversation
                    .as_ref()
                    .and_then(|c| c.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let same_conv = prev_id == new_id
                    || (prev_id.is_empty()
                        && !new_id.is_empty()
                        && self
                            .conversation
                            .as_ref()
                            .and_then(|c| c.get("jobs"))
                            .and_then(|v| v.as_array())
                            .is_some_and(|arr| {
                                arr.iter().any(|j| {
                                    j.get("id")
                                        .and_then(|v| v.as_str())
                                        .is_some_and(|id| prev_job_ids.iter().any(|p| p == id))
                                })
                            }));
                if !same_conv {
                    self.at_bottom = true;
                    self.last_scroll_abs_y = 0.0;
                    self.last_scroll_content_h = 0.0;
                    self.last_scroll_viewport_h = 0.0;
                }
                self.read_only = ev.get("readOnly").and_then(|v| v.as_bool()).unwrap_or(false);
                if let Some(wd) = ev.get("localWorkdir").and_then(|v| v.as_str()) {
                    self.local_workdir = wd.to_string();
                }
                self.parsed_answers.clear();
                self.decoded_activity_imgs.clear();
                if let Some(conv) = self.conversation.clone() {
                    self.cache_conversation_assets(&conv);
                }
                self.sync_question_and_task_anims();
                if self.active_pane == Some(DrawerPane::Files) {
                    self.send_ipc_bg(json!({ "cmd": "files_load" }));
                } else if self.active_pane == Some(DrawerPane::Sharing) {
                    self.send_ipc_bg(json!({ "cmd": "share_load" }));
                }
                if self.at_bottom {
                    self.pending_scroll_bottom = true;
                    out_task = operation::snap_to_end(self.scroll_id.clone());
                }
            }
            "job_update" => {
                if let Some(lt) = ev.get("liveText").and_then(|v| v.as_str()) {
                    self.status_main = lt.to_string();
                }
                if let Some(lt) = ev.get("liveTail").and_then(|v| v.as_str()) {
                    self.status_tail = lt.to_string();
                    self.status_stale = ev.get("isStale").and_then(|v| v.as_bool()).unwrap_or(false);
                }
                if let Some(job_raw) = ev.get("job") {
                    let mut j = job_raw.clone();
                    if let Some(rc) = ev.get("resultCard") {
                        j["_resultCard"] = rc.clone();
                    }
                    self.cache_job_assets(&j);
                    let jid = j.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let j_cid = j.get("conversationId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let j_title = j
                        .get("title")
                        .or_else(|| j.get("goal"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("新しいチャット")
                        .to_string();
                    let replace_id = ev.get("replaceJobId").and_then(|v| v.as_str());
                    let is_optimistic = j.get("_optimistic").and_then(|v| v.as_bool()).unwrap_or(false);
                    if self.conversation.is_none() {
                        self.conversation = Some(json!({
                            "id": j_cid,
                            "title": j_title,
                            "jobs": [],
                        }));
                    }
                    let mut changed = false;
                    let mut just_finished = false;
                    if let Some(ref mut conv) = self.conversation {
                        if conv.get("id").and_then(|v| v.as_str()).unwrap_or("").is_empty() && !j_cid.is_empty() {
                            conv["id"] = json!(j_cid);
                        }
                        if let Some(t) = j.get("title").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
                            let cur_t = conv.get("title").and_then(|v| v.as_str()).unwrap_or("");
                            if cur_t.is_empty() || cur_t == "新しいチャット" || is_optimistic {
                                conv["title"] = json!(t);
                            }
                        }
                        if let Some(jobs) = conv.get_mut("jobs").and_then(|v| v.as_array_mut()) {
                            let new_status = j.get("status").and_then(|v| v.as_str()).unwrap_or("");
                            let new_is_done = matches!(
                                new_status,
                                "completed" | "failed" | "cancelled" | "interrupted" | "waiting_user"
                            );
                            let pos = jobs
                                .iter()
                                .position(|x| x.get("id").and_then(|v| v.as_str()) == Some(jid.as_str()))
                                .or_else(|| {
                                    replace_id.and_then(|rid| {
                                        jobs.iter().position(|x| x.get("id").and_then(|v| v.as_str()) == Some(rid))
                                    })
                                })
                                .or_else(|| {
                                    if !is_optimistic {
                                        jobs.iter().rposition(|x| {
                                            x.get("_optimistic").and_then(|v| v.as_bool()) == Some(true)
                                        })
                                    } else {
                                        None
                                    }
                                });
                            if let Some(pos) = pos {
                                let old_was_pending = matches!(
                                    jobs[pos].get("status").and_then(|v| v.as_str()).unwrap_or(""),
                                    "running" | "queued"
                                );
                                just_finished = old_was_pending && new_is_done;
                                changed = jobs[pos] != j;
                                jobs[pos] = j;
                            } else {
                                just_finished = new_is_done;
                                jobs.push(j);
                                changed = true;
                            }
                        }
                    }
                    self.sync_question_and_task_anims();
                    if self.at_bottom && (changed || just_finished) {
                        self.pending_scroll_bottom = true;
                        out_task = operation::snap_to_end(self.scroll_id.clone());
                    }
                }
                if let Some(n) = ev.get("notice").filter(|v| !v.is_null()) {
                    self.notice = Some(n.clone());
                    self.anim.notice.set_target(1.0, 200.0);
                }
            }
            "job_removed" => {
                let rid = ev.get("jobId").and_then(|v| v.as_str()).unwrap_or("");
                if !rid.is_empty() {
                    let mut drop_conv = false;
                    if let Some(ref mut conv) = self.conversation {
                        if let Some(jobs) = conv.get_mut("jobs").and_then(|v| v.as_array_mut()) {
                            jobs.retain(|x| x.get("id").and_then(|v| v.as_str()) != Some(rid));
                            if jobs.is_empty() && conv.get("id").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
                                drop_conv = true;
                            }
                        }
                    }
                    if drop_conv {
                        self.conversation = None;
                    }
                }
            }
            "notice" => {
                if ev.get("clear").and_then(|v| v.as_bool()).unwrap_or(false) {
                    self.notice = None;
                    self.anim.notice.set_target(0.0, 180.0);
                } else {
                    self.notice = Some(ev);
                    self.anim.notice.set_target(1.0, 200.0);
                }
            }
            "attachments" => {
                self.attachments = ev.get("items").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                self.upload_stages = ev.get("stages").and_then(|v| v.as_object()).cloned().unwrap_or_default();
                self.rebuild_attachment_thumbs();
            }
            "sent_attach_thumbs" => {
                if let Some(items) = ev.get("items").and_then(|v| v.as_array()) {
                    for it in items {
                        let key = it.get("key").and_then(|v| v.as_str()).unwrap_or("");
                        let du = it.get("previewUrl").and_then(|v| v.as_str()).unwrap_or("");
                        if !key.is_empty() {
                            if let Some((_, b64)) = du.split_once("base64,") {
                                if let Ok(bytes) = B64.decode(b64) {
                                    self.sent_attach_thumbs
                                        .insert(key.to_string(), img_w::Handle::from_bytes(bytes));
                                }
                            }
                        }
                    }
                    while self.sent_attach_thumbs.len() > 128 {
                        if let Some(k) = self.sent_attach_thumbs.keys().next().cloned() {
                            self.sent_attach_thumbs.remove(&k);
                        } else {
                            break;
                        }
                    }
                }
            }
            "attachment_preview" => {
                let title = ev.get("title").and_then(|v| v.as_str()).unwrap_or("プレビュー").to_string();
                let k = ev.get("kind").and_then(|v| v.as_str()).unwrap_or("text");
                let modal = if k == "image" {
                    let du = ev.get("dataUrl").and_then(|v| v.as_str()).unwrap_or("");
                    let handle = du
                        .split_once("base64,")
                        .and_then(|(_, b64)| B64.decode(b64).ok())
                        .map(img_w::Handle::from_bytes);
                    ModalKind::Lightbox { title, img: handle, text: String::new() }
                } else {
                    let txt = ev.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    ModalKind::Lightbox { title, img: None, text: txt }
                };
                self.open_modal(modal);
            }
            "files" => {
                self.files_loading = ev.get("loading").and_then(|v| v.as_bool()).unwrap_or(false);
                self.files_items = ev.get("files").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            }
            "workers" => {
                self.workers_panels = ev.get("panels").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            }
            "sharing" => {
                self.sharing_entries = ev.get("entries").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            }
            "log" => {
                let line = ev.get("line").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let level = ev.get("level").and_then(|v| v.as_str()).unwrap_or("info").to_string();
                self.logs.push((line, level));
                if self.logs.len() > 600 {
                    self.logs.remove(0);
                }
            }
            "logs_reset" => {
                self.logs = ev
                    .get("lines")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .map(|r| {
                                (
                                    r.get("line").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                    r.get("level").and_then(|v| v.as_str()).unwrap_or("info").to_string(),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
            }
            "autotune" => {
                self.autotune = ev;
            }
            "pc_windows" => {
                self.pc_windows = ev.get("windows").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                self.selected_window_id = ev.get("selectedId").and_then(|v| v.as_i64()).unwrap_or(0) as isize;
                self.selected_window_title = ev.get("selectedTitle").and_then(|v| v.as_str()).unwrap_or("").into();
            }
            "toast" => {
                let lvl = ev.get("level").and_then(|v| v.as_str()).unwrap_or("ok");
                let msg = ev.get("message").and_then(|v| v.as_str()).unwrap_or("");
                self.anim.push_toast(lvl, msg);
            }
            "command" => {
                if ev.get("action").and_then(|v| v.as_str()) == Some("focus_input") {
                    out_task = operation::focus("prompt-editor");
                }
            }
            _ => {}
        }
        self.refresh_loading_bar_target();
        out_task
    }

    fn rebuild_attachment_thumbs(&mut self) {
        self.attachment_thumbs.clear();
        for a in &self.attachments {
            let p = a.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(du) = a.get("previewUrl").and_then(|v| v.as_str()) {
                if let Some((_, b64)) = du.split_once("base64,") {
                    if let Ok(bytes) = B64.decode(b64) {
                        self.attachment_thumbs.insert(p.to_string(), img_w::Handle::from_bytes(bytes));
                    }
                }
            }
        }
        self.anim.attach_tray.set_target(if self.attachments.is_empty() { 0.0 } else { 1.0 }, 190.0);
    }

    fn open_modal(&mut self, kind: ModalKind) {
        self.selection_scope.clear();
        if matches!(kind, ModalKind::Palette) {
            self.palette_query.clear();
            self.palette_idx = 0;
        }
        self.displayed_modal = Some(kind.clone());
        self.active_modal = Some(kind);
        self.anim.modal.set_target(1.0, 175.0);
    }

    fn close_modal(&mut self) {
        self.active_modal = None;
        self.anim.modal.set_target(0.0, 150.0);
    }

    fn toggle_pane(&mut self, pane: DrawerPane) {
        if pane == DrawerPane::Workers {
            return;
        }
        if self.active_pane == Some(pane) {
            self.active_pane = None;
            self.anim.drawer.set_target(0.0, 220.0);
        } else {
            self.active_pane = Some(pane);
            self.displayed_pane = Some(pane);
            self.anim.drawer.set_target(1.0, 220.0);
            self.anim.drawer_width.set_target(if pane.is_wide() { 520.0 } else { 390.0 }, 220.0);
            match pane {
                DrawerPane::Files => self.send_ipc_bg(json!({ "cmd": "files_load" })),
                DrawerPane::Sharing => self.send_ipc_bg(json!({ "cmd": "share_load" })),
                DrawerPane::Tools => self.send_ipc_bg(json!({ "cmd": "refresh_pc_windows" })),
                _ => {}
            }
        }
    }

    fn execute_palette_action(&mut self, idx: usize) {
        let items = palette_items(&self.palette_query);
        let Some(act) = items.get(idx) else { return };
        if act.disabled {
            return;
        }
        self.close_modal();
        match act.kind {
            "new_chat" => self.send_ipc(json!({ "cmd": "new_chat" })),
            "connect" => self.send_ipc_bg(json!({ "cmd": "connect" })),
            "theme" => self.send_ipc(json!({ "cmd": "theme_toggle" })),
            "mode:standard" => self.send_ipc(json!({ "cmd": "switch_work_mode", "mode": "standard" })),
            "mode:chat" => self.send_ipc(json!({ "cmd": "switch_work_mode", "mode": "chat" })),
            "mode:coding" => self.send_ipc(json!({ "cmd": "switch_work_mode", "mode": "coding" })),
            "mode:pc_control" => self.send_ipc(json!({ "cmd": "switch_work_mode", "mode": "pc_control" })),
            "pane:workers" => self.toggle_pane(DrawerPane::Workers),
            "pane:tools" => self.toggle_pane(DrawerPane::Tools),
            "pane:settings" => self.toggle_pane(DrawerPane::Settings),
            "restart_server" => self.send_ipc_bg(json!({ "cmd": "restart_server" })),
            "stop_pc" => self.send_ipc(json!({ "cmd": "stop_pc_all" })),
            k if k.starts_with("slash:") => {
                self.send_ipc(json!({ "cmd": "send_message", "text": &k[6..] }));
            }
            _ => {}
        }
    }

    fn apply_slash_item(&mut self, idx: usize) {
        let txt = self.composer_content.text();
        let hits = filtered_slash_items(&txt);
        let Some(it) = hits.get(idx) else { return };
        if it.disabled {
            return;
        }
        self.anim.slash_menu.set_target(0.0, 140.0);
        if !it.cmd.starts_with('!') {
            self.composer_content = text_editor::Content::with_text(it.cmd);
            return;
        }
        self.composer_content = text_editor::Content::new();
        match it.cmd {
            "!new" => self.send_ipc(json!({ "cmd": "new_chat" })),
            "!connect" => self.send_ipc_bg(json!({ "cmd": "connect" })),
            "!compact" => self.send_ipc(json!({ "cmd": "send_message", "text": "/compact" })),
            "!status" => self.send_ipc(json!({ "cmd": "send_message", "text": "/status" })),
            "!help" => self.send_ipc(json!({ "cmd": "send_message", "text": "/help" })),
            "!pane:workers" => self.toggle_pane(DrawerPane::Workers),
            "!pane:tools" => self.toggle_pane(DrawerPane::Tools),
            "!pane:settings" => self.toggle_pane(DrawerPane::Settings),
            _ => {}
        }
    }

    pub fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::BackendAction(act) => match act {
                UiAction::PushEvent(ev) => return self.apply_backend_event(ev),
                UiAction::ApplyTheme(dark) => apply_dwm_appearance(self.hwnd, dark),
                UiAction::FlashTaskbar => flash_taskbar(self.hwnd),
            },
            Message::WinEvent(id, ev) => {
                self.window_id = Some(id);
                match ev {
                    window::Event::Opened { .. } => {
                        let initial_dark = self.is_dark();
                        let geom = self.controller.saved_window_geometry();
                        return window::run(id, move |w| {
                            use window::raw_window_handle::RawWindowHandle;
                            let hwnd = match w.window_handle().ok().map(|h| h.as_raw()) {
                                Some(RawWindowHandle::Win32(h)) => h.hwnd.get(),
                                _ => 0,
                            };
                            if hwnd != 0 {
                                apply_dwm_appearance(hwnd, initial_dark);
                                set_window_cloaked(hwnd, true);
                                show_window_initial(hwnd, geom);
                            }
                            hwnd
                        })
                        .map(Message::GotHwnd);
                    }
                    window::Event::CloseRequested => {
                        let placement = capture_window_placement(self.hwnd);
                        self.controller.on_window_closing(placement);
                        #[cfg(windows)]
                        {
                            let tid = self.hotkey_tid.load(Ordering::SeqCst);
                            if tid != 0 {
                                unsafe {
                                    let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
                                }
                            }
                        }
                        return window::close(id);
                    }
                    window::Event::FileHovered(_) => {
                        self.hover_drop_count += 1;
                        self.anim.drop_overlay.set_target(1.0, 140.0);
                    }
                    window::Event::FilesHoveredLeft => {
                        self.hover_drop_count = 0;
                        self.anim.drop_overlay.set_target(0.0, 140.0);
                    }
                    window::Event::FileDropped(path) => {
                        self.hover_drop_count = 0;
                        self.anim.drop_overlay.set_target(0.0, 140.0);
                        let s = path.to_string_lossy().into_owned();
                        self.send_ipc_bg(json!({ "cmd": "add_dropped_files", "paths": [s] }));
                    }
                    _ => {}
                }
            }
            Message::GotHwnd(hwnd) => {
                if hwnd != 0 {
                    self.hwnd = hwnd;
                    self.controller.set_hwnd(hwnd);
                    self.pending_reveal_frames = 2;
                    return Task::perform(
                        tokio::time::sleep(Duration::from_millis(250)),
                        |_| Message::RevealWindow,
                    );
                }
            }
            Message::RevealWindow => {
                if self.pending_reveal_frames > 0 && self.hwnd != 0 {
                    self.pending_reveal_frames = 0;
                    set_window_cloaked(self.hwnd, false);
                }
            }
            Message::AnimFrame(now) => {
                let cycle = self.is_any_loading_or_active() || self.standby || self.connected;
                self.anim.tick(now, cycle);
                if self.active_pane.is_none() && self.anim.drawer.value() <= 0.015 {
                    self.displayed_pane = None;
                }
                if self.active_modal.is_none() && self.anim.modal.value() <= 0.015 {
                    self.displayed_modal = None;
                }
                if self.pending_reveal_frames > 0 {
                    self.pending_reveal_frames -= 1;
                    if self.pending_reveal_frames == 0 && self.hwnd != 0 {
                        set_window_cloaked(self.hwnd, false);
                    }
                }
            }
            Message::KeyEvent(keyboard::Event::KeyPressed {
                key,
                modified_key,
                physical_key,
                modifiers,
                ..
            }) => {
                if modifiers.control() {
                    match key.to_latin(physical_key) {
                        Some('k') => {
                            if matches!(self.active_modal, Some(ModalKind::Palette)) {
                                self.close_modal();
                            } else {
                                self.open_modal(ModalKind::Palette);
                                return operation::focus("palette-input");
                            }
                        }
                        Some('n') => self.send_ipc(json!({ "cmd": "new_chat" })),
                        Some('b') => {
                            self.sidebar_open = !self.sidebar_open;
                            self.anim.sidebar.set_target(if self.sidebar_open { 1.0 } else { 0.0 }, 210.0);
                        }
                        Some('f') => {
                            self.find_open = true;
                            self.anim.find_bar.set_target(1.0, 180.0);
                            return operation::focus("find-input");
                        }
                        _ => {
                            if key.as_ref() == keyboard::Key::Character("/") {
                                if matches!(self.active_modal, Some(ModalKind::Shortcuts)) {
                                    self.close_modal();
                                } else {
                                    self.open_modal(ModalKind::Shortcuts);
                                }
                            }
                        }
                    }
                } else if matches!(key.as_ref(), keyboard::Key::Named(keyboard::key::Named::F5)) {
                    self.send_ipc_bg(json!({ "cmd": "connect" }));
                } else if matches!(key.as_ref(), keyboard::Key::Named(keyboard::key::Named::Escape)) {
                    if self.active_modal.is_some() {
                        self.close_modal();
                    } else if !self.composer_content.text().is_empty() && !filtered_slash_items(&self.composer_content.text()).is_empty() {
                        self.anim.slash_menu.set_target(0.0, 140.0);
                    } else if self.find_open {
                        self.find_open = false;
                        self.anim.find_bar.set_target(0.0, 180.0);
                    } else if self.active_pane.is_some() {
                        self.active_pane = None;
                        self.anim.drawer.set_target(0.0, 220.0);
                    } else {
                        self.selection_scope.clear();
                    }
                } else if matches!(self.active_modal, Some(ModalKind::Palette)) {
                    let items = palette_items(&self.palette_query);
                    let count = items.len();
                    if count > 0 {
                        let step = match modified_key.as_ref() {
                            keyboard::Key::Named(keyboard::key::Named::ArrowDown) => Some(1isize),
                            keyboard::Key::Named(keyboard::key::Named::ArrowUp) => Some(-1isize),
                            _ => None,
                        };
                        if let Some(d) = step {
                            for k in 1..=count {
                                let cand = (((self.palette_idx as isize + d * k as isize) % count as isize + count as isize) % count as isize) as usize;
                                if !items[cand].disabled {
                                    self.palette_idx = cand;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            Message::KeyEvent(_) => {}
            Message::ClipboardTextPaste(maybe_txt) => {
                if let Some(txt) = maybe_txt {
                    self.composer_content
                        .perform(text_editor::Action::Edit(text_editor::Edit::Paste(Arc::new(txt))));
                }
            }
            Message::ClearCopyFeedback(k) => {
                if self.copied_key.as_deref() == Some(k.as_str()) {
                    self.copied_key = None;
                }
            }
            Message::ToggleTheme => self.send_ipc(json!({ "cmd": "theme_toggle" })),
            Message::OpenModal(k) => {
                let focus_pal = matches!(k, ModalKind::Palette);
                self.open_modal(k);
                if focus_pal {
                    return operation::focus("palette-input");
                }
            }
            Message::CloseModal => self.close_modal(),
            Message::NewChat => {
                self.parsed_answers.clear();
                self.decoded_activity_imgs.clear();
                self.selection_scope.clear();
                self.at_bottom = true;
                self.last_scroll_abs_y = 0.0;
                self.last_scroll_content_h = 0.0;
                self.last_scroll_viewport_h = 0.0;
                self.pending_scroll_bottom = true;
                self.send_ipc(json!({ "cmd": "new_chat" }));
            }
            Message::RefreshHistory => self.send_ipc_bg(json!({ "cmd": "refresh_history" })),
            Message::HistorySearchChanged(s) => self.history_query = s,
            Message::SetHistoryFilter(f) => self.history_filter = f,
            Message::OpenConversation(cid) => {
                self.selection_scope.clear();
                self.at_bottom = true;
                self.last_scroll_abs_y = 0.0;
                self.last_scroll_content_h = 0.0;
                self.last_scroll_viewport_h = 0.0;
                self.pending_scroll_bottom = true;
                self.send_ipc_bg(json!({ "cmd": "open_conversation", "id": cid }));
            }
            Message::CopyText(txt, key_opt) => {
                let _ = clipboard_set_text(&txt);
                self.anim.push_toast("ok", "コピーしました");
                if let Some(k) = key_opt {
                    let k2 = k.clone();
                    self.copied_key = Some(k);
                    return Task::perform(
                        tokio::time::sleep(Duration::from_millis(1300)),
                        move |_| Message::ClearCopyFeedback(k2),
                    );
                }
            }
            Message::SelectionCopied(txt) => {
                if !txt.is_empty() {
                    let _ = clipboard_set_text(&txt);
                    self.anim.push_toast("ok", "コピーしました");
                }
            }
            Message::SelectionMouseUp => {
                self.selection_scope.end_drag();
            }
            Message::SelectionClear => {
                self.selection_scope.clear();
            }
            Message::CopyActiveSelection => {
                let txt = self.selection_scope.copied_text();
                if !txt.is_empty() {
                    let _ = clipboard_set_text(&txt);
                    self.anim.push_toast("ok", "コピーしました");
                }
            }
            Message::ConnectNow => self.send_ipc_bg(json!({ "cmd": "connect" })),
            Message::ToggleSidebar => {
                self.sidebar_open = !self.sidebar_open;
                self.anim.sidebar.set_target(if self.sidebar_open { 1.0 } else { 0.0 }, 210.0);
            }
            Message::TogglePane(p) => self.toggle_pane(p),
            Message::CloseDrawer => {
                self.active_pane = None;
                self.anim.drawer.set_target(0.0, 220.0);
            }
            Message::SetTuningOpen(open) => {
                self.tuning_open = open;
                self.send_ipc(json!({ "cmd": "set_tuning_open", "open": open }));
            }
            Message::ToggleFind => {
                self.find_open = !self.find_open;
                self.anim.find_bar.set_target(if self.find_open { 1.0 } else { 0.0 }, 180.0);
                if self.find_open {
                    return operation::focus("find-input");
                }
            }
            Message::FindQueryChanged(q) => {
                self.selection_scope.clear();
                self.find_query = q;
                self.find_match_idx = 0;
            }
            Message::StepFind(delta) => {
                let hits = self.find_match_jobs();
                if !hits.is_empty() {
                    let len = hits.len() as isize;
                    let cur = self.find_match_idx as isize;
                    self.find_match_idx = (((cur + delta as isize) % len + len) % len) as usize;
                }
            }
            Message::ToggleAccordion(key, default_open) => {
                self.anim.toggle_accordion(&key, default_open);
            }
            Message::ToggleTaskCollapse => {
                self.task_strip_collapsed = !self.task_strip_collapsed;
                self.anim.task_body.set_target(if self.task_strip_collapsed { 0.0 } else { 1.0 }, 190.0);
            }
            Message::EditorAction(act) => {
                if !matches!(act, text_editor::Action::Scroll { .. }) {
                    self.selection_scope.clear();
                }
                self.composer_content.perform(act);
                let hits = filtered_slash_items(&self.composer_content.text());
                let has_slash = !hits.is_empty();
                if !has_slash {
                    self.slash_idx = 0;
                } else if hits.get(self.slash_idx).map_or(true, |it| it.disabled) {
                    self.slash_idx = hits.iter().position(|it| !it.disabled).unwrap_or(0);
                }
                self.anim.slash_menu.set_target(if has_slash { 1.0 } else { 0.0 }, 150.0);
            }
            Message::InsertStarter(prompt) => {
                self.composer_content = text_editor::Content::with_text(&prompt);
                return operation::focus("prompt-editor");
            }
            Message::SendPrompt => {
                let txt = self.composer_content.text();
                if !txt.trim().is_empty() || !self.attachments.is_empty() {
                    self.composer_content = text_editor::Content::new();
                    self.anim.slash_menu.set_target(0.0, 140.0);
                    self.at_bottom = true;
                    self.pending_scroll_bottom = true;
                    self.send_ipc(json!({ "cmd": "send_message", "text": txt }));
                    self.refresh_loading_bar_target();
                    return operation::snap_to_end(self.scroll_id.clone());
                }
            }
            Message::ComposerCtrlV => {
                match clipboard_read_payload() {
                    Ok(ClipboardPayload::PngBytes(_))
                    | Ok(ClipboardPayload::DibBytes(_))
                    | Ok(ClipboardPayload::Files(_)) => {
                        self.send_ipc(json!({ "cmd": "paste_clipboard" }));
                    }
                    _ => return clipboard::read().map(Message::ClipboardTextPaste),
                }
            }
            Message::GlobalPasteCheck => {
                if matches!(
                    clipboard_read_payload(),
                    Ok(ClipboardPayload::PngBytes(_))
                        | Ok(ClipboardPayload::DibBytes(_))
                        | Ok(ClipboardPayload::Files(_))
                ) {
                    self.send_ipc(json!({ "cmd": "paste_clipboard" }));
                }
            }
            Message::SlashMove(delta) => {
                let hits = filtered_slash_items(&self.composer_content.text());
                let count = hits.len();
                if count > 0 {
                    let len = count as isize;
                    let cur = self.slash_idx as isize;
                    let step = if delta >= 0 { 1isize } else { -1isize };
                    for k in 1..=count {
                        let cand = (((cur + step * k as isize) % len + len) % len) as usize;
                        if !hits[cand].disabled {
                            self.slash_idx = cand;
                            break;
                        }
                    }
                }
            }
            Message::SlashAccept => {
                self.apply_slash_item(self.slash_idx);
                return operation::focus("prompt-editor");
            }
            Message::SlashPick(idx) => {
                self.apply_slash_item(idx);
                return operation::focus("prompt-editor");
            }
            Message::CancelJob => self.send_ipc(json!({ "cmd": "cancel_job" })),
            Message::AnswerQuestion(qid, ans) => {
                self.send_ipc(json!({ "cmd": "answer_question", "questionId": qid, "answer": ans }));
            }
            Message::ContinueChat => self.send_ipc(json!({ "cmd": "continue_chat" })),
            Message::CloseNotice => {
                self.notice = None;
                self.anim.notice.set_target(0.0, 180.0);
            }
            Message::LoadFullJob(jid) => self.send_ipc_bg(json!({ "cmd": "load_full_job", "jobId": jid })),
            Message::SaveDownloads(jid) => self.send_ipc_bg(json!({ "cmd": "save_downloads", "jobId": jid })),
            Message::OpenExternal(url) => self.send_ipc(json!({ "cmd": "open_external", "target": url })),
            Message::PickAttachments => self.send_ipc_bg(json!({ "cmd": "pick_attachments" })),
            Message::ClearAttachments => self.send_ipc(json!({ "cmd": "clear_attachments" })),
            Message::RemoveAttachment(p) => self.send_ipc(json!({ "cmd": "remove_attachment", "path": p })),
            Message::PreviewAttachment(i) => self.send_ipc(json!({ "cmd": "preview_attachment", "index": i })),
            Message::PreviewSentAttachment {
                conv_id,
                path,
                display_name,
            } => {
                self.send_ipc_bg(json!({
                    "cmd": "preview_sent_attachment",
                    "conversationId": conv_id,
                    "path": path,
                    "displayName": display_name,
                }));
            }
            Message::OpenLightboxImage(title, h) => {
                self.open_modal(ModalKind::Lightbox { title, img: Some(h), text: String::new() });
            }
            Message::SelectModelRole(_) => {}
            Message::SelectWorkMode(m) => self.send_ipc(json!({ "cmd": "switch_work_mode", "mode": m.as_str() })),
            Message::TogglePlanMode => {}
            Message::OpenTuningQuick => {
                self.tuning_open = true;
                self.active_pane = Some(DrawerPane::Settings);
                self.displayed_pane = Some(DrawerPane::Settings);
                self.anim.drawer.set_target(1.0, 220.0);
                self.anim.drawer_width.set_target(520.0, 220.0);
                self.send_ipc(json!({ "cmd": "set_tuning_open", "open": true }));
            }
            Message::ChatScrolled {
                abs_y,
                dist_from_bottom,
                content_h,
                viewport_h,
            } => {
                let prev_abs_y = self.last_scroll_abs_y;
                let prev_content_h = self.last_scroll_content_h;
                let prev_viewport_h = self.last_scroll_viewport_h;
                self.last_scroll_abs_y = abs_y;
                self.last_scroll_content_h = content_h;
                self.last_scroll_viewport_h = viewport_h;
                let not_scrollable = content_h <= viewport_h + 1.0 || dist_from_bottom.is_nan();
                if not_scrollable || dist_from_bottom <= 24.0 {
                    self.at_bottom = true;
                    self.pending_scroll_bottom = false;
                } else {
                    let prev_max_y = if prev_viewport_h > 0.0 {
                        (prev_content_h - prev_viewport_h).max(0.0)
                    } else {
                        (prev_content_h - viewport_h).max(0.0)
                    };
                    let cur_max_y = (content_h - viewport_h).max(0.0);
                    let content_grew = prev_content_h > 0.0
                        && (content_h > prev_content_h + 1.0 || cur_max_y > prev_max_y + 1.0);
                    let user_scrolled_up = abs_y < prev_abs_y - 1.0;
                    if (self.at_bottom && content_grew && !user_scrolled_up)
                        || (self.pending_scroll_bottom && !user_scrolled_up)
                    {
                        self.at_bottom = true;
                        self.pending_scroll_bottom = true;
                        self.refresh_loading_bar_target();
                        return operation::snap_to_end(self.scroll_id.clone());
                    }
                    self.at_bottom = false;
                    self.pending_scroll_bottom = false;
                }
            }
            Message::ScrollToBottom => {
                self.at_bottom = true;
                self.pending_scroll_bottom = true;
                self.refresh_loading_bar_target();
                return operation::snap_to_end(self.scroll_id.clone());
            }
            Message::SetWorkspaceEnabled(b) => self.send_ipc(json!({ "cmd": "set_workspace_enabled", "enabled": b })),
            Message::SetLocalCli(b) => self.send_ipc(json!({ "cmd": "set_local_cli", "enabled": b })),
            Message::SetRunOnPc(b) => self.send_ipc(json!({ "cmd": "set_run_on_pc", "enabled": b })),
            Message::SetPcEnabled(b) => self.send_ipc(json!({ "cmd": "set_pc_enabled", "enabled": b })),
            Message::SetPcStandby(b) => self.send_ipc(json!({ "cmd": "set_pc_standby", "enabled": b })),
            Message::PickWorkdir => self.send_ipc_bg(json!({ "cmd": "pick_workdir" })),
            Message::OpenLocalFolder => self.send_ipc(json!({ "cmd": "open_local_folder" })),
            Message::ResetWorkdir => self.send_ipc(json!({ "cmd": "reset_workdir" })),
            Message::AddReadFolder => self.send_ipc_bg(json!({ "cmd": "add_read_folder" })),
            Message::ClearReadFolders => self.send_ipc(json!({ "cmd": "clear_read_folders" })),
            Message::RemoveReadFolder(i) => self.send_ipc(json!({ "cmd": "remove_read_folder", "index": i })),
            Message::StopPcAll => self.send_ipc(json!({ "cmd": "stop_pc_all" })),
            Message::WindowFilterChanged(s) => self.window_filter = s,
            Message::RefreshPcWindows => self.send_ipc(json!({ "cmd": "refresh_pc_windows" })),
            Message::SelectPcWindow(id) => self.send_ipc(json!({ "cmd": "select_pc_window", "id": id })),
            Message::DraftApi(s) => self.settings_draft.api = s,
            Message::DraftAutoConnect(b) => self.settings_draft.auto_connect = b,
            Message::SaveAndConnect => {
                let d = &self.settings_draft;
                let api = d.api.trim().to_string();
                self.send_ipc(json!({ "cmd": "update_pref", "api": api, "autoConnect": d.auto_connect }));
                self.send_ipc_bg(json!({ "cmd": "connect", "api": api, "autoConnect": d.auto_connect }));
            }
            Message::StartServer => {
                let d = &self.settings_draft;
                self.send_ipc_bg(json!({ "cmd": "start_server", "api": d.api.trim(), "autoConnect": d.auto_connect }));
            }
            Message::RestartServer => {
                let d = &self.settings_draft;
                self.send_ipc_bg(json!({ "cmd": "restart_server", "api": d.api.trim(), "autoConnect": d.auto_connect }));
            }
            Message::StopServer => {
                let d = &self.settings_draft;
                self.send_ipc_bg(json!({ "cmd": "stop_server", "api": d.api.trim() }));
            }
            Message::DraftCtx(s) => self.settings_draft.ctx = s,
            Message::DraftTok(s) => self.settings_draft.tok = s,
            Message::DraftEffort(e) => self.settings_draft.effort = e,
            Message::DraftBudget(s) => self.settings_draft.budget = s,
            Message::ApplyServerSettings => {
                let d = &self.settings_draft;
                let ctx = d.ctx.trim().parse::<i32>().unwrap_or(16384).clamp(2048, 32768);
                let tok = d.tok.trim().parse::<i32>().unwrap_or(1800).clamp(128, 8192);
                let bdg = d.budget.trim().parse::<i32>().unwrap_or(-1).clamp(-1, 8192);
                self.send_ipc_bg(json!({
                    "cmd": "apply_settings",
                    "api": d.api.trim(),
                    "autoConnect": d.auto_connect,
                    "contextWindow": ctx,
                    "maxTokens": tok,
                    "reasoningEffort": d.effort.as_str(),
                    "reasoningBudgetTokens": bdg,
                }));
            }
            Message::SelectPreset(name) => {
                self.settings_draft.sampling_preset = name.clone();
                let (t, tp, tk, mp, rp, pp, fp) = match name.as_str() {
                    "balanced" => (0.20, 0.95, 40, 0.05, 1.10, 0.0, 0.0),
                    "creative" => (0.70, 0.95, 60, 0.03, 1.15, 0.10, 0.10),
                    _ => (0.05, 0.95, 40, 0.05, 1.10, 0.0, 0.0),
                };
                self.settings_draft.temperature = t;
                self.settings_draft.top_p = tp;
                self.settings_draft.top_k = tk;
                self.settings_draft.min_p = mp;
                self.settings_draft.repeat_penalty = rp;
                self.settings_draft.presence_penalty = pp;
                self.settings_draft.frequency_penalty = fp;
                self.send_ipc_bg(json!({
                    "cmd": "apply_settings",
                    "samplingPreset": name,
                    "temperature": t,
                    "top_p": tp,
                    "top_k": tk,
                    "min_p": mp,
                    "repeat_penalty": rp,
                    "presence_penalty": pp,
                    "frequency_penalty": fp,
                }));
            }
            Message::ResetSampling => self.send_ipc_bg(json!({ "cmd": "reset_sampling" })),
            Message::SliderTemp(v) => { self.settings_draft.temperature = v; self.settings_draft.sampling_preset = "custom".into(); }
            Message::SliderTopP(v) => { self.settings_draft.top_p = v; self.settings_draft.sampling_preset = "custom".into(); }
            Message::SliderTopK(v) => { self.settings_draft.top_k = v; self.settings_draft.sampling_preset = "custom".into(); }
            Message::SliderMinP(v) => { self.settings_draft.min_p = v; self.settings_draft.sampling_preset = "custom".into(); }
            Message::SliderRepeat(v) => { self.settings_draft.repeat_penalty = v; self.settings_draft.sampling_preset = "custom".into(); }
            Message::SliderPresence(v) => { self.settings_draft.presence_penalty = v; self.settings_draft.sampling_preset = "custom".into(); }
            Message::SliderFrequency(v) => { self.settings_draft.frequency_penalty = v; self.settings_draft.sampling_preset = "custom".into(); }
            Message::ApplySampling => {
                let d = &self.settings_draft;
                self.send_ipc_bg(json!({
                    "cmd": "apply_settings",
                    "temperature": d.temperature,
                    "top_p": d.top_p,
                    "top_k": d.top_k,
                    "min_p": d.min_p,
                    "repeat_penalty": d.repeat_penalty,
                    "presence_penalty": d.presence_penalty,
                    "frequency_penalty": d.frequency_penalty,
                }));
            }
            Message::RefreshPerf => self.send_ipc_bg(json!({ "cmd": "refresh_perf" })),
            Message::SelectTuneMode(m) => self.settings_draft.tune_mode = m,
            Message::StartAutoTune => {
                let m = self.settings_draft.tune_mode.as_str();
                self.send_ipc_bg(json!({ "cmd": "start_autotune", "mode": m }));
            }
            Message::FilesRefresh => self.send_ipc_bg(json!({ "cmd": "files_load" })),
            Message::FilesPreview(p) => self.send_ipc_bg(json!({ "cmd": "files_preview", "path": p })),
            Message::DraftShareIp(s) => self.settings_draft.share_ip = s,
            Message::DraftShareMode(m) => self.settings_draft.share_mode = m,
            Message::ShareAdd => {
                let ip = self.settings_draft.share_ip.trim().to_string();
                let mode = self.settings_draft.share_mode.as_str();
                if !ip.is_empty() {
                    self.send_ipc_bg(json!({ "cmd": "share_add", "ip": ip, "mode": mode }));
                }
            }
            Message::ShareRemove(i) => self.send_ipc_bg(json!({ "cmd": "share_remove", "index": i })),
            Message::LogFilterChanged(s) => self.log_filter = s,
            Message::CopyVisibleLogs => {
                let f = self.log_filter.to_ascii_lowercase();
                let txt = self
                    .logs
                    .iter()
                    .filter(|(l, _)| f.is_empty() || l.to_ascii_lowercase().contains(&f))
                    .map(|(l, _)| l.trim_end_matches(['\r', '\n']))
                    .collect::<Vec<_>>()
                    .join("\r\n");
                let _ = clipboard_set_text(&txt);
                self.anim.push_toast("ok", "コピーしました");
            }
            Message::PaletteQueryChanged(s) => {
                self.selection_scope.clear();
                self.palette_query = s;
                self.palette_idx = palette_items(&self.palette_query)
                    .iter()
                    .position(|a| !a.disabled)
                    .unwrap_or(0);
            }
            Message::PaletteRun(i) => self.execute_palette_action(i),
            Message::Noop => {}
        }
        self.refresh_loading_bar_target();
        Task::none()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let pal = theme::palette(self.is_dark());
        let shell = row![
            container(render_sidebar(self, pal)).height(Length::Fill),
            render_main_workspace(self, pal),
            container(render_right_drawer(self, pal)).height(Length::Fill),
        ]
        .height(Length::Fill)
        .width(Length::Fill);

        let mut layers: Vec<Element<'_, Message>> = vec![
            container(shell)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::canvas_style(pal))
                .into(),
        ];
        layers.extend(render_overlays(self, pal));

        stack(layers)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            Subscription::run_with(RxBridge(1, Arc::clone(&self.rx_slot)), backend_stream),
            window::events().map(|(id, ev)| Message::WinEvent(id, ev)),
            event::listen_with(|ev, status, _| match ev {
                iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    Some(Message::SelectionMouseUp)
                }
                iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                    if status == event::Status::Ignored =>
                {
                    Some(Message::SelectionClear)
                }
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key,
                    physical_key,
                    modifiers,
                    ..
                }) if status == event::Status::Ignored
                    && modifiers.command()
                    && !modifiers.alt()
                    && key.to_latin(physical_key) == Some('c') =>
                {
                    Some(Message::CopyActiveSelection)
                }
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key,
                    physical_key,
                    modifiers,
                    ..
                }) if status == event::Status::Ignored
                    && modifiers.control()
                    && !modifiers.alt()
                    && key.to_latin(physical_key) == Some('v') =>
                {
                    Some(Message::GlobalPasteCheck)
                }
                iced::Event::Keyboard(ke) => Some(Message::KeyEvent(ke)),
                _ => None,
            }),
        ];
        if self.pending_reveal_frames > 0 || self.anim.needs_frame(self.is_any_loading_or_active()) {
            subs.push(window::frames().map(Message::AnimFrame));
        } else if self.standby {
            subs.push(iced::time::every(Duration::from_millis(80)).map(Message::AnimFrame));
        }
        Subscription::batch(subs)
    }

    pub fn theme(&self) -> Theme {
        theme::iced_theme(self.is_dark())
    }
}

pub fn run_gui(cli_args: &[String]) -> i32 {
    if std::env::var_os("ICED_BACKEND").is_none() {
        std::env::set_var("ICED_BACKEND", "wgpu,tiny-skia");
    }
    #[cfg(windows)]
    if std::env::var_os("WGPU_BACKEND").is_none() {
        let preferred = if std::path::Path::new(r"C:\Windows\System32\vulkan-1.dll").exists() {
            "vulkan"
        } else {
            "dx12"
        };
        std::env::set_var("WGPU_BACKEND", preferred);
    }
    if std::env::var_os("WGPU_POWER_PREFERENCE").is_none() {
        std::env::set_var("WGPU_POWER_PREFERENCE", "low");
    }
    std::thread::spawn(|| {
        let _ = iced::advanced::graphics::text::font_system();
    });

    let (tx, rx) = mpsc::unbounded_channel::<UiAction>();
    let controller = AppController::new(
        cli_args,
        Arc::new(move |act| {
            let _ = tx.send(act);
        }),
    );

    let (sx, sy, sw, sh, _smax) = controller.saved_window_geometry();
    let (init_gui, init_api, init_workdir, init_roots, init_win_title) =
        controller.initial_ui_snapshot();
    #[cfg(windows)]
    let hotkey_tid = {
        let hotkey_tid = Arc::new(AtomicU32::new(0));
        let hk_tid_setter = Arc::clone(&hotkey_tid);
        let hk_ctrl = Arc::clone(&controller);
        std::thread::spawn(move || unsafe {
            hk_tid_setter.store(GetCurrentThreadId(), Ordering::SeqCst);
            let ok = RegisterHotKey(None, 1, MOD_CONTROL | MOD_SHIFT, VK_F12.0 as u32).is_ok();
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.message == WM_HOTKEY && msg.wParam.0 == 1 {
                    hk_ctrl.trigger_emergency_pc_stop();
                }
            }
            if ok {
                let _ = UnregisterHotKey(None, 1);
            }
        });
        hotkey_tid
    };

    struct BootInit {
        controller: Arc<AppController>,
        rx: mpsc::UnboundedReceiver<UiAction>,
        #[cfg(windows)]
        hotkey_tid: Arc<AtomicU32>,
        init_gui: Value,
        init_api: String,
        init_workdir: String,
        init_roots: Vec<String>,
        init_win_title: String,
    }
    let init_slot = Arc::new(Mutex::new(Some(BootInit {
        controller,
        rx,
        #[cfg(windows)]
        hotkey_tid,
        init_gui,
        init_api,
        init_workdir,
        init_roots,
        init_win_title,
    })));
    let boot_slot = Arc::clone(&init_slot);

    let win_settings = window::Settings {
        size: Size::new(sw as f32, sh as f32),
        min_size: Some(Size::new(1000.0, 660.0)),
        maximized: false,
        visible: !cfg!(windows),
        position: if let (Some(x), Some(y)) = (sx, sy) {
            window::Position::Specific(Point::new(x as f32, y as f32))
        } else {
            window::Position::Centered
        },
        exit_on_close_request: false,
        ..window::Settings::default()
    };

    let res = iced::application(
        move || {
            let boot = boot_slot.lock().take().expect("iced boot called twice");
            let ctrl_init = Arc::clone(&boot.controller);
            std::thread::spawn(move || {
                ctrl_init.handle_ipc(json!({ "cmd": "init" }));
            });
            let empty_srv = json!({});
            let mut settings_draft = SettingsDraft::default();
            settings_draft.sync_from(&boot.init_gui, &empty_srv);
            let state = UiState {
                controller: boot.controller,
                rx_slot: Arc::new(Mutex::new(Some(boot.rx))),
                window_id: None,
                hwnd: 0,
                #[cfg(windows)]
                hotkey_tid: boot.hotkey_tid,
                connected: false,
                connecting: false,
                busy: false,
                standby: false,
                elevated: false,
                computer_name: String::new(),
                api_url: boot.init_api,
                loading_history: false,
                loading_conversation: false,
                loading_preview: false,
                gui: boot.init_gui,
                server: empty_srv,
                capabilities: json!({}),
                perf: json!({}),
                autotune: json!({}),
                conversations: Vec::new(),
                conversation: None,
                read_only: false,
                attachments: Vec::new(),
                attachment_thumbs: HashMap::new(),
                upload_stages: Map::new(),
                pc_windows: Vec::new(),
                selected_window_id: 0,
                selected_window_title: boot.init_win_title,
                local_workdir: boot.init_workdir,
                read_roots: boot.init_roots,
                workers_panels: Vec::new(),
                files_items: Vec::new(),
                files_loading: false,
                sharing_entries: Vec::new(),
                logs: Vec::new(),
                notice: None,
                status_main: "未接続".into(),
                status_tail: String::new(),
                status_stale: false,
                sidebar_open: true,
                active_pane: None,
                displayed_pane: None,
                tuning_open: false,
                history_filter: HistoryFilter::All,
                history_query: String::new(),
                window_filter: String::new(),
                log_filter: String::new(),
                find_open: false,
                find_query: String::new(),
                find_match_idx: 0,
                task_strip_collapsed: false,
                active_modal: None,
                displayed_modal: None,
                palette_query: String::new(),
                palette_idx: 0,
                slash_idx: 0,
                settings_draft,
                composer_content: text_editor::Content::new(),
                copied_key: None,
                scroll_id: Id::new("chat-scroll"),
                at_bottom: true,
                last_scroll_abs_y: 0.0,
                last_scroll_content_h: 0.0,
                last_scroll_viewport_h: 0.0,
                pending_scroll_bottom: false,
                hover_drop_count: 0,
                parsed_answers: HashMap::new(),
                decoded_activity_imgs: HashMap::new(),
                sent_attach_thumbs: HashMap::new(),
                selection_scope: SelectionScope::default(),
                anim: UiAnimations::default(),
                pending_reveal_frames: 0,
            };
            (state, Task::none())
        },
        UiState::update,
        UiState::view,
    )
    .title(|_: &UiState| String::from("Qwen27"))
    .theme(UiState::theme)
    .default_font(UI_FONT)
    .subscription(UiState::subscription)
    .window(win_settings)
    .run();

    if res.is_ok() { 0 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use iced::advanced::layout::Limits;
    use iced::advanced::renderer::{self, Headless};
    use iced::advanced::widget::Tree;
    use iced::advanced::{Layout, Renderer as _};
    use std::io::Write;
    use std::path::Path;

    fn make_mock_state(
        dark: bool,
        drawer_open: bool,
        find_open: bool,
        empty_welcome_with_notice: bool,
    ) -> UiState {
        let controller = AppController::new(&["Qwen27".to_string()], Arc::new(|_| {}));
        let (_tx, rx) = mpsc::unbounded_channel::<UiAction>();
        let mut anim = UiAnimations::default();
        if drawer_open {
            anim.drawer = crate::ui::anim::EaseAnim::new(1.0);
            anim.drawer_width = crate::ui::anim::EaseAnim::new(520.0);
        }
        if find_open {
            anim.find_bar = crate::ui::anim::EaseAnim::new(1.0);
        }
        if empty_welcome_with_notice {
            anim.notice = crate::ui::anim::EaseAnim::new(1.0);
        }
        let gui = json!({
            "darkTheme": dark,
            "modelRole": "planner",
            "workMode": if empty_welcome_with_notice { "pc_control" } else { "coding" },
            "planMode": false,
            "workspaceEnabled": true,
            "contextWindow": 16384,
            "maxTokens": 4096,
            "reasoningEffort": "medium"
        });
        let long_title = "qwen_frameworkのgui等を改良してモダンなネイティブRust UIへ全面換装する長文テストタイトル";
        let mut convs = vec![
            json!({
                "id": "conv-1",
                "title": long_title,
                "status": "completed",
                "jobCount": 3,
                "updatedAt": "2026-11-23T10:00:00Z"
            }),
            json!({
                "id": "conv-2",
                "title": "非常に長いタイトルの会話項目がIDボタンに重ならないか確認するテスト",
                "status": "running",
                "jobCount": 1,
                "updatedAt": "2026-11-23T11:30:00Z"
            }),
        ];
        for i in 3..=18 {
            convs.push(json!({
                "id": format!("conv-{}", i),
                "title": format!("過去の検証チャット項目 #{}（スクロール枠干渉チェック）", i),
                "status": "completed",
                "jobCount": 2,
                "updatedAt": "2026-11-23T09:00:00Z"
            }));
        }
        let conv = json!({
            "id": "conv-1",
            "title": long_title,
            "jobs": [
                {
                    "id": "job-1",
                    "status": "completed",
                    "createdAt": "2026-11-23T10:00:00.000Z",
                    "finishedAt": "2026-11-23T10:00:04.200Z",
                    "perf": { "elapsedMs": 4200, "tokensPerSec": 38.6, "completionTokens": 162 },
                    "attachments": [{ "path": "uploads/sample.png", "bytes": 12345 }],
                    "goal": "上辺のUI重なりと記号のセンタリングをテストして確認する",
                    "result": {
                        "review": {
                            "final_answer": "### テスト確認\n- アイコン中央揃えを確認\n- 上部ヘッダーをクリップ済み\n\n```rust\nlet ok = true;\n```"
                        }
                    }
                }
            ]
        });
        let notice = if empty_welcome_with_notice {
            Some(json!({
                "label": "API への接続に失敗しました: https://localhost:8443（対象のコンピューターによって拒否されたため、接続できませんでした。(os error 10061)）。接続先とサーバーの状態を確認してください。",
                "level": "error"
            }))
        } else {
            None
        };
        let mut state = UiState {
            controller,
            rx_slot: Arc::new(Mutex::new(Some(rx))),
            window_id: None,
            hwnd: 0,
            #[cfg(windows)]
            hotkey_tid: Arc::new(AtomicU32::new(0)),
            connected: !empty_welcome_with_notice,
            connecting: false,
            busy: false,
            standby: false,
            elevated: false,
            computer_name: "TEST-PC".into(),
            api_url: "https://127.0.0.1:8443".into(),
            loading_history: false,
            loading_conversation: false,
            loading_preview: false,
            gui,
            server: json!({ "contextWindow": 16384, "maxTokens": 4096 }),
            capabilities: json!({}),
            perf: json!({ "tokensPerSec": 42.5, "gpuMemUsedMb": 14200, "gpuMemTotalMb": 32768 }),
            autotune: json!({}),
            conversations: if empty_welcome_with_notice { Vec::new() } else { convs },
            conversation: if empty_welcome_with_notice { None } else { Some(conv.clone()) },
            read_only: false,
            attachments: Vec::new(),
            attachment_thumbs: HashMap::new(),
            upload_stages: Map::new(),
            pc_windows: Vec::new(),
            selected_window_id: 0,
            selected_window_title: String::new(),
            local_workdir: "C:\\Qwen27Work".into(),
            read_roots: Vec::new(),
            workers_panels: Vec::new(),
            files_items: Vec::new(),
            files_loading: false,
            sharing_entries: Vec::new(),
            logs: Vec::new(),
            notice,
            status_main: if empty_welcome_with_notice {
                "未接続".into()
            } else {
                "サーバー接続中 · とても長いステータスメッセージが右端の検索ボタンに重ならないこと".into()
            },
            status_tail: if empty_welcome_with_notice { String::new() } else { "42.5 tok/s · 最新".into() },
            status_stale: false,
            sidebar_open: true,
            active_pane: if drawer_open { Some(DrawerPane::Settings) } else { None },
            displayed_pane: if drawer_open { Some(DrawerPane::Settings) } else { None },
            tuning_open: drawer_open,
            history_filter: HistoryFilter::All,
            history_query: String::new(),
            window_filter: String::new(),
            log_filter: String::new(),
            find_open,
            find_query: if find_open { "テスト".into() } else { String::new() },
            find_match_idx: 0,
            task_strip_collapsed: false,
            active_modal: None,
            displayed_modal: None,
            palette_query: String::new(),
            palette_idx: 0,
            slash_idx: 0,
            settings_draft: SettingsDraft::default(),
            composer_content: text_editor::Content::new(),
            copied_key: None,
            scroll_id: Id::new("chat-scroll-test"),
            at_bottom: true,
            last_scroll_abs_y: 0.0,
            last_scroll_content_h: 0.0,
            last_scroll_viewport_h: 0.0,
            pending_scroll_bottom: false,
            hover_drop_count: 0,
            parsed_answers: HashMap::new(),
            decoded_activity_imgs: HashMap::new(),
            sent_attach_thumbs: HashMap::new(),
            selection_scope: SelectionScope::default(),
            anim,
            pending_reveal_frames: 0,
        };
        if !empty_welcome_with_notice {
            state.cache_conversation_assets(&conv);
            let mut png = Vec::from(&[137u8, 80, 78, 71, 13, 10, 26, 10][..]);
            let mut ihdr = Vec::with_capacity(13);
            ihdr.extend_from_slice(&16u32.to_be_bytes());
            ihdr.extend_from_slice(&16u32.to_be_bytes());
            ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
            push_png_chunk(&mut png, b"IHDR", &ihdr);
            let mut raw = Vec::with_capacity(16 * (1 + 16 * 4));
            for y in 0..16u8 {
                raw.push(0);
                for x in 0..16u8 {
                    raw.extend_from_slice(&[88 + x * 8, 101 + y * 6, 242, 255]);
                }
            }
            let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
            z.write_all(&raw).unwrap();
            push_png_chunk(&mut png, b"IDAT", &z.finish().unwrap());
            push_png_chunk(&mut png, b"IEND", &[]);
            let h = img_w::Handle::from_bytes(png);
            state.sent_attach_thumbs.insert("uploads/sample.png".into(), h.clone());
            state.sent_attach_thumbs.insert("conv-1:uploads/sample.png".into(), h);
        }
        state
    }

    fn png_crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg() & 0xedb8_8320;
                crc = (crc >> 1) ^ mask;
            }
        }
        !crc
    }

    fn push_png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) {
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(tag);
        out.extend_from_slice(payload);
        let mut chk = Vec::with_capacity(4 + payload.len());
        chk.extend_from_slice(tag);
        chk.extend_from_slice(payload);
        out.extend_from_slice(&png_crc32(&chk).to_be_bytes());
    }

    fn save_rgba_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
        let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
        let row_bytes = w as usize * 4;
        for y in 0..h as usize {
            raw.push(0);
            raw.extend_from_slice(&rgba[y * row_bytes..(y + 1) * row_bytes]);
        }
        let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
        z.write_all(&raw).unwrap();
        let compressed = z.finish().unwrap();

        let mut png = Vec::from(&[137u8, 80, 78, 71, 13, 10, 26, 10][..]);
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        push_png_chunk(&mut png, b"IHDR", &ihdr);
        push_png_chunk(&mut png, b"IDAT", &compressed);
        push_png_chunk(&mut png, b"IEND", &[]);
        let _ = std::fs::write(path, png);
    }

    #[test]
    fn headless_layout_and_startup_timing() {
        #[cfg(windows)]
        if std::env::var_os("WGPU_BACKEND").is_none() {
            let preferred = if std::path::Path::new(r"C:\Windows\System32\vulkan-1.dll").exists() {
                "vulkan"
            } else {
                "dx12"
            };
            std::env::set_var("WGPU_BACKEND", preferred);
        }
        if std::env::var_os("WGPU_POWER_PREFERENCE").is_none() {
            std::env::set_var("WGPU_POWER_PREFERENCE", "low");
        }
        let t_font = Instant::now();
        let _ = iced::advanced::graphics::text::font_system();
        eprintln!("TIMING font_system: {} ms", t_font.elapsed().as_millis());

        let dump_dir = std::env::var_os("QWEN27_DUMP_SHOTS_DIR").map(std::path::PathBuf::from);
        let bench_wgpu = std::env::var_os("QWEN27_BENCH_WGPU").is_some();

        let t_init = Instant::now();
        let (mut renderer, backend_used) = if dump_dir.is_some() || bench_wgpu {
            if let Some(r) = iced::futures::executor::block_on(iced::Renderer::new(
                UI_FONT,
                iced::Pixels(14.0),
                Some("wgpu"),
            )) {
                (r, "wgpu")
            } else {
                (
                    iced::futures::executor::block_on(iced::Renderer::new(
                        UI_FONT,
                        iced::Pixels(14.0),
                        Some("tiny-skia"),
                    ))
                    .expect("tiny-skia fallback"),
                    "tiny-skia",
                )
            }
        } else {
            (
                iced::futures::executor::block_on(iced::Renderer::new(
                    UI_FONT,
                    iced::Pixels(14.0),
                    Some("tiny-skia"),
                ))
                .expect("tiny-skia headless renderer"),
                "tiny-skia",
            )
        };
        eprintln!("TIMING renderer({}): {} ms", backend_used, t_init.elapsed().as_millis());

        let cases = [
            ("light_welcome_notice_1320.png", 1320u32, 860u32, false, false, false, true),
            ("light_1320.png", 1320u32, 860u32, false, false, false, false),
            ("dark_drawer_1320.png", 1320u32, 860u32, true, true, true, false),
            ("light_narrow_1000.png", 1000u32, 660u32, false, false, false, false),
        ];
        for (idx, (name, w, h, dark, drawer_open, find_open, empty_welcome)) in
            cases.into_iter().enumerate()
        {
            let t_frame = Instant::now();
            let mut state = make_mock_state(dark, drawer_open, find_open, empty_welcome);
            if idx == 1 {
                state.anim.push_toast("ok", "設定を保存しました");
                state.anim.toasts[0].age_ms = 500.0;
            }
            let pal = theme::palette(dark);
            let theme = state.theme();
            let mut elem = state.view();
            let limits = Limits::new(Size::ZERO, Size::new(w as f32, h as f32));
            let mut tree = Tree::new(&elem);
            let node = elem.as_widget_mut().layout(&mut tree, &renderer, &limits);
            if idx == 1 {
                assert_eq!(node.children().len(), 2, "expected base shell + toast overlay");
                let toast_layer = &node.children()[1];
                let toast_col = toast_layer.children().first().expect("toast_col node");
                let th = toast_col.size().height;
                assert!(
                    th > 20.0 && th < 100.0,
                    "toast column height must be compact, got {}",
                    th
                );
            }
            for _ in 0..3 {
                renderer.reset(iced::Rectangle::with_size(Size::new(w as f32, h as f32)));
                elem.as_widget().draw(
                    &tree,
                    &mut renderer,
                    &theme,
                    &renderer::Style { text_color: pal.ink },
                    Layout::new(&node),
                    iced::mouse::Cursor::Unavailable,
                    &iced::Rectangle::with_size(Size::new(w as f32, h as f32)),
                );
            }
            let rgba = renderer.screenshot(Size::new(w, h), 1.0, pal.bg_canvas);
            if idx == 0 {
                eprintln!(
                    "TIMING {} 3frames_{}x{}: {} ms",
                    backend_used,
                    w,
                    h,
                    t_frame.elapsed().as_millis()
                );
            }
            assert_eq!(rgba.len(), (w * h * 4) as usize);
            if let Some(ref dir) = dump_dir {
                let _ = std::fs::create_dir_all(dir);
                save_rgba_png(&dir.join(name), w, h, &rgba);
            }
        }
    }

    #[test]
    fn eight_b_controls_are_disabled() {
        let mut state = make_mock_state(false, false, false, false);
        state.toggle_pane(DrawerPane::Workers);
        assert_eq!(state.active_pane, None);

        let p_hits = palette_items("8B");
        let w_pos = p_hits
            .iter()
            .position(|it| it.kind == "pane:workers")
            .expect("palette should list 8B pane item");
        assert!(p_hits[w_pos].disabled);
        state.palette_query = "8B".into();
        state.execute_palette_action(w_pos);
        assert_eq!(state.active_pane, None);

        let s_hits = filtered_slash_items("/workers");
        assert_eq!(s_hits.len(), 1);
        assert_eq!(s_hits[0].label, "/workers");
        assert!(s_hits[0].disabled);
        state.composer_content = text_editor::Content::with_text("/workers");
        state.apply_slash_item(0);
        assert_eq!(state.active_pane, None);

        let all_slash = filtered_slash_items("/");
        for cmd in [
            "/plan", "/goal", "/compact", "/grill-me", "/boost", "/teamwork", "/learn",
            "/browser", "/schedule", "/mode", "/effort", "/status", "/clear", "/help",
        ] {
            assert!(
                all_slash.iter().any(|it| it.label.starts_with(cmd)),
                "missing slash command {}",
                cmd
            );
        }
        assert!(filtered_slash_items("/plan my task").is_empty());

        let pal_all = palette_items("");
        assert!(pal_all.iter().any(|it| it.kind == "slash:/compact"));
        assert!(pal_all.iter().any(|it| it.kind == "restart_server"));
    }

    #[test]
    fn scroll_does_not_snap_when_scrolled_up_and_optimistic_job_visible() {
        let mut state = make_mock_state(false, false, false, true);
        assert!(state.conversation.is_none());
        assert!(state.at_bottom);

        let _ = state.apply_backend_event(json!({
            "type": "job_update",
            "liveText": "依頼を送信しています…",
            "job": {
                "id": "pending-1",
                "conversationId": "",
                "status": "queued",
                "goal": "テスト送信メッセージ",
                "activity": [],
                "_optimistic": true
            }
        }));
        let conv = state.conversation.as_ref().expect("optimistic job must create conversation");
        assert_eq!(conv["jobs"].as_array().unwrap().len(), 1);
        assert_eq!(conv["jobs"][0]["goal"].as_str(), Some("テスト送信メッセージ"));

        let _ = state.apply_backend_event(json!({
            "type": "job_update",
            "replaceJobId": "pending-1",
            "job": {
                "id": "real-1",
                "conversationId": "conv-new",
                "title": "新しいテスト",
                "status": "running",
                "goal": "テスト送信メッセージ",
                "activity": []
            }
        }));
        let conv = state.conversation.as_ref().unwrap();
        assert_eq!(conv["id"].as_str(), Some("conv-new"));
        assert_eq!(conv["jobs"].as_array().unwrap().len(), 1);
        assert_eq!(conv["jobs"][0]["id"].as_str(), Some("real-1"));

        let _ = state.update(Message::ChatScrolled {
            abs_y: 520.0,
            dist_from_bottom: 0.0,
            content_h: 1200.0,
            viewport_h: 680.0,
        });
        assert!(state.at_bottom);

        // Content growth while at bottom (before snap_to_end settles) keeps auto-scroll active.
        let _ = state.update(Message::ChatScrolled {
            abs_y: 520.0,
            dist_from_bottom: 700.0,
            content_h: 1900.0,
            viewport_h: 680.0,
        });
        assert!(state.at_bottom, "content growth without scrolling up must keep at_bottom");

        // Snap settles at new bottom, then user manually scrolls upward -> disables auto-scroll.
        let _ = state.update(Message::ChatScrolled {
            abs_y: 1220.0,
            dist_from_bottom: 0.0,
            content_h: 1900.0,
            viewport_h: 680.0,
        });
        let _ = state.update(Message::ChatScrolled {
            abs_y: 1070.0,
            dist_from_bottom: 150.0,
            content_h: 1900.0,
            viewport_h: 680.0,
        });
        assert!(!state.at_bottom, "manual upward scroll must disable auto-scroll");

        // Subsequent completion growth + conversation reload must NOT snap user back down.
        let _ = state.apply_backend_event(json!({
            "type": "job_update",
            "job": {
                "id": "real-1",
                "conversationId": "conv-new",
                "status": "completed",
                "goal": "テスト送信メッセージ",
                "result": { "review": { "final_answer": "完了しました。" } }
            }
        }));
        let _ = state.update(Message::ChatScrolled {
            abs_y: 1070.0,
            dist_from_bottom: 500.0,
            content_h: 2250.0,
            viewport_h: 680.0,
        });
        assert!(!state.at_bottom);

        let _ = state.apply_backend_event(json!({
            "type": "conversation",
            "conversation": {
                "id": "conv-new",
                "title": "新しいテスト",
                "jobs": [{ "id": "real-1", "status": "completed", "goal": "テスト送信メッセージ" }]
            }
        }));
        assert!(!state.at_bottom, "same conversation reload must keep scrolled-up position");

        let _ = state.update(Message::ChatScrolled {
            abs_y: 1560.0,
            dist_from_bottom: 10.0,
            content_h: 2250.0,
            viewport_h: 680.0,
        });
        assert!(state.at_bottom);
        let _ = state.update(Message::ChatScrolled {
            abs_y: 1400.0,
            dist_from_bottom: 170.0,
            content_h: 2250.0,
            viewport_h: 680.0,
        });
        assert!(!state.at_bottom);
        let _ = state.update(Message::ChatScrolled {
            abs_y: 0.0,
            dist_from_bottom: f32::NAN,
            content_h: 400.0,
            viewport_h: 680.0,
        });
        assert!(state.at_bottom);

        let _ = state.apply_backend_event(json!({
            "type": "conversation",
            "conversation": { "id": "conv-other", "title": "別チャット", "jobs": [] }
        }));
        assert!(state.at_bottom, "switching conversation must reset scroll to bottom");

        // First message in empty chat expands beyond viewport height -> stays auto-scrolling.
        let _ = state.apply_backend_event(json!({
            "type": "job_update",
            "job": {
                "id": "real-2",
                "conversationId": "conv-other",
                "status": "completed",
                "goal": "初回送信",
                "result": { "review": { "final_answer": "初回回答" } }
            }
        }));
        let _ = state.update(Message::ChatScrolled {
            abs_y: 0.0,
            dist_from_bottom: 900.0,
            content_h: 1580.0,
            viewport_h: 680.0,
        });
        assert!(state.at_bottom, "first-message height expansion must keep at_bottom");

        state.conversation = None;
        let _ = state.apply_backend_event(json!({
            "type": "job_update",
            "job": { "id": "pending-err", "status": "queued", "goal": "x", "_optimistic": true }
        }));
        assert!(state.conversation.is_some());
        let _ = state.apply_backend_event(json!({ "type": "job_removed", "jobId": "pending-err" }));
        assert!(state.conversation.is_none());
    }

    #[test]
    fn scroll_survives_job_completion_conversation_reload_and_loading_bar_hide() {
        use iced::advanced::widget::operation::{self as adv_op, Operation};

        struct ScrollProbe {
            target: Id,
            action: Option<ScrollProbeAction>,
            found: bool,
            bounds_h: f32,
            content_h: f32,
            translation_y: f32,
        }
        enum ScrollProbeAction {
            SnapEnd,
            ScrollToY(f32),
        }
        impl Operation for ScrollProbe {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn scrollable(
                &mut self,
                id: Option<&Id>,
                bounds: iced::Rectangle,
                content_bounds: iced::Rectangle,
                translation: iced::Vector,
                state: &mut dyn adv_op::Scrollable,
            ) {
                if id == Some(&self.target) {
                    self.found = true;
                    self.bounds_h = bounds.height;
                    self.content_h = content_bounds.height;
                    self.translation_y = translation.y;
                    match self.action {
                        Some(ScrollProbeAction::SnapEnd) => {
                            state.snap_to(adv_op::scrollable::RelativeOffset::END.into());
                        }
                        Some(ScrollProbeAction::ScrollToY(y)) => {
                            state.scroll_to(adv_op::scrollable::AbsoluteOffset {
                                x: None,
                                y: Some(y),
                            });
                        }
                        None => {}
                    }
                }
            }
        }

        let renderer = iced::futures::executor::block_on(iced::Renderer::new(
            UI_FONT,
            iced::Pixels(14.0),
            Some("tiny-skia"),
        ))
        .expect("tiny-skia headless renderer");
        let limits = Limits::new(Size::ZERO, Size::new(1200.0, 720.0));

        let long_body: String = (1..=45)
            .map(|i| format!("段落 {}: これはスクロール位置の維持を検証するための十分な長さを持つ本文テキストです。", i))
            .collect::<Vec<_>>()
            .join("\n\n");
        let running_job = json!({
            "id": "job-scroll-1",
            "conversationId": "conv-scroll-1",
            "status": "running",
            "goal": "長い回答を生成するテスト",
            "activity": [
                { "tool": "thinking", "status": "running", "text": "手順を整理しています。\n".repeat(12) },
                { "tool": "local.list", "status": "completed", "summary": "一覧確認" }
            ],
            "partialAnswer": long_body,
        });
        let completed_job = json!({
            "id": "job-scroll-1",
            "conversationId": "conv-scroll-1",
            "status": "completed",
            "goal": "長い回答を生成するテスト",
            "activity": [
                { "tool": "thinking", "status": "completed", "text": "手順を整理しました。" },
                { "tool": "local.list", "status": "completed", "summary": "一覧確認" }
            ],
            "result": { "review": { "final_answer": format!("{}\n\n最終行: 完了しました。", long_body) } },
            "_resultCard": { "line": "完了", "badge": "OK" }
        });
        let reloaded_conv = json!({
            "id": "conv-scroll-1",
            "title": "スクロール検証",
            "jobs": [completed_job.clone()]
        });

        let step_layout = |state: &UiState,
                           tree: &mut Option<Tree>,
                           action: Option<ScrollProbeAction>|
         -> ScrollProbe {
            let mut el = state.view();
            let t = match tree {
                Some(existing) => {
                    existing.diff(&el);
                    existing
                }
                None => tree.insert(Tree::new(&el)),
            };
            let node = el.as_widget_mut().layout(t, &renderer, &limits);
            let mut probe = ScrollProbe {
                target: state.scroll_id.clone(),
                action,
                found: false,
                bounds_h: 0.0,
                content_h: 0.0,
                translation_y: 0.0,
            };
            el.as_widget_mut()
                .operate(t, Layout::new(&node), &renderer, &mut probe);
            assert!(probe.found, "chat scrollable must remain mounted");
            probe
        };

        // Case A: User is at bottom -> stays at bottom through job completion, conversation
        // refresh, loading_bar hiding to 0.0, sidebar toggle, and find bar open.
        let mut st_bottom = make_mock_state(true, false, false, false);
        let init_conv = json!({
            "id": "conv-scroll-1",
            "title": "スクロール検証",
            "jobs": [running_job.clone()]
        });
        st_bottom.conversation = Some(init_conv.clone());
        st_bottom.cache_conversation_assets(&init_conv);
        st_bottom.busy = true;
        st_bottom.anim.loading_bar = crate::ui::anim::EaseAnim::new(1.0);
        let mut tree_a: Option<Tree> = None;
        let _ = step_layout(&st_bottom, &mut tree_a, Some(ScrollProbeAction::SnapEnd));
        let p_run = step_layout(&st_bottom, &mut tree_a, None);
        let max_run = (p_run.content_h - p_run.bounds_h).max(0.0);
        assert!(max_run > 300.0, "test content must overflow viewport, max={}", max_run);
        assert!((p_run.translation_y - max_run).abs() <= 2.0);
        let _ = st_bottom.update(Message::ChatScrolled {
            abs_y: p_run.translation_y,
            dist_from_bottom: 0.0,
            content_h: p_run.content_h,
            viewport_h: p_run.bounds_h,
        });
        assert!(st_bottom.at_bottom);

        // Job completes, busy turns false, trailing conversation refresh arrives, and
        // loading_bar fades all the way to 0.0 (previously dropped child[1] and reset y -> 0).
        let _ = st_bottom.apply_backend_event(json!({
            "type": "job_update",
            "job": completed_job.clone(),
            "liveText": "完了"
        }));
        assert!(st_bottom.at_bottom);
        let _ = step_layout(&st_bottom, &mut tree_a, Some(ScrollProbeAction::SnapEnd));
        let _ = st_bottom.apply_backend_event(json!({
            "type": "connection",
            "connected": true,
            "busy": false,
            "connecting": false,
            "standby": false
        }));
        let _ = st_bottom.apply_backend_event(json!({
            "type": "conversation",
            "conversation": reloaded_conv.clone()
        }));
        assert!(st_bottom.at_bottom);
        let _ = step_layout(&st_bottom, &mut tree_a, Some(ScrollProbeAction::SnapEnd));

        // Multi-frame ChatScrolled before snap settles (e.g. viewport changes by 1px while
        // loading bar shrinks) must keep at_bottom == true.
        let _ = st_bottom.update(Message::ChatScrolled {
            abs_y: p_run.translation_y,
            dist_from_bottom: 180.0,
            content_h: p_run.content_h + 180.0,
            viewport_h: p_run.bounds_h + 1.0,
        });
        assert!(st_bottom.at_bottom);
        assert!(st_bottom.pending_scroll_bottom);
        let _ = st_bottom.update(Message::ChatScrolled {
            abs_y: p_run.translation_y,
            dist_from_bottom: 179.0,
            content_h: p_run.content_h + 180.0,
            viewport_h: p_run.bounds_h + 2.0,
        });
        assert!(st_bottom.at_bottom, "pending snap must survive consecutive layout frames");

        // Fade loading_bar completely to 0.0 without re-snapping: Scrollable tree MUST stay intact!
        st_bottom.anim.loading_bar = crate::ui::anim::EaseAnim::new(0.0);
        let p_after_bar = step_layout(&st_bottom, &mut tree_a, None);
        let max_after = (p_after_bar.content_h - p_after_bar.bounds_h).max(0.0);
        assert!(max_after > 300.0);
        assert!(
            (p_after_bar.translation_y - max_after).abs() <= 2.0,
            "hiding top_progress_bar must NOT reset scroll to top (got y={}, max={})",
            p_after_bar.translation_y,
            max_after
        );

        // Collapsing sidebar and opening find bar (fh == 0.0 on frame 0) must also preserve scroll.
        st_bottom.sidebar_open = false;
        st_bottom.anim.sidebar = crate::ui::anim::EaseAnim::new(0.0);
        let p_after_sidebar = step_layout(&st_bottom, &mut tree_a, None);
        assert!(
            p_after_sidebar.translation_y > 200.0,
            "collapsing sidebar must NOT reset scroll to top (got y={})",
            p_after_sidebar.translation_y
        );
        let _ = st_bottom.update(Message::ToggleFind);
        let p_after_find0 = step_layout(&st_bottom, &mut tree_a, None);
        assert!(
            p_after_find0.translation_y > 200.0,
            "opening find bar (frame 0) must NOT reset scroll to top (got y={})",
            p_after_find0.translation_y
        );

        // Case B: User scrolled up while job is running -> job completion, trailing conversation
        // reload, and loading_bar fade-out must preserve exact scrolled-up y (never reset to 0 or snap down).
        let mut st_up = make_mock_state(true, false, false, false);
        st_up.conversation = Some(init_conv.clone());
        st_up.cache_conversation_assets(&init_conv);
        st_up.busy = true;
        st_up.anim.loading_bar = crate::ui::anim::EaseAnim::new(1.0);
        let mut tree_b: Option<Tree> = None;
        let p0 = step_layout(&st_up, &mut tree_b, Some(ScrollProbeAction::SnapEnd));
        let _ = st_up.update(Message::ChatScrolled {
            abs_y: (p0.content_h - p0.bounds_h).max(0.0),
            dist_from_bottom: 0.0,
            content_h: p0.content_h,
            viewport_h: p0.bounds_h,
        });
        let desired_y = 220.0f32;
        let _ = step_layout(&st_up, &mut tree_b, Some(ScrollProbeAction::ScrollToY(desired_y)));
        let p_up = step_layout(&st_up, &mut tree_b, None);
        assert!((p_up.translation_y - desired_y).abs() <= 1.5);
        let _ = st_up.update(Message::ChatScrolled {
            abs_y: desired_y,
            dist_from_bottom: (p_up.content_h - p_up.bounds_h - desired_y).max(0.0),
            content_h: p_up.content_h,
            viewport_h: p_up.bounds_h,
        });
        assert!(!st_up.at_bottom, "scrolled-up user must have at_bottom=false");

        let _ = st_up.apply_backend_event(json!({
            "type": "job_update",
            "job": completed_job,
            "liveText": "完了"
        }));
        assert!(!st_up.at_bottom);
        assert!(!st_up.pending_scroll_bottom);
        let p_done = step_layout(&st_up, &mut tree_b, None);
        assert!(
            (p_done.translation_y - desired_y).abs() <= 1.5,
            "job completion must preserve scrolled-up y={} (got {})",
            desired_y,
            p_done.translation_y
        );

        let _ = st_up.apply_backend_event(json!({
            "type": "connection",
            "connected": true,
            "busy": false,
            "connecting": false,
            "standby": false
        }));
        let _ = st_up.apply_backend_event(json!({
            "type": "conversation",
            "conversation": reloaded_conv
        }));
        assert!(!st_up.at_bottom);
        st_up.anim.loading_bar = crate::ui::anim::EaseAnim::new(0.0);
        let p_final = step_layout(&st_up, &mut tree_b, None);
        assert!(
            (p_final.translation_y - desired_y).abs() <= 1.5,
            "conversation reload + hiding loading_bar must preserve scrolled-up y={} (got {})",
            desired_y,
            p_final.translation_y
        );
    }
}
