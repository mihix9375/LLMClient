use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesktopWindowEntry {
    pub id: isize,
    pub title: String,
    pub elevated: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub process_name: String,
}

#[derive(Clone, Debug)]
pub enum ClipboardPayload {
    #[cfg_attr(not(windows), allow(dead_code))]
    Files(Vec<PathBuf>),
    PngBytes(Vec<u8>),
    #[cfg_attr(not(windows), allow(dead_code))]
    DibBytes(Vec<u8>),
    Text,
    Empty,
}

#[allow(dead_code)]
pub fn quote_win_arg(arg: &str) -> String {
    if arg.is_empty() || arg.chars().any(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '"')) {
        let mut out = String::from("\"");
        let mut backslashes = 0usize;
        for ch in arg.chars() {
            if ch == '\\' {
                backslashes += 1;
            } else if ch == '"' {
                for _ in 0..(backslashes * 2 + 1) {
                    out.push('\\');
                }
                out.push('"');
                backslashes = 0;
            } else {
                for _ in 0..backslashes {
                    out.push('\\');
                }
                backslashes = 0;
                out.push(ch);
            }
        }
        for _ in 0..(backslashes * 2) {
            out.push('\\');
        }
        out.push('"');
        out
    } else {
        arg.to_string()
    }
}

pub fn executable_dir() -> Result<PathBuf, String> {
    let exe = executable_path()?;
    exe.parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| "実行ファイルの親フォルダーを取得できません".to_string())
}

pub fn open_external_target(target: &str) -> Result<(), String> {
    open::that(target).map_err(|e| format!("外部で開けません ({}): {}", target, e))
}

#[cfg(windows)]
mod platform {
use super::{ClipboardPayload, DesktopWindowEntry};
use std::ffi::OsStr;
use std::fs;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, GENERIC_WRITE, HANDLE, HGLOBAL, HMODULE, HWND, LPARAM,
    RECT, RPC_E_CHANGED_MODE,
};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_CLOAK, DWMWA_TEXT_COLOR,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FindClose, FindFirstFileW, GetFileAttributesW, MoveFileExW, WriteFile,
    CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_SHARE_NONE,
    INVALID_FILE_ATTRIBUTES, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, WIN32_FIND_DATAW,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Console::{AttachConsole, SetConsoleCP, SetConsoleOutputCP, ATTACH_PARENT_PROCESS};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
    QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::SystemInformation::{ComputerNameNetBIOS, GetComputerNameExW};
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_ALLOWMULTISELECT, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR,
    OPENFILENAMEW,
};
use windows::Win32::UI::Shell::{
    DragQueryFileW, FileOpenDialog, IFileOpenDialog, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, HDROP,
    SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FlashWindowEx, GetWindow, GetWindowPlacement, GetWindowTextLengthW,
    GetWindowTextW, GetWindowThreadProcessId, IsWindow, IsWindowVisible, SetWindowPlacement,
    ShowWindow, FLASHWINFO, FLASHW_ALL, FLASHW_TIMERNOFG, GW_OWNER, SW_SHOW, SW_SHOWMAXIMIZED,
    SW_SHOWNORMAL, WINDOWPLACEMENT,
};

const SECURITY_MANDATORY_MEDIUM_RID_U32: u32 = 0x0000_2000;
const SECURITY_MANDATORY_HIGH_RID_U32: u32 = 0x0000_3000;
const BLOCKED_WINDOW_EXES: &[&str] = &[
    "codex.exe",
    "chatgpt.exe",
    "credentialuibroker.exe",
    "logonui.exe",
    "lockapp.exe",
    "consent.exe",
];

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn to_wide_null(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = OsStr::new(s).encode_wide().collect();
    v.push(0);
    v
}

pub fn from_wide_nul(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    std::ffi::OsString::from_wide(&buf[..len])
        .to_string_lossy()
        .into_owned()
}

pub fn attach_parent_console_utf8() {
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_ok() {
            let _ = SetConsoleOutputCP(65001);
            let _ = SetConsoleCP(65001);
        } else {
            let _ = SetConsoleOutputCP(65001);
            let _ = SetConsoleCP(65001);
        }
    }
}

pub fn executable_path() -> Result<PathBuf, String> {
    let mut size = 512usize;
    loop {
        let mut buf = vec![0u16; size];
        let written = unsafe { GetModuleFileNameW(Some(HMODULE::default()), &mut buf) } as usize;
        if written == 0 {
            return Err("実行ファイルのパスを取得できません".into());
        }
        if written < buf.len() {
            buf.truncate(written);
            return Ok(PathBuf::from(std::ffi::OsString::from_wide(&buf)));
        }
        size *= 2;
        if size > 32768 {
            return Err("実行ファイルのパスが長すぎます".into());
        }
    }
}

const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;

pub fn no_links(path: &Path) -> Result<(), String> {
    let mut cur = PathBuf::new();
    for comp in path.components() {
        cur.push(comp);
        if matches!(comp, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        let wide = to_wide_null(&cur.to_string_lossy());
        let attrs = unsafe { GetFileAttributesW(PCWSTR(wide.as_ptr())) };
        if attrs != INVALID_FILE_ATTRIBUTES && (attrs & FILE_ATTRIBUTE_REPARSE_POINT.0) != 0 {
            let mut find_data = WIN32_FIND_DATAW::default();
            let is_link = match unsafe { FindFirstFileW(PCWSTR(wide.as_ptr()), &mut find_data) } {
                Ok(h) => {
                    unsafe {
                        let _ = FindClose(h);
                    }
                    find_data.dwReserved0 == IO_REPARSE_TAG_SYMLINK
                        || find_data.dwReserved0 == IO_REPARSE_TAG_MOUNT_POINT
                }
                Err(_) => true,
            };
            if is_link {
                return Err(format!(
                    "シンボリックリンクまたはジャンクション経由のアクセスは拒否されました: {}",
                    cur.display()
                ));
            }
        }
    }
    Ok(())
}

pub fn write_file_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| "保存先の親フォルダーが不正です".to_string())?;
    no_links(parent)?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("保存先フォルダーの作成に失敗しました: {}", e))?;
    if path.exists() {
        no_links(path)?;
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!(
        ".qwen-tmp-{}-{}-{}",
        unsafe { GetCurrentProcessId() },
        stamp,
        seq
    );
    let tmp_path = parent.join(tmp_name);
    let wide_tmp = to_wide_null(&tmp_path.to_string_lossy());
    let wide_dst = to_wide_null(&path.to_string_lossy());

    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide_tmp.as_ptr()),
            GENERIC_WRITE.0,
            FILE_SHARE_NONE,
            None,
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .map_err(|e| format!("一時ファイルを作成できません ({}): {}", tmp_path.display(), e))?;

    let mut written_total = 0usize;
    let write_res = (|| -> Result<(), String> {
        while written_total < data.len() {
            let mut chunk_written = 0u32;
            let chunk = &data[written_total..data.len().min(written_total + 1024 * 1024)];
            unsafe { WriteFile(handle, Some(chunk), Some(&mut chunk_written), None) }
                .map_err(|e| format!("ファイル書き込みに失敗しました: {}", e))?;
            if chunk_written == 0 {
                return Err("ファイル書き込みが中断されました".into());
            }
            written_total += chunk_written as usize;
        }
        Ok(())
    })();

    unsafe {
        let _ = CloseHandle(handle);
    }
    if let Err(err) = write_res {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }

    if let Err(e) = unsafe {
        MoveFileExW(
            PCWSTR(wide_tmp.as_ptr()),
            PCWSTR(wide_dst.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!("ファイルを確定できません ({}): {}", path.display(), e));
    }
    Ok(())
}

unsafe fn process_integrity_rid(process: HANDLE) -> u32 {
    if process.is_invalid() || process.0.is_null() {
        return SECURITY_MANDATORY_MEDIUM_RID_U32;
    }
    let mut token = HANDLE::default();
    if OpenProcessToken(process, TOKEN_QUERY, &mut token).is_err() {
        return SECURITY_MANDATORY_MEDIUM_RID_U32;
    }
    let mut len = 0u32;
    let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut len);
    let mut rid = SECURITY_MANDATORY_MEDIUM_RID_U32;
    if len > 0 {
        let mut buf = vec![0u8; len as usize];
        if GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buf.as_mut_ptr().cast()),
            len,
            &mut len,
        )
        .is_ok()
        {
            let tml = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
            if !tml.Label.Sid.is_invalid() {
                let count_ptr = GetSidSubAuthorityCount(tml.Label.Sid);
                if !count_ptr.is_null() && *count_ptr > 0 {
                    let sub_ptr = GetSidSubAuthority(tml.Label.Sid, (*count_ptr - 1) as u32);
                    if !sub_ptr.is_null() {
                        rid = *sub_ptr;
                    }
                }
            }
        }
    }
    let _ = CloseHandle(token);
    rid
}

pub fn is_current_process_elevated() -> bool {
    unsafe { process_integrity_rid(GetCurrentProcess()) >= SECURITY_MANDATORY_HIGH_RID_U32 }
}

pub fn is_window_process_elevated(hwnd_val: isize) -> bool {
    if hwnd_val == 0 {
        return false;
    }
    let hwnd = HWND(hwnd_val as *mut core::ffi::c_void);
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    if pid == 0 {
        return false;
    }
    match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
        Ok(process) => {
            let elevated = unsafe { process_integrity_rid(process) >= SECURITY_MANDATORY_HIGH_RID_U32 };
            unsafe {
                let _ = CloseHandle(process);
            }
            elevated
        }
        Err(e) => e.code() == ERROR_ACCESS_DENIED.to_hresult(),
    }
}

unsafe fn inspect_window_process(hwnd: HWND, include_self_pid: u32) -> Option<(String, bool)> {
    if !IsWindowVisible(hwnd).as_bool() {
        return None;
    }
    if GetWindow(hwnd, GW_OWNER).is_ok() {
        return None;
    }
    if GetWindowTextLengthW(hwnd) <= 0 {
        return None;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == 0 || pid == include_self_pid {
        return None;
    }
    let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
    let mut path_buf = vec![0u16; 32768];
    let mut size = path_buf.len() as u32;
    let ok = QueryFullProcessImageNameW(
        process,
        PROCESS_NAME_FORMAT(0),
        PWSTR(path_buf.as_mut_ptr()),
        &mut size,
    )
    .is_ok();
    let elevated = if ok {
        process_integrity_rid(process) >= SECURITY_MANDATORY_HIGH_RID_U32
    } else {
        false
    };
    let _ = CloseHandle(process);
    if !ok {
        return None;
    }
    let full = from_wide_nul(&path_buf[..size as usize]);
    let file_name = Path::new(&full)
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if BLOCKED_WINDOW_EXES.iter().any(|&b| b == file_name) || file_name.starts_with("qwen27") {
        return None;
    }
    Some((file_name, elevated))
}

pub fn is_window_eligible(hwnd_val: isize) -> bool {
    if hwnd_val == 0 {
        return false;
    }
    let hwnd = HWND(hwnd_val as *mut core::ffi::c_void);
    let self_pid = unsafe { GetCurrentProcessId() };
    unsafe { inspect_window_process(hwnd, self_pid).is_some() }
}

pub fn is_valid_window(hwnd_val: isize) -> bool {
    if hwnd_val == 0 {
        return false;
    }
    unsafe { IsWindow(Some(HWND(hwnd_val as *mut core::ffi::c_void))).as_bool() }
}

pub fn desktop_windows() -> Vec<DesktopWindowEntry> {
    struct EnumCtx {
        self_pid: u32,
        items: Vec<DesktopWindowEntry>,
    }
    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        let ctx = &mut *(lparam.0 as *mut EnumCtx);
        if let Some((process_name, elevated)) = inspect_window_process(hwnd, ctx.self_pid) {
            let mut buf = [0u16; 512];
            let len = GetWindowTextW(hwnd, &mut buf);
            if len > 0 {
                let title = from_wide_nul(&buf[..len as usize]);
                if !title.is_empty() {
                    ctx.items.push(DesktopWindowEntry {
                        id: hwnd.0 as isize,
                        title,
                        elevated,
                        process_name,
                    });
                }
            }
        }
        true.into()
    }
    let mut ctx = EnumCtx {
        self_pid: unsafe { GetCurrentProcessId() },
        items: Vec::new(),
    };
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut EnumCtx as isize));
    }
    ctx.items
}

pub fn computer_name() -> String {
    let mut buf = [0u16; 256];
    let mut size = buf.len() as u32;
    let ok = unsafe {
        GetComputerNameExW(
            ComputerNameNetBIOS,
            Some(PWSTR(buf.as_mut_ptr())),
            &mut size,
        )
    }
    .is_ok();
    if ok && size > 0 {
        from_wide_nul(&buf[..size as usize])
    } else {
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "WindowsPC".into())
    }
}

pub fn clipboard_set_text(text: &str) -> Result<(), String> {
    let wide = to_wide_null(text);
    let bytes = wide.len() * size_of::<u16>();
    unsafe {
        OpenClipboard(None).map_err(|_| "クリップボードを開けません".to_string())?;
        let res = (|| -> Result<(), String> {
            EmptyClipboard().map_err(|_| "クリップボードを初期化できません".to_string())?;
            let hmem: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, bytes)
                .map_err(|_| "クリップボード用メモリを確保できません".to_string())?;
            let ptr = GlobalLock(hmem) as *mut u16;
            if ptr.is_null() {
                return Err("クリップボード用メモリをロックできません".into());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            let _ = GlobalUnlock(hmem);
            SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(hmem.0)))
                .map_err(|_| "クリップボードへ書き込めません".to_string())?;
            Ok(())
        })();
        let _ = CloseClipboard();
        res
    }
}

pub fn clipboard_read_payload() -> Result<ClipboardPayload, String> {
    unsafe fn read_hglobal_bytes(fmt: u32, min_len: usize) -> Option<Vec<u8>> {
        let handle = GetClipboardData(fmt).ok()?;
        let hglobal = HGLOBAL(handle.0);
        let sz = GlobalSize(hglobal);
        if sz < min_len {
            return None;
        }
        let ptr = GlobalLock(hglobal) as *const u8;
        if ptr.is_null() {
            return None;
        }
        let data = std::slice::from_raw_parts(ptr, sz).to_vec();
        let _ = GlobalUnlock(hglobal);
        Some(data)
    }

    unsafe {
        OpenClipboard(None).map_err(|_| "クリップボードを開けません".to_string())?;
        let res = (|| -> Result<ClipboardPayload, String> {
            if let Ok(handle) = GetClipboardData(CF_HDROP.0 as u32) {
                let hdrop = HDROP(handle.0);
                let count = DragQueryFileW(hdrop, u32::MAX, None);
                let mut files = Vec::new();
                for i in 0..count {
                    let len = DragQueryFileW(hdrop, i, None);
                    if len > 0 {
                        let mut buf = vec![0u16; len as usize + 2];
                        let got = DragQueryFileW(hdrop, i, Some(&mut buf));
                        if got > 0 {
                            files.push(PathBuf::from(from_wide_nul(&buf[..got as usize])));
                        }
                    }
                }
                if !files.is_empty() {
                    return Ok(ClipboardPayload::Files(files));
                }
            }
            let png_w = to_wide_null("PNG");
            let png_fmt = RegisterClipboardFormatW(PCWSTR(png_w.as_ptr()));
            if png_fmt != 0 {
                if let Some(data) = read_hglobal_bytes(png_fmt, 8) {
                    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
                        return Ok(ClipboardPayload::PngBytes(data));
                    }
                }
            }
            if let Some(data) = read_hglobal_bytes(CF_DIBV5.0 as u32, 40) {
                return Ok(ClipboardPayload::DibBytes(data));
            }
            if let Some(data) = read_hglobal_bytes(CF_DIB.0 as u32, 40) {
                return Ok(ClipboardPayload::DibBytes(data));
            }
            if let Ok(handle) = GetClipboardData(CF_UNICODETEXT.0 as u32) {
                let hglobal = HGLOBAL(handle.0);
                let sz = GlobalSize(hglobal);
                if sz >= 2 {
                    let ptr = GlobalLock(hglobal) as *const u16;
                    if !ptr.is_null() {
                        let words = std::slice::from_raw_parts(ptr, sz / 2);
                        let text = from_wide_nul(words);
                        let _ = GlobalUnlock(hglobal);
                        if !text.is_empty() {
                            return Ok(ClipboardPayload::Text);
                        }
                    }
                }
            }
            Ok(ClipboardPayload::Empty)
        })();
        let _ = CloseClipboard();
        res
    }
}

pub fn pick_files_dialog(owner_hwnd: isize, title: &str) -> Vec<PathBuf> {
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let uninit = hr != RPC_E_CHANGED_MODE && hr.is_ok();
    let res = (|| {
        let mut buffer = vec![0u16; 65536];
        let title_w = to_wide_null(title);
        let filter_w: Vec<u16> = "すべてのファイル (*.*)\0*.*\0画像・資料\0*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.gif;*.txt;*.md;*.json;*.pdf\0\0"
            .encode_utf16()
            .collect();
        let mut ofn: OPENFILENAMEW = unsafe { zeroed() };
        ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = HWND(owner_hwnd as *mut core::ffi::c_void);
        ofn.lpstrFilter = PCWSTR(filter_w.as_ptr());
        ofn.nFilterIndex = 1;
        ofn.lpstrFile = PWSTR(buffer.as_mut_ptr());
        ofn.nMaxFile = buffer.len() as u32;
        ofn.lpstrTitle = PCWSTR(title_w.as_ptr());
        ofn.Flags = OFN_ALLOWMULTISELECT | OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_NOCHANGEDIR;

        if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
            return Vec::new();
        }
        let first_end = buffer.iter().position(|&c| c == 0).unwrap_or(0);
        if first_end == 0 {
            return Vec::new();
        }
        let first = from_wide_nul(&buffer[..first_end]);
        let mut pos = first_end + 1;
        if pos >= buffer.len() || buffer[pos] == 0 {
            return vec![PathBuf::from(first)];
        }
        let base = PathBuf::from(first);
        let mut out = Vec::new();
        while pos < buffer.len() && buffer[pos] != 0 {
            let end = buffer[pos..]
                .iter()
                .position(|&c| c == 0)
                .map(|rel| pos + rel)
                .unwrap_or(buffer.len());
            let name = from_wide_nul(&buffer[pos..end]);
            if !name.is_empty() {
                out.push(base.join(name));
            }
            pos = end + 1;
        }
        out
    })();
    if uninit {
        unsafe { CoUninitialize() };
    }
    res
}

pub fn pick_folder_dialog(owner_hwnd: isize, title: &str) -> Option<PathBuf> {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let uninit = hr != RPC_E_CHANGED_MODE && hr.is_ok();
        let res = (|| -> Option<PathBuf> {
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            let opts = dialog.GetOptions().ok()?;
            dialog
                .SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)
                .ok()?;
            let title_w = to_wide_null(title);
            let _ = dialog.SetTitle(PCWSTR(title_w.as_ptr()));
            let owner = if owner_hwnd != 0 {
                Some(HWND(owner_hwnd as *mut core::ffi::c_void))
            } else {
                None
            };
            dialog.Show(owner).ok()?;
            let item = dialog.GetResult().ok()?;
            let ptr = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
            if ptr.is_null() {
                return None;
            }
            let mut len = 0usize;
            while *ptr.0.add(len) != 0 {
                len += 1;
            }
            let path_str = from_wide_nul(std::slice::from_raw_parts(ptr.0, len));
            CoTaskMemFree(Some(ptr.0.cast()));
            if path_str.is_empty() {
                None
            } else {
                Some(PathBuf::from(path_str))
            }
        })();
        if uninit {
            CoUninitialize();
        }
        res
    }
}

pub fn apply_dwm_appearance(hwnd_val: isize, dark: bool) {
    if hwnd_val == 0 {
        return;
    }
    let hwnd = HWND(hwnd_val as *mut core::ffi::c_void);
    let dark_val: i32 = if dark { 1 } else { 0 };
    let corner_round: u32 = 2; // DWMWCP_ROUND
    let caption_color: u32 = if dark { 0x001D_1410 } else { 0x00F7_F1EE };
    let text_color: u32 = if dark { 0x00FF_F2EE } else { 0x002A_170F };
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&dark_val as *const i32).cast(),
            size_of::<i32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&corner_round as *const u32).cast(),
            size_of::<u32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            (&caption_color as *const u32).cast(),
            size_of::<u32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_TEXT_COLOR,
            (&text_color as *const u32).cast(),
            size_of::<u32>() as u32,
        );
    }
}

pub fn set_window_cloaked(hwnd_val: isize, cloaked: bool) {
    if hwnd_val == 0 {
        return;
    }
    let hwnd = HWND(hwnd_val as *mut core::ffi::c_void);
    let val: i32 = if cloaked { 1 } else { 0 };
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            (&val as *const i32).cast(),
            size_of::<i32>() as u32,
        );
    }
}

pub fn show_window_initial(hwnd_val: isize, geom: (Option<i32>, Option<i32>, i32, i32, bool)) {
    if hwnd_val == 0 {
        return;
    }
    if let (Some(x), Some(y)) = (geom.0, geom.1) {
        restore_window_placement(hwnd_val, x, y, geom.2, geom.3, geom.4);
    } else {
        let hwnd = HWND(hwnd_val as *mut core::ffi::c_void);
        unsafe {
            let _ = ShowWindow(hwnd, if geom.4 { SW_SHOWMAXIMIZED } else { SW_SHOWNORMAL });
        }
    }
}

pub fn flash_taskbar(hwnd_val: isize) {
    if hwnd_val == 0 {
        return;
    }
    let mut info = FLASHWINFO {
        cbSize: size_of::<FLASHWINFO>() as u32,
        hwnd: HWND(hwnd_val as *mut core::ffi::c_void),
        dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
        uCount: 4,
        dwTimeout: 0,
    };
    unsafe {
        let _ = FlashWindowEx(&mut info);
    }
}

pub fn capture_window_placement(hwnd_val: isize) -> Option<(i32, i32, i32, i32, bool)> {
    if hwnd_val == 0 {
        return None;
    }
    let hwnd = HWND(hwnd_val as *mut core::ffi::c_void);
    let mut wp = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    if unsafe { GetWindowPlacement(hwnd, &mut wp) }.is_err() {
        return None;
    }
    let r = wp.rcNormalPosition;
    let w = (r.right - r.left).max(1000);
    let h = (r.bottom - r.top).max(660);
    let maximized = wp.showCmd == SW_SHOWMAXIMIZED.0 as u32;
    Some((r.left, r.top, w, h, maximized))
}

pub fn restore_window_placement(hwnd_val: isize, x: i32, y: i32, w: i32, h: i32, maximized: bool) {
    if hwnd_val == 0 {
        return;
    }
    let hwnd = HWND(hwnd_val as *mut core::ffi::c_void);
    let w = w.max(1000);
    let h = h.max(660);
    let mut rc = RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    };
    let monitor = unsafe { MonitorFromRect(&rc, MONITOR_DEFAULTTONEAREST) };
    let mut mi = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut mi) }.as_bool() {
        let wa = mi.rcWork;
        let cw = w.min((wa.right - wa.left).max(800));
        let ch = h.min((wa.bottom - wa.top).max(600));
        rc.left = x.clamp(wa.left, (wa.right - cw).max(wa.left));
        rc.top = y.clamp(wa.top, (wa.bottom - ch).max(wa.top));
        rc.right = rc.left + cw;
        rc.bottom = rc.top + ch;
    }
    let wp = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        flags: Default::default(),
        showCmd: if maximized {
            SW_SHOWMAXIMIZED.0 as u32
        } else {
            SW_SHOWNORMAL.0 as u32
        },
        ptMinPosition: Default::default(),
        ptMaxPosition: Default::default(),
        rcNormalPosition: rc,
    };
    unsafe {
        let _ = SetWindowPlacement(hwnd, &wp);
        if !maximized {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
    }
}
}

#[cfg(windows)]
pub use platform::*;

#[cfg(not(windows))]
mod platform {
    use super::{ClipboardPayload, DesktopWindowEntry};
    use std::fs;
    use std::io::Write;
    use std::path::{Component, Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(1);

    pub fn executable_path() -> Result<PathBuf, String> {
        std::env::current_exe()
            .map_err(|e| format!("実行ファイルのパスを取得できません: {}", e))
    }

    pub fn no_links(path: &Path) -> Result<(), String> {
        let mut cur = PathBuf::new();
        for comp in path.components() {
            cur.push(comp);
            if matches!(
                comp,
                Component::Prefix(_) | Component::RootDir | Component::CurDir | Component::ParentDir
            ) {
                continue;
            }
            if let Ok(meta) = fs::symlink_metadata(&cur) {
                if meta.file_type().is_symlink() {
                    return Err(format!(
                        "シンボリックリンク経由のアクセスは拒否されました: {}",
                        cur.display()
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn write_file_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| "保存先の親フォルダーが不正です".to_string())?;
        no_links(parent)?;
        fs::create_dir_all(parent)
            .map_err(|e| format!("保存先フォルダーの作成に失敗しました: {}", e))?;
        if fs::symlink_metadata(path).is_ok() {
            no_links(path)?;
        }
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_path = parent.join(format!(
            ".qwen-tmp-{}-{}-{}",
            std::process::id(),
            stamp,
            seq
        ));

        let write_res = (|| -> Result<(), String> {
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp_path)
                .map_err(|e| format!("一時ファイルを作成できません ({}): {}", tmp_path.display(), e))?;
            f.write_all(data)
                .map_err(|e| format!("ファイル書き込みに失敗しました: {}", e))?;
            f.sync_all()
                .map_err(|e| format!("ファイル同期に失敗しました: {}", e))?;
            Ok(())
        })();

        if let Err(err) = write_res {
            let _ = fs::remove_file(&tmp_path);
            return Err(err);
        }
        if let Err(e) = fs::rename(&tmp_path, path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(format!("ファイルを確定できません ({}): {}", path.display(), e));
        }
        Ok(())
    }

    pub fn is_current_process_elevated() -> bool {
        #[cfg(unix)]
        {
            unsafe { libc::geteuid() == 0 }
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    #[allow(dead_code)]
    pub fn is_window_process_elevated(_hwnd_val: isize) -> bool {
        false
    }

    #[allow(dead_code)]
    pub fn is_window_eligible(_hwnd_val: isize) -> bool {
        false
    }

    pub fn is_valid_window(_hwnd_val: isize) -> bool {
        false
    }

    pub fn desktop_windows() -> Vec<DesktopWindowEntry> {
        Vec::new()
    }

    pub fn computer_name() -> String {
        if let Ok(raw) = fs::read_to_string("/etc/hostname") {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        #[cfg(unix)]
        {
            let mut buf = [0u8; 256];
            let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
            if rc == 0 {
                let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                let name = String::from_utf8_lossy(&buf[..len]).trim().to_string();
                if !name.is_empty() {
                    return name;
                }
            }
        }
        std::env::var("HOSTNAME")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Linux".into())
    }

    fn pipe_to_cmd(prog: &str, args: &[&str], data: &[u8]) -> bool {
        let Ok(mut child) = Command::new(prog)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            return false;
        };
        if let Some(mut stdin) = child.stdin.take() {
            if stdin.write_all(data).is_err() {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
        child.wait().map(|s| s.success()).unwrap_or(false)
    }

    pub fn clipboard_set_text(text: &str) -> Result<(), String> {
        let bytes = text.as_bytes();
        if std::env::var_os("WAYLAND_DISPLAY").is_some() && pipe_to_cmd("wl-copy", &[], bytes) {
            return Ok(());
        }
        if pipe_to_cmd("xclip", &["-selection", "clipboard"], bytes)
            || pipe_to_cmd("xsel", &["--clipboard", "--input"], bytes)
            || pipe_to_cmd("wl-copy", &[], bytes)
        {
            return Ok(());
        }
        Err("クリップボードツール (wl-copy / xclip / xsel) が見つかりません".into())
    }

    pub fn clipboard_read_payload() -> Result<ClipboardPayload, String> {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            if let Ok(types) = Command::new("wl-paste").arg("--list-types").output() {
                if types.status.success() {
                    let list = String::from_utf8_lossy(&types.stdout);
                    if list.lines().any(|l| l.trim() == "image/png") {
                        if let Ok(out) = Command::new("wl-paste")
                            .args(["--type", "image/png"])
                            .output()
                        {
                            if out.status.success()
                                && out.stdout.starts_with(b"\x89PNG\r\n\x1a\n")
                            {
                                return Ok(ClipboardPayload::PngBytes(out.stdout));
                            }
                        }
                    }
                    if list.lines().any(|l| l.trim().starts_with("text/")) {
                        return Ok(ClipboardPayload::Text);
                    }
                }
            }
        }
        if std::env::var_os("DISPLAY").is_some() {
            if let Ok(targets) = Command::new("xclip")
                .args(["-selection", "clipboard", "-o", "-t", "TARGETS"])
                .output()
            {
                if targets.status.success() {
                    let list = String::from_utf8_lossy(&targets.stdout);
                    if list.lines().any(|l| l.trim() == "image/png") {
                        if let Ok(out) = Command::new("xclip")
                            .args(["-selection", "clipboard", "-o", "-t", "image/png"])
                            .output()
                        {
                            if out.status.success()
                                && out.stdout.starts_with(b"\x89PNG\r\n\x1a\n")
                            {
                                return Ok(ClipboardPayload::PngBytes(out.stdout));
                            }
                        }
                    }
                    if list
                        .lines()
                        .any(|l| matches!(l.trim(), "UTF8_STRING" | "STRING" | "text/plain"))
                    {
                        return Ok(ClipboardPayload::Text);
                    }
                }
            }
        }
        Ok(ClipboardPayload::Empty)
    }

    pub fn pick_files_dialog(_owner_hwnd: isize, title: &str) -> Vec<PathBuf> {
        if let Ok(out) = Command::new("zenity")
            .args([
                "--file-selection",
                "--multiple",
                "--separator=\n",
                "--title",
                title,
            ])
            .output()
        {
            if out.status.success() {
                return String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .collect();
            }
        }
        if let Ok(out) = Command::new("kdialog")
            .args([
                "--getopenfilename",
                ".",
                "--multiple",
                "--separate-output",
                "--title",
                title,
            ])
            .output()
        {
            if out.status.success() {
                return String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .collect();
            }
        }
        Vec::new()
    }

    pub fn pick_folder_dialog(_owner_hwnd: isize, title: &str) -> Option<PathBuf> {
        if let Ok(out) = Command::new("zenity")
            .args(["--file-selection", "--directory", "--title", title])
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(PathBuf::from(s));
                }
            }
        }
        if let Ok(out) = Command::new("kdialog")
            .args(["--getexistingdirectory", ".", "--title", title])
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(PathBuf::from(s));
                }
            }
        }
        None
    }

    pub fn apply_dwm_appearance(_hwnd_val: isize, _dark: bool) {}

    pub fn set_window_cloaked(_hwnd_val: isize, _cloaked: bool) {}

    pub fn show_window_initial(
        _hwnd_val: isize,
        _geom: (Option<i32>, Option<i32>, i32, i32, bool),
    ) {
    }

    pub fn flash_taskbar(_hwnd_val: isize) {}

    pub fn capture_window_placement(_hwnd_val: isize) -> Option<(i32, i32, i32, i32, bool)> {
        None
    }

    #[allow(dead_code)]
    pub fn restore_window_placement(
        _hwnd_val: isize,
        _x: i32,
        _y: i32,
        _w: i32,
        _h: i32,
        _maximized: bool,
    ) {
    }
}

#[cfg(not(windows))]
pub use platform::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn quote_arg_handles_spaces_and_quotes() {
        assert_eq!(quote_win_arg("simple"), "simple");
        assert_eq!(quote_win_arg("a b"), "\"a b\"");
        assert_eq!(quote_win_arg("a\"b\\"), "\"a\\\"b\\\\\"");
    }

    #[test]
    fn write_file_atomic_and_no_links_roundtrip() {
        let base = std::env::temp_dir().join(format!(
            "qwen27_winutil_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        let file = base.join("sample.txt");
        write_file_atomic(&file, b"hello rust\n").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello rust\n");
        no_links(&file).unwrap();
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn executable_dir_is_available() {
        let dir = executable_dir().unwrap();
        assert!(dir.is_dir());
    }
}
