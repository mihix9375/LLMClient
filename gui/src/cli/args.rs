use crate::cli::term::StdinKind;
use crate::config::{parse_cli_args, CliArgs};

const SUBCOMMANDS: &[&str] = &[
    "health", "status", "identity", "commands", "windows", "gui-settings", "settings", "configure",
    "preset", "perf", "models", "autotune", "tune-status", "tune-apply", "standby", "fetch", "save",
    "cancel", "answer", "interrupt", "conversations", "conversation", "compact", "memory",
    "schedules", "share", "files", "upload", "classify", "profiles", "adapters", "adapter-profiles",
];

const VALUE_FLAGS: &[&str] = &[
    "-p", "--prompt", "--url", "--api-url", "--ssh-target", "--work-mode", "--mode", "--role",
    "--model-role", "--context", "--tokens", "--max-tokens", "--iterations", "--max-iterations",
    "--effort", "--reasoning-effort", "--budget", "--reasoning-budget", "--preset", "--temp",
    "--temperature", "--top-p", "--top-k", "--min-p", "--repeat-penalty", "--presence-penalty",
    "--frequency-penalty", "--workdir", "--read-dir", "--window", "--attach", "--conv",
    "--conversation", "--parent", "--parent-job", "--file", "--output", "--out", "--path", "--ip",
    "--task", "--tune-mode", "--answer", "--question-id",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvocationMode {
    Help,
    Version,
    Interactive,
    OneShotRun,
    Subcommand(String),
    Gui,
}

#[derive(Clone, Debug)]
pub struct CliInvocation {
    pub mode: InvocationMode,
    pub cli: CliArgs,
    pub prompt: Option<String>,
    pub stream: Option<bool>,
    pub quiet: bool,
    pub save_outputs: bool,
    pub no_color: bool,
    pub stdin_kind: StdinKind,
}

pub fn is_direct_subcommand(cmd: &str) -> bool {
    SUBCOMMANDS.contains(&cmd)
}

fn expand_eq_flags(raw: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(raw.len());
    for (idx, arg) in raw.iter().enumerate() {
        if idx > 0 && arg.starts_with('-') {
            if let Some((flag, val)) = arg.split_once('=') {
                if VALUE_FLAGS.contains(&flag) {
                    out.push(flag.to_string());
                    out.push(val.to_string());
                    continue;
                }
            }
        }
        out.push(arg.clone());
    }
    out
}

pub fn classify_invocation(
    raw_args: &[String],
    stdin_kind: StdinKind,
    has_display: bool,
) -> Result<CliInvocation, String> {
    let args = expand_eq_flags(raw_args);
    let mut forwarded = vec![args.first().cloned().unwrap_or_else(|| "Qwen27".into())];
    let (mut positionals, mut prompt, mut stream) = (Vec::new(), None, None);
    let (mut want_help, mut want_ver, mut want_cli, mut want_gui) = (false, false, false, false);
    let (mut quiet, mut save_outputs, mut no_color, mut explicit_sync) = (false, false, false, false);

    let mut i = 1usize;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-h" | "--help" => want_help = true,
            "-V" | "--version" => want_ver = true,
            "--cli" | "--chat" | "--interactive" | "-i" => want_cli = true,
            "--gui" => want_gui = true,
            "-p" | "--prompt" => {
                if i + 1 >= args.len() { return Err(format!("Missing value for {}", a)); }
                i += 1;
                prompt = Some(args[i].clone());
            }
            "--stream" => stream = Some(true),
            "--no-stream" => stream = Some(false),
            "-q" | "--quiet" => quiet = true,
            "--save" => save_outputs = true,
            "--no-color" => no_color = true,
            "--sync-gui" | "--no-sync-gui" => { explicit_sync = true; forwarded.push(args[i].clone()); }
            "--connect" => forwarded.push("--auto-connect".into()),
            flag if VALUE_FLAGS.contains(&flag) => {
                if i + 1 >= args.len() { return Err(format!("Missing value for {}", flag)); }
                forwarded.push(args[i].clone());
                i += 1;
                forwarded.push(args[i].clone());
            }
            other if !other.starts_with('-') => { positionals.push(args[i].clone()); forwarded.push(args[i].clone()); }
            _ => forwarded.push(args[i].clone()),
        }
        i += 1;
    }

    let first_pos = positionals.first().map(|s| s.as_str());
    let mut cli = parse_cli_args(&forwarded)?;
    if !cli.output_dir.is_empty() { save_outputs = true; }

    let mode = if want_help || first_pos == Some("help") {
        InvocationMode::Help
    } else if want_ver || first_pos == Some("version") {
        InvocationMode::Version
    } else if want_cli || matches!(first_pos, Some("chat" | "cli")) {
        if matches!(first_pos, Some("chat" | "cli")) && cli.command == first_pos.unwrap_or("") { cli.command.clear(); }
        InvocationMode::Interactive
    } else if prompt.is_some() || !cli.goal_file.is_empty() || first_pos == Some("run") {
        InvocationMode::OneShotRun
    } else if let Some(cmd) = first_pos {
        if is_direct_subcommand(cmd) { InvocationMode::Subcommand(cmd.to_string()) }
        else { return Err(format!("不明なコマンドです: '{}' (プロンプトは -p \"...\" または run \"...\" を使用。--help で一覧)", cmd)); }
    } else if want_gui {
        if !has_display { return Err("GUI ディスプレイが見つかりません (DISPLAY / WAYLAND_DISPLAY 未設定)。--cli または -p を使用してください".into()); }
        InvocationMode::Gui
    } else if stdin_kind == StdinKind::Redirected {
        InvocationMode::OneShotRun
    } else if !has_display {
        if stdin_kind == StdinKind::Tty { InvocationMode::Interactive }
        else { return Err("非対話環境のため GUI を起動できません。-p <prompt> またはサブコマンドを指定してください".into()); }
    } else {
        InvocationMode::Gui
    };

    if !explicit_sync {
        cli.sync_gui = matches!(
            &mode,
            InvocationMode::Subcommand(c)
                if matches!(c.as_str(), "configure" | "preset" | "gui-settings" | "tune-apply" | "autotune")
        );
    }

    Ok(CliInvocation { mode, cli, prompt, stream, quiet, save_outputs, no_color, stdin_kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn gui_and_headless_auto_routing() {
        assert_eq!(classify_invocation(&s(&["Qwen27"]), StdinKind::None, true).unwrap().mode, InvocationMode::Gui);
        assert_eq!(classify_invocation(&s(&["Qwen27", "--work-mode", "chat", "--no-connect"]), StdinKind::Tty, true).unwrap().mode, InvocationMode::Gui);
        let h = classify_invocation(&s(&["qwen27", "--work-mode=chat"]), StdinKind::Tty, false).unwrap();
        assert_eq!(h.mode, InvocationMode::Interactive);
        assert_eq!(h.cli.gui_patch["workMode"], "chat");
        assert!(classify_invocation(&s(&["qwen27"]), StdinKind::None, false).is_err());
    }

    #[test]
    fn interactive_oneshot_piped_and_subcommands() {
        for flag in ["--cli", "--chat", "--interactive", "-i", "chat", "cli"] {
            let r = classify_invocation(&s(&["qwen27", flag]), StdinKind::Tty, true).unwrap();
            assert_eq!(r.mode, InvocationMode::Interactive);
            assert!(!r.cli.sync_gui);
        }
        let p = classify_invocation(&s(&["qwen27", "-p", "こんにちは", "--work-mode=chat", "--no-stream", "-q"]), StdinKind::Tty, true).unwrap();
        assert_eq!(p.mode, InvocationMode::OneShotRun);
        assert_eq!(p.prompt.as_deref(), Some("こんにちは"));
        assert_eq!(p.stream, Some(false));
        assert!(p.quiet && !p.cli.sync_gui);

        assert_eq!(classify_invocation(&s(&["qwen27"]), StdinKind::Redirected, true).unwrap().mode, InvocationMode::OneShotRun);
        let sub = classify_invocation(&s(&["qwen27", "health", "--json"]), StdinKind::Tty, true).unwrap();
        assert_eq!(sub.mode, InvocationMode::Subcommand("health".into()));
        assert!(sub.cli.json_out);
        assert!(classify_invocation(&s(&["qwen27", "configure", "--effort", "low"]), StdinKind::Tty, true).unwrap().cli.sync_gui);
        assert!(!classify_invocation(&s(&["qwen27", "configure", "--effort", "low", "--no-sync-gui"]), StdinKind::Tty, true).unwrap().cli.sync_gui);
        assert!(classify_invocation(&s(&["qwen27", "helth"]), StdinKind::Tty, true).is_err());
        assert!(classify_invocation(&s(&["qwen27", "--unknown-flag"]), StdinKind::Tty, true).is_err());
    }
}
