//! The Claude Code headless wrapper (`#565 headless-coordinator`, milestone M2).
//!
//! `agentctl run-headless NAME` runs this loop in the foreground, normally under a systemd user
//! unit. It owns one persistent `claude -p` stream-json process and is the **only drainer** of
//! the agent's queue: senders only enqueue and wake it through the control socket.
//!
//! Delivery of one queue item, in order:
//!
//! 1. Under the queue's delivery lock, move the item to `inflight/` and stamp the at-most-once
//!    barrier (`possibly_submitted`, `inflight_at`, `paste_tracking`, `inflight_from`) plus
//!    `harness_input_id`, then `paste_started_at`, then write the input line. An item without
//!    `paste_started_at` was provably never written.
//! 2. `command_lifecycle queued` for that ID stamps `harness_queued_at`: the harness has it.
//! 3. `started` means the model has it in context: the item moves to `processed/` with
//!    `started_at`, and only then is `input.started` logged. The queue stamp is authoritative.
//! 4. `cancelled` before `started` returns the item to its lane, unseen. No `queued` within the
//!    acknowledgement deadline moves it to `failed/` as `possibly_submitted`.
//!
//! The next item is written only after the previous one started. Items marked `deliver: now`
//! (owner chat, `send --urgent`) are written mid-turn and start between tool calls;
//! `turn-boundary` items (the default) wait until the running turn ends. Higher `priority`
//! (`urgent`, `normal`, `low`) goes first, then queue order.
//!
//! On start, before draining, an `inflight/` item with `paste_started_at` is looked up in Claude
//! Code's transcript; one the model received moves to `processed/` (`recovered: transcript`),
//! and the rest go through the queue's standard recovery, which never replays a guess. When the
//! harness exits unexpectedly it is restarted with `--resume SESSION`, with backoff, up to a
//! limit; then the wrapper exits nonzero and its supervisor decides.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use agent_queue::fsio::{atomic_json, transition, Blocking};
use agent_queue::queue::{
    self, directory_identity, lock_delivery, pending_in_queue_order, prepare, recover_inflight,
    set_aside, set_aside_reason, unix_seconds, DirectoryIdentity, QueueDirectories,
};
use fs2::FileExt;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::claude::{self, InputState, Signal};
use super::control;
use super::events::{self, EventWriter};

/// File name of the lock a running wrapper holds in the agent directory.
pub const LOCK_FILE: &str = "headless.lock";
/// Exit status when another wrapper already serves the agent.
pub const EXIT_LOCKED: i32 = 75;
/// File in the agent directory naming the running harness process.
pub const HARNESS_FILE: &str = "harness.json";

/// How the wrapper runs one agent.
#[derive(Clone, Debug)]
pub struct WrapperConfig {
    /// Agent name.
    pub agent: String,
    /// The agent's registry directory, holding `queue/`, `events/` and the control socket.
    pub agent_directory: PathBuf,
    /// Working directory of the harness.
    pub cwd: PathBuf,
    /// Harness session identifier (a UUID).
    pub session: String,
    /// Harness command: the executable and any arguments before the protocol arguments.
    pub command: Vec<String>,
    /// Operator arguments after the protocol and session arguments (model, settings,
    /// permission mode, tool rules).
    pub arguments: Vec<String>,
    /// Longest wait for `queued` after writing an input, and for `started` after the harness
    /// went idle with the input still queued.
    pub ack_deadline: Duration,
    /// Time a newly started harness gets before the acknowledgement clock of its first inputs
    /// runs: startup (hooks, MCP servers, loading a long transcript) reads no input.
    pub startup_grace: Duration,
    /// Harness restarts allowed within `restart_window` before the wrapper gives up.
    pub max_restarts: u32,
    /// Window over which restarts are counted.
    pub restart_window: Duration,
    /// Longest wait for the harness to exit after its input is closed at stop.
    pub stop_timeout: Duration,
    /// Claude Code configuration directory, for transcript lookups; `None` disables them.
    pub config_directory: Option<PathBuf>,
}

impl WrapperConfig {
    /// Defaults for `agent` in `agent_directory`, running `claude` in `cwd`.
    #[must_use]
    pub fn new(agent: &str, agent_directory: &Path, cwd: &Path, session: &str) -> Self {
        Self {
            agent: agent.to_owned(),
            agent_directory: agent_directory.to_path_buf(),
            cwd: cwd.to_path_buf(),
            session: session.to_owned(),
            command: vec!["claude".to_owned()],
            arguments: Vec::new(),
            ack_deadline: Duration::from_secs(30),
            startup_grace: Duration::from_secs(120),
            max_restarts: 5,
            restart_window: Duration::from_secs(600),
            stop_timeout: Duration::from_secs(60),
            config_directory: claude::config_directory(),
        }
    }

    fn transcript(&self) -> Option<PathBuf> {
        self.config_directory
            .as_deref()
            .map(|config| claude::transcript_path(&self.cwd, &self.session, config))
    }
}

/// What the wrapper tells its owner about the harness processes it starts.
pub trait Hooks {
    /// A harness process started with this PID; an error is logged and does not stop the
    /// wrapper.
    fn harness_spawned(&self, pid: u32) -> std::result::Result<(), String>;
}

/// Hooks that record nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoHooks;

impl Hooks for NoHooks {
    fn harness_spawned(&self, _pid: u32) -> std::result::Result<(), String> {
        Ok(())
    }
}

/// Delivery order and policy of one queue item.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Priority {
    Urgent,
    Normal,
    Low,
}

fn priority_of(document: &Map<String, Value>) -> Priority {
    match document.get("priority").and_then(Value::as_str) {
        Some("urgent") => Priority::Urgent,
        Some("low") => Priority::Low,
        _ => Priority::Normal,
    }
}

fn priority_name(priority: Priority) -> &'static str {
    match priority {
        Priority::Urgent => "urgent",
        Priority::Normal => "normal",
        Priority::Low => "low",
    }
}

/// `now` when the item asks for it, or is urgent and does not ask otherwise.
fn delivers_now(document: &Map<String, Value>) -> bool {
    match document.get("deliver").and_then(Value::as_str) {
        Some("now") => true,
        Some(_) => false,
        None => priority_of(document) == Priority::Urgent,
    }
}

/// An input written to the harness that has not started yet.
#[derive(Debug)]
struct Written {
    queue_id: String,
    uuid: String,
    inflight: PathBuf,
    lane: PathBuf,
    document: Map<String, Value>,
    written_at: Instant,
    queued_at: Option<Instant>,
}

struct Harness {
    child: Child,
    /// Lines for the writer thread, which owns the harness's stdin, so a harness that stops
    /// reading never blocks the main loop. Dropping it closes the harness's input.
    input: Option<Sender<Vec<u8>>>,
    instance: u64,
    initialized: bool,
    spawned_at: Instant,
    /// When the process was seen to exit; its output may still be draining.
    exited_at: Option<Instant>,
}

impl Drop for Harness {
    /// A wrapper that leaves on an error must not orphan its harness, which runs in its own
    /// process group: end it the way a stop deadline does.
    fn drop(&mut self) {
        drop(self.input.take());
        if matches!(self.child.try_wait(), Ok(None)) {
            terminate_group(&mut self.child);
        }
    }
}

enum Message {
    Line(u64, String),
    Eof(u64),
    Control(control::Request),
}

fn control_message(request: control::Request) -> Message {
    Message::Control(request)
}

fn fail(message: impl Into<String>) -> crate::agent::AgentError {
    crate::agent::AgentError::Delivery(message.into())
}

type Result<T> = std::result::Result<T, crate::agent::AgentError>;

fn io_fail(action: &str, error: impl std::fmt::Display) -> crate::agent::AgentError {
    fail(format!("{action}: {error}"))
}

/// The running wrapper.
struct Wrapper {
    config: WrapperConfig,
    writer: EventWriter,
    queue_root: PathBuf,
    directories: QueueDirectories,
    queue_identity: DirectoryIdentity,
    sender: Sender<Message>,
    harness: Option<Harness>,
    next_instance: u64,
    busy: bool,
    turn_causation: Option<String>,
    turn_started: Option<Instant>,
    current: Option<Written>,
    started_inputs: HashMap<String, String>,
    tools: HashMap<String, (String, Instant)>,
    pending_controls: HashMap<String, SyncSender<Value>>,
    next_request: u64,
    interrupt_requested: bool,
    stopping: Option<String>,
    stop_deadline: Option<Instant>,
    status: &'static str,
    rescan: bool,
    last_scan: Instant,
    pause_until: Option<Instant>,
    restarts: Vec<Instant>,
    restart_at: Option<Instant>,
    /// The harness died inside a turn: tell the agent once the next harness runs.
    restart_notice: bool,
    /// When the harness last finished a turn or started; the clock for an input that was
    /// queued but never started.
    idle_since: Instant,
}

impl Wrapper {
    fn emit(&mut self, kind: &str, causation: Option<&str>, data: Value) -> Result<u64> {
        self.writer
            .append(kind, causation, data)
            .map_err(|error| io_fail("write event log", error))
    }

    fn set_status(&mut self, status: &'static str) -> Result<()> {
        if self.status != status {
            self.status = status;
            self.emit("status", None, json!({"status": status}))?;
        }
        Ok(())
    }

    fn spawn_harness(&mut self, hooks: &dyn Hooks) -> Result<()> {
        let resume = self.config.transcript().is_some_and(|path| path.is_file());
        let mut argv: Vec<String> = self.config.command[1..].to_vec();
        argv.extend(claude::protocol_arguments());
        if resume {
            argv.extend(["--resume".to_owned(), self.config.session.clone()]);
        } else {
            argv.extend(["--session-id".to_owned(), self.config.session.clone()]);
        }
        argv.extend(claude::effective_arguments(&self.config.arguments));
        let mut command = Command::new(&self.config.command[0]);
        command
            .args(&argv)
            .current_dir(&self.config.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .env("AGENTCTL_AGENT", &self.config.agent)
            .process_group(0);
        let mut child = command
            .spawn()
            .map_err(|error| io_fail(&format!("start {}", self.config.command[0]), error))?;
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stdin = child.stdin.take().expect("piped stdin");
        self.next_instance += 1;
        let instance = self.next_instance;
        let sender = self.sender.clone();
        let pid = child.id();
        // Own the child before anything else can fail, so an error ends it (see `Drop`).
        let (input, lines) = mpsc::channel::<Vec<u8>>();
        self.harness = Some(Harness {
            child,
            input: Some(input),
            instance,
            initialized: false,
            spawned_at: Instant::now(),
            exited_at: None,
        });
        thread::Builder::new()
            .name("headless-harness-stdout".to_owned())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut buffer = Vec::new();
                loop {
                    buffer.clear();
                    match reader.read_until(b'\n', &mut buffer) {
                        Ok(0) => break,
                        Ok(_) => {
                            // A line that is not UTF-8 is still a line, not the end of output.
                            let line = String::from_utf8_lossy(&buffer)
                                .trim_end_matches(['\n', '\r'])
                                .to_owned();
                            if sender.send(Message::Line(instance, line)).is_err() {
                                return;
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
                let _ = sender.send(Message::Eof(instance));
            })
            .map_err(|error| io_fail("start harness reader", error))?;
        thread::Builder::new()
            .name("headless-harness-stdin".to_owned())
            .spawn(move || {
                for line in lines {
                    if stdin.write_all(&line).and_then(|()| stdin.flush()).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| io_fail("start harness writer", error))?;
        self.record_harness(pid);
        let digest = format!(
            "{:x}",
            Sha256::digest(self.config.command.join("\0") + "\0" + &argv.join("\0"))
        );
        self.emit(
            "lifecycle.harness_spawned",
            None,
            json!({"pid": pid, "resume": resume, "argv_sha256": digest}),
        )?;
        if let Err(message) = hooks.harness_spawned(pid) {
            self.emit(
                "error",
                None,
                json!({"class": "record_harness_identity", "message": message}),
            )?;
        }
        self.rescan = true;
        Ok(())
    }

    /// Settle `inflight/` items before the first drain: proven by the transcript, returned to
    /// their lane when provably never written, or quarantined.
    fn recover(&mut self) -> Result<()> {
        let _lock = lock_delivery(&self.queue_root, self.queue_identity, &Blocking)?;
        let transcript = self.config.transcript();
        for path in queue::json_paths(&self.directories.inflight)? {
            let Ok(mut document) = queue::load_message(&path) else {
                continue;
            };
            let uuid = document
                .get("harness_input_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let written = document.contains_key("paste_started_at");
            let proven = match (&uuid, &transcript) {
                (Some(uuid), Some(transcript)) if written => {
                    claude::transcript_received(transcript, uuid)
                }
                _ => false,
            };
            if !proven {
                continue;
            }
            let queue_id = queue::message_id_from_path(&path)?;
            strip_stamps(&mut document);
            document.insert("delivery_state".to_owned(), json!("processed"));
            document.insert("confirmed_at".to_owned(), json!(unix_seconds()));
            document.insert("recovered".to_owned(), json!("transcript"));
            atomic_json(&path, &Value::Object(document))?;
            let filename = path.file_name().expect("listed path has a file name");
            transition(&path, &self.directories.processed.join(filename))?;
            self.emit(
                "input.started",
                Some(&queue_id),
                json!({"queue_id": queue_id, "harness_input_id": uuid, "recovered": "transcript"}),
            )?;
        }
        for queue_id in recover_inflight(&self.directories)? {
            self.emit(
                "error",
                Some(&queue_id),
                json!({"class": "possibly_submitted", "queue_id": queue_id,
                       "message": "an input written before a restart is not in the transcript; quarantined, never replayed"}),
            )?;
        }
        Ok(())
    }

    /// The pending items in delivery order, each with its document. Unreadable items are
    /// quarantined as `invalid_message`.
    fn pending(&mut self) -> Result<Vec<(PathBuf, Map<String, Value>)>> {
        let mut items = Vec::new();
        for path in pending_in_queue_order(&self.directories)? {
            match queue::load_message(&path) {
                Ok(document)
                    if document
                        .get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.is_empty()) =>
                {
                    items.push((path, document));
                }
                Ok(_) | Err(_) => {
                    let _lock = lock_delivery(&self.queue_root, self.queue_identity, &Blocking)?;
                    if path.exists() {
                        let error = "queued item is not an object with a nonempty text";
                        let identifier = queue::quarantine_raw(
                            &path,
                            &self.directories.failed,
                            "invalid_message",
                            error,
                        )?;
                        self.emit(
                            "error",
                            Some(&identifier),
                            json!({"class": "invalid_message", "queue_id": identifier, "message": error}),
                        )?;
                    }
                }
            }
        }
        items.sort_by_key(|(_, document)| priority_of(document));
        Ok(items)
    }

    fn maybe_deliver(&mut self) -> Result<()> {
        if self.current.is_some()
            || self.stopping.is_some()
            || self.pause_until.is_some_and(|until| Instant::now() < until)
            || self
                .harness
                .as_ref()
                .is_none_or(|harness| harness.input.is_none())
        {
            return Ok(());
        }
        self.pause_until = None;
        for (path, document) in self.pending()? {
            if self.busy && !delivers_now(&document) {
                continue;
            }
            if self.deliver(&path, document)? {
                return Ok(());
            }
        }
        Ok(())
    }

    /// Hand one pending item to the harness. Returns false when it was set aside or vanished.
    fn deliver(&mut self, path: &Path, mut document: Map<String, Value>) -> Result<bool> {
        let lock = lock_delivery(&self.queue_root, self.queue_identity, &Blocking)?;
        if !path.exists() {
            return Ok(false);
        }
        let queue_id = queue::message_id_from_path(path)?;
        let filename = path.file_name().expect("queued path has a file name");
        let inflight = self.directories.inflight.join(filename);
        transition(path, &inflight)?;
        if let Some((reason, detail)) =
            set_aside_reason(&self.directories, &queue_id, &document, queue::now_millis())
        {
            set_aside(
                &self.queue_root,
                &self.directories,
                &inflight,
                document,
                reason,
                &detail,
            )?;
            return Ok(false);
        }
        let lane = if path.starts_with(&self.directories.inbox_v2) {
            "inbox.v2"
        } else {
            "inbox"
        };
        // A fresh ID per attempt: a returned or reused queue ID never matches an older input in
        // the transcript, which recovery reads as proof.
        let uuid = claude::random_uuid().map_err(|error| io_fail("make an input ID", error))?;
        let attempts = queue::delivery_attempts(&document, path).unwrap_or(0);
        document.insert("possibly_submitted".to_owned(), json!(true));
        document.insert("delivery_state".to_owned(), json!("inflight"));
        document.insert("inflight_at".to_owned(), json!(unix_seconds()));
        document.insert("paste_tracking".to_owned(), json!(true));
        document.insert("inflight_from".to_owned(), json!(lane));
        document.insert("harness_input_id".to_owned(), json!(uuid));
        document.insert("delivery_attempts".to_owned(), json!(attempts + 1));
        document.remove("paste_started_at");
        atomic_json(&inflight, &Value::Object(document.clone()))?;
        document.insert("paste_started_at".to_owned(), json!(unix_seconds()));
        atomic_json(&inflight, &Value::Object(document.clone()))?;
        drop(lock);
        let text = document
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let line = claude::input_line(&uuid, &text, &self.config.session);
        let wrote = self
            .harness
            .as_ref()
            .and_then(|harness| harness.input.as_ref())
            .map(|input| input.send(line.into_bytes()));
        if let Some(Err(error)) = wrote {
            // The harness is going away; recovery settles the item from its transcript.
            self.emit(
                "error",
                Some(&queue_id),
                json!({"class": "harness_write", "queue_id": queue_id, "message": error.to_string()}),
            )?;
        }
        self.current = Some(Written {
            queue_id,
            uuid,
            inflight,
            lane: path.to_path_buf(),
            document,
            written_at: Instant::now(),
            queued_at: None,
        });
        Ok(true)
    }

    fn input_signal(&mut self, uuid: &str, state: InputState) -> Result<()> {
        let is_current = self
            .current
            .as_ref()
            .is_some_and(|current| current.uuid == uuid);
        match state {
            InputState::Queued if is_current => {
                let current = self.current.as_mut().expect("current input");
                current.queued_at = Some(Instant::now());
                current
                    .document
                    .insert("harness_queued_at".to_owned(), json!(unix_seconds()));
                atomic_json(&current.inflight, &Value::Object(current.document.clone()))?;
                let document = &current.document;
                let data = json!({
                    "queue_id": current.queue_id,
                    "harness_input_id": uuid,
                    "from": document.get("from").and_then(Value::as_str).unwrap_or("agentctl"),
                    "priority": priority_name(priority_of(document)),
                    "deliver": if delivers_now(document) { "now" } else { "turn-boundary" },
                    "excerpt": events::excerpt(
                        document.get("text").and_then(Value::as_str).unwrap_or_default(),
                        events::EXCERPT_BYTES,
                    ),
                });
                let queue_id = current.queue_id.clone();
                self.emit("input.queued", Some(&queue_id), data)?;
            }
            InputState::Started if is_current => {
                let mut current = self.current.take().expect("current input");
                let mid_turn = self.busy;
                strip_stamps(&mut current.document);
                current
                    .document
                    .insert("delivery_state".to_owned(), json!("processed"));
                current
                    .document
                    .insert("started_at".to_owned(), json!(unix_seconds()));
                current
                    .document
                    .insert("confirmed_at".to_owned(), json!(unix_seconds()));
                current
                    .document
                    .insert("mid_turn".to_owned(), json!(mid_turn));
                {
                    let _lock = lock_delivery(&self.queue_root, self.queue_identity, &Blocking)?;
                    atomic_json(&current.inflight, &Value::Object(current.document.clone()))?;
                    let filename = current
                        .inflight
                        .file_name()
                        .expect("inflight path has a file name")
                        .to_owned();
                    transition(
                        &current.inflight,
                        &self.directories.processed.join(filename),
                    )?;
                }
                if !self.busy {
                    self.begin_turn("input", Some(current.queue_id.clone()))?;
                }
                self.emit(
                    "input.started",
                    Some(&current.queue_id),
                    json!({"queue_id": current.queue_id, "harness_input_id": uuid, "mid_turn": mid_turn}),
                )?;
                self.started_inputs
                    .insert(uuid.to_owned(), current.queue_id.clone());
                self.rescan = true;
            }
            InputState::Cancelled if is_current => {
                let mut current = self.current.take().expect("current input");
                strip_stamps(&mut current.document);
                current.document.remove("possibly_submitted");
                current.document.remove("inflight_at");
                current.document.remove("harness_queued_at");
                current
                    .document
                    .insert("delivery_state".to_owned(), json!("pending"));
                {
                    let _lock = lock_delivery(&self.queue_root, self.queue_identity, &Blocking)?;
                    atomic_json(&current.inflight, &Value::Object(current.document.clone()))?;
                    transition(&current.inflight, &current.lane)?;
                }
                self.emit(
                    "input.cancelled",
                    Some(&current.queue_id),
                    json!({"queue_id": current.queue_id, "harness_input_id": uuid, "before_start": true}),
                )?;
                // Let the cancelling interrupt finish before the item is offered again.
                self.pause_until = Some(Instant::now() + Duration::from_secs(2));
            }
            InputState::Completed | InputState::Cancelled => {
                let queue_id = self.started_inputs.remove(uuid);
                let kind = if state == InputState::Completed {
                    "input.completed"
                } else {
                    "input.cancelled"
                };
                self.emit(
                    kind,
                    queue_id.as_deref(),
                    json!({"queue_id": queue_id, "harness_input_id": uuid, "before_start": false}),
                )?;
            }
            InputState::Queued | InputState::Started => {
                // An input this generation did not write, such as one recovered from the
                // transcript: log it without a queue item.
                if state == InputState::Started && !self.busy {
                    self.begin_turn("input", None)?;
                }
                if state == InputState::Started {
                    self.emit(
                        "input.started",
                        None,
                        json!({"queue_id": null, "harness_input_id": uuid, "mid_turn": self.busy}),
                    )?;
                }
            }
        }
        Ok(())
    }

    fn begin_turn(&mut self, cause: &str, causation: Option<String>) -> Result<()> {
        self.busy = true;
        self.turn_started = Some(Instant::now());
        self.turn_causation = causation;
        self.writer.begin_turn();
        let causation = self.turn_causation.clone();
        self.emit(
            "turn.started",
            causation.as_deref(),
            json!({"cause": cause}),
        )?;
        self.set_status("busy")
    }

    fn harness_line(&mut self, line: &str) -> Result<()> {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return Ok(());
        };
        for signal in claude::parse(&value) {
            let causation = self.turn_causation.clone();
            let causation = causation.as_deref();
            match signal {
                Signal::Input { uuid, state } => self.input_signal(&uuid, state)?,
                Signal::Init {
                    version,
                    model,
                    permission_mode,
                    session,
                    tools,
                } => {
                    let first = self
                        .harness
                        .as_mut()
                        .is_some_and(|harness| !std::mem::replace(&mut harness.initialized, true));
                    if first {
                        self.emit(
                            "lifecycle.harness",
                            None,
                            json!({"harness": "claude", "version": version, "model": model,
                                   "permission_mode": permission_mode, "session": session, "tools": tools}),
                        )?;
                    }
                    if !self.busy {
                        self.begin_turn("self", None)?;
                    }
                }
                Signal::Text(text) => {
                    self.emit("assistant.text", causation, json!({"text": text}))?;
                }
                Signal::ToolStarted { id, tool, summary } => {
                    self.tools
                        .insert(id.clone(), (tool.clone(), Instant::now()));
                    self.emit(
                        "tool.started",
                        causation,
                        json!({"tool": tool, "summary": summary, "tool_use_id": id}),
                    )?;
                }
                Signal::ToolCompleted { id, ok, bytes } => {
                    let (tool, began) = self
                        .tools
                        .remove(&id)
                        .map_or((String::new(), None), |(tool, began)| (tool, Some(began)));
                    let duration_ms = began.map(|began| began.elapsed().as_millis() as u64);
                    self.emit(
                        "tool.completed",
                        causation,
                        json!({"tool": tool, "tool_use_id": id, "ok": ok, "output_bytes": bytes,
                               "duration_ms": duration_ms}),
                    )?;
                }
                Signal::Result {
                    subtype,
                    duration_ms,
                    usage,
                    cost_usd,
                    denials,
                } => {
                    for (tool, summary) in denials {
                        self.emit(
                            "permission.denied",
                            causation,
                            json!({"tool": tool, "summary": summary}),
                        )?;
                    }
                    let outcome = match subtype.as_str() {
                        "success" => "completed",
                        "error_during_execution" if self.interrupt_requested => "interrupted",
                        _ => "error",
                    };
                    let wall = self
                        .turn_started
                        .map(|began| began.elapsed().as_millis() as u64);
                    self.emit(
                        "turn.completed",
                        causation,
                        json!({"outcome": outcome, "subtype": subtype, "usage": usage,
                               "cost_usd": cost_usd, "duration_ms": duration_ms.or(wall)}),
                    )?;
                    self.busy = false;
                    self.interrupt_requested = false;
                    self.turn_causation = None;
                    self.turn_started = None;
                    self.idle_since = Instant::now();
                    self.tools.clear();
                    self.set_status("idle")?;
                    self.rescan = true;
                }
                Signal::CompactStarted => {
                    self.emit("compact.started", causation, json!({}))?;
                }
                Signal::CompactCompleted {
                    trigger,
                    pre_tokens,
                    post_tokens,
                } => {
                    self.emit(
                        "compact.completed",
                        causation,
                        json!({"trigger": trigger, "pre_tokens": pre_tokens, "post_tokens": post_tokens}),
                    )?;
                }
                Signal::ControlResponse { request_id, body } => {
                    if let Some(reply) = self.pending_controls.remove(&request_id) {
                        let _ = reply.try_send(
                            json!({"ok": true, "request_id": request_id, "response": body}),
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn control(&mut self, request: control::Request) -> Result<()> {
        let op = request.body["op"].as_str().unwrap_or("").to_owned();
        let answer = match op.as_str() {
            "wake" => {
                self.rescan = true;
                json!({"ok": true})
            }
            "status" => json!({
                "ok": true,
                "status": self.status,
                "busy": self.busy,
                "turn": self.writer.turn(),
                "generation": self.writer.generation(),
                "session": self.config.session,
                "wrapper_pid": std::process::id(),
                "harness_pid": self.harness.as_ref().map(|harness| harness.child.id()),
                "current": self.current.as_ref().map(|current| current.queue_id.clone()),
                "last_seq": self.writer.next_seq().saturating_sub(1),
                "stopping": self.stopping,
            }),
            "interrupt" => {
                let input = self
                    .harness
                    .as_ref()
                    .and_then(|harness| harness.input.clone());
                match input {
                    Some(input) => {
                        self.next_request += 1;
                        let request_id = format!("agentctl-interrupt-{}", self.next_request);
                        let line = claude::interrupt_line(&request_id);
                        match input.send(line.into_bytes()) {
                            Ok(()) => {
                                // Only a running turn can end interrupted.
                                self.interrupt_requested = self.busy;
                                let causation = self.turn_causation.clone();
                                self.emit(
                                    "interrupt.requested",
                                    causation.as_deref(),
                                    json!({"request_id": request_id, "busy": self.busy}),
                                )?;
                                self.pending_controls.insert(request_id, request.reply);
                                return Ok(());
                            }
                            Err(error) => {
                                json!({"ok": false, "error": format!("harness input closed: {error}")})
                            }
                        }
                    }
                    None => json!({"ok": false, "error": "no harness is running"}),
                }
            }
            "stop" => {
                if self.stopping.is_none() {
                    self.stopping = Some("stop-request".to_owned());
                }
                json!({"ok": true, "stopping": true})
            }
            other => json!({"ok": false, "error": format!("unknown op {other:?}")}),
        };
        let _ = request.reply.try_send(answer);
        Ok(())
    }

    fn check_ack_deadline(&mut self) -> Result<()> {
        let Some(current) = self.current.as_ref() else {
            return Ok(());
        };
        let now = Instant::now();
        let error = match current.queued_at {
            None => {
                // The clock starts once the harness has had its startup grace.
                let grace_end = self.harness.as_ref().map_or(current.written_at, |harness| {
                    harness.spawned_at + self.config.startup_grace
                });
                let start = current.written_at.max(grace_end);
                (now > start + self.config.ack_deadline).then(|| {
                    format!(
                        "the harness did not acknowledge the input within {} s",
                        self.config.ack_deadline.as_secs()
                    )
                })
            }
            Some(queued_at) => {
                // Queued but not started while the harness sits idle: it will not start it.
                let since = queued_at.max(self.idle_since);
                (!self.busy && now > since + self.config.ack_deadline).then(|| {
                    format!(
                        "the harness queued the input but stayed idle for {} s without starting it",
                        self.config.ack_deadline.as_secs()
                    )
                })
            }
        };
        let Some(error) = error else {
            return Ok(());
        };
        let mut current = self.current.take().expect("current input");
        strip_stamps(&mut current.document);
        current
            .document
            .insert("delivery_failed_at".to_owned(), json!(unix_seconds()));
        current
            .document
            .insert("delivery_error".to_owned(), json!(error));
        {
            let _lock = lock_delivery(&self.queue_root, self.queue_identity, &Blocking)?;
            atomic_json(&current.inflight, &Value::Object(current.document.clone()))?;
            let filename = current
                .inflight
                .file_name()
                .expect("inflight path has a file name")
                .to_owned();
            let failed = self.directories.failed.join(filename);
            transition(&current.inflight, &failed)?;
            queue::failed_metadata(&failed, "possibly_submitted", &error)?;
        }
        self.emit(
            "error",
            Some(&current.queue_id),
            json!({"class": "possibly_submitted", "queue_id": current.queue_id, "message": error}),
        )?;
        Ok(())
    }

    /// The harness's output ended: reap it, then restart it or give up.
    fn harness_exited(&mut self) -> Result<Option<i32>> {
        let Some(mut harness) = self.harness.take() else {
            return Ok(None);
        };
        drop(harness.input.take());
        let status = wait_bounded(&mut harness.child, Duration::from_secs(10));
        let code = status.and_then(|status| status.code());
        drop(harness);
        self.clear_harness_record();
        if self.busy && self.stopping.is_none() {
            self.restart_notice = true;
        }
        self.interrupt_requested = false;
        self.busy = false;
        self.turn_causation = None;
        self.tools.clear();
        self.started_inputs.clear();
        // An input written but not started stays in inflight/; recovery settles it from the
        // transcript before the next harness gets anything.
        self.current = None;
        for (_, reply) in self.pending_controls.drain() {
            let _ = reply.try_send(json!({"ok": false, "error": "the harness exited"}));
        }
        if let Some(cause) = self.stopping.clone() {
            self.emit(
                "lifecycle.harness_exited",
                None,
                json!({"code": code, "next": "stopping"}),
            )?;
            self.set_status("stopped")?;
            self.emit(
                "lifecycle.stopped",
                None,
                json!({"cause": cause, "harness_code": code}),
            )?;
            return Ok(Some(0));
        }
        let now = Instant::now();
        self.restarts
            .retain(|at| now.duration_since(*at) < self.config.restart_window);
        if self.restarts.len() as u32 >= self.config.max_restarts {
            self.emit(
                "lifecycle.harness_exited",
                None,
                json!({"code": code, "next": format!("giving up after {} restarts in {} s",
                        self.restarts.len(), self.config.restart_window.as_secs())}),
            )?;
            self.set_status("stopped")?;
            self.emit(
                "lifecycle.stopped",
                None,
                json!({"cause": "restart-limit", "harness_code": code}),
            )?;
            return Ok(Some(1));
        }
        let delay = Duration::from_secs(2u64.saturating_mul(1 + self.restarts.len() as u64));
        self.restarts.push(now);
        self.restart_at = Some(now + delay);
        self.emit(
            "lifecycle.harness_exited",
            None,
            json!({"code": code, "next": format!("restarting with --resume in {} s", delay.as_secs())}),
        )?;
        self.set_status("unreachable")?;
        Ok(None)
    }

    fn harness_record_path(&self) -> PathBuf {
        self.config.agent_directory.join(HARNESS_FILE)
    }

    /// Remember the running harness's kernel identity, so a later wrapper refuses to start a
    /// second harness while this one lives (for example after this wrapper was killed).
    fn record_harness(&mut self, pid: u32) {
        let identity = agent_queue::identity::observe(u64::from(pid))
            .ok()
            .and_then(|live| serde_json::to_value(live.identity).ok());
        if let Some(identity) = identity {
            let _ = atomic_json(
                &self.harness_record_path(),
                &json!({"schema": "agentctl-headless-harness/v1", "identity": identity}),
            );
        }
    }

    fn clear_harness_record(&self) {
        let _ = std::fs::remove_file(self.harness_record_path());
    }

    /// Queue one notice for the agent, at a turn boundary, unless one with this ID exists.
    fn enqueue_notice(&mut self, id: &str, text: &str) -> Result<()> {
        let enqueued = queue::enqueue_into(
            &self.queue_root,
            queue::Lane::InboxV2,
            Some(id),
            true,
            &Blocking,
            |identifier| {
                json!({"id": identifier, "text": text, "queued_at": unix_seconds(),
                       "delivery_attempts": 0, "from": "wrapper", "priority": "normal",
                       "deliver": "turn-boundary"})
            },
        );
        if let Err(error) = enqueued {
            self.emit(
                "error",
                None,
                json!({"class": "restart_notice", "message": error.to_string()}),
            )?;
        }
        self.rescan = true;
        Ok(())
    }

    fn restart_text(&self, what: &str) -> String {
        let last_seq = self.writer.next_seq().saturating_sub(1);
        format!(
            "[agentctl] {what} at {}; the conversation was resumed, but the turn in progress was cut short. Last event before the restart: events#{last_seq} (agentctl tail {} --at {last_seq}). Re-read your records before continuing.",
            iso_now(),
            self.config.agent,
        )
    }

    /// A harness whose process exited while a descendant still holds its output open sends no
    /// end of output: treat it as exited a few seconds after the process is gone.
    fn harness_gone_without_eof(&mut self) -> bool {
        let Some(harness) = self.harness.as_mut() else {
            return false;
        };
        if harness.exited_at.is_none() && matches!(harness.child.try_wait(), Ok(Some(_))) {
            harness.exited_at = Some(Instant::now());
        }
        harness
            .exited_at
            .is_some_and(|at| at.elapsed() > Duration::from_secs(5))
    }

    fn begin_stop_if_requested(&mut self, terminate: &AtomicBool) {
        if terminate.load(Ordering::SeqCst) && self.stopping.is_none() {
            self.stopping = Some("signal".to_owned());
        }
        if self.stopping.is_some() && self.stop_deadline.is_none() {
            self.stop_deadline = Some(Instant::now() + self.config.stop_timeout);
            if let Some(harness) = self.harness.as_mut() {
                // Closing its input lets the harness finish what it holds, then exit.
                drop(harness.input.take());
            }
        }
    }

    fn enforce_stop_deadline(&mut self) {
        let Some(deadline) = self.stop_deadline else {
            return;
        };
        if Instant::now() < deadline {
            return;
        }
        if let Some(harness) = self.harness.as_mut() {
            terminate_group(&mut harness.child);
        }
        self.stop_deadline = Some(Instant::now() + Duration::from_secs(3600));
    }
}

/// Remove the recovery stamps that only an `inflight/` item carries.
fn strip_stamps(document: &mut Map<String, Value>) {
    document.remove("paste_tracking");
    document.remove("inflight_from");
    document.remove("paste_started_at");
}

fn wait_bounded(child: &mut Child, limit: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                terminate_group(child);
                return child.wait().ok();
            }
            Err(_) => return None,
        }
    }
}

/// SIGTERM the harness's process group, then SIGKILL it after five seconds.
fn terminate_group(child: &mut Child) {
    let Ok(pid) = i32::try_from(child.id()) else {
        return;
    };
    // SAFETY: signalling our own child's process group, which `process_group(0)` created.
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    // SAFETY: as above.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
}

/// The PID of a harness a previous wrapper started that is still alive, if any.
fn live_previous_harness(agent_directory: &Path) -> Option<u64> {
    let document = agent_queue::fsio::read_json(&agent_directory.join(HARNESS_FILE)).ok()?;
    let identity: agent_queue::identity::ProcessIdentity =
        serde_json::from_value(document.get("identity")?.clone()).ok()?;
    (agent_queue::identity::liveness(&identity) == agent_queue::identity::ProcessLiveness::Alive)
        .then_some(identity.pid)
}

/// Whether the previous generation ended without a clean, idle stop.
fn previous_ended_uncleanly(events_dir: &Path, generation: u64) -> bool {
    let Ok(files) = events::generation_files(events_dir) else {
        return false;
    };
    let Some((_, path)) = files.iter().rev().find(|(number, _)| *number < generation) else {
        return false;
    };
    let Ok((events, _, _)) = events::read_from(path, 0) else {
        return false;
    };
    let busy_at_end = events
        .iter()
        .rev()
        .find(|event| event.kind == "turn.started" || event.kind == "turn.completed")
        .is_some_and(|event| event.kind == "turn.started");
    let clean = events.last().is_some_and(|event| {
        event.kind == "lifecycle.stopped" && event.data["cause"] != "restart-limit"
    });
    !clean || busy_at_end
}

/// Take the agent's headless lock without waiting; `None` when another wrapper holds it.
pub fn try_lock(agent_directory: &Path) -> Result<Option<File>> {
    let path = agent_directory.join(LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&path)
        .map_err(|error| io_fail(&format!("open {}", path.display()), error))?;
    let mut attempts = 0;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(Some(file)),
            Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                return Ok(None)
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted && attempts < 5 => {
                attempts += 1;
            }
            Err(error) => return Err(io_fail(&format!("lock {}", path.display()), error)),
        }
    }
}

/// Run the wrapper until it is stopped (exit 0), gives up (1), or finds another wrapper
/// running (75). `terminate` is set by SIGTERM or SIGINT; `hooks` hears of every harness
/// process started.
pub fn run(config: WrapperConfig, terminate: Arc<AtomicBool>, hooks: &dyn Hooks) -> Result<i32> {
    if config.command.is_empty() {
        return Err(fail("the harness command is empty"));
    }
    let Some(lock) = try_lock(&config.agent_directory)? else {
        eprintln!(
            "agentctl run-headless: another wrapper already serves {}; refusing to start a second harness",
            config.agent
        );
        return Ok(EXIT_LOCKED);
    };
    if let Some(pid) = live_previous_harness(&config.agent_directory) {
        eprintln!(
            "agentctl run-headless: the harness a previous wrapper of {} started (pid {pid}) is still running; stop it before starting another on the same session",
            config.agent
        );
        return Ok(EXIT_LOCKED);
    }
    let queue_root = config.agent_directory.join("queue");
    let directories = prepare(&queue_root)?;
    queue::prepare_lane(&queue_root, &directories.inbox_v2)?;
    let queue_identity = directory_identity(&queue_root)?;
    let events_dir = events::directory(&config.agent_directory);
    // Judged before the writer prunes old generations at open.
    let unclean = previous_ended_uncleanly(&events_dir, u64::MAX);
    let mut writer = EventWriter::open(&events_dir, &config.agent)
        .map_err(|error| io_fail("open event log", error))?;
    writer.set_session(Some(config.session.clone()));
    let (sender, receiver): (Sender<Message>, Receiver<Message>) = mpsc::channel();
    control::serve(
        &control::socket_path(&config.agent_directory),
        sender.clone(),
        control_message,
    )
    .map_err(|error| io_fail("serve control socket", error))?;
    let generation = writer.generation();
    let mut wrapper = Wrapper {
        writer,
        queue_root,
        directories,
        queue_identity,
        sender,
        harness: None,
        next_instance: 0,
        busy: false,
        turn_causation: None,
        turn_started: None,
        current: None,
        started_inputs: HashMap::new(),
        tools: HashMap::new(),
        pending_controls: HashMap::new(),
        next_request: 0,
        interrupt_requested: false,
        stopping: None,
        stop_deadline: None,
        status: "starting",
        rescan: true,
        last_scan: Instant::now(),
        pause_until: None,
        restarts: Vec::new(),
        restart_at: None,
        restart_notice: false,
        idle_since: Instant::now(),
        config,
    };
    let settings_digest = settings_digest(&wrapper.config.arguments);
    wrapper.emit(
        "lifecycle.started",
        None,
        json!({
            "harness": "claude",
            "harness_version": null,
            "wrapper": crate::build_info::document(),
            "wrapper_pid": std::process::id(),
            "cwd": wrapper.config.cwd,
            "settings_sha256": settings_digest,
            "pruned_generations": wrapper.writer.pruned(),
        }),
    )?;
    wrapper.recover()?;
    if unclean && generation > 1 {
        let text = wrapper.restart_text("Your headless wrapper restarted after an unclean stop");
        wrapper.enqueue_notice(&format!("restart-g{generation}"), &text)?;
    }
    wrapper.spawn_harness(hooks)?;
    wrapper.set_status("idle")?;
    let exit = loop {
        wrapper.begin_stop_if_requested(&terminate);
        match receiver.recv_timeout(Duration::from_millis(250)) {
            Ok(Message::Line(instance, line)) => {
                if wrapper
                    .harness
                    .as_ref()
                    .is_some_and(|harness| harness.instance == instance)
                {
                    wrapper.harness_line(&line)?;
                }
            }
            Ok(Message::Eof(instance)) => {
                if wrapper
                    .harness
                    .as_ref()
                    .is_some_and(|harness| harness.instance == instance)
                {
                    if let Some(code) = wrapper.harness_exited()? {
                        break code;
                    }
                }
            }
            Ok(Message::Control(request)) => wrapper.control(request)?,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break 1,
        }
        if wrapper.harness_gone_without_eof() {
            if let Some(code) = wrapper.harness_exited()? {
                break code;
            }
        }
        wrapper.check_ack_deadline()?;
        wrapper.enforce_stop_deadline();
        if wrapper.harness.is_none() && wrapper.stopping.is_some() {
            wrapper.set_status("stopped")?;
            let cause = wrapper.stopping.clone();
            wrapper.emit("lifecycle.stopped", None, json!({"cause": cause}))?;
            break 0;
        }
        if wrapper.harness.is_none() && wrapper.restart_at.is_some_and(|at| Instant::now() >= at) {
            wrapper.restart_at = None;
            wrapper.recover()?;
            wrapper.spawn_harness(hooks)?;
            wrapper.idle_since = Instant::now();
            wrapper.set_status("idle")?;
            if std::mem::take(&mut wrapper.restart_notice) {
                let text = wrapper.restart_text("Your harness exited and was restarted");
                let id = format!("restart-g{generation}-h{}", wrapper.next_instance);
                wrapper.enqueue_notice(&id, &text)?;
            }
        }
        if wrapper.rescan || wrapper.last_scan.elapsed() >= Duration::from_secs(1) {
            wrapper.rescan = false;
            wrapper.last_scan = Instant::now();
            wrapper.maybe_deliver()?;
        }
    };
    let _ = std::fs::remove_file(control::socket_path(&wrapper.config.agent_directory));
    drop(wrapper);
    drop(lock);
    Ok(exit)
}

fn settings_digest(arguments: &[String]) -> Option<String> {
    let mut iterator = arguments.iter();
    while let Some(argument) = iterator.next() {
        let path = if let Some(path) = argument.strip_prefix("--settings=") {
            Some(path.to_owned())
        } else if argument == "--settings" {
            iterator.next().cloned()
        } else {
            None
        };
        if let Some(path) = path {
            return std::fs::read(&path)
                .ok()
                .map(|bytes| format!("{:x}", Sha256::digest(bytes)));
        }
    }
    None
}

fn iso_now() -> String {
    let seconds = (events::now_millis() / 1000) as libc::time_t;
    // SAFETY: gmtime_r writes only into the zeroed `tm` it is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let converted = unsafe { libc::gmtime_r(&seconds, &mut tm) };
    if converted.is_null() {
        return seconds.to_string();
    }
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

#[cfg(test)]
mod tests;
