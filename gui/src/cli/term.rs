use std::io::{self, IsTerminal};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
#[path = "term_unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "term_win.rs"]
mod platform;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StdinKind {
    Tty,
    Redirected,
    None,
}

pub(super) static SIGINT_COUNT: AtomicU64 = AtomicU64::new(0);
pub(super) static JOB_ACTIVE: AtomicBool = AtomicBool::new(false);
pub(super) static CANCEL_JOB: AtomicBool = AtomicBool::new(false);
pub(super) static FORCE_ABORT: AtomicBool = AtomicBool::new(false);
pub(super) static EXIT_SIGNAL: AtomicBool = AtomicBool::new(false);
static LAST_IDLE_MS: AtomicU64 = AtomicU64::new(0);

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn set_job_active(active: bool) {
    if active {
        CANCEL_JOB.store(false, Ordering::SeqCst);
        FORCE_ABORT.store(false, Ordering::SeqCst);
    }
    JOB_ACTIVE.store(active, Ordering::SeqCst);
}

pub fn sigint_count() -> u64 {
    SIGINT_COUNT.load(Ordering::SeqCst)
}

pub fn is_cancel_job() -> bool {
    CANCEL_JOB.load(Ordering::SeqCst)
}

pub fn is_force_abort() -> bool {
    FORCE_ABORT.load(Ordering::SeqCst)
}

pub fn is_exit_signal() -> bool {
    EXIT_SIGNAL.load(Ordering::SeqCst)
}

pub fn should_exit_on_idle_sigint() -> bool {
    let now = epoch_ms();
    let prev = LAST_IDLE_MS.swap(now, Ordering::SeqCst);
    prev > 0 && now.saturating_sub(prev) <= 1500
}

pub fn reset_idle_sigint() {
    LAST_IDLE_MS.store(0, Ordering::SeqCst);
}

pub fn has_gui_display() -> bool {
    #[cfg(target_os = "linux")]
    {
        let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
        set("DISPLAY") || set("WAYLAND_DISPLAY")
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

pub fn use_ansi_stderr(no_color: bool) -> bool {
    !no_color && std::env::var_os("NO_COLOR").is_none() && io::stderr().is_terminal()
}

pub fn is_interrupted_io(err: &io::Error) -> bool {
    if err.kind() == io::ErrorKind::Interrupted {
        return true;
    }
    #[cfg(windows)]
    {
        err.raw_os_error() == Some(995)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub use platform::{
    detect_stdin_kind, install_sigint_handler, stdin_has_available_data, ConsoleSession,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn stdin_probe_never_blocks_and_idle_sigint_debounces() {
        let start = Instant::now();
        let _ = stdin_has_available_data(20);
        assert!(start.elapsed() < Duration::from_millis(500));
        reset_idle_sigint();
        assert!(!should_exit_on_idle_sigint());
        assert!(should_exit_on_idle_sigint());
        reset_idle_sigint();
    }
}
