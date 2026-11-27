#[cfg(windows)]
mod platform {
use crate::desktop_keys::{
    compute_key_diff, is_ui_key_for_sustain, make_single_input, parse_keys, send_inputs,
    send_single_input, validate_drag_params, validate_sustain_params, validate_video_params,
};
use crate::image_codec::{
    compute_luma_motion_score, encode_hbitmap_jpeg, encode_hbitmap_to_budget, extract_luma_32x18,
    grab_window_bitmap, ComScope,
};
use crate::local_ws::LocalWorkspace;
use crate::win_util::{
    attach_parent_console_utf8, from_wide_nul, is_current_process_elevated, is_window_eligible,
    is_window_process_elevated, quote_win_arg, to_wide_null,
};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use parking_lot::{Condvar, Mutex};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::mem::size_of;
use std::path::PathBuf;
use std::ptr::null;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, FillRect, GetDC,
    GetStockObject, ReleaseDC, SelectObject, DKGRAY_BRUSH, HBRUSH, HGDIOBJ, WHITE_BRUSH,
};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Pipes::{CreatePipe, PeekNamedPipe};
use windows::Win32::System::Threading::{
    CreateProcessW, TerminateProcess, WaitForSingleObject, CREATE_NO_WINDOW, PROCESS_INFORMATION,
    STARTF_USESTDHANDLES, STARTUPINFOW,
};
use windows::Win32::Foundation::{HANDLE_FLAGS, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0};
use windows::Win32::Foundation::SetHandleInformation;
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationElement, IUIAutomationTreeWalker,
    IUIAutomationValuePattern, UIA_ValuePatternId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MapVirtualKeyW, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, VK_ESCAPE, VK_LBUTTON, VK_RBUTTON, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, BringWindowToTop, GetAncestor, GetDesktopWindow, GetForegroundWindow,
    GetSystemMetrics, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindow, SetForegroundWindow, SetWindowPos, ShowWindow, WindowFromPoint, ASFW_ANY,
    GA_ROOTOWNER, HWND_NOTOPMOST, HWND_TOPMOST, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_SHOWNOACTIVATE,
    WHEEL_DELTA,
};

struct SustainShared {
    active: bool,
    stop_thread: bool,
    mode: String,
    names: Vec<String>,
    target_vks: Vec<u16>,
    pressed_vks: Vec<u16>,
    expires_at: Option<Instant>,
    repeat_down: bool,
    next_toggle_at: Option<Instant>,
    auto_released: bool,
    release_reason: String,
    target_hwnd: isize,
    target_pid: u32,
}

impl Default for SustainShared {
    fn default() -> Self {
        Self {
            active: false,
            stop_thread: false,
            mode: "hold".into(),
            names: Vec::new(),
            target_vks: Vec::new(),
            pressed_vks: Vec::new(),
            expires_at: None,
            repeat_down: false,
            next_toggle_at: None,
            auto_released: false,
            release_reason: String::new(),
            target_hwnd: 0,
            target_pid: 0,
        }
    }
}

fn belongs_to_target(candidate: HWND, target: HWND, target_pid: u32) -> bool {
    if candidate.0.is_null() {
        return false;
    }
    if candidate == target {
        return true;
    }
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(candidate, Some(&mut pid));
    }
    if target_pid != 0 && pid == target_pid {
        return true;
    }
    unsafe { GetAncestor(candidate, GA_ROOTOWNER) == GetAncestor(target, GA_ROOTOWNER) }
}

fn release_sustain_locked(st: &mut SustainShared, reason: &str, auto_released: bool) -> Vec<String> {
    let prev = st.names.clone();
    for &vk in st.pressed_vks.iter().rev() {
        send_single_input(vk, false);
    }
    st.pressed_vks.clear();
    st.target_vks.clear();
    st.names.clear();
    st.active = false;
    st.expires_at = None;
    st.repeat_down = false;
    st.next_toggle_at = None;
    if !prev.is_empty() || auto_released {
        st.auto_released = auto_released;
        st.release_reason = reason.to_string();
    }
    prev
}

fn clip_str(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

pub struct DesktopSession {
    target: HWND,
    target_pid: u32,
    automation: IUIAutomation,
    walker: IUIAutomationTreeWalker,
    elements: Vec<IUIAutomationElement>,
    names: Vec<String>,
    focused_at_obs: Option<IUIAutomationElement>,
    observed: bool,
    sustain: Arc<(Mutex<SustainShared>, Condvar)>,
    sustain_thread: Option<JoinHandle<()>>,
}

impl DesktopSession {
    pub fn new(target_val: isize) -> Result<Self, String> {
        let target = HWND(target_val as *mut core::ffi::c_void);
        let mut target_pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(target, Some(&mut target_pid));
        }
        let automation: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
                .map_err(|_| "UI Automation initialization failed".to_string())?;
        let walker = unsafe { automation.ControlViewWalker() }
            .map_err(|_| "UI Automation tree walker failed".to_string())?;
        if let Ok(limits) = automation.cast::<IUIAutomation2>() {
            unsafe {
                let _ = limits.SetConnectionTimeout(1500);
                let _ = limits.SetTransactionTimeout(1500);
            }
        }
        let shared = Arc::new((
            Mutex::new(SustainShared {
                target_hwnd: target_val,
                target_pid,
                ..SustainShared::default()
            }),
            Condvar::new(),
        ));
        let s2 = Arc::clone(&shared);
        let handle = thread::spawn(move || {
            let (mu, cv) = &*s2;
            let mut guard = mu.lock();
            while !guard.stop_thread {
                cv.wait_for(&mut guard, Duration::from_millis(35));
                if guard.stop_thread {
                    break;
                }
                if !guard.active {
                    continue;
                }
                let now = Instant::now();
                if guard.expires_at.map(|e| now >= e).unwrap_or(true) {
                    release_sustain_locked(&mut guard, "ttl_expired", true);
                    continue;
                }
                let t_hwnd = HWND(guard.target_hwnd as *mut core::ffi::c_void);
                let t_pid = guard.target_pid;
                let fg = unsafe { GetForegroundWindow() };
                if !unsafe { IsWindow(Some(t_hwnd)) }.as_bool() || !belongs_to_target(fg, t_hwnd, t_pid)
                {
                    release_sustain_locked(&mut guard, "focus_lost", true);
                    continue;
                }
                if guard.mode == "repeat"
                    && !guard.target_vks.is_empty()
                    && guard.next_toggle_at.map(|t| now >= t).unwrap_or(false)
                {
                    if guard.repeat_down {
                        for &vk in guard.pressed_vks.clone().iter().rev() {
                            send_single_input(vk, false);
                        }
                        guard.pressed_vks.clear();
                        guard.repeat_down = false;
                        guard.next_toggle_at = Some(now + Duration::from_millis(135));
                    } else {
                        for vk in guard.target_vks.clone() {
                            if send_single_input(vk, true) {
                                guard.pressed_vks.push(vk);
                            }
                        }
                        guard.repeat_down = true;
                        guard.next_toggle_at = Some(now + Duration::from_millis(45));
                    }
                }
            }
        });

        Ok(Self {
            target,
            target_pid,
            automation,
            walker,
            elements: Vec::new(),
            names: Vec::new(),
            focused_at_obs: None,
            observed: false,
            sustain: shared,
            sustain_thread: Some(handle),
        })
    }

    fn clear_observation(&mut self) {
        self.elements.clear();
        self.names.clear();
        self.focused_at_obs = None;
    }

    fn window_center_visible(&self) -> bool {
        let mut rc = RECT::default();
        if unsafe { GetWindowRect(self.target, &mut rc) }.is_err() {
            return false;
        }
        let pt = POINT {
            x: (rc.left + rc.right) / 2,
            y: (rc.top + rc.bottom) / 2,
        };
        let hit = unsafe { WindowFromPoint(pt) };
        belongs_to_target(hit, self.target, self.target_pid)
    }

    fn activate(&self, require_focus: bool, read_only_visual: bool) -> Result<HWND, String> {
        if self.target.0.is_null() || !unsafe { IsWindow(Some(self.target)) }.as_bool() {
            return Err("選択したウィンドウは見つかりません".into());
        }
        if !read_only_visual {
            let uipi_blocked =
                !is_current_process_elevated() && is_window_process_elevated(self.target.0 as isize);
            if unsafe { IsIconic(self.target) }.as_bool() {
                unsafe {
                    let _ = ShowWindow(self.target, SW_SHOWNOACTIVATE);
                }
            }
            if !belongs_to_target(unsafe { GetForegroundWindow() }, self.target, self.target_pid)
                && !uipi_blocked
            {
                unsafe {
                    let _ = SetWindowPos(
                        self.target,
                        Some(HWND_TOPMOST),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                    let _ = SetWindowPos(
                        self.target,
                        Some(HWND_NOTOPMOST),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                    let _ = BringWindowToTop(self.target);
                    let _ = SetForegroundWindow(self.target);
                }
                thread::sleep(Duration::from_millis(45));
            }
            if belongs_to_target(unsafe { GetForegroundWindow() }, self.target, self.target_pid) {
                thread::sleep(Duration::from_millis(45));
            }
            if !belongs_to_target(unsafe { GetForegroundWindow() }, self.target, self.target_pid)
                && (require_focus || !self.window_center_visible())
            {
                return Err("対象ウィンドウを前面にしてください".into());
            }
        }
        Ok(self.target)
    }

    fn walk(
        &mut self,
        el: &IUIAutomationElement,
        out: &mut Vec<Value>,
        depth: usize,
    ) {
        if depth > 8 || self.elements.len() >= 60 {
            return;
        }
        let off = unsafe { el.CurrentIsOffscreen() }.map(|b| b.as_bool()).unwrap_or(false);
        let pass = unsafe { el.CurrentIsPassword() }.map(|b| b.as_bool()).unwrap_or(true);
        let enabled = unsafe { el.CurrentIsEnabled() }.map(|b| b.as_bool()).unwrap_or(false);
        if off || pass {
            return;
        }
        let full_name = unsafe { el.CurrentName() }
            .map(|b| b.to_string())
            .unwrap_or_default();
        let role = unsafe { el.CurrentLocalizedControlType() }
            .map(|b| b.to_string())
            .unwrap_or_default();
        let rect = unsafe { el.CurrentBoundingRectangle() }.unwrap_or_default();
        let index = self.elements.len();
        self.elements.push(el.clone());
        self.names.push(full_name.clone());

        let mut row = json!({
            "index": index,
            "name": clip_str(&full_name, 240),
            "role": role,
            "enabled": enabled,
            "bounds": [rect.left, rect.top, rect.right, rect.bottom],
        });
        if let Ok(vp) = unsafe { el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) } {
            if let Ok(val) = unsafe { vp.CurrentValue() } {
                row["value"] = json!(clip_str(&val.to_string(), 360));
            }
        }
        out.push(row);

        let mut child_opt = unsafe { self.walker.GetFirstChildElement(el) }.ok();
        while let Some(child) = child_opt {
            if self.elements.len() >= 60 {
                break;
            }
            self.walk(&child, out, depth + 1);
            child_opt = unsafe { self.walker.GetNextSiblingElement(&child) }.ok();
        }
    }

    fn stop_sustain(&self, reason: &str) -> Vec<String> {
        let mut guard = self.sustain.0.lock();
        release_sustain_locked(&mut guard, reason, false)
    }

    fn set_sustain(&self, key_names: &[String], mode: &str, ms: i32) -> Result<(), String> {
        let (clamped_ms, target_vks) = validate_sustain_params(key_names, mode, ms)?;
        if key_names.is_empty() {
            self.stop_sustain("explicit");
            return Ok(());
        }
        let mut guard = self.sustain.0.lock();
        if guard.stop_thread {
            return Ok(());
        }
        let now = Instant::now();
        if mode == "hold" {
            let (to_release, to_press) = compute_key_diff(&guard.pressed_vks, &target_vks);
            for vk in to_release {
                send_single_input(vk, false);
                guard.pressed_vks.retain(|&x| x != vk);
            }
            for vk in to_press {
                if !send_single_input(vk, true) {
                    release_sustain_locked(&mut guard, "error", false);
                    return Err("キーを送信できません".into());
                }
                guard.pressed_vks.push(vk);
            }
            guard.pressed_vks = target_vks.clone();
            guard.mode = mode.to_string();
            guard.names = key_names.to_vec();
            guard.target_vks = target_vks;
            guard.active = true;
            guard.expires_at = Some(now + Duration::from_millis(clamped_ms as u64));
            guard.repeat_down = false;
            guard.next_toggle_at = None;
            guard.auto_released = false;
            guard.release_reason.clear();
        } else {
            for vk in guard.pressed_vks.clone().into_iter().rev() {
                send_single_input(vk, false);
            }
            guard.pressed_vks.clear();
            for &vk in &target_vks {
                if !send_single_input(vk, true) {
                    release_sustain_locked(&mut guard, "error", false);
                    return Err("キーを送信できません".into());
                }
                guard.pressed_vks.push(vk);
            }
            guard.mode = mode.to_string();
            guard.names = key_names.to_vec();
            guard.target_vks = target_vks;
            guard.active = true;
            guard.expires_at = Some(now + Duration::from_millis(clamped_ms as u64));
            guard.repeat_down = true;
            guard.next_toggle_at = Some(now + Duration::from_millis(45));
            guard.auto_released = false;
            guard.release_reason.clear();
        }
        Ok(())
    }

    fn sustain_status_json(&self) -> Value {
        let guard = self.sustain.0.lock();
        let remaining_ms = if guard.active {
            guard
                .expires_at
                .and_then(|e| e.checked_duration_since(Instant::now()))
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        } else {
            0
        };
        json!({
            "active": guard.active,
            "mode": guard.mode,
            "keys": guard.names,
            "remainingMs": remaining_ms,
            "autoReleased": guard.auto_released,
            "releaseReason": guard.release_reason,
        })
    }

    fn norm_to_abs_virtual_screen(nx: f64, ny: f64, window: HWND) -> Result<(i32, i32, i32, i32), String> {
        let mut rc = RECT::default();
        unsafe { GetWindowRect(window, &mut rc) }
            .map_err(|_| "ウィンドウ領域を取得できません".to_string())?;
        let w = (rc.right - rc.left).max(1);
        let h = (rc.bottom - rc.top).max(1);
        let px = rc.left + ((nx.clamp(0.0, 1.0) * (w as f64 - 1.0)).round() as i32);
        let py = rc.top + ((ny.clamp(0.0, 1.0) * (h as f64 - 1.0)).round() as i32);
        let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
        let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(2);
        let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(2);
        let ax = (((px - vx) as i64) * 65535 / ((vw - 1) as i64)) as i32;
        let ay = (((py - vy) as i64) * 65535 / ((vh - 1) as i64)) as i32;
        Ok((ax, ay, px, py))
    }

    fn click_at_point(&self, window: HWND, nx: f64, ny: f64, button: &str) -> Result<(), String> {
        if !(0.0..=1.0).contains(&nx) || !(0.0..=1.0).contains(&ny) || !nx.is_finite() || !ny.is_finite() {
            return Err("click_at coordinates must be in 0.0 to 1.0".into());
        }
        if !matches!(button, "left" | "right" | "double") {
            return Err("Unknown click_at button".into());
        }
        let (ax, ay, _, _) = Self::norm_to_abs_virtual_screen(nx, ny, window)?;
        let mut mv = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0::default(),
        };
        mv.Anonymous.mi.dx = ax;
        mv.Anonymous.mi.dy = ay;
        mv.Anonymous.mi.dwFlags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
        if !send_inputs(&[mv]) {
            return Err("クリックを送信できません".into());
        }
        let (down_flag, up_flag) = if button == "right" {
            (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
        } else {
            (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
        };
        let clicks = if button == "double" { 2 } else { 1 };
        for c in 0..clicks {
            let mut d = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0::default() };
            let mut u = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0::default() };
            d.Anonymous.mi.dwFlags = down_flag;
            u.Anonymous.mi.dwFlags = up_flag;
            if !send_inputs(&[d, u]) {
                return Err("クリックを送信できません".into());
            }
            if c + 1 < clicks {
                thread::sleep(Duration::from_millis(40));
            }
        }
        Ok(())
    }

    fn drag_window(
        &self,
        window: HWND,
        from_x: f64,
        from_y: f64,
        to_x: f64,
        to_y: f64,
        button: &str,
        duration_ms: i32,
        mut frames: Option<&mut Vec<Value>>,
        t0: Instant,
        cap_w: &mut i32,
        cap_h: &mut i32,
    ) -> Result<(), String> {
        validate_drag_params(from_x, from_y, to_x, to_y, button)?;
        let dur = duration_ms.clamp(60, 2000);
        let (_, _, p0x, p0y) = Self::norm_to_abs_virtual_screen(from_x, from_y, window)?;
        let (_, _, p1x, p1y) = Self::norm_to_abs_virtual_screen(to_x, to_y, window)?;
        if button == "none" {
            let steps = (dur / 25).clamp(4, 30);
            let mut sent_x = 0i32;
            let mut sent_y = 0i32;
            let dx_tot = p1x - p0x;
            let dy_tot = p1y - p0y;
            for step in 1..=steps {
                let nx = (dx_tot as f64 * (step as f64 / steps as f64)).round() as i32;
                let ny = (dy_tot as f64 * (step as f64 / steps as f64)).round() as i32;
                let mut mv = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0::default() };
                mv.Anonymous.mi.dx = nx - sent_x;
                mv.Anonymous.mi.dy = ny - sent_y;
                mv.Anonymous.mi.dwFlags = MOUSEEVENTF_MOVE;
                if !send_inputs(&[mv]) {
                    return Err("マウス移動を送信できません".into());
                }
                sent_x = nx;
                sent_y = ny;
                if let Some(ref mut f) = frames {
                    if step == steps / 2 && f.len() < 7 {
                        let _ = self.append_frame(window, f, t0, cap_w, cap_h, 7, None, None, None);
                    }
                }
                thread::sleep(Duration::from_millis((dur / steps).max(8) as u64));
            }
            return Ok(());
        }
        let (ax0, ay0, _, _) = Self::norm_to_abs_virtual_screen(from_x, from_y, window)?;
        let mut smv = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0::default() };
        smv.Anonymous.mi.dx = ax0;
        smv.Anonymous.mi.dy = ay0;
        smv.Anonymous.mi.dwFlags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
        if !send_inputs(&[smv]) {
            return Err("ドラッグを開始できません".into());
        }
        let (down_flag, up_flag) = if button == "right" {
            (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
        } else {
            (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
        };
        let mut d = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0::default() };
        d.Anonymous.mi.dwFlags = down_flag;
        if !send_inputs(&[d]) {
            return Err("ドラッグを開始できません".into());
        }
        let steps = (dur / 25).clamp(4, 24);
        for step in 1..=steps {
            let t = step as f64 / steps as f64;
            let cx = from_x + (to_x - from_x) * t;
            let cy = from_y + (to_y - from_y) * t;
            let (ax, ay, _, _) = Self::norm_to_abs_virtual_screen(cx, cy, window)?;
            let mut mv = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0::default() };
            mv.Anonymous.mi.dx = ax;
            mv.Anonymous.mi.dy = ay;
            mv.Anonymous.mi.dwFlags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
            if !send_inputs(&[mv]) {
                let mut u = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0::default() };
                u.Anonymous.mi.dwFlags = up_flag;
                let _ = send_inputs(&[u]);
                return Err("ドラッグ中に失敗しました".into());
            }
            thread::sleep(Duration::from_millis((dur / steps).max(8) as u64));
        }
        let mut u = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0::default() };
        u.Anonymous.mi.dwFlags = up_flag;
        let _ = send_inputs(&[u]);
        Ok(())
    }

    fn press_key(
        &self,
        key_name: &str,
        hold_ms: i32,
        capture_win: Option<HWND>,
        mut frames: Option<&mut Vec<Value>>,
        t0: Instant,
        cap_w: &mut i32,
        cap_h: &mut i32,
    ) -> Result<(), String> {
        let combo = parse_keys(key_name)?;
        let downs: Vec<INPUT> = combo.iter().map(|&vk| make_single_input(vk, true)).collect();
        let ups: Vec<INPUT> = combo.iter().rev().map(|&vk| make_single_input(vk, false)).collect();
        if !send_inputs(&downs) {
            return Err("キー入力を送信できません".into());
        }
        let hold = hold_ms.clamp(20, 3000);
        if capture_win.is_some() && hold >= 250 {
            let half = hold / 2;
            thread::sleep(Duration::from_millis(half as u64));
            if let (Some(w), Some(ref mut f)) = (capture_win, &mut frames) {
                let _ = self.append_frame(w, f, t0, cap_w, cap_h, 7, None, None, None);
            }
            thread::sleep(Duration::from_millis((hold - half).max(10) as u64));
        } else {
            thread::sleep(Duration::from_millis(hold as u64));
        }
        let _ = send_inputs(&ups);
        Ok(())
    }

    fn scroll_window(&self, window: HWND, amount: i32) -> Result<(), String> {
        if !(-5..=5).contains(&amount) {
            return Err("Scroll amount must be -5 to 5".into());
        }
        let (ax, ay, px, py) = Self::norm_to_abs_virtual_screen(0.5, 0.5, window)?;
        let hit = unsafe { WindowFromPoint(POINT { x: px, y: py }) };
        if !belongs_to_target(hit, self.target, self.target_pid) {
            return Err("対象ウィンドウが隠れています".into());
        }
        let mut mv = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0::default() };
        let mut wh = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0::default() };
        mv.Anonymous.mi.dx = ax;
        mv.Anonymous.mi.dy = ay;
        mv.Anonymous.mi.dwFlags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
        wh.Anonymous.mi.dwFlags = MOUSEEVENTF_WHEEL;
        wh.Anonymous.mi.mouseData = (amount * WHEEL_DELTA as i32) as u32;
        if !send_inputs(&[mv, wh]) {
            return Err("スクロールできません".into());
        }
        Ok(())
    }

    fn append_frame(
        &self,
        window: HWND,
        frames: &mut Vec<Value>,
        t0: Instant,
        out_w: &mut i32,
        out_h: &mut i32,
        cap: usize,
        out_full: Option<&mut Vec<u8>>,
        first_luma: Option<&mut Vec<u8>>,
        last_luma: Option<&mut Vec<u8>>,
    ) -> bool {
        if frames.len() >= cap {
            return false;
        }
        let Ok((bmp, w, h)) = grab_window_bitmap(window) else {
            return false;
        };
        let Some(jpg) = encode_hbitmap_jpeg(bmp.0, 70_000, [640, 480], [0.65, 0.50]) else {
            return false;
        };
        if let Some(full_dst) = out_full {
            if let Some(fb) = encode_hbitmap_jpeg(bmp.0, 220_000, [1920, 1440], [0.82, 0.68]) {
                *full_dst = fb;
            }
        }
        if first_luma.is_some() || last_luma.is_some() {
            let luma = extract_luma_32x18(bmp.0);
            if !luma.is_empty() {
                if let Some(fl) = first_luma {
                    if fl.is_empty() {
                        *fl = luma.clone();
                    }
                }
                if let Some(ll) = last_luma {
                    *ll = luma;
                }
            }
        }
        *out_w = w;
        *out_h = h;
        frames.push(json!({
            "tMs": t0.elapsed().as_millis() as i64,
            "mime": "image/jpeg",
            "base64": B64.encode(&jpg),
            "bytes": jpg.len(),
        }));
        true
    }

    fn capture_video(&self, window: HWND, duration_ms: i32, fps: i32) -> Result<Value, String> {
        let (clamped_ms, clamped_fps) = validate_video_params(duration_ms, fps);
        let count = ((clamped_ms * clamped_fps) / 1000 + 1).clamp(2, 10) as usize;
        let interval = (clamped_ms / ((count as i32) - 1)).max(50) as u64;
        let t0 = Instant::now();
        let mut frames = Vec::with_capacity(count);
        let (mut w, mut h) = (0, 0);
        for i in 0..count {
            self.append_frame(window, &mut frames, t0, &mut w, &mut h, count, None, None, None);
            if i + 1 < count {
                thread::sleep(Duration::from_millis(interval));
            }
        }
        if frames.is_empty() {
            return Err("動画フレームの取得に失敗しました".into());
        }
        Ok(json!({
            "ok": true,
            "frames": frames,
            "width": w,
            "height": h,
            "fps": clamped_fps,
            "durationMs": clamped_ms,
            "mode": "fast",
        }))
    }

    fn capture_screenshot(&self, window: HWND) -> Result<Value, String> {
        let (bmp, w, h) = grab_window_bitmap(window)?;
        let (vlm_bytes, ext) = if let Some(jpg) =
            encode_hbitmap_jpeg(bmp.0, 90_000, [960, 768], [0.75, 0.60])
        {
            (jpg, ".jpg")
        } else {
            encode_hbitmap_to_budget(bmp.0, 200_000)
                .ok_or_else(|| "スクリーンショット画像の圧縮に失敗しました".to_string())?
        };
        let full_bytes = encode_hbitmap_jpeg(bmp.0, 260_000, [1920, 1440], [0.84, 0.70]);
        let mime = if ext == ".jpg" || ext == ".jpeg" {
            "image/jpeg"
        } else {
            "image/png"
        };
        let mut res = json!({
            "ok": true,
            "mime": mime,
            "base64": B64.encode(&vlm_bytes),
            "width": w,
            "height": h,
            "bytes": vlm_bytes.len(),
            "mode": "fast",
        });
        if let Some(fb) = full_bytes {
            res["fullMime"] = json!("image/jpeg");
            res["fullBase64"] = json!(B64.encode(&fb));
            res["fullBytes"] = json!(fb.len());
        }
        Ok(res)
    }

    pub fn perform(&mut self, request: &Value) -> Value {
        let tool = request
            .get("tool")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let args = request.get("args").cloned().unwrap_or_else(|| json!({}));
        let read_only_visual = tool == "desktop.screenshot" || tool == "desktop.video";
        if !read_only_visual
            && !is_current_process_elevated()
            && is_window_process_elevated(self.target.0 as isize)
        {
            return json!({
                "ok": false,
                "executed": false,
                "code": "UIPI_ELEVATED_TARGET",
                "uipiBlocked": true,
                "error": "対象ウィンドウが管理者権限で動作中のため、非管理者 Qwen27.exe からは直接操作できません（Windows UIPI制限）。通常権限で対象を起動するか、画面/動画観察（Desktop.Screenshot / Desktop.Video）＋ User.Ask 手順案内、または管理者起動をご利用ください。",
                "sustain": self.sustain_status_json(),
            });
        }
        match self.perform_impl(tool, &args, read_only_visual) {
            Ok(mut res) => {
                res["sustain"] = self.sustain_status_json();
                res
            }
            Err(err) => json!({ "error": err }),
        }
    }

    fn perform_impl(
        &mut self,
        tool: &str,
        args: &Value,
        read_only_visual: bool,
    ) -> Result<Value, String> {
        if tool == "desktop.release" {
            self.observed = false;
            let prev = self.stop_sustain("explicit");
            let win = self.activate(false, false)?;
            thread::sleep(Duration::from_millis(70));
            let mut res = self.capture_screenshot(win)?;
            res["released"] = json!(prev);
            return Ok(res);
        }
        let sustain_has_keys = tool == "desktop.sustain"
            && args
                .get("keys")
                .and_then(|v| v.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false);
        let need_focus = tool == "desktop.key"
            || tool == "desktop.hold_key"
            || tool == "desktop.type"
            || tool == "desktop.observe"
            || tool == "desktop.click"
            || sustain_has_keys
            || (tool == "desktop.drag"
                && args.get("button").and_then(|v| v.as_str()).unwrap_or("left") == "none");
        let window = self.activate(need_focus, read_only_visual)?;

        if tool == "desktop.observe" {
            self.stop_sustain("observe");
            self.clear_observation();
            let root = unsafe { self.automation.ElementFromHandle(window) }
                .map_err(|_| "対象の操作要素を取得できません".to_string())?;
            let mut elements = Vec::new();
            self.walk(&root, &mut elements, 0);
            self.focused_at_obs = unsafe { self.automation.GetFocusedElement() }.ok();
            self.observed = true;
            let mut buf = [0u16; 512];
            let len = unsafe { GetWindowTextW(window, &mut buf) }.max(0) as usize;
            return Ok(json!({
                "window": from_wide_nul(&buf[..len]),
                "elements": elements,
                "instruction": "UI要素クリック・入力を使う場合は再度observeしてください。定型キーやclick_at/sequenceはそのまま連続実行できます。",
            }));
        }
        if tool == "desktop.screenshot" {
            self.observed = false;
            return self.capture_screenshot(window);
        }
        if tool == "desktop.video" {
            self.observed = false;
            let ms = args.get("ms").and_then(|v| v.as_i64()).unwrap_or(1500) as i32;
            let fps = args.get("fps").and_then(|v| v.as_i64()).unwrap_or(4) as i32;
            return self.capture_video(window, ms, fps);
        }

        let capture_frames = args.get("captureFrames").and_then(|v| v.as_bool()).unwrap_or(false);
        let mut frames = Vec::new();
        let mut full_bytes = Vec::new();
        let mut first_luma = Vec::new();
        let mut last_luma = Vec::new();
        let (mut cap_w, mut cap_h) = (0i32, 0i32);
        let t0 = Instant::now();

        let with_frames = |mut res: Value,
                           frames: Vec<Value>,
                           full_bytes: Vec<u8>,
                           first_luma: &[u8],
                           last_luma: &[u8],
                           cap_w: i32,
                           cap_h: i32| {
            if capture_frames && !frames.is_empty() {
                let len = frames.len();
                res["frames"] = Value::Array(frames);
                res["fps"] = json!(4);
                if cap_w > 0 {
                    res["width"] = json!(cap_w);
                }
                if cap_h > 0 {
                    res["height"] = json!(cap_h);
                }
                if !full_bytes.is_empty() {
                    res["fullMime"] = json!("image/jpeg");
                    res["fullBase64"] = json!(B64.encode(&full_bytes));
                    res["fullBytes"] = json!(full_bytes.len());
                }
                if len >= 2 && !first_luma.is_empty() && first_luma.len() == last_luma.len() {
                    let score = compute_luma_motion_score(first_luma, last_luma);
                    res["motionScore"] = json!((score * 10000.0).round() / 10000.0);
                    res["screenChanged"] = json!(score >= 0.015);
                }
            }
            res
        };

        if tool == "desktop.sustain" {
            self.observed = false;
            let mut keys_vec = Vec::new();
            if let Some(arr) = args.get("keys").and_then(|v| v.as_array()) {
                for item in arr {
                    let s = item
                        .as_str()
                        .ok_or_else(|| "Each key must be a string".to_string())?;
                    keys_vec.push(s.to_string());
                }
            } else if args.get("keys").map(|v| !v.is_null()).unwrap_or(false) {
                return Err("keys must be an array of strings".into());
            }
            let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("hold");
            let ms = args.get("ms").and_then(|v| v.as_i64()).unwrap_or(15000) as i32;
            let _ = validate_sustain_params(&keys_vec, mode, ms)?;
            if keys_vec.is_empty() {
                let prev = self.stop_sustain("explicit");
                thread::sleep(Duration::from_millis(70));
                let mut res = if capture_frames {
                    self.append_frame(
                        window,
                        &mut frames,
                        t0,
                        &mut cap_w,
                        &mut cap_h,
                        8,
                        Some(&mut full_bytes),
                        None,
                        None,
                    );
                    with_frames(
                        json!({ "ok": true, "mode": "fast" }),
                        frames,
                        full_bytes,
                        &first_luma,
                        &last_luma,
                        cap_w,
                        cap_h,
                    )
                } else {
                    self.capture_screenshot(window)?
                };
                res["released"] = json!(prev);
                return Ok(res);
            }
            if capture_frames {
                self.append_frame(
                    window,
                    &mut frames,
                    t0,
                    &mut cap_w,
                    &mut cap_h,
                    8,
                    None,
                    Some(&mut first_luma),
                    Some(&mut last_luma),
                );
            }
            self.set_sustain(&keys_vec, mode, ms)?;
            thread::sleep(Duration::from_millis(180));
            if capture_frames {
                self.append_frame(
                    window,
                    &mut frames,
                    t0,
                    &mut cap_w,
                    &mut cap_h,
                    8,
                    None,
                    Some(&mut first_luma),
                    Some(&mut last_luma),
                );
            }
            return Ok(with_frames(
                json!({ "ok": true, "mode": "fast" }),
                frames,
                full_bytes,
                &first_luma,
                &last_luma,
                cap_w,
                cap_h,
            ));
        }

        if tool == "desktop.click_at" {
            self.observed = false;
            if capture_frames {
                self.append_frame(
                    window,
                    &mut frames,
                    t0,
                    &mut cap_w,
                    &mut cap_h,
                    8,
                    None,
                    Some(&mut first_luma),
                    Some(&mut last_luma),
                );
            }
            let x = args.get("x").and_then(|v| v.as_f64()).ok_or_else(|| "x が必要です".to_string())?;
            let y = args.get("y").and_then(|v| v.as_f64()).ok_or_else(|| "y が必要です".to_string())?;
            let button = args.get("button").and_then(|v| v.as_str()).unwrap_or("left");
            self.click_at_point(window, x, y, button)?;
            if capture_frames {
                thread::sleep(Duration::from_millis(160));
                self.append_frame(
                    window,
                    &mut frames,
                    t0,
                    &mut cap_w,
                    &mut cap_h,
                    8,
                    None,
                    Some(&mut first_luma),
                    Some(&mut last_luma),
                );
            }
            return Ok(with_frames(
                json!({ "ok": true, "mode": "fast" }),
                frames,
                full_bytes,
                &first_luma,
                &last_luma,
                cap_w,
                cap_h,
            ));
        }

        if tool == "desktop.key" {
            self.observed = false;
            let key_name = args
                .get("key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "key が必要です".to_string())?;
            if is_ui_key_for_sustain(key_name).unwrap_or(false) {
                self.stop_sustain("ui_key");
            }
            if capture_frames {
                self.append_frame(
                    window,
                    &mut frames,
                    t0,
                    &mut cap_w,
                    &mut cap_h,
                    8,
                    None,
                    Some(&mut first_luma),
                    Some(&mut last_luma),
                );
            }
            self.press_key(key_name, 35, None, None, t0, &mut cap_w, &mut cap_h)?;
            if capture_frames {
                thread::sleep(Duration::from_millis(160));
                self.append_frame(
                    window,
                    &mut frames,
                    t0,
                    &mut cap_w,
                    &mut cap_h,
                    8,
                    None,
                    Some(&mut first_luma),
                    Some(&mut last_luma),
                );
            }
            return Ok(with_frames(
                json!({ "ok": true, "mode": "fast" }),
                frames,
                full_bytes,
                &first_luma,
                &last_luma,
                cap_w,
                cap_h,
            ));
        }

        if tool == "desktop.hold_key" {
            self.observed = false;
            self.stop_sustain("hold_key");
            let key_name = args
                .get("key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "key が必要です".to_string())?;
            let ms = args.get("ms").and_then(|v| v.as_i64()).unwrap_or(500) as i32;
            if capture_frames {
                self.append_frame(
                    window,
                    &mut frames,
                    t0,
                    &mut cap_w,
                    &mut cap_h,
                    8,
                    None,
                    Some(&mut first_luma),
                    Some(&mut last_luma),
                );
            }
            self.press_key(
                key_name,
                ms,
                if capture_frames { Some(window) } else { None },
                if capture_frames { Some(&mut frames) } else { None },
                t0,
                &mut cap_w,
                &mut cap_h,
            )?;
            if capture_frames {
                thread::sleep(Duration::from_millis(140));
                self.append_frame(
                    window,
                    &mut frames,
                    t0,
                    &mut cap_w,
                    &mut cap_h,
                    8,
                    None,
                    Some(&mut first_luma),
                    Some(&mut last_luma),
                );
            }
            return Ok(with_frames(
                json!({ "ok": true, "mode": "fast" }),
                frames,
                full_bytes,
                &first_luma,
                &last_luma,
                cap_w,
                cap_h,
            ));
        }

        if tool == "desktop.drag" {
            self.observed = false;
            let fx = args.get("fromX").and_then(|v| v.as_f64()).ok_or("fromX が必要です")?;
            let fy = args.get("fromY").and_then(|v| v.as_f64()).ok_or("fromY が必要です")?;
            let tx = args.get("toX").and_then(|v| v.as_f64()).ok_or("toX が必要です")?;
            let ty = args.get("toY").and_then(|v| v.as_f64()).ok_or("toY が必要です")?;
            let btn = args.get("button").and_then(|v| v.as_str()).unwrap_or("left");
            let dur = args.get("durationMs").and_then(|v| v.as_i64()).unwrap_or(300) as i32;
            if capture_frames {
                self.append_frame(
                    window,
                    &mut frames,
                    t0,
                    &mut cap_w,
                    &mut cap_h,
                    8,
                    None,
                    Some(&mut first_luma),
                    Some(&mut last_luma),
                );
            }
            self.drag_window(
                window,
                fx,
                fy,
                tx,
                ty,
                btn,
                dur,
                if capture_frames { Some(&mut frames) } else { None },
                t0,
                &mut cap_w,
                &mut cap_h,
            )?;
            if capture_frames {
                thread::sleep(Duration::from_millis(140));
                self.append_frame(
                    window,
                    &mut frames,
                    t0,
                    &mut cap_w,
                    &mut cap_h,
                    8,
                    None,
                    Some(&mut first_luma),
                    Some(&mut last_luma),
                );
            }
            return Ok(with_frames(
                json!({ "ok": true, "mode": "fast" }),
                frames,
                full_bytes,
                &first_luma,
                &last_luma,
                cap_w,
                cap_h,
            ));
        }

        if tool == "desktop.scroll" {
            self.observed = false;
            if capture_frames {
                self.append_frame(
                    window,
                    &mut frames,
                    t0,
                    &mut cap_w,
                    &mut cap_h,
                    8,
                    None,
                    Some(&mut first_luma),
                    Some(&mut last_luma),
                );
            }
            let amount = args.get("amount").and_then(|v| v.as_i64()).ok_or("amount が必要です")? as i32;
            self.scroll_window(window, amount)?;
            if capture_frames {
                thread::sleep(Duration::from_millis(160));
                self.append_frame(
                    window,
                    &mut frames,
                    t0,
                    &mut cap_w,
                    &mut cap_h,
                    8,
                    None,
                    Some(&mut first_luma),
                    Some(&mut last_luma),
                );
            }
            return Ok(with_frames(
                json!({ "ok": true, "mode": "fast" }),
                frames,
                full_bytes,
                &first_luma,
                &last_luma,
                cap_w,
                cap_h,
            ));
        }

        if tool == "desktop.sequence" {
            self.observed = false;
            self.stop_sustain("sequence");
            let actions = args
                .get("actions")
                .and_then(|v| v.as_array())
                .ok_or_else(|| "actions must contain 1 to 8 steps".to_string())?;
            if actions.is_empty() || actions.len() > 8 {
                return Err("actions must contain 1 to 8 steps".into());
            }
            if capture_frames {
                self.append_frame(
                    window,
                    &mut frames,
                    t0,
                    &mut cap_w,
                    &mut cap_h,
                    8,
                    None,
                    Some(&mut first_luma),
                    Some(&mut last_luma),
                );
            }
            let mut executed = 0usize;
            for item in actions {
                let kind = item
                    .get("action")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "Each sequence item requires string 'action'".to_string())?;
                let step_focus = kind == "key"
                    || kind == "hold_key"
                    || (kind == "drag"
                        && item.get("button").and_then(|v| v.as_str()).unwrap_or("left") == "none");
                let active = self.activate(step_focus, false)?;
                match kind {
                    "key" | "hold_key" => {
                        let k = item
                            .get("key")
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                            .ok_or_else(|| format!("{} action requires 'key'", kind))?;
                        let ms = item
                            .get("ms")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(if kind == "hold_key" { 500 } else { 35 }) as i32;
                        self.press_key(
                            k,
                            ms,
                            if capture_frames { Some(active) } else { None },
                            if capture_frames { Some(&mut frames) } else { None },
                            t0,
                            &mut cap_w,
                            &mut cap_h,
                        )?;
                    }
                    "click_at" => {
                        let x = item.get("x").and_then(|v| v.as_f64()).ok_or("click_at action requires normalized 'x' and 'y' (0.0-1.0)")?;
                        let y = item.get("y").and_then(|v| v.as_f64()).ok_or("click_at action requires normalized 'x' and 'y' (0.0-1.0)")?;
                        let b = item.get("button").and_then(|v| v.as_str()).unwrap_or("left");
                        self.click_at_point(active, x, y, b)?;
                    }
                    "drag" => {
                        let fx = item.get("fromX").and_then(|v| v.as_f64()).ok_or("drag action requires 'fromX','fromY','toX','toY' (0.0-1.0)")?;
                        let fy = item.get("fromY").and_then(|v| v.as_f64()).ok_or("drag action requires 'fromX','fromY','toX','toY' (0.0-1.0)")?;
                        let tx = item.get("toX").and_then(|v| v.as_f64()).ok_or("drag action requires 'fromX','fromY','toX','toY' (0.0-1.0)")?;
                        let ty = item.get("toY").and_then(|v| v.as_f64()).ok_or("drag action requires 'fromX','fromY','toX','toY' (0.0-1.0)")?;
                        let b = item.get("button").and_then(|v| v.as_str()).unwrap_or("left");
                        let ms = item.get("ms").and_then(|v| v.as_i64()).unwrap_or(300) as i32;
                        self.drag_window(
                            active,
                            fx,
                            fy,
                            tx,
                            ty,
                            b,
                            ms,
                            if capture_frames { Some(&mut frames) } else { None },
                            t0,
                            &mut cap_w,
                            &mut cap_h,
                        )?;
                    }
                    "scroll" => {
                        let amt = item.get("amount").and_then(|v| v.as_i64()).ok_or("scroll action requires integer 'amount' (-5 to 5)")? as i32;
                        self.scroll_window(active, amt)?;
                    }
                    "wait" => {
                        let ms = item.get("ms").and_then(|v| v.as_i64()).unwrap_or(50).clamp(0, 5000) as u64;
                        thread::sleep(Duration::from_millis(ms));
                    }
                    _ => return Err("Unknown sequence action".into()),
                }
                executed += 1;
                if capture_frames && executed < actions.len() && frames.len() < 6 {
                    self.append_frame(active, &mut frames, t0, &mut cap_w, &mut cap_h, 7, None, None, None);
                }
            }
            if capture_frames {
                thread::sleep(Duration::from_millis(140));
                self.append_frame(
                    window,
                    &mut frames,
                    t0,
                    &mut cap_w,
                    &mut cap_h,
                    8,
                    None,
                    Some(&mut first_luma),
                    Some(&mut last_luma),
                );
            }
            return Ok(with_frames(
                json!({ "ok": true, "executed": executed, "mode": "fast" }),
                frames,
                full_bytes,
                &first_luma,
                &last_luma,
                cap_w,
                cap_h,
            ));
        }

        if !self.observed {
            return Err("先にdesktop.observeで現在の状態を確認してください".into());
        }
        self.observed = false;
        if tool == "desktop.click" {
            self.stop_sustain("click");
            let idx = args
                .get("element")
                .and_then(|v| v.as_i64())
                .ok_or("element が必要です")?;
            if idx < 0 || (idx as usize) >= self.elements.len() {
                return Err("Unknown element index".into());
            }
            let el = self.elements[idx as usize].clone();
            let off = unsafe { el.CurrentIsOffscreen() }.map(|b| b.as_bool()).unwrap_or(true);
            let enabled = unsafe { el.CurrentIsEnabled() }.map(|b| b.as_bool()).unwrap_or(false);
            let pass = unsafe { el.CurrentIsPassword() }.map(|b| b.as_bool()).unwrap_or(true);
            let cur_name = unsafe { el.CurrentName() }
                .map_err(|_| "要素が変更されています".to_string())?
                .to_string();
            if cur_name != self.names[idx as usize] {
                return Err("表示内容が変わりました。observeし直してください".into());
            }
            if off || !enabled || pass {
                return Err("この要素はクリックできません".into());
            }
            let bounds = unsafe { el.CurrentBoundingRectangle() }
                .map_err(|_| "要素の位置が変わりました".to_string())?;
            let pt = POINT {
                x: (bounds.left + bounds.right) / 2,
                y: (bounds.top + bounds.bottom) / 2,
            };
            let hit = unsafe { WindowFromPoint(pt) };
            if hit.0.is_null() || unsafe { GetAncestor(hit, GA_ROOTOWNER) != GetAncestor(self.target, GA_ROOTOWNER) } {
                return Err("対象以外のウィンドウが重なっています".into());
            }
            let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
            let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
            let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(2);
            let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(2);
            let ax = (((pt.x - vx) as i64) * 65535 / ((vw - 1) as i64)) as i32;
            let ay = (((pt.y - vy) as i64) * 65535 / ((vh - 1) as i64)) as i32;
            let mut m = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0::default() };
            let mut d = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0::default() };
            let mut u = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0::default() };
            m.Anonymous.mi.dx = ax;
            m.Anonymous.mi.dy = ay;
            m.Anonymous.mi.dwFlags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
            d.Anonymous.mi.dwFlags = MOUSEEVENTF_LEFTDOWN;
            u.Anonymous.mi.dwFlags = MOUSEEVENTF_LEFTUP;
            if !send_inputs(&[m, d, u]) {
                return Err("クリックを送信できません".into());
            }
            return Ok(json!({ "ok": true, "next": "desktop.observe" }));
        }
        if tool == "desktop.type" {
            self.stop_sustain("type");
            let text = args
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or("text が必要です")?;
            let units: Vec<u16> = text.encode_utf16().collect();
            if units.len() > 2000 {
                return Err("Input is limited to 2000 characters".into());
            }
            let focused = unsafe { self.automation.GetFocusedElement() }
                .map_err(|_| "入力欄を選択してください".to_string())?;
            let pass = unsafe { focused.CurrentIsPassword() }.map(|b| b.as_bool()).unwrap_or(true);
            let mut same = false;
            if let Some(ref obs_f) = self.focused_at_obs {
                same = unsafe { self.automation.CompareElements(&focused, obs_f) }
                    .map(|b| b.as_bool())
                    .unwrap_or(false);
            }
            if !same {
                return Err("入力欄が変わりました。observeし直してください".into());
            }
            if pass {
                return Err("保護された入力欄は操作できません".into());
            }
            let mut inputs = Vec::with_capacity(units.len() * 2);
            for u in units {
                let mut kd = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT::default() } };
                let mut ku = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT::default() } };
                kd.Anonymous.ki.wScan = u;
                kd.Anonymous.ki.dwFlags = KEYEVENTF_UNICODE;
                ku.Anonymous.ki.wScan = u;
                ku.Anonymous.ki.dwFlags = KEYEVENTF_UNICODE | KEYEVENTF_KEYUP;
                inputs.push(kd);
                inputs.push(ku);
            }
            if !inputs.is_empty() && !send_inputs(&inputs) {
                return Err("文字を入力できません".into());
            }
            return Ok(json!({ "ok": true, "next": "desktop.observe" }));
        }
        Err("Unknown desktop tool".into())
    }
}

impl Drop for DesktopSession {
    fn drop(&mut self) {
        {
            let mut guard = self.sustain.0.lock();
            guard.stop_thread = true;
            release_sustain_locked(&mut guard, "shutdown", false);
        }
        self.sustain.1.notify_all();
        if let Some(h) = self.sustain_thread.take() {
            let _ = h.join();
        }
        self.clear_observation();
    }
}

pub fn run_self_test() -> i32 {
    if parse_keys("CTRL+A").map(|v| v.len()).unwrap_or(0) != 2 {
        return 1;
    }
    if parse_keys("SPACE").map(|v| v.len()).unwrap_or(0) != 1
        || parse_keys("W").map(|v| v.len()).unwrap_or(0) != 1
        || parse_keys("F1").map(|v| v.len()).unwrap_or(0) != 1
        || parse_keys("0").map(|v| v.len()).unwrap_or(0) != 1
    {
        return 1;
    }
    if parse_keys("m").ok() != Some(vec![b'M' as u16])
        || parse_keys("escape").ok() != Some(vec![VK_ESCAPE.0])
        || parse_keys("Mキー").ok() != Some(vec![b'M' as u16])
    {
        return 1;
    }
    unsafe {
        if MapVirtualKeyW(b'W' as u32, MAPVK_VK_TO_VSC) == 0
            || MapVirtualKeyW(b'M' as u32, MAPVK_VK_TO_VSC) == 0
            || MapVirtualKeyW(VK_ESCAPE.0 as u32, MAPVK_VK_TO_VSC) == 0
        {
            return 1;
        }
    }
    if parse_keys("WIN+R").is_ok() || parse_keys("ALT+F4").is_ok() {
        return 1;
    }
    if parse_keys("MOUSE_LEFT").ok() != Some(vec![VK_LBUTTON.0])
        || parse_keys("LBUTTON").ok() != Some(vec![VK_LBUTTON.0])
        || parse_keys("MOUSE_RIGHT").ok() != Some(vec![VK_RBUTTON.0])
        || parse_keys("RBUTTON").ok() != Some(vec![VK_RBUTTON.0])
    {
        return 1;
    }
    for k in ["F", "G", "J", "C", "B", "T", "M"] {
        if !is_ui_key_for_sustain(k).unwrap_or(false) {
            return 1;
        }
    }
    for k in ["W", "SPACE", "1", "E", "Q", "R"] {
        if is_ui_key_for_sustain(k).unwrap_or(true) {
            return 1;
        }
    }
    let same = vec![42u8; 576];
    let diff = vec![120u8; 576];
    if compute_luma_motion_score(&same, &same) != 0.0
        || compute_luma_motion_score(&same, &diff) <= 0.015
        || compute_luma_motion_score(&[], &same) != 0.0
    {
        return 1;
    }
    if validate_sustain_params(&["W".into(), "SHIFT".into()], "hold", 8000)
        != Ok((8000, vec![b'W' as u16, VK_SHIFT.0]))
    {
        return 1;
    }
    if validate_sustain_params(&["MOUSE_LEFT".into()], "repeat", 20000)
        != Ok((20000, vec![VK_LBUTTON.0]))
    {
        return 1;
    }
    if validate_sustain_params(&["MOUSE_LEFT".into()], "repeat", 45000)
        != Ok((30000, vec![VK_LBUTTON.0]))
    {
        return 1;
    }
    if validate_sustain_params(&[], "hold", 100) != Ok((500, vec![])) {
        return 1;
    }
    if validate_sustain_params(&["W".into(), "A".into(), "S".into(), "D".into(), "SHIFT".into()], "hold", 8000).is_ok()
        || validate_sustain_params(&["W".into()], "tap", 8000).is_ok()
        || validate_sustain_params(&["WIN+R".into()], "hold", 8000).is_ok()
    {
        return 1;
    }
    let (rel1, prs1) = compute_key_diff(&[b'W' as u16], &[b'W' as u16, VK_SHIFT.0]);
    if !rel1.is_empty() || prs1 != vec![VK_SHIFT.0] {
        return 1;
    }
    let (rel2, prs2) = compute_key_diff(&[b'W' as u16, VK_SHIFT.0], &[b'A' as u16]);
    if rel2 != vec![VK_SHIFT.0, b'W' as u16] || prs2 != vec![b'A' as u16] {
        return 1;
    }
    if validate_drag_params(0.1, 0.2, 0.8, 0.9, "left").is_err()
        || validate_drag_params(0.5, 0.5, 0.7, 0.5, "none").is_err()
        || validate_drag_params(-0.1, 0.5, 0.5, 0.5, "left").is_ok()
        || validate_drag_params(0.1, 0.5, 0.5, 0.5, "middle").is_ok()
    {
        return 1;
    }
    unsafe {
        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            return 1;
        }
        let mem_dc = CreateCompatibleDC(Some(screen_dc));
        let bmp = if !mem_dc.is_invalid() {
            CreateCompatibleBitmap(screen_dc, 64, 64)
        } else {
            Default::default()
        };
        let bmp_w = if !mem_dc.is_invalid() {
            CreateCompatibleBitmap(screen_dc, 64, 64)
        } else {
            Default::default()
        };
        if mem_dc.is_invalid() || bmp.is_invalid() || bmp_w.is_invalid() {
            if !bmp.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(bmp.0));
            }
            if !bmp_w.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(bmp_w.0));
            }
            if !mem_dc.is_invalid() {
                let _ = DeleteDC(mem_dc);
            }
            ReleaseDC(None, screen_dc);
            return 1;
        }
        let old = SelectObject(mem_dc, HGDIOBJ(bmp.0));
        let r = RECT { left: 0, top: 0, right: 64, bottom: 64 };
        let _ = FillRect(mem_dc, &r, HBRUSH(GetStockObject(DKGRAY_BRUSH).0));
        let _ = SelectObject(mem_dc, HGDIOBJ(bmp_w.0));
        let _ = FillRect(mem_dc, &r, HBRUSH(GetStockObject(WHITE_BRUSH).0));
        let _ = SelectObject(mem_dc, old);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(None, screen_dc);

        let img = encode_hbitmap_to_budget(bmp, 900_000);
        let jpg = encode_hbitmap_jpeg(bmp, 90_000, [960, 768], [0.75, 0.60]);
        let ld = extract_luma_32x18(bmp);
        let lw = extract_luma_32x18(bmp_w);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteObject(HGDIOBJ(bmp_w.0));

        if img.is_none() || jpg.map(|j| j.is_empty()).unwrap_or(true) {
            return 1;
        }
        if ld.len() != 576 || lw.len() != 576 {
            return 1;
        }
        if compute_luma_motion_score(&ld, &ld) != 0.0
            || compute_luma_motion_score(&ld, &lw) <= 0.015
        {
            return 1;
        }
        if validate_video_params(9999, 10) != (4000, 6)
            || validate_video_params(100, 1) != (400, 2)
        {
            return 1;
        }
        let _ = is_current_process_elevated();
        let _ = is_window_process_elevated(GetDesktopWindow().0 as isize);
    }
    println!("Desktop key restrictions passed (no UI input sent)");
    let _ = io::stdout().flush();
    0
}

pub fn run_desktop_worker(args: &[String]) -> i32 {
    if args.len() == 2 && args[1] == "--self-test" {
        attach_parent_console_utf8();
        return run_self_test();
    }
    if args.len() < 3 || args.len() % 2 != 1 || args[1] != "--target" {
        return 2;
    }
    let _com = ComScope::ensure_mta();
    let target: isize = match args[2].parse::<u64>() {
        Ok(v) => v as isize,
        Err(_) => {
            println!("{}", json!({ "error": "Invalid target handle" }));
            return 1;
        }
    };
    let mut desktop = if target != 0 {
        match DesktopSession::new(target) {
            Ok(d) => Some(d),
            Err(e) => {
                println!("{}", json!({ "error": e }));
                return 1;
            }
        }
    } else {
        None
    };

    let local = if args.len() >= 5 {
        if args[3] != "--root" {
            println!("{}", json!({ "error": "Invalid local workspace argument" }));
            return 1;
        }
        let root = PathBuf::from(&args[4]);
        let mut read_roots = Vec::new();
        let mut i = 5usize;
        while i + 1 < args.len() {
            if args[i] != "--read-root" {
                println!("{}", json!({ "error": "Invalid read-only folder argument" }));
                return 1;
            }
            read_roots.push(PathBuf::from(&args[i + 1]));
            i += 2;
        }
        match LocalWorkspace::new(root, read_roots) {
            Ok(w) => Some(w),
            Err(e) => {
                println!("{}", json!({ "error": e }));
                return 1;
            }
        }
    } else {
        None
    };

    let stdin = io::stdin();
    let stdout = io::stdout();
    for line_res in stdin.lock().lines() {
        let Ok(line) = line_res else { break };
        let clean = line.trim_start_matches('\u{FEFF}');
        if clean.trim().is_empty() {
            continue;
        }
        if clean.len() > 3_145_728 {
            let mut out = stdout.lock();
            let _ = writeln!(out, "{}", json!({ "error": "Request too large" }));
            let _ = out.flush();
            continue;
        }
        let reply = match serde_json::from_str::<Value>(clean) {
            Err(e) => json!({ "error": e.to_string() }),
            Ok(req) => {
                let tool = req.get("tool").and_then(|v| v.as_str()).unwrap_or("");
                if tool.starts_with("local.") {
                    if let Some(ref l) = local {
                        match l.perform(&req, |prog| {
                            let mut out = stdout.lock();
                            let _ = writeln!(out, "{}", json!({ "progress": prog }));
                            let _ = out.flush();
                        }) {
                            Ok(v) => v,
                            Err(e) => json!({ "error": e }),
                        }
                    } else {
                        json!({ "error": "このPCのCLIは許可されていません" })
                    }
                } else if let Some(ref mut d) = desktop {
                    d.perform(&req)
                } else {
                    json!({ "error": "画面操作は許可されていません" })
                }
            }
        };
        let mut out = stdout.lock();
        let _ = writeln!(out, "{}", reply);
        let _ = out.flush();
    }
    0
}

pub struct DesktopWorkerProcess {
    target: isize,
    local_root: PathBuf,
    read_roots: Vec<PathBuf>,
    process: HANDLE,
    input: HANDLE,
    output: HANDLE,
    buffered: String,
    last_id: String,
    last_result: Value,
}

unsafe impl Send for DesktopWorkerProcess {}

impl DesktopWorkerProcess {
    pub fn new(target: isize, local_root: PathBuf, read_roots: Vec<PathBuf>) -> Self {
        Self {
            target,
            local_root,
            read_roots,
            process: HANDLE::default(),
            input: HANDLE::default(),
            output: HANDLE::default(),
            buffered: String::new(),
            last_id: String::new(),
            last_result: Value::Null,
        }
    }

    fn resolve_worker_binary() -> Result<PathBuf, String> {
        if let Ok(env_bin) = std::env::var("QWEN_CLIENT_BINARY") {
            let p = PathBuf::from(env_bin.trim());
            if p.is_file() {
                return Ok(p);
            }
        }
        let self_exe = crate::win_util::executable_path()?;
        let name = self_exe
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if name.contains("local-test") || name.contains("_test") {
            if let Some(dir) = self_exe.parent() {
                let c1 = dir.join("Qwen27.exe");
                if c1.is_file() {
                    return Ok(c1);
                }
                if let Some(p) = dir.parent() {
                    let c2 = p.join("Qwen27.exe");
                    if c2.is_file() {
                        return Ok(c2);
                    }
                }
            }
        }
        Ok(self_exe)
    }

    fn start(&mut self) -> Result<(), String> {
        if !self.process.is_invalid() && !self.process.0.is_null() {
            return Ok(());
        }
        if self.target != 0 && !is_window_eligible(self.target) {
            return Err("選択したウィンドウは操作できません".into());
        }
        let sec = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null::<core::ffi::c_void>() as *mut core::ffi::c_void,
            bInheritHandle: true.into(),
        };
        let mut child_in = HANDLE::default();
        let mut child_out = HANDLE::default();
        let mut p_in = HANDLE::default();
        let mut p_out = HANDLE::default();
        unsafe {
            CreatePipe(&mut child_in, &mut p_in, Some(&sec), 65536)
                .and_then(|_| CreatePipe(&mut p_out, &mut child_out, Some(&sec), 65536))
        }
        .map_err(|_| "PC操作用のパイプを作成できません".to_string())?;
        unsafe {
            let _ = SetHandleInformation(p_in, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0));
            let _ = SetHandleInformation(p_out, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0));
        }

        let bin = Self::resolve_worker_binary()?;
        let mut cmd = format!(
            "{} --target {}",
            quote_win_arg(&bin.to_string_lossy()),
            self.target as u64
        );
        if !self.local_root.as_os_str().is_empty() {
            cmd.push_str(&format!(" --root {}", quote_win_arg(&self.local_root.to_string_lossy())));
            for folder in &self.read_roots {
                cmd.push_str(&format!(" --read-root {}", quote_win_arg(&folder.to_string_lossy())));
            }
        }
        let bin_w = to_wide_null(&bin.to_string_lossy());
        let mut cmd_w = to_wide_null(&cmd);
        let mut si = STARTUPINFOW::default();
        si.cb = size_of::<STARTUPINFOW>() as u32;
        si.dwFlags = STARTF_USESTDHANDLES;
        si.hStdInput = child_in;
        si.hStdOutput = child_out;
        si.hStdError = child_out;
        let mut pi = PROCESS_INFORMATION::default();
        let launched = unsafe {
            CreateProcessW(
                PCWSTR(bin_w.as_ptr()),
                Some(PWSTR(cmd_w.as_mut_ptr())),
                None,
                None,
                true,
                CREATE_NO_WINDOW,
                None,
                PCWSTR::null(),
                &si,
                &mut pi,
            )
        };
        unsafe {
            let _ = CloseHandle(child_in);
            let _ = CloseHandle(child_out);
        }
        if launched.is_err() {
            unsafe {
                let _ = CloseHandle(p_in);
                let _ = CloseHandle(p_out);
            }
            return Err("PC操作ワーカーを起動できません".into());
        }
        unsafe {
            let _ = CloseHandle(pi.hThread);
            let _ = AllowSetForegroundWindow(pi.dwProcessId);
        }
        self.process = pi.hProcess;
        self.input = p_in;
        self.output = p_out;
        Ok(())
    }

    pub fn reset(&mut self) {
        if !self.input.is_invalid() && !self.input.0.is_null() {
            unsafe {
                let _ = CloseHandle(self.input);
            }
            self.input = HANDLE::default();
        }
        if !self.process.is_invalid() && !self.process.0.is_null() {
            if unsafe { WaitForSingleObject(self.process, 250) } != WAIT_OBJECT_0 {
                unsafe {
                    let _ = TerminateProcess(self.process, 1);
                }
            }
            unsafe {
                let _ = CloseHandle(self.process);
            }
            self.process = HANDLE::default();
        }
        if !self.output.is_invalid() && !self.output.0.is_null() {
            unsafe {
                let _ = CloseHandle(self.output);
            }
            self.output = HANDLE::default();
        }
        self.buffered.clear();
    }

    pub fn perform<F, K>(
        &mut self,
        action: &Value,
        stop: &AtomicBool,
        mut progress: F,
        mut keep_alive: K,
    ) -> Result<Value, String>
    where
        F: FnMut(Value),
        K: FnMut() -> bool,
    {
        if stop.load(Ordering::SeqCst) {
            return Err("PC操作は中断されました".into());
        }
        let id = action
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !id.is_empty() && self.last_id == id {
            return Ok(self.last_result.clone());
        }
        let mut line = serde_json::to_string(action).map_err(|e| e.to_string())?;
        line.push('\n');
        if line.len() > 3_145_728 {
            return Err("PC操作の入力が大きすぎます".into());
        }
        self.start()?;
        unsafe {
            let _ = AllowSetForegroundWindow(ASFW_ANY);
        }
        let mut written = 0u32;
        let wr = unsafe { WriteFile(self.input, Some(line.as_bytes()), Some(&mut written), None) };
        if wr.is_err() || written as usize != line.len() {
            self.reset();
            return Err("PC操作ヘルパーと通信できません".into());
        }
        let tool = action.get("tool").and_then(|v| v.as_str()).unwrap_or("");
        let wait_secs: u64 = if tool == "local.run" {
            let ts = action
                .get("args")
                .and_then(|a| a.get("timeoutSeconds"))
                .and_then(|v| v.as_i64())
                .unwrap_or(120)
                .clamp(1, 3600) as u64;
            ts + 5
        } else if matches!(tool, "local.put" | "local.get" | "local.put_abort") {
            40
        } else {
            25
        };
        let deadline = Instant::now() + Duration::from_secs(wait_secs);
        let mut checked = Instant::now();

        while !stop.load(Ordering::SeqCst) && Instant::now() < deadline {
            if checked.elapsed() >= Duration::from_secs(1) {
                checked = Instant::now();
                if !keep_alive() {
                    self.reset();
                    return Err("PC操作は中断されました".into());
                }
            }
            if let Some(nl) = self.buffered.find('\n') {
                let row_str = self.buffered[..nl].to_string();
                self.buffered.drain(..=nl);
                let parsed: Value = serde_json::from_str(&row_str).map_err(|e| e.to_string())?;
                if let Some(prog) = parsed.get("progress") {
                    progress(prog.clone());
                    continue;
                }
                self.last_id = id;
                self.last_result = parsed.clone();
                return Ok(parsed);
            }
            let mut avail = 0u32;
            if unsafe { PeekNamedPipe(self.output, None, 0, None, Some(&mut avail), None) }.is_err() {
                self.reset();
                return Err("PC操作ヘルパーが終了しました".into());
            }
            if avail > 0 {
                let mut buf = [0u8; 8192];
                let to_read = (avail as usize).min(buf.len());
                let mut count = 0u32;
                if unsafe { ReadFile(self.output, Some(&mut buf[..to_read]), Some(&mut count), None) }
                    .is_err()
                {
                    self.reset();
                    return Err("PC操作の結果を読めません".into());
                }
                self.buffered.push_str(&String::from_utf8_lossy(&buf[..count as usize]));
                if self.buffered.len() > 3_145_728 {
                    self.reset();
                    return Err("PC操作の応答が大きすぎます".into());
                }
            } else {
                thread::sleep(Duration::from_millis(12));
            }
        }
        self.reset();
        Err("PC操作がタイムアウトまたは中断されました".into())
    }
}

impl Drop for DesktopWorkerProcess {
    fn drop(&mut self) {
        self.reset();
    }
}
}

#[cfg(windows)]
pub use platform::{run_desktop_worker, DesktopWorkerProcess};

#[cfg(not(windows))]
mod platform {
    use crate::local_ws::LocalWorkspace;
    use serde_json::Value;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};

    pub fn run_desktop_worker(_args: &[String]) -> i32 {
        eprintln!("Desktop worker (--self-test / --target) is supported on Windows only.");
        1
    }

    pub struct DesktopWorkerProcess {
        local_root: PathBuf,
        read_roots: Vec<PathBuf>,
        ws: Option<LocalWorkspace>,
        last_id: String,
        last_result: Value,
    }

    impl DesktopWorkerProcess {
        pub fn new(_target: isize, local_root: PathBuf, read_roots: Vec<PathBuf>) -> Self {
            Self {
                local_root,
                read_roots,
                ws: None,
                last_id: String::new(),
                last_result: Value::Null,
            }
        }

        #[allow(dead_code)]
        pub fn reset(&mut self) {
            self.ws = None;
        }

        pub fn perform<F, K>(
            &mut self,
            action: &Value,
            stop: &AtomicBool,
            progress: F,
            mut keep_alive: K,
        ) -> Result<Value, String>
        where
            F: FnMut(Value),
            K: FnMut() -> bool,
        {
            if stop.load(Ordering::SeqCst) || !keep_alive() {
                return Err("操作は中断されました".into());
            }
            let id = action
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !id.is_empty() && self.last_id == id {
                return Ok(self.last_result.clone());
            }
            let tool = action.get("tool").and_then(|v| v.as_str()).unwrap_or("");
            if !tool.starts_with("local.") {
                return Err("PC画面操作 (desktop.*) は Windows のみ対応しています".into());
            }
            if self.local_root.as_os_str().is_empty() {
                return Err("ローカル作業フォルダーが設定されていません".into());
            }
            if self.ws.is_none() {
                self.ws = Some(LocalWorkspace::new(
                    self.local_root.clone(),
                    self.read_roots.clone(),
                )?);
            }
            let ws = self.ws.as_ref().expect("local workspace initialized");
            let res = ws.perform(action, progress)?;
            self.last_id = id;
            self.last_result = res.clone();
            Ok(res)
        }
    }
}

#[cfg(not(windows))]
pub use platform::{run_desktop_worker, DesktopWorkerProcess};
