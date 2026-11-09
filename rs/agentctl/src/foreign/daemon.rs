//! Per-agent turn-runner daemon: ordered queued turns until stopped.
//!
//! Owns one durable harness session and drives it turn by turn: claim the
//! next inbox message under the intake lock, run it through the harness
//! behind the launch gate, stream events into the transcript, publish
//! `last-message.txt`, and append the `===TURN-DONE <seq> rc=<n> <iso>===`
//! sentinel a coordinator watches. Crash-visible by design: a harness exit
//! or timeout records its label and the runner keeps living; only a STOP
//! marker or SIGTERM ends the loop.

use std::io::{BufRead, BufReader, Read, Write as _};
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::lifecycle::*;
use super::ownership::{
    kill_harness_tree_at, set_child_subreaper, spawn_owned_harness_at, ForeignOwnershipError,
    HarnessRecord, OwnedHarnessProcess, OwnedWaitError, ProcessIdentity, SpawnHarnessError,
    SpawnOwnedHarnessOptions,
};
use super::registry::{
    agent_dir_at, failed_dir_at, foreign_base, inflight_dir_at, processed_dir_at,
    read_active_harness_at, read_registry_at, record_active_harness_at, remove_active_harness_at,
    terminate_active_harness_at, with_registry_lock_at, write_durable_json, AgentRecord, Message,
};
use super::runner::*;

/// How long leftovers may keep output pipes open after the leader exits.
const LEFTOVER_GRACE: Duration = Duration::from_secs(1);

fn op_err(code: &'static str, message: impl Into<String>) -> ForeignOwnershipError {
    ForeignOwnershipError {
        code,
        message: message.into(),
    }
}

fn io_op(context: &str, path: &Path, error: &std::io::Error) -> ForeignOwnershipError {
    op_err(
        "registry_io",
        format!("cannot {context} {}: {error}", path.display()),
    )
}

/// Runner command and timeout configuration.
#[derive(Clone, Debug, Default)]
pub struct RunnerConfig {
    /// Launch-gate options for owned spawns.
    pub gate: SpawnOwnedHarnessOptions,
    /// Per-backend-call timeout passed to the backend runner.
    pub backend_timeout_seconds: Option<f64>,
    /// Whole-turn watchdog timeout (`$SUBAGENTS_TURN_TIMEOUT` when unset).
    pub turn_timeout: Option<Duration>,
}

struct Daemon {
    base: PathBuf,
    proc_root: PathBuf,
    name: String,
    config: RunnerConfig,
    stop: Arc<AtomicBool>,
}

impl Daemon {
    fn append(&self, text: &str) -> Result<(), ForeignOwnershipError> {
        let path = transcript_path_at(&self.base, &self.name)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| io_op("open transcript", &path, &error))?;
        file.write_all(text.as_bytes())
            .map_err(|error| io_op("append transcript", &path, &error))?;
        if !text.ends_with('\n') {
            file.write_all(b"\n")
                .map_err(|error| io_op("append transcript", &path, &error))?;
        }
        Ok(())
    }

    fn exit_reason(reason: &str) {
        eprintln!("[subagents] runner clean exit: {reason}");
    }

    fn set_status(&self, status: &str, mark_turn: bool) -> Result<(), ForeignOwnershipError> {
        with_registry_lock_at(&self.base, |agents| {
            if let Some(record) = agents.get_mut(&self.name) {
                record.status = status.to_string();
                if mark_turn {
                    record.last_turn_at = Some(now_iso());
                }
            }
            Ok(())
        })
    }

    fn record_session_id(&self, session_id: &str) -> Result<(), ForeignOwnershipError> {
        with_registry_lock_at(&self.base, |agents| {
            if let Some(record) = agents.get_mut(&self.name) {
                if record.session_id.is_none() {
                    record.session_id = Some(session_id.to_string());
                }
            }
            Ok(())
        })
    }

    fn runner_identity(&self) -> Result<ProcessIdentity, ForeignOwnershipError> {
        let pid = std::process::id() as i64;
        Ok(ProcessIdentity::new(
            pid,
            pid_start_time_at(&self.proc_root, pid)?,
            Some(super::ownership::current_boot_id_at(&self.proc_root)?),
        ))
    }

    fn record_runner_pid(&self, stage_token: Option<&str>) -> Result<(), ForeignOwnershipError> {
        let identity = self.runner_identity()?;
        if let Some(token) = stage_token {
            return write_staged_runner_at(&self.base, &self.name, token, &identity);
        }
        let pid_path = runner_pid_path_at(&self.base, &self.name)?;
        std::fs::write(&pid_path, format!("{}\n", identity.pid))
            .map_err(|error| io_op("write runner pid", &pid_path, &error))?;
        with_registry_lock_at(&self.base, |agents| {
            let Some(record) = agents.get_mut(&self.name) else {
                return Err(op_err(
                    "unknown_agent",
                    format!(
                        "runner started for unknown agent '{}' (no registry row)",
                        self.name
                    ),
                ));
            };
            record.runner_pid = Some(identity.pid);
            record.runner_started_at = identity.started_at.clone();
            record.runner_boot_id = identity.boot_id.clone();
            record.status = "idle".to_string();
            Ok(())
        })
    }

    fn finish_ownership(
        &self,
        proc: &mut OwnedHarnessProcess,
    ) -> Result<(), ForeignOwnershipError> {
        let record = proc.ownership().cloned();
        let complete = proc.finish();
        if let Some(report) = proc.cleanup() {
            if report.killed > 0 {
                self.append(&format!(
                    "===HARNESS CLEANUP {} {}===",
                    now_iso(),
                    report.describe()
                ))?;
            }
        }
        let Some(record) = record else {
            return Ok(());
        };
        if complete {
            return remove_active_harness_at(&self.base, &self.name, &record);
        }
        let detail = match proc.cleanup() {
            None => "the harness leader was not reaped".to_string(),
            Some(report) => report.describe(),
        };
        let path = super::registry::active_harness_path_at(&self.base, &self.name)?;
        self.append(&format!(
            "===HARNESS CLEANUP INCOMPLETE {} {detail}; ownership kept in {}===",
            now_iso(),
            path.display()
        ))
    }

    fn clear_previous_harness(&self) -> Result<(), ForeignOwnershipError> {
        let Some(record) = read_active_harness_at(&self.base, &self.name)? else {
            return Ok(());
        };
        if record.legacy() {
            if let Some(rec) = read_registry_at(&self.base)?.get(&self.name).cloned() {
                // Raises the legacy refusal while that leader is alive.
                terminate_active_harness_at(&self.base, &self.proc_root, &rec)?;
            }
            return Ok(());
        }
        let report = kill_harness_tree_at(&self.proc_root, &record, None, false, false, None)?;
        if !report.complete() {
            return Err(op_err(
                "harness_cleanup_incomplete",
                format!(
                    "the previous harness tree is still present ({}); not starting another",
                    report.describe()
                ),
            ));
        }
        remove_active_harness_at(&self.base, &self.name, &record)
    }

    fn spawn_harness(
        &self,
        argv: &[String],
        cwd: &str,
        stdin: bool,
    ) -> Result<OwnedHarnessProcess, ForeignOwnershipError> {
        let _lock = harness_lock_at(&self.base, &self.name)?;
        if self.stop.load(Ordering::SeqCst)
            || stop_path_at(&self.base, &self.name).is_ok_and(|path| path.exists())
        {
            return Err(op_err(
                "interrupted",
                "worker was stopped before harness launch",
            ));
        }
        self.clear_previous_harness()?;
        let recorded: std::cell::RefCell<Vec<HarnessRecord>> = Default::default();
        let runner = self.runner_identity()?;
        let name = self.name.clone();
        let base = self.base.clone();
        let spawned = spawn_owned_harness_at(
            &self.proc_root,
            &self.config.gate,
            argv,
            Path::new(cwd),
            stdin,
            &runner,
            |value| {
                record_active_harness_at(&base, &name, value, &now_iso())?;
                recorded.borrow_mut().push(value.clone());
                Ok(())
            },
            Some(&mut |note: &str| eprintln!("[subagents] {note}")),
        );
        match spawned {
            Ok(proc) => Ok(proc),
            Err(error) => {
                for value in recorded.borrow().iter() {
                    let _ = remove_active_harness_at(&self.base, &self.name, value);
                }
                Err(match error {
                    SpawnHarnessError::Operation(error) => error,
                    SpawnHarnessError::Io(error) => op_err(
                        "harness_spawn_failed",
                        format!("cannot spawn harness: {error}"),
                    ),
                })
            }
        }
    }

    /// Run `work` while a watchdog enforces stop, turn timeout, and the
    /// leftover sweep; returns the work result and whether it timed out.
    fn with_owned_harness<T>(
        &self,
        proc: &mut OwnedHarnessProcess,
        work: impl FnOnce(&mut OwnedHarnessProcess) -> Result<T, ForeignOwnershipError>,
    ) -> Result<(T, bool), ForeignOwnershipError> {
        let timed_out = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let watchdog = {
            let stop = Arc::clone(&self.stop);
            let timed_out = Arc::clone(&timed_out);
            let finished = Arc::clone(&finished);
            let proc_root = self.proc_root.clone();
            let record = proc.ownership().cloned();
            let handle = proc.leader_handle_try_clone().and_then(|result| {
                result
                    .map_err(|error| {
                        op_err(
                            "harness_spawn_failed",
                            format!("cannot clone leader handle: {error}"),
                        )
                    })
                    .ok()
            });
            let timeout = self
                .config
                .turn_timeout
                .unwrap_or_else(|| Duration::from_secs(turn_timeout_s().max(0) as u64));
            std::thread::spawn(move || {
                let deadline = Instant::now() + timeout;
                let mut exited_at: Option<Instant> = None;
                let mut swept = false;
                let kill =
                    |record: &Option<HarnessRecord>,
                     handle: &Option<super::ownership::ProcessHandle>| {
                        if let (Some(record), Some(handle)) = (record, handle) {
                            let _ = kill_harness_tree_at(
                                &proc_root,
                                record,
                                Some(handle),
                                true,
                                true,
                                None,
                            );
                        } else if let Some(handle) = handle {
                            handle.send(libc::SIGKILL);
                        }
                    };
                while !finished.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(50));
                    if finished.load(Ordering::SeqCst) {
                        break;
                    }
                    // STOP permits the active turn to finish during the
                    // caller's grace period; SIGTERM interrupts after it.
                    if stop.load(Ordering::SeqCst) {
                        kill(&record, &handle);
                        return;
                    }
                    if Instant::now() >= deadline {
                        timed_out.store(true, Ordering::SeqCst);
                        kill(&record, &handle);
                        return;
                    }
                    let Some(handle) = &handle else { continue };
                    if swept {
                        continue;
                    }
                    if exited_at.is_none() && handle.exited() {
                        exited_at = Some(Instant::now());
                    }
                    if exited_at.is_some_and(|since| Instant::now() - since >= LEFTOVER_GRACE) {
                        if let Some(record) = &record {
                            let _ = kill_harness_tree_at(
                                &proc_root,
                                record,
                                Some(handle),
                                true,
                                true,
                                None,
                            );
                        } else {
                            handle.send(libc::SIGKILL);
                        }
                        swept = true;
                    }
                }
            })
        };
        let result = work(proc);
        let result = match result {
            Ok(value) => Ok(value),
            Err(error) => {
                let _ = proc.kill_tree();
                let _ = proc.wait(Some(super::ownership::default_kill_wait()));
                Err(error)
            }
        };
        finished.store(true, Ordering::SeqCst);
        // The watchdog polls `finished` every 50 ms and its kills are
        // bounded; give it a second, then detach it if still running.
        let join_deadline = Instant::now() + Duration::from_secs(1);
        while !watchdog.is_finished() && Instant::now() < join_deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if watchdog.is_finished() {
            let _ = watchdog.join();
        }
        let finish = self.finish_ownership(proc);
        match (result, finish) {
            (Ok(value), Ok(())) => Ok((value, timed_out.load(Ordering::SeqCst))),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }
}

pub(crate) fn exit_status_rc(status: std::process::ExitStatus) -> i64 {
    status
        .code()
        .map(i64::from)
        .unwrap_or_else(|| -i64::from(status.signal().unwrap_or(0)))
}

fn from_wait(error: OwnedWaitError) -> ForeignOwnershipError {
    match error {
        OwnedWaitError::Operation(error) => error,
        OwnedWaitError::Io(error) => op_err(
            "harness_spawn_failed",
            format!("cannot wait for harness: {error}"),
        ),
        OwnedWaitError::Timeout(timeout) => op_err(
            "harness_spawn_failed",
            format!("harness wait timed out after {}s", timeout.as_secs()),
        ),
    }
}

pub(crate) fn wait_rc(
    result: Result<std::process::ExitStatus, OwnedWaitError>,
) -> Result<i64, ForeignOwnershipError> {
    match result {
        Ok(status) => Ok(exit_status_rc(status)),
        Err(OwnedWaitError::Operation(error)) => Err(error),
        Err(OwnedWaitError::Io(error)) => Err(op_err(
            "harness_spawn_failed",
            format!("cannot wait for harness: {error}"),
        )),
        Err(OwnedWaitError::Timeout(timeout)) => Err(op_err(
            "harness_spawn_failed",
            format!("harness wait timed out after {}s", timeout.as_secs()),
        )),
    }
}

fn tail_chars(text: &str, limit: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit {
        return text.to_string();
    }
    chars[chars.len() - limit..].iter().collect()
}

fn first_lines(text: &str, count: usize, width: usize) -> Vec<String> {
    text.trim()
        .lines()
        .take(count)
        .map(|line| line.chars().take(width).collect())
        .collect()
}

impl Daemon {
    fn run_codex_turn(
        &self,
        rec: &AgentRecord,
        msg: &Message,
    ) -> Result<(), ForeignOwnershipError> {
        let resume = rec.session_id.is_some();
        let argv = build_codex_argv(
            &self.base,
            rec,
            msg,
            std::env::var("SUBAGENT_EFFORT").ok().as_deref(),
        );
        let answer_path = PathBuf::from(&argv[argv.iter().position(|a| a == "-o").unwrap() + 1]);
        let _ = std::fs::remove_file(&answer_path);
        let last_path = last_message_path_at(&self.base, &self.name)?;
        std::fs::write(&last_path, "").map_err(|e| io_op("clear last message", &last_path, &e))?;
        self.set_status("busy", false)?;
        write_event_at(
            &self.base,
            "turn_started",
            &self.name,
            Some(msg.seq),
            None,
            Some(&last_message_preview_at(&self.base, &self.name, 240)?),
        )?;
        let model_note = msg
            .model
            .as_deref()
            .or(rec.model.as_deref())
            .unwrap_or("<codex-default>");
        self.append(&format!(
            "===TURN {} START {} mode={} model={model_note}===\n>>> {}",
            msg.seq,
            now_iso(),
            if resume { "resume" } else { "exec" },
            msg.text
        ))?;
        let mut proc = self.spawn_harness(&argv, &rec.cwd, true)?;
        let name = self.name.clone();
        let base = self.base.clone();
        let text = msg.text.clone();
        let (rc, timed_out) = self.with_owned_harness(&mut proc, |proc| {
            let mut stdin = proc
                .take_stdin()
                .ok_or_else(|| op_err("harness_spawn_failed", "codex stdin was not piped"))?;
            let stdout = proc
                .take_stdout()
                .ok_or_else(|| op_err("harness_spawn_failed", "codex stdout was not piped"))?;
            let stderr = proc.take_stderr();
            let stderr_thread = stderr.map(|mut stderr| {
                std::thread::spawn(move || {
                    let mut chunks: Vec<String> = Vec::new();
                    let mut buf = [0_u8; 4096];
                    loop {
                        match stderr.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                chunks.push(String::from_utf8_lossy(&buf[..n]).into_owned());
                                if chunks.len() > 4 {
                                    chunks.remove(0);
                                }
                            }
                        }
                    }
                    chunks
                })
            });
            match stdin.write_all(text.as_bytes()) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {}
                Err(error) => return Err(io_op("write codex stdin", Path::new("stdin"), &error)),
            }
            drop(stdin);
            let transcript_path = transcript_path_at(&base, &name)?;
            let mut transcript = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&transcript_path)
                .map_err(|e| io_op("open transcript", &transcript_path, &e))?;
            let mut reader = BufReader::new(stdout);
            let mut observer = CodexTurnObserver::default();
            loop {
                let mut line = String::new();
                let read = reader
                    .read_line(&mut line)
                    .map_err(|e| io_op("read codex stdout", Path::new("stdout"), &e))?;
                if read == 0 {
                    break;
                }
                transcript
                    .write_all(line.as_bytes())
                    .map_err(|e| io_op("append transcript", &transcript_path, &e))?;
                transcript
                    .flush()
                    .map_err(|e| io_op("flush transcript", &transcript_path, &e))?;
                let stripped = line.trim();
                for echo in pane_echo_lines(stripped) {
                    println!("{echo}");
                }
                observer.push_line(stripped);
            }
            let rc = wait_rc(proc.wait(None))?;
            let stderr_chunks = stderr_thread
                .map(|thread| thread.join().unwrap_or_default())
                .unwrap_or_default();
            Ok((rc, observer, stderr_chunks))
        })?;
        let (rc, observer, stderr_chunks) = rc;
        let stderr_tail = stderr_chunks.concat().trim().to_string();
        if let Some(session) = &observer.captured_session {
            if !resume {
                self.record_session_id(session)?;
            }
        }
        let mut last_msg = String::new();
        if answer_path.exists() {
            last_msg = std::fs::read_to_string(&answer_path)
                .map_err(|e| io_op("read codex answer", &answer_path, &e))?
                .trim()
                .to_string();
        }
        if rc != 0 && last_msg.is_empty() {
            let detail = observer
                .captured_error
                .clone()
                .or_else(|| {
                    let tail = tail_chars(&stderr_tail, 2000);
                    if tail.is_empty() {
                        None
                    } else {
                        Some(tail)
                    }
                })
                .unwrap_or_else(|| format!("process exited {rc}"));
            last_msg = format!("[HARNESS ERROR] {detail}");
        }
        std::fs::write(&last_path, &last_msg)
            .map_err(|e| io_op("write last message", &last_path, &e))?;
        let rc_label = if timed_out {
            "timeout".to_string()
        } else {
            rc.to_string()
        };
        self.append(&format!("===TURN {} OUTPUT===\n{last_msg}", msg.seq))?;
        if rc != 0 && !stderr_tail.is_empty() {
            self.append(&format!(
                "===TURN {} STDERR (tail)===\n{}",
                msg.seq,
                tail_chars(&stderr_tail, 2000)
            ))?;
        }
        self.append(&format!(
            "===TURN-DONE {} rc={rc_label} {}===",
            msg.seq,
            now_iso()
        ))?;
        write_event_at(
            &self.base,
            "TURN-DONE",
            &self.name,
            Some(msg.seq),
            Some(&rc_label),
            Some(&last_message_preview_at(&self.base, &self.name, 240)?),
        )?;
        self.set_status(if rc == 0 { "idle" } else { "error" }, true)
    }

    fn run_agy_turn(&self, rec: &AgentRecord, msg: &Message) -> Result<(), ForeignOwnershipError> {
        let resume = rec.session_id.is_some();
        let turn_log = agy_turn_log_path(&self.base, &self.name, msg.seq);
        let argv = build_agy_argv(rec, msg, &turn_log, turn_timeout_s());
        let requested_model = msg.model.clone().or_else(|| rec.model.clone());
        let model_note = requested_model.as_deref().unwrap_or("<agy-default>");
        self.set_status("busy", false)?;
        write_event_at(
            &self.base,
            "turn_started",
            &self.name,
            Some(msg.seq),
            None,
            Some(&last_message_preview_at(&self.base, &self.name, 240)?),
        )?;
        self.append(&format!(
            "===TURN {} START {} mode={} harness=agy model={model_note} log={}===\n>>> {}",
            msg.seq,
            now_iso(),
            if resume { "resume" } else { "exec" },
            turn_log.display(),
            msg.text
        ))?;
        let mut proc = self.spawn_harness(&argv, &rec.cwd, false)?;
        let ((rc, stdout_text, stderr_text), timed_out) =
            self.with_owned_harness(&mut proc, |proc| {
                let (status, stdout, stderr) = proc.wait_with_output(None).map_err(from_wait)?;
                Ok((exit_status_rc(status), stdout, stderr))
            })?;
        let mut nosession = false;
        if !resume {
            match capture_agy_session_id(&turn_log) {
                Some(session) => self.record_session_id(&session)?,
                None => nosession = true,
            }
        }
        let raw_label = if timed_out {
            "timeout".to_string()
        } else if nosession {
            "nosession".to_string()
        } else {
            rc.to_string()
        };
        let (turn_log_text, turn_log_warning) = read_agy_turn_log(&turn_log);
        let outcome = classify_agy_outcome(&raw_label, &stdout_text, &stderr_text, &turn_log_text);
        // The nosession raw label must survive classification when nothing
        // stronger (quota/auth) matched, as in the established runner.
        let outcome = if nosession && outcome.rc_label == raw_label {
            outcome.clone()
        } else {
            outcome
        };
        let mut last_message = stdout_text.clone();
        if last_message.trim().is_empty() && !outcome.message_if_empty.is_empty() {
            last_message = outcome.message_if_empty.clone();
        }
        let last_path = last_message_path_at(&self.base, &self.name)?;
        std::fs::write(&last_path, &last_message)
            .map_err(|e| io_op("write last message", &last_path, &e))?;
        for line in first_lines(last_message.trim(), 12, 200) {
            println!("| {line}");
        }
        if last_message.trim().lines().count() > 12 {
            println!("| ... (more lines in transcript)");
        }
        let log_text_for_note = if turn_log.exists() {
            Some(turn_log_text.as_str())
        } else {
            None
        };
        self.append(&agy_model_note_from_text(
            requested_model.as_deref(),
            log_text_for_note,
            msg.seq,
        ))?;
        if let Some(warning) = &turn_log_warning {
            self.append(warning)?;
        }
        if nosession {
            self.append(&format!(
                "===AGY SESSION ERROR seq={} log={} missing 'Stream completed for <uuid>'; refusing to continue without session_id===",
                msg.seq,
                turn_log.display()
            ))?;
        }
        self.append(&format!(
            "===TURN {} OUTPUT===\n{}",
            msg.seq,
            last_message.trim()
        ))?;
        if !stderr_text.trim().is_empty() {
            self.append(&format!(
                "===TURN {} STDERR (tail)===\n{}",
                msg.seq,
                tail_chars(stderr_text.trim(), 2000)
            ))?;
        }
        let done_suffix = if outcome.done_fields.is_empty() {
            String::new()
        } else {
            format!(" {}", outcome.done_fields.join(" "))
        };
        self.append(&format!(
            "===TURN-DONE {} rc={}{} {}===",
            msg.seq,
            outcome.rc_label,
            done_suffix,
            now_iso()
        ))?;
        write_event_at(
            &self.base,
            "TURN-DONE",
            &self.name,
            Some(msg.seq),
            Some(&outcome.rc_label),
            Some(&last_message_preview_at(&self.base, &self.name, 240)?),
        )?;
        self.set_status(&outcome.status, true)
    }

    fn run_muse_turn(&self, rec: &AgentRecord, msg: &Message) -> Result<(), ForeignOwnershipError> {
        let argv =
            build_muse_argv(rec, msg).map_err(|message| op_err("invalid_harness_args", message))?;
        let requested_model = msg
            .model
            .as_deref()
            .or(rec.model.as_deref())
            .unwrap_or("<muse-default>");
        self.set_status("busy", false)?;
        write_event_at(
            &self.base,
            "turn_started",
            &self.name,
            Some(msg.seq),
            None,
            Some(&last_message_preview_at(&self.base, &self.name, 240)?),
        )?;
        self.append(&format!(
            "===TURN {} START {} mode=exec harness=muse model={requested_model} session={}===\n>>> {}",
            msg.seq,
            now_iso(),
            rec.session_id.as_deref().unwrap_or_default(),
            msg.text
        ))?;
        let session_id = rec.session_id.clone().unwrap_or_default();
        let mut proc = self.spawn_harness(&argv, &rec.cwd, false)?;
        let base = self.base.clone();
        let name = self.name.clone();
        let ((rc, mut validator, stderr_chunks), timed_out) =
            self.with_owned_harness(&mut proc, |proc| {
                let stdout = proc
                    .take_stdout()
                    .ok_or_else(|| op_err("harness_spawn_failed", "muse stdout was not piped"))?;
                let stderr_thread = proc.take_stderr().map(|mut stderr| {
                    std::thread::spawn(move || {
                        let mut chunks: Vec<String> = Vec::new();
                        let mut buf = [0_u8; 4096];
                        loop {
                            match stderr.read(&mut buf) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => match std::str::from_utf8(&buf[..n]) {
                                    Ok(text) => chunks.push(text.to_string()),
                                    Err(error) => {
                                        chunks.push(format!(
                                            "[Muse stderr Unicode decode error: {error}]"
                                        ));
                                        break;
                                    }
                                },
                            }
                            if chunks.len() > 16 {
                                chunks.remove(0);
                            }
                        }
                        chunks
                    })
                });
                let transcript_path = transcript_path_at(&base, &name)?;
                let mut transcript = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&transcript_path)
                    .map_err(|e| io_op("open transcript", &transcript_path, &e))?;
                let mut reader = BufReader::new(stdout);
                let mut validator = MuseTurnValidator::new(session_id.clone());
                loop {
                    let mut line_bytes: Vec<u8> = Vec::new();
                    let read = {
                        let mut limited =
                            std::io::Read::take(&mut reader, (MUSE_MAX_EVENT_BYTES + 1) as u64);
                        limited
                            .read_until(b'\n', &mut line_bytes)
                            .map_err(|e| io_op("read muse stdout", Path::new("stdout"), &e))?
                    };
                    if read == 0 {
                        break;
                    }
                    let outcome = validator.push_line(&line_bytes);
                    let (written, protocol) = match outcome {
                        MuseLineOutcome::Written => (true, false),
                        MuseLineOutcome::ProtocolError { written, .. } => (written, true),
                    };
                    if written {
                        transcript
                            .write_all(&line_bytes)
                            .map_err(|e| io_op("append transcript", &transcript_path, &e))?;
                        transcript
                            .flush()
                            .map_err(|e| io_op("flush transcript", &transcript_path, &e))?;
                    }
                    if protocol {
                        break;
                    }
                }
                if validator.protocol_error().is_some() && proc.poll().map_err(from_wait)?.is_none()
                {
                    proc.kill_tree()?;
                }
                let rc = wait_rc(proc.wait(None))?;
                let stderr_chunks = stderr_thread
                    .map(|thread| thread.join().unwrap_or_default())
                    .unwrap_or_default();
                Ok((rc, validator, stderr_chunks))
            })?;
        let stderr_tail = tail_chars(stderr_chunks.concat().trim(), 8000);
        let outcome = validator.finish(rc, timed_out, &stderr_tail);
        let last_path = last_message_path_at(&self.base, &self.name)?;
        std::fs::write(&last_path, &outcome.answer)
            .map_err(|e| io_op("write last message", &last_path, &e))?;
        for line in first_lines(outcome.answer.trim(), 12, 200) {
            println!("| {line}");
        }
        if outcome.answer.trim().lines().count() > 12 {
            println!("| ... (more lines in transcript)");
        }
        self.append(&format!(
            "===TURN {} OUTPUT===\n{}",
            msg.seq, outcome.answer
        ))?;
        if !stderr_tail.is_empty() {
            self.append(&format!(
                "===TURN {} STDERR (tail)===\n{stderr_tail}",
                msg.seq
            ))?;
        }
        self.append(&format!(
            "===TURN-DONE {} rc={} {}===",
            msg.seq,
            outcome.rc_label,
            now_iso()
        ))?;
        write_event_at(
            &self.base,
            "TURN-DONE",
            &self.name,
            Some(msg.seq),
            Some(&outcome.rc_label),
            Some(&last_message_preview_at(&self.base, &self.name, 240)?),
        )?;
        self.set_status(&outcome.status, true)
    }

    fn run_turn(&self, msg: &Message) -> Result<(), ForeignOwnershipError> {
        let rec = with_registry_lock_at(&self.base, |agents| Ok(agents.get(&self.name).cloned()))?;
        let Some(rec) = rec else {
            return Err(op_err(
                "unknown_agent",
                format!("agent '{}' vanished from registry mid-run", self.name),
            ));
        };
        match rec.harness.as_str() {
            "codex" => self.run_codex_turn(&rec, msg),
            "agy" => self.run_agy_turn(&rec, msg),
            "muse" => self.run_muse_turn(&rec, msg),
            other => {
                self.append(&format!(
                    "===TURN {} START {} mode=error harness={other}===\n>>> {}",
                    msg.seq,
                    now_iso(),
                    msg.text
                ))?;
                self.append(&format!(
                    "===TURN-DONE {} rc=unsupported_harness {}===",
                    msg.seq,
                    now_iso()
                ))?;
                self.set_status("error", true)
            }
        }
    }
}

fn sync_directory(path: &Path) -> Result<(), ForeignOwnershipError> {
    std::fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| io_op("sync directory", path, &error))
}

fn fsync_file(path: &Path) -> Result<(), ForeignOwnershipError> {
    std::fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| io_op("fsync", path, &error))
}

impl Daemon {
    fn quarantine(&self, path: &Path, reason: &str) -> Result<(), ForeignOwnershipError> {
        let failed = failed_dir_at(&self.base, &self.name)?;
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut destination = failed.join(&file_name);
        if destination.exists() {
            let stem = Path::new(&file_name)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            let ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos());
            destination = failed.join(format!("{stem}-{ns}.json"));
        }
        let sidecar = destination.with_extension("json.error");
        write_durable_json(
            &sidecar,
            &serde_json::json!({
                "outcome": "possibly_submitted",
                "error": reason,
                "recorded_at": now_iso(),
            }),
        )
        .map_err(|error| io_op("write quarantine sidecar", &sidecar, &error))?;
        std::fs::rename(path, &destination)
            .map_err(|error| io_op("quarantine request", path, &error))?;
        sync_directory(&failed)?;
        if let Some(parent) = path.parent() {
            sync_directory(parent)?;
        }
        self.set_status("error", false)?;
        write_event_at(
            &self.base,
            "turn_quarantined",
            &self.name,
            None,
            Some("possibly_submitted"),
            Some(reason),
        )
    }

    fn recover_inflight(&self) -> Result<(), ForeignOwnershipError> {
        let dir = inflight_dir_at(&self.base, &self.name)?;
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_op("list inflight", &dir, &error)),
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        paths.sort();
        for path in paths {
            self.quarantine(
                &path,
                "runner exited after claiming this turn; inspect its transcript before retrying",
            )?;
        }
        Ok(())
    }

    fn consume(&self, msg_path: &Path) -> Result<(), ForeignOwnershipError> {
        let _intake = intake_lock_at(&self.base, &self.name)?;
        self.recover_inflight()?;
        if !msg_path.exists() {
            return Ok(());
        }
        let agent_dir = agent_dir_at(&self.base, &self.name)?;
        if agent_dir.join("automation-paused").exists()
            || migration_pause_path_at(&self.base, &self.name).is_ok_and(|path| path.exists())
        {
            return Ok(());
        }
        let file_name = msg_path
            .file_name()
            .ok_or_else(|| op_err("invalid_turn_request", "pending path has no file name"))?;
        let destination = processed_dir_at(&self.base, &self.name)?.join(file_name);
        if destination.exists() {
            return self.quarantine(
                msg_path,
                "this turn sequence already has a completed request",
            );
        }
        let claimed = inflight_dir_at(&self.base, &self.name)?.join(file_name);
        fsync_file(msg_path)?;
        std::fs::rename(msg_path, &claimed)
            .map_err(|error| io_op("claim request", msg_path, &error))?;
        if let Some(parent) = claimed.parent() {
            sync_directory(parent)?;
        }
        if let Some(parent) = msg_path.parent() {
            sync_directory(parent)?;
        }
        let msg = match Message::from_path(&claimed) {
            Ok(msg) => msg,
            Err(error) => {
                return self.quarantine(&claimed, &format!("invalid turn request: {error}"));
            }
        };
        self.run_turn(&msg)?;
        for output in [
            transcript_path_at(&self.base, &self.name)?,
            last_message_path_at(&self.base, &self.name)?,
        ] {
            if output.exists() {
                fsync_file(&output)?;
            }
        }
        std::fs::rename(&claimed, &destination)
            .map_err(|error| io_op("complete request", &claimed, &error))?;
        if let Some(parent) = destination.parent() {
            sync_directory(parent)?;
        }
        if let Some(parent) = claimed.parent() {
            sync_directory(parent)?;
        }
        Ok(())
    }

    fn run(&mut self, mut stage_token: Option<String>) -> Result<i32, ForeignOwnershipError> {
        {
            let stop_flag = Arc::clone(&self.stop);
            let _ = signal_hook::flag::register(libc::SIGTERM, Arc::clone(&stop_flag));
        }
        // An inherited SIG_IGN would reap exited children at once, freeing
        // a harness leader's number while its leftovers are still swept.
        unsafe {
            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
        }
        // Orphans of a harness tree (including double-forked setsid
        // escapees) become this runner's children, so the tree stays reachable.
        if !set_child_subreaper() {
            eprintln!(
                "[subagents] runner could not become a child subreaper; \
                 orphaned harness descendants are reachable only by generation"
            );
        }
        ensure_agent_dirs_at(&self.base, &self.name)?;
        if stage_token.is_none() {
            if let Some(old) = read_registry_at(&self.base)?.get(&self.name).cloned() {
                if old.runner_pid.is_some() {
                    if runner_identity_alive_at(&self.proc_root, &old)?
                        && old.runner_pid != Some(std::process::id() as i64)
                    {
                        return Err(op_err(
                            "runner_already_live",
                            format!("worker '{}' already has a live runner", self.name),
                        ));
                    }
                    terminate_active_harness_at(&self.base, &self.proc_root, &old)?;
                }
            }
        }
        self.record_runner_pid(stage_token.as_deref())?;
        if let Some(token) = stage_token.clone() {
            while !self.stop.load(Ordering::SeqCst) {
                if stop_path_at(&self.base, &self.name).is_ok_and(|path| path.exists()) {
                    Self::exit_reason("stop marker while awaiting staged activation");
                    return Ok(0);
                }
                if staged_runner_activate_path_at(&self.base, &self.name, &token)
                    .is_ok_and(|path| path.exists())
                {
                    acknowledge_staged_runner_activation_at(&self.base, &self.name, &token)?;
                    stage_token = None;
                    // The migration committed this runner to the registry;
                    // keep runner.pid naming the same process.
                    let pid_path = runner_pid_path_at(&self.base, &self.name)?;
                    std::fs::write(&pid_path, format!("{}\n", std::process::id()))
                        .map_err(|e| io_op("write runner pid", &pid_path, &e))?;
                    break;
                }
                if !staged_runner_path_at(&self.base, &self.name, &token)
                    .is_ok_and(|path| path.exists())
                {
                    Self::exit_reason("staged marker removed before activation acknowledgement");
                    return Ok(0);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        self.append(&format!(
            "===RUNNER UP {} pid={} {}===",
            self.name,
            std::process::id(),
            now_iso()
        ))?;
        {
            let _intake = intake_lock_at(&self.base, &self.name)?;
            self.recover_inflight()?;
        }
        while !self.stop.load(Ordering::SeqCst) {
            if stop_path_at(&self.base, &self.name).is_ok_and(|path| path.exists()) {
                break;
            }
            if stage_token.is_none()
                && migration_pause_path_at(&self.base, &self.name).is_ok_and(|path| path.exists())
            {
                let identity = ProcessIdentity::new(
                    std::process::id() as i64,
                    pid_start_time_at(&self.proc_root, std::process::id() as i64)?,
                    None,
                );
                acknowledge_migration_pause_at(&self.base, &self.name, &identity)?;
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            let agent_dir = agent_dir_at(&self.base, &self.name)?;
            if agent_dir.join("automation-paused").exists() {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            let Some(pending) = next_pending_message_at(&self.base, &self.name)? else {
                super::ownership::reap_adopted_children(&[]);
                std::thread::sleep(Duration::from_millis(500));
                continue;
            };
            self.consume(&pending)?;
        }
        self.append(&format!("===RUNNER DOWN {} {}===", self.name, now_iso()))?;
        Self::exit_reason("stop marker or SIGTERM after active runner loop");
        Ok(0)
    }
}

/// Run one worker's turn-runner daemon until a stop marker or SIGTERM.
pub fn run_runner(
    base: &Path,
    proc_root: &Path,
    name: &str,
    stage_token: Option<String>,
    config: RunnerConfig,
) -> Result<i32, ForeignOwnershipError> {
    super::registry::require_valid_name(name)?;
    let mut daemon = Daemon {
        base: base.to_path_buf(),
        proc_root: proc_root.to_path_buf(),
        name: name.to_string(),
        config,
        stop: Arc::new(AtomicBool::new(false)),
    };
    daemon.run(stage_token)
}

/// Run the daemon with the default foreign base and procfs.
pub fn run_runner_default(
    name: &str,
    stage_token: Option<String>,
) -> Result<i32, ForeignOwnershipError> {
    run_runner(
        &foreign_base(),
        Path::new("/proc"),
        name,
        stage_token,
        RunnerConfig::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_base(test: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("agentctl-b3-7-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn gate_double(root: &Path) -> SpawnOwnedHarnessOptions {
        // The production gate is this binary's `--chat-launch-gate` entry,
        // which a unit-test binary cannot be; compile the same protocol.
        let source = root.join("gate.rs");
        std::fs::write(
            &source,
            "use std::fs::File;\n\
             use std::io::{Read, Write};\n\
             use std::os::fd::FromRawFd;\n\
             use std::os::unix::process::CommandExt;\n\
             extern \"C\" { fn prctl(option: i32, a2: usize, a3: usize, a4: usize, a5: usize) -> i32; fn getppid() -> i32; }\n\
             fn fail(message: String) -> ! { let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.join(\"gate-error.log\"))); if let Some(path) = dir { let _ = std::fs::write(path, message); } std::process::exit(125); }\n\
             fn main() {\n\
                 let args: Vec<String> = std::env::args().collect();\n\
                 if args.len() < 6 || args[4] != \"--\" { fail(format!(\"bad argv: {args:?}\")); }\n\
                 let parent: i32 = args[1].parse().unwrap_or_else(|_| fail(\"bad parent\".into()));\n\
                 let act: i32 = args[2].parse().unwrap_or_else(|_| fail(\"bad activation fd\".into()));\n\
                 let ready: i32 = args[3].parse().unwrap_or_else(|_| fail(\"bad readiness fd\".into()));\n\
                 if unsafe { getppid() } != parent { fail(format!(\"ppid changed\")); }\n\
                 if unsafe { prctl(1, 9, 0, 0, 0) } != 0 { fail(\"prctl failed\".into()); }\n\
                 if unsafe { getppid() } != parent { fail(\"ppid changed\".into()); }\n\
                 let mut ready_file = unsafe { File::from_raw_fd(ready) };\n\
                 if ready_file.write_all(b\"1\").is_err() { fail(\"readiness write failed\".into()); }\n\
                 drop(ready_file);\n\
                 let mut act_file = unsafe { File::from_raw_fd(act) };\n\
                 let mut activation = [0_u8; 2];\n\
                 let read = act_file.read(&mut activation).unwrap_or(0);\n\
                 drop(act_file);\n\
                 if &activation[..read] != b\"1\" || unsafe { getppid() } != parent { fail(format!(\"bad activation {activation:?}\")); }\n\
                 let error = std::process::Command::new(&args[5]).args(&args[6..]).exec();\n\
                 fail(format!(\"exec failed: {error}\"));\n\
             }\n",
        )
        .unwrap();
        let binary = root.join("gate");
        let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
        let compiled = std::process::Command::new(&rustc)
            .args(["--edition", "2021", "-O"])
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .output()
            .expect("compile gate double");
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        SpawnOwnedHarnessOptions {
            gate_prefix: vec![binary.to_string_lossy().into_owned()],
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn daemon_runs_a_fake_codex_turn_end_to_end() {
        use std::os::unix::fs::PermissionsExt as _;
        // The daemon makes its process a child subreaper and reaps adopted
        // children, so it must not share the test binary process with
        // sibling tests. In the child process this same test runs the
        // daemon itself; the parent drives it through files only.
        if std::env::var_os("AGENTCTL_B37_DAEMON_CHILD").is_some() {
            let base = PathBuf::from(std::env::var("AGENTCTL_B37_BASE").unwrap());
            let gate = std::env::var("AGENTCTL_B37_GATE").unwrap();
            let config = RunnerConfig {
                gate: SpawnOwnedHarnessOptions {
                    gate_prefix: vec![gate],
                },
                ..RunnerConfig::default()
            };
            let outcome = run_runner(&base, Path::new("/proc"), "worker-1", None, config);
            assert_eq!(outcome.unwrap(), 0);
            return;
        }
        let base = fake_base("daemon-e2e");
        let work = base.join("work");
        std::fs::create_dir_all(&work).unwrap();
        // Seed the worker row the daemon will run for.
        with_registry_lock_at(&base, |agents| {
            agents.insert(
                "worker-1".to_string(),
                AgentRecord {
                    name: "worker-1".to_string(),
                    harness: "codex".to_string(),
                    backend: "tmux".to_string(),
                    cwd: work.to_string_lossy().into_owned(),
                    status: "idle".to_string(),
                    next_seq: 1,
                    created_at: "2026-11-07T00:00:00Z".to_string(),
                    ..AgentRecord::default()
                },
            );
            Ok(())
        })
        .unwrap();
        // Fake Codex: consume stdin, stream two events, write the -o answer.
        let fake = base.join("fake-codex.sh");
        std::fs::write(
            &fake,
            "#!/bin/bash\n\
             out=\"\"; prev=\"\"\n\
             for a in \"$@\"; do if [ \"$prev\" = \"-o\" ]; then out=\"$a\"; fi; prev=\"$a\"; done\n\
             cat >/dev/null\n\
             printf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"sess-fake-1\"}'\n\
             printf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"worked\"}}'\n\
             printf 'fake answer\\n' > \"$out\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        // CODEX_BIN is read per turn; tests asserting argv[0] call codex_bin() too.
        std::env::set_var("CODEX_BIN", &fake);
        enqueue_message_at(&base, "worker-1", "do the thing", None).unwrap();
        let gate = gate_double(&base);
        let mut handle = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "foreign::daemon::tests::daemon_runs_a_fake_codex_turn_end_to_end",
                "--nocapture",
            ])
            .env("AGENTCTL_B37_DAEMON_CHILD", "1")
            .env("AGENTCTL_B37_BASE", &base)
            .env("AGENTCTL_B37_GATE", &gate.gate_prefix[0])
            .spawn()
            .expect("spawn daemon test process");
        // Wait for the turn to complete (processed file + idle status).
        let deadline = Instant::now() + Duration::from_secs(30);
        let done = loop {
            let processed = processed_dir_at(&base, "worker-1")
                .unwrap()
                .join("000000000001.json");
            let status = read_registry_at(&base)
                .ok()
                .and_then(|agents| agents.get("worker-1").map(|rec| rec.status.clone()));
            if processed.exists() && status.as_deref() == Some("idle") {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        std::fs::write(stop_path_at(&base, "worker-1").unwrap(), "").unwrap();
        let outcome = handle.wait().expect("daemon test process");
        assert!(done, "turn did not complete; daemon exit: {outcome:?}");
        assert!(outcome.success(), "daemon process failed: {outcome:?}");
        let transcript =
            std::fs::read_to_string(transcript_path_at(&base, "worker-1").unwrap()).unwrap();
        assert!(
            transcript.contains("===TURN 1 OUTPUT===\nfake answer"),
            "{transcript}"
        );
        assert!(transcript.contains("===TURN-DONE 1 rc=0"), "{transcript}");
        assert!(
            transcript.contains("===RUNNER DOWN worker-1"),
            "{transcript}"
        );
        let record = &read_registry_at(&base).unwrap()["worker-1"];
        assert_eq!(record.session_id.as_deref(), Some("sess-fake-1"));
        let last =
            std::fs::read_to_string(last_message_path_at(&base, "worker-1").unwrap()).unwrap();
        assert_eq!(last, "fake answer");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn consume_quarantines_an_invalid_request() {
        let base = fake_base("quarantine");
        with_registry_lock_at(&base, |agents| {
            agents.insert(
                "worker-1".to_string(),
                AgentRecord {
                    name: "worker-1".to_string(),
                    status: "idle".to_string(),
                    next_seq: 2,
                    ..AgentRecord::default()
                },
            );
            Ok(())
        })
        .unwrap();
        ensure_agent_dirs_at(&base, "worker-1").unwrap();
        let bad = crate::foreign::registry::inbox_dir_at(&base, "worker-1")
            .unwrap()
            .join("000000000001.json");
        std::fs::write(&bad, "{not json").unwrap();
        let daemon = Daemon {
            base: base.clone(),
            proc_root: PathBuf::from("/proc"),
            name: "worker-1".to_string(),
            config: RunnerConfig::default(),
            stop: Arc::new(AtomicBool::new(false)),
        };
        daemon.consume(&bad).unwrap();
        let failed = failed_dir_at(&base, "worker-1").unwrap();
        assert!(failed.join("000000000001.json").exists());
        assert!(failed.join("000000000001.json.error").exists());
        assert_eq!(read_registry_at(&base).unwrap()["worker-1"].status, "error");
        let _ = std::fs::remove_dir_all(&base);
    }
}
