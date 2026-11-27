use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

pub const UNKNOWN_ERROR_LABEL: &str = "エラーが起きました";
pub const UNKNOWN_ERROR_HINT: &str = "詳細は下の原文を確認してください";
pub const CONTINUE_HINT: &str = "「続ける」で同じチャットのまま続きを依頼できます";
pub const CONTINUE_MESSAGE: &str = "つづけて";
#[allow(dead_code)]
pub const RESEND_LABEL: &str = "もう一度送る";
pub const EMPTY_GOAL_MESSAGE: &str = "依頼を入力してから送信してください";
pub const NOT_CONNECTED_MESSAGE: &str = "先にサーバーへ接続してください（左下の「接続 / 更新」）";
pub const READ_ONLY_MESSAGE: &str = "閲覧専用のチャットには送信できません";

#[derive(Clone, Debug)]
struct StageRule {
    pattern: String,
    label: String,
}

#[derive(Clone, Debug)]
struct ErrorRule {
    contains: String,
    label: String,
    hint: String,
    retry: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorCodeWording {
    pub code: String,
    pub label: String,
    pub hint: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorNotice {
    pub label: String,
    pub hint: String,
    pub retry: bool,
    pub raw: String,
    pub known: bool,
}

struct WordingTables {
    stages: Vec<StageRule>,
    errors: Vec<ErrorRule>,
    error_codes: Vec<ErrorCodeWording>,
}

static TABLES: Lazy<WordingTables> = Lazy::new(|| {
    let raw: Value = serde_json::from_str(include_str!("../wording.json"))
        .expect("embedded wording.json must be valid JSON");

    let mut stages = Vec::new();
    if let Some(arr) = raw.get("stages").and_then(|v| v.as_array()) {
        for item in arr {
            let pattern = item.get("match").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let label = item.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if pattern.is_empty() {
                continue;
            }
            stages.push(StageRule { pattern: pattern.clone(), label: label.clone() });
            // Inject real runtime extra patterns immediately following base rule just like wording.hpp
            match pattern.as_str() {
                "waiting" => stages.push(StageRule {
                    pattern: "waiting_user".into(),
                    label: "回答待ち".into(),
                }),
                "cancelled" => stages.push(StageRule {
                    pattern: "interrupted".into(),
                    label: "停止しました".into(),
                }),
                "working:{task}" => stages.push(StageRule {
                    pattern: "working:{task}:*".into(),
                    label: "作業中（{task}）".into(),
                }),
                "repair:{n}" => stages.push(StageRule {
                    pattern: "repair:{n}:*".into(),
                    label: "修正（{n}回目）".into(),
                }),
                "planner:advice" => stages.push(StageRule {
                    pattern: "interrupting".into(),
                    label: "追加指示を処理中".into(),
                }),
                "tool:task.dispute" => {
                    stages.push(StageRule {
                        pattern: "tool:local.fetch".into(),
                        label: "サーバーからPCへファイルを写し中".into(),
                    });
                    stages.push(StageRule {
                        pattern: "tool:workspace.fetch".into(),
                        label: "PCからサーバーへファイルを写し中".into(),
                    });
                }
                _ => {}
            }
        }
    }

    let mut errors = Vec::new();
    if let Some(arr) = raw.get("errors").and_then(|v| v.as_array()) {
        for item in arr {
            let contains = item.get("contains").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let label = item.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let hint = item.get("hint").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let retry = item.get("retry").and_then(|v| v.as_bool()).unwrap_or(false);
            if !contains.is_empty() {
                errors.push(ErrorRule { contains, label, hint, retry });
            }
        }
    }

    let mut error_codes = Vec::new();
    if let Some(obj) = raw.get("errorCodes").and_then(|v| v.as_object()) {
        for (code, item) in obj {
            let label = item.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let hint = item.get("hint").and_then(|v| v.as_str()).unwrap_or("").to_string();
            error_codes.push(ErrorCodeWording {
                code: code.clone(),
                label,
                hint,
            });
        }
    }

    WordingTables {
        stages,
        errors,
        error_codes,
    }
});

fn match_at(pat: &[u8], s: &[u8], i: usize, caps: &mut HashMap<String, String>) -> bool {
    if pat.is_empty() {
        return i == s.len();
    }
    if pat[0] == b'*' {
        for end in (i..=s.len()).rev() {
            let mut trial = caps.clone();
            if let Ok(val) = std::str::from_utf8(&s[i..end]) {
                trial.entry("tool".to_string()).or_insert_with(|| val.to_string());
                if match_at(&pat[1..], s, end, &mut trial) {
                    *caps = trial;
                    return true;
                }
            }
        }
        return false;
    }
    if pat[0] == b'{' {
        if let Some(close_rel) = pat.iter().position(|&b| b == b'}') {
            let name = std::str::from_utf8(&pat[1..close_rel]).unwrap_or("");
            let is_digits = name == "n";
            let mut end = i;
            while end < s.len() {
                let ok = if is_digits {
                    s[end].is_ascii_digit()
                } else {
                    s[end] != b':'
                };
                if !ok {
                    break;
                }
                end += 1;
            }
            for try_end in (i + 1..=end).rev() {
                if let Ok(val) = std::str::from_utf8(&s[i..try_end]) {
                    let mut trial = caps.clone();
                    trial.insert(name.to_string(), val.to_string());
                    if match_at(&pat[close_rel + 1..], s, try_end, &mut trial) {
                        *caps = trial;
                        return true;
                    }
                }
            }
            return false;
        }
    }
    i < s.len() && s[i] == pat[0] && match_at(&pat[1..], s, i + 1, caps)
}

fn expand_label(label: &str, caps: &HashMap<String, String>, raw: &str) -> String {
    let mut out = String::with_capacity(label.len() + 16);
    let mut rest = label;
    while !rest.is_empty() {
        if rest.starts_with('{') {
            if let Some(close) = rest.find('}') {
                let key = &rest[1..close];
                if key == "raw" {
                    out.push_str(raw);
                    rest = &rest[close + 1..];
                    continue;
                }
                if let Some(val) = caps.get(key) {
                    out.push_str(val);
                    rest = &rest[close + 1..];
                    continue;
                }
            }
        }
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    out
}

pub fn error_code_wording(code: &str) -> Option<&'static ErrorCodeWording> {
    TABLES.error_codes.iter().find(|row| row.code == code)
}

pub fn error_code_of(row: &Value) -> String {
    let Some(obj) = row.as_object() else {
        return String::new();
    };
    for key in ["errorCode", "code"] {
        if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
            return s.to_string();
        }
    }
    if let Some(res) = obj.get("result").and_then(|v| v.as_object()) {
        if let Some(s) = res.get("errorCode").and_then(|v| v.as_str()) {
            return s.to_string();
        }
    }
    String::new()
}

#[allow(dead_code)]
pub fn error_code_label(row: &Value) -> String {
    let code = error_code_of(row);
    error_code_wording(&code)
        .map(|w| w.label.clone())
        .unwrap_or_default()
}

#[allow(dead_code)]
pub fn error_code_hint(row: &Value) -> String {
    let code = error_code_of(row);
    error_code_wording(&code)
        .map(|w| w.hint.clone())
        .unwrap_or_default()
}

pub fn stage_label_with_known(raw: &str) -> (String, bool) {
    for rule in &TABLES.stages {
        let mut caps = HashMap::new();
        if !match_at(rule.pattern.as_bytes(), raw.as_bytes(), 0, &mut caps) {
            continue;
        }
        if rule.pattern == "*" && raw.starts_with("worker:") {
            if let Some(colon_rel) = raw[7..].find(':') {
                let colon = 7 + colon_rel;
                if colon > 7 && colon + 1 < raw.len() {
                    let (rest, inner_known) = stage_label_with_known(&raw[colon + 1..]);
                    if inner_known {
                        return (format!("作業担当{} · {}", &raw[7..colon], rest), true);
                    }
                }
            }
        }
        let known = rule.pattern != "*";
        return (expand_label(&rule.label, &caps, raw), known);
    }
    (raw.to_string(), false)
}

pub fn stage_label(raw: &str) -> String {
    stage_label_with_known(raw).0
}

pub fn stage_log_line(raw: &str) -> String {
    let (label, known) = stage_label_with_known(raw);
    if known {
        format!("{}  ({})", label, raw)
    } else {
        label
    }
}

fn trim_ascii_whitespace(text: &str) -> &str {
    text.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\r' | '\n'))
}

fn clip_1500_chars(text: &str) -> String {
    let count = text.chars().count();
    if count <= 1500 {
        return text.to_string();
    }
    let mut clipped: String = text.chars().take(1500).collect();
    clipped.push('…');
    clipped
}

pub fn error_notice(raw_text: &str) -> ErrorNotice {
    let text = trim_ascii_whitespace(raw_text);
    let clip = clip_1500_chars(text);
    for rule in &TABLES.errors {
        if text.contains(&rule.contains) {
            return ErrorNotice {
                label: rule.label.clone(),
                hint: rule.hint.clone(),
                retry: rule.retry,
                raw: clip,
                known: true,
            };
        }
    }
    ErrorNotice {
        label: UNKNOWN_ERROR_LABEL.to_string(),
        hint: UNKNOWN_ERROR_HINT.to_string(),
        retry: false,
        raw: clip,
        known: false,
    }
}

pub fn plain_notice(text: &str) -> ErrorNotice {
    ErrorNotice {
        label: text.to_string(),
        hint: String::new(),
        retry: false,
        raw: String::new(),
        known: true,
    }
}

pub fn input_notice(text: &str) -> ErrorNotice {
    let notice = error_notice(text);
    if notice.known {
        return notice;
    }
    if text.bytes().any(|b| b >= 0x80) {
        return ErrorNotice {
            label: trim_ascii_whitespace(text).to_string(),
            hint: String::new(),
            retry: false,
            raw: String::new(),
            known: true,
        };
    }
    notice
}

#[allow(dead_code)]
pub fn resend_hint(hint: &str) -> String {
    hint.replace("「続ける」", &format!("「{}」", RESEND_LABEL))
}

pub fn job_end_notice(
    status: &str,
    achieved: i32,
    error: &str,
    stop_reason: &str,
    detail: &str,
) -> Option<ErrorNotice> {
    if status == "cancelled" {
        let mut notice = error_notice(error);
        if !notice.known {
            notice = ErrorNotice {
                label: stage_label("cancelled"),
                hint: "続ける場合は「続ける」を押してください".to_string(),
                retry: true,
                raw: if error.is_empty() {
                    String::new()
                } else {
                    notice.raw
                },
                known: true,
            };
        }
        notice.retry = true;
        return Some(notice);
    }
    if status == "failed" || status == "interrupted" {
        let mut notice = error_notice(error);
        if error.is_empty() {
            notice = ErrorNotice {
                label: if status == "failed" {
                    "処理が失敗しました".to_string()
                } else {
                    stage_label(status)
                },
                hint: CONTINUE_HINT.to_string(),
                retry: true,
                raw: String::new(),
                known: true,
            };
        } else if !notice.known {
            notice.retry = true;
        }
        return Some(notice);
    }
    if status == "completed"
        && stop_reason == "error"
        && achieved != 1
        && !trim_ascii_whitespace(detail).is_empty()
    {
        let mut notice = error_notice(detail);
        if !notice.known {
            notice.retry = true;
        }
        return Some(notice);
    }
    if status == "completed" && achieved == 0 {
        return Some(ErrorNotice {
            label: "目標を達成できませんでした".to_string(),
            hint: CONTINUE_HINT.to_string(),
            retry: true,
            raw: String::new(),
            known: true,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stage_labels_match_contract() {
        assert_eq!(stage_label("queued"), "順番待ち");
        assert_eq!(stage_label("waiting_user"), "回答待ち");
        assert_eq!(stage_label("working:T1"), "作業中（T1）");
        assert_eq!(stage_label("working:T2:coder"), "作業中（T2）");
        assert_eq!(stage_label("repair:2:T1"), "修正（2回目）");
        assert_eq!(stage_label("worker:2:agent:3"), "作業担当2 · ステップ3");
        assert_eq!(
            stage_label("worker:1:tool:local.read"),
            "作業担当1 · PCのファイルを確認中"
        );
        assert_eq!(
            stage_log_line("agent:4"),
            "計画 ステップ4  (agent:4)"
        );
        assert_eq!(stage_label("chat:reply"), "返信中");
        assert_eq!(stage_label("custom:unknown"), "custom:unknown");
    }

    #[test]
    fn error_wording_and_invalid_session() {
        let n = error_notice("PC操作セッションが無効です: closed");
        assert!(n.known);
        assert_eq!(n.label, "PC操作セッションが切れました");
        assert_eq!(n.hint, "PC連携待機を再接続するか、作業環境からやり直してください");
        assert!(n.retry);

        let unk = error_notice("Something exploded");
        assert!(!unk.known);
        assert_eq!(unk.label, UNKNOWN_ERROR_LABEL);

        let jp = input_notice("独自のエラーです");
        assert!(jp.known);
        assert_eq!(jp.label, "独自のエラーです");
        assert_eq!(
            resend_hint("続ける場合は「続ける」を押してください"),
            "続ける場合は「もう一度送る」を押してください"
        );
    }

    #[test]
    fn error_codes_and_job_end_notice() {
        let row = json!({"result": {"errorCode": "RESUME_STALLED"}});
        assert_eq!(error_code_of(&row), "RESUME_STALLED");
        assert!(!error_code_label(&row).is_empty());
        assert!(!error_code_hint(&row).is_empty());

        let end = job_end_notice("completed", 0, "", "", "").unwrap();
        assert_eq!(end.label, "目標を達成できませんでした");
        assert!(end.retry);
        assert!(job_end_notice("completed", 1, "", "", "").is_none());
    }
}
