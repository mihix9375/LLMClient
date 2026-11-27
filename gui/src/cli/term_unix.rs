use super::{StdinKind, CANCEL_JOB, EXIT_SIGNAL, FORCE_ABORT, JOB_ACTIVE, SIGINT_COUNT};
use std::io::{self, IsTerminal};
use std::sync::atomic::Ordering;

pub fn detect_stdin_kind() -> StdinKind {
    if io::stdin().is_terminal() {
        return StdinKind::Tty;
    }
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(0, &mut st) } == 0 {
        let kind = st.st_mode & libc::S_IFMT;
        if kind == libc::S_IFIFO || kind == libc::S_IFREG || kind == libc::S_IFSOCK {
            return StdinKind::Redirected;
        }
    }
    StdinKind::None
}

pub fn stdin_has_available_data(timeout_ms: u64) -> bool {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(0, &mut st) } == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFREG {
        return true;
    }
    let mut pfd = libc::pollfd {
        fd: 0,
        events: libc::POLLIN,
        revents: 0,
    };
    let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms.min(i32::MAX as u64) as libc::c_int) };
    rc > 0 && (pfd.revents & libc::POLLIN) != 0
}

pub struct ConsoleSession;

impl ConsoleSession {
    pub fn attach(_allow_alloc: bool) -> Self {
        Self
    }
}

extern "C" fn unix_sigint(_: libc::c_int) {
    SIGINT_COUNT.fetch_add(1, Ordering::SeqCst);
    if JOB_ACTIVE.load(Ordering::SeqCst) && CANCEL_JOB.swap(true, Ordering::SeqCst) {
        FORCE_ABORT.store(true, Ordering::SeqCst);
    }
}

extern "C" fn unix_sigterm(_: libc::c_int) {
    EXIT_SIGNAL.store(true, Ordering::SeqCst);
    CANCEL_JOB.store(true, Ordering::SeqCst);
    FORCE_ABORT.store(true, Ordering::SeqCst);
}

pub struct SigIntGuard;

pub fn install_sigint_handler() -> SigIntGuard {
    EXIT_SIGNAL.store(false, Ordering::SeqCst);
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = unix_sigint as *const () as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        sa.sa_flags = 0;
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());

        let mut term_sa: libc::sigaction = std::mem::zeroed();
        term_sa.sa_sigaction = unix_sigterm as *const () as usize;
        libc::sigemptyset(&mut term_sa.sa_mask);
        term_sa.sa_flags = 0;
        libc::sigaction(libc::SIGTERM, &term_sa, std::ptr::null_mut());
    }
    SigIntGuard
}

impl Drop for SigIntGuard {
    fn drop(&mut self) {
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = libc::SIG_DFL;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
            libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
        }
    }
}
