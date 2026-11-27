use crate::wording::stage_label;
use crate::worker_panels::{execution_stage_label, step_label};
use serde_json::Value;
use std::io::Write;
use std::time::Instant;

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Debug, PartialEq, Eq)]
pub enum DeltaAction<'a> {
    None,
    Append(&'a str),
    ReplaceNewline(&'a str),
}

pub fn compute_delta<'a>(printed: &str, current: &'a str) -> DeltaAction<'a> {
    if current.is_empty() || current == printed {
        return DeltaAction::None;
    }
    if printed.is_empty() {
        return DeltaAction::Append(current);
    }
    if let Some(tail) = current.strip_prefix(printed) {
        return DeltaAction::Append(tail);
    }
    DeltaAction::ReplaceNewline(current)
}

pub fn poll_interval_ms(work_mode: &str, streaming_active: bool) -> u64 {
    match work_mode {
        "chat" => 80,
        "pc_control" => 150,
        _ if streaming_active => 250,
        _ => 400,
    }
}

pub struct StreamPrinter {
    printed: String,
    stream_enabled: bool,
    quiet: bool,
    ansi: bool,
    status_visible: bool,
    spin_idx: usize,
    last_stage: String,
    last_act_len: usize,
    started_at: Instant,
}

impl StreamPrinter {
    pub fn new(stream_enabled: bool, quiet: bool, ansi: bool) -> Self {
        Self {
            printed: String::new(),
            stream_enabled,
            quiet,
            ansi,
            status_visible: false,
            spin_idx: 0,
            last_stage: String::new(),
            last_act_len: 0,
            started_at: Instant::now(),
        }
    }

    pub fn clear_status(&mut self, err_w: &mut dyn Write) {
        if self.status_visible && self.ansi {
            let _ = write!(err_w, "\r\x1b[2K");
            let _ = err_w.flush();
        }
        self.status_visible = false;
    }

    pub fn update_stage(&mut self, job: &Value, err_w: &mut dyn Write) {
        if self.quiet || !self.printed.is_empty() {
            return;
        }
        let stage = job.get("stage").and_then(|v| v.as_str()).unwrap_or("");
        let label = execution_stage_label(job);
        let text = if label.is_empty() { stage_label(stage) } else { label };
        if self.ansi {
            let frame = SPINNER[self.spin_idx % SPINNER.len()];
            self.spin_idx = self.spin_idx.wrapping_add(1);
            let _ = write!(err_w, "\r\x1b[2K\x1b[2m{} {} ({:.1}s)\x1b[0m", frame, text, self.started_at.elapsed().as_secs_f64());
            let _ = err_w.flush();
            self.status_visible = true;
            self.last_stage = stage.to_string();
        } else if stage != self.last_stage {
            let _ = writeln!(err_w, "[{}] {}", stage, text);
            let _ = err_w.flush();
            self.last_stage = stage.to_string();
        }
    }

    pub fn update_activity(&mut self, job: &Value, err_w: &mut dyn Write) {
        if self.quiet || !self.printed.is_empty() {
            return;
        }
        let Some(rows) = job.get("activity").and_then(|v| v.as_array()) else { return };
        if rows.len() <= self.last_act_len {
            return;
        }
        self.clear_status(err_w);
        for row in &rows[self.last_act_len..] {
            let lbl = step_label(row);
            if lbl.is_empty() { continue; }
            if self.ansi {
                let _ = writeln!(err_w, "\x1b[2m  • {}\x1b[0m", lbl);
            } else {
                let _ = writeln!(err_w, "  • {}", lbl);
            }
        }
        let _ = err_w.flush();
        self.last_act_len = rows.len();
    }

    pub fn push_partial(&mut self, partial: &str, out_w: &mut dyn Write, err_w: &mut dyn Write) {
        if !self.stream_enabled {
            return;
        }
        match compute_delta(&self.printed, partial) {
            DeltaAction::None => {}
            DeltaAction::Append(delta) => {
                self.clear_status(err_w);
                let _ = write!(out_w, "{}", delta);
                let _ = out_w.flush();
                self.printed = partial.to_string();
            }
            DeltaAction::ReplaceNewline(full) => {
                self.clear_status(err_w);
                let _ = write!(out_w, "\n{}", full);
                let _ = out_w.flush();
                self.printed = full.to_string();
            }
        }
    }

    pub fn finish(&mut self, final_text: &str, out_w: &mut dyn Write, err_w: &mut dyn Write) {
        self.clear_status(err_w);
        if self.printed.is_empty() {
            if !final_text.is_empty() {
                let _ = write!(out_w, "{}", final_text);
                if !final_text.ends_with('\n') { let _ = writeln!(out_w); }
            }
        } else if let Some(tail) = final_text.strip_prefix(&self.printed) {
            if !tail.is_empty() { let _ = write!(out_w, "{}", tail); }
            if !final_text.ends_with('\n') { let _ = writeln!(out_w); }
        } else if !final_text.is_empty() && final_text.trim() != self.printed.trim() {
            let _ = write!(out_w, "\n{}", final_text);
            if !final_text.ends_with('\n') { let _ = writeln!(out_w); }
        } else if !self.printed.ends_with('\n') {
            let _ = writeln!(out_w);
        }
        let _ = out_w.flush();
    }

    pub fn print_footer(&self, job: &Value, err_w: &mut dyn Write) {
        if self.quiet { return; }
        let mut parts = vec![format!("{:.1}s", self.started_at.elapsed().as_secs_f64())];
        if let Some(t) = job.get("perf").and_then(|p| p.get("tokensPerSec")).and_then(|v| v.as_f64()).filter(|v| v.is_finite() && *v > 0.0) {
            parts.push(format!("{:.1} tok/s", t));
        }
        if let Some(cid) = job.get("conversationId").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            parts.push(format!("conv:{}", cid.chars().take(8).collect::<String>()));
        }
        let line = format!("[{}]", parts.join(" · "));
        if self.ansi { let _ = writeln!(err_w, "\x1b[2m{}\x1b[0m", line); } else { let _ = writeln!(err_w, "{}", line); }
        let _ = err_w.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_diff_and_stream_printer_work() {
        assert_eq!(compute_delta("", "こん"), DeltaAction::Append("こん"));
        assert_eq!(compute_delta("こん", "こんにちは"), DeltaAction::Append("にちは"));
        assert_eq!(compute_delta("こんにちは", "こんにちは"), DeltaAction::None);
        assert_eq!(compute_delta("古い", "新しい"), DeltaAction::ReplaceNewline("新しい"));

        let (mut p, mut out, mut err) = (StreamPrinter::new(true, true, false), Vec::new(), Vec::new());
        p.push_partial("答えは", &mut out, &mut err);
        p.push_partial("答えは 2", &mut out, &mut err);
        p.finish("答えは 2 です。", &mut out, &mut err);
        assert_eq!(String::from_utf8(out).unwrap(), "答えは 2 です。\n");

        let (mut b, mut out2) = (StreamPrinter::new(false, true, false), Vec::new());
        b.push_partial("途中", &mut out2, &mut err);
        assert!(out2.is_empty());
        b.finish("最終回答", &mut out2, &mut err);
        assert_eq!(String::from_utf8(out2).unwrap(), "最終回答\n");

        assert_eq!(poll_interval_ms("chat", false), 80);
        assert_eq!(poll_interval_ms("pc_control", false), 150);
        assert_eq!(poll_interval_ms("standard", true), 250);
        assert_eq!(poll_interval_ms("standard", false), 400);
    }
}
