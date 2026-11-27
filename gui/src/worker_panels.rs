use crate::live_status::iso_ms;
use crate::wording::{error_code_of, error_code_wording, stage_label_with_known};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn task_phase_label(phase: &str) -> &'static str {
    match phase {
        "implement" => "実装",
        "integrate" => "結合",
        "verify" => "統合検証",
        "review" => "レビュー",
        "investigate" => "調査",
        "contract" => "契約",
        _ => "",
    }
}

pub fn name_list_label(names: &Value) -> String {
    let Some(arr) = names.as_array() else {
        return String::new();
    };
    let mut out = String::new();
    let mut shown = 0usize;
    let mut total = 0usize;
    for name in arr {
        let Some(s) = name.as_str() else {
            continue;
        };
        total += 1;
        if total > 3 {
            continue;
        }
        if shown > 0 {
            out.push_str(", ");
        }
        shown += 1;
        out.push_str(s);
    }
    if total > 3 {
        out.push_str(&format!(" ほか{}件", total - 3));
    }
    out
}

pub fn task_own_label(task: &Value) -> String {
    let text = task.get("files").map(name_list_label).unwrap_or_default();
    if text.is_empty() {
        String::new()
    } else {
        format!("担当: {}", text)
    }
}

pub fn task_uses_label(task: &Value) -> String {
    let text = task.get("uses").map(name_list_label).unwrap_or_default();
    if text.is_empty() {
        String::new()
    } else {
        format!("参照: {}", text)
    }
}

pub fn text_field<'a>(obj: &'a Value, key: &str, fallback: &'a str) -> &'a str {
    obj.get(key).and_then(|v| v.as_str()).unwrap_or(fallback)
}

pub fn flag_field(obj: &Value, key: &str) -> bool {
    obj.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

pub fn task_progress_label(batches: &Value, job_status: &str) -> String {
    let mut tasks: Option<&Vec<Value>> = None;
    if let Some(arr) = batches.as_array() {
        for batch in arr {
            if let Some(wt) = batch.get("workerTasks").and_then(|v| v.as_array()) {
                if !wt.is_empty() {
                    tasks = Some(wt);
                }
            }
        }
    }
    let Some(tasks) = tasks else {
        return "27Bが設計・分割中".to_string();
    };
    let mut complete = 0usize;
    let mut failed = 0usize;
    let mut undetermined = 0usize;
    let mut active = String::new();
    for task in tasks {
        match text_field(task, "status", "") {
            "completed" => complete += 1,
            "failed" => failed += 1,
            "undetermined" => undetermined += 1,
            "running" => active = text_field(task, "title", "").to_string(),
            _ => {}
        }
    }
    let mut text = format!("{} / {}件完了", complete, tasks.len());
    if failed > 0 {
        text.push_str(&format!(" · {}件失敗", failed));
    }
    if undetermined > 0 {
        text.push_str(&format!(" · {}件判断なし", undetermined));
    }
    if job_status == "cancelled" || job_status == "interrupted" {
        text.push_str(" · 中断");
    } else if job_status == "waiting_user" {
        text.push_str(" · 回答待ち");
    } else if job_status == "failed" {
        text.push_str(" · 停止");
    } else if complete == tasks.len() {
        text.push_str(if job_status == "running" {
            " · 27Bが最終確認中"
        } else {
            " · 作業終了"
        });
    } else if job_status == "completed" {
        text.push_str(" · 未完了の作業あり");
    } else if !active.is_empty() {
        text.push_str(&format!(" · 現在: {}", active));
    }
    text
}

pub fn execution_stage_label(job: &Value) -> String {
    let stage = text_field(job, "stage", "");
    if stage.contains("planner:advice") {
        return "27Bが8Bの相談に回答中".to_string();
    }
    let batches = job
        .get("activity")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    if stage.starts_with("worker:") {
        let mut label = String::new();
        if let Some(batch_arr) = batches.as_array() {
            for batch in batch_arr {
                if let Some(tasks) = batch.get("workerTasks").and_then(|v| v.as_array()) {
                    for task in tasks {
                        if text_field(task, "status", "") == "running" {
                            label = format!(
                                "8B · {} · {}",
                                task_phase_label(text_field(task, "phase", "")),
                                text_field(task, "title", "")
                            );
                        }
                    }
                }
            }
        }
        if !label.is_empty() {
            return format!(
                "{}  |  {}",
                task_progress_label(&batches, text_field(job, "status", "")),
                label
            );
        }
        if let Some(colon_rel) = stage[7..].find(':') {
            let rest = &stage[7 + colon_rel + 1..];
            if rest.starts_with("tool:") {
                let (what, known) = stage_label_with_known(stage);
                if known {
                    return what;
                }
            }
        }
        return "8Bが担当タスクを実行・確認中".to_string();
    }
    if stage.starts_with("agent:") {
        let opts = job.get("options");
        if opts.and_then(|o| o.get("modelRole")).and_then(|v| v.as_str()) == Some("worker") {
            return "8Bが実行結果を確認中".to_string();
        }
        let delegated = batches
            .as_array()
            .map(|arr| {
                arr.iter().any(|b| {
                    b.get("workerTasks")
                        .and_then(|t| t.as_array())
                        .map(|t| !t.is_empty())
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        if delegated {
            return "27Bが結果を確認・必要なら再設計中".to_string();
        }
        if opts.and_then(|o| o.get("planMode")).and_then(|v| v.as_bool()) == Some(false) {
            return "27Bが直接作業・確認中（Direct 27B）".to_string();
        }
        return "27Bが設計・実装タスクを分割中".to_string();
    }
    let (label, known) = stage_label_with_known(stage);
    if known {
        label
    } else {
        "依頼を進めています…".to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerPanel {
    pub key: String,
    pub role: String,
    pub model: String,
    pub adapter: String,
    pub status: String,
    pub tasks: Vec<Value>,
    pub name: String,
    pub title: String,
}

pub fn task_display_state(state: &str, job_status: &str) -> String {
    if state != "running" && state != "queued" {
        return state.to_string();
    }
    if job_status == "waiting_user" && state == "running" {
        return "waiting_user".to_string();
    }
    if matches!(job_status, "cancelled" | "interrupted" | "failed") {
        return job_status.to_string();
    }
    if job_status == "completed" {
        return "not_run".to_string();
    }
    state.to_string()
}

pub fn state_label(state: &str) -> &'static str {
    match state {
        "running" => "実行中",
        "queued" => "待機",
        "completed" => "完了",
        "failed" => "失敗",
        "undetermined" => "判断なし",
        "waiting_user" => "回答待ち",
        "cancelled" => "中断",
        "interrupted" => "停止",
        "not_run" => "未完了",
        _ => "状態不明",
    }
}

pub fn state_mark(state: &str) -> &'static str {
    match state {
        "completed" => "✓ ",
        "failed" => "✕ ",
        "undetermined" => "△ ",
        "running" => "● ",
        "waiting_user" => "？ ",
        _ => "",
    }
}

pub fn duration_label(seconds: i64) -> String {
    if seconds < 0 {
        String::new()
    } else if seconds < 60 {
        format!("{}秒", seconds)
    } else if seconds < 3600 {
        let rem = seconds % 60;
        if rem > 0 {
            format!("{}分{}秒", seconds / 60, rem)
        } else {
            format!("{}分", seconds / 60)
        }
    } else {
        format!("{}時間{}分", seconds / 3600, (seconds % 3600) / 60)
    }
}

pub fn writes_file(tool: &str) -> bool {
    if !tool.starts_with("local.") && !tool.starts_with("workspace.") {
        return false;
    }
    ["write", "create", "replace", "append", "insert", "edit"]
        .iter()
        .any(|verb| tool.contains(verb))
}

pub fn step_label(row: &Value) -> String {
    let tool = text_field(row, "tool", "");
    if tool == "workspace.check" {
        return "27Bの case で照合".to_string();
    }
    if tool == "task.dispute" {
        let id = text_field(row, "case", "");
        return if id.is_empty() {
            "照合の判定を27Bに依頼".to_string()
        } else {
            format!("照合の判定を27Bに依頼: {}", id)
        };
    }
    let local = tool.starts_with("local.");
    let server = tool.starts_with("workspace.");
    if tool.ends_with(".expect") {
        let count = row
            .get("cases")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let prefix = if local {
            "PCの"
        } else if server {
            "サーバーの"
        } else {
            ""
        };
        let suffix = if count > 0 {
            format!(": case {}件", count)
        } else {
            String::new()
        };
        return format!("{}照合{}", prefix, suffix);
    }
    let what = if tool == "planner.ask" {
        "27Bに相談"
    } else if tool == "user.ask" {
        "ユーザーに質問"
    } else if tool.contains(".run") {
        "コマンド"
    } else if writes_file(tool) {
        if tool.contains("write") || tool.contains("create") {
            "ファイル作成"
        } else {
            "ファイル編集"
        }
    } else if tool.contains(".read") {
        "ファイル確認"
    } else if tool.contains(".list") {
        "フォルダー確認"
    } else if tool.contains(".search") {
        "検索"
    } else {
        tool
    };
    let prefix = if local {
        "PCの"
    } else if server {
        "サーバーの"
    } else {
        ""
    };
    let mut detail = text_field(row, "command", "");
    if detail.is_empty() {
        detail = text_field(
            row,
            if tool == "planner.ask" || tool == "user.ask" {
                "query"
            } else {
                "path"
            },
            "",
        );
    }
    let clean: String = detail
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let short = if clean.chars().count() > 60 {
        let mut s: String = clean.chars().take(60).collect();
        s.push('…');
        s
    } else {
        clean
    };
    if short.is_empty() {
        format!("{}{}", prefix, what)
    } else {
        format!("{}{}: {}", prefix, what, short)
    }
}

#[derive(Clone, Debug, Default)]
pub struct TaskSteps {
    pub steps: usize,
    pub seconds: i64,
    pub now: String,
    pub written: Vec<String>,
}

pub fn task_steps(task: &Value) -> TaskSteps {
    let mut out = TaskSteps {
        steps: 0,
        seconds: -1,
        now: String::new(),
        written: Vec::new(),
    };
    let Some(rows) = task.get("activity").and_then(|v| v.as_array()) else {
        return out;
    };
    let mut first: i64 = -1;
    let mut last: i64 = -1;
    for row in rows {
        let tool = text_field(row, "tool", "");
        if tool.is_empty()
            || tool == "thinking"
            || tool == "finish"
            || tool.starts_with("history.")
        {
            continue;
        }
        out.steps += 1;
        let start = iso_ms(text_field(row, "startedAt", "")) / 1000;
        let end = iso_ms(text_field(row, "finishedAt", "")) / 1000;
        if start > 0 && (first < 0 || start < first) {
            first = start;
        }
        last = last.max(start).max(end);
        if text_field(row, "status", "") == "running" {
            out.now = step_label(row);
        }
        if writes_file(tool) && text_field(row, "status", "") != "failed" {
            let path = text_field(row, "path", "");
            if !path.is_empty() && !out.written.iter().any(|w| w == path) {
                out.written.push(path.to_string());
            }
        }
    }
    if first >= 0 && last >= first {
        out.seconds = last - first;
    }
    out
}

pub fn is_refused_resume(row: &Value) -> bool {
    let tool = text_field(row, "tool", "");
    if tool != "worker.delegate" && tool != "worker.session" {
        return false;
    }
    text_field(row, "operation", "") == "worker.resume"
        && error_code_wording(&error_code_of(row)).is_some()
        && row.get("workerTasks").is_none()
        && row.get("workerActivity").is_none()
        && row.get("resumedFromStep").is_none()
}

pub fn batch_origin(batch: &Value) -> i64 {
    batch
        .get("resumedFromStep")
        .or_else(|| batch.get("step"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
}

pub fn current_worker_batches(rows: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut positions: HashMap<i64, usize> = HashMap::new();
    for row in rows {
        let tool = text_field(row, "tool", "");
        if (tool != "worker.delegate" && tool != "worker.session") || is_refused_resume(row) {
            continue;
        }
        let origin = batch_origin(row);
        if let Some(&pos) = positions.get(&origin) {
            out[pos] = row.clone();
        } else {
            positions.insert(origin, out.len());
            out.push(row.clone());
        }
    }
    out
}

pub fn worker_role(task: &Value) -> String {
    let role = text_field(task, "role", "");
    if !role.is_empty() {
        return role.to_string();
    }
    let title = text_field(task, "title", "");
    if title.contains("テスト") || title.contains("検証") {
        "tester".to_string()
    } else if title.contains("レビュー") {
        "reviewer".to_string()
    } else if title.contains("実装") || title.contains("作成") || title.contains("コンパイル") {
        "coder".to_string()
    } else {
        "worker".to_string()
    }
}

pub fn planner_activity(rows: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for row in rows {
        let mut clean = row.clone();
        let children = clean
            .get("workerActivity")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if let Some(obj) = clean.as_object_mut() {
            obj.remove("workerActivity");
            obj.remove("workerAnswer");
        }
        let batch_step = clean.get("step").and_then(|v| v.as_i64()).unwrap_or(0);
        out.push(clean);
        for child in &children {
            let mut reasoning = text_field(child, "advisorReasoning", "").to_string();
            if reasoning.is_empty()
                && !flag_field(child, "advisorReasoningSeparate")
                && text_field(child, "advisorModel", "") == "Qwen27"
            {
                reasoning = text_field(child, "modelReasoning", "").to_string();
            }
            let answer = text_field(child, "plannerAnswer", "");
            if reasoning.is_empty() && answer.is_empty() {
                continue;
            }
            let child_step = child.get("step").and_then(|v| v.as_i64()).unwrap_or(0);
            let task_idx = child.get("taskIndex").and_then(|v| v.as_i64()).unwrap_or(0);
            let chars = child
                .get("advisorReasoningCharacters")
                .or_else(|| child.get("reasoningCharacters"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            out.push(json!({
                "step": child_step,
                "displayKey": format!("advice/{}/{}/{}", batch_step, task_idx, child_step),
                "tool": "thinking",
                "reasoningLabel": "27Bの相談対応",
                "modelReasoning": reasoning,
                "reasoningCharacters": chars,
                "thoughtSummary": answer,
                "status": text_field(child, "status", "")
            }));
        }
    }
    out
}

pub fn collect_worker_panels(
    panels: &mut Vec<WorkerPanel>,
    job_id: &str,
    rows: &[Value],
    job_status: &str,
) {
    for batch in current_worker_batches(rows) {
        let tool = text_field(&batch, "tool", "");
        if tool != "worker.delegate" && tool != "worker.session" {
            continue;
        }
        let mut tasks = batch
            .get("workerTasks")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if tasks.is_empty() && batch.get("workerActivity").is_some() {
            tasks.push(json!({
                "index": 1,
                "title": "実作業",
                "status": text_field(&batch, "status", "")
            }));
        }
        let origin = batch_origin(&batch);
        let default_model = text_field(&batch, "workerModel", "8B");
        for task in &tasks {
            let index = task.get("index").and_then(|v| v.as_i64()).unwrap_or(1) - 1;
            let role = worker_role(task);
            let model = text_field(task, "model", default_model).to_string();
            let adapter = text_field(task, "adapter", "不明（旧履歴）").to_string();
            let task_id = text_field(task, "taskId", "");
            let key = json!([
                job_id,
                origin,
                if task_id.is_empty() {
                    format!("index:{}", index)
                } else {
                    format!("task:{}", task_id)
                }
            ])
            .to_string();

            let pos = if let Some(p) = panels.iter().position(|p| p.key == key) {
                p
            } else {
                panels.push(WorkerPanel {
                    key: key.clone(),
                    role: role.clone(),
                    model: model.clone(),
                    adapter: adapter.clone(),
                    status: String::new(),
                    tasks: Vec::new(),
                    name: String::new(),
                    title: String::new(),
                });
                panels.len() - 1
            };

            let panel = &mut panels[pos];
            panel.role = role;
            panel.model = model;
            panel.adapter = adapter;
            panel.name = text_field(task, "assignee", "").to_string();
            panel.title = text_field(task, "title", "実作業").to_string();

            let mut entry = task.clone();
            if let Some(obj) = entry.as_object_mut() {
                if flag_field(&batch, "workerActivityOmitted") {
                    obj.insert("activityOmitted".into(), Value::Bool(true));
                    obj.insert(
                        "activityOmittedState".into(),
                        Value::String(text_field(&batch, "workerActivityOmittedState", "").into()),
                    );
                }
                if flag_field(&batch, "workerActivityTrimmed") {
                    obj.insert("activityTrimmed".into(), Value::Bool(true));
                    obj.insert(
                        "activityTotalCount".into(),
                        json!(batch
                            .get("workerActivityTotalCount")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0)),
                    );
                    obj.insert(
                        "activityOmittedState".into(),
                        Value::String(text_field(&batch, "workerActivityOmittedState", "").into()),
                    );
                }
                let st_disp = task_display_state(text_field(task, "status", ""), job_status);
                obj.insert("jobId".into(), Value::String(job_id.to_string()));
                obj.insert("status".into(), Value::String(st_disp.clone()));
                obj.insert("stateLabel".into(), Value::String(state_label(&st_disp).into()));
                obj.insert("stateMark".into(), Value::String(state_mark(&st_disp).into()));
                let own_lbl = task_own_label(task);
                if !own_lbl.is_empty() {
                    obj.insert("ownLabel".into(), Value::String(own_lbl));
                }
                let uses_lbl = task_uses_label(task);
                if !uses_lbl.is_empty() {
                    obj.insert("usesLabel".into(), Value::String(uses_lbl));
                }
                let id_str = format!(
                    "{}/{}/{}",
                    job_id,
                    origin,
                    if task_id.is_empty() {
                        index.to_string()
                    } else {
                        format!("task:{}", task_id)
                    }
                );
                obj.insert("id".into(), Value::String(id_str.clone()));
                let mut child_acts = Vec::new();
                if let Some(children) = batch.get("workerActivity").and_then(|v| v.as_array()) {
                    for child in children {
                        if child.get("taskIndex").and_then(|v| v.as_i64()).unwrap_or(0) == index {
                            let mut c = child.clone();
                            if let Some(co) = c.as_object_mut() {
                                if !co.contains_key("advisorReasoning")
                                    && !flag_field(child, "advisorReasoningSeparate")
                                    && text_field(child, "advisorModel", "") == "Qwen27"
                                {
                                    co.remove("modelReasoning");
                                }
                                co.remove("advisorReasoning");
                                co.remove("advisorReasoningCharacters");
                            }
                            child_acts.push(c);
                        }
                    }
                }
                obj.insert("activity".into(), Value::Array(child_acts));
            }
            let ts = task_steps(&entry);
            let step_str = if !ts.now.is_empty() {
                ts.now
            } else {
                let dur = duration_label(ts.seconds);
                if ts.steps > 0 && !dur.is_empty() {
                    format!("{}ステップ ({})", ts.steps, dur)
                } else if ts.steps > 0 {
                    format!("{}ステップ", ts.steps)
                } else {
                    String::new()
                }
            };
            if !step_str.is_empty() {
                if let Some(obj) = entry.as_object_mut() {
                    obj.insert("stepLabel".into(), Value::String(step_str));
                }
            }
            let entry_id = text_field(&entry, "id", "").to_string();
            if let Some(existing) = panel
                .tasks
                .iter_mut()
                .find(|t| text_field(t, "id", "") == entry_id)
            {
                *existing = entry;
            } else {
                panel.tasks.push(entry);
            }
        }
    }

    for panel in panels.iter_mut() {
        panel.status = "completed".to_string();
        let latest = panel
            .tasks
            .last()
            .map(|t| text_field(t, "jobId", "").to_string())
            .unwrap_or_default();
        let mut rank = -1;
        for task in &panel.tasks {
            if text_field(task, "jobId", "") == latest {
                let st = text_field(task, "status", "");
                let priority = match st {
                    "running" => 6,
                    "waiting_user" => 5,
                    "queued" => 4,
                    "failed" | "undetermined" => 3,
                    "cancelled" | "interrupted" => 2,
                    "not_run" => 1,
                    _ => 0,
                };
                if priority >= rank {
                    rank = priority;
                    panel.status = st.to_string();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn worker_panels_collects_and_separates_advice() {
        let rows = vec![json!({
            "step": 3,
            "tool": "worker.delegate",
            "workerModel": "Qwen-8B",
            "workerTasks": [{
                "index": 1,
                "taskId": "T1",
                "title": "UI 実装",
                "phase": "implement",
                "status": "running",
                "files": ["a.rs", "b.rs", "c.rs", "d.rs"]
            }],
            "workerActivity": [{
                "taskIndex": 0,
                "step": 1,
                "tool": "planner.ask",
                "advisorReasoning": "internal thought",
                "plannerAnswer": "proceed",
                "status": "completed"
            }]
        })];

        let plan_rows = planner_activity(&rows);
        assert_eq!(plan_rows.len(), 2);
        assert_eq!(text_field(&plan_rows[1], "reasoningLabel", ""), "27Bの相談対応");

        let mut panels = Vec::new();
        collect_worker_panels(&mut panels, "job-1", &rows, "running");
        assert_eq!(panels.len(), 1);
        assert_eq!(panels[0].status, "running");
        assert_eq!(task_own_label(&rows[0]["workerTasks"][0]), "担当: a.rs, b.rs, c.rs ほか1件");
    }
}
