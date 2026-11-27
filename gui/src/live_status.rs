use crate::wording::{error_code_of, error_code_wording};
use serde_json::Value;

pub const K_QUIET_MS: i64 = 180_000;
pub const K_STALE_MS: i64 = 600_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Liveness {
    pub newest_ms: i64,
    pub step_start_ms: i64,
}

pub fn iso_ms(text: &str) -> i64 {
    let b = text.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return 0;
    }
    let parse_n = |slice: &[u8]| -> Option<i64> {
        if !slice.iter().all(|c| c.is_ascii_digit()) {
            return None;
        }
        std::str::from_utf8(slice).ok()?.parse::<i64>().ok()
    };
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(s)) = (
        parse_n(&b[0..4]),
        parse_n(&b[5..7]),
        parse_n(&b[8..10]),
        parse_n(&b[11..13]),
        parse_n(&b[14..16]),
        parse_n(&b[17..19]),
    ) else {
        return 0;
    };
    if mo < 1 || mo > 12 || d < 1 || d > 31 || h < 0 || h > 23 || mi < 0 || mi > 59 || s < 0 || s > 60 || y < 1970 {
        return 0;
    }
    let mut ms: i64 = 0;
    let mut at = 19;
    if at < b.len() && b[at] == b'.' {
        at += 1;
        let mut digits = 0;
        while at < b.len() && b[at].is_ascii_digit() {
            if digits < 3 {
                ms = ms * 10 + (b[at] - b'0') as i64;
                digits += 1;
            }
            at += 1;
        }
        while digits < 3 {
            ms *= 10;
            digits += 1;
        }
    }
    let tz = &text[at..];
    if tz != "Z" && tz != "+00:00" {
        return 0;
    }
    let yy = y - if mo <= 2 { 1 } else { 0 };
    let era = yy / 400;
    let yoe = (yy - era * 400) as u64;
    let adj_mo = (mo + if mo > 2 { -3 } else { 9 }) as u64;
    let doy = (153 * adj_mo + 2) / 5 + (d as u64) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + (doe as i64) - 719_468;
    ((days * 24 + h) * 60 + mi) * 60_000 + s * 1000 + ms
}

pub fn scan_live(rows: &Value, live: &mut Liveness) {
    let Some(arr) = rows.as_array() else {
        return;
    };
    let stamp = |row: &Value, key: &str| -> i64 {
        row.get(key)
            .and_then(|v| v.as_str())
            .map(iso_ms)
            .unwrap_or(0)
    };
    for row in arr {
        if !row.is_object() {
            continue;
        }
        for key in [
            "startedAt",
            "finishedAt",
            "reasoningUpdatedAt",
            "executionStartedAt",
            "executionFinishedAt",
        ] {
            live.newest_ms = live.newest_ms.max(stamp(row, key));
        }
        if let Some(sup) = row.get("supervision").and_then(|v| v.as_array()) {
            for verdict in sup {
                if verdict.is_object() {
                    live.newest_ms = live.newest_ms.max(stamp(verdict, "time"));
                }
            }
        }
        let children = row.get("workerActivity").and_then(|v| v.as_array());
        let nested = children.map(|c| !c.is_empty()).unwrap_or(false);
        if !nested && row.get("status").and_then(|v| v.as_str()) == Some("running") {
            live.step_start_ms = live.step_start_ms.max(stamp(row, "startedAt"));
        }
        if let Some(child_arr) = children {
            if !child_arr.is_empty() {
                scan_live(&Value::Array(child_arr.clone()), live);
            }
        }
    }
}

pub fn liveness(job: &Value) -> Liveness {
    let mut live = Liveness::default();
    if let Some(act) = job.get("activity") {
        scan_live(act, &mut live);
    }
    for key in ["startedAt", "createdAt"] {
        if let Some(s) = job.get(key).and_then(|v| v.as_str()) {
            let at = iso_ms(s);
            if at != 0 {
                live.newest_ms = live.newest_ms.max(at);
                break;
            }
        }
    }
    live
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LiveClock {
    pub skew_ms: i64,
    pub known: bool,
}

impl LiveClock {
    pub fn observe(&mut self, server_ms: i64, client_ms: i64) {
        if server_ms <= 0 {
            return;
        }
        let d = (client_ms - server_ms).clamp(-3_600_000, 3_600_000);
        if !self.known || d < self.skew_ms {
            self.skew_ms = d;
            self.known = true;
        }
    }

    pub fn age(&self, server_ms: i64, client_ms: i64) -> i64 {
        if server_ms <= 0 {
            -1
        } else {
            (client_ms - self.skew_ms - server_ms).max(0)
        }
    }
}

pub fn last_update_ago(
    read_only: bool,
    clock: &LiveClock,
    newest_ms: i64,
    now_ms: i64,
    seen_ms: i64,
) -> i64 {
    if read_only {
        return -1;
    }
    let ago = if clock.known {
        clock.age(newest_ms, now_ms)
    } else {
        -1
    };
    if seen_ms < 0 {
        ago
    } else if ago < 0 {
        seen_ms
    } else {
        ago.min(seen_ms)
    }
}

pub fn format_span(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    if s < 60 {
        format!("{}秒", s)
    } else if s < 3600 {
        format!("{}分{:02}秒", s / 60, s % 60)
    } else {
        format!("{}時間{:02}分", s / 3600, (s % 3600) / 60)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveTail {
    pub step: String,
    pub fresh: String,
    pub stale: bool,
}

pub fn live_tail(step_ms: i64, ago_ms: i64, pc_control: bool) -> LiveTail {
    let mut tail = LiveTail::default();
    if !pc_control && step_ms >= 0 {
        tail.step = format!("  ·  この手順 {}", format_span(step_ms));
    }
    if ago_ms >= 0 {
        let suffix = if ago_ms >= K_STALE_MS {
            "（長く応答がありません）"
        } else if ago_ms >= K_QUIET_MS {
            "（応答を待っています）"
        } else {
            ""
        };
        tail.fresh = format!("  ·  最終更新 {}前{}", format_span(ago_ms), suffix);
        tail.stale = ago_ms >= K_STALE_MS;
    }
    tail
}

pub fn stop_reason_label(reason: &str) -> (String, bool) {
    let table = [
        ("achieved", "目標を達成"),
        ("not_achieved", "目標を達成できずに終了"),
        ("handoff", "確認区切りで計画担当へ引き継ぎ"),
        ("failed", "最終回答に至らず停止"),
        ("cancelled", "ユーザーが中断"),
        ("error", "エラーで停止"),
        ("circuit_breaker", "失敗の繰り返しで停止"),
        ("advisor_no_progress", "助言後も進展がなく停止"),
        ("supervisor_stop", "27Bの監督判断で中断"),
        ("context_limit", "入力上限で停止"),
        ("max_iterations", "ステップ上限で停止"),
    ];
    for (k, v) in table {
        if reason == k {
            return (v.to_string(), true);
        }
    }
    let text = if reason.is_empty() {
        String::new()
    } else {
        format!("その他（{}）", reason)
    };
    (text, false)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResultCard {
    pub line: String,
    pub badge: String,
}

fn find_breaker<'a>(rows: &'a Value, found: &mut Option<&'a Value>) {
    let Some(arr) = rows.as_array() else {
        return;
    };
    for row in arr {
        if !row.is_object() {
            continue;
        }
        if let Some(cb) = row.get("circuitBreaker") {
            if cb.is_object() && cb.get("open").and_then(|v| v.as_bool()) == Some(true) {
                *found = Some(cb);
            }
        }
        if let Some(children) = row.get("workerActivity") {
            find_breaker(children, found);
        }
    }
}

fn count_runs(rows: &Value, runs: &mut i64, failed: &mut i64) {
    let Some(arr) = rows.as_array() else {
        return;
    };
    for row in arr {
        if !row.is_object() {
            continue;
        }
        let tool = row.get("tool").and_then(|v| v.as_str()).unwrap_or("");
        let code = row.get("exitCode").and_then(|v| v.as_i64());
        let done = row.get("executed").and_then(|v| v.as_bool()).unwrap_or(true);
        let reused = row.get("reused").and_then(|v| v.as_bool()).unwrap_or(false);
        if tool.ends_with(".run") && code.is_some() && done && !reused {
            *runs += 1;
            if code.unwrap_or(0) != 0 {
                *failed += 1;
            }
        }
        if let Some(children) = row.get("workerActivity") {
            count_runs(children, runs, failed);
        }
    }
}

fn count_error_code(rows: &Value, code: &str, count: &mut i64) {
    let Some(arr) = rows.as_array() else {
        return;
    };
    for row in arr {
        if !row.is_object() {
            continue;
        }
        if error_code_of(row) == code {
            *count += 1;
        }
        if let Some(children) = row.get("workerActivity") {
            count_error_code(children, code, count);
        }
    }
}

pub fn result_card(job: &Value, pc_control: bool) -> ResultCard {
    let empty_obj = Value::Object(Default::default());
    let r = job
        .get("result")
        .filter(|v| v.is_object())
        .unwrap_or(&empty_obj);
    let num = |o: &Value, k: &str, fallback: i64| -> i64 {
        o.get(k).and_then(|v| v.as_i64()).unwrap_or(fallback)
    };
    let status = job.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let achieved = r.get("achieved").and_then(|v| v.as_bool()) == Some(true);
    let mut line = if status == "cancelled" {
        "中断しました".to_string()
    } else if status == "failed" {
        "失敗しました".to_string()
    } else if status == "interrupted" {
        "中断されました".to_string()
    } else if achieved {
        "✓ 完了".to_string()
    } else {
        "未達成の内容を回答済み".to_string()
    };

    let reason = r.get("stopReason").and_then(|v| v.as_str()).unwrap_or("");
    if !reason.is_empty() {
        line.push_str("  ·  停止理由: ");
        line.push_str(&stop_reason_label(reason).0);
    }

    let commands = r
        .get("executionEvidence")
        .and_then(|v| v.get("commands"))
        .and_then(|v| v.as_array());
    let mut total = num(r, "commandsExecuted", -1);
    let mut failed: i64 = 0;
    let mut partial = false;
    if let Some(cmds) = commands {
        for c in cmds {
            if c.get("exitCode").and_then(|v| v.as_i64()).unwrap_or(0) != 0 {
                failed += 1;
            }
        }
        if total < 0 {
            total = cmds.len() as i64;
        } else {
            partial = total > cmds.len() as i64;
        }
    }

    let mut runs = 0;
    let mut run_failed = 0;
    if let Some(act) = job.get("activity") {
        count_runs(act, &mut runs, &mut run_failed);
    }
    if runs > 0 && runs > total {
        total = runs;
        partial = false;
    }
    failed = failed.max(run_failed);

    if total >= 0 {
        line.push_str(&format!("  ·  コマンド {}回", total));
        if total > 0 && (commands.is_some() || failed > 0 || runs > 0) {
            line.push_str(&format!(
                "（失敗 {}回{}）",
                failed,
                if partial { "以上" } else { "" }
            ));
        }
    }

    let mode = job
        .get("options")
        .and_then(|v| v.get("workMode"))
        .and_then(|v| v.as_str());
    let is_pc = pc_control || mode == Some("pc_control");
    let is_fast_chat = mode == Some("chat") && r.get("chatFast").and_then(|v| v.as_bool()) == Some(true);
    if !is_pc && !is_fast_chat {
        line.push_str(&format!("  ·  {}ステップ", num(r, "iterations", 0)));
    }

    let mut breaker = None;
    if let Some(act) = job.get("activity") {
        find_breaker(act, &mut breaker);
    }
    let detail = if let Some(b) = breaker {
        let tool = b.get("tool").and_then(|v| v.as_str()).unwrap_or("");
        let n = num(b, "failures", 0);
        if tool.is_empty() || n <= 0 {
            String::new()
        } else {
            format!("（{} が{}回続けて）", tool, n)
        }
    } else {
        String::new()
    };
    let stalled = breaker
        .and_then(|b| b.get("message"))
        .and_then(|v| v.as_str())
        .map(|s| s.contains("得られていません"))
        .unwrap_or(false);

    let mut badge = if reason == "circuit_breaker" {
        if stalled {
            format!("⚠ 同じ操作で進展がないため停止しました{}", detail)
        } else {
            format!("⚠ 同じ失敗の繰り返しを検知して停止しました{}", detail)
        }
    } else if reason == "advisor_no_progress" {
        "⚠ 助言を受けても進展がなく停止しました".to_string()
    } else if breaker.is_some() {
        if stalled {
            format!("⚠ 途中で同じ操作の繰り返しを検知しました{}", detail)
        } else {
            format!("⚠ 途中で同じ失敗の繰り返しを検知しました{}", detail)
        }
    } else {
        String::new()
    };

    let mut resume_stalled = 0;
    let mut rejected = 0;
    if let Some(act) = job.get("activity") {
        count_error_code(act, "RESUME_STALLED", &mut resume_stalled);
        count_error_code(act, "PLAN_REJECTED", &mut rejected);
    }
    let job_code = error_code_of(job);
    if job_code == "RESUME_STALLED" {
        resume_stalled = resume_stalled.max(1);
    }
    if job_code == "PLAN_REJECTED" {
        rejected = rejected.max(1);
    }
    if rejected > 0 {
        line.push_str(&format!("  ·  計画の差し戻し {}回", rejected));
    }
    let mut add_badge = |text: String| {
        if !badge.is_empty() {
            badge.push_str("\n");
        }
        badge.push_str(&text);
    };
    if resume_stalled > 0 {
        add_badge(format!(
            "⚠ 同じ失敗が続いたため、計画役の再開を止めました（{}回）",
            resume_stalled
        ));
    }
    if job_code != "RESUME_STALLED" && job_code != "PLAN_REJECTED" {
        if let Some(known) = error_code_wording(&job_code) {
            add_badge(format!("⚠ {}", known.label));
        }
    }

    ResultCard { line, badge }
}

pub fn flashes_taskbar(status: &str) -> bool {
    matches!(
        status,
        "completed" | "failed" | "cancelled" | "interrupted" | "waiting_user"
    )
}

pub fn format_elapsed_precise(ms: i64) -> String {
    if ms < 0 {
        return String::new();
    }
    if ms < 60_000 {
        return format!("{:.1}秒", (ms as f64) / 1000.0);
    }
    if ms < 3_600_000 {
        let total_sec = (ms + 500) / 1000;
        return format!("{}分{:02}秒", total_sec / 60, total_sec % 60);
    }
    let total_min = (ms + 30_000) / 60_000;
    format!("{}時間{:02}分", total_min / 60, total_min % 60)
}

pub fn job_elapsed_ms(job: &Value) -> Option<i64> {
    if let Some(ms) = job
        .get("perf")
        .and_then(|p| p.get("elapsedMs"))
        .and_then(|v| v.as_i64())
    {
        if ms >= 0 {
            return Some(ms);
        }
    }
    let end = job
        .get("finishedAt")
        .and_then(|v| v.as_str())
        .map(iso_ms)
        .unwrap_or(0);
    let start = job
        .get("createdAt")
        .and_then(|v| v.as_str())
        .map(iso_ms)
        .filter(|&t| t > 0)
        .or_else(|| {
            job.get("startedAt")
                .and_then(|v| v.as_str())
                .map(iso_ms)
                .filter(|&t| t > 0)
        })
        .unwrap_or(0);
    if start > 0 && end > 0 {
        Some((end - start).max(0))
    } else {
        None
    }
}

pub fn job_tokens_per_sec(job: &Value) -> Option<f64> {
    job.get("perf")
        .and_then(|p| p.get("tokensPerSec"))
        .and_then(|v| v.as_f64())
        .or_else(|| {
            job.get("_perf")
                .and_then(|p| p.get("tokensPerSec"))
                .and_then(|v| v.as_f64())
        })
        .filter(|v| v.is_finite() && *v > 0.0)
}

pub fn format_turn_metrics(job: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(ms) = job_elapsed_ms(job) {
        let s = format_elapsed_precise(ms);
        if !s.is_empty() {
            parts.push(s);
        }
    }
    if let Some(tps) = job_tokens_per_sec(job) {
        parts.push(format!("{:.1} tok/s", tps));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn iso_ms_and_clock_and_spans() {
        assert_eq!(iso_ms("1970-01-01T00:00:00Z"), 0);
        assert_eq!(iso_ms("1970-01-01T00:00:01.25Z"), 1250);
        assert_eq!(iso_ms("2026-10-22T16:57:09.599Z"), 1_792_688_229_599);
        assert_eq!(iso_ms("not-a-timestamp"), 0);

        let mut clock = LiveClock::default();
        clock.observe(10_000, 15_000);
        assert_eq!(clock.age(11_000, 16_500), 500);
        assert_eq!(format_span(65_000), "1分05秒");
        assert_eq!(format_span(7_380_000), "2時間03分");
    }

    #[test]
    fn result_card_formats_commands_and_pc_control() {
        let job = json!({
            "status": "completed",
            "options": {"workMode": "coding"},
            "result": {
                "achieved": true,
                "stopReason": "achieved",
                "commandsExecuted": 2,
                "iterations": 4,
                "executionEvidence": {"commands": [{"exitCode": 0}, {"exitCode": 1}]}
            }
        });
        let card = result_card(&job, false);
        assert!(card.line.contains("✓ 完了"));
        assert!(card.line.contains("コマンド 2回（失敗 1回）"));
        assert!(card.line.contains("4ステップ"));

        let pc_card = result_card(&job, true);
        assert!(!pc_card.line.contains("ステップ"));

        let chat_job = json!({
            "status": "completed",
            "options": {"workMode": "chat"},
            "result": {
                "achieved": true,
                "stopReason": "achieved",
                "iterations": 1,
                "chatFast": true
            }
        });
        let chat_card = result_card(&chat_job, false);
        assert!(!chat_card.line.contains("ステップ"));
    }

    #[test]
    fn turn_metrics_format_elapsed_and_speed() {
        assert_eq!(format_elapsed_precise(4200), "4.2秒");
        assert_eq!(format_elapsed_precise(65_400), "1分05秒");
        assert_eq!(format_elapsed_precise(3_660_000), "1時間01分");

        let j_perf = json!({
            "createdAt": "2026-11-23T10:00:00.000Z",
            "finishedAt": "2026-11-23T10:00:09.000Z",
            "perf": { "elapsedMs": 4200, "tokensPerSec": 38.6 }
        });
        assert_eq!(format_turn_metrics(&j_perf), "4.2秒 · 38.6 tok/s");

        let j_fallback = json!({
            "createdAt": "2026-11-23T10:00:00.000Z",
            "finishedAt": "2026-11-23T10:01:12.000Z",
            "_perf": { "tokensPerSec": 41.2 }
        });
        assert_eq!(format_turn_metrics(&j_fallback), "1分12秒 · 41.2 tok/s");
    }
}
