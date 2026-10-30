//! The event-stream tail, supervised by this server (`#709 vt-live-tail-rust`).
//!
//! Until this module the tail ran as a separate Python process
//! (`scripts/live-hint-adapter.py --records`, retired by this change; its last
//! version is in git history). It spawned the provider CLI's conversation tail,
//! POSTed each record to `/api/v1/live/tail`, kept the stream position of the
//! last acknowledged record in a state file, restarted the stream when the
//! conversation list changed, and reported a stream that could not resume from
//! its saved position as a gap. This module does the same work in-process: it
//! spawns the same command, reads the same JSON-lines records from its stdout,
//! and applies each through [`crate::live_tail::ingest`] — the exact code path
//! the HTTP route uses — so `/api/v1/live/tail` stays only for external sources
//! (a chat bridge's event sinks, `#708 vt-live-push`).
//!
//! Behaviour kept identical to the adapter's records mode:
//!
//! * `{channels}` (comma-separated, sorted) and `{catchup}` (the saved
//!   position, or `[ingest] tail_initial_catchup`) are filled in argument by
//!   argument; the command is executed directly, never by a shell.
//! * Heartbeats and stall/revival notices (`HEARTBEAT`, `STALL`, `REVIVAL`)
//!   are dropped locally and never move the position.
//! * A line that cannot be read at all (not a JSON object, or past the record
//!   bound) is skipped and reported to the hub as a gap, never retried: the
//!   stream would replay it from the saved position for ever.
//! * A record the checker refuses, or whose message the provider cannot
//!   normalize (`ChatError::Shape`, the HTTP route's 400), is skipped and the
//!   position moves past it, for the same reason.
//! * The position advances only after a record is applied, only forward, and
//!   is saved atomically with mode 0600 after each advance.
//! * A stream whose stderr says it cannot resume (`queue overflow`,
//!   `retention overflow`, `no longer queued`) is reported as a gap and the
//!   position is forgotten, so the next stream starts from `tail_initial_catchup`.
//! * The conversation list is re-read on its refresh interval (or when channels
//!   are restored); a change restarts the stream with the new list and the
//!   saved position.
//! * With no conversation to tail yet, the supervisor waits (1 s, doubling to
//!   30 s, said once) instead of spawning a tail that would fail — the
//!   adapter's wait-for-ready, with the server's own channel list in place of
//!   its `GET /api/v1/live/tail` polls.
//! * A stream that ends, or a record that cannot be applied after retries, is
//!   restarted with backoff (1 s, doubling to 30 s, reset once a stream
//!   applies a record). The adapter exited for its service manager instead;
//!   the backoff is what keeps a failing tail from spending the owner's
//!   provider rate limit on respawns.
//!
//! Memory is bounded the way the adapter bounded it: one record line at a
//! time (past the bound, the rest of the line is drained and dropped), stderr
//! scanned a bounded chunk at a time and never logged, and no queue between
//! the child and the hub — applying a record is the backpressure, so a slow
//! provider read slows the pipe, never a buffer. Nothing a record carries is
//! logged; refusals name the field and the rule, never a value.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};
use tokio::sync::watch;

use crate::chat::ChatError;
use crate::live_tail::TailOutcome;
use crate::state::AppState;

/// The most bytes of one record line kept: the ingest bound, and the record around it.
const MAX_LINE_BYTES: usize = crate::slack::tail::MAX_RECORD_BYTES;
/// The most bytes of the saved-position file read.
const MAX_STATE_BYTES: u64 = 4096;
/// Record types dropped locally, as the adapter dropped them.
const CONTROL_TYPES: [&str; 3] = ["HEARTBEAT", "STALL", "REVIVAL"];
/// What the stream client writes to stderr when it cannot resume from a saved position.
const OVERFLOW_MARKERS: [&str; 3] = ["queue overflow", "retention overflow", "no longer queued"];
/// Delivery retries for one record before the stream is restarted (the adapter's schedule).
const RETRY_DELAYS: [Duration; 4] = [
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
];
/// First pause before restarting a stream or while waiting for a conversation.
const FIRST_PAUSE: Duration = Duration::from_secs(1);
/// Longest pause before restarting a stream or while waiting for a conversation.
const MAX_PAUSE: Duration = Duration::from_secs(30);
/// How long a child group gets to exit after SIGTERM before SIGKILL.
const TERM_GRACE: Duration = Duration::from_secs(2);

/// The stream position of the last record this server applied, kept in a private file.
///
/// The adapter's `StreamCursor`, with the same file shape (`{"cursor": N}`), so an existing
/// deployment's file is read as it stands and no position is lost in the move.
#[derive(Debug)]
pub struct TailCursor {
    path: PathBuf,
    value: Option<u64>,
}

impl TailCursor {
    /// A cursor for `path`, not yet loaded.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path, value: None }
    }

    /// The saved position, once loaded.
    #[must_use]
    pub fn value(&self) -> Option<u64> {
        self.value
    }

    /// Read the saved position; a missing file is no position.
    ///
    /// # Errors
    ///
    /// A string naming the problem, never file content, when the file cannot be read or does
    /// not hold this shape: starting from a guessed position would replay or skip records.
    pub fn load(&mut self) -> Result<(), String> {
        use std::io::Read as _;
        let raw = match std::fs::File::open(&self.path) {
            Ok(mut file) => {
                let mut raw = Vec::new();
                match file
                    .by_ref()
                    .take(MAX_STATE_BYTES + 1)
                    .read_to_end(&mut raw)
                {
                    Ok(_) => raw,
                    Err(_) => return Err("cannot read the tail state file".to_owned()),
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.value = None;
                return Ok(());
            }
            Err(_) => return Err("cannot read the tail state file".to_owned()),
        };
        if raw.len() as u64 > MAX_STATE_BYTES {
            return Err("the tail state file is too large".to_owned());
        }
        let value: serde_json::Value = serde_json::from_slice(&raw)
            .map_err(|_| "the tail state file is not JSON".to_owned())?;
        let cursor = value.get("cursor");
        self.value =
            match cursor {
                None | Some(serde_json::Value::Null) => None,
                Some(value) => Some(value.as_u64().ok_or_else(|| {
                    "the tail state file holds no valid stream position".to_owned()
                })?),
            };
        Ok(())
    }

    /// Move the position forward to `sequence`, never back, and save it.
    ///
    /// # Errors
    ///
    /// A string naming the problem when the file cannot be written.
    pub fn advance(&mut self, sequence: u64) -> Result<(), String> {
        if self.value.is_some_and(|held| sequence <= held) {
            return Ok(());
        }
        // Save first, then hold: a failed save leaves the held position behind the record, so
        // a retry saves it again instead of counting it saved.
        self.save(Some(sequence))?;
        self.value = Some(sequence);
        Ok(())
    }

    /// Forget the position: the stream cannot resume from it.
    ///
    /// # Errors
    ///
    /// A string naming the problem when the file cannot be written.
    pub fn forget(&mut self) -> Result<(), String> {
        self.save(None)?;
        self.value = None;
        Ok(())
    }

    fn save(&self, value: Option<u64>) -> Result<(), String> {
        let body = serde_json::json!({ "cursor": value }).to_string();
        let directory = self
            .path
            .parent()
            .ok_or_else(|| "the tail state file has no directory".to_owned())?;
        std::fs::create_dir_all(directory)
            .map_err(|_| "cannot create the tail state directory".to_owned())?;
        let temporary = directory.join(format!(".live-tail-{}.tmp", std::process::id()));
        write_private(&temporary, body.as_bytes())?;
        std::fs::rename(&temporary, &self.path).map_err(|_| {
            let _ = std::fs::remove_file(&temporary);
            "cannot write the tail state file".to_owned()
        })
    }
}

fn write_private(path: &Path, body: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "cannot write the tail state file".to_owned())?;
    file.write_all(body)
        .and_then(|()| file.sync_all())
        .map_err(|_| "cannot write the tail state file".to_owned())
}

/// Fill `{channels}` and `{catchup}` in each argument; nothing is split or joined by a shell.
#[must_use]
pub fn build_command(template: &[String], channels: &[String], catchup: &str) -> Vec<String> {
    let filled = channels.join(",");
    template
        .iter()
        .map(|argument| {
            argument
                .replace("{channels}", &filled)
                .replace("{catchup}", catchup)
        })
        .collect()
}

/// The stream position a record carries, when it gives one.
fn sequence_of(value: &serde_json::Value) -> Option<u64> {
    value.get("iris_seq_id").and_then(serde_json::Value::as_u64)
}

/// What one line of the stream did.
#[derive(Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// A blank line.
    Blank,
    /// A heartbeat or notice, dropped locally; the position did not move.
    Control,
    /// A line that could not be read at all; the hub was told of a gap.
    Gapped,
    /// A record applied (any [`TailOutcome`]); the position moved past it when it had one.
    Applied(TailOutcome),
    /// A record refused as malformed; skipped, with the position moved past it.
    Refused,
    /// A record that could not be applied and may succeed on a retry.
    Failed,
}

/// Tell the hub the stream lost what a line carried: every fed channel is read again.
///
/// The adapter POSTed `{"type":"GAP", ...}`; in-process the checked record is built directly.
async fn ingest_gap(state: &AppState, now_ms: i64) {
    let record = crate::slack::tail::TailRecord::Gap {
        from: None,
        to: None,
    };
    let _ = crate::live_tail::ingest(state, record, now_ms).await;
}

/// Apply one line of the stream, at `now_ms`.
///
/// This is the supervisor's per-record step and the parity tests' seam: blank and control
/// lines, unreadable lines (a gap is ingested), refused records (skipped, position moved
/// past), and applied records (position moved past) behave exactly as the retired adapter
/// plus the HTTP route behaved between them. A record that fails for a retryable reason is
/// [`ApplyOutcome::Failed`] and the position does not move; the supervisor retries, then
/// restarts the stream, as the adapter retried the POST and then exited for its manager.
pub async fn apply_line(
    state: &AppState,
    cursor: &mut TailCursor,
    line: &[u8],
    now_ms: i64,
) -> ApplyOutcome {
    let candidate = trim_line(line);
    if candidate.is_empty() {
        return ApplyOutcome::Blank;
    }
    if candidate.len() > MAX_LINE_BYTES {
        // One record no bound can carry: skipped as a gap, as the adapter skipped it, so the
        // stream does not replay it from the saved position for ever.
        tracing::warn!("a stream line exceeds the record bound; skipped as a gap");
        ingest_gap(state, now_ms).await;
        return ApplyOutcome::Gapped;
    }
    let value: serde_json::Value = match serde_json::from_slice::<serde_json::Value>(candidate) {
        Ok(value) if value.is_object() => value,
        _ => {
            tracing::warn!("a stream line is not a JSON object; skipped as a gap");
            ingest_gap(state, now_ms).await;
            return ApplyOutcome::Gapped;
        }
    };
    let kind = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if CONTROL_TYPES.contains(&kind) {
        return ApplyOutcome::Control;
    }
    let sequence = sequence_of(&value);
    let record = match crate::slack::tail::parse(&value) {
        Ok(record) => record,
        Err(error) => {
            tracing::warn!(
                field = error.field,
                rule = error.rule,
                "refused a stream record as malformed; skipping it"
            );
            return if advance(cursor, sequence) {
                ApplyOutcome::Refused
            } else {
                ApplyOutcome::Failed
            };
        }
    };
    match crate::live_tail::ingest(state, record, now_ms).await {
        Ok(outcome) => {
            if advance(cursor, sequence) {
                ApplyOutcome::Applied(outcome)
            } else {
                ApplyOutcome::Failed
            }
        }
        Err(ChatError::Shape(_)) => {
            tracing::warn!(
                field = "slack_message",
                rule = "is not a message the provider's reads recognize",
                "refused a stream record as malformed; skipping it"
            );
            if advance(cursor, sequence) {
                ApplyOutcome::Refused
            } else {
                ApplyOutcome::Failed
            }
        }
        Err(error) => {
            tracing::warn!(%error, "a stream record could not be applied; the stream will retry it");
            ApplyOutcome::Failed
        }
    }
}

fn trim_line(line: &[u8]) -> &[u8] {
    let mut line = line;
    if line.last() == Some(&b'\n') {
        line = &line[..line.len() - 1];
    }
    if line.last() == Some(&b'\r') {
        line = &line[..line.len() - 1];
    }
    let start = line
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(line.len());
    let end = line
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    &line[start..end]
}

/// Move the saved position past a record. `false` when it could not be saved: the adapter
/// exited there, so the record counts as unapplied and the restarted stream replays it.
fn advance(cursor: &mut TailCursor, sequence: Option<u64>) -> bool {
    match sequence {
        None => true,
        Some(sequence) => match cursor.advance(sequence) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(error, "the stream position could not be saved");
                false
            }
        },
    }
}

/// The conversations to tail now: the fixed list, or the server's own list.
fn conversations(state: &AppState) -> Vec<String> {
    let fixed = &state.config.ingest.tail_channels;
    if !fixed.is_empty() {
        return fixed.clone();
    }
    crate::live_tail::conversations(state)
}

#[cfg(unix)]
fn signal_group(pid: u32, signal: rustix::process::Signal) {
    if let Some(pid) = rustix::process::Pid::from_raw(pid as i32) {
        let _ = rustix::process::kill_process_group(pid, signal);
    }
}

#[cfg(not(unix))]
fn signal_group(_pid: u32, _signal: ()) {}

#[cfg(unix)]
fn group_exists(pid: u32) -> bool {
    rustix::process::Pid::from_raw(pid as i32)
        .is_some_and(|pid| rustix::process::test_kill_process_group(pid).is_ok())
}

/// Terminate a child's whole process group and reap its leader, escalating after a bound.
///
/// The tail command runs a managed runtime with hundreds of threads and children of its own;
/// killing only the leader would leave them holding the subscription. A leader that exits on
/// SIGTERM does not end the matter: the group is polled, and survivors are SIGKILLed, as the
/// adapter polled it.
async fn clean_child(child: &mut tokio::process::Child) {
    let Some(pid) = child.id() else {
        let _ = child.kill().await;
        return;
    };
    #[cfg(unix)]
    signal_group(pid, rustix::process::Signal::TERM);
    #[cfg(not(unix))]
    let _ = child.start_kill();
    let _ = tokio::time::timeout(TERM_GRACE, child.wait()).await;
    #[cfg(unix)]
    {
        let deadline = tokio::time::Instant::now() + TERM_GRACE;
        while group_exists(pid) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        if group_exists(pid) {
            signal_group(pid, rustix::process::Signal::KILL);
        }
    }
    let _ = tokio::time::timeout(TERM_GRACE, child.wait()).await;
}

/// Read and discard the child's stderr, noting only whether it reported an overflow.
///
/// Stderr is never logged: it can carry event content and credentials. Chunks are bounded and
/// the marker scan is over a lower-cased copy of at most one chunk plus a tail, so a line split
/// across chunks is still seen without holding a line of any size.
fn drain_stderr(
    stderr: tokio::process::ChildStderr,
    overflow: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = BufReader::with_capacity(4096, stderr);
        let mut tail: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let mut window = std::mem::take(&mut tail);
                    window.extend_from_slice(&chunk[..read]);
                    let lowered = String::from_utf8_lossy(&window).to_lowercase();
                    if OVERFLOW_MARKERS
                        .iter()
                        .any(|marker| lowered.contains(marker))
                    {
                        overflow.store(true, Ordering::SeqCst);
                    }
                    let keep = window.len().min(64);
                    tail = window[window.len() - keep..].to_vec();
                }
            }
        }
    })
}

/// One bounded line: `Ok(Some(line))`, `Ok(None)` at EOF, or oversize drained to its newline.
///
/// The buffer never holds more than the record bound plus one chunk, however long the line is.
async fn read_bounded_line(
    reader: &mut BufReader<tokio::process::ChildStdout>,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut line: Vec<u8> = Vec::new();
    let mut oversize = false;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(if line.is_empty() && !oversize {
                None
            } else {
                Some(line)
            });
        }
        let (take, done) = match available.iter().position(|byte| *byte == b'\n') {
            Some(index) => (index + 1, true),
            None => (available.len(), false),
        };
        if !oversize {
            if line.len() + take > MAX_LINE_BYTES + 2 {
                oversize = true;
                line.clear();
                line.extend_from_slice(b"__oversize__");
            } else {
                line.extend_from_slice(&available[..take]);
            }
        }
        reader.consume(take);
        if done {
            return Ok(Some(line));
        }
    }
}

/// Why one stream run ended.
#[derive(Debug, PartialEq, Eq)]
enum StreamEnd {
    /// The child closed stdout or exited.
    Ended,
    /// The conversation list changed; restart with the new list.
    Restart,
    /// The stream cannot resume from the saved position.
    Overflow,
    /// A record could not be applied after retries.
    Failed,
    /// The server is shutting down.
    Shutdown,
}

/// Run one stream to its end, applying each record as it arrives.
async fn run_stream(
    state: &AppState,
    cursor: &mut TailCursor,
    channels: &[String],
    shutdown: &mut watch::Receiver<bool>,
    restart: &Arc<AtomicBool>,
    child_pid: &Arc<std::sync::Mutex<Option<u32>>>,
) -> (StreamEnd, bool) {
    let catchup = cursor
        .value()
        .map(|value| value.to_string())
        .unwrap_or_else(|| state.config.ingest.tail_initial_catchup.clone());
    let argv = build_command(&state.config.ingest.tail_command, channels, &catchup);
    tracing::info!(
        conversations = channels.len(),
        resumed = cursor.value().is_some(),
        "tailing conversations from the saved position"
    );
    let mut command = tokio::process::Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            tracing::warn!(%error, "could not start the tail command; will retry");
            return (StreamEnd::Failed, false);
        }
    };
    if let Ok(mut held) = child_pid.lock() {
        *held = child.id();
    }
    if restart.load(Ordering::SeqCst) {
        clean_child(&mut child).await;
        return (StreamEnd::Restart, false);
    }
    let overflow = Arc::new(AtomicBool::new(false));
    let drainer = child
        .stderr
        .take()
        .map(|stderr| drain_stderr(stderr, Arc::clone(&overflow)));
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            clean_child(&mut child).await;
            return (StreamEnd::Failed, false);
        }
    };
    let mut reader = BufReader::with_capacity(8192, stdout);
    let mut applied = false;
    let ended = loop {
        if *shutdown.borrow() {
            break StreamEnd::Shutdown;
        }
        if restart.load(Ordering::SeqCst) {
            break StreamEnd::Restart;
        }
        let line = tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break StreamEnd::Shutdown;
                }
                continue;
            }
            line = read_bounded_line(&mut reader) => line,
        };
        let line = match line {
            Ok(Some(line)) => line,
            Ok(None) => break StreamEnd::Ended,
            Err(_) => break StreamEnd::Failed,
        };
        // One record, retried on a retryable failure exactly as the adapter retried its POST.
        let mut outcome = apply_line(state, cursor, &line, now_ms()).await;
        let mut attempt = 0;
        while outcome == ApplyOutcome::Failed && attempt < RETRY_DELAYS.len() {
            tokio::time::sleep(RETRY_DELAYS[attempt]).await;
            attempt += 1;
            if *shutdown.borrow() {
                break;
            }
            outcome = apply_line(state, cursor, &line, now_ms()).await;
        }
        if outcome == ApplyOutcome::Failed {
            break StreamEnd::Failed;
        }
        if matches!(outcome, ApplyOutcome::Applied(_)) {
            applied = true;
        }
    };
    clean_child(&mut child).await;
    // The drainer ends when the dead child's stderr closes; only then is the overflow flag
    // final, so it is read after cleanup, never raced against it.
    if let Some(drainer) = drainer {
        let _ = tokio::time::timeout(TERM_GRACE, drainer).await;
    }
    if let Ok(mut held) = child_pid.lock() {
        *held = None;
    }
    if restart.load(Ordering::SeqCst) {
        return (StreamEnd::Restart, applied);
    }
    if ended == StreamEnd::Ended && overflow.load(Ordering::SeqCst) {
        return (StreamEnd::Overflow, applied);
    }
    (ended, applied)
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

async fn sleep_or_shutdown(shutdown: &mut watch::Receiver<bool>, pause: Duration) -> bool {
    tokio::select! {
        () = tokio::time::sleep(pause) => false,
        changed = shutdown.changed() => changed.is_err() || *shutdown.borrow(),
    }
}

/// Supervise the tail until the server shuts down.
///
/// Spawn this as a background task when `[ingest] tail_command` is set. It never panics the
/// server: a tail that cannot start, a corrupt state file, or a stream that keeps failing is
/// logged and retried with backoff (or, for a corrupt state file, not retried — replaying from
/// a guessed position would be worse than no tail, and the poller still runs).
pub async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let ingest = &state.config.ingest;
    let Some(state_file) = ingest.tail_state_file.clone() else {
        tracing::warn!("the in-server tail has no state file; not tailing");
        return;
    };
    let mut cursor = TailCursor::new(state_file);
    if let Err(error) = cursor.load() {
        tracing::warn!(
            error,
            "the in-server tail is off: the saved stream position cannot be used"
        );
        return;
    }
    let refresh = Duration::from_secs(ingest.tail_channels_refresh_seconds);
    let restart = Arc::new(AtomicBool::new(false));
    let child_pid: Arc<std::sync::Mutex<Option<u32>>> = Arc::new(std::sync::Mutex::new(None));
    // The channel watcher: only for a dynamic list. A fixed list never changes.
    let watcher = if ingest.tail_channels.is_empty() {
        let state = state.clone();
        let restart = Arc::clone(&restart);
        let child_pid = Arc::clone(&child_pid);
        Some(tokio::spawn(async move {
            let mut held = conversations(&state);
            loop {
                tokio::time::sleep(refresh).await;
                let current = conversations(&state);
                if current != held {
                    tracing::info!(
                        conversations = current.len(),
                        "the conversation list changed; restarting the stream"
                    );
                    restart.store(true, Ordering::SeqCst);
                    #[cfg(unix)]
                    if let Some(pid) = child_pid.lock().ok().and_then(|held| *held) {
                        signal_group(pid, rustix::process::Signal::TERM);
                    }
                    held = current;
                }
            }
        }))
    } else {
        None
    };
    let mut pause = FIRST_PAUSE;
    let mut waiting_logged = false;
    'supervise: loop {
        if *shutdown.borrow() {
            break;
        }
        let channels = conversations(&state);
        if channels.is_empty() {
            if !waiting_logged {
                tracing::info!("no conversation to tail yet; asking again later");
                waiting_logged = true;
            }
            if sleep_or_shutdown(&mut shutdown, refresh.min(pause)).await {
                break;
            }
            pause = (pause * 2).min(MAX_PAUSE);
            continue;
        }
        waiting_logged = false;
        pause = FIRST_PAUSE;
        if restart.swap(false, Ordering::SeqCst) {
            // The watcher fired between streams; fall through with the fresh list.
        }
        let (ended, applied) = run_stream(
            &state,
            &mut cursor,
            &channels,
            &mut shutdown,
            &restart,
            &child_pid,
        )
        .await;
        if applied {
            pause = FIRST_PAUSE;
        }
        match ended {
            StreamEnd::Shutdown => break 'supervise,
            StreamEnd::Restart => {
                pause = FIRST_PAUSE;
                continue 'supervise;
            }
            StreamEnd::Overflow => {
                ingest_gap(&state, now_ms()).await;
                if let Err(error) = cursor.forget() {
                    tracing::warn!(error, "the stream position could not be forgotten");
                }
                tracing::warn!(
                    "the stream could not resume from the saved position; told the hub of the gap \
                     and forgot the position"
                );
            }
            StreamEnd::Ended => {
                tracing::warn!("the event stream ended unexpectedly; restarting it");
            }
            StreamEnd::Failed => {
                tracing::warn!("the event stream failed; restarting it");
            }
        }
        if sleep_or_shutdown(&mut shutdown, pause).await {
            break;
        }
        pause = (pause * 2).min(MAX_PAUSE);
        // A watcher that fired while no stream ran restarts the next stream at once.
        if restart.load(Ordering::SeqCst) {
            pause = FIRST_PAUSE;
        }
    }
    if let Some(watcher) = watcher {
        watcher.abort();
    }
    tracing::info!("the in-server tail stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_filled_argument_by_argument() {
        let template = vec![
            "tail".to_owned(),
            "--channel-id={channels}".to_owned(),
            "--catchup={catchup}".to_owned(),
            "; {channels}".to_owned(),
        ];
        assert_eq!(
            build_command(
                &template,
                &["C0000AAAA1".to_owned(), "C0000BBBB2".to_owned()],
                "41"
            ),
            vec![
                "tail",
                "--channel-id=C0000AAAA1,C0000BBBB2",
                "--catchup=41",
                "; C0000AAAA1,C0000BBBB2"
            ]
        );
    }

    #[test]
    fn the_cursor_moves_only_forward_and_survives_a_reload() {
        let directory =
            std::env::temp_dir().join(format!("live-tail-cursor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let path = directory.join("live-tail.json");
        let mut cursor = TailCursor::new(path.clone());
        cursor.load().expect("missing file loads");
        assert_eq!(cursor.value(), None);
        cursor.advance(9).expect("advance saves");
        cursor.advance(4).expect("backwards is a no-op");
        let mut reloaded = TailCursor::new(path.clone());
        reloaded.load().expect("reload");
        assert_eq!(reloaded.value(), Some(9));
        reloaded.forget().expect("forget saves");
        let mut forgotten = TailCursor::new(path);
        forgotten.load().expect("reload forgotten");
        assert_eq!(forgotten.value(), None);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_corrupt_cursor_is_refused_rather_than_guessed() {
        let directory =
            std::env::temp_dir().join(format!("live-tail-corrupt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("directory");
        let path = directory.join("live-tail.json");
        std::fs::write(&path, "{\"cursor\": -1}").expect("write");
        let mut cursor = TailCursor::new(path);
        assert!(cursor.load().is_err());
        let _ = std::fs::remove_dir_all(&directory);
    }
}
