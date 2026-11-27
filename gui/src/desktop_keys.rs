#![cfg(windows)]

use windows::Win32::UI::Input::KeyboardAndMouse::{
    MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MAPVK_VK_TO_VSC, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEINPUT, VIRTUAL_KEY,
    VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_HOME, VK_INSERT,
    VK_LBUTTON, VK_LEFT, VK_NEXT, VK_PRIOR, VK_RBUTTON, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE,
    VK_TAB, VK_UP,
};

pub const VK_LBUTTON_U16: u16 = VK_LBUTTON.0;
pub const VK_RBUTTON_U16: u16 = VK_RBUTTON.0;

pub fn parse_keys(raw_name: &str) -> Result<Vec<u16>, String> {
    let mut name: String = raw_name
        .chars()
        .filter(|&c| !matches!(c, ' ' | '\t' | '\r' | '\n'))
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if let Some(stripped) = name.strip_suffix("キー") {
        name = stripped.to_string();
    }
    if name.starts_with("KEY_") || name.starts_with("KEY-") {
        name.drain(0..4);
    } else if name.starts_with("VK_") {
        name.drain(0..3);
    }
    name = match name.as_str() {
        "ESCAPE" => "ESC".into(),
        "RETURN" => "ENTER".into(),
        "SPACEBAR" => "SPACE".into(),
        "BACK" => "BACKSPACE".into(),
        "DEL" => "DELETE".into(),
        "PGUP" => "PAGEUP".into(),
        "PGDN" => "PAGEDOWN".into(),
        _ => name,
    };

    let vk_ctrl = VK_CONTROL.0;
    let vk_shift = VK_SHIFT.0;
    let mapped: Option<Vec<u16>> = match name.as_str() {
        "ENTER" => Some(vec![VK_RETURN.0]),
        "TAB" => Some(vec![VK_TAB.0]),
        "ESC" => Some(vec![VK_ESCAPE.0]),
        "BACKSPACE" => Some(vec![VK_BACK.0]),
        "DELETE" => Some(vec![VK_DELETE.0]),
        "SPACE" => Some(vec![VK_SPACE.0]),
        "SHIFT" => Some(vec![vk_shift]),
        "CTRL" => Some(vec![vk_ctrl]),
        "UP" => Some(vec![VK_UP.0]),
        "DOWN" => Some(vec![VK_DOWN.0]),
        "LEFT" => Some(vec![VK_LEFT.0]),
        "RIGHT" => Some(vec![VK_RIGHT.0]),
        "HOME" => Some(vec![VK_HOME.0]),
        "END" => Some(vec![VK_END.0]),
        "PAGEUP" => Some(vec![VK_PRIOR.0]),
        "PAGEDOWN" => Some(vec![VK_NEXT.0]),
        "MOUSE_LEFT" | "LBUTTON" => Some(vec![VK_LBUTTON_U16]),
        "MOUSE_RIGHT" | "RBUTTON" => Some(vec![VK_RBUTTON_U16]),
        "CTRL+A" => Some(vec![vk_ctrl, b'A' as u16]),
        "CTRL+C" => Some(vec![vk_ctrl, b'C' as u16]),
        "CTRL+V" => Some(vec![vk_ctrl, b'V' as u16]),
        "CTRL+Z" => Some(vec![vk_ctrl, b'Z' as u16]),
        "CTRL+S" => Some(vec![vk_ctrl, b'S' as u16]),
        "CTRL+F" => Some(vec![vk_ctrl, b'F' as u16]),
        "CTRL+L" => Some(vec![vk_ctrl, b'L' as u16]),
        "CTRL+O" => Some(vec![vk_ctrl, b'O' as u16]),
        "CTRL+W" => Some(vec![vk_ctrl, b'W' as u16]),
        "SHIFT+TAB" => Some(vec![vk_shift, VK_TAB.0]),
        _ => None,
    };
    if let Some(v) = mapped {
        return Ok(v);
    }
    let bytes = name.as_bytes();
    if bytes.len() == 1 && (bytes[0].is_ascii_uppercase() || bytes[0].is_ascii_digit()) {
        return Ok(vec![bytes[0] as u16]);
    }
    if bytes.len() >= 2 && bytes[0] == b'F' && name[1..].bytes().all(|b| b.is_ascii_digit()) {
        if let Ok(f) = name[1..].parse::<u16>() {
            if (1..=12).contains(&f) {
                return Ok(vec![VK_F1.0 + (f - 1)]);
            }
        }
    }
    Err("Unsupported key. System shortcuts are not available.".into())
}

pub fn is_ui_key_for_sustain(raw_name: &str) -> Result<bool, String> {
    let vks = parse_keys(raw_name)?;
    Ok(vks.into_iter().any(|vk| {
        vk == VK_ESCAPE.0
            || vk == b'M' as u16
            || vk == b'F' as u16
            || vk == b'G' as u16
            || vk == b'J' as u16
            || vk == b'C' as u16
            || vk == b'B' as u16
            || vk == b'T' as u16
            || vk == VK_TAB.0
            || vk == VK_RETURN.0
            || vk == VK_BACK.0
            || vk == VK_DELETE.0
            || vk == VK_CONTROL.0
            || (vk >= VK_F1.0 && vk <= VK_F1.0 + 11)
    }))
}

pub fn validate_sustain_params(
    key_names: &[String],
    mode: &str,
    ms: i32,
) -> Result<(i32, Vec<u16>), String> {
    if key_names.len() > 4 {
        return Err("Sustain keys are limited to 4".into());
    }
    if mode != "hold" && mode != "repeat" {
        return Err("Sustain mode must be 'hold' or 'repeat'".into());
    }
    let mut unique = Vec::new();
    for name in key_names {
        for vk in parse_keys(name)? {
            if !unique.contains(&vk) {
                unique.push(vk);
            }
        }
    }
    Ok((ms.clamp(500, 30000), unique))
}

pub fn compute_key_diff(pressed: &[u16], target: &[u16]) -> (Vec<u16>, Vec<u16>) {
    let mut to_release = Vec::new();
    for &vk in pressed.iter().rev() {
        if !target.contains(&vk) {
            to_release.push(vk);
        }
    }
    let mut to_press = Vec::new();
    for &vk in target {
        if !pressed.contains(&vk) {
            to_press.push(vk);
        }
    }
    (to_release, to_press)
}

pub fn validate_drag_params(
    from_x: f64,
    from_y: f64,
    to_x: f64,
    to_y: f64,
    button: &str,
) -> Result<(), String> {
    let ok_coord = [from_x, from_y, to_x, to_y]
        .iter()
        .all(|v| v.is_finite() && (0.0..=1.0).contains(v));
    if !ok_coord {
        return Err("Drag coordinates must be in 0.0 to 1.0".into());
    }
    if button != "left" && button != "right" && button != "none" {
        return Err("Unknown drag button".into());
    }
    Ok(())
}

pub fn validate_video_params(duration_ms: i32, fps: i32) -> (i32, i32) {
    let out_ms = (if duration_ms > 0 { duration_ms } else { 1500 }).clamp(400, 4000);
    let out_fps = (if fps > 0 { fps } else { 4 }).clamp(2, 6);
    (out_ms, out_fps)
}

fn is_extended_vk(vk: u16) -> bool {
    vk == VK_UP.0
        || vk == VK_DOWN.0
        || vk == VK_LEFT.0
        || vk == VK_RIGHT.0
        || vk == VK_HOME.0
        || vk == VK_END.0
        || vk == VK_PRIOR.0
        || vk == VK_NEXT.0
        || vk == VK_DELETE.0
        || vk == VK_INSERT.0
}

pub fn make_single_input(vk: u16, down: bool) -> INPUT {
    if vk == VK_LBUTTON_U16 || vk == VK_RBUTTON_U16 {
        let flags = if vk == VK_LBUTTON_U16 {
            if down { MOUSEEVENTF_LEFTDOWN } else { MOUSEEVENTF_LEFTUP }
        } else if down {
            MOUSEEVENTF_RIGHTDOWN
        } else {
            MOUSEEVENTF_RIGHTUP
        };
        return INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dwFlags: flags,
                    ..Default::default()
                },
            },
        };
    }
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
    let ext = if is_extended_vk(vk) {
        KEYEVENTF_EXTENDEDKEY
    } else {
        Default::default()
    };
    let up = if down {
        Default::default()
    } else {
        KEYEVENTF_KEYUP
    };
    let ki = if scan != 0 {
        KEYBDINPUT {
            wVk: VIRTUAL_KEY(0),
            wScan: scan,
            dwFlags: KEYEVENTF_SCANCODE | up | ext,
            ..Default::default()
        }
    } else {
        KEYBDINPUT {
            wVk: VIRTUAL_KEY(vk),
            wScan: 0,
            dwFlags: up | ext,
            ..Default::default()
        }
    };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki },
    }
}

pub fn send_single_input(vk: u16, down: bool) -> bool {
    let i = [make_single_input(vk, down)];
    unsafe { SendInput(&i, std::mem::size_of::<INPUT>() as i32) == 1 }
}

pub fn send_inputs(inputs: &[INPUT]) -> bool {
    if inputs.is_empty() {
        return true;
    }
    unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) as usize == inputs.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_key_restrictions_pass_no_ui_input() {
        assert_eq!(parse_keys("CTRL+A").unwrap().len(), 2);
        assert_eq!(parse_keys("SPACE").unwrap().len(), 1);
        assert_eq!(parse_keys("W").unwrap().len(), 1);
        assert_eq!(parse_keys("F1").unwrap().len(), 1);
        assert_eq!(parse_keys("0").unwrap().len(), 1);
        assert_eq!(parse_keys("m").unwrap(), vec![b'M' as u16]);
        assert_eq!(parse_keys("escape").unwrap(), vec![VK_ESCAPE.0]);
        assert_eq!(parse_keys("Mキー").unwrap(), vec![b'M' as u16]);
        unsafe {
            assert_ne!(MapVirtualKeyW(b'W' as u32, MAPVK_VK_TO_VSC), 0);
            assert_ne!(MapVirtualKeyW(b'M' as u32, MAPVK_VK_TO_VSC), 0);
            assert_ne!(MapVirtualKeyW(VK_ESCAPE.0 as u32, MAPVK_VK_TO_VSC), 0);
        }
        assert!(parse_keys("WIN+R").is_err());
        assert!(parse_keys("ALT+F4").is_err());
        assert_eq!(parse_keys("MOUSE_LEFT").unwrap(), vec![VK_LBUTTON_U16]);
        assert_eq!(parse_keys("MOUSE_RIGHT").unwrap(), vec![VK_RBUTTON_U16]);

        for k in ["F", "G", "J", "C", "B", "T", "M"] {
            assert!(is_ui_key_for_sustain(k).unwrap());
        }
        for k in ["W", "SPACE", "1", "E", "Q", "R"] {
            assert!(!is_ui_key_for_sustain(k).unwrap());
        }

        let (ms, vks) = validate_sustain_params(&["W".into(), "SHIFT".into()], "hold", 8000).unwrap();
        assert_eq!(ms, 8000);
        assert_eq!(vks, vec![b'W' as u16, VK_SHIFT.0]);
        assert_eq!(
            validate_sustain_params(&["MOUSE_LEFT".into()], "repeat", 45000).unwrap(),
            (30000, vec![VK_LBUTTON_U16])
        );
        assert!(validate_sustain_params(&["W".into(), "A".into(), "S".into(), "D".into(), "SHIFT".into()], "hold", 8000).is_err());
        assert!(validate_sustain_params(&["W".into()], "tap", 8000).is_err());
        assert!(validate_sustain_params(&["WIN+R".into()], "hold", 8000).is_err());

        let (rel, prs) = compute_key_diff(&[b'W' as u16], &[b'W' as u16, VK_SHIFT.0]);
        assert!(rel.is_empty());
        assert_eq!(prs, vec![VK_SHIFT.0]);

        validate_drag_params(0.1, 0.2, 0.8, 0.9, "left").unwrap();
        validate_drag_params(0.5, 0.5, 0.7, 0.5, "none").unwrap();
        assert!(validate_drag_params(-0.1, 0.5, 0.5, 0.5, "left").is_err());
        assert!(validate_drag_params(0.1, 0.5, 0.5, 0.5, "middle").is_err());

        assert_eq!(validate_video_params(9999, 10), (4000, 6));
        assert_eq!(validate_video_params(100, 1), (400, 2));
    }
}
