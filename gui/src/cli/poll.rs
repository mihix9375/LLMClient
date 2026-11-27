use crate::cli::stream::{poll_interval_ms, StreamPrinter};
use crate::cli::term;
use crate::tls_http::ApiClient;
use serde_json::{json, Value};
use std::io::{self, Write};
use std::thread;
use std::time::Duration;

pub enum PollOutcome {
    Done(Value),
    WaitingUser(Value),
    Cancelled(Value),
}

pub fn poll_job(
    api: &ApiClient,
    job_id: &str,
    work_mode: &str,
    printer: &mut StreamPrinter,
) -> Result<PollOutcome, String> {
    struct ActiveGuard;
    impl Drop for ActiveGuard {
        fn drop(&mut self) {
            term::set_job_active(false);
        }
    }
    term::set_job_active(true);
    let _guard = ActiveGuard;

    let mut sent_cancel = false;
    let mut prev_ans_rev = -1i64;
    let mut last_job = json!({ "id": job_id, "status": "cancelled" });
    let mut out_w = io::stdout();
    let mut err_w = io::stderr();

    loop {
        if term::is_cancel_job() && !sent_cancel {
            sent_cancel = true;
            printer.clear_status(&mut err_w);
            let _ = writeln!(err_w, "\n中断を要求しています…");
            let _ = err_w.flush();
            let _ = api.post(&format!("/jobs/{}/cancel", job_id), &json!({}), 5000);
        }
        if term::is_force_abort() {
            api.cancel_io();
            printer.clear_status(&mut err_w);
            return Ok(PollOutcome::Cancelled(last_job));
        }

        let job = api.get(&format!("/jobs/{}", job_id), 15000)?;
        last_job = job.clone();
        let status = job.get("status").and_then(|v| v.as_str()).unwrap_or("");
        printer.update_activity(&job, &mut err_w);

        let ans_rev = job.get("partialAnswerRevision").and_then(|v| v.as_i64()).unwrap_or(0);
        let mut streaming_active = false;
        if ans_rev != prev_ans_rev {
            prev_ans_rev = ans_rev;
            if let Some(pa) = job.get("partialAnswer").and_then(|v| v.as_str()) {
                if !pa.is_empty() {
                    streaming_active = true;
                    printer.push_partial(pa, &mut out_w, &mut err_w);
                }
            }
        }
        if status == "running" || status == "queued" {
            printer.update_stage(&job, &mut err_w);
            thread::sleep(Duration::from_millis(poll_interval_ms(work_mode, streaming_active)));
            continue;
        }
        printer.clear_status(&mut err_w);
        return Ok(match status {
            "waiting_user" => PollOutcome::WaitingUser(job),
            "cancelled" | "interrupted" => PollOutcome::Cancelled(job),
            _ => PollOutcome::Done(job),
        });
    }
}
