use super::{StdinKind, CANCEL_JOB, EXIT_SIGNAL, FORCE_ABORT, JOB_ACTIVE, SIGINT_COUNT};
use std::ffi::OsStr;
use std::io::{self, Write};
use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetFileType, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_TYPE_CHAR, FILE_TYPE_DISK, FILE_TYPE_PIPE, OPEN_EXISTING,
};
use windows::Win32::System::Console::{
    AllocConsole, AttachConsole, FreeConsole, GetConsoleCP, GetConsoleMode, GetConsoleOutputCP,
    GetStdHandle, SetConsoleCP, SetConsoleCtrlHandler, SetConsoleMode, SetConsoleOutputCP,
    SetStdHandle, ATTACH_PARENT_PROCESS, CONSOLE_MODE, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT,
    CTRL_C_EVENT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, STD_ERROR_HANDLE, STD_HANDLE,
    STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows::Win32::System::Pipes::PeekNamedPipe;

fn is_valid_std_handle(h: HANDLE) -> bool {
    if h.is_invalid() || h.0.is_null() { return false; }
    let ft = unsafe { GetFileType(h) };
    ft == FILE_TYPE_DISK || ft == FILE_TYPE_CHAR || ft == FILE_TYPE_PIPE
}

pub fn detect_stdin_kind() -> StdinKind {
    let Ok(h) = (unsafe { GetStdHandle(STD_INPUT_HANDLE) }) else { return StdinKind::None; };
    if h.is_invalid() || h.0.is_null() { return StdinKind::None; }
    let ft = unsafe { GetFileType(h) };
    if ft == FILE_TYPE_PIPE || ft == FILE_TYPE_DISK { return StdinKind::Redirected; }
    if ft == FILE_TYPE_CHAR {
        let mut mode = CONSOLE_MODE(0);
        if unsafe { GetConsoleMode(h, &mut mode) }.is_ok() { return StdinKind::Tty; }
    }
    StdinKind::None
}

pub fn stdin_has_available_data(timeout_ms: u64) -> bool {
    let Ok(h) = (unsafe { GetStdHandle(STD_INPUT_HANDLE) }) else { return false; };
    if h.is_invalid() || h.0.is_null() { return false; }
    let ft = unsafe { GetFileType(h) };
    if ft == FILE_TYPE_DISK { return true; }
    if ft != FILE_TYPE_PIPE { return false; }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let mut avail = 0u32;
        if unsafe { PeekNamedPipe(h, None, 0, None, Some(&mut avail), None) }.is_err() {
            return false;
        }
        if avail > 0 { return true; }
        if Instant::now() >= deadline { return false; }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn open_console_device(name: &str) -> Option<HANDLE> {
    let wide: Vec<u16> = OsStr::new(name).encode_wide().chain(Some(0)).collect();
    unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
        .ok()
    }
}

pub struct ConsoleSession {
    allocated: bool,
    saved_cp: Option<(u32, u32)>,
    saved_modes: Vec<(HANDLE, CONSOLE_MODE)>,
    rebound: Vec<(STD_HANDLE, HANDLE, HANDLE)>,
}

impl ConsoleSession {
    pub fn attach(allow_alloc: bool) -> Self {
        let mut sess = Self { allocated: false, saved_cp: None, saved_modes: Vec::new(), rebound: Vec::new() };
        let specs = [(STD_INPUT_HANDLE, "CONIN$"), (STD_OUTPUT_HANDLE, "CONOUT$"), (STD_ERROR_HANDLE, "CONOUT$")];
        let origs: Vec<(STD_HANDLE, &str, HANDLE, bool)> = specs
            .iter()
            .map(|&(id, dev)| {
                let h = unsafe { GetStdHandle(id) }.unwrap_or_default();
                (id, dev, h, is_valid_std_handle(h))
            })
            .collect();

        if origs.iter().any(|x| !x.3) {
            let mut have = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_ok();
            if !have && allow_alloc && unsafe { AllocConsole() }.is_ok() {
                have = true;
                sess.allocated = true;
            }
            if have {
                for &(id, dev, orig, had) in &origs {
                    if had {
                        let _ = unsafe { SetStdHandle(id, orig) };
                    } else if let Some(new_h) = open_console_device(dev) {
                        if unsafe { SetStdHandle(id, new_h) }.is_ok() { sess.rebound.push((id, orig, new_h)); }
                        else { let _ = unsafe { CloseHandle(new_h) }; }
                    }
                }
            }
        }

        let mut any_console = false;
        for id in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            if let Ok(h) = unsafe { GetStdHandle(id) } {
                let mut mode = CONSOLE_MODE(0);
                if is_valid_std_handle(h) && unsafe { GetConsoleMode(h, &mut mode) }.is_ok() {
                    any_console = true;
                    let next = CONSOLE_MODE(mode.0 | ENABLE_VIRTUAL_TERMINAL_PROCESSING.0);
                    if next != mode && unsafe { SetConsoleMode(h, next) }.is_ok() { sess.saved_modes.push((h, mode)); }
                }
            }
        }
        if any_console {
            if !sess.allocated { sess.saved_cp = Some((unsafe { GetConsoleCP() }, unsafe { GetConsoleOutputCP() })); }
            let _ = unsafe { SetConsoleOutputCP(65001) };
            let _ = unsafe { SetConsoleCP(65001) };
        }
        sess
    }
}

impl Drop for ConsoleSession {
    fn drop(&mut self) {
        let _ = io::stdout().flush();
        let _ = io::stderr().flush();
        for &(h, mode) in &self.saved_modes { let _ = unsafe { SetConsoleMode(h, mode) }; }
        if let Some((in_cp, out_cp)) = self.saved_cp {
            if in_cp != 0 { let _ = unsafe { SetConsoleCP(in_cp) }; }
            if out_cp != 0 { let _ = unsafe { SetConsoleOutputCP(out_cp) }; }
        }
        for &(id, orig, opened) in &self.rebound {
            let _ = unsafe { SetStdHandle(id, orig) };
            let _ = unsafe { CloseHandle(opened) };
        }
        if self.allocated { let _ = unsafe { FreeConsole() }; }
    }
}

unsafe extern "system" fn ctrl_handler(ctrl_type: u32) -> windows::core::BOOL {
    if ctrl_type == CTRL_C_EVENT || ctrl_type == CTRL_BREAK_EVENT {
        SIGINT_COUNT.fetch_add(1, Ordering::SeqCst);
        if JOB_ACTIVE.load(Ordering::SeqCst) && CANCEL_JOB.swap(true, Ordering::SeqCst) {
            FORCE_ABORT.store(true, Ordering::SeqCst);
        }
        true.into()
    } else if ctrl_type == CTRL_CLOSE_EVENT {
        EXIT_SIGNAL.store(true, Ordering::SeqCst);
        CANCEL_JOB.store(true, Ordering::SeqCst);
        FORCE_ABORT.store(true, Ordering::SeqCst);
        false.into()
    } else {
        false.into()
    }
}

pub struct SigIntGuard;

pub fn install_sigint_handler() -> SigIntGuard {
    EXIT_SIGNAL.store(false, Ordering::SeqCst);
    let _ = unsafe { SetConsoleCtrlHandler(Some(ctrl_handler), true) };
    SigIntGuard
}

impl Drop for SigIntGuard {
    fn drop(&mut self) { let _ = unsafe { SetConsoleCtrlHandler(Some(ctrl_handler), false) }; }
}
