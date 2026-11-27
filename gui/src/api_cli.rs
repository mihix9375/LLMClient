use crate::attachments::build_attachment_from_file;
use crate::cli::term::ConsoleSession;
use crate::config::{
    config_root_dir, diag_install, diag_line, find_window_match, infer_preset_index,
    load_gui_settings_file, parse_cli_args, preset_index_from_name, preset_name_from_index,
    preset_sampling, resolve_api_candidates, safe_artifact, sampling_from_json, sampling_to_json,
    save_gui_settings_file, strip_unc_prefix, valid_id, validate_context_and_tokens,
    validate_sampling_values, validate_workdir, CliArgs, SamplingValues,
};
use crate::desktop_worker::DesktopWorkerProcess;
use crate::tls_http::ApiClient;
use crate::win_util::{computer_name, desktop_windows, no_links, write_file_atomic};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

static STANDBY_STOP: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
unsafe extern "system" fn standby_ctrl_handler(ctrl_type: u32) -> windows::core::BOOL {
    use windows::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT};
    if ctrl_type == CTRL_C_EVENT || ctrl_type == CTRL_BREAK_EVENT || ctrl_type == CTRL_CLOSE_EVENT {
        STANDBY_STOP.store(true, Ordering::SeqCst);
        true.into()
    } else {
        false.into()
    }
}

#[cfg(unix)]
extern "C" fn unix_standby_handler(_: libc::c_int) {
    STANDBY_STOP.store(true, Ordering::SeqCst);
}

fn install_standby_ctrl() {
    #[cfg(windows)]
    unsafe {
        let _ = windows::Win32::System::Console::SetConsoleCtrlHandler(Some(standby_ctrl_handler), true);
    }
    #[cfg(unix)]
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = unix_standby_handler as *const () as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
        libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
    }
}

fn remove_standby_ctrl() {
    #[cfg(windows)]
    unsafe {
        let _ = windows::Win32::System::Console::SetConsoleCtrlHandler(Some(standby_ctrl_handler), false);
    }
    #[cfg(unix)]
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = libc::SIG_DFL;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
        libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
    }
}

fn emit(v: &Value) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{}", v);
    let _ = out.flush();
}

pub fn save_job_outputs(job: &Value, downloads_dir: &Path) -> Result<PathBuf, String> {
    let id = job
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| valid_id(s))
        .ok_or_else(|| "Invalid job ID".to_string())?;
    let root = strip_unc_prefix(if downloads_dir.is_absolute() {
        downloads_dir.join(id)
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(downloads_dir)
            .join(id)
    });
    no_links(&root)?;
    let artifacts = job
        .get("result")
        .and_then(|r| r.get("artifacts"))
        .and_then(|a| a.as_array())
        .cloned()
        .unwrap_or_default();
    for file in &artifacts {
        let p = file
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Artifact missing path".to_string())?;
        if !safe_artifact(p) {
            return Err("Unsafe artifact filename".into());
        }
    }
    let job_pretty = serde_json::to_string_pretty(job).map_err(|e| e.to_string())?;
    write_file_atomic(&root.join("job.json"), job_pretty.as_bytes())?;
    for file in &artifacts {
        let rel = file.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let content = file.get("content").and_then(|v| v.as_str()).unwrap_or("");
        write_file_atomic(&root.join("artifacts").join(rel), content.as_bytes())?;
    }
    if let Some(final_ans) = job
        .get("result")
        .and_then(|r| r.get("review"))
        .and_then(|rv| rv.get("final_answer"))
        .and_then(|v| v.as_str())
    {
        write_file_atomic(&root.join("final.md"), final_ans.as_bytes())?;
    }
    Ok(root)
}

pub fn resolve_api_url(root: &Path, explicit_url: &str) -> String {
    if !explicit_url.trim().is_empty() {
        return explicit_url.trim().trim_end_matches('/').to_string();
    }
    if let Ok(env_u) = std::env::var("QWEN_API_URL") {
        if !env_u.trim().is_empty() {
            return env_u.trim().trim_end_matches('/').to_string();
        }
    }
    resolve_api_candidates(root, "")
        .into_iter()
        .next()
        .unwrap_or_else(|| "https://localhost:8443".to_string())
}

pub fn connect_api_with_failover(root: &Path, explicit_url: &str) -> Result<ApiClient, String> {
    if !explicit_url.trim().is_empty() {
        return ApiClient::new(explicit_url.trim());
    }
    if let Ok(env_u) = std::env::var("QWEN_API_URL") {
        if !env_u.trim().is_empty() {
            return ApiClient::new(env_u.trim());
        }
    }
    let candidates = resolve_api_candidates(root, "");
    let mut errs: Vec<String> = Vec::new();
    for url in &candidates {
        match ApiClient::new(url).and_then(|c| c.get("/health", 2500).map(|_| c)) {
            Ok(c) => return Ok(c),
            Err(e) => errs.push(format!("{} ({})", url, e)),
        }
    }
    if let Some(first) = candidates.first() {
        if errs.is_empty() {
            return ApiClient::new(first);
        }
        return Err(format!("Framework API へ接続できません: {}", errs.join(" / ")));
    }
    Err("有効な HTTPS API URL が見つかりません".into())
}

fn cli_help_json() -> Value {
    json!({
        "usage": "Qwen27.exe --api <command> [args] [options]",
        "commands": [
            "health", "identity", "commands", "windows", "gui-settings", "settings", "configure",
            "preset", "perf", "models [status|start|stop|restart|perf]", "autotune", "tune-status",
            "tune-apply", "standby", "run", "status", "fetch", "save", "cancel", "answer",
            "interrupt", "conversations", "conversation", "compact", "memory", "schedules",
            "share", "files", "upload", "classify", "profiles", "adapters", "adapter-profiles"
        ],
        "options": "--url, --auto-connect/--no-connect, --work-mode (standard|chat|coding|pc_control), --role (planner|worker), --plan/--no-plan, --context, --tokens, --effort (none|low|medium|xhigh), --budget, --preset (code|balanced|creative), --temp, --top-p, --top-k, --min-p, --repeat-penalty, --presence-penalty, --frequency-penalty, --workspace/--no-workspace, --local-cli/--no-local-cli, --run-on-pc/--server-windows, --workdir, --read-dir, --clear-read-dirs, --window, --pc/--no-pc, --standby/--no-standby, --shared-desktop, --attach, --conv, --parent, --file, --output, --local-only, --no-sync-gui",
    })
}

pub fn build_cli_desktop_tools(allow_desktop: bool, allow_local: bool) -> Value {
    let mut tools = Vec::new();
    if allow_desktop {
        for t in [
            "desktop.observe",
            "desktop.screenshot",
            "desktop.video",
            "desktop.click",
            "desktop.click_at",
            "desktop.drag",
            "desktop.type",
            "desktop.key",
            "desktop.hold_key",
            "desktop.scroll",
            "desktop.sequence",
            "desktop.sustain",
            "desktop.release",
        ] {
            tools.push(Value::String(t.to_string()));
        }
    }
    if allow_local {
        for t in ["local.list", "local.read", "local.write", "local.run"] {
            tools.push(Value::String(t.to_string()));
        }
    }
    Value::Array(tools)
}

pub fn resolve_cli_read_roots(
    gui: &Value,
    extra_dirs: &[String],
    clear_old: bool,
) -> Result<Vec<PathBuf>, String> {
    let mut roots = Vec::new();
    if !clear_old {
        if let Some(arr) = gui.get("readOnlyFolders").and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(s) = item.as_str() {
                    let p = PathBuf::from(s);
                    if p.is_dir() {
                        if let Ok(canon) = fs::canonicalize(&p) {
                            roots.push(strip_unc_prefix(canon));
                        }
                    }
                }
            }
        }
    }
    for d in extra_dirs {
        let canon = validate_workdir(d)?;
        if !roots.contains(&canon) {
            roots.push(canon);
        }
    }
    Ok(roots)
}

pub fn resolve_cli_workspace(
    root: &Path,
    custom_dir: &str,
    fallback_name: &str,
) -> Result<PathBuf, String> {
    let ws = if custom_dir.is_empty() {
        root.join("local-workspaces").join(fallback_name)
    } else {
        validate_workdir(custom_dir)?
    };
    no_links(&ws)?;
    fs::create_dir_all(&ws).map_err(|e| format!("作業フォルダーを作成できません: {}", e))?;
    Ok(ws)
}

pub(crate) struct CliDesktopBridge {
    api_url: String,
    pub(crate) session_id: String,
    token: String,
    stop: Arc<AtomicBool>,
    poll_api: Option<Arc<ApiClient>>,
    worker: Option<JoinHandle<()>>,
}

impl CliDesktopBridge {
    fn new() -> Self {
        Self {
            api_url: String::new(),
            session_id: String::new(),
            token: String::new(),
            stop: Arc::new(AtomicBool::new(false)),
            poll_api: None,
            worker: None,
        }
    }

    fn open_session(
        &mut self,
        api_url: &str,
        win_title: &str,
        allow_desktop: bool,
        allow_local: bool,
    ) -> Result<String, String> {
        self.api_url = api_url.to_string();
        let setup = ApiClient::new(api_url)?;
        let mut body = json!({ "tools": build_cli_desktop_tools(allow_desktop, allow_local) });
        if !win_title.is_empty() {
            body["windowTitle"] = json!(win_title);
        }
        let session = setup.post("/desktop/sessions", &body, 15000)?;
        self.session_id = session
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("session id がありません")?
            .to_string();
        self.token = session
            .get("token")
            .and_then(|v| v.as_str())
            .ok_or("session token がありません")?
            .to_string();
        Ok(self.session_id.clone())
    }

    fn start_loop(
        &mut self,
        hwnd: isize,
        workspace: PathBuf,
        read_roots: Vec<PathBuf>,
        allow_desktop: bool,
        allow_local: bool,
    ) -> Result<(), String> {
        if self.session_id.is_empty() {
            return Ok(());
        }
        self.stop.store(false, Ordering::SeqCst);
        let poll_api = Arc::new(ApiClient::new(&self.api_url)?);
        self.poll_api = Some(Arc::clone(&poll_api));
        let stop = Arc::clone(&self.stop);
        let sid = self.session_id.clone();
        let token = self.token.clone();

        self.worker = Some(thread::spawn(move || {
            let mut dw = DesktopWorkerProcess::new(hwnd, workspace, read_roots);
            while !stop.load(Ordering::SeqCst) {
                let poll_res = poll_api.post(
                    &format!("/desktop/{}/poll", sid),
                    &json!({ "token": token }),
                    25000,
                );
                let Ok(poll) = poll_res else {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(150));
                    continue;
                };
                if let Some(action) = poll.get("action").filter(|v| !v.is_null()) {
                    let action_id = action.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let tool = action.get("tool").and_then(|v| v.as_str()).unwrap_or("");
                    let res = if (tool.starts_with("local.") && !allow_local)
                        || (tool.starts_with("desktop.") && !allow_desktop)
                    {
                        json!({ "error": "このPC操作は許可されていません" })
                    } else {
                        match dw.perform(
                            action,
                            &stop,
                            |output| {
                                let _ = poll_api.post(
                                    &format!("/desktop/{}/progress", sid),
                                    &json!({ "token": token, "actionId": action_id, "output": output }),
                                    5000,
                                );
                            },
                            || !stop.load(Ordering::SeqCst),
                        ) {
                            Ok(v) => v,
                            Err(e) => json!({ "error": e }),
                        }
                    };
                    if !stop.load(Ordering::SeqCst) {
                        let _ = poll_api.post(
                            &format!("/desktop/{}/result", sid),
                            &json!({ "token": token, "actionId": action_id, "result": res }),
                            10000,
                        );
                    }
                } else {
                    thread::sleep(Duration::from_millis(60));
                }
            }
        }));
        Ok(())
    }

    pub(crate) fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(ref api) = self.poll_api {
            api.cancel_io();
        }
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
        self.poll_api = None;
        if !self.session_id.is_empty() {
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

impl Drop for CliDesktopBridge {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_cli_standby(
    api_url: &str,
    root: &Path,
    hwnd: isize,
    win_title: &str,
    workdir: &str,
    read_roots: Vec<PathBuf>,
    allow_local: bool,
) -> Result<i32, String> {
    STANDBY_STOP.store(false, Ordering::SeqCst);
    install_standby_ctrl();
    let workspace = if allow_local {
        resolve_cli_workspace(root, workdir, "shared")?
    } else {
        PathBuf::new()
    };
    let comp_name = computer_name();
    let api = ApiClient::new(api_url)?;
    let session = api.post(
        "/desktop/sessions",
        &json!({ "tools": build_cli_desktop_tools(true, allow_local), "windowTitle": win_title }),
        15000,
    )?;
    let sid = session
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("session id がありません")?
        .to_string();
    let stoken = session
        .get("token")
        .and_then(|v| v.as_str())
        .ok_or("session token がありません")?
        .to_string();
    api.post(
        "/desktop/shared",
        &json!({
            "enabled": true,
            "id": sid,
            "token": stoken,
            "windowTitle": win_title,
            "computerName": comp_name,
        }),
        10000,
    )?;
    emit(&json!({
        "type": "standby",
        "status": "listening",
        "id": sid,
        "windowTitle": win_title,
        "hwnd": hwnd,
        "localCliEnabled": allow_local,
    }));

    let mut dw = DesktopWorkerProcess::new(hwnd, workspace, read_roots);
    while !STANDBY_STOP.load(Ordering::SeqCst) {
        match api.post(
            &format!("/desktop/{}/poll", sid),
            &json!({ "token": stoken }),
            25000,
        ) {
            Ok(poll) => {
                if let Some(action) = poll.get("action").filter(|v| !v.is_null()) {
                    let aid = action.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let res = match dw.perform(
                        action,
                        &STANDBY_STOP,
                        |output| {
                            let _ = api.post(
                                &format!("/desktop/{}/progress", sid),
                                &json!({ "token": stoken, "actionId": aid, "output": output }),
                                5000,
                            );
                        },
                        || !STANDBY_STOP.load(Ordering::SeqCst),
                    ) {
                        Ok(v) => v,
                        Err(e) => json!({ "error": e }),
                    };
                    if !STANDBY_STOP.load(Ordering::SeqCst) {
                        let _ = api.post(
                            &format!("/desktop/{}/result", sid),
                            &json!({ "token": stoken, "actionId": aid, "result": res }),
                            10000,
                        );
                    }
                } else {
                    thread::sleep(Duration::from_millis(80));
                }
            }
            Err(_) => {
                if STANDBY_STOP.load(Ordering::SeqCst) {
                    break;
                }
                thread::sleep(Duration::from_millis(300));
            }
        }
    }

    if let Ok(cleanup) = ApiClient::new(api_url) {
        let _ = cleanup.post(
            "/desktop/shared",
            &json!({ "enabled": false, "id": sid, "token": stoken }),
            2000,
        );
        let _ = cleanup.post(
            &format!("/desktop/{}/close", sid),
            &json!({ "token": stoken }),
            2000,
        );
    }
    remove_standby_ctrl();
    emit(&json!({ "type": "standby", "status": "stopped", "id": sid }));
    Ok(0)
}

fn sync_read_dirs_into_gui_patch(args: &mut CliArgs, current_gui: &Value) -> Result<(), String> {
    if !args.clear_read_dirs && args.read_dirs.is_empty() {
        return Ok(());
    }
    let roots = resolve_cli_read_roots(current_gui, &args.read_dirs, args.clear_read_dirs)?;
    let arr: Vec<Value> = roots
        .into_iter()
        .map(|p| Value::String(p.to_string_lossy().into_owned()))
        .collect();
    args.gui_patch.insert("readOnlyFolders".into(), Value::Array(arr));
    Ok(())
}

pub(crate) fn apply_settings_and_gui(
    api: Option<&ApiClient>,
    root: &Path,
    args: &mut CliArgs,
) -> Result<Value, String> {
    let mut gui = load_gui_settings_file(root);
    sync_read_dirs_into_gui_patch(args, &gui)?;
    if !args.workdir.is_empty() {
        let wd = validate_workdir(&args.workdir)?;
        args.gui_patch
            .insert("localWorkdir".into(), json!(wd.to_string_lossy()));
    }
    if args.command == "preset" {
        if args.positionals.is_empty() {
            return Err("Usage: preset <code|balanced|creative>".into());
        }
        let idx = preset_index_from_name(&args.positionals[0]);
        if !(0..=2).contains(&idx) {
            return Err("preset must be code, balanced, or creative".into());
        }
        let s = preset_sampling(idx);
        let mut srv_val = Value::Object(std::mem::take(&mut args.server_patch));
        sampling_to_json(&s, &mut srv_val);
        if let Value::Object(m) = srv_val {
            args.server_patch = m;
        }
        let mut gui_val = Value::Object(std::mem::take(&mut args.gui_patch));
        sampling_to_json(&s, &mut gui_val);
        gui_val["samplingPreset"] = json!(preset_name_from_index(idx));
        if let Value::Object(m) = gui_val {
            args.gui_patch = m;
        }
    }

    let mut server_result = Value::Null;
    if let (Some(client), false, false) = (api, args.local_only, args.server_patch.is_empty()) {
        let current = client.get("/settings", 15000)?;
        let payload = Value::Object(args.server_patch.clone());
        let ctx = payload
            .get("contextWindow")
            .and_then(|v| v.as_i64())
            .or_else(|| current.get("contextWindow").and_then(|v| v.as_i64()))
            .unwrap_or(8192) as i32;
        let tok = payload
            .get("maxTokens")
            .and_then(|v| v.as_i64())
            .or_else(|| current.get("maxTokens").and_then(|v| v.as_i64()))
            .unwrap_or(1800) as i32;
        validate_context_and_tokens(ctx, tok)?;
        let mut samp = SamplingValues::default();
        sampling_from_json(&current, &mut samp);
        sampling_from_json(&payload, &mut samp);
        validate_sampling_values(&samp)?;
        if !args.gui_patch.contains_key("samplingPreset") {
            args.gui_patch.insert(
                "samplingPreset".into(),
                json!(preset_name_from_index(infer_preset_index(&samp))),
            );
        }
        server_result = client.post("/settings", &payload, 420_000)?;
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
            if let Some(v) = server_result.get(k) {
                args.gui_patch.insert(k.to_string(), v.clone());
            }
        }
    } else if !args.server_patch.is_empty() {
        let mut samp = SamplingValues::default();
        sampling_from_json(&gui, &mut samp);
        let patch_val = Value::Object(args.gui_patch.clone());
        sampling_from_json(&patch_val, &mut samp);
        validate_sampling_values(&samp)?;
        if !args.gui_patch.contains_key("samplingPreset") {
            args.gui_patch.insert(
                "samplingPreset".into(),
                json!(preset_name_from_index(infer_preset_index(&samp))),
            );
        }
        if args.gui_patch.contains_key("contextWindow") || args.gui_patch.contains_key("maxTokens")
        {
            let ctx = args
                .gui_patch
                .get("contextWindow")
                .and_then(|v| v.as_i64())
                .or_else(|| gui.get("contextWindow").and_then(|v| v.as_i64()))
                .unwrap_or(16384) as i32;
            let tok = args
                .gui_patch
                .get("maxTokens")
                .and_then(|v| v.as_i64())
                .or_else(|| gui.get("maxTokens").and_then(|v| v.as_i64()))
                .unwrap_or(1800) as i32;
            validate_context_and_tokens(ctx, tok)?;
        }
    }

    if args.sync_gui && !args.gui_patch.is_empty() {
        gui = save_gui_settings_file(root, &Value::Object(args.gui_patch.clone()))?;
    } else if !args.gui_patch.is_empty() {
        if let Some(obj) = gui.as_object_mut() {
            for (k, v) in &args.gui_patch {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    if let (Some(client), true, false) = (api, server_result.is_null(), args.local_only) {
        if let Ok(s) = client.get("/settings", 10000) {
            server_result = s;
        }
    }
    Ok(json!({ "gui": gui, "server": server_result }))
}

fn run_stdin_command(request: &Value, root: &Path, api: &ApiClient) -> Result<i32, String> {
    let command = request
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or("command が必要です")?;
    let id = request.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let job_path = || -> Result<String, String> {
        if !valid_id(id) {
            return Err("Provide a valid job ID".into());
        }
        Ok(format!("/jobs/{}", id))
    };
    let empty = json!({});

    match command {
        "health" => emit(&json!({ "type": "result", "value": api.get("/health", 15000)? })),
        "commands" => emit(&json!({ "type": "result", "value": api.get("/commands", 15000)? })),
        "identity" => emit(&json!({ "type": "result", "value": api.get("/identity", 15000)? })),
        "windows" => {
            let list: Vec<Value> = desktop_windows()
                .into_iter()
                .map(|w| serde_json::to_value(w).unwrap())
                .collect();
            emit(&json!({ "type": "result", "value": list }));
        }
        "gui_settings" => emit(&json!({ "type": "result", "value": load_gui_settings_file(root) })),
        "gui_configure" => {
            let s = request.get("settings").ok_or("settings が必要です")?;
            emit(&json!({ "type": "result", "value": save_gui_settings_file(root, s)? }));
        }
        "settings" => emit(&json!({ "type": "result", "value": api.get("/settings", 15000)? })),
        "configure" => {
            let next = request.get("settings").ok_or("settings が必要です")?;
            emit(&json!({ "type": "result", "value": api.post("/settings", next, 420_000)? }));
        }
        "profiles" => emit(&json!({ "type": "result", "value": api.get("/model-profiles", 15000)? })),
        "adapters" => emit(&json!({ "type": "result", "value": api.get("/worker/adapters", 15000)? })),
        "adapter_profiles" => emit(&json!({ "type": "result", "value": api.get("/worker/adapter-profiles", 15000)? })),
        "models_status" => emit(&json!({ "type": "result", "value": api.get("/models/status", 15000)? })),
        "models_perf" | "perf" => emit(&json!({ "type": "result", "value": api.get("/models/perf", 15000)? })),
        "models_start" => emit(&json!({ "type": "result", "value": api.post("/models/start", &empty, 120_000)? })),
        "models_stop" => emit(&json!({ "type": "result", "value": api.post("/models/stop", &empty, 60_000)? })),
        "models_restart" => emit(&json!({ "type": "result", "value": api.post("/models/restart", &empty, 120_000)? })),
        "compact" => {
            if !valid_id(id) {
                return Err("Provide a valid conversation ID".into());
            }
            let focus = request.get("focus").and_then(|v| v.as_str()).unwrap_or("");
            let body = if focus.is_empty() { empty.clone() } else { json!({ "focus": focus }) };
            emit(&json!({ "type": "result", "value": api.post(&format!("/conversations/{}/compact", id), &body, 30_000)? }));
        }
        "memory_list" => emit(&json!({ "type": "result", "value": api.get("/memory", 15000)? })),
        "memory_add" => emit(&json!({ "type": "result", "value": api.post("/memory", request, 15000)? })),
        "memory_delete" => {
            if id.is_empty() {
                return Err("Provide memory id".into());
            }
            emit(&json!({ "type": "result", "value": api.delete(&format!("/memory/{}", id), 15000)? }));
        }
        "schedules_list" => emit(&json!({ "type": "result", "value": api.get("/schedules", 15000)? })),
        "schedule_create" => emit(&json!({ "type": "result", "value": api.post("/schedules", request, 15000)? })),
        "schedule_delete" => {
            if id.is_empty() {
                return Err("Provide schedule id".into());
            }
            emit(&json!({ "type": "result", "value": api.delete(&format!("/schedules/{}", id), 15000)? }));
        }
        "tune_status" => emit(&json!({ "type": "result", "value": api.get("/tune/status", 15000)? })),
        "tune_auto" => emit(&json!({ "type": "result", "value": api.post("/tune/auto", request, 120_000)? })),
        "tune_apply" => emit(&json!({ "type": "result", "value": api.post("/tune/apply", request, 120_000)? })),
        "conversations" => emit(&json!({ "type": "result", "value": api.get("/conversations", 15000)? })),
        "conversation" => {
            if !valid_id(id) {
                return Err("Provide a valid conversation ID".into());
            }
            emit(&json!({ "type": "result", "value": api.get(&format!("/conversations/{}", id), 15000)? }));
        }
        "share" => {
            if !valid_id(id) {
                return Err("Provide a valid conversation ID".into());
            }
            let ip = request.get("ip").and_then(|v| v.as_str()).ok_or("ip が必要です")?;
            let body = json!({
                "ip": ip,
                "mode": request.get("mode").and_then(|v| v.as_str()).unwrap_or("content"),
                "remove": request.get("remove").and_then(|v| v.as_bool()).unwrap_or(false),
            });
            emit(&json!({ "type": "result", "value": api.post(&format!("/conversations/{}/access", id), &body, 15000)? }));
        }
        "files" => {
            if !valid_id(id) {
                return Err("Provide a valid conversation ID".into());
            }
            let q = request.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let enc = utf8_percent_encode(q, NON_ALPHANUMERIC).to_string();
            let p = if q.is_empty() {
                format!("/conversations/{}/files", id)
            } else {
                format!("/conversations/{}/files?path={}", id, enc)
            };
            emit(&json!({ "type": "result", "value": api.get(&p, 15000)? }));
        }
        "upload" => {
            if !valid_id(id) {
                return Err("Provide a valid conversation ID".into());
            }
            let mut files = Vec::new();
            let paths = request
                .get("paths")
                .and_then(|v| v.as_array())
                .ok_or("paths が必要です")?;
            for item in paths {
                let p = item.as_str().ok_or("Invalid file path")?;
                let att = build_attachment_from_file(Path::new(p))?;
                files.push(json!({ "path": att["path"], "base64": att["base64"] }));
            }
            emit(&json!({
                "type": "result",
                "value": api.post(&format!("/conversations/{}/files", id), &json!({ "files": files }), 60_000)?,
            }));
        }
        "status" => {
            let path = if id.is_empty() { "/jobs".to_string() } else { job_path()? };
            emit(&json!({ "type": "result", "value": api.get(&path, 15000)? }));
        }
        "fetch" => emit(&json!({ "type": "result", "job": api.get(&job_path()?, 15000)? })),
        "save" => {
            let job = api.get(&job_path()?, 15000)?;
            let out_dir = match request.get("outputDir").and_then(|v| v.as_str()) {
                Some(d) if !d.is_empty() => root.join(d),
                _ => root.join("downloads"),
            };
            let saved = save_job_outputs(&job, &out_dir)?;
            emit(&json!({ "type": "result", "savedDir": saved.to_string_lossy(), "job": job }));
        }
        "cancel" => {
            let p = format!("{}/cancel", job_path()?);
            emit(&json!({ "type": "result", "value": api.post(&p, &empty, 15000)? }));
        }
        "answer" => {
            let p = format!("{}/answer", job_path()?);
            let qid = request.get("questionId").and_then(|v| v.as_str()).ok_or("questionId が必要です")?;
            let ans = request.get("answer").and_then(|v| v.as_str()).ok_or("answer が必要です")?;
            let body = json!({ "questionId": qid, "answer": ans });
            emit(&json!({ "type": "result", "job": api.post(&p, &body, 15000)? }));
        }
        "interrupt" => {
            let p = format!("{}/interrupt", job_path()?);
            let txt = request.get("text").and_then(|v| v.as_str()).ok_or("text が必要です")?;
            let body = json!({ "text": txt });
            emit(&json!({ "type": "result", "job": api.post(&p, &body, 15000)? }));
        }
        "classify" => emit(&json!({ "type": "result", "value": api.post("/classify", request, 120_000)? })),
        "run" => {
            let goal = request.get("goal").and_then(|v| v.as_str()).ok_or("goal が必要です")?;
            let mut body = json!({ "goal": goal });
            if let Some(opts) = request.get("options") {
                body["options"] = opts.clone();
            }
            if let Some(f) = request.get("files") {
                body["files"] = f.clone();
            }
            if request.get("files").is_none() {
                if let Some(paths) = request.get("paths").and_then(|v| v.as_array()) {
                    let mut files = Vec::new();
                    for item in paths {
                        if let Some(s) = item.as_str() {
                            let att = build_attachment_from_file(Path::new(s))?;
                            files.push(json!({ "path": att["path"], "base64": att["base64"] }));
                        }
                    }
                    if !files.is_empty() {
                        body["files"] = Value::Array(files);
                    }
                }
            }
            let submitted = api.post("/jobs", &body, 15000)?;
            let job_id = submitted
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or("job id がありません")?
                .to_string();
            emit(&json!({ "type": "started", "id": job_id, "job": submitted }));
            if !request.get("wait").and_then(|v| v.as_bool()).unwrap_or(true) {
                return Ok(0);
            }
            let mut last_state = String::new();
            let mut last_act = -1i64;
            let mut last_ans = -1i64;
            loop {
                let job = api.get(&format!("/jobs/{}", job_id), 15000)?;
                let status = job.get("status").and_then(|v| v.as_str()).unwrap_or("");
                let stage = job.get("stage").and_then(|v| v.as_str()).unwrap_or("");
                let st = format!("{}:{}", status, stage);
                if last_state != st {
                    emit(&json!({ "type": "progress", "id": job_id, "stage": stage, "status": status }));
                    last_state = st;
                }
                let act_rev = job.get("activityRevision").and_then(|v| v.as_i64()).unwrap_or(0);
                if act_rev != last_act {
                    if let Some(act) = job.get("activity") {
                        emit(&json!({ "type": "activity", "id": job_id, "activity": act }));
                    }
                    last_act = act_rev;
                }
                let ans_rev = job.get("partialAnswerRevision").and_then(|v| v.as_i64()).unwrap_or(0);
                if ans_rev != last_ans {
                    if let Some(pa) = job.get("partialAnswer") {
                        emit(&json!({ "type": "streaming", "id": job_id, "partialAnswer": pa }));
                    }
                    last_ans = ans_rev;
                }
                if status == "waiting_user" {
                    emit(&json!({
                        "type": "question",
                        "id": job_id,
                        "question": job.get("question").cloned().unwrap_or_else(|| json!({})),
                        "job": job,
                    }));
                    return Ok(3);
                }
                if status != "running" && status != "queued" {
                    let mut res = json!({ "type": "result", "job": job });
                    if request.get("download").and_then(|v| v.as_bool()).unwrap_or(false) {
                        let out_dir = match request.get("outputDir").and_then(|v| v.as_str()) {
                            Some(d) if !d.is_empty() => root.join(d),
                            _ => root.join("downloads"),
                        };
                        res["savedDir"] = json!(save_job_outputs(&res["job"], &out_dir)?.to_string_lossy());
                    }
                    emit(&res);
                    return Ok(if status == "completed" { 0 } else { 2 });
                }
                thread::sleep(Duration::from_millis(500));
            }
        }
        _ => return Err("Unknown command".into()),
    }
    Ok(0)
}

fn run_cli_subcommand(args: &mut CliArgs, root: &Path) -> Result<i32, String> {
    if args.help || args.command == "help" {
        emit(&json!({ "type": "result", "value": cli_help_json() }));
        return Ok(0);
    }
    if args.command == "windows" {
        let list: Vec<Value> = desktop_windows()
            .into_iter()
            .map(|w| serde_json::to_value(w).unwrap())
            .collect();
        if !args.window_query.is_empty() {
            let matched = find_window_match(&list, &args.window_query);
            let ok = matched.is_some();
            emit(&json!({ "type": "result", "value": { "windows": list, "matched": matched } }));
            return Ok(if ok { 0 } else { 1 });
        }
        emit(&json!({ "type": "result", "value": list }));
        return Ok(0);
    }
    if args.command == "gui-settings" {
        let res = apply_settings_and_gui(None, root, args)?;
        emit(&json!({ "type": "result", "value": res["gui"] }));
        return Ok(0);
    }

    let api = connect_api_with_failover(root, &args.api_url)?;
    let empty = json!({});
    let need_id = |idx: usize| -> Result<String, String> {
        let s = args
            .positionals
            .get(idx)
            .ok_or_else(|| "Provide a valid UUID".to_string())?;
        if !valid_id(s) {
            return Err("Provide a valid UUID".into());
        }
        Ok(s.clone())
    };

    match args.command.as_str() {
        "settings" | "configure" | "preset" => {
            let client = if args.local_only { None } else { Some(&api) };
            let res = apply_settings_and_gui(client, root, args)?;
            emit(&json!({ "type": "result", "value": res }));
            return Ok(0);
        }
        "health" => { emit(&json!({ "type": "result", "value": api.get("/health", 15000)? })); return Ok(0); }
        "commands" => { emit(&json!({ "type": "result", "value": api.get("/commands", 15000)? })); return Ok(0); }
        "identity" => { emit(&json!({ "type": "result", "value": api.get("/identity", 15000)? })); return Ok(0); }
        "profiles" => { emit(&json!({ "type": "result", "value": api.get("/model-profiles", 15000)? })); return Ok(0); }
        "adapters" => { emit(&json!({ "type": "result", "value": api.get("/worker/adapters", 15000)? })); return Ok(0); }
        "adapter-profiles" => { emit(&json!({ "type": "result", "value": api.get("/worker/adapter-profiles", 15000)? })); return Ok(0); }
        "perf" => { emit(&json!({ "type": "result", "value": api.get("/models/perf", 15000)? })); return Ok(0); }
        "tune-status" => { emit(&json!({ "type": "result", "value": api.get("/tune/status", 15000)? })); return Ok(0); }
        "tune-apply" => {
            let res = api.post("/tune/apply", &empty, 120_000)?;
            if args.sync_gui {
                if let Some(s_obj) = res.get("settings").filter(|v| v.is_object()) {
                    let mut s = SamplingValues::default();
                    sampling_from_json(s_obj, &mut s);
                    let mut patch = json!({});
                    sampling_to_json(&s, &mut patch);
                    patch["samplingPreset"] = json!(preset_name_from_index(infer_preset_index(&s)));
                    let _ = save_gui_settings_file(root, &patch);
                }
            }
            emit(&json!({ "type": "result", "value": res }));
            return Ok(0);
        }
        "autotune" => {
            let started = api.post(
                "/tune/auto",
                &json!({ "task": args.task, "mode": args.tune_mode }),
                15000,
            )?;
            emit(&json!({ "type": "progress", "stage": "started", "value": started }));
            let mut status = json!({});
            for _ in 0..180 {
                thread::sleep(Duration::from_secs(1));
                status = api.get("/tune/status", 10000)?;
                let st = status.get("status").and_then(|v| v.as_str()).unwrap_or("");
                emit(&json!({
                    "type": "progress",
                    "stage": status.get("currentStage").and_then(|v| v.as_str()).unwrap_or(""),
                    "status": st,
                    "progressPercent": status.get("progressPercent").and_then(|v| v.as_i64()).unwrap_or(0),
                }));
                if matches!(st, "completed" | "failed" | "cancelled") {
                    break;
                }
            }
            if args.apply_tune
                && status.get("status").and_then(|v| v.as_str()) == Some("completed")
                && !status.get("alreadyOptimal").and_then(|v| v.as_bool()).unwrap_or(false)
            {
                let applied = api.post("/tune/apply", &empty, 120_000)?;
                if args.sync_gui {
                    if let Some(s_obj) = applied.get("settings").filter(|v| v.is_object()) {
                        let mut s = SamplingValues::default();
                        sampling_from_json(s_obj, &mut s);
                        let mut patch = json!({});
                        sampling_to_json(&s, &mut patch);
                        patch["samplingPreset"] = json!(preset_name_from_index(infer_preset_index(&s)));
                        let _ = save_gui_settings_file(root, &patch);
                    }
                }
                status["appliedResult"] = applied;
            }
            let ok = status.get("status").and_then(|v| v.as_str()) == Some("completed");
            emit(&json!({ "type": "result", "value": status }));
            return Ok(if ok { 0 } else { 2 });
        }
        "models" => {
            let sub = args.positionals.first().map(|s| s.as_str()).unwrap_or("status");
            match sub {
                "status" => emit(&json!({ "type": "result", "value": api.get("/models/status", 15000)? })),
                "perf" => emit(&json!({ "type": "result", "value": api.get("/models/perf", 15000)? })),
                "start" => emit(&json!({ "type": "result", "value": api.post("/models/start", &empty, 120_000)? })),
                "stop" => emit(&json!({ "type": "result", "value": api.post("/models/stop", &empty, 60_000)? })),
                "restart" => emit(&json!({ "type": "result", "value": api.post("/models/restart", &empty, 120_000)? })),
                _ => return Err("Usage: models [status|start|stop|restart|perf]".into()),
            }
            return Ok(0);
        }
        "compact" => {
            let cid = need_id(0)?;
            let focus = if args.positionals.len() > 1 { args.positionals[1..].join(" ") } else { String::new() };
            let body = if focus.is_empty() { empty.clone() } else { json!({ "focus": focus }) };
            emit(&json!({ "type": "result", "value": api.post(&format!("/conversations/{}/compact", cid), &body, 30_000)? }));
            return Ok(0);
        }
        "memory" => {
            let sub = args.positionals.first().map(|s| s.as_str()).unwrap_or("list");
            match sub {
                "list" => emit(&json!({ "type": "result", "value": api.get("/memory", 15000)? })),
                "add" => {
                    let txt = args.positionals.get(1..).map(|s| s.join(" ")).unwrap_or_default();
                    if txt.trim().is_empty() { return Err("Usage: memory add <text>".into()); }
                    emit(&json!({ "type": "result", "value": api.post("/memory", &json!({ "text": txt }), 15000)? }));
                }
                "delete" | "remove" => {
                    let mid = args.positionals.get(1).ok_or_else(|| "Usage: memory delete <id>".to_string())?;
                    emit(&json!({ "type": "result", "value": api.delete(&format!("/memory/{}", mid), 15000)? }));
                }
                _ => return Err("Usage: memory [list|add <text>|delete <id>]".into()),
            }
            return Ok(0);
        }
        "schedules" => {
            let sub = args.positionals.first().map(|s| s.as_str()).unwrap_or("list");
            match sub {
                "list" => emit(&json!({ "type": "result", "value": api.get("/schedules", 15000)? })),
                "add" => {
                    if args.positionals.len() < 3 {
                        return Err("Usage: schedules add \"<in 10m|every 1h>\" <goal>".into());
                    }
                    let spec = &args.positionals[1];
                    let goal = args.positionals[2..].join(" ");
                    let mut body = json!({ "spec": spec, "goal": goal });
                    if !args.conv_id.is_empty() {
                        body["conversationId"] = json!(args.conv_id);
                    }
                    emit(&json!({ "type": "result", "value": api.post("/schedules", &body, 15000)? }));
                }
                "delete" | "remove" | "cancel" => {
                    let sid = args.positionals.get(1).ok_or_else(|| "Usage: schedules delete <id>".to_string())?;
                    emit(&json!({ "type": "result", "value": api.delete(&format!("/schedules/{}", sid), 15000)? }));
                }
                _ => return Err("Usage: schedules [list|add <spec> <goal>|delete <id>]".into()),
            }
            return Ok(0);
        }
        "standby" => return run_standby_for_args(&api, args, root),
        "conversations" => { emit(&json!({ "type": "result", "value": api.get("/conversations", 15000)? })); return Ok(0); }
        "conversation" => { emit(&json!({ "type": "result", "value": api.get(&format!("/conversations/{}", need_id(0)?), 15000)? })); return Ok(0); }
        "share" => {
            let cid = need_id(0)?;
            let ip = if !args.ip.is_empty() {
                args.ip.clone()
            } else if args.positionals.len() > 1 {
                args.positionals[1].clone()
            } else {
                return Err("Provide <conversationId> <ip>".into());
            };
            let mode = if !args.mode_arg.is_empty() {
                args.mode_arg.clone()
            } else if args.positionals.len() > 2 {
                args.positionals[2].clone()
            } else {
                "content".into()
            };
            let body = json!({ "ip": ip, "mode": mode, "remove": args.remove });
            emit(&json!({ "type": "result", "value": api.post(&format!("/conversations/{}/access", cid), &body, 15000)? }));
            return Ok(0);
        }
        "files" => {
            let cid = need_id(0)?;
            let q = if !args.path_arg.is_empty() {
                args.path_arg.clone()
            } else if args.positionals.len() > 1 {
                args.positionals[1].clone()
            } else {
                String::new()
            };
            let enc = utf8_percent_encode(&q, NON_ALPHANUMERIC).to_string();
            let path = if q.is_empty() {
                format!("/conversations/{}/files", cid)
            } else {
                format!("/conversations/{}/files?path={}", cid, enc)
            };
            emit(&json!({ "type": "result", "value": api.get(&path, 15000)? }));
            return Ok(0);
        }
        "upload" => {
            let cid = need_id(0)?;
            let mut files = Vec::new();
            for p in args.positionals.iter().skip(1).chain(args.attach_files.iter()) {
                let att = build_attachment_from_file(Path::new(p))?;
                files.push(json!({ "path": att["path"], "base64": att["base64"] }));
            }
            if files.is_empty() {
                return Err("Specify at least one file to upload".into());
            }
            let body = json!({ "files": files });
            emit(&json!({ "type": "result", "value": api.post(&format!("/conversations/{}/files", cid), &body, 60_000)? }));
            return Ok(0);
        }
        "status" => {
            if args.positionals.is_empty() {
                emit(&json!({ "type": "result", "value": api.get("/jobs", 15000)? }));
            } else {
                emit(&json!({ "type": "result", "value": api.get(&format!("/jobs/{}", need_id(0)?), 15000)? }));
            }
            return Ok(0);
        }
        "fetch" | "save" => {
            let job = api.get(&format!("/jobs/{}", need_id(0)?), 15000)?;
            let out_dir = if args.output_dir.is_empty() {
                root.join("downloads")
            } else {
                root.join(&args.output_dir)
            };
            let saved = save_job_outputs(&job, &out_dir)?;
            emit(&json!({ "type": "result", "job": job, "savedDir": saved.to_string_lossy() }));
            return Ok(0);
        }
        "cancel" => {
            emit(&json!({ "type": "result", "value": api.post(&format!("/jobs/{}/cancel", need_id(0)?), &empty, 15000)? }));
            return Ok(0);
        }
        "answer" => {
            let jid = need_id(0)?;
            let qid = if !args.question_id.is_empty() {
                args.question_id.clone()
            } else if args.positionals.len() > 1 {
                args.positionals[1].clone()
            } else {
                return Err("Provide questionId".into());
            };
            let mut ans = args.answer_text.clone();
            if ans.is_empty() && args.positionals.len() > 2 {
                ans = args.positionals[2..].join(" ");
            }
            if ans.is_empty() {
                return Err("Provide answer text".into());
            }
            let body = json!({ "questionId": qid, "answer": ans });
            emit(&json!({ "type": "result", "job": api.post(&format!("/jobs/{}/answer", jid), &body, 15000)? }));
            return Ok(0);
        }
        "interrupt" => {
            let jid = need_id(0)?;
            let txt = args.positionals[1..].join(" ");
            if txt.is_empty() {
                return Err("Provide interrupt text".into());
            }
            emit(&json!({ "type": "result", "job": api.post(&format!("/jobs/{}/interrupt", jid), &json!({ "text": txt }), 15000)? }));
            return Ok(0);
        }
        "classify" => {
            let txt = args.positionals.join(" ");
            if txt.is_empty() {
                return Err("Provide prompt text to classify".into());
            }
            emit(&json!({ "type": "result", "value": api.post("/classify", &json!({ "text": txt }), 120_000)? }));
            return Ok(0);
        }
        "run" => {
            let goal = if !args.goal_file.is_empty() {
                fs::read_to_string(&args.goal_file)
                    .map_err(|_| format!("Cannot open goal file: {}", args.goal_file))?
            } else {
                args.positionals.join(" ")
            };
            if goal.is_empty() {
                return Err("Provide a prompt (or --file <path>)".into());
            }
            let mut attached = Vec::new();
            for p in &args.attach_files {
                let att = build_attachment_from_file(Path::new(p))?;
                attached.push(json!({ "path": att["path"], "base64": att["base64"] }));
            }
            if attached.len() > 5 {
                return Err("At most 5 attachments".into());
            }
            if goal.encode_utf16().count() > 6000 {
                return Err("Goal must contain 1-6000 characters".into());
            }

            let mut prep = build_job_request(&api, args, root, &goal, &attached, None)?;
            let job_id = prep.job_id.clone();
            let work_mode = prep.work_mode.clone();

            let mut prev_state = String::new();
            let mut prev_act = -1i64;
            let mut prev_ans = -1i64;
            loop {
                let job = api.get(&format!("/jobs/{}", job_id), 15000)?;
                let status = job.get("status").and_then(|v| v.as_str()).unwrap_or("");
                let stage = job.get("stage").and_then(|v| v.as_str()).unwrap_or("");
                let st = format!("{}:{}", status, stage);
                if prev_state != st {
                    emit(&json!({ "type": "progress", "id": job_id, "stage": stage, "status": status }));
                    prev_state = st;
                }
                let act_rev = job.get("activityRevision").and_then(|v| v.as_i64()).unwrap_or(0);
                if act_rev != prev_act {
                    if let Some(act) = job.get("activity") {
                        emit(&json!({ "type": "activity", "id": job_id, "activity": act }));
                    }
                    prev_act = act_rev;
                }
                let ans_rev = job.get("partialAnswerRevision").and_then(|v| v.as_i64()).unwrap_or(0);
                if ans_rev != prev_ans {
                    if let Some(pa) = job.get("partialAnswer") {
                        emit(&json!({ "type": "streaming", "id": job_id, "partialAnswer": pa }));
                    }
                    prev_ans = ans_rev;
                }
                if status == "waiting_user" {
                    let q = job.get("question").cloned().unwrap_or_else(|| json!({}));
                    if args.auto_answer {
                        let qid = q.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let _ = api.post(
                            &format!("/jobs/{}/answer", job_id),
                            &json!({ "questionId": qid, "answer": args.answer_text }),
                            15000,
                        )?;
                        prev_state.clear();
                        continue;
                    }
                    emit(&json!({ "type": "question", "id": job_id, "question": q, "job": job }));
                    prep.bridge.stop();
                    return Ok(3);
                }
                if status != "running" && status != "queued" {
                    prep.bridge.stop();
                    let out_dir = if args.output_dir.is_empty() {
                        root.join("downloads")
                    } else {
                        root.join(&args.output_dir)
                    };
                    let saved = save_job_outputs(&job, &out_dir)?;
                    emit(&json!({ "type": "result", "job": job, "savedDir": saved.to_string_lossy() }));
                    return Ok(if status == "completed" { 0 } else { 2 });
                }
                let ms = match work_mode.as_str() {
                    "chat" => 80,
                    "pc_control" => 150,
                    _ => 400,
                };
                thread::sleep(Duration::from_millis(ms));
            }
        }
        other => Err(format!("Unknown command: {}", other)),
    }
}

pub(crate) struct PreparedJob {
    pub job_id: String,
    pub work_mode: String,
    pub bridge: CliDesktopBridge,
}

pub(crate) fn run_standby_for_args(
    api: &ApiClient,
    args: &mut CliArgs,
    root: &Path,
) -> Result<i32, String> {
    let gui = apply_settings_and_gui(None, root, args)?["gui"].clone();
    let query = if args.window_query.is_empty() {
        gui.get("pcWindowTitle").and_then(|v| v.as_str()).unwrap_or("").to_string()
    } else {
        args.window_query.clone()
    };
    if query.is_empty() {
        return Err("Specify --window <title|hwnd> or save pcWindowTitle first".into());
    }
    let list: Vec<Value> = desktop_windows()
        .into_iter()
        .map(|w| serde_json::to_value(w).unwrap())
        .collect();
    let matched = find_window_match(&list, &query)
        .ok_or_else(|| format!("Target window not found: {}", query))?;
    let allow_local = if args.has_local_cli_override {
        args.local_cli_override
    } else {
        gui.get("localCliEnabled").and_then(|v| v.as_bool()).unwrap_or(true)
    };
    let read_roots = resolve_cli_read_roots(&gui, &args.read_dirs, args.clear_read_dirs)?;
    let hwnd = matched.get("id").and_then(|v| v.as_i64()).unwrap_or(0) as isize;
    let title = matched.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let wd = if args.workdir.is_empty() {
        gui.get("localWorkdir").and_then(|v| v.as_str()).unwrap_or("")
    } else {
        &args.workdir
    };
    run_cli_standby(api.base_url(), root, hwnd, title, wd, read_roots, allow_local)
}

pub(crate) fn build_job_request(
    api: &ApiClient,
    args: &mut CliArgs,
    root: &Path,
    goal: &str,
    attached: &[Value],
    slash_cmd: Option<&str>,
) -> Result<PreparedJob, String> {
    let merged = apply_settings_and_gui(Some(api), root, args)?;
    let gui = &merged["gui"];
    let srv = &merged["server"];
    let health = api.get("/health", 15000)?;
    let caps = health.get("capabilities").cloned().unwrap_or_else(|| json!({}));
    let ws_capable = caps.get("workspace").and_then(|v| v.as_bool()).unwrap_or(false);
    let local_capable = caps.get("localCli").and_then(|v| v.as_bool()).unwrap_or(false);
    let server_win_avail = caps
        .get("serverWindows")
        .and_then(|sw| sw.get("available"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let work_mode = gui.get("workMode").and_then(|v| v.as_str()).unwrap_or("standard");
    let model_role = gui.get("modelRole").and_then(|v| v.as_str()).unwrap_or("planner");
    let plan_mode = gui.get("planMode").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut use_ws = gui.get("workspaceEnabled").and_then(|v| v.as_bool()).unwrap_or(true);
    if !ws_capable
        && work_mode != "pc_control"
        && attached.is_empty()
        && !args.gui_patch.contains_key("workspaceEnabled")
    {
        use_ws = false;
    }
    if work_mode == "pc_control" || !attached.is_empty() || (slash_cmd.is_some() && ws_capable) {
        use_ws = true;
    }
    if use_ws && !ws_capable {
        return Err("作業環境対応のAPIに接続し直してください".into());
    }

    let desktop_allowed = if args.has_pc_override {
        args.pc_override
    } else {
        gui.get("pcEnabled").and_then(|v| v.as_bool()).unwrap_or(false)
    };
    let local_allowed = if args.has_local_cli_override {
        args.local_cli_override
    } else {
        gui.get("localCliEnabled").and_then(|v| v.as_bool()).unwrap_or(true)
    };
    let run_on_pc = gui.get("runOnPc").and_then(|v| v.as_bool()).unwrap_or(false);
    if work_mode == "pc_control" && !desktop_allowed && !args.shared_desktop {
        return Err("pc_control mode requires --pc (with --window) or --shared-desktop".into());
    }

    let mut options = json!({
        "contextWindow": srv.get("contextWindow").and_then(|v| v.as_i64()).or_else(|| gui.get("contextWindow").and_then(|v| v.as_i64())).unwrap_or(8192),
        "maxIterations": 0,
        "maxTokens": srv.get("maxTokens").and_then(|v| v.as_i64()).or_else(|| gui.get("maxTokens").and_then(|v| v.as_i64())).unwrap_or(1800),
        "modelRole": model_role,
    });
    if let Some(re) = srv.get("reasoningEffort") {
        options["reasoningEffort"] = re.clone();
    }
    if let Some(rb) = srv.get("reasoningBudgetTokens") {
        options["reasoningBudgetTokens"] = rb.clone();
    }
    let mut samp = SamplingValues::default();
    sampling_from_json(srv, &mut samp);
    sampling_to_json(&samp, &mut options);
    if work_mode != "standard" {
        options["workMode"] = json!(work_mode);
    }
    if work_mode == "chat" || work_mode == "pc_control" {
        for k in [
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
            if !args.server_patch.contains_key(k) {
                if let Some(m) = options.as_object_mut() {
                    m.remove(k);
                }
            }
        }
        if model_role == "planner" {
            options["planMode"] = json!(false);
        }
    } else if work_mode == "coding" {
        for k in ["maxTokens", "temperature"] {
            if !args.server_patch.contains_key(k) {
                if let Some(m) = options.as_object_mut() {
                    m.remove(k);
                }
            }
        }
        if model_role == "planner" {
            options["planMode"] = json!(plan_mode);
        }
    } else if model_role == "planner" {
        options["planMode"] = json!(plan_mode);
    }
    if let Some(sc) = slash_cmd {
        options["slashCommand"] = json!(sc);
        if model_role == "planner" {
            options["planMode"] = json!(false);
        }
    }

    if use_ws {
        options["execution"] = json!("workspace");
    }
    let mut machine = String::new();
    if server_win_avail && use_ws {
        machine = if run_on_pc || desktop_allowed || args.shared_desktop {
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
        && use_ws
        && local_allowed
        && machine != "server-windows";
    if use_local && !local_capable {
        return Err("PCのCLIに対応したAPIへ接続し直してください".into());
    }
    if args.shared_desktop {
        options["useSharedDesktop"] = json!(true);
    }
    if !args.conv_id.is_empty() {
        options["conversationId"] = json!(args.conv_id);
    } else if !args.parent_id.is_empty() {
        options["parentJobId"] = json!(args.parent_id);
    }

    let mut bridge = CliDesktopBridge::new();
    let mut target_hwnd = 0isize;
    let mut target_title = String::new();
    if !args.shared_desktop && (desktop_allowed || use_local) {
        if desktop_allowed {
            let query = if args.window_query.is_empty() {
                gui.get("pcWindowTitle").and_then(|v| v.as_str()).unwrap_or("").to_string()
            } else {
                args.window_query.clone()
            };
            if query.is_empty() {
                return Err("PC screen control requires --window <title|hwnd>".into());
            }
            let list: Vec<Value> = desktop_windows()
                .into_iter()
                .map(|w| serde_json::to_value(w).unwrap())
                .collect();
            let matched = find_window_match(&list, &query)
                .ok_or_else(|| format!("Target window not found: {}", query))?;
            target_hwnd = matched.get("id").and_then(|v| v.as_i64()).unwrap_or(0) as isize;
            target_title = matched.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
        }
        options["desktopSession"] = json!(bridge.open_session(
            api.base_url(),
            &target_title,
            desktop_allowed,
            use_local,
        )?);
    }

    let mut body = json!({ "goal": goal, "options": options });
    if !attached.is_empty() {
        body["files"] = Value::Array(attached.to_vec());
    }
    let submitted = api.post("/jobs", &body, 15000)?;
    let job_id = submitted
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("job id がありません")?
        .to_string();
    if !bridge.session_id.is_empty() {
        let ws = if use_local {
            let custom = if args.workdir.is_empty() {
                gui.get("localWorkdir").and_then(|v| v.as_str()).unwrap_or("")
            } else {
                &args.workdir
            };
            let fb = submitted
                .get("workspaceId")
                .and_then(|v| v.as_str())
                .unwrap_or(&job_id);
            resolve_cli_workspace(root, custom, fb)?
        } else {
            PathBuf::new()
        };
        bridge.start_loop(
            target_hwnd,
            ws,
            resolve_cli_read_roots(gui, &args.read_dirs, args.clear_read_dirs)?,
            desktop_allowed,
            use_local,
        )?;
    }
    Ok(PreparedJob {
        job_id,
        work_mode: work_mode.to_string(),
        bridge,
    })
}

pub fn run_diag_selftest() -> i32 {
    let _console = ConsoleSession::attach(false);
    diag_install();
    let dir = config_root_dir();
    let crash_dir = dir.join("crash");
    if fs::create_dir_all(&crash_dir).is_err() {
        return 1;
    }
    let report = crash_dir.join(format!("diag-selftest-{}.txt", std::process::id()));
    let body = format!(
        "diag selftest ok\npid={}\ntime={}\n",
        std::process::id(),
        chrono::Local::now().to_rfc3339()
    );
    if write_file_atomic(&report, body.as_bytes()).is_err() {
        return 1;
    }
    diag_line(&format!("diag selftest: wrote {}", report.display()));
    let log = dir.join("gui-startup.log");
    if report.is_file() && log.is_file() {
        0
    } else {
        1
    }
}

pub fn run_api_cli(args: &[String]) -> i32 {
    let _console = ConsoleSession::attach(false);
    let root = config_root_dir();
    match (|| -> Result<i32, String> {
        let mut parsed = parse_cli_args(args)?;
        if !parsed.stdin_mode {
            return run_cli_subcommand(&mut parsed, &root);
        }
        let mut input = String::new();
        for line in io::stdin().lock().lines() {
            let l = line.map_err(|e| e.to_string())?;
            input.push_str(&l);
            if input.len() > 4_000_000 {
                return Err("Request too large".into());
            }
        }
        let clean = input.strip_prefix('\u{FEFF}').unwrap_or(&input);
        let req: Value = serde_json::from_str(clean).map_err(|e| e.to_string())?;
        let api = connect_api_with_failover(&root, "")?;
        run_stdin_command(&req, &root, &api)
    })() {
        Ok(code) => code,
        Err(e) => {
            emit(&json!({ "type": "error", "message": e }));
            1
        }
    }
}
