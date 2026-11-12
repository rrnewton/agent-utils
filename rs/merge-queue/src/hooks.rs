//! Event hooks (5891): configurable commands the queue fires, push-based,
//! when something happens — a red verdict, an ejection, a green-flaky
//! hold, a landing, a base move, an intake refusal, an escalation.
//!
//! Each hook is a shell command (like the precheck and peripheral hooks)
//! called with a versioned JSON payload (`merge-queue-hook/v1`) on stdin
//! and `$MQ_HOOK_EVENT` / `$MQ_HOOK_PR` in its environment. Hooks are
//! fire-and-forget: a bounded worker pool runs them, each with a timeout,
//! and the queue never waits on a hook or fails because of one. Every
//! finished hook is logged to `<state>/hooks.jsonl` with its exit status,
//! duration and stdout/stderr tails. By default (`notify_mode: both`) the
//! existing `--notify` delivery is unchanged beside them; `hooks-only`
//! suppresses that delivery for a verdict whose hook is configured, and
//! `notify-only` suppresses a hook whose verdict notify will deliver
//! (see [`NotifyMode`]). A hook is how a team reacts programmatically
//! (page a shepherd, post to a channel) with low latency, including when
//! the notified agent is gone or in a long turn.
//!
//! Payload shape (fields that do not apply to an event are empty):
//!
//! ```json
//! {"schema":"merge-queue-hook/v1","event":"on_land","at":"...","pr":1,
//!  "prs":[1],"head":"<sha>","shepherd":"alice","notify":"alice",
//!  "landed_sha":"<sha>","old_base":"<sha>","new_base":"<sha>",
//!  "failing":["tests/a"],"logs":[".../run.log"],"culprit":"tests/a",
//!  "reason":"...","detail":"..."}
//! ```

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::model::{now, NotifyMode};
use crate::state::HooksConfig;

/// Payload schema.
pub const HOOK_SCHEMA: &str = "merge-queue-hook/v1";
/// Log schema (one line per finished hook in `<state>/hooks.jsonl`).
pub const HOOK_LOG_SCHEMA: &str = "merge-queue-hook-log/v1";
/// How many hook jobs may wait for a worker; more are dropped and logged.
const QUEUE_BOUND: usize = 64;
/// Workers running hooks at once (also bounded by `hooks.max_concurrent`,
/// read when the pool starts).
const WORKERS: usize = 2;
/// Bytes of stdout/stderr tail kept in the log.
const TAIL_BYTES: usize = 2048;

struct Job {
    state: PathBuf,
    event: String,
    command: String,
    payload: String,
    pr: i64,
    timeout: Duration,
}

fn sender() -> Option<SyncSender<Job>> {
    static POOL: OnceLock<Option<SyncSender<Job>>> = OnceLock::new();
    POOL.get_or_init(|| {
        let (tx, rx) = sync_channel::<Job>(QUEUE_BOUND);
        let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
        for _ in 0..WORKERS {
            let rx = std::sync::Arc::clone(&rx);
            std::thread::spawn(move || loop {
                let job = {
                    let Ok(receiver) = rx.lock() else { return };
                    receiver.recv()
                };
                match job {
                    Ok(job) => run_one(job),
                    Err(_) => return,
                }
            });
        }
        Some(tx)
    })
    .clone()
}

/// The hook event that reports the same state change as the notify
/// verdict `verdict`, when one exists. Verdicts with no counterpart
/// (`withdrawn`, `refusal-deferred`, held verdicts other than
/// `held-green-flaky`) are notify-only in the sense that no hook can
/// take their place.
pub fn hook_for_verdict(verdict: &str) -> Option<&'static str> {
    match verdict {
        "landed" => Some("on_land"),
        "ejected" => Some("on_eject"),
        "escalated" => Some("on_escalated"),
        "held-green-flaky" => Some("on_held"),
        _ => None,
    }
}

/// The notify verdict that reports the same state change as hook
/// `event`, when one exists. Events with no counterpart (`on_red`,
/// `on_base_moved`, `on_intake_refused`) are hook-only in the same sense.
pub fn verdict_for_hook(event: &str) -> Option<&'static str> {
    match event {
        "on_land" => Some("landed"),
        "on_eject" => Some("ejected"),
        "on_escalated" => Some("escalated"),
        "on_held" => Some("held-green-flaky"),
        _ => None,
    }
}

/// In `hooks-only` mode, the hook that makes notify delivery of
/// `verdict` redundant, when that hook is actually configured. `None`
/// means notify must still fire (the mode is not `hooks-only`, the
/// verdict has no hook counterpart, or no command is configured for
/// it) so the change is never silently dropped.
pub fn notify_suppressed_by(
    mode: NotifyMode,
    verdict: &str,
    hooks: &HooksConfig,
) -> Option<&'static str> {
    if mode != NotifyMode::HooksOnly {
        return None;
    }
    let event = hook_for_verdict(verdict)?;
    hooks.command_for(event).is_some().then_some(event)
}

/// In `notify-only` mode, whether hook `event` is redundant for an item
/// whose notify target is `notify`: only when notify reports the same
/// state change and a target will actually receive it. Events notify
/// never reports, and items with no notify target, keep their hook.
pub fn hook_suppressed_by_notify(mode: NotifyMode, event: &str, notify: &str) -> bool {
    mode == NotifyMode::NotifyOnly && verdict_for_hook(event).is_some() && !notify.is_empty()
}

/// Fire one event hook, if `config` names a command for it. Never blocks
/// meaningfully and never fails the caller: a full queue drops the hook
/// and says so in the log.
pub fn fire(state: &Path, config: &HooksConfig, event: &str, mut payload: Value) {
    let Some(command) = config.command_for(event) else {
        return;
    };
    payload["schema"] = json!(HOOK_SCHEMA);
    payload["event"] = json!(event);
    if payload.get("at").is_none() {
        payload["at"] = json!(now());
    }
    let pr = payload["pr"].as_i64().unwrap_or_default();
    let job = Job {
        state: state.to_path_buf(),
        event: event.to_owned(),
        command: command.to_owned(),
        payload: payload.to_string(),
        pr,
        timeout: Duration::from_secs(if config.timeout_secs == 0 {
            30
        } else {
            config.timeout_secs
        }),
    };
    if let Some(tx) = sender() {
        match tx.try_send(job) {
            Ok(()) => {}
            Err(TrySendError::Full(job)) => {
                log_line(
                    &job.state,
                    &json!({
                        "schema": HOOK_LOG_SCHEMA, "event": job.event, "pr": job.pr,
                        "at": now(), "exit": "dropped-queue-full", "elapsed_ms": 0,
                        "stdout_tail": "", "stderr_tail": "",
                    }),
                );
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

fn tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(TAIL_BYTES);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

fn run_one(job: Job) {
    let started = Instant::now();
    let out_dir = job.state.join("hooks-out");
    let _ = std::fs::create_dir_all(&out_dir);
    let stem = format!("{}-{}-{}", job.event, job.pr, std::process::id());
    let out_path = out_dir.join(format!("{stem}.out"));
    let err_path = out_dir.join(format!("{stem}.err"));
    let (exit, mut stdout, mut stderr) = match spawn(&job, &out_path, &err_path) {
        Ok(mut child) => {
            let code = loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        break status.code().map_or("signal".to_owned(), |c| c.to_string())
                    }
                    Ok(None) => {
                        if started.elapsed() > job.timeout {
                            let _ = child.kill();
                            let _ = child.wait();
                            break "timeout".to_owned();
                        }
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    Err(error) => break format!("wait-failed: {error}"),
                }
            };
            (
                code,
                std::fs::read(&out_path).unwrap_or_default(),
                std::fs::read(&err_path).unwrap_or_default(),
            )
        }
        Err(error) => (format!("spawn-failed: {error}"), Vec::new(), Vec::new()),
    };
    let _ = std::fs::remove_file(&out_path);
    let _ = std::fs::remove_file(&err_path);
    stdout.drain(..stdout.len().saturating_sub(TAIL_BYTES));
    stderr.drain(..stderr.len().saturating_sub(TAIL_BYTES));
    log_line(
        &job.state,
        &json!({
            "schema": HOOK_LOG_SCHEMA,
            "event": job.event,
            "pr": job.pr,
            "at": now(),
            "exit": exit,
            "elapsed_ms": started.elapsed().as_millis() as u64,
            "stdout_tail": tail(&stdout),
            "stderr_tail": tail(&stderr),
        }),
    );
}

fn spawn(job: &Job, out_path: &Path, err_path: &Path) -> std::io::Result<std::process::Child> {
    let out = std::fs::File::create(out_path)?;
    let err = std::fs::File::create(err_path)?;
    let mut child = Command::new("sh")
        .args(["-c", &job.command])
        .env("MQ_HOOK_EVENT", &job.event)
        .env("MQ_HOOK_PR", job.pr.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(job.payload.as_bytes());
    }
    Ok(child)
}

/// Append one line to `<state>/hooks.jsonl` (best effort: hooks never
/// fail the queue, including its own log).
pub fn log_line(state: &Path, line: &Value) {
    let _ = std::fs::create_dir_all(state);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.join("hooks.jsonl"))
    {
        // One formatted buffer, one write: concurrent hook workers
        // appending at once must never interleave inside a line (#1357).
        let text = format!("{line}\n");
        let _ = file.write_all(text.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_state(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "merge-queue-hooks-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn wait_for(path: &Path, what: &str) -> String {
        for _ in 0..200 {
            if let Ok(text) = std::fs::read_to_string(path) {
                if !text.is_empty() {
                    return text;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("timed out waiting for {what} at {}", path.display());
    }

    fn config(command: &str) -> HooksConfig {
        HooksConfig {
            on_land: command.to_owned(),
            timeout_secs: 2,
            ..HooksConfig::default()
        }
    }

    #[test]
    fn a_hook_gets_the_payload_and_its_exit_is_logged() {
        let state = temp_state("payload");
        let got = state.join("got.json");
        let cfg = config(&format!("cat > {}", got.display()));
        fire(
            &state,
            &cfg,
            "on_land",
            json!({"pr": 7, "shepherd": "alice", "landed_sha": "abc"}),
        );
        let payload: Value = serde_json::from_str(&wait_for(&got, "hook payload")).unwrap();
        assert_eq!(payload["schema"], HOOK_SCHEMA);
        assert_eq!(payload["event"], "on_land");
        assert_eq!(payload["pr"], 7);
        assert_eq!(payload["shepherd"], "alice");
        assert_eq!(payload["landed_sha"], "abc");
        let log = wait_for(&state.join("hooks.jsonl"), "hook log");
        assert!(log.contains("\"exit\":\"0\""), "{log}");
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_failing_hook_is_logged_and_a_slow_one_times_out() {
        let state = temp_state("failing");
        let cfg = config("echo oops >&2; exit 3");
        fire(&state, &cfg, "on_land", json!({"pr": 1}));
        let log = wait_for(&state.join("hooks.jsonl"), "failure log");
        assert!(log.contains("\"exit\":\"3\""), "{log}");
        assert!(log.contains("oops"), "{log}");

        let state = temp_state("slow");
        let cfg = config("sleep 10");
        let started = Instant::now();
        fire(&state, &cfg, "on_land", json!({"pr": 2}));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "fire never waits on the hook"
        );
        let log = wait_for(&state.join("hooks.jsonl"), "timeout log");
        assert!(log.contains("\"exit\":\"timeout\""), "{log}");
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn notify_mode_suppression_needs_a_configured_counterpart() {
        let hooks = config("true");
        // hooks-only + a configured on_land suppresses the landed notify.
        assert_eq!(
            notify_suppressed_by(NotifyMode::HooksOnly, "landed", &hooks),
            Some("on_land")
        );
        // hooks-only with no hook for the verdict still notifies.
        assert_eq!(
            notify_suppressed_by(NotifyMode::HooksOnly, "ejected", &hooks),
            None
        );
        assert_eq!(
            notify_suppressed_by(NotifyMode::HooksOnly, "withdrawn", &hooks),
            None
        );
        // The default and notify-only never suppress notify.
        assert_eq!(
            notify_suppressed_by(NotifyMode::Both, "landed", &hooks),
            None
        );
        assert_eq!(
            notify_suppressed_by(NotifyMode::NotifyOnly, "landed", &hooks),
            None
        );
        // notify-only suppresses only a hook notify will actually deliver.
        assert!(hook_suppressed_by_notify(
            NotifyMode::NotifyOnly,
            "on_land",
            "lane"
        ));
        assert!(!hook_suppressed_by_notify(
            NotifyMode::NotifyOnly,
            "on_land",
            ""
        ));
        assert!(!hook_suppressed_by_notify(
            NotifyMode::NotifyOnly,
            "on_red",
            "lane"
        ));
        assert!(!hook_suppressed_by_notify(
            NotifyMode::Both,
            "on_land",
            "lane"
        ));
    }

    #[test]
    fn concurrent_log_lines_never_interleave() {
        // #1357 mq-hooks-log-interleave: many hook workers finish at once;
        // every appended line must stay whole and parse.
        let state = temp_state("interleave");
        let threads = 8;
        let per_thread = 50;
        let mut handles = Vec::new();
        for t in 0..threads {
            let state = state.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..per_thread {
                    log_line(
                        &state,
                        &json!({"thread": t, "i": i, "pad": "x".repeat(4096)}),
                    );
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        let text = std::fs::read_to_string(state.join("hooks.jsonl")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), threads * per_thread, "{} lines", lines.len());
        let mut seen = std::collections::BTreeSet::new();
        for line in lines {
            let value: Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("unparseable line: {e}: {:.80}", line));
            seen.insert((
                value["thread"].as_u64().unwrap(),
                value["i"].as_u64().unwrap(),
            ));
        }
        assert_eq!(seen.len(), threads * per_thread);
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn no_command_no_hook() {
        let state = temp_state("off");
        fire(&state, &HooksConfig::default(), "on_land", json!({"pr": 1}));
        std::thread::sleep(Duration::from_millis(150));
        assert!(!state.join("hooks.jsonl").exists());
        let _ = std::fs::remove_dir_all(&state);
    }
}
