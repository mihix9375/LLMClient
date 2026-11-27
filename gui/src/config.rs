use crate::win_util::{executable_dir, no_links, write_file_atomic};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SamplingValues {
    pub temperature: f64,
    pub top_p: f64,
    pub top_k: i32,
    pub min_p: f64,
    pub repeat_penalty: f64,
    pub presence_penalty: f64,
    pub frequency_penalty: f64,
}

impl Default for SamplingValues {
    fn default() -> Self {
        Self {
            temperature: 0.05,
            top_p: 0.95,
            top_k: 40,
            min_p: 0.05,
            repeat_penalty: 1.10,
            presence_penalty: 0.0,
            frequency_penalty: 0.0,
        }
    }
}

pub fn preset_index_from_name(raw: &str) -> i32 {
    match raw.to_ascii_lowercase().as_str() {
        "0" | "code" | "precise" => 0,
        "1" | "balanced" | "standard" => 1,
        "2" | "creative" => 2,
        "3" | "custom" => 3,
        _ => -1,
    }
}

pub fn preset_name_from_index(index: i32) -> &'static str {
    match index {
        0 => "code",
        1 => "balanced",
        2 => "creative",
        _ => "custom",
    }
}

pub fn preset_sampling(index: i32) -> SamplingValues {
    match index {
        1 => SamplingValues {
            temperature: 0.20,
            ..SamplingValues::default()
        },
        2 => SamplingValues {
            temperature: 0.70,
            top_p: 0.95,
            top_k: 60,
            min_p: 0.03,
            repeat_penalty: 1.15,
            presence_penalty: 0.10,
            frequency_penalty: 0.10,
        },
        _ => SamplingValues::default(),
    }
}

pub fn infer_preset_index(v: &SamplingValues) -> i32 {
    let close = |a: f64, b: f64| (a - b).abs() < 0.005;
    if close(v.temperature, 0.05)
        && close(v.top_p, 0.95)
        && v.top_k == 40
        && close(v.min_p, 0.05)
        && close(v.repeat_penalty, 1.10)
        && close(v.presence_penalty, 0.0)
        && close(v.frequency_penalty, 0.0)
    {
        return 0;
    }
    if close(v.temperature, 0.20)
        && close(v.top_p, 0.95)
        && v.top_k == 40
        && close(v.min_p, 0.05)
        && close(v.repeat_penalty, 1.10)
        && close(v.presence_penalty, 0.0)
        && close(v.frequency_penalty, 0.0)
    {
        return 1;
    }
    if close(v.temperature, 0.70)
        && close(v.top_p, 0.95)
        && v.top_k == 60
        && close(v.min_p, 0.03)
        && close(v.repeat_penalty, 1.15)
        && close(v.presence_penalty, 0.10)
        && close(v.frequency_penalty, 0.10)
    {
        return 2;
    }
    3
}

pub fn is_valid_work_mode(mode: &str) -> bool {
    matches!(mode, "standard" | "chat" | "coding" | "pc_control")
}

pub fn is_valid_model_role(role: &str) -> bool {
    matches!(role, "planner" | "worker")
}

pub fn is_valid_effort(effort: &str) -> bool {
    matches!(effort, "none" | "low" | "medium" | "xhigh")
}

pub fn validate_context_and_tokens(context_window: i32, max_tokens: i32) -> Result<(), String> {
    if !(2048..=32768).contains(&context_window) {
        return Err("contextWindow must be 2048-32768".into());
    }
    if !(128..=8192).contains(&max_tokens) {
        return Err("maxTokens must be 128-8192".into());
    }
    if max_tokens + 256 >= context_window {
        return Err("contextWindow must exceed maxTokens + 256".into());
    }
    Ok(())
}

pub fn validate_sampling_values(v: &SamplingValues) -> Result<(), String> {
    if !v.temperature.is_finite() || v.temperature < 0.0 || v.temperature > 2.0 {
        return Err("temperature must be 0.0-2.0".into());
    }
    if !v.top_p.is_finite() || v.top_p < 0.0 || v.top_p > 1.0 {
        return Err("top_p must be 0.0-1.0".into());
    }
    if !(1..=100).contains(&v.top_k) {
        return Err("top_k must be 1-100".into());
    }
    if !v.min_p.is_finite() || v.min_p < 0.0 || v.min_p > 1.0 {
        return Err("min_p must be 0.0-1.0".into());
    }
    if !v.repeat_penalty.is_finite() || v.repeat_penalty < 1.0 || v.repeat_penalty > 2.0 {
        return Err("repeat_penalty must be 1.0-2.0".into());
    }
    if !v.presence_penalty.is_finite() || v.presence_penalty < -2.0 || v.presence_penalty > 2.0 {
        return Err("presence_penalty must be -2.0-2.0".into());
    }
    if !v.frequency_penalty.is_finite() || v.frequency_penalty < -2.0 || v.frequency_penalty > 2.0 {
        return Err("frequency_penalty must be -2.0-2.0".into());
    }
    Ok(())
}

pub fn sampling_from_json(src: &Value, dst: &mut SamplingValues) {
    let Some(obj) = src.as_object() else {
        return;
    };
    let num = |k: &str| obj.get(k).and_then(|v| v.as_f64());
    if let Some(x) = num("temperature") {
        dst.temperature = x;
    }
    if let Some(x) = num("top_p").or_else(|| num("topP")) {
        dst.top_p = x;
    }
    if let Some(x) = obj
        .get("top_k")
        .or_else(|| obj.get("topK"))
        .and_then(|v| v.as_i64())
    {
        dst.top_k = x as i32;
    }
    if let Some(x) = num("min_p").or_else(|| num("minP")) {
        dst.min_p = x;
    }
    if let Some(x) = num("repeat_penalty").or_else(|| num("repeatPenalty")) {
        dst.repeat_penalty = x;
    }
    if let Some(x) = num("presence_penalty").or_else(|| num("presencePenalty")) {
        dst.presence_penalty = x;
    }
    if let Some(x) = num("frequency_penalty").or_else(|| num("frequencyPenalty")) {
        dst.frequency_penalty = x;
    }
}

pub fn sampling_to_json(src: &SamplingValues, dst: &mut Value) {
    if let Some(obj) = dst.as_object_mut() {
        obj.insert("temperature".into(), json!(src.temperature));
        obj.insert("top_p".into(), json!(src.top_p));
        obj.insert("top_k".into(), json!(src.top_k));
        obj.insert("min_p".into(), json!(src.min_p));
        obj.insert("repeat_penalty".into(), json!(src.repeat_penalty));
        obj.insert("presence_penalty".into(), json!(src.presence_penalty));
        obj.insert("frequency_penalty".into(), json!(src.frequency_penalty));
    }
}

pub fn default_gui_settings() -> Value {
    json!({
        "api": "https://localhost:8443",
        "autoConnect": true,
        "darkTheme": true,
        "workspaceEnabled": true,
        "localCliEnabled": true,
        "runOnPc": false,
        "pcEnabled": false,
        "pcStandby": false,
        "pcWindowTitle": "",
        "localWorkdir": "",
        "readOnlyFolders": [],
        "localWorkdirs": {},
        "modelRole": "planner",
        "workMode": "standard",
        "planMode": false,
        "samplingPreset": "code",
        "contextWindow": 16384,
        "maxTokens": 1800,
        "reasoningEffort": "none",
        "reasoningBudgetTokens": 2048,
        "temperature": 0.05,
        "top_p": 0.95,
        "top_k": 40,
        "min_p": 0.05,
        "repeat_penalty": 1.10,
        "presence_penalty": 0.0,
        "frequency_penalty": 0.0
    })
}

pub fn is_supported_gui_key(key: &str) -> bool {
    default_gui_settings()
        .as_object()
        .is_some_and(|m| m.contains_key(key))
        || matches!(
            key,
            "lastJob" | "conversationId" | "windowX" | "windowY" | "windowWidth" | "windowHeight" | "windowMaximized"
        )
}

pub fn load_gui_settings_file(root: &Path) -> Value {
    let mut out = default_gui_settings();
    if let Ok(bytes) = fs::read_to_string(root.join("gui-settings.json")) {
        if let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(&bytes) {
            if let Some(dst) = out.as_object_mut() {
                for (k, v) in parsed {
                    if is_supported_gui_key(&k) {
                        dst.insert(k, v);
                    }
                }
            }
        }
    }
    out
}

pub fn save_gui_settings_file(root: &Path, patch: &Value) -> Result<Value, String> {
    let mut merged = load_gui_settings_file(root);
    if let (Some(dst), Some(src)) = (merged.as_object_mut(), patch.as_object()) {
        for (k, v) in src {
            if is_supported_gui_key(k) {
                dst.insert(k.clone(), v.clone());
            }
        }
    }
    let text = serde_json::to_string_pretty(&merged).map_err(|e| e.to_string())?;
    write_file_atomic(&root.join("gui-settings.json"), text.as_bytes())?;
    Ok(merged)
}

pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    if b.len() != 36 {
        return false;
    }
    for (idx, &ch) in b.iter().enumerate() {
        if matches!(idx, 8 | 13 | 18 | 23) {
            if ch != b'-' {
                return false;
            }
        } else if !matches!(ch, b'0'..=b'9' | b'a'..=b'f') {
            return false;
        }
    }
    true
}

pub fn is_reserved_device_name(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or("");
    let upper = stem.to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    if upper.len() == 4
        && (upper.starts_with("COM") || upper.starts_with("LPT"))
        && matches!(upper.as_bytes()[3], b'1'..=b'9')
    {
        return true;
    }
    false
}

pub fn safe_artifact(path: &str) -> bool {
    if path.is_empty() || path.len() > 160 {
        return false;
    }
    if !path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b'-'))
    {
        return false;
    }
    for part in path.split('/') {
        if part.is_empty()
            || part.starts_with('.')
            || part.ends_with('.')
            || is_reserved_device_name(part)
        {
            return false;
        }
    }
    true
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkdirCommand {
    Other,
    Choose,
    Set(String),
    Show,
    Reset,
}

pub fn parse_workdir_command(raw: &str) -> Result<WorkdirCommand, String> {
    let text = raw.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\r' | '\n'));
    if text.is_empty() {
        return Ok(WorkdirCommand::Other);
    }
    if text == "/pwd" {
        return Ok(WorkdirCommand::Show);
    }
    if text != "/workdir" && !text.starts_with("/workdir ") && !text.starts_with("/workdir\t") {
        return Ok(WorkdirCommand::Other);
    }
    let rest = text[8..].trim_start_matches(|c: char| c == ' ' || c == '\t');
    if rest.is_empty() {
        return Ok(WorkdirCommand::Choose);
    }
    if rest == "default" || rest == "reset" {
        return Ok(WorkdirCommand::Reset);
    }
    let mut path = rest;
    if path.len() >= 2
        && ((path.starts_with('"') && path.ends_with('"'))
            || (path.starts_with('\'') && path.ends_with('\'')))
    {
        path = &path[1..path.len() - 1];
    }
    if path.contains('\r') || path.contains('\n') {
        return Err("/workdirは1行で指定してください".into());
    }
    Ok(WorkdirCommand::Set(path.to_string()))
}

pub fn strip_unc_prefix(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{}", stripped))
    } else if let Some(stripped) = s.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path
    }
}

pub fn validate_workdir(raw: &str) -> Result<PathBuf, String> {
    let input = Path::new(raw);
    if !input.is_absolute() {
        return Err("作業フォルダーは絶対パスで指定してください".into());
    }
    let clean = strip_unc_prefix(
        input
            .components()
            .collect::<PathBuf>(),
    );
    if clean.parent().is_none() {
        return Err("ドライブ全体ではなく、作業用のフォルダーを指定してください".into());
    }
    no_links(input)?;
    if !input.is_dir() {
        return Err("指定した作業フォルダーがありません。先にフォルダーを作成してください".into());
    }
    let canon = fs::canonicalize(input).map_err(|e| format!("作業フォルダーを確認できません: {}", e))?;
    Ok(strip_unc_prefix(canon))
}

#[derive(Clone, Debug)]
pub struct CliArgs {
    pub stdin_mode: bool,
    pub help: bool,
    pub json_out: bool,
    pub local_only: bool,
    pub sync_gui: bool,
    pub command: String,
    pub positionals: Vec<String>,
    pub api_url: String,
    pub server_patch: Map<String, Value>,
    pub gui_patch: Map<String, Value>,
    pub window_query: String,
    pub workdir: String,
    pub read_dirs: Vec<String>,
    pub clear_read_dirs: bool,
    pub attach_files: Vec<String>,
    pub conv_id: String,
    pub parent_id: String,
    pub goal_file: String,
    pub output_dir: String,
    pub path_arg: String,
    pub ip: String,
    pub mode_arg: String,
    pub remove: bool,
    pub task: String,
    pub tune_mode: String,
    pub apply_tune: bool,
    pub shared_desktop: bool,
    pub auto_answer: bool,
    pub answer_text: String,
    pub question_id: String,
    pub has_pc_override: bool,
    pub pc_override: bool,
    pub has_local_cli_override: bool,
    pub local_cli_override: bool,
    pub full_job: bool,
    pub attach_clipboard: bool,
    pub history_filter: String,
    pub history_query: String,
    pub remove_read_dirs: Vec<String>,
    pub dark_theme: Option<bool>,
}

impl Default for CliArgs {
    fn default() -> Self {
        Self {
            stdin_mode: false,
            help: false,
            json_out: false,
            local_only: false,
            sync_gui: true,
            command: String::new(),
            positionals: Vec::new(),
            api_url: String::new(),
            server_patch: Map::new(),
            gui_patch: Map::new(),
            window_query: String::new(),
            workdir: String::new(),
            read_dirs: Vec::new(),
            clear_read_dirs: false,
            attach_files: Vec::new(),
            conv_id: String::new(),
            parent_id: String::new(),
            goal_file: String::new(),
            output_dir: String::new(),
            path_arg: String::new(),
            ip: String::new(),
            mode_arg: String::new(),
            remove: false,
            task: "coding".into(),
            tune_mode: "quick".into(),
            apply_tune: false,
            shared_desktop: false,
            auto_answer: false,
            answer_text: String::new(),
            question_id: String::new(),
            has_pc_override: false,
            pc_override: false,
            has_local_cli_override: false,
            local_cli_override: false,
            full_job: false,
            attach_clipboard: false,
            history_filter: String::new(),
            history_query: String::new(),
            remove_read_dirs: Vec::new(),
            dark_theme: None,
        }
    }
}

pub fn parse_cli_args(args: &[String]) -> Result<CliArgs, String> {
    let mut out = CliArgs::default();
    let mut start = 1usize;
    if args.len() >= 2 {
        let a1 = args[1].as_str();
        if a1 == "--api" || a1 == "-api" || a1 == "api" {
            start = 2;
        }
    }
    if start >= args.len() {
        out.stdin_mode = true;
        return Ok(out);
    }

    let parse_int = |s: &str, name: &str| -> Result<i32, String> {
        s.parse::<i32>()
            .map_err(|_| format!("Invalid integer for {}", name))
    };
    let parse_double = |s: &str, name: &str| -> Result<f64, String> {
        let v = s
            .parse::<f64>()
            .map_err(|_| format!("Invalid number for {}", name))?;
        if !v.is_finite() {
            return Err(format!("Invalid number for {}", name));
        }
        Ok(v)
    };

    let mut i = start;
    while i < args.len() {
        let raw = &args[i];
        let mut need_val = |flag: &str| -> Result<String, String> {
            if i + 1 >= args.len() {
                return Err(format!("Missing value for {}", flag));
            }
            i += 1;
            Ok(args[i].clone())
        };

        match raw.as_str() {
            "-h" | "--help" => out.help = true,
            "--json" => out.json_out = true,
            "--local-only" => out.local_only = true,
            "--no-sync-gui" => out.sync_gui = false,
            "--sync-gui" => out.sync_gui = true,
            "--url" | "--api-url" => {
                let v = need_val(raw)?;
                out.api_url = v.clone();
                out.gui_patch.insert("api".into(), Value::String(v));
            }
            "--tunnel" => return Err("SSH is disabled; configure direct HTTPS via --url".into()),
            "--no-tunnel" => {}
            "--ssh-target" => {
                let _ = need_val(raw)?;
                return Err("SSH is disabled; configure direct HTTPS via --url".into());
            }
            "--auto-connect" => {
                out.gui_patch.insert("autoConnect".into(), Value::Bool(true));
            }
            "--no-connect" => {
                out.gui_patch.insert("autoConnect".into(), Value::Bool(false));
            }
            "--work-mode" => {
                let v = need_val(raw)?;
                if !is_valid_work_mode(&v) {
                    return Err(format!("Unknown workMode: {}", v));
                }
                out.gui_patch.insert("workMode".into(), Value::String(v));
            }
            "--mode" => out.mode_arg = need_val(raw)?,
            "--role" | "--model-role" => {
                let v = need_val(raw)?;
                if !is_valid_model_role(&v) {
                    return Err(format!("Invalid modelRole: {}", v));
                }
                out.gui_patch.insert("modelRole".into(), Value::String(v));
            }
            "--plan" => {
                out.gui_patch.insert("planMode".into(), Value::Bool(true));
            }
            "--no-plan" | "--direct" => {
                out.gui_patch.insert("planMode".into(), Value::Bool(false));
            }
            "--context" => {
                let v = parse_int(&need_val(raw)?, "contextWindow")?;
                out.server_patch.insert("contextWindow".into(), json!(v));
                out.gui_patch.insert("contextWindow".into(), json!(v));
            }
            "--tokens" | "--max-tokens" => {
                let v = parse_int(&need_val(raw)?, "maxTokens")?;
                out.server_patch.insert("maxTokens".into(), json!(v));
                out.gui_patch.insert("maxTokens".into(), json!(v));
            }
            "--iterations" | "--max-iterations" => {
                let v = parse_int(&need_val(raw)?, "maxIterations")?;
                out.server_patch.insert("maxIterations".into(), json!(v));
            }
            "--effort" | "--reasoning-effort" => {
                let v = need_val(raw)?;
                if !is_valid_effort(&v) {
                    return Err(format!("Invalid reasoningEffort: {}", v));
                }
                out.server_patch.insert("reasoningEffort".into(), json!(v));
                out.gui_patch.insert("reasoningEffort".into(), json!(v));
            }
            "--budget" | "--reasoning-budget" => {
                let v = parse_int(&need_val(raw)?, "reasoningBudgetTokens")?;
                if !(-1..=8192).contains(&v) {
                    return Err("reasoningBudgetTokens must be -1..8192".into());
                }
                out.server_patch
                    .insert("reasoningBudgetTokens".into(), json!(v));
                out.gui_patch
                    .insert("reasoningBudgetTokens".into(), json!(v));
            }
            "--preset" => {
                let idx = preset_index_from_name(&need_val(raw)?);
                if !(0..=2).contains(&idx) {
                    return Err("preset must be code, balanced, or creative".into());
                }
                let s = preset_sampling(idx);
                let mut sp = Value::Object(out.server_patch.clone());
                let mut gp = Value::Object(out.gui_patch.clone());
                sampling_to_json(&s, &mut sp);
                sampling_to_json(&s, &mut gp);
                out.server_patch = sp.as_object().cloned().unwrap_or_default();
                out.gui_patch = gp.as_object().cloned().unwrap_or_default();
                out.gui_patch
                    .insert("samplingPreset".into(), json!(preset_name_from_index(idx)));
            }
            "--temp" | "--temperature" => {
                let v = parse_double(&need_val(raw)?, "temperature")?;
                out.server_patch.insert("temperature".into(), json!(v));
                out.gui_patch.insert("temperature".into(), json!(v));
            }
            "--top-p" => {
                let v = parse_double(&need_val(raw)?, "top_p")?;
                out.server_patch.insert("top_p".into(), json!(v));
                out.gui_patch.insert("top_p".into(), json!(v));
            }
            "--top-k" => {
                let v = parse_int(&need_val(raw)?, "top_k")?;
                out.server_patch.insert("top_k".into(), json!(v));
                out.gui_patch.insert("top_k".into(), json!(v));
            }
            "--min-p" => {
                let v = parse_double(&need_val(raw)?, "min_p")?;
                out.server_patch.insert("min_p".into(), json!(v));
                out.gui_patch.insert("min_p".into(), json!(v));
            }
            "--repeat-penalty" => {
                let v = parse_double(&need_val(raw)?, "repeat_penalty")?;
                out.server_patch.insert("repeat_penalty".into(), json!(v));
                out.gui_patch.insert("repeat_penalty".into(), json!(v));
            }
            "--presence-penalty" => {
                let v = parse_double(&need_val(raw)?, "presence_penalty")?;
                out.server_patch.insert("presence_penalty".into(), json!(v));
                out.gui_patch.insert("presence_penalty".into(), json!(v));
            }
            "--frequency-penalty" => {
                let v = parse_double(&need_val(raw)?, "frequency_penalty")?;
                out.server_patch
                    .insert("frequency_penalty".into(), json!(v));
                out.gui_patch.insert("frequency_penalty".into(), json!(v));
            }
            "--workspace" => {
                out.gui_patch
                    .insert("workspaceEnabled".into(), Value::Bool(true));
            }
            "--no-workspace" => {
                out.gui_patch
                    .insert("workspaceEnabled".into(), Value::Bool(false));
            }
            "--local-cli" => {
                out.has_local_cli_override = true;
                out.local_cli_override = true;
                out.gui_patch
                    .insert("localCliEnabled".into(), Value::Bool(true));
            }
            "--no-local-cli" => {
                out.has_local_cli_override = true;
                out.local_cli_override = false;
                out.gui_patch
                    .insert("localCliEnabled".into(), Value::Bool(false));
            }
            "--run-on-pc" => {
                out.gui_patch.insert("runOnPc".into(), Value::Bool(true));
            }
            "--server-windows" => {
                out.gui_patch.insert("runOnPc".into(), Value::Bool(false));
            }
            "--workdir" => {
                let v = need_val(raw)?;
                out.workdir = v.clone();
                out.gui_patch.insert("localWorkdir".into(), Value::String(v));
            }
            "--read-dir" => out.read_dirs.push(need_val(raw)?),
            "--clear-read-dirs" => {
                out.clear_read_dirs = true;
                out.gui_patch.insert("readOnlyFolders".into(), json!([]));
            }
            "--pc" => {
                out.has_pc_override = true;
                out.pc_override = true;
                out.gui_patch.insert("pcEnabled".into(), Value::Bool(true));
            }
            "--no-pc" => {
                out.has_pc_override = true;
                out.pc_override = false;
                out.gui_patch.insert("pcEnabled".into(), Value::Bool(false));
            }
            "--window" => {
                let v = need_val(raw)?;
                out.window_query = v.clone();
                out.gui_patch
                    .insert("pcWindowTitle".into(), Value::String(v));
            }
            "--standby" => {
                out.gui_patch.insert("pcStandby".into(), Value::Bool(true));
            }
            "--no-standby" => {
                out.gui_patch.insert("pcStandby".into(), Value::Bool(false));
            }
            "--shared-desktop" => out.shared_desktop = true,
            "--attach" => out.attach_files.push(need_val(raw)?),
            "--attach-clipboard" | "--paste" => out.attach_clipboard = true,
            "--conv" | "--conversation" => out.conv_id = need_val(raw)?,
            "--parent" | "--parent-job" => out.parent_id = need_val(raw)?,
            "--file" => out.goal_file = need_val(raw)?,
            "--output" | "--out" => out.output_dir = need_val(raw)?,
            "--path" => out.path_arg = need_val(raw)?,
            "--ip" => out.ip = need_val(raw)?,
            "--remove" => out.remove = true,
            "--full" => out.full_job = true,
            "--filter" => out.history_filter = need_val(raw)?,
            "--query" => out.history_query = need_val(raw)?,
            "--remove-read-dir" => out.remove_read_dirs.push(need_val(raw)?),
            "--dark-theme" => {
                out.dark_theme = Some(true);
                out.gui_patch.insert("darkTheme".into(), Value::Bool(true));
            }
            "--light-theme" => {
                out.dark_theme = Some(false);
                out.gui_patch.insert("darkTheme".into(), Value::Bool(false));
            }
            "--task" => out.task = need_val(raw)?,
            "--tune-mode" => out.tune_mode = need_val(raw)?,
            "--apply" => out.apply_tune = true,
            "--answer" => {
                out.auto_answer = true;
                out.answer_text = need_val(raw)?;
            }
            "--question-id" => out.question_id = need_val(raw)?,
            other => {
                if other.starts_with('-') {
                    return Err(format!("Unknown option: {}", other));
                } else if out.command.is_empty() {
                    out.command = other.to_string();
                } else {
                    out.positionals.push(other.to_string());
                }
            }
        }
        i += 1;
    }

    if !out.mode_arg.is_empty() && out.command != "share" {
        if !is_valid_work_mode(&out.mode_arg) {
            return Err(format!("Unknown workMode: {}", out.mode_arg));
        }
        out.gui_patch
            .insert("workMode".into(), Value::String(out.mode_arg.clone()));
    }
    if out.command.is_empty() && !out.help {
        out.command = "help".to_string();
    }
    Ok(out)
}

#[derive(Clone, Debug)]
pub struct HttpsTrust {
    pub urls: Vec<String>,
    pub sha256: String,
    pub pins: HashMap<String, String>,
}

pub fn load_https_trust(root: &Path) -> Result<Option<HttpsTrust>, String> {
    let path = root.join("https-server.json");
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("https-server.json read error: {}", e))?;
    let val: Value =
        serde_json::from_str(&text).map_err(|e| format!("https-server.json parse error: {}", e))?;
    let sha = val
        .get("sha256")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut urls = Vec::new();
    let mut pins = HashMap::new();
    if let Some(arr) = val.get("urls").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(s) = item.as_str() {
                urls.push(s.to_string());
            }
        }
    }
    if let Some(u) = val.get("url").and_then(|v| v.as_str()) {
        if !urls.iter().any(|x| x == u) {
            urls.push(u.to_string());
        }
    }
    if let Some(obj) = val.get("pins").and_then(|v| v.as_object()) {
        for (u, p) in obj {
            if let Some(ps) = p.as_str() {
                pins.insert(u.clone(), ps.to_string());
                if !urls.iter().any(|x| x == u) {
                    urls.push(u.clone());
                }
            }
        }
    }
    Ok(Some(HttpsTrust { urls, sha256: sha, pins }))
}

fn is_legacy_tunnel_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://127.0.0.1:18090") || lower.starts_with("http://localhost:18090")
}

pub fn resolve_api_candidates(root: &Path, preferred: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |raw: &str| {
        let clean = raw.trim().trim_end_matches('/').to_string();
        if !clean.is_empty() && !out.iter().any(|x| x == &clean) {
            out.push(clean);
        }
    };
    let trust_urls = load_https_trust(root)
        .ok()
        .flatten()
        .map(|t| t.urls)
        .unwrap_or_default();
    let has_https_trust = trust_urls.iter().any(|u| u.starts_with("https://"));
    let pref_clean = preferred.trim().trim_end_matches('/');
    if !pref_clean.is_empty() && !(has_https_trust && is_legacy_tunnel_url(pref_clean)) {
        push(pref_clean);
    }
    for u in &trust_urls {
        push(u);
    }
    if root.join("gui-settings.json").exists() {
        let gui = load_gui_settings_file(root);
        if let Some(saved) = gui.get("api").and_then(|v| v.as_str()) {
            if !saved.trim().is_empty() && !(has_https_trust && is_legacy_tunnel_url(saved)) {
                push(saved);
            }
        }
    }
    if out.is_empty() {
        out.push("https://localhost:8443".to_string());
    }
    out
}

pub fn normalize_sha256_pin(pin: &str) -> Result<String, String> {
    if pin.len() != 64 || !pin.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid TLS SHA-256 fingerprint".into());
    }
    Ok(pin.to_ascii_uppercase())
}

pub fn find_window_match(windows: &[Value], query: &str) -> Option<Value> {
    if query.is_empty() {
        return None;
    }
    if query.bytes().all(|b| b.is_ascii_digit()) {
        if let Ok(target_id) = query.parse::<u64>() {
            if let Some(found) = windows
                .iter()
                .find(|w| w.get("id").and_then(|v| v.as_u64()) == Some(target_id))
            {
                return Some(found.clone());
            }
        }
    }
    let needle = query.to_ascii_lowercase();
    if let Some(exact) = windows.iter().find(|w| {
        w.get("title")
            .and_then(|v| v.as_str())
            .map(|t| t.to_ascii_lowercase() == needle)
            .unwrap_or(false)
    }) {
        return Some(exact.clone());
    }
    windows
        .iter()
        .find(|w| {
            w.get("title")
                .and_then(|v| v.as_str())
                .map(|t| t.to_ascii_lowercase().contains(&needle))
                .unwrap_or(false)
        })
        .cloned()
}

pub fn trim_log_if_oversized(path: &Path, max_bytes: u64, keep_bytes: u64) {
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    let size = meta.len();
    if size <= max_bytes || keep_bytes >= size || keep_bytes == 0 {
        return;
    }
    let Ok(mut reader) = fs::File::open(path) else {
        return;
    };
    if reader.seek(SeekFrom::Start(size - keep_bytes)).is_err() {
        return;
    }
    let mut buf = vec![0u8; keep_bytes as usize];
    let Ok(n) = reader.read(&mut buf) else {
        return;
    };
    drop(reader);
    if n == 0 {
        return;
    }
    buf.truncate(n);
    if let Some(nl) = buf.iter().position(|&b| b == b'\n') {
        if nl + 1 < buf.len() {
            buf.drain(..=nl);
        }
    }
    let _ = write_file_atomic(path, &buf);
}

#[cfg(unix)]
fn dir_is_writable(dir: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    if matches!(
        dir.to_str(),
        Some("/bin" | "/sbin" | "/usr/bin" | "/usr/sbin" | "/usr/local/bin" | "/usr/local/sbin")
    ) {
        return false;
    }
    let Ok(cpath) = CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::access(cpath.as_ptr(), libc::W_OK) == 0 }
}

pub fn config_root_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(dir) = executable_dir() {
            return dir;
        }
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }
    #[cfg(not(windows))]
    {
        if let Ok(home) = std::env::var("QWEN_HOME") {
            let trimmed = home.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }
        if let Ok(dir) = executable_dir() {
            #[cfg(unix)]
            if dir_is_writable(&dir) {
                return dir;
            }
            #[cfg(not(unix))]
            return dir;
        }
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            let trimmed = xdg.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join("qwen27");
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            let trimmed = home.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join(".config").join("qwen27");
            }
        }
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }
}

pub fn diag_install() {
    let dir = config_root_dir();
    let log = dir.join("gui-startup.log");
    trim_log_if_oversized(&log, 512 * 1024, 256 * 1024);
}

pub fn diag_line(text: &str) {
    let dir = config_root_dir();
    let _ = fs::create_dir_all(&dir);
    let log = dir.join("gui-startup.log");
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let pid = std::process::id();
    let row = format!("{} pid={} rust-gui {}\n", now, pid, text);
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(log) {
        let _ = f.write_all(row.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_args_and_validation_work() {
        let args = vec![
            "Qwen27.exe".into(),
            "--api".into(),
            "run".into(),
            "--work-mode".into(),
            "coding".into(),
            "--preset".into(),
            "balanced".into(),
            "--attach".into(),
            "a.txt".into(),
            "hello world".into(),
        ];
        let parsed = parse_cli_args(&args).unwrap();
        assert!(!parsed.stdin_mode);
        assert_eq!(parsed.command, "run");
        assert_eq!(parsed.positionals, vec!["hello world"]);
        assert_eq!(parsed.attach_files, vec!["a.txt"]);
        assert_eq!(parsed.gui_patch["workMode"], "coding");
        assert_eq!(parsed.gui_patch["samplingPreset"], "balanced");
        assert_eq!(parsed.server_patch["temperature"], 0.2);

        let ext = vec![
            "Qwen27.exe".into(),
            "--api".into(),
            "conversations".into(),
            "--full".into(),
            "--attach-clipboard".into(),
            "--filter".into(),
            "done".into(),
            "--query".into(),
            "test".into(),
            "--dark-theme".into(),
        ];
        let ext_parsed = parse_cli_args(&ext).unwrap();
        assert!(ext_parsed.full_job);
        assert!(ext_parsed.attach_clipboard);
        assert_eq!(ext_parsed.history_filter, "done");
        assert_eq!(ext_parsed.history_query, "test");
        assert_eq!(ext_parsed.dark_theme, Some(true));
        assert_eq!(ext_parsed.gui_patch["darkTheme"], Value::Bool(true));

        let empty = vec!["Qwen27.exe".into(), "--api".into()];
        assert!(parse_cli_args(&empty).unwrap().stdin_mode);

        let ssh_flag = vec!["Qwen27.exe".into(), "--api".into(), "health".into(), "--tunnel".into()];
        assert!(parse_cli_args(&ssh_flag).is_err());
        let ssh_target = vec![
            "Qwen27.exe".into(),
            "--api".into(),
            "health".into(),
            "--ssh-target".into(),
            "user@192.0.2.10".into(),
        ];
        assert!(parse_cli_args(&ssh_target).is_err());

        let no_tunnel = vec![
            "Qwen27.exe".into(),
            "--api".into(),
            "health".into(),
            "--no-tunnel".into(),
        ];
        assert!(parse_cli_args(&no_tunnel).is_ok());

        let cand_dir = std::env::temp_dir().join(format!("qwen_cand_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&cand_dir);
        fs::write(
            cand_dir.join("https-server.json"),
            r#"{"url":"https://192.0.2.10:8443","urls":["https://192.0.2.10:8443","https://198.51.100.10:8443"]}"#,
        )
        .unwrap();
        let cands = resolve_api_candidates(&cand_dir, "http://127.0.0.1:18090");
        let _ = fs::remove_dir_all(&cand_dir);
        assert_eq!(
            cands,
            vec![
                "https://192.0.2.10:8443".to_string(),
                "https://198.51.100.10:8443".to_string()
            ]
        );
    }

    #[test]
    fn ids_artifacts_and_log_trim_work() {
        assert!(valid_id("12345678-1234-1234-1234-1234567890ab"));
        assert!(!valid_id("12345678-1234-1234-1234-1234567890AB"));
        assert!(safe_artifact("uploads/1-a.txt"));
        assert!(!safe_artifact("uploads/CON.txt"));
        assert!(!safe_artifact("../secret.txt"));

        assert_eq!(
            default_gui_settings().get("darkTheme").and_then(|v| v.as_bool()),
            Some(true)
        );

        let tmp = std::env::temp_dir().join(format!("qwen_trim_test_{}.log", std::process::id()));
        let mut content = String::new();
        for idx in 0..200 {
            content.push_str(&format!("line-{:04}-payload-0123456789\n", idx));
        }
        fs::write(&tmp, &content).unwrap();
        trim_log_if_oversized(&tmp, 1024, 512);
        let after = fs::read_to_string(&tmp).unwrap();
        let _ = fs::remove_file(&tmp);
        assert!(after.len() <= 512);
        assert!(after.starts_with("line-"));
        assert!(!config_root_dir().as_os_str().is_empty());
    }
}
