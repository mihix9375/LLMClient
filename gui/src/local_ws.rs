use crate::text_edit::{apply_text_edit, text_lines, TextEditError};
#[cfg(windows)]
use crate::win_util::{
    computer_name, from_wide_nul, is_current_process_elevated, no_links, quote_win_arg, to_wide_null,
};
#[cfg(not(windows))]
use crate::win_util::{computer_name, is_current_process_elevated, no_links};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
#[cfg(not(windows))]
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(windows)]
use std::mem::size_of;
use std::path::{Component, Path, PathBuf};
#[cfg(windows)]
use std::ptr::null;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
#[cfg(windows)]
use windows::core::{PCWSTR, PWSTR};
#[cfg(windows)]
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, FILETIME, GENERIC_READ,
    GENERIC_WRITE, HANDLE, WAIT_OBJECT_0,
};
#[cfg(windows)]
use windows::Win32::Security::SECURITY_ATTRIBUTES;
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, GetFileAttributesExW, GetFileAttributesW, GetFileSizeEx,
    GetFileType, MoveFileExW, ReadFile, ReplaceFileW, SetFilePointerEx, WriteFile, CREATE_NEW,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_BEGIN, FILE_FLAG_SEQUENTIAL_SCAN,
    FILE_FLAG_WRITE_THROUGH, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_TYPE_DISK, GetFileExInfoStandard,
    INVALID_FILE_ATTRIBUTES, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, OPEN_EXISTING,
    REPLACEFILE_IGNORE_MERGE_ERRORS, SearchPathW, WIN32_FILE_ATTRIBUTE_DATA,
};
#[cfg(windows)]
use windows::Win32::Foundation::{HANDLE_FLAGS, HANDLE_FLAG_INHERIT};
#[cfg(windows)]
use windows::Win32::Foundation::SetHandleInformation;
#[cfg(windows)]
use windows::Win32::System::SystemInformation::{GetSystemDirectoryW, GetSystemTimeAsFileTime};
#[cfg(windows)]
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
#[cfg(windows)]
use windows::Win32::System::Pipes::{CreatePipe, PeekNamedPipe};
#[cfg(windows)]
use windows::Win32::System::Threading::{
    CreateProcessW, GetCurrentProcessId, GetExitCodeProcess, ResumeThread, TerminateProcess,
    WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED, PROCESS_INFORMATION,
    STARTF_USESTDHANDLES, STARTUPINFOW,
};

const K_CHUNK: usize = 1_048_576;
const K_FETCH_MAX: u64 = 8_388_608;
static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

#[cfg(windows)]
struct OwnedHandle(HANDLE);
#[cfg(windows)]
impl OwnedHandle {
    fn take(&mut self) -> HANDLE {
        let h = self.0;
        self.0 = HANDLE::default();
        h
    }
}
#[cfg(windows)]
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() && !self.0 .0.is_null() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

fn match_wildcard(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().flat_map(|c| c.to_lowercase()).collect();
    let t: Vec<char> = text.chars().flat_map(|c| c.to_lowercase()).collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut match_idx = 0usize;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            pi += 1;
            match_idx = ti;
        } else if let Some(s) = star {
            pi = s + 1;
            match_idx += 1;
            ti = match_idx;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn lexically_normal(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(
                    out.components().next_back(),
                    Some(Component::Prefix(_)) | Some(Component::RootDir) | None
                ) {
                    out.pop();
                }
            }
            c => out.push(c),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

fn beneath(root: &Path, file: &Path) -> bool {
    let mut a = root.components();
    let mut b = file.components();
    loop {
        match (a.next(), b.next()) {
            (None, _) => return true,
            (Some(_), None) => return false,
            (Some(ca), Some(cb)) => {
                #[cfg(windows)]
                {
                    let sa = ca.as_os_str().to_string_lossy().to_ascii_lowercase();
                    let sb = cb.as_os_str().to_string_lossy().to_ascii_lowercase();
                    if sa != sb {
                        return false;
                    }
                }
                #[cfg(not(windows))]
                {
                    if ca.as_os_str() != cb.as_os_str() {
                        return false;
                    }
                }
            }
        }
    }
}

#[cfg(windows)]
fn cleanup_temps_in(root: &Path) {
    let ft_now: FILETIME = unsafe { GetSystemTimeAsFileTime() };
    let now_u64 = ((ft_now.dwHighDateTime as u64) << 32) | (ft_now.dwLowDateTime as u64);
    let limit = now_u64.saturating_sub(36_000_000_000);
    let mut visited = 0usize;
    fn walk(dir: &Path, depth: usize, visited: &mut usize, limit: u64) {
        let Ok(rd) = fs::read_dir(dir) else {
            return;
        };
        for ent in rd.flatten() {
            *visited += 1;
            if *visited > 20_000 {
                return;
            }
            let p = ent.path();
            let name = p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
            let wide = to_wide_null(&p.to_string_lossy());
            let mut data = WIN32_FILE_ATTRIBUTE_DATA::default();
            let ok = unsafe {
                GetFileAttributesExW(
                    PCWSTR(wide.as_ptr()),
                    GetFileExInfoStandard,
                    (&mut data as *mut WIN32_FILE_ATTRIBUTE_DATA).cast(),
                )
            }
            .is_ok();
            if !ok || (data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0) != 0 {
                continue;
            }
            if (data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0) != 0 {
                if depth < 12 && !matches!(name.as_ref(), ".git" | "node_modules" | "target" | "build" | "dist" | ".venv" | "__pycache__") {
                    walk(&p, depth + 1, visited, limit);
                }
                continue;
            }
            if !name.contains(".qwen-tmp-") {
                continue;
            }
            let t = ((data.ftLastWriteTime.dwHighDateTime as u64) << 32)
                | (data.ftLastWriteTime.dwLowDateTime as u64);
            if t < limit {
                let _ = fs::remove_file(&p);
            }
        }
    }
    walk(root, 0, &mut visited, limit);
}

#[cfg(not(windows))]
fn cleanup_temps_in(root: &Path) {
    let now = std::time::SystemTime::now();
    let max_age = Duration::from_secs(3600);
    let mut visited = 0usize;
    fn walk(dir: &Path, depth: usize, visited: &mut usize, now: std::time::SystemTime, max_age: Duration) {
        let Ok(rd) = fs::read_dir(dir) else {
            return;
        };
        for ent in rd.flatten() {
            *visited += 1;
            if *visited > 20_000 {
                return;
            }
            let p = ent.path();
            let name = p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
            let Ok(meta) = fs::symlink_metadata(&p) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if depth < 12 && !matches!(name.as_ref(), ".git" | "node_modules" | "target" | "build" | "dist" | ".venv" | "__pycache__") {
                    walk(&p, depth + 1, visited, now, max_age);
                }
                continue;
            }
            if !name.contains(".qwen-tmp-") {
                continue;
            }
            if let Ok(mtime) = meta.modified() {
                if now.duration_since(mtime).map(|age| age > max_age).unwrap_or(false) {
                    let _ = fs::remove_file(&p);
                }
            }
        }
    }
    walk(root, 0, &mut visited, now, max_age);
}

fn device_like(part: &str) -> bool {
    if part.is_empty() || part.ends_with('.') || part.ends_with(' ') {
        return true;
    }
    let upper = part.to_ascii_uppercase();
    let stem = upper.split('.').next().unwrap_or("");
    matches!(
        stem,
        "CON" | "PRN" | "AUX" | "NUL"
            | "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9"
            | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9"
    )
}

fn sha256_lower_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let dig = h.finalize();
    let mut out = String::with_capacity(64);
    for b in dig {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

#[cfg(windows)]
fn sha256_file_hex(file: &Path) -> Result<String, String> {
    let wide = to_wide_null(&file.to_string_lossy());
    let h = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_SEQUENTIAL_SCAN,
            None,
        )
    }
    .map_err(|_| "置き換え前のファイルを読めません".to_string())?;
    let _guard = OwnedHandle(h);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 262_144];
    loop {
        let mut read = 0u32;
        unsafe { ReadFile(h, Some(&mut buf), Some(&mut read), None) }
            .map_err(|_| "置き換え前のファイルを読めません".to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read as usize]);
    }
    let dig = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in dig {
        out.push_str(&format!("{:02x}", b));
    }
    Ok(out)
}

#[cfg(not(windows))]
fn sha256_file_hex(file: &Path) -> Result<String, String> {
    let mut f = fs::File::open(file).map_err(|_| "置き換え前のファイルを読めません".to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 262_144];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|_| "置き換え前のファイルを読めません".to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let dig = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in dig {
        out.push_str(&format!("{:02x}", b));
    }
    Ok(out)
}

fn read_text_file(file: &Path) -> Result<String, String> {
    let meta = fs::metadata(file).map_err(|_| "ファイルを開けません".to_string())?;
    if !meta.is_file() || meta.len() > 65536 {
        return Err("読み込みは64KiB以内のテキストファイルに限ります".into());
    }
    let bytes = fs::read(file).map_err(|_| "ファイルを開けません".to_string())?;
    if bytes.is_empty() {
        return Ok(String::new());
    }
    let mut cp = 65001u32;
    let mut offset = 0usize;
    if bytes.starts_with(&[0xFF, 0xFE]) {
        cp = 1200;
        offset = 2;
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        cp = 1201;
        offset = 2;
    } else if !bytes.starts_with(&[0xEF, 0xBB, 0xBF]) && bytes.len() >= 8 {
        let (mut odd, mut even) = (0usize, 0usize);
        for (i, &b) in bytes.iter().enumerate() {
            if b == 0 {
                if i % 2 == 1 {
                    odd += 1;
                } else {
                    even += 1;
                }
            }
        }
        if odd * 4 >= bytes.len() && even * 8 < bytes.len() {
            cp = 1200;
        } else if even * 4 >= bytes.len() && odd * 8 < bytes.len() {
            cp = 1201;
        }
    }
    let slice = &bytes[offset..];
    if slice.is_empty() {
        return Ok(String::new());
    }
    if cp == 65001 {
        if slice.contains(&0) {
            return Err("バイナリまたは未対応の文字コードです。NULを含むため読み込みません".into());
        }
        let s = std::str::from_utf8(slice).map_err(|_| {
            "テキストをUTF-8として読めません。バイナリまたは未対応の文字コードです".to_string()
        })?;
        return Ok(s.to_string());
    }
    if slice.len() % 2 != 0 {
        return Err("文字コードが不正です（UTF-16の長さが奇数です）".into());
    }
    let mut units = Vec::with_capacity(slice.len() / 2);
    for pair in slice.chunks_exact(2) {
        let u = if cp == 1200 {
            u16::from_le_bytes([pair[0], pair[1]])
        } else {
            u16::from_be_bytes([pair[0], pair[1]])
        };
        if u == 0 {
            return Err("バイナリデータです。NULを含むためテキストとして読み込みません".into());
        }
        units.push(u);
    }
    String::from_utf16(&units).map_err(|_| "UTF-16のサロゲート文字が不正です".to_string())
}

fn clip_utf8_prefix(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn iso_now_utc() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(windows)]
fn search_binary_path(exe_name: &str) -> Option<String> {
    let wname = to_wide_null(exe_name);
    let mut buf = [0u16; 260];
    let len = unsafe { SearchPathW(PCWSTR::null(), PCWSTR(wname.as_ptr()), PCWSTR::null(), Some(&mut buf), None) };
    if len > 0 && (len as usize) < buf.len() {
        Some(from_wide_nul(&buf[..len as usize]))
    } else {
        None
    }
}

#[cfg(not(windows))]
fn search_binary_path(exe_name: &str) -> Option<String> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let cand = dir.join(exe_name);
        if let Ok(meta) = fs::metadata(&cand) {
            if meta.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if meta.permissions().mode() & 0o111 == 0 {
                        continue;
                    }
                }
                return Some(cand.to_string_lossy().into_owned());
            }
        }
    }
    None
}

pub fn compilers_info() -> Value {
    #[cfg(windows)]
    {
        json!({
            "g++": search_binary_path("g++.exe"),
            "clang++": search_binary_path("clang++.exe"),
            "cl": search_binary_path("cl.exe"),
        })
    }
    #[cfg(not(windows))]
    {
        json!({
            "g++": search_binary_path("g++"),
            "clang++": search_binary_path("clang++"),
            "cc": search_binary_path("cc"),
        })
    }
}

pub fn has_winget() -> bool {
    #[cfg(windows)]
    {
        search_binary_path("winget.exe").is_some()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
#[derive(Clone, Debug)]
struct PowerShellInfo {
    bin: PathBuf,
    is_pwsh7: bool,
    label: String,
    major: u32,
    version: Option<String>,
}

#[cfg(windows)]
static RESOLVED_POWERSHELL: OnceLock<PowerShellInfo> = OnceLock::new();

#[cfg(windows)]
fn path_file_attr_exists(path: &Path) -> bool {
    let wide = to_wide_null(&path.to_string_lossy());
    let attr = unsafe { GetFileAttributesW(PCWSTR(wide.as_ptr())) };
    attr != INVALID_FILE_ATTRIBUTES && (attr & FILE_ATTRIBUTE_DIRECTORY.0) == 0
}

#[cfg(windows)]
fn parse_pwsh_version(stdout: &str) -> Option<(u32, String)> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let first = trimmed.split('.').next().unwrap_or("");
        if let Ok(major) = first.parse::<u32>() {
            if major >= 7 {
                return Some((major, trimmed.to_string()));
            }
        }
    }
    None
}

#[cfg(windows)]
fn pwsh_candidates() -> Vec<PathBuf> {
    let mut raw = Vec::new();
    let mut push_pf = |base: &str| {
        if !base.is_empty() {
            raw.push(PathBuf::from(base).join("PowerShell").join("7").join("pwsh.exe"));
            raw.push(
                PathBuf::from(base)
                    .join("PowerShell")
                    .join("7-preview")
                    .join("pwsh.exe"),
            );
        }
    };
    if let Ok(pf) = std::env::var("ProgramFiles") {
        push_pf(&pf);
    }
    push_pf(r"C:\Program Files");
    if let Ok(local_app) = std::env::var("LOCALAPPDATA") {
        if !local_app.is_empty() {
            raw.push(
                PathBuf::from(&local_app)
                    .join("Microsoft")
                    .join("WindowsApps")
                    .join("pwsh.exe"),
            );
        }
    }
    if let Some(found) = search_binary_path("pwsh.exe") {
        raw.push(PathBuf::from(found));
    }
    let mut deduped = Vec::new();
    for path in raw {
        let key = path.to_string_lossy().to_ascii_lowercase();
        if !deduped
            .iter()
            .any(|existing: &PathBuf| existing.to_string_lossy().to_ascii_lowercase() == key)
        {
            deduped.push(path);
        }
    }
    deduped
}

#[cfg(windows)]
fn probe_pwsh_candidate(bin: &Path) -> Option<(u32, String)> {
    if !path_file_attr_exists(bin) {
        return None;
    }
    let sec = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null::<core::ffi::c_void>() as *mut core::ffi::c_void,
        bInheritHandle: true.into(),
    };
    let mut out_read_raw = HANDLE::default();
    let mut out_write_raw = HANDLE::default();
    unsafe { CreatePipe(&mut out_read_raw, &mut out_write_raw, Some(&sec), 4096) }.ok()?;
    let out_read = OwnedHandle(out_read_raw);
    let mut out_write = OwnedHandle(out_write_raw);
    unsafe {
        let _ = SetHandleInformation(out_read.0, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0));
    }

    let nul_w = to_wide_null("NUL");
    let nul_raw = unsafe {
        CreateFileW(
            PCWSTR(nul_w.as_ptr()),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            Some(&sec),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .ok()?;
    let nul_guard = OwnedHandle(nul_raw);

    let job_raw = unsafe { CreateJobObjectW(None, PCWSTR::null()) }.ok()?;
    let job_guard = OwnedHandle(job_raw);
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
    unsafe {
        SetInformationJobObject(
            job_guard.0,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    }
    .ok()?;

    let cmd_line = format!(
        "{} -NoLogo -NoProfile -NonInteractive -Command {}",
        quote_win_arg(&bin.to_string_lossy()),
        quote_win_arg("$PSVersionTable.PSVersion.ToString()")
    );
    let bin_w = to_wide_null(&bin.to_string_lossy());
    let mut cmd_w = to_wide_null(&cmd_line);

    let mut si = STARTUPINFOW::default();
    si.cb = size_of::<STARTUPINFOW>() as u32;
    si.dwFlags = STARTF_USESTDHANDLES;
    si.hStdInput = nul_guard.0;
    si.hStdOutput = out_write.0;
    si.hStdError = nul_guard.0;
    let mut pi = PROCESS_INFORMATION::default();

    unsafe {
        CreateProcessW(
            PCWSTR(bin_w.as_ptr()),
            Some(PWSTR(cmd_w.as_mut_ptr())),
            None,
            None,
            true,
            CREATE_NO_WINDOW | CREATE_SUSPENDED,
            None,
            PCWSTR::null(),
            &si,
            &mut pi,
        )
    }
    .ok()?;

    let proc_guard = OwnedHandle(pi.hProcess);
    let thread_guard = OwnedHandle(pi.hThread);
    if unsafe { AssignProcessToJobObject(job_guard.0, proc_guard.0) }.is_err() {
        unsafe {
            let _ = TerminateProcess(proc_guard.0, 1);
        }
        return None;
    }
    unsafe {
        let _ = ResumeThread(thread_guard.0);
    }
    let ow = out_write.take();
    unsafe {
        let _ = CloseHandle(ow);
    }

    let mut out_bytes = Vec::<u8>::new();
    let start = Instant::now();
    loop {
        let done = unsafe { WaitForSingleObject(proc_guard.0, 20) } == WAIT_OBJECT_0;
        loop {
            let mut avail = 0u32;
            if unsafe { PeekNamedPipe(out_read.0, None, 0, None, Some(&mut avail), None) }.is_err()
                || avail == 0
            {
                break;
            }
            let mut buf = [0u8; 1024];
            let to_read = (avail as usize).min(buf.len());
            let mut count = 0u32;
            if unsafe { ReadFile(out_read.0, Some(&mut buf[..to_read]), Some(&mut count), None) }
                .is_err()
                || count == 0
            {
                break;
            }
            let room = 4096usize.saturating_sub(out_bytes.len());
            out_bytes.extend_from_slice(&buf[..(count as usize).min(room)]);
        }
        if done {
            break;
        }
        if start.elapsed() > Duration::from_secs(10) {
            unsafe {
                let _ = TerminateJobObject(job_guard.0, 1);
            }
            return None;
        }
    }

    let mut exit_code = 1u32;
    unsafe {
        let _ = GetExitCodeProcess(proc_guard.0, &mut exit_code);
    }
    if exit_code != 0 {
        return None;
    }
    parse_pwsh_version(&String::from_utf8_lossy(&out_bytes))
}

#[cfg(windows)]
fn fallback_powershell_51() -> PowerShellInfo {
    let mut sys_buf = [0u16; 260];
    let slen = unsafe { GetSystemDirectoryW(Some(&mut sys_buf)) } as usize;
    let ps_bin = PathBuf::from(from_wide_nul(&sys_buf[..slen.min(sys_buf.len())]))
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    PowerShellInfo {
        bin: ps_bin,
        is_pwsh7: false,
        label: "PowerShell 5.1".to_string(),
        major: 5,
        version: Some("5.1".to_string()),
    }
}

#[cfg(windows)]
fn resolve_powershell_info() -> PowerShellInfo {
    RESOLVED_POWERSHELL
        .get_or_init(|| {
            for candidate in pwsh_candidates() {
                if let Some((major, version)) = probe_pwsh_candidate(&candidate) {
                    return PowerShellInfo {
                        bin: candidate,
                        is_pwsh7: true,
                        label: format!("PowerShell {}", major),
                        major,
                        version: Some(version),
                    };
                }
            }
            fallback_powershell_51()
        })
        .clone()
}

#[cfg(not(windows))]
#[derive(Clone, Debug)]
struct UnixShellInfo {
    bin: PathBuf,
    label: String,
    major: u32,
    version: Option<String>,
}

#[cfg(not(windows))]
static RESOLVED_UNIX_SHELL: OnceLock<UnixShellInfo> = OnceLock::new();

#[cfg(not(windows))]
fn resolve_unix_shell_info() -> UnixShellInfo {
    RESOLVED_UNIX_SHELL
        .get_or_init(|| {
            let bash_path = if Path::new("/bin/bash").is_file() {
                Some(PathBuf::from("/bin/bash"))
            } else {
                search_binary_path("bash").map(PathBuf::from)
            };
            if let Some(bin) = bash_path {
                let mut major = 5u32;
                let mut version = None;
                if let Ok(out) = std::process::Command::new(&bin)
                    .args(["-c", "printf '%s' \"$BASH_VERSION\""])
                    .output()
                {
                    if out.status.success() {
                        let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
                        if !raw.is_empty() {
                            if let Some(m) = raw
                                .split('.')
                                .next()
                                .and_then(|s| s.parse::<u32>().ok())
                            {
                                major = m;
                            }
                            version = Some(raw);
                        }
                    }
                }
                return UnixShellInfo {
                    bin,
                    label: "bash".to_string(),
                    major,
                    version,
                };
            }
            UnixShellInfo {
                bin: PathBuf::from("/bin/sh"),
                label: "sh".to_string(),
                major: 1,
                version: None,
            }
        })
        .clone()
}

pub fn resolve_powershell_bin() -> (PathBuf, bool, String) {
    #[cfg(windows)]
    {
        let info = resolve_powershell_info();
        (info.bin, info.is_pwsh7, info.label)
    }
    #[cfg(not(windows))]
    {
        let info = resolve_unix_shell_info();
        (info.bin, false, info.label)
    }
}

fn resolve_powershell_version() -> (u32, Option<String>) {
    #[cfg(windows)]
    {
        let info = resolve_powershell_info();
        (info.major, info.version)
    }
    #[cfg(not(windows))]
    {
        let info = resolve_unix_shell_info();
        (info.major, info.version)
    }
}

#[cfg(windows)]
fn entry_list_meta(p: &Path, _name: &str) -> (bool, &'static str) {
    let wp = to_wide_null(&p.to_string_lossy());
    let attr = unsafe { GetFileAttributesW(PCWSTR(wp.as_ptr())) };
    let is_hidden = attr != INVALID_FILE_ATTRIBUTES && (attr & FILE_ATTRIBUTE_HIDDEN.0) != 0;
    let kind = if attr != INVALID_FILE_ATTRIBUTES && (attr & FILE_ATTRIBUTE_REPARSE_POINT.0) != 0 {
        "symlink"
    } else if p.is_dir() {
        "directory"
    } else {
        "file"
    };
    (is_hidden, kind)
}

#[cfg(not(windows))]
fn entry_list_meta(p: &Path, name: &str) -> (bool, &'static str) {
    let is_hidden = name.starts_with('.');
    let kind = match fs::symlink_metadata(p) {
        Ok(m) if m.file_type().is_symlink() => "symlink",
        Ok(m) if m.is_dir() => "directory",
        _ => "file",
    };
    (is_hidden, kind)
}

#[cfg(windows)]
fn is_non_link_existing(p: &Path) -> bool {
    let wp = to_wide_null(&p.to_string_lossy());
    let attr = unsafe { GetFileAttributesW(PCWSTR(wp.as_ptr())) };
    attr != INVALID_FILE_ATTRIBUTES && (attr & FILE_ATTRIBUTE_REPARSE_POINT.0) == 0
}

#[cfg(not(windows))]
fn is_non_link_existing(p: &Path) -> bool {
    fs::symlink_metadata(p)
        .map(|m| !m.file_type().is_symlink())
        .unwrap_or(false)
}

pub struct LocalWorkspace {
    root: PathBuf,
    read_roots: Vec<PathBuf>,
}

impl LocalWorkspace {
    pub fn new(root: PathBuf, read_roots: Vec<PathBuf>) -> Result<Self, String> {
        let abs = if root.is_absolute() {
            root
        } else {
            std::env::current_dir()
                .map_err(|e| e.to_string())?
                .join(root)
        };
        let norm_root = lexically_normal(&abs);
        no_links(&norm_root)?;
        fs::create_dir_all(&norm_root).map_err(|e| format!("作業フォルダーを作れません: {}", e))?;
        let mut canon_reads = Vec::new();
        for folder in read_roots {
            no_links(&folder)?;
            let canon = fs::canonicalize(&folder)
                .map_err(|_| "資料フォルダーがありません".to_string())?;
            if !canon.is_dir() {
                return Err("資料フォルダーがありません".into());
            }
            canon_reads.push(crate::config::strip_unc_prefix(canon));
        }
        let ws = Self {
            root: crate::config::strip_unc_prefix(norm_root),
            read_roots: canon_reads,
        };
        let cleanup_root = ws.root.clone();
        std::thread::spawn(move || cleanup_temps_in(&cleanup_root));
        Ok(ws)
    }

    fn resolve_path(&self, relative: &str, directory: bool, read_only: bool) -> Result<PathBuf, String> {
        if relative.is_empty() {
            return Err("無効なパスです".into());
        }
        let input = PathBuf::from(relative);
        if relative.contains(':') && !input.is_absolute() {
            return Err("無効なパスです".into());
        }
        if input
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            return Err("親フォルダーへの移動は許可されていません".into());
        }
        let joined = if input.is_absolute() {
            input
        } else {
            self.root.join(input)
        };
        let file = crate::config::strip_unc_prefix(lexically_normal(&joined));
        let mut allowed = beneath(&self.root, &file);
        if read_only {
            for r in &self.read_roots {
                if beneath(r, &file) {
                    allowed = true;
                    break;
                }
            }
        }
        if !allowed {
            return Err(
                "読み取りは専用フォルダーとGUIで選択した資料フォルダーだけ、書き込みは専用フォルダー内だけ許可されています"
                    .into(),
            );
        }
        #[cfg(windows)]
        {
            let s = file.to_string_lossy();
            if s.chars().skip(3).any(|c| c == ':') {
                return Err("代替データストリームは許可されていません".into());
            }
        }
        no_links(&file)?;
        if !directory && file == self.root {
            return Err("ファイル名を指定してください".into());
        }
        Ok(file)
    }

    #[cfg(windows)]
    fn atomic_write_checked(
        &self,
        file: &Path,
        text: &str,
        expected_before: Option<&str>,
    ) -> Result<(), TextEditError> {
        if let Some(p) = file.parent() {
            fs::create_dir_all(p).map_err(|e| TextEditError::new("IO_ERROR", e.to_string()))?;
        }
        no_links(file).map_err(|e| TextEditError::new("LINK_REJECTED", e))?;
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let temp_str = format!(
            "{}.qwen-tmp-{}-{}",
            file.to_string_lossy(),
            unsafe { GetCurrentProcessId() },
            seq
        );
        let temp = PathBuf::from(&temp_str);
        let wide_temp = to_wide_null(&temp_str);
        let wide_file = to_wide_null(&file.to_string_lossy());

        let write_res = (|| -> Result<(), TextEditError> {
            let h = unsafe {
                CreateFileW(
                    PCWSTR(wide_temp.as_ptr()),
                    GENERIC_WRITE.0,
                    Default::default(),
                    None,
                    CREATE_NEW,
                    FILE_ATTRIBUTE_NORMAL | FILE_FLAG_WRITE_THROUGH,
                    None,
                )
            }
            .map_err(|_| TextEditError::new("IO_ERROR", "編集用の一時ファイルを作成できません"))?;
            {
                let _guard = OwnedHandle(h);
                let data = text.as_bytes();
                let mut written = 0u32;
                unsafe { WriteFile(h, Some(data), Some(&mut written), None) }.map_err(|_| {
                    TextEditError::new("IO_ERROR", "編集内容を保存できません")
                })?;
                if written as usize != data.len() {
                    return Err(TextEditError::new("IO_ERROR", "編集内容を保存できません"));
                }
                unsafe { FlushFileBuffers(h) }
                    .map_err(|_| TextEditError::new("IO_ERROR", "編集内容を保存できません"))?;
            }
            no_links(file).map_err(|e| TextEditError::new("LINK_REJECTED", e))?;
            if let Some(before) = expected_before {
                let current =
                    read_text_file(file).map_err(|e| TextEditError::new("IO_ERROR", e))?;
                if current != before {
                    return Err(TextEditError::new(
                        "FILE_CHANGED",
                        "読み取り後にファイルが変更されています。local.readで最新の行番号を確認してください",
                    ));
                }
                unsafe {
                    ReplaceFileW(
                        PCWSTR(wide_file.as_ptr()),
                        PCWSTR(wide_temp.as_ptr()),
                        PCWSTR::null(),
                        REPLACEFILE_IGNORE_MERGE_ERRORS,
                        None,
                        None,
                    )
                }
                .map_err(|_| {
                    TextEditError::new(
                        "IO_ERROR",
                        "ファイルを置き換えられません。元のファイルは保持されています",
                    )
                })?;
            } else {
                unsafe {
                    MoveFileExW(
                        PCWSTR(wide_temp.as_ptr()),
                        PCWSTR(wide_file.as_ptr()),
                        MOVEFILE_WRITE_THROUGH,
                    )
                }
                .map_err(|_| {
                    TextEditError::new(
                        "FILE_EXISTS",
                        "同名のファイルが存在するため新規作成していません。local.readの後に編集操作を使ってください",
                    )
                })?;
            }
            Ok(())
        })();

        if write_res.is_err() {
            let _ = fs::remove_file(&temp);
        }
        write_res
    }

    #[cfg(not(windows))]
    fn atomic_write_checked(
        &self,
        file: &Path,
        text: &str,
        expected_before: Option<&str>,
    ) -> Result<(), TextEditError> {
        if let Some(p) = file.parent() {
            fs::create_dir_all(p).map_err(|e| TextEditError::new("IO_ERROR", e.to_string()))?;
        }
        no_links(file).map_err(|e| TextEditError::new("LINK_REJECTED", e))?;
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let temp = PathBuf::from(format!(
            "{}.qwen-tmp-{}-{}",
            file.to_string_lossy(),
            std::process::id(),
            seq
        ));

        let write_res = (|| -> Result<(), TextEditError> {
            {
                let mut f = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temp)
                    .map_err(|_| {
                        TextEditError::new("IO_ERROR", "編集用の一時ファイルを作成できません")
                    })?;
                f.write_all(text.as_bytes())
                    .map_err(|_| TextEditError::new("IO_ERROR", "編集内容を保存できません"))?;
                f.sync_all()
                    .map_err(|_| TextEditError::new("IO_ERROR", "編集内容を保存できません"))?;
            }
            no_links(file).map_err(|e| TextEditError::new("LINK_REJECTED", e))?;
            if let Some(before) = expected_before {
                let current =
                    read_text_file(file).map_err(|e| TextEditError::new("IO_ERROR", e))?;
                if current != before {
                    return Err(TextEditError::new(
                        "FILE_CHANGED",
                        "読み取り後にファイルが変更されています。local.readで最新の行番号を確認してください",
                    ));
                }
                fs::rename(&temp, file).map_err(|_| {
                    TextEditError::new(
                        "IO_ERROR",
                        "ファイルを置き換えられません。元のファイルは保持されています",
                    )
                })?;
            } else {
                match fs::hard_link(&temp, file) {
                    Ok(()) => {
                        let _ = fs::remove_file(&temp);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        return Err(TextEditError::new(
                            "FILE_EXISTS",
                            "同名のファイルが存在するため新規作成していません。local.readの後に編集操作を使ってください",
                        ));
                    }
                    Err(_) => {
                        if fs::symlink_metadata(file).is_ok() {
                            return Err(TextEditError::new(
                                "FILE_EXISTS",
                                "同名のファイルが存在するため新規作成していません。local.readの後に編集操作を使ってください",
                            ));
                        }
                        fs::rename(&temp, file).map_err(|_| {
                            TextEditError::new("IO_ERROR", "ファイルを新規作成できません")
                        })?;
                    }
                }
            }
            Ok(())
        })();

        if write_res.is_err() {
            let _ = fs::remove_file(&temp);
        }
        write_res
    }

    fn fetch_target(&self, relative: &str) -> Result<PathBuf, TextEditError> {
        let file = self
            .resolve_path(relative, false, false)
            .map_err(|e| TextEditError::new("FETCH_BAD_PATH", e))?;
        if !beneath(&self.root, &file) || file == self.root {
            return Err(TextEditError::new(
                "FETCH_BAD_PATH",
                "書き込み先は専用フォルダー内のファイルに限ります",
            ));
        }
        let rel = file
            .strip_prefix(&self.root)
            .map_err(|_| TextEditError::new("FETCH_BAD_PATH", "不正なパスです"))?;
        let mut all = String::new();
        for comp in rel.components() {
            let part = comp.as_os_str().to_string_lossy();
            if !all.is_empty() {
                all.push(std::path::MAIN_SEPARATOR);
            }
            all.push_str(&part);
            if part.is_empty()
                || part == "."
                || part == ".."
                || device_like(&part)
                || part.contains(".qwen-tmp-")
            {
                return Err(TextEditError::new(
                    "FETCH_BAD_PATH",
                    format!("使えないファイル名です: {}", part),
                ));
            }
            if part
                .chars()
                .any(|c| (c as u32) < 32 || "<>:\"|?*/\\".contains(c))
            {
                return Err(TextEditError::new(
                    "FETCH_BAD_PATH",
                    format!("ファイル名に使えない文字があります: {}", part),
                ));
            }
        }
        if all.is_empty() || all.encode_utf16().count() > 240 {
            return Err(TextEditError::new(
                "FETCH_BAD_PATH",
                "パスが空か240文字を超えています",
            ));
        }
        Ok(file)
    }

    fn fetch_temp(file: &Path, id: &str) -> Result<PathBuf, TextEditError> {
        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(TextEditError::new("FETCH_BAD_PATH", "transferIdが不正です"));
        }
        Ok(PathBuf::from(format!("{}.qwen-tmp-{}", file.to_string_lossy(), id)))
    }

    #[cfg(windows)]
    fn open_temp_retry(temp: &Path, create: bool) -> Result<HANDLE, u32> {
        let wide = to_wide_null(&temp.to_string_lossy());
        for attempt in 0..7 {
            let res = unsafe {
                CreateFileW(
                    PCWSTR(wide.as_ptr()),
                    GENERIC_READ.0 | GENERIC_WRITE.0,
                    FILE_SHARE_READ | FILE_SHARE_DELETE,
                    None,
                    if create { CREATE_NEW } else { OPEN_EXISTING },
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                )
            };
            match res {
                Ok(h) => return Ok(h),
                Err(e) => {
                    let code = (e.code().0 as u32) & 0xFFFF;
                    if create
                        || attempt >= 6
                        || (code != ERROR_SHARING_VIOLATION.0 && code != ERROR_ACCESS_DENIED.0)
                    {
                        return Err(code);
                    }
                    std::thread::sleep(Duration::from_millis(80));
                }
            }
        }
        Err(ERROR_ACCESS_DENIED.0)
    }

    #[cfg(windows)]
    fn move_temp_retry(temp: &Path, file: &Path, replace: bool) -> bool {
        let wide_temp = to_wide_null(&temp.to_string_lossy());
        let wide_file = to_wide_null(&file.to_string_lossy());
        let flags = if replace {
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH
        } else {
            MOVEFILE_WRITE_THROUGH
        };
        for attempt in 0..7 {
            match unsafe {
                MoveFileExW(PCWSTR(wide_temp.as_ptr()), PCWSTR(wide_file.as_ptr()), flags)
            } {
                Ok(()) => return true,
                Err(e) => {
                    let code = (e.code().0 as u32) & 0xFFFF;
                    if attempt >= 6
                        || (code != ERROR_SHARING_VIOLATION.0 && code != ERROR_ACCESS_DENIED.0)
                    {
                        return false;
                    }
                    std::thread::sleep(Duration::from_millis(80));
                }
            }
        }
        false
    }

    fn put(&self, args: &Value) -> Result<Value, TextEditError> {
        let res = self.put_inner(args);
        if res.is_err() {
            if let (Some(idx), Some(rel), Some(tid)) = (
                args.get("index").and_then(|v| v.as_i64()),
                args.get("path").and_then(|v| v.as_str()),
                args.get("transferId").and_then(|v| v.as_str()),
            ) {
                if idx > 0 {
                    if let (Ok(f), Ok(t)) = (
                        self.fetch_target(rel),
                        Self::fetch_temp(&self.fetch_target(rel).unwrap_or_default(), tid),
                    ) {
                        let _ = f;
                        let _ = fs::remove_file(t);
                    }
                }
            }
        }
        res
    }

    #[cfg(windows)]
    fn put_inner(&self, args: &Value) -> Result<Value, TextEditError> {
        let relative = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TextEditError::new("FETCH_BAD_PATH", "pathが必要です"))?;
        let id = args
            .get("transferId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TextEditError::new("FETCH_BAD_PATH", "transferIdが必要です"))?;
        let expected = args
            .get("sha256")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TextEditError::new("FETCH_HASH_MISMATCH", "sha256が必要です"))?;
        let encoded = args
            .get("base64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TextEditError::new("FETCH_HASH_MISMATCH", "base64が必要です"))?;

        let index = args
            .get("index")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| TextEditError::new("FETCH_BAD_PATH", "index/offset/totalは0以上の整数です"))?;
        let offset = args
            .get("offset")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| TextEditError::new("FETCH_BAD_PATH", "index/offset/totalは0以上の整数です"))?;
        let total = args
            .get("total")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| TextEditError::new("FETCH_BAD_PATH", "index/offset/totalは0以上の整数です"))?;
        let last = args
            .get("final")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| TextEditError::new("FETCH_BAD_PATH", "finalが必要です"))?;
        let overwrite = args
            .get("overwrite")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let file = self.fetch_target(relative)?;
        let temp = Self::fetch_temp(&file, id)?;
        let parent_len = file
            .parent()
            .map(|p| p.to_string_lossy().encode_utf16().count())
            .unwrap_or(0);
        if temp.to_string_lossy().encode_utf16().count() > 259 || parent_len > 247 {
            return Err(TextEditError::new(
                "FETCH_BAD_PATH",
                "作業フォルダーを含むパスが長すぎます（一時ファイルを含めて259文字まで）",
            ));
        }
        if total > K_FETCH_MAX {
            return Err(TextEditError::new(
                "FETCH_TOO_LARGE",
                "1ファイルは8MiBまでです",
            ));
        }
        if index == 0
            && (expected.len() != 64
                || !expected
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        {
            return Err(TextEditError::new(
                "FETCH_HASH_MISMATCH",
                "sha256は小文字16進64文字です",
            ));
        }
        if encoded.len() > 1_398_104 {
            return Err(TextEditError::new("FETCH_TOO_LARGE", "1塊は1MiBまでです"));
        }
        let bytes = B64.decode(encoded).map_err(|_| {
            if index > 0 {
                let _ = fs::remove_file(&temp);
            }
            TextEditError::new("FETCH_HASH_MISMATCH", "base64が不正です。一時ファイルを消しました")
        })?;
        if bytes.len() > K_CHUNK {
            if index > 0 {
                let _ = fs::remove_file(&temp);
            }
            return Err(TextEditError::new("FETCH_TOO_LARGE", "1塊は1MiBまでです"));
        }
        if index == 0 {
            let wfile = to_wide_null(&file.to_string_lossy());
            let attr = unsafe { GetFileAttributesW(PCWSTR(wfile.as_ptr())) };
            if attr != INVALID_FILE_ATTRIBUTES {
                if (attr & FILE_ATTRIBUTE_DIRECTORY.0) != 0 {
                    return Err(TextEditError::new(
                        "FETCH_BAD_PATH",
                        "同名のフォルダーがあります",
                    ));
                }
                if !overwrite {
                    return Err(TextEditError::new(
                        "FETCH_EXISTS",
                        "同名のファイルがあります。上書きするならoverwrite=trueを指定してください",
                    ));
                }
            }
            if let Some(p) = file.parent() {
                fs::create_dir_all(p).map_err(|e| {
                    TextEditError::new("FETCH_BAD_PATH", format!("フォルダーを作れません: {}", e))
                })?;
            }
            no_links(&file).map_err(|e| TextEditError::new("FETCH_BAD_PATH", e))?;
        }

        let h = Self::open_temp_retry(&temp, index == 0).map_err(|_| {
            if index == 0 {
                TextEditError::new(
                    "FETCH_BAD_PATH",
                    "一時ファイルを作成できません（同じtransferIdが使用中）",
                )
            } else {
                TextEditError::new(
                    "FETCH_PC_UNAVAILABLE",
                    "この転送の一時ファイルがありません",
                )
            }
        })?;

        let write_stage = (|| -> Result<Option<Value>, TextEditError> {
            let guard = OwnedHandle(h);
            let mut size = 0i64;
            unsafe { GetFileSizeEx(guard.0, &mut size) }.map_err(|_| {
                TextEditError::new("IO_ERROR", "一時ファイルの大きさを取得できません")
            })?;
            if size < 0 || size as u64 != offset || offset + (bytes.len() as u64) > total {
                drop(guard);
                let _ = fs::remove_file(&temp);
                return Err(TextEditError::new(
                    "FETCH_HASH_MISMATCH",
                    "塊の位置が合いません（順番違いまたは大きさ超過）。一時ファイルを消しました",
                ));
            }
            unsafe { SetFilePointerEx(guard.0, size, None, FILE_BEGIN) }.map_err(|_| {
                TextEditError::new("IO_ERROR", "一時ファイルの末尾へ移動できません")
            })?;
            let mut done = 0usize;
            while done < bytes.len() {
                let mut written = 0u32;
                let end = (done + (1 << 20)).min(bytes.len());
                unsafe { WriteFile(guard.0, Some(&bytes[done..end]), Some(&mut written), None) }
                    .map_err(|_| {
                        TextEditError::new("IO_ERROR", "一時ファイルへ書き込めません")
                    })?;
                if written == 0 {
                    return Err(TextEditError::new(
                        "IO_ERROR",
                        "一時ファイルへ書き込めません",
                    ));
                }
                done += written as usize;
            }
            if !last {
                return Ok(Some(json!({ "received": offset + (bytes.len() as u64) })));
            }
            unsafe { FlushFileBuffers(guard.0) }.map_err(|_| {
                TextEditError::new("IO_ERROR", "一時ファイルを保存できません")
            })?;
            drop(guard);
            let _ = &mut ();
            Ok(None)
        })();

        match write_stage {
            Ok(Some(v)) => return Ok(v),
            Ok(None) => {}
            Err(e) => {
                let _ = fs::remove_file(&temp);
                return Err(e);
            }
        }

        let back = fs::read(&temp).map_err(|_| {
            let _ = fs::remove_file(&temp);
            TextEditError::new("IO_ERROR", "一時ファイルを読み直せません")
        })?;
        let hash = sha256_lower_hex(&back);
        if back.len() as u64 != total || hash != expected {
            let _ = fs::remove_file(&temp);
            return Err(TextEditError::new(
                "FETCH_HASH_MISMATCH",
                "大きさまたはSHA-256が合いません。一時ファイルを消しました",
            ));
        }

        let wfile = to_wide_null(&file.to_string_lossy());
        let before = unsafe { GetFileAttributesW(PCWSTR(wfile.as_ptr())) };
        let exists = before != INVALID_FILE_ATTRIBUTES;
        let mut replaced_info: Option<Value> = None;
        if exists {
            if (before & FILE_ATTRIBUTE_DIRECTORY.0) != 0 {
                let _ = fs::remove_file(&temp);
                return Err(TextEditError::new(
                    "FETCH_BAD_PATH",
                    "同名のフォルダーがあります",
                ));
            }
            if !overwrite {
                let _ = fs::remove_file(&temp);
                return Err(TextEditError::new(
                    "FETCH_EXISTS",
                    "転送中に同名のファイルができました",
                ));
            }
            let mut attr_data = WIN32_FILE_ATTRIBUTE_DATA::default();
            if unsafe {
                GetFileAttributesExW(
                    PCWSTR(wfile.as_ptr()),
                    GetFileExInfoStandard,
                    (&mut attr_data as *mut WIN32_FILE_ATTRIBUTE_DATA).cast(),
                )
            }
            .is_err()
            {
                let _ = fs::remove_file(&temp);
                return Err(TextEditError::new(
                    "IO_ERROR",
                    "置き換え前のファイルを調べられません",
                ));
            }
            let old_size =
                ((attr_data.nFileSizeHigh as u64) << 32) | (attr_data.nFileSizeLow as u64);
            let mut rep = json!({ "bytes": old_size });
            if old_size <= K_FETCH_MAX {
                match sha256_file_hex(&file) {
                    Ok(oh) => rep["sha256"] = json!(oh),
                    Err(e) => {
                        let _ = fs::remove_file(&temp);
                        return Err(TextEditError::new("IO_ERROR", e));
                    }
                }
            }
            replaced_info = Some(rep);
        }

        no_links(&file).map_err(|e| {
            let _ = fs::remove_file(&temp);
            TextEditError::new("FETCH_BAD_PATH", e)
        })?;
        if !Self::move_temp_retry(&temp, &file, exists) {
            let now_attr = unsafe { GetFileAttributesW(PCWSTR(wfile.as_ptr())) };
            let _ = fs::remove_file(&temp);
            if !exists && now_attr != INVALID_FILE_ATTRIBUTES {
                return Err(TextEditError::new(
                    "FETCH_EXISTS",
                    "転送中に同名のファイルができました",
                ));
            }
            return Err(TextEditError::new("IO_ERROR", "ファイルを配置できません"));
        }

        let mut out = json!({
            "path": relative,
            "absolutePath": file.to_string_lossy(),
            "bytes": back.len(),
            "sha256": hash,
        });
        if let Some(rep) = replaced_info {
            out["replaced"] = rep;
        }
        Ok(out)
    }

    #[cfg(not(windows))]
    fn put_inner(&self, args: &Value) -> Result<Value, TextEditError> {
        let relative = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TextEditError::new("FETCH_BAD_PATH", "pathが必要です"))?;
        let id = args
            .get("transferId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TextEditError::new("FETCH_BAD_PATH", "transferIdが必要です"))?;
        let expected = args
            .get("sha256")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TextEditError::new("FETCH_HASH_MISMATCH", "sha256が必要です"))?;
        let encoded = args
            .get("base64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TextEditError::new("FETCH_HASH_MISMATCH", "base64が必要です"))?;

        let index = args
            .get("index")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| TextEditError::new("FETCH_BAD_PATH", "index/offset/totalは0以上の整数です"))?;
        let offset = args
            .get("offset")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| TextEditError::new("FETCH_BAD_PATH", "index/offset/totalは0以上の整数です"))?;
        let total = args
            .get("total")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| TextEditError::new("FETCH_BAD_PATH", "index/offset/totalは0以上の整数です"))?;
        let last = args
            .get("final")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| TextEditError::new("FETCH_BAD_PATH", "finalが必要です"))?;
        let overwrite = args
            .get("overwrite")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let file = self.fetch_target(relative)?;
        let temp = Self::fetch_temp(&file, id)?;
        if total > K_FETCH_MAX {
            return Err(TextEditError::new(
                "FETCH_TOO_LARGE",
                "1ファイルは8MiBまでです",
            ));
        }
        if index == 0
            && (expected.len() != 64
                || !expected
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        {
            return Err(TextEditError::new(
                "FETCH_HASH_MISMATCH",
                "sha256は小文字16進64文字です",
            ));
        }
        if encoded.len() > 1_398_104 {
            return Err(TextEditError::new("FETCH_TOO_LARGE", "1塊は1MiBまでです"));
        }
        let bytes = B64.decode(encoded).map_err(|_| {
            if index > 0 {
                let _ = fs::remove_file(&temp);
            }
            TextEditError::new("FETCH_HASH_MISMATCH", "base64が不正です。一時ファイルを消しました")
        })?;
        if bytes.len() > K_CHUNK {
            if index > 0 {
                let _ = fs::remove_file(&temp);
            }
            return Err(TextEditError::new("FETCH_TOO_LARGE", "1塊は1MiBまでです"));
        }
        if index == 0 {
            if let Ok(meta) = fs::symlink_metadata(&file) {
                if meta.is_dir() {
                    return Err(TextEditError::new(
                        "FETCH_BAD_PATH",
                        "同名のフォルダーがあります",
                    ));
                }
                if !overwrite {
                    return Err(TextEditError::new(
                        "FETCH_EXISTS",
                        "同名のファイルがあります。上書きするならoverwrite=trueを指定してください",
                    ));
                }
            }
            if let Some(p) = file.parent() {
                fs::create_dir_all(p).map_err(|e| {
                    TextEditError::new("FETCH_BAD_PATH", format!("フォルダーを作れません: {}", e))
                })?;
            }
            no_links(&file).map_err(|e| TextEditError::new("FETCH_BAD_PATH", e))?;
        }

        let mut f = if index == 0 {
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|_| {
                    TextEditError::new(
                        "FETCH_BAD_PATH",
                        "一時ファイルを作成できません（同じtransferIdが使用中）",
                    )
                })?
        } else {
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&temp)
                .map_err(|_| {
                    TextEditError::new(
                        "FETCH_PC_UNAVAILABLE",
                        "この転送の一時ファイルがありません",
                    )
                })?
        };

        let write_stage = (|| -> Result<Option<Value>, TextEditError> {
            let size = f
                .metadata()
                .map_err(|_| TextEditError::new("IO_ERROR", "一時ファイルの大きさを取得できません"))?
                .len();
            if size != offset || offset + (bytes.len() as u64) > total {
                return Err(TextEditError::new(
                    "FETCH_HASH_MISMATCH",
                    "塊の位置が合いません（順番違いまたは大きさ超過）。一時ファイルを消しました",
                ));
            }
            f.seek(SeekFrom::Start(size))
                .map_err(|_| TextEditError::new("IO_ERROR", "一時ファイルの末尾へ移動できません"))?;
            f.write_all(&bytes)
                .map_err(|_| TextEditError::new("IO_ERROR", "一時ファイルへ書き込めません"))?;
            if !last {
                return Ok(Some(json!({ "received": offset + (bytes.len() as u64) })));
            }
            f.sync_all()
                .map_err(|_| TextEditError::new("IO_ERROR", "一時ファイルを保存できません"))?;
            Ok(None)
        })();
        drop(f);

        match write_stage {
            Ok(Some(v)) => return Ok(v),
            Ok(None) => {}
            Err(e) => {
                let _ = fs::remove_file(&temp);
                return Err(e);
            }
        }

        let back = fs::read(&temp).map_err(|_| {
            let _ = fs::remove_file(&temp);
            TextEditError::new("IO_ERROR", "一時ファイルを読み直せません")
        })?;
        let hash = sha256_lower_hex(&back);
        if back.len() as u64 != total || hash != expected {
            let _ = fs::remove_file(&temp);
            return Err(TextEditError::new(
                "FETCH_HASH_MISMATCH",
                "大きさまたはSHA-256が合いません。一時ファイルを消しました",
            ));
        }

        let existing_meta = fs::symlink_metadata(&file).ok();
        let exists = existing_meta.is_some();
        let mut replaced_info: Option<Value> = None;
        if let Some(meta) = existing_meta {
            if meta.is_dir() {
                let _ = fs::remove_file(&temp);
                return Err(TextEditError::new(
                    "FETCH_BAD_PATH",
                    "同名のフォルダーがあります",
                ));
            }
            if !overwrite {
                let _ = fs::remove_file(&temp);
                return Err(TextEditError::new(
                    "FETCH_EXISTS",
                    "転送中に同名のファイルができました",
                ));
            }
            let old_size = meta.len();
            let mut rep = json!({ "bytes": old_size });
            if old_size <= K_FETCH_MAX {
                match sha256_file_hex(&file) {
                    Ok(oh) => rep["sha256"] = json!(oh),
                    Err(e) => {
                        let _ = fs::remove_file(&temp);
                        return Err(TextEditError::new("IO_ERROR", e));
                    }
                }
            }
            replaced_info = Some(rep);
        }

        no_links(&file).map_err(|e| {
            let _ = fs::remove_file(&temp);
            TextEditError::new("FETCH_BAD_PATH", e)
        })?;
        if exists {
            if fs::rename(&temp, &file).is_err() {
                let _ = fs::remove_file(&temp);
                return Err(TextEditError::new("IO_ERROR", "ファイルを配置できません"));
            }
        } else {
            match fs::hard_link(&temp, &file) {
                Ok(()) => {
                    let _ = fs::remove_file(&temp);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let _ = fs::remove_file(&temp);
                    return Err(TextEditError::new(
                        "FETCH_EXISTS",
                        "転送中に同名のファイルができました",
                    ));
                }
                Err(_) => {
                    if fs::symlink_metadata(&file).is_ok() {
                        let _ = fs::remove_file(&temp);
                        return Err(TextEditError::new(
                            "FETCH_EXISTS",
                            "転送中に同名のファイルができました",
                        ));
                    }
                    if fs::rename(&temp, &file).is_err() {
                        let _ = fs::remove_file(&temp);
                        return Err(TextEditError::new("IO_ERROR", "ファイルを配置できません"));
                    }
                }
            }
        }

        let mut out = json!({
            "path": relative,
            "absolutePath": file.to_string_lossy(),
            "bytes": back.len(),
            "sha256": hash,
        });
        if let Some(rep) = replaced_info {
            out["replaced"] = rep;
        }
        Ok(out)
    }

    #[cfg(windows)]
    fn get(&self, args: &Value) -> Result<Value, TextEditError> {
        let relative = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TextEditError::new("FETCH_BAD_PATH", "pathが必要です"))?;
        let offset = args.get("offset").and_then(|v| v.as_u64()).ok_or_else(|| {
            TextEditError::new("FETCH_BAD_PATH", "offset/lengthは0以上の整数です")
        })?;
        let length = args.get("length").and_then(|v| v.as_u64()).ok_or_else(|| {
            TextEditError::new("FETCH_BAD_PATH", "offset/lengthは0以上の整数です")
        })?;
        if length > K_CHUNK as u64 {
            return Err(TextEditError::new(
                "FETCH_TOO_LARGE",
                "lengthは1MiBまでです",
            ));
        }

        let file = match self.resolve_path(relative, false, true) {
            Ok(f) => f,
            Err(e) => {
                let p = PathBuf::from(relative);
                let has_dots = p.components().any(|c| matches!(c, Component::ParentDir));
                if !has_dots && !relative.is_empty() {
                    let cand = lexically_normal(&(if p.is_absolute() { p } else { self.root.join(p) }));
                    let mut allowed = beneath(&self.root, &cand);
                    for r in &self.read_roots {
                        allowed |= beneath(r, &cand);
                    }
                    let wc = to_wide_null(&cand.to_string_lossy());
                    let a = unsafe { GetFileAttributesW(PCWSTR(wc.as_ptr())) };
                    if allowed
                        && a != INVALID_FILE_ATTRIBUTES
                        && (a & FILE_ATTRIBUTE_REPARSE_POINT.0) != 0
                    {
                        return Err(TextEditError::new(
                            "FETCH_NOT_A_FILE",
                            "リンクは転送できません",
                        ));
                    }
                }
                return Err(TextEditError::new("FETCH_BAD_PATH", e));
            }
        };

        let base_opt = if beneath(&self.root, &file) {
            Some(&self.root)
        } else {
            self.read_roots.iter().find(|r| beneath(r, &file))
        };
        if let Some(base) = base_opt {
            if let Ok(rel) = file.strip_prefix(base) {
                for comp in rel.components() {
                    if device_like(&comp.as_os_str().to_string_lossy()) {
                        return Err(TextEditError::new(
                            "FETCH_NOT_A_FILE",
                            "デバイス名または末尾が . か空白の名前は転送できません",
                        ));
                    }
                }
            }
        }

        let wfile = to_wide_null(&file.to_string_lossy());
        let attr = unsafe { GetFileAttributesW(PCWSTR(wfile.as_ptr())) };
        if attr == INVALID_FILE_ATTRIBUTES {
            return Err(TextEditError::new(
                "FETCH_SOURCE_MISSING",
                "転送元がありません",
            ));
        }
        if (attr & (FILE_ATTRIBUTE_DIRECTORY.0 | FILE_ATTRIBUTE_REPARSE_POINT.0)) != 0 {
            return Err(TextEditError::new(
                "FETCH_NOT_A_FILE",
                "普通のファイルではありません",
            ));
        }
        let h = unsafe {
            CreateFileW(
                PCWSTR(wfile.as_ptr()),
                GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(|_| TextEditError::new("FETCH_SOURCE_MISSING", "転送元を開けません"))?;
        let guard = OwnedHandle(h);
        if unsafe { GetFileType(guard.0) } != FILE_TYPE_DISK {
            return Err(TextEditError::new(
                "FETCH_NOT_A_FILE",
                "普通のファイルではありません",
            ));
        }
        let mut size = 0i64;
        unsafe { GetFileSizeEx(guard.0, &mut size) }
            .map_err(|_| TextEditError::new("IO_ERROR", "転送元の大きさを取得できません"))?;
        if size < 0 || (size as u64) > K_FETCH_MAX {
            return Err(TextEditError::new(
                "FETCH_TOO_LARGE",
                format!("転送元は{}バイトで、8MiBを超えています", size),
            ));
        }
        let mut all = vec![0u8; size as usize];
        let mut done = 0usize;
        while done < all.len() {
            let mut read = 0u32;
            let end = (done + (1 << 20)).min(all.len());
            if unsafe { ReadFile(guard.0, Some(&mut all[done..end]), Some(&mut read), None) }
                .is_err()
                || read == 0
            {
                break;
            }
            done += read as usize;
        }
        if done != all.len() {
            return Err(TextEditError::new("IO_ERROR", "転送元を最後まで読めません"));
        }
        let start = (offset as usize).min(all.len());
        let count = (length as usize).min(all.len() - start);
        let piece = &all[start..start + count];
        Ok(json!({
            "base64": B64.encode(piece),
            "bytes": piece.len(),
            "total": all.len(),
            "sha256": sha256_lower_hex(&all),
        }))
    }

    #[cfg(not(windows))]
    fn get(&self, args: &Value) -> Result<Value, TextEditError> {
        let relative = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TextEditError::new("FETCH_BAD_PATH", "pathが必要です"))?;
        let offset = args.get("offset").and_then(|v| v.as_u64()).ok_or_else(|| {
            TextEditError::new("FETCH_BAD_PATH", "offset/lengthは0以上の整数です")
        })?;
        let length = args.get("length").and_then(|v| v.as_u64()).ok_or_else(|| {
            TextEditError::new("FETCH_BAD_PATH", "offset/lengthは0以上の整数です")
        })?;
        if length > K_CHUNK as u64 {
            return Err(TextEditError::new(
                "FETCH_TOO_LARGE",
                "lengthは1MiBまでです",
            ));
        }

        let file = match self.resolve_path(relative, false, true) {
            Ok(f) => f,
            Err(e) => {
                let p = PathBuf::from(relative);
                let has_dots = p.components().any(|c| matches!(c, Component::ParentDir));
                if !has_dots && !relative.is_empty() {
                    let cand = lexically_normal(&(if p.is_absolute() { p } else { self.root.join(p) }));
                    let mut allowed = beneath(&self.root, &cand);
                    for r in &self.read_roots {
                        allowed |= beneath(r, &cand);
                    }
                    if allowed
                        && fs::symlink_metadata(&cand)
                            .map(|m| m.file_type().is_symlink())
                            .unwrap_or(false)
                    {
                        return Err(TextEditError::new(
                            "FETCH_NOT_A_FILE",
                            "リンクは転送できません",
                        ));
                    }
                }
                return Err(TextEditError::new("FETCH_BAD_PATH", e));
            }
        };

        let base_opt = if beneath(&self.root, &file) {
            Some(&self.root)
        } else {
            self.read_roots.iter().find(|r| beneath(r, &file))
        };
        if let Some(base) = base_opt {
            if let Ok(rel) = file.strip_prefix(base) {
                for comp in rel.components() {
                    if device_like(&comp.as_os_str().to_string_lossy()) {
                        return Err(TextEditError::new(
                            "FETCH_NOT_A_FILE",
                            "デバイス名または末尾が . か空白の名前は転送できません",
                        ));
                    }
                }
            }
        }

        let meta = fs::symlink_metadata(&file)
            .map_err(|_| TextEditError::new("FETCH_SOURCE_MISSING", "転送元がありません"))?;
        if meta.file_type().is_symlink() || !meta.file_type().is_file() {
            return Err(TextEditError::new(
                "FETCH_NOT_A_FILE",
                "普通のファイルではありません",
            ));
        }
        let size = meta.len();
        if size > K_FETCH_MAX {
            return Err(TextEditError::new(
                "FETCH_TOO_LARGE",
                format!("転送元は{}バイトで、8MiBを超えています", size),
            ));
        }
        let all = fs::read(&file)
            .map_err(|_| TextEditError::new("IO_ERROR", "転送元を最後まで読めません"))?;
        let start = (offset as usize).min(all.len());
        let count = (length as usize).min(all.len() - start);
        let piece = &all[start..start + count];
        Ok(json!({
            "base64": B64.encode(piece),
            "bytes": piece.len(),
            "total": all.len(),
            "sha256": sha256_lower_hex(&all),
        }))
    }

    pub fn perform<F>(&self, request: &Value, progress: F) -> Result<Value, String>
    where
        F: FnMut(Value),
    {
        let tool = request
            .get("tool")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "tool が必要です".to_string())?;
        let args = request.get("args").cloned().unwrap_or_else(|| json!({}));
        let relative = args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".");

        if tool == "local.list" {
            let rel = if relative.is_empty() { "." } else { relative };
            let folder = self.resolve_path(rel, true, true)?;
            if !folder.is_dir() {
                return Err("local.listはフォルダー用です。ファイルの本文はlocal.readで取得してください".into());
            }
            let hidden = args.get("includeHidden").and_then(|v| v.as_bool()).unwrap_or(true);
            let mut files = Vec::new();
            let mut total = 0i32;
            let mut skipped = 0i32;
            let entries = fs::read_dir(&folder).map_err(|e| format!("フォルダーを読めません: {}", e))?;
            for entry in entries.flatten() {
                let p = entry.path();
                let entry_name = entry.file_name().to_string_lossy().into_owned();
                let (is_hidden, kind) = entry_list_meta(&p, &entry_name);
                if !hidden && is_hidden {
                    skipped += 1;
                    continue;
                }
                total += 1;
                if files.len() >= 200 {
                    continue;
                }
                files.push(json!({
                    "name": entry_name,
                    "hidden": is_hidden,
                    "type": kind,
                }));
            }
            let read_roots: Vec<String> = self
                .read_roots
                .iter()
                .map(|r| r.to_string_lossy().into_owned())
                .collect();
            let msg = if total > 0 {
                "一覧を取得しました"
            } else if skipped > 0 {
                "隠し項目を除くと空です。includeHidden=trueで確認できます"
            } else {
                "フォルダーは空です（隠し項目もありません）"
            };
            let (_, _, shell_label) = resolve_powershell_bin();
            let (shell_major, shell_version) = resolve_powershell_version();
            let mut listed = json!({
                "localProtocol": 3,
                "computerName": computer_name(),
                "platform": if cfg!(windows) { "windows" } else { "linux" },
                "shell": shell_label,
                "shellMajor": shell_major,
                "shellVersion": shell_version,
                "path": folder.to_string_lossy(),
                "workspaceRoot": self.root.to_string_lossy(),
                "readOnlyRoots": read_roots,
                "entries": files,
                "count": total,
                "empty": total == 0,
                "hiddenSkipped": skipped,
                "includeHidden": hidden,
                "truncated": total > 200,
                "message": msg,
                "administrator": is_current_process_elevated(),
                "winget": has_winget(),
                "networkPolicy": "download_allowed",
                "internetDownload": true,
                "allowUpload": false,
            });
            if args.get("environment").and_then(|v| v.as_bool()).unwrap_or(false) {
                listed["compilers"] = compilers_info();
            }
            return Ok(listed);
        }

        if tool == "local.read" {
            let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            if pattern.is_empty() {
                let file = self.resolve_path(relative, false, true)?;
                let text = read_text_file(&file)?;
                let lines = text_lines(&text);
                let mut res = json!({
                    "localProtocol": 3,
                    "path": file.to_string_lossy(),
                    "workspaceRoot": self.root.to_string_lossy(),
                    "version": sha256_lower_hex(text.as_bytes()),
                    "content": text,
                    "totalLines": lines.len(),
                });
                if let Some(first_val) = args.get("startLine").and_then(|v| v.as_i64()) {
                    let count = args.get("lineCount").and_then(|v| v.as_i64()).unwrap_or(120);
                    if first_val < 1
                        || count < 1
                        || count > 400
                        || (!lines.is_empty() && (first_val as usize) > lines.len())
                        || (lines.is_empty() && first_val != 1)
                    {
                        return Err(format!(
                            "startLineは1始まり、lineCountは1〜400です。現在の総行数は{}です",
                            lines.len()
                        ));
                    }
                    let first = first_val as usize;
                    let cnt = count as usize;
                    let mut numbered = String::new();
                    let mut last = first - 1;
                    let mut partial = false;
                    for i in (first - 1)..lines.len().min(first - 1 + cnt) {
                        let line_body = &text[lines[i].begin..lines[i].text_end];
                        let row_str = format!("{} | {}\n", i + 1, line_body);
                        if numbered.len() + row_str.len() > 8000 {
                            let rem = 8000 - numbered.len();
                            numbered.push_str(clip_utf8_prefix(&row_str, rem));
                            partial = true;
                            last = i + 1;
                            break;
                        }
                        numbered.push_str(&row_str);
                        last = i + 1;
                    }
                    res["content"] = json!(numbered);
                    res["numbered"] = json!(true);
                    res["startLine"] = json!(first);
                    res["endLine"] = json!(last);
                    res["truncated"] = json!(partial || first > 1 || last < lines.len());
                    res["partialLine"] = json!(partial);
                }
                return Ok(res);
            }
            if pattern.len() > 160 || pattern.contains('/') || pattern.contains('\\') || pattern.contains(':') {
                return Err("patternはフォルダー直下のファイル名（*.txt等）を指定してください".into());
            }
            let dir = self.resolve_path(relative, true, true)?;
            let mut files = Vec::new();
            let mut bytes = 0u64;
            let mut truncated = false;
            for entry in fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
                let fname = entry.file_name().to_string_lossy().into_owned();
                if !match_wildcard(pattern, &fname) {
                    continue;
                }
                let p = entry.path();
                if !is_non_link_existing(&p) || !p.is_file() {
                    continue;
                }
                let fsize = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                if files.len() >= 20 || bytes + fsize > 100_000 {
                    truncated = true;
                    continue;
                }
                match no_links(&p).and_then(|_| read_text_file(&p)) {
                    Ok(content) => {
                        bytes += content.len() as u64;
                        files.push(json!({ "path": p.to_string_lossy(), "content": content }));
                    }
                    Err(err) => {
                        files.push(json!({ "path": p.to_string_lossy(), "error": err }));
                    }
                }
            }
            return Ok(json!({
                "files": files,
                "count": files.len(),
                "empty": files.is_empty(),
                "truncated": truncated,
                "workspaceRoot": self.root.to_string_lossy(),
            }));
        }

        if tool == "local.search" {
            let rel = if relative.is_empty() { "." } else { relative };
            let target = self.resolve_path(rel, true, true)?;
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "queryが必要です".to_string())?;
            let pattern = args.get("filePattern").and_then(|v| v.as_str()).unwrap_or("*");
            let recursive = args.get("recursive").and_then(|v| v.as_bool()).unwrap_or(false);
            if query.is_empty() || query.len() > 1000 {
                return Err("queryは1〜1000バイトの本文検索文字列です".into());
            }
            if pattern.len() > 160 || pattern.contains('/') || pattern.contains('\\') || pattern.contains(':') {
                return Err("filePatternは*.cppのようなファイル名パターンです".into());
            }

            let mut matches = Vec::new();
            let mut visited = 0usize;
            let mut scanned = 0usize;
            let mut skipped = 0usize;
            let mut truncated = false;

            let inspect = |file: &Path,
                           matches: &mut Vec<Value>,
                           scanned: &mut usize,
                           skipped: &mut usize,
                           truncated: &mut bool| {
                let fname = file.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
                if !match_wildcard(pattern, &fname) {
                    return;
                }
                if no_links(file).is_err() {
                    *skipped += 1;
                    return;
                }
                let Ok(text) = read_text_file(file) else {
                    *skipped += 1;
                    return;
                };
                if text.contains('\0') {
                    *skipped += 1;
                    return;
                }
                *scanned += 1;
                let lines = text_lines(&text);
                for (i, line) in lines.iter().enumerate() {
                    let slice = &text[line.begin..line.text_end];
                    if !slice.contains(query) {
                        continue;
                    }
                    if matches.len() >= 50 {
                        *truncated = true;
                        break;
                    }
                    let val_str = if slice.len() > 600 {
                        format!("{}…", clip_utf8_prefix(slice, 600))
                    } else {
                        slice.to_string()
                    };
                    let rel_path = if beneath(&self.root, file) {
                        file.strip_prefix(&self.root)
                            .map(|r| r.to_string_lossy().into_owned())
                            .unwrap_or_else(|_| file.to_string_lossy().into_owned())
                    } else {
                        file.to_string_lossy().into_owned()
                    };
                    matches.push(json!({
                        "path": rel_path,
                        "line": i + 1,
                        "text": val_str,
                    }));
                }
            };

            if target.is_file() {
                inspect(&target, &mut matches, &mut scanned, &mut skipped, &mut truncated);
            } else if target.is_dir() {
                let mut dirs = vec![(target.clone(), 0usize)];
                while let Some((d, depth)) = dirs.pop() {
                    let Ok(rd) = fs::read_dir(&d) else { continue; };
                    for entry in rd.flatten() {
                        visited += 1;
                        if visited > 1000 || matches.len() >= 50 {
                            truncated = true;
                            break;
                        }
                        let p = entry.path();
                        if !is_non_link_existing(&p) {
                            continue;
                        }
                        if p.is_dir() {
                            let dname = p.file_name().map(|s| s.to_string_lossy()).unwrap_or_default();
                            if recursive && dname != ".git" && dname != "node_modules" && dname != "build" {
                                if depth < 6 {
                                    dirs.push((p, depth + 1));
                                } else {
                                    truncated = true;
                                }
                            }
                        } else if p.is_file() {
                            inspect(&p, &mut matches, &mut scanned, &mut skipped, &mut truncated);
                        }
                    }
                    if visited > 1000 || matches.len() >= 50 {
                        break;
                    }
                }
            } else {
                return Err("検索対象のファイルまたはフォルダーがありません".into());
            }

            let empty = matches.is_empty();
            return Ok(json!({
                "matches": matches,
                "count": matches.len(),
                "filesScanned": scanned,
                "skippedFiles": skipped,
                "truncated": truncated || skipped > 0,
                "query": query,
                "message": if empty { "一致はありません（truncatedも確認してください）" } else { "一致した行を取得しました" },
                "workspaceRoot": self.root.to_string_lossy(),
            }));
        }

        if tool == "local.create" || tool == "local.edit" {
            let file = self.resolve_path(relative, false, false)?;
            let create = tool == "local.create";
            let res: Result<Value, TextEditError> = (|| {
                let mut before = String::new();
                let after: String;
                if create {
                    if file.exists() {
                        return Err(TextEditError::new(
                            "FILE_EXISTS",
                            "既存ファイルは新規作成で上書きできません。local.readの後に置換・挿入・行削除を使ってください",
                        ));
                    }
                    after = args
                        .get("text")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| TextEditError::new("EDIT_OPERATION", "textが必要です"))?
                        .to_string();
                    if after.len() > 65536 || after.contains('\0') {
                        return Err(TextEditError::new(
                            "FILE_TOO_LARGE",
                            "新規作成は64KiB以内のUTF-8テキストに限ります",
                        ));
                    }
                    self.atomic_write_checked(&file, &after, None)?;
                } else {
                    before = read_text_file(&file).map_err(|e| TextEditError::new("IO_ERROR", e))?;
                    if before.contains('\0') {
                        return Err(TextEditError::new("BINARY_FILE", "バイナリは編集できません"));
                    }
                    let expected = args.get("expectedVersion").and_then(|v| v.as_str()).unwrap_or("");
                    if expected.is_empty() {
                        return Err(TextEditError::new(
                            "READ_REQUIRED",
                            "先にlocal.readで編集対象を読み取ってください",
                        ));
                    }
                    if sha256_lower_hex(before.as_bytes()) != expected {
                        return Err(TextEditError::new(
                            "FILE_CHANGED",
                            "読み取り後にファイルが変更されています。書き込まずに停止しました。local.readで最新の内容と行番号を取得してください",
                        ));
                    }
                    after = apply_text_edit(&before, &args)?;
                    if after != before {
                        self.atomic_write_checked(&file, &after, Some(&before))?;
                    }
                }
                let op_name = if create {
                    "create"
                } else {
                    args.get("operation").and_then(|v| v.as_str()).unwrap_or("edit")
                };
                let msg = if after == before && !create {
                    "変更内容は同一でした"
                } else {
                    "指定範囲の編集を保存しました"
                };
                Ok(json!({
                    "written": relative,
                    "absolutePath": file.to_string_lossy(),
                    "workspaceRoot": self.root.to_string_lossy(),
                    "version": sha256_lower_hex(after.as_bytes()),
                    "bytes": after.len(),
                    "operation": op_name,
                    "totalLines": text_lines(&after).len(),
                    "edit": {
                        "before": before,
                        "after": after,
                        "created": create,
                    },
                    "message": msg,
                }))
            })();
            return Ok(match res {
                Ok(v) => v,
                Err(e) => {
                    let mut obj = json!({
                        "error": e.message,
                        "code": e.code,
                        "changed": false,
                        "executed": false,
                        "path": relative,
                        "readRequired": matches!(e.code, "FILE_CHANGED" | "READ_REQUIRED" | "LINE_RANGE"),
                    });
                    if e.matches >= 0 {
                        obj["matchCount"] = json!(e.matches);
                    }
                    obj
                }
            });
        }

        if tool == "local.write" {
            let file = self.resolve_path(relative, false, false)?;
            let text = args
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "textが必要です".to_string())?;
            if text.len() > 65536 {
                return Err("書き込みは64KiBまでです".into());
            }
            if let Some(p) = file.parent() {
                fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            no_links(&file)?;
            let existed = file.exists();
            let mut edit = json!({
                "before": "",
                "after": text,
                "created": !existed,
            });
            if existed {
                match read_text_file(&file) {
                    Ok(prev) => edit["before"] = json!(prev),
                    Err(_) => edit["omitted"] = json!("unreadable"),
                }
            }
            fs::write(&file, text.as_bytes()).map_err(|_| "ファイルを書き込めません".to_string())?;
            return Ok(json!({
                "written": relative,
                "workspaceRoot": self.root.to_string_lossy(),
                "absolutePath": file.to_string_lossy(),
                "bytes": text.len(),
                "edit": edit,
            }));
        }

        if tool == "local.put" || tool == "local.get" || tool == "local.put_abort" {
            let res = if tool == "local.put" {
                self.put(&args)
            } else if tool == "local.get" {
                self.get(&args)
            } else {
                let rel = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "pathが必要です".to_string())?;
                let tid = args
                    .get("transferId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "transferIdが必要です".to_string())?;
                let res_abort = (|| -> Result<Value, TextEditError> {
                    let f = self.fetch_target(rel)?;
                    let t = Self::fetch_temp(&f, tid)?;
                    let _ = fs::remove_file(t);
                    Ok(json!({ "aborted": true }))
                })();
                res_abort
            };
            return Ok(match res {
                Ok(v) => v,
                Err(e) => json!({ "error": e.message, "code": e.code }),
            });
        }

        if tool != "local.run" {
            return Err("Unknown local tool".into());
        }

        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "command が必要です".to_string())?;
        if command.is_empty() || command.len() > 16000 {
            return Err("Command is empty or too long".into());
        }
        let timeout = args.get("timeoutSeconds").and_then(|v| v.as_i64()).unwrap_or(120);
        if !(1..=3600).contains(&timeout) {
            return Err("timeoutSecondsは1〜3600秒を指定してください".into());
        }
        let started_at = iso_now_utc();
        let rel = if relative.is_empty() { "." } else { relative };
        let cwd = self.resolve_path(rel, true, false)?;
        if !cwd.is_dir() {
            return Err("作業ディレクトリがありません".into());
        }
        self.run_shell_command(&cwd, command, timeout, &started_at, progress)
    }

    #[cfg(windows)]
    fn run_shell_command<F>(
        &self,
        cwd: &Path,
        command: &str,
        timeout: i64,
        started_at: &str,
        mut progress: F,
    ) -> Result<Value, String>
    where
        F: FnMut(Value),
    {
        let sec = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null::<core::ffi::c_void>() as *mut core::ffi::c_void,
            bInheritHandle: true.into(),
        };
        let mut out_read_raw = HANDLE::default();
        let mut out_write_raw = HANDLE::default();
        let mut err_read_raw = HANDLE::default();
        let mut err_write_raw = HANDLE::default();
        unsafe { CreatePipe(&mut out_read_raw, &mut out_write_raw, Some(&sec), 65536) }
            .map_err(|_| "Cannot create stdout pipe".to_string())?;
        let out_read = OwnedHandle(out_read_raw);
        let mut out_write = OwnedHandle(out_write_raw);
        unsafe { CreatePipe(&mut err_read_raw, &mut err_write_raw, Some(&sec), 65536) }
            .map_err(|_| "Cannot create stderr pipe".to_string())?;
        let err_read = OwnedHandle(err_read_raw);
        let mut err_write = OwnedHandle(err_write_raw);
        unsafe {
            let _ = SetHandleInformation(out_read.0, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0));
            let _ = SetHandleInformation(err_read.0, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0));
        }

        let nul_w = to_wide_null("NUL");
        let nul_raw = unsafe {
            CreateFileW(
                PCWSTR(nul_w.as_ptr()),
                GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                Some(&sec),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(|_| "NUL デバイスを開けません".to_string())?;
        let nul_guard = OwnedHandle(nul_raw);

        let job_raw = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
            .map_err(|_| "Cannot create command job".to_string())?;
        let job_guard = OwnedHandle(job_raw);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        unsafe {
            SetInformationJobObject(
                job_guard.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        }
        .map_err(|_| "Cannot configure command job".to_string())?;

        let (mut ps_bin, is_pwsh7, mut shell_label) = resolve_powershell_bin();
        let (mut shell_major, mut shell_version) = resolve_powershell_version();

        let ps_script = format!(
            "[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false); $OutputEncoding = [Console]::OutputEncoding; & {{ {}\n $global:qwenCommandOk=$?; $global:qwenNativeExit=$LASTEXITCODE }} | Out-String -Stream -Width 240 | ForEach-Object {{ [Console]::WriteLine($_) }}; if (-not $qwenCommandOk) {{ if ($qwenNativeExit) {{ exit $qwenNativeExit }}; exit 1 }}",
            command
        );
        let cwd_w = to_wide_null(&cwd.to_string_lossy());

        let mut si = STARTUPINFOW::default();
        si.cb = size_of::<STARTUPINFOW>() as u32;
        si.dwFlags = STARTF_USESTDHANDLES;
        si.hStdInput = nul_guard.0;
        si.hStdOutput = out_write.0;
        si.hStdError = err_write.0;
        let mut pi = PROCESS_INFORMATION::default();

        let spawn_ps = |bin: &Path, pi_out: &mut PROCESS_INFORMATION| {
            let cmd_line = format!(
                "{} -NoLogo -NoProfile -NonInteractive -Command {}",
                quote_win_arg(&bin.to_string_lossy()),
                quote_win_arg(&ps_script)
            );
            let bin_w = to_wide_null(&bin.to_string_lossy());
            let mut cmd_w = to_wide_null(&cmd_line);
            unsafe {
                CreateProcessW(
                    PCWSTR(bin_w.as_ptr()),
                    Some(PWSTR(cmd_w.as_mut_ptr())),
                    None,
                    None,
                    true,
                    CREATE_NO_WINDOW | CREATE_SUSPENDED,
                    None,
                    PCWSTR(cwd_w.as_ptr()),
                    &si,
                    pi_out,
                )
            }
        };

        if spawn_ps(&ps_bin, &mut pi).is_err() {
            if is_pwsh7 {
                let fb = fallback_powershell_51();
                ps_bin = fb.bin;
                shell_label = fb.label;
                shell_major = fb.major;
                shell_version = fb.version;
                spawn_ps(&ps_bin, &mut pi)
                    .map_err(|_| "PowerShellを起動できません".to_string())?;
            } else {
                return Err("PowerShellを起動できません".to_string());
            }
        }

        let proc_guard = OwnedHandle(pi.hProcess);
        let thread_guard = OwnedHandle(pi.hThread);
        if unsafe { AssignProcessToJobObject(job_guard.0, proc_guard.0) }.is_err() {
            unsafe {
                let _ = TerminateProcess(proc_guard.0, 1);
            }
            return Err("Cannot track command process tree".into());
        }
        unsafe {
            let _ = ResumeThread(thread_guard.0);
        }
        let ow = out_write.take();
        let ew = err_write.take();
        unsafe {
            let _ = CloseHandle(ow);
            let _ = CloseHandle(ew);
        }

        let mut out_bytes = Vec::<u8>::new();
        let mut err_bytes = Vec::<u8>::new();
        let mut total_bytes = 0usize;
        let mut truncated = false;
        let start = Instant::now();
        let mut last_prog = Instant::now();
        let mut changed = false;

        let make_snapshot = |o: &[u8], e: &[u8], tr: bool| {
            json!({
                "stdout": String::from_utf8_lossy(o),
                "stderr": String::from_utf8_lossy(e),
                "truncated": tr,
                "workspaceRoot": self.root.to_string_lossy(),
                "cwd": cwd.to_string_lossy(),
                "startedAt": started_at,
                "timeoutSeconds": timeout,
            })
        };

        let drain_pipe = |pipe: HANDLE,
                          dst: &mut Vec<u8>,
                          max_keep: usize,
                          total: &mut usize,
                          chg: &mut bool,
                          tr: &mut bool| {
            loop {
                let mut avail = 0u32;
                if unsafe { PeekNamedPipe(pipe, None, 0, None, Some(&mut avail), None) }.is_err()
                    || avail == 0
                {
                    break;
                }
                let mut buf = [0u8; 8192];
                let to_read = (avail as usize).min(buf.len());
                let mut count = 0u32;
                if unsafe { ReadFile(pipe, Some(&mut buf[..to_read]), Some(&mut count), None) }
                    .is_err()
                    || count == 0
                {
                    break;
                }
                let n = count as usize;
                *total += n;
                *chg = true;
                let room = max_keep.saturating_sub(dst.len());
                let keep = n.min(room);
                dst.extend_from_slice(&buf[..keep]);
                if keep < n {
                    *tr = true;
                }
                if *total > 1_048_576 {
                    break;
                }
            }
        };

        loop {
            let done = unsafe { WaitForSingleObject(proc_guard.0, 25) } == WAIT_OBJECT_0;
            drain_pipe(
                out_read.0,
                &mut out_bytes,
                16000,
                &mut total_bytes,
                &mut changed,
                &mut truncated,
            );
            drain_pipe(
                err_read.0,
                &mut err_bytes,
                8000,
                &mut total_bytes,
                &mut changed,
                &mut truncated,
            );
            if changed && (done || last_prog.elapsed() >= Duration::from_millis(300)) {
                progress(make_snapshot(&out_bytes, &err_bytes, truncated));
                last_prog = Instant::now();
                changed = false;
            }
            if total_bytes > 1_048_576 {
                unsafe {
                    let _ = TerminateJobObject(job_guard.0, 1);
                }
                return Ok(json!({
                    "error": "コマンド出力が1MiBを超えたため停止しました",
                    "stdout": String::from_utf8_lossy(&out_bytes),
                    "stderr": String::from_utf8_lossy(&err_bytes),
                    "truncated": true,
                }));
            }
            if start.elapsed() > Duration::from_secs(timeout as u64) {
                unsafe {
                    let _ = TerminateJobObject(job_guard.0, 1);
                }
                let mut res = make_snapshot(&out_bytes, &err_bytes, truncated);
                res["error"] = json!("コマンドが設定時間を超えたため停止しました");
                res["timedOut"] = json!(true);
                res["finishedAt"] = json!(iso_now_utc());
                return Ok(res);
            }
            if done {
                break;
            }
        }

        let mut exit_code = 1u32;
        unsafe {
            let _ = GetExitCodeProcess(proc_guard.0, &mut exit_code);
        }
        let mut result = make_snapshot(&out_bytes, &err_bytes, truncated);
        result["exitCode"] = json!(exit_code);
        result["shell"] = json!(shell_label);
        result["shellMajor"] = json!(shell_major);
        if let Some(ref ver) = shell_version {
            result["shellVersion"] = json!(ver);
        }
        result["finishedAt"] = json!(iso_now_utc());
        result["executed"] = json!(true);
        result["message"] = json!(if out_bytes.is_empty() && err_bytes.is_empty() {
            format!("実行されましたが出力なし（exit code: {}）", exit_code)
        } else {
            "標準出力・標準エラーを取得しました".to_string()
        });
        Ok(result)
    }

    #[cfg(not(windows))]
    fn run_shell_command<F>(
        &self,
        cwd: &Path,
        command: &str,
        timeout: i64,
        started_at: &str,
        mut progress: F,
    ) -> Result<Value, String>
    where
        F: FnMut(Value),
    {
        use parking_lot::Mutex;
        use std::process::{Command, Stdio};
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;

        let (shell_bin, _, shell_label) = resolve_powershell_bin();
        let (shell_major, shell_version) = resolve_powershell_version();

        let mut cmd = Command::new(&shell_bin);
        cmd.args(["-c", command])
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("シェル ({}) を起動できません: {}", shell_label, e))?;
        let pid = child.id();

        struct CaptureBuf {
            out: Vec<u8>,
            err: Vec<u8>,
            total: usize,
            truncated: bool,
            changed: bool,
        }
        let cap = Arc::new(Mutex::new(CaptureBuf {
            out: Vec::new(),
            err: Vec::new(),
            total: 0,
            truncated: false,
            changed: false,
        }));
        let overflow = Arc::new(AtomicBool::new(false));

        let spawn_reader = |mut pipe: Box<dyn Read + Send>,
                            is_stdout: bool,
                            cap: Arc<Mutex<CaptureBuf>>,
                            overflow: Arc<AtomicBool>| {
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match pipe.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let mut g = cap.lock();
                            g.total += n;
                            g.changed = true;
                            let (dst, max_keep) = if is_stdout {
                                (&mut g.out, 16000usize)
                            } else {
                                (&mut g.err, 8000usize)
                            };
                            let room = max_keep.saturating_sub(dst.len());
                            let keep = n.min(room);
                            dst.extend_from_slice(&buf[..keep]);
                            if keep < n {
                                g.truncated = true;
                            }
                            if g.total > 1_048_576 {
                                g.truncated = true;
                                overflow.store(true, Ordering::SeqCst);
                                break;
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            })
        };

        let out_handle = child
            .stdout
            .take()
            .map(|p| spawn_reader(Box::new(p), true, Arc::clone(&cap), Arc::clone(&overflow)));
        let err_handle = child
            .stderr
            .take()
            .map(|p| spawn_reader(Box::new(p), false, Arc::clone(&cap), Arc::clone(&overflow)));

        let kill_tree = |child: &mut std::process::Child| {
            #[cfg(unix)]
            unsafe {
                let _ = libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
        };

        let make_snapshot = |o: &[u8], e: &[u8], tr: bool| {
            json!({
                "stdout": String::from_utf8_lossy(o),
                "stderr": String::from_utf8_lossy(e),
                "truncated": tr,
                "workspaceRoot": self.root.to_string_lossy(),
                "cwd": cwd.to_string_lossy(),
                "startedAt": started_at,
                "timeoutSeconds": timeout,
            })
        };

        let start = Instant::now();
        let mut last_prog = Instant::now();
        let exit_status;
        loop {
            if overflow.load(Ordering::SeqCst) {
                kill_tree(&mut child);
                if let Some(h) = out_handle {
                    let _ = h.join();
                }
                if let Some(h) = err_handle {
                    let _ = h.join();
                }
                let g = cap.lock();
                return Ok(json!({
                    "error": "コマンド出力が1MiBを超えたため停止しました",
                    "stdout": String::from_utf8_lossy(&g.out),
                    "stderr": String::from_utf8_lossy(&g.err),
                    "truncated": true,
                }));
            }
            if start.elapsed() > Duration::from_secs(timeout as u64) {
                kill_tree(&mut child);
                if let Some(h) = out_handle {
                    let _ = h.join();
                }
                if let Some(h) = err_handle {
                    let _ = h.join();
                }
                let g = cap.lock();
                let mut res = make_snapshot(&g.out, &g.err, g.truncated);
                res["error"] = json!("コマンドが設定時間を超えたため停止しました");
                res["timedOut"] = json!(true);
                res["finishedAt"] = json!(iso_now_utc());
                return Ok(res);
            }
            match child.try_wait() {
                Ok(Some(st)) => {
                    exit_status = Some(st);
                    break;
                }
                Ok(None) => {
                    let snap = {
                        let mut g = cap.lock();
                        if g.changed && last_prog.elapsed() >= Duration::from_millis(300) {
                            g.changed = false;
                            Some(make_snapshot(&g.out, &g.err, g.truncated))
                        } else {
                            None
                        }
                    };
                    if let Some(s) = snap {
                        progress(s);
                        last_prog = Instant::now();
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(_) => {
                    exit_status = None;
                    break;
                }
            }
        }

        if let Some(h) = out_handle {
            let _ = h.join();
        }
        if let Some(h) = err_handle {
            let _ = h.join();
        }

        let (out_bytes, err_bytes, truncated, changed) = {
            let g = cap.lock();
            (g.out.clone(), g.err.clone(), g.truncated, g.changed)
        };
        if changed {
            progress(make_snapshot(&out_bytes, &err_bytes, truncated));
        }
        let exit_code: i32 = match exit_status {
            Some(st) => {
                if let Some(code) = st.code() {
                    code
                } else {
                    #[cfg(unix)]
                    {
                        use std::os::unix::process::ExitStatusExt;
                        st.signal().map(|s| 128 + s).unwrap_or(1)
                    }
                    #[cfg(not(unix))]
                    {
                        1
                    }
                }
            }
            None => 1,
        };
        let mut result = make_snapshot(&out_bytes, &err_bytes, truncated);
        result["exitCode"] = json!(exit_code);
        result["shell"] = json!(shell_label);
        result["shellMajor"] = json!(shell_major);
        if let Some(ref ver) = shell_version {
            result["shellVersion"] = json!(ver);
        }
        result["finishedAt"] = json!(iso_now_utc());
        result["executed"] = json!(true);
        result["message"] = json!(if out_bytes.is_empty() && err_bytes.is_empty() {
            format!("実行されましたが出力なし（exit code: {}）", exit_code)
        } else {
            "標準出力・標準エラーを取得しました".to_string()
        });
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn local_workspace_crud_and_run() {
        let root = std::env::temp_dir().join(format!(
            "qwen27_localws_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let ws = LocalWorkspace::new(root.clone(), vec![]).unwrap();
        let created = ws
            .perform(
                &json!({
                    "tool": "local.create",
                    "args": { "path": "hello.txt", "text": "alpha\nbeta\ngamma\n" }
                }),
                |_| {},
            )
            .unwrap();
        assert_eq!(created["written"].as_str(), Some("hello.txt"));

        let read_res = ws
            .perform(
                &json!({ "tool": "local.read", "args": { "path": "hello.txt" } }),
                |_| {},
            )
            .unwrap();
        let ver = read_res["version"].as_str().unwrap().to_string();
        assert_eq!(read_res["totalLines"].as_u64(), Some(3));

        let edit_res = ws
            .perform(
                &json!({
                    "tool": "local.edit",
                    "args": {
                        "path": "hello.txt",
                        "expectedVersion": ver,
                        "operation": "replace",
                        "oldText": "beta",
                        "newText": "BETA2"
                    }
                }),
                |_| {},
            )
            .unwrap();
        assert_eq!(edit_res["edit"]["after"].as_str(), Some("alpha\nBETA2\ngamma\n"));

        let search_res = ws
            .perform(
                &json!({ "tool": "local.search", "args": { "query": "BETA2" } }),
                |_| {},
            )
            .unwrap();
        assert_eq!(search_res["count"].as_u64(), Some(1));

        let list_res = ws
            .perform(
                &json!({ "tool": "local.list", "args": { "environment": true } }),
                |_| {},
            )
            .unwrap();
        assert_eq!(list_res["localProtocol"].as_u64(), Some(3));
        assert!(list_res.get("compilers").is_some());

        let cmd = if cfg!(windows) {
            "Write-Output 'ok-rust'"
        } else {
            "printf 'ok-rust\\n'"
        };
        let run_res = ws
            .perform(
                &json!({
                    "tool": "local.run",
                    "args": { "command": cmd, "timeoutSeconds": 10 }
                }),
                |_| {},
            )
            .unwrap();
        assert_eq!(run_res["exitCode"].as_u64(), Some(0));
        assert!(run_res["stdout"].as_str().unwrap_or("").contains("ok-rust"));

        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn powershell_detection_and_execution() {
        assert_eq!(parse_pwsh_version("7.7.0\r\n"), Some((7, "7.7.0".to_string())));
        assert_eq!(parse_pwsh_version("8.0.1\n"), Some((8, "8.0.1".to_string())));
        assert_eq!(parse_pwsh_version("5.1.26100\r\n"), None);
        assert_eq!(parse_pwsh_version(""), None);

        let candidates = pwsh_candidates();
        assert!(candidates.iter().any(|p| {
            p.to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(r"powershell\7\pwsh.exe")
        }));

        let (ps_bin, is_pwsh7, shell_label) = resolve_powershell_bin();
        assert!(!ps_bin.as_os_str().is_empty());

        let root = std::env::temp_dir().join(format!(
            "qwen27_pwsh_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let ws = LocalWorkspace::new(root.clone(), vec![]).unwrap();
        let list_res = ws
            .perform(&json!({ "tool": "local.list", "args": {} }), |_| {})
            .unwrap();
        assert_eq!(list_res["shell"].as_str(), Some(shell_label.as_str()));

        if is_pwsh7 {
            assert!(shell_label.starts_with("PowerShell 7") || shell_label.starts_with("PowerShell 8"));
            assert_eq!(list_res["shellMajor"].as_u64(), Some(7));

            let chain_ok = ws
                .perform(
                    &json!({
                        "tool": "local.run",
                        "args": { "command": "Write-Output 'part1' && Write-Output 'part2' && Write-Output '日本語-utf8'", "timeoutSeconds": 15 }
                    }),
                    |_| {},
                )
                .unwrap();
            assert_eq!(chain_ok["exitCode"].as_u64(), Some(0));
            assert_eq!(chain_ok["shell"].as_str(), Some(shell_label.as_str()));
            let out = chain_ok["stdout"].as_str().unwrap_or("");
            assert!(out.contains("part1") && out.contains("part2") && out.contains("日本語-utf8"));

            let chain_fail = ws
                .perform(
                    &json!({
                        "tool": "local.run",
                        "args": { "command": "cmd /c exit 7 && Write-Output 'never'", "timeoutSeconds": 15 }
                    }),
                    |_| {},
                )
                .unwrap();
            assert_eq!(chain_fail["exitCode"].as_u64(), Some(7));
            assert!(!chain_fail["stdout"].as_str().unwrap_or("").contains("never"));

            let chain_or = ws
                .perform(
                    &json!({
                        "tool": "local.run",
                        "args": { "command": "cmd /c exit 7 || Write-Output 'recovered'", "timeoutSeconds": 15 }
                    }),
                    |_| {},
                )
                .unwrap();
            assert_eq!(chain_or["exitCode"].as_u64(), Some(0));
            assert!(chain_or["stdout"].as_str().unwrap_or("").contains("recovered"));
        } else {
            assert_eq!(shell_label, "PowerShell 5.1");
            assert_eq!(list_res["shellMajor"].as_u64(), Some(5));
            assert!(ps_bin
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with("powershell.exe"));

            let fb = ws
                .perform(
                    &json!({
                        "tool": "local.run",
                        "args": { "command": "Write-Output 'ps51-fallback'", "timeoutSeconds": 15 }
                    }),
                    |_| {},
                )
                .unwrap();
            assert_eq!(fb["exitCode"].as_u64(), Some(0));
            assert_eq!(fb["shell"].as_str(), Some("PowerShell 5.1"));
            assert!(fb["stdout"].as_str().unwrap_or("").contains("ps51-fallback"));
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn unix_bash_execution_and_put_get_work() {
        let root = std::env::temp_dir().join(format!(
            "qwen27_unix_ws_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let ws = LocalWorkspace::new(root.clone(), vec![]).unwrap();
        let list_res = ws
            .perform(&json!({ "tool": "local.list", "args": {} }), |_| {})
            .unwrap();
        assert_eq!(list_res["platform"].as_str(), Some("linux"));
        assert!(matches!(list_res["shell"].as_str(), Some("bash" | "sh")));

        let ok = ws
            .perform(
                &json!({
                    "tool": "local.run",
                    "args": { "command": "echo part1 && echo 日本語-utf8", "timeoutSeconds": 10 }
                }),
                |_| {},
            )
            .unwrap();
        assert_eq!(ok["exitCode"].as_i64(), Some(0));
        let out = ok["stdout"].as_str().unwrap_or("");
        assert!(out.contains("part1") && out.contains("日本語-utf8"));

        let fail = ws
            .perform(
                &json!({
                    "tool": "local.run",
                    "args": { "command": "exit 7", "timeoutSeconds": 10 }
                }),
                |_| {},
            )
            .unwrap();
        assert_eq!(fail["exitCode"].as_i64(), Some(7));

        let payload = b"hello-unix-transfer";
        let sha = sha256_lower_hex(payload);
        let put_res = ws
            .perform(
                &json!({
                    "tool": "local.put",
                    "args": {
                        "path": "bin.dat",
                        "transferId": "t1",
                        "sha256": sha,
                        "base64": B64.encode(payload),
                        "index": 0,
                        "offset": 0,
                        "total": payload.len(),
                        "final": true,
                        "overwrite": false
                    }
                }),
                |_| {},
            )
            .unwrap();
        assert_eq!(put_res["bytes"].as_u64(), Some(payload.len() as u64));

        let get_res = ws
            .perform(
                &json!({
                    "tool": "local.get",
                    "args": { "path": "bin.dat", "offset": 0, "length": 1024 }
                }),
                |_| {},
            )
            .unwrap();
        assert_eq!(get_res["sha256"].as_str(), Some(sha.as_str()));

        let _ = fs::remove_dir_all(&root);
    }
}
