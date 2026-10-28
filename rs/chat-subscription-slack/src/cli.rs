//! Transport over an installed Slack command-line client.
//!
//! The operator configures the client's absolute path. The transport uses these commands:
//!
//! | Operation | Command |
//! | --- | --- |
//! | inbound stream | `slack.conversation tail --channel-id=C… --event-types=message.created --include-iris-seq-id --output=json --heartbeats --catchup=…` |
//! | post | `slack.message send --channel-id=C… --markdown=file://- [--thread-ts=…] [--as=A…] --output=json` |
//! | reaction | `slack.reaction add --channel-id=C… --message-ts=… --name=… [--as=A…] --output=json` |
//! | listing | `slack.message list --channel-id=C… [--thread-ts=…] --oldest=… [--as=A…] --output=json` |
//! | rescan listing | `slack.message list --channel-id=C… [--thread-ts=…] --oldest=… [--page-token=…] --limit=20 --columns=… --content-format=mrkdwn --output=json` |
//! | sender lookup | `slack.user describe --user=<unixname\|email\|ID> --columns=user_id [--as=A…] --output=json` |
//! | human-author check | `slack.user describe --user=U… --columns=user_id,is_bot [--as=A…] --output=json` |
//! | current members | `slack.conversation members --channel-id=C… --limit=20 --columns=user_id [--page-token=…] [--as=A…] --output=json` |
//!
//! Member listings follow every page within one invocation's timeout and return no partial
//! result. They refuse incomplete source data, dropped rows, inconsistent pagination, repeated
//! tokens or more than [`chat_subscription::MAX_ALLOWED_SENDERS`] distinct user IDs.
//!
//! Message bodies travel on standard input (`file://-`), so they never appear in a process
//! argument list. A client that holds a person's credentials ([`ClientIdentity::User`], the
//! default) is given `--as=<app id>` on every write; without that flag it would act as the human
//! whose credentials it holds, which this transport never does. A client that runs as the app's
//! Crewmate ([`ClientIdentity::Crewmate`]) already acts through the app recorded on it and refuses
//! `--as`, so it is never given one. [`CliOptions::prefix_arguments`] go before the subcommand on
//! every run, such as a client flag that selects the Crewmate.
//!
//! Each tail stdout line is one JSON object: `{"type": "message.created", "iris_seq_id": N,
//! "payload": {"container_id": "C…", "slack_message": {"channel_id", "message_ts", "thread_ts",
//! "slack_user_id", "bot_id", "app_id", "subtype", "text", "team_id", "file_ids"}}}`, plus
//! `{"type": "HEARTBEAT"}` and `{"type": "GAP", ...}` records.

use std::collections::BTreeSet;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::ids::{self, AppId};
use crate::transport::{
    Catchup, EventStream, ListQuery, ListedMessage, PostMessage, ScanPage, ScanQuery,
    ScannedMessage, SlackMessage, SlackTransport, StreamCancel, StreamRecord, StreamWait,
    SubscribeSpec, TextFormat, TransportError,
};

/// Largest accepted stream line; longer lines are dropped with a notice.
pub const MAX_LINE_BYTES: usize = 256 * 1024;
pub(crate) const MAX_STDOUT_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_STDERR_BYTES: usize = 64 * 1024;
const TAIL_STOP_GRACE: Duration = Duration::from_secs(3);

/// Optional tail settings, passed through to the client when set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TailOptions {
    /// `--device-id`: a stable subscriber identity.
    pub device_id: Option<String>,
    /// `--caller`: attribution label.
    pub caller: Option<String>,
    /// `--app-id`: subscriber app.
    pub subscriber_app_id: Option<String>,
    /// `--mode`: subscription mode.
    pub mode: Option<String>,
    /// `--heartbeat-interval` seconds.
    pub heartbeat_seconds: Option<u64>,
    /// `--stall-timeout` seconds.
    pub stall_timeout_seconds: Option<u64>,
}

/// Who the configured client acts as when it writes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClientIdentity {
    /// The client holds a person's credentials, so every write passes `--as=<app id>` to act as
    /// the app instead.
    #[default]
    User,
    /// The client runs as the app's Crewmate, which acts through the app recorded on it and
    /// refuses `--as`, so no write passes it.
    Crewmate,
}

impl ClientIdentity {
    /// The `--as` argument a write by this client takes to act as `app_id`, if any.
    #[must_use]
    pub fn app_argument(self, app_id: &AppId) -> Option<String> {
        match self {
            Self::User => Some(format!("--as={app_id}")),
            Self::Crewmate => None,
        }
    }
}

/// Configuration of the command-line transport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliOptions {
    /// Absolute path of the client executable.
    pub program: PathBuf,
    /// Arguments placed before the subcommand on every invocation.
    pub prefix_arguments: Vec<String>,
    /// Who the client acts as when it writes.
    pub identity: ClientIdentity,
    /// Extra non-secret environment values set on every invocation.
    pub environment: Vec<(String, String)>,
    /// Deadline for one non-streaming invocation.
    pub timeout: Duration,
    /// Stream settings.
    pub tail: TailOptions,
}

/// [`SlackTransport`] that runs the configured client.
#[derive(Clone, Debug)]
pub struct CliTransport {
    options: CliOptions,
}

impl CliTransport {
    /// Create a transport. The program path must be absolute.
    ///
    /// # Errors
    ///
    /// Returns a description when the path is relative.
    pub fn new(options: CliOptions) -> Result<Self, String> {
        if !options.program.is_absolute() {
            return Err("the Slack client path must be absolute".to_owned());
        }
        Ok(Self { options })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.options.program);
        command.args(&self.options.prefix_arguments);
        command.envs(
            self.options
                .environment
                .iter()
                .map(|(name, value)| (name, value)),
        );
        command
    }

    fn run(&self, arguments: &[String], stdin: Option<&[u8]>) -> Result<Completed, RunFailure> {
        let mut command = self.command();
        command.args(arguments);
        run_bounded(command, stdin, self.options.timeout)
    }
}

/// The client arguments for one stream.
#[must_use]
pub fn tail_arguments(spec: &SubscribeSpec, options: &TailOptions) -> Vec<String> {
    let mut arguments = vec![
        "slack.conversation".to_owned(),
        "tail".to_owned(),
        format!("--channel-id={}", spec.channel_ids.join(",")),
        "--event-types=message.created".to_owned(),
        "--include-iris-seq-id".to_owned(),
        "--output=json".to_owned(),
        "--no-color".to_owned(),
        "--heartbeats".to_owned(),
    ];
    match &spec.catchup {
        Catchup::None => arguments.push("--catchup=none".to_owned()),
        Catchup::Oldest => arguments.push("--catchup=oldest".to_owned()),
        Catchup::Sequence(sequence) => arguments.push(format!("--catchup={sequence}")),
        Catchup::SinceSeconds(seconds) => arguments.push(format!("--since={seconds}s")),
    }
    if let Some(value) = &options.device_id {
        arguments.push(format!("--device-id={value}"));
    }
    if let Some(value) = &options.caller {
        arguments.push(format!("--caller={value}"));
    }
    if let Some(value) = &options.subscriber_app_id {
        arguments.push(format!("--app-id={value}"));
    }
    if let Some(value) = &options.mode {
        arguments.push(format!("--mode={value}"));
    }
    if let Some(value) = options.heartbeat_seconds {
        arguments.push(format!("--heartbeat-interval={value}"));
    }
    if let Some(value) = options.stall_timeout_seconds {
        arguments.push(format!("--stall-timeout={value}"));
    }
    arguments
}

fn text_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// The valid Slack file IDs in a row's `file_ids`, which the client reports either as a list or
/// as one comma-separated string. At most [`MAX_FILE_IDS`] are kept, in order, without repeats.
#[must_use]
pub fn file_ids_field(value: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    let mut push = |candidate: &str| {
        let candidate = candidate.trim();
        if ids::valid_file_id(candidate)
            && ids.len() < MAX_FILE_IDS
            && !ids.iter().any(|id| id == candidate)
        {
            ids.push(candidate.to_owned());
        }
    };
    match value.get("file_ids") {
        Some(Value::String(joined)) => joined.split([',', ' ']).for_each(&mut push),
        Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).for_each(push),
        _ => {}
    }
    ids
}

/// Most file IDs kept from one message.
pub const MAX_FILE_IDS: usize = 20;

/// Classify one stdout line of the tail.
#[must_use]
pub fn parse_tail_line(line: &str) -> StreamRecord {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return StreamRecord::Notice("blank line".to_owned());
    }
    let value: Value = match serde_json::from_str(trimmed) {
        Ok(value) => value,
        Err(_) => return StreamRecord::Notice("non-JSON stream line".to_owned()),
    };
    let record_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match record_type {
        "HEARTBEAT" => return StreamRecord::Heartbeat,
        "GAP" => {
            let from = value.get("lost_from_iris_seq_id").map(Value::to_string);
            let to = value.get("lost_to_iris_seq_id").map(Value::to_string);
            let reason = value
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unspecified");
            return StreamRecord::Gap(format!(
                "event stream lost events from sequence {} to {}: {reason}",
                from.unwrap_or_else(|| "?".to_owned()),
                to.unwrap_or_else(|| "?".to_owned())
            ));
        }
        "STALL" | "REVIVAL" => {
            return StreamRecord::Notice(format!("stream reported {record_type}"));
        }
        _ => {}
    }
    let Some(sequence) = value.get("iris_seq_id").and_then(Value::as_u64) else {
        return StreamRecord::Notice(format!("unsequenced record of type {record_type:?}"));
    };
    let payload = value.get("payload").unwrap_or(&Value::Null);
    let event_type = payload
        .get("event_type")
        .and_then(Value::as_str)
        .unwrap_or(record_type)
        .to_owned();
    let message = (event_type == "message.created")
        .then(|| payload.get("slack_message"))
        .flatten()
        .and_then(|slack| {
            let message_ts = text_field(slack, "message_ts")?;
            let channel_id =
                text_field(slack, "channel_id").or_else(|| text_field(payload, "container_id"))?;
            Some(Box::new(SlackMessage {
                channel_id,
                message_ts,
                thread_ts: text_field(slack, "thread_ts"),
                user_id: text_field(slack, "slack_user_id"),
                bot_id: text_field(slack, "bot_id"),
                app_id: text_field(slack, "app_id"),
                subtype: text_field(slack, "subtype"),
                text: slack
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                team_id: text_field(slack, "team_id"),
                file_ids: file_ids_field(slack),
            }))
        });
    StreamRecord::Event {
        sequence,
        event_type,
        message,
    }
}

/// Classify one stderr line of the tail. A resume position that has aged out of the provider's
/// retention is reported as a gap; everything else is a notice.
#[must_use]
pub fn classify_tail_stderr(line: &str) -> StreamRecord {
    let lower = line.to_ascii_lowercase();
    if lower.contains("overflow") || lower.contains("no longer queued") {
        StreamRecord::Gap(
            "the event stream could not replay from the saved position (retention overflow)"
                .to_owned(),
        )
    } else {
        StreamRecord::Notice(format!(
            "stream stderr: {}",
            crate::transport::bounded(line)
        ))
    }
}

/// Proves, from one tail's own output, a stream position that is safe to commit.
///
/// The tail filters events in the client, so its sequence keeps moving through events the
/// plugin never sees, and only an event for the channel names a sequence on standard output.
/// Heartbeats name none. The one other position the client reports is the subscribe response
/// it writes to standard error once catch-up is done. With `--catchup=<S>` and the explicit
/// subscription mode (the default), the client:
///
/// 1. writes `Catching up from S...`, then reads the stream from `S` inclusive to the head it
///    reports, every event, filtered or not, writing each matching event to standard output
///    and flushing it;
/// 2. writes `Fetched N events` only when that read ends normally; every failure (retention
///    overflow, throttling, a server error) writes its own message instead;
/// 3. subscribes from `lastKnownPosition` = the last sequence it read, and writes
///    `Subscribe response: SubscribeResponse(seqId=A, lastIssuedSeqId=B, ...,
///    requestedPositionSeqId=C)`, where `C` is the position it asked for and `A` the one
///    the stream resolved. Delivery resumes after it;
/// 4. writes a `HEARTBEAT` line to standard output after each later subscription refresh.
///
/// So when 1 to 3 appear in order with `A == C`, `C >= S` and `B >= C`, every matching event
/// at or before `C` was written to standard output before the subscribe response was written.
/// The plugin reads the two pipes separately, so the response can arrive before those events
/// do. A heartbeat is the barrier: it is written after the response, so once one has been read
/// from standard output every event written before the response has been read too. Then `C`
/// is proven. Only the first subscribe response counts, and any other shape proves nothing.
/// Neither does a standard output line dropped before the heartbeat, since it may have been one
/// of those events.
///
/// If catch-up read nothing at all, the client subscribes without a position and the response
/// names the head, which catch-up did not prove. That cannot give `C > S`: catch-up reads from
/// `S` inclusive, `S` was issued (it came from an event or an earlier proof), and a position
/// that is no longer retained fails with an overflow instead of printing `Fetched`. So the
/// first page holds at least `S`, and nothing earlier than any retained event can be missing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionProof {
    stage: ProofStage,
    candidate: Option<u64>,
    heartbeat_seen: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProofStage {
    AwaitCatchup(u64),
    CatchingUp(u64),
    Fetched(u64),
    Done,
}

impl PositionProof {
    /// A proof for a stream opened with `spec` in subscription `mode` (`None` is the default,
    /// explicit). Only a sequence catch-up in explicit mode can prove anything.
    #[must_use]
    pub fn new(spec: &SubscribeSpec, mode: Option<&str>) -> Self {
        let stage = match spec.catchup {
            Catchup::Sequence(start) if matches!(mode, None | Some("explicit")) => {
                ProofStage::AwaitCatchup(start)
            }
            _ => ProofStage::Done,
        };
        Self {
            stage,
            candidate: None,
            heartbeat_seen: false,
        }
    }

    /// Observe one standard error line; returns the position it completes a proof of.
    pub fn stderr(&mut self, line: &str) -> Option<u64> {
        let line = strip_ansi(line);
        let line = line.trim();
        if let Some(response) = line.strip_prefix("Subscribe response:") {
            if let ProofStage::Fetched(start) = self.stage {
                self.candidate =
                    subscribed_position(response).filter(|&position| position >= start);
            }
            self.stage = ProofStage::Done;
        } else if let Some(from) = line
            .strip_prefix("Catching up from ")
            .and_then(|rest| rest.strip_suffix("..."))
        {
            self.stage = match self.stage {
                ProofStage::AwaitCatchup(start) if from.parse() == Ok(start) => {
                    ProofStage::CatchingUp(start)
                }
                _ => ProofStage::Done,
            };
        } else if line.starts_with("Fetched ") && line.ends_with(" events") {
            self.stage = match self.stage {
                ProofStage::CatchingUp(start) => ProofStage::Fetched(start),
                _ => ProofStage::Done,
            };
        }
        self.ready()
    }

    /// Observe one standard output record; returns the position it completes a proof of.
    pub fn stdout(&mut self, record: &StreamRecord) -> Option<u64> {
        match record {
            StreamRecord::Heartbeat => self.heartbeat_seen = true,
            // A line read before the barrier that could have been an event but was dropped
            // (oversized, not JSON, no sequence) may be a channel message at or before the
            // position, so the position is no longer proven.
            StreamRecord::Notice(notice)
                if !self.heartbeat_seen
                    && notice != "blank line"
                    && !notice.starts_with("stream reported ") =>
            {
                self.stage = ProofStage::Done;
                self.candidate = None;
            }
            _ => {}
        }
        self.ready()
    }

    fn ready(&mut self) -> Option<u64> {
        if self.heartbeat_seen {
            self.candidate.take()
        } else {
            None
        }
    }
}

/// `C` from `SubscribeResponse(seqId=A, lastIssuedSeqId=B, ..., requestedPositionSeqId=C)`,
/// only when `A == C <= B`.
fn subscribed_position(response: &str) -> Option<u64> {
    let fields = response
        .trim()
        .strip_prefix("SubscribeResponse(")?
        .strip_suffix(')')?;
    let field = |name: &str| -> Option<u64> {
        let mut values = fields
            .split(',')
            .filter_map(|field| field.trim().strip_prefix(name)?.strip_prefix('='));
        let value = values.next()?.parse().ok()?;
        values.next().is_none().then_some(value)
    };
    let resolved = field("seqId")?;
    let requested = field("requestedPositionSeqId")?;
    let head = field("lastIssuedSeqId")?;
    (resolved == requested && requested <= head).then_some(requested)
}

/// Remove terminal colour sequences (`ESC [ ... m`), which the client writes to standard error
/// even with `--no-color`.
fn strip_ansi(line: &str) -> String {
    let mut plain = String::with_capacity(line.len());
    let mut characters = line.chars();
    while let Some(character) = characters.next() {
        if character == '\u{1b}' {
            for skipped in characters.by_ref() {
                if skipped.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(character);
        }
    }
    plain
}

/// Read one line of at most `limit` bytes; longer lines are consumed and reported as truncated.
///
/// Returns `Ok(None)` at end of input, otherwise whether the line was truncated.
///
/// # Errors
///
/// Returns the underlying read error.
pub fn read_bounded_line(
    reader: &mut impl BufRead,
    line: &mut Vec<u8>,
    limit: usize,
) -> io::Result<Option<bool>> {
    line.clear();
    let mut truncated = false;
    let mut read_any = false;
    loop {
        let available = match reader.fill_buf() {
            Ok(buffer) => buffer,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if available.is_empty() {
            return Ok(read_any.then_some(truncated));
        }
        read_any = true;
        let (chunk, found_newline) = match available.iter().position(|byte| *byte == b'\n') {
            Some(index) => (&available[..index], true),
            None => (available, false),
        };
        let room = limit.saturating_sub(line.len());
        if chunk.len() > room {
            truncated = true;
        }
        line.extend_from_slice(&chunk[..chunk.len().min(room)]);
        let consumed = chunk.len() + usize::from(found_newline);
        reader.consume(consumed);
        if found_newline {
            return Ok(Some(truncated));
        }
    }
}

enum Pumped {
    Record(StreamRecord),
    Stderr(String),
    Closed(String),
    Cancelled,
}

struct ChannelCancel(Sender<Pumped>);

impl StreamCancel for ChannelCancel {
    fn cancel(&self) {
        let _ = self.0.send(Pumped::Cancelled);
    }
}

/// A running tail process.
pub struct CliStream {
    child: Option<Child>,
    _stdin: Option<ChildStdin>,
    receiver: Receiver<Pumped>,
    sender: Sender<Pumped>,
    cancelled: bool,
    proof: PositionProof,
    proven: Option<u64>,
}

impl EventStream for CliStream {
    fn next(&mut self, wait: Duration) -> StreamWait {
        if self.cancelled {
            return StreamWait::Cancelled;
        }
        if let Some(position) = self.proven.take() {
            return StreamWait::Record(StreamRecord::Position(position));
        }
        match self.receiver.recv_timeout(wait) {
            Ok(Pumped::Record(record)) => {
                self.proven = self.proof.stdout(&record);
                StreamWait::Record(record)
            }
            Ok(Pumped::Stderr(line)) => {
                self.proven = self.proof.stderr(&line);
                StreamWait::Record(classify_tail_stderr(&line))
            }
            Ok(Pumped::Closed(reason)) => StreamWait::Closed(reason),
            Ok(Pumped::Cancelled) => {
                self.cancelled = true;
                StreamWait::Cancelled
            }
            Err(RecvTimeoutError::Timeout) => StreamWait::Idle,
            Err(RecvTimeoutError::Disconnected) => {
                StreamWait::Closed("stream readers ended".to_owned())
            }
        }
    }

    fn canceller(&self) -> Arc<dyn StreamCancel> {
        Arc::new(ChannelCancel(self.sender.clone()))
    }

    fn close(&mut self) -> Result<(), String> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        stop_child(&mut child, TAIL_STOP_GRACE)
    }
}

impl Drop for CliStream {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

/// Ask a child to stop with SIGTERM so it can unsubscribe, then SIGKILL after `grace`.
fn stop_child(child: &mut Child, grace: Duration) -> Result<(), String> {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return Ok(());
    }
    terminate(child.id());
    match wait_until(child, Instant::now() + grace) {
        Ok(Some(_)) => return Ok(()),
        Ok(None) => {}
        Err(error) => return Err(format!("waiting for the stream process failed: {error}")),
    }
    child
        .kill()
        .and_then(|()| child.wait().map(|_| ()))
        .map_err(|error| format!("killing the stream process failed: {error}"))
}

#[allow(unsafe_code)]
fn terminate(process_id: u32) {
    let Ok(process_id) = libc::pid_t::try_from(process_id) else {
        return;
    };
    // SAFETY: `kill` has no memory-safety preconditions. The PID belongs to a child this process
    // has not yet reaped, so it cannot have been reused.
    unsafe {
        libc::kill(process_id, libc::SIGTERM);
    }
}

/// Wait for `child` to exit until `deadline`, and reap it. Returns `None` when the deadline
/// passes first; the child is then still running and unreaped.
///
/// The wait blocks on a pidfd, so it costs no wakeups while the child runs. On a kernel without
/// `pidfd_open` it falls back to checking every 10 ms.
///
/// # Errors
///
/// Returns the error of a failed wait.
pub fn wait_until(child: &mut Child, deadline: Instant) -> io::Result<Option<ExitStatus>> {
    if let Some(status) = child.try_wait()? {
        return Ok(Some(status));
    }
    match pidfd_open(child.id()) {
        Ok(pidfd) => loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if poll_readable(&pidfd, remaining)? {
                // A readable pidfd means the process has exited; this reaps it.
                return child.wait().map(Some);
            }
            if Instant::now() >= deadline {
                return child.try_wait();
            }
        },
        Err(_) => loop {
            if let Some(status) = child.try_wait()? {
                return Ok(Some(status));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            thread::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(10)),
            );
        },
    }
}

#[allow(unsafe_code)]
fn pidfd_open(process_id: u32) -> io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;
    let process_id = libc::pid_t::try_from(process_id)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "process id out of range"))?;
    // SAFETY: `pidfd_open` takes a PID and flags and returns a new descriptor or -1; it reads no
    // memory of ours. The PID is an unreaped child of this process, so it names that child.
    let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, process_id, 0) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    let descriptor = i32::try_from(descriptor)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "pidfd out of range"))?;
    // SAFETY: the kernel just returned this descriptor, and nothing else owns it.
    Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(descriptor) })
}

/// Wait up to `timeout` for `descriptor` to become readable; `true` when it did.
#[allow(unsafe_code)]
fn poll_readable(descriptor: &std::os::fd::OwnedFd, timeout: Duration) -> io::Result<bool> {
    use std::os::fd::AsRawFd;
    let mut entry = libc::pollfd {
        fd: descriptor.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // Round up, so a wait never ends just before its deadline and spins.
    let millis = timeout
        .as_micros()
        .div_ceil(1_000)
        .min(u128::from(i32::MAX.unsigned_abs()));
    let millis = i32::try_from(millis).unwrap_or(i32::MAX);
    loop {
        // SAFETY: `entry` is one valid, initialised pollfd that outlives the call.
        let ready = unsafe { libc::poll(&mut entry, 1, millis) };
        if ready >= 0 {
            return Ok(ready > 0);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn pump_stdout(stdout: impl Read + Send + 'static, sender: Sender<Pumped>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = Vec::new();
        loop {
            match read_bounded_line(&mut reader, &mut line, MAX_LINE_BYTES) {
                Ok(None) => break,
                Ok(Some(true)) => {
                    let _ = sender.send(Pumped::Record(StreamRecord::Notice(
                        "oversized stream line dropped".to_owned(),
                    )));
                }
                Ok(Some(false)) => {
                    let record = parse_tail_line(&String::from_utf8_lossy(&line));
                    if sender.send(Pumped::Record(record)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = sender.send(Pumped::Closed(format!("stream read failed: {error}")));
                    return;
                }
            }
        }
        let _ = sender.send(Pumped::Closed(
            "the stream process closed its output".to_owned(),
        ));
    });
}

fn pump_stderr(stderr: impl Read + Send + 'static, sender: Sender<Pumped>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut line = Vec::new();
        while let Ok(Some(_)) = read_bounded_line(&mut reader, &mut line, 4_096) {
            let line = String::from_utf8_lossy(&line).into_owned();
            if sender.send(Pumped::Stderr(line)).is_err() {
                return;
            }
        }
    });
}

/// A finished non-streaming invocation.
#[derive(Debug)]
pub(crate) struct Completed {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

#[derive(Debug)]
enum RunFailure {
    /// The process could not be started: nothing happened.
    Spawn(io::Error),
    /// The deadline passed; the process was killed after it had started.
    Timeout,
    /// Waiting failed after start.
    Wait(io::Error),
}

pub(crate) fn collect(
    stream: impl Read + Send + 'static,
    limit: usize,
) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stream
            .take(u64::try_from(limit).unwrap_or(u64::MAX))
            .read_to_end(&mut buffer);
        buffer
    })
}

fn run_bounded(
    mut command: Command,
    stdin: Option<&[u8]>,
    timeout: Duration,
) -> Result<Completed, RunFailure> {
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(RunFailure::Spawn)?;
    let writer = match (stdin, child.stdin.take()) {
        (Some(bytes), Some(mut pipe)) => {
            let bytes = bytes.to_vec();
            Some(thread::spawn(move || {
                let _ = pipe.write_all(&bytes);
            }))
        }
        _ => None,
    };
    let stdout = collect(
        child.stdout.take().expect("stdout is piped"),
        MAX_STDOUT_BYTES,
    );
    let stderr = collect(
        child.stderr.take().expect("stderr is piped"),
        MAX_STDERR_BYTES,
    );
    let deadline = Instant::now() + timeout;
    let status = match wait_until(&mut child, deadline) {
        Ok(Some(status)) => status,
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunFailure::Timeout);
        }
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunFailure::Wait(error));
        }
    };
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    Ok(Completed {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// The first JSON value in a client's stdout, tolerating leading non-JSON lines.
#[must_use]
pub fn first_json_value(stdout: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(stdout);
    if let Ok(value) = serde_json::from_str::<Value>(text.trim()) {
        return Some(value);
    }
    let start = text.find(['{', '['])?;
    serde_json::Deserializer::from_str(&text[start..])
        .into_iter::<Value>()
        .next()
        .and_then(Result::ok)
}

fn find_string<'a>(value: &'a Value, keys: &[&str], depth: usize) -> Option<&'a str> {
    if depth > 6 {
        return None;
    }
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(found) = map
                    .get(*key)
                    .and_then(Value::as_str)
                    .filter(|ts| ids::valid_ts(ts))
                {
                    return Some(found);
                }
            }
            map.values()
                .find_map(|child| find_string(child, keys, depth + 1))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_string(child, keys, depth + 1)),
        _ => None,
    }
}

fn find_bool(value: &Value, key: &str, depth: usize) -> Option<bool> {
    if depth > 6 {
        return None;
    }
    match value {
        Value::Object(map) => map.get(key).and_then(Value::as_bool).or_else(|| {
            map.values()
                .find_map(|child| find_bool(child, key, depth + 1))
        }),
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_bool(child, key, depth + 1)),
        _ => None,
    }
}

/// Slack error codes that mean the operation can never succeed as submitted.
const PERMANENT_ERRORS: &[&str] = &[
    "msg_too_long",
    "no_text",
    "invalid_blocks",
    "too_many_attachments",
    "invalid_name",
    "too_many_reactions",
];

/// Map an exited client invocation that reported failure to a [`TransportError`].
///
/// An error document from the client (`{"status": "error", "message": ...}`) means Slack or the
/// client refused the call, so the write did not happen. A failure with no readable error
/// document cannot prove that, and is reported with an unknown outcome.
fn exit_failure(completed: &Completed, write: bool) -> TransportError {
    // The client writes its error document to stdout or stderr depending on the command;
    // the live rate-limit document arrives on stderr (#615 chat-file-messages). Whichever
    // stream carries an error document wins; a non-error stdout document must not hide it.
    let parsed = [&completed.stdout, &completed.stderr]
        .into_iter()
        .find_map(|bytes| {
            first_json_value(bytes)
                .filter(|value| value.get("status").and_then(Value::as_str) == Some("error"))
        })
        .or_else(|| first_json_value(&completed.stdout));
    let message = parsed
        .as_ref()
        .filter(|value| value.get("status").and_then(Value::as_str) == Some("error"))
        .and_then(|value| value.get("message").and_then(Value::as_str))
        .map(str::to_owned);
    let stderr = String::from_utf8_lossy(&completed.stderr);
    let Some(message) = message else {
        let detail = format!(
            "client exited with {} and no error document; stderr: {}",
            completed.status,
            crate::transport::bounded(&stderr)
        );
        return if write {
            TransportError::unknown("client_failed", detail)
        } else {
            TransportError::not_applied("client_failed", detail, true)
        };
    };
    let lower = message.to_ascii_lowercase();
    if let Some(code) = PERMANENT_ERRORS.iter().find(|code| lower.contains(**code)) {
        return TransportError::not_applied(code, message, false);
    }
    if lower.contains("ratelimited")
        || lower.contains("rate_limited")
        || lower.contains("rate-limited")
    {
        return TransportError::not_applied("ratelimited", message, true);
    }
    let code = [
        "not_in_channel",
        "channel_not_found",
        "is_archived",
        "thread_not_found",
        "message_not_found",
        "missing_scope",
        "invalid_auth",
        "not_authed",
        "account_inactive",
        "restricted_action",
    ]
    .into_iter()
    .find(|code| lower.contains(code))
    .unwrap_or("slack_error");
    TransportError::not_applied(code, message, true)
}

fn run_failure(failure: RunFailure, write: bool) -> TransportError {
    match failure {
        RunFailure::Spawn(error) => TransportError::not_applied(
            "client_unavailable",
            format!("the Slack client could not be started: {error}"),
            true,
        ),
        RunFailure::Timeout if write => TransportError::unknown(
            "transport_timeout",
            "the Slack client did not finish before its deadline",
        ),
        RunFailure::Timeout => TransportError::not_applied(
            "transport_timeout",
            "the Slack client did not finish before its deadline",
            true,
        ),
        RunFailure::Wait(error) if write => {
            TransportError::unknown("transport_wait_failed", format!("waiting failed: {error}"))
        }
        RunFailure::Wait(error) => TransportError::not_applied(
            "transport_wait_failed",
            format!("waiting failed: {error}"),
            true,
        ),
    }
}

fn directory_unreadable(detail: &str) -> TransportError {
    TransportError::not_applied("directory_unreadable", detail, false)
}

/// Directory reads accept one complete JSON document, never a prefix of a truncated result.
fn directory_document(completed: &Completed) -> Result<Value, TransportError> {
    if !completed.status.success() {
        return Err(exit_failure(completed, false));
    }
    if completed.stdout.len() >= MAX_STDOUT_BYTES {
        return Err(directory_unreadable(
            "the directory response reached the output limit; pass explicit Slack user IDs instead",
        ));
    }
    let text = std::str::from_utf8(&completed.stdout)
        .map_err(|_| directory_unreadable("the directory response was not UTF-8"))?;
    let start = text
        .find(['{', '['])
        .ok_or_else(|| directory_unreadable("the directory response was not a JSON document"))?;
    let parsed: Value = serde_json::from_str(&text[start..]).map_err(|_| {
        directory_unreadable("the directory response was not one complete JSON document")
    })?;
    if parsed.get("status").and_then(Value::as_str) == Some("error") {
        return Err(exit_failure(completed, false));
    }
    Ok(parsed)
}

fn resolved_sender(document: &Value) -> Result<String, TransportError> {
    let data = document.get("data").unwrap_or(document);
    let row = match data {
        Value::Object(_) => data,
        Value::Array(rows) if rows.len() == 1 => &rows[0],
        _ => {
            return Err(directory_unreadable("the lookup did not return exactly one person; pass a Slack user ID from the person's Slack profile"));
        }
    };
    row.get("user_id")
        .and_then(Value::as_str)
        .filter(|id| ids::valid_user_id(id))
        .map(str::to_owned)
        .ok_or_else(|| directory_unreadable("the lookup did not return a Slack user ID; pass the ID from the person's Slack profile"))
}

fn exact_human_sender(document: &Value, sender_id: &str) -> bool {
    let data = document.get("data").unwrap_or(document);
    let row = match data {
        Value::Object(_) => data,
        Value::Array(rows) if rows.len() == 1 => &rows[0],
        _ => return false,
    };
    row.get("user_id").and_then(Value::as_str) == Some(sender_id)
        && row.get("is_bot").and_then(Value::as_bool) == Some(false)
}

fn member_page(document: &Value) -> Result<(Vec<String>, Option<String>), TransportError> {
    if document
        .pointer("/completeness/source_complete")
        .and_then(Value::as_bool)
        != Some(true)
        || document.get("dropped_row_count").and_then(Value::as_u64) != Some(0)
    {
        return Err(directory_unreadable("the member listing did not confirm complete source data with no dropped rows; no partial listing is returned"));
    }
    let rows = document
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| directory_unreadable("the member listing did not contain a data array"))?;
    let members = rows
        .iter()
        .map(|row| {
            row.get("user_id")
                .and_then(Value::as_str)
                .filter(|id| ids::valid_user_id(id))
                .map(str::to_owned)
                .ok_or_else(|| directory_unreadable("a member row did not contain a Slack user ID; no partial listing is returned"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let pagination = document.get("pagination").ok_or_else(|| {
        directory_unreadable(
            "the member listing did not report pagination; no partial listing is returned",
        )
    })?;
    let has_next = pagination
        .get("has_next_page")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            directory_unreadable("the member listing did not say whether another page exists")
        })?;
    let token = match pagination.get("next_page_token") {
        Some(Value::String(token))
            if token.len() <= 4_096 && !token.chars().any(char::is_control) =>
        {
            (!token.is_empty()).then(|| token.clone())
        }
        Some(Value::Null) | None => None,
        _ => {
            return Err(directory_unreadable(
                "the member listing had an invalid page token",
            ))
        }
    };
    if has_next != token.is_some() {
        return Err(directory_unreadable(
            "the member listing had inconsistent pagination; no partial listing is returned",
        ));
    }
    Ok((members, token))
}

/// The client arguments of one rescan listing page. It runs as the configured member, like the
/// tail, so it takes no `--as`; `mrkdwn` is the stored text an event carries.
#[must_use]
pub fn scan_arguments(query: &ScanQuery<'_>) -> Vec<String> {
    let mut arguments = vec![
        "slack.message".to_owned(),
        "list".to_owned(),
        format!("--channel-id={}", query.channel_id),
    ];
    if let Some(thread_ts) = query.thread_ts {
        arguments.push(format!("--thread-ts={thread_ts}"));
    }
    arguments.push(format!("--oldest={}", query.oldest));
    if let Some(token) = query.page_token {
        arguments.push(format!("--page-token={token}"));
    }
    arguments.extend([
        "--limit=20".to_owned(),
        "--columns=message_ts,thread_ts,author_user_id,subtype,content,file_ids,reply_count,latest_reply"
            .to_owned(),
        "--content-format=mrkdwn".to_owned(),
        "--output=json".to_owned(),
    ]);
    arguments
}

/// Decode one rescan listing page. Rows without a valid `message_ts` are skipped; a page whose
/// pagination says another page exists without a usable token is refused.
///
/// # Errors
///
/// Returns `listing_unreadable` for a document without rows or with inconsistent pagination.
pub fn scan_page(document: &Value, channel_id: &str) -> Result<ScanPage, TransportError> {
    let unreadable =
        |detail: &str| TransportError::not_applied("listing_unreadable", detail.to_owned(), true);
    let rows = document
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| unreadable("the client listing had no data array"))?;
    let messages = rows
        .iter()
        .filter_map(|row| {
            let message_ts = text_field(row, "message_ts").filter(|ts| ids::valid_ts(ts))?;
            Some(ScannedMessage {
                message: SlackMessage {
                    channel_id: channel_id.to_owned(),
                    message_ts,
                    thread_ts: text_field(row, "thread_ts").filter(|ts| ids::valid_ts(ts)),
                    user_id: text_field(row, "author_user_id"),
                    subtype: text_field(row, "subtype"),
                    text: row
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    file_ids: file_ids_field(row),
                    ..SlackMessage::default()
                },
                reply_count: row.get("reply_count").and_then(Value::as_u64).unwrap_or(0),
                latest_reply: text_field(row, "latest_reply").filter(|ts| ids::valid_ts(ts)),
            })
        })
        .collect();
    let pagination = document.get("pagination").unwrap_or(&Value::Null);
    let has_next = pagination
        .get("has_next_page")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let token = match pagination.get("next_page_token") {
        Some(Value::String(token))
            if !token.is_empty()
                && token.len() <= 4_096
                && !token.chars().any(char::is_control) =>
        {
            Some(token.clone())
        }
        Some(Value::String(token)) if token.is_empty() => None,
        Some(Value::Null) | None => None,
        _ => return Err(unreadable("the client listing had an invalid page token")),
    };
    if has_next && token.is_none() {
        return Err(unreadable(
            "the client listing said another page exists but gave no page token",
        ));
    }
    Ok(ScanPage {
        messages,
        next_page_token: token.filter(|_| has_next),
    })
}

/// The client arguments of one post, whose body the client reads from standard input. `app`
/// is the `--as` argument, if the client takes one: see [`ClientIdentity::app_argument`].
/// `blocks_file`, a private file holding the post's Block Kit blocks, makes the body their
/// notification fallback.
#[must_use]
pub(crate) fn post_arguments(
    app: Option<String>,
    channel_id: &str,
    thread_ts: Option<&str>,
    format: TextFormat,
    blocks_file: Option<&Path>,
) -> Vec<String> {
    let body_flag = match format {
        TextFormat::Markdown => "--markdown=file://-",
        TextFormat::Text => "--text=file://-",
        TextFormat::SlackMrkdwn | TextFormat::Converted => "--slack-mrkdwn=file://-",
    };
    let mut arguments = vec![
        "slack.message".to_owned(),
        "send".to_owned(),
        format!("--channel-id={channel_id}"),
        body_flag.to_owned(),
    ];
    if let Some(path) = blocks_file {
        arguments.push(format!("--blocks=file://{}", path.display()));
    }
    if let Some(thread_ts) = thread_ts {
        arguments.push(format!("--thread-ts={thread_ts}"));
    }
    arguments.extend(app);
    arguments.push("--output=json".to_owned());
    arguments
}

/// The result of one finished post: the new message's `ts`, or the failure.
pub(crate) fn post_outcome(completed: &Completed) -> Result<String, TransportError> {
    if !completed.status.success() {
        return Err(exit_failure(completed, true));
    }
    let parsed = first_json_value(&completed.stdout).ok_or_else(|| {
        TransportError::unknown(
            "receipt_unreadable",
            "the client reported success without readable output",
        )
    })?;
    if parsed.get("status").and_then(Value::as_str) == Some("error") {
        return Err(exit_failure(completed, true));
    }
    find_string(&parsed, &["message_ts", "ts"], 0)
        .map(str::to_owned)
        .ok_or_else(|| {
            TransportError::unknown(
                "receipt_unreadable",
                "the client reported success without a message ts",
            )
        })
}

/// A post's Block Kit blocks in a new owner-only file under the temporary directory, for the
/// client's `--blocks=file://…`, deleted when dropped.
struct BlocksFile {
    path: PathBuf,
}

impl BlocksFile {
    fn create(blocks: &str) -> io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        let path = std::env::temp_dir().join(format!(
            "agentctl-slack-blocks-{}-{nanos}-{}.json",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let written = Self { path };
        file.write_all(blocks.as_bytes())?;
        file.sync_all()?;
        Ok(written)
    }
}

impl Drop for BlocksFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl CliTransport {
    /// The configured client, with its environment, ready for `arguments`.
    pub(crate) fn client_command(&self, arguments: &[String]) -> Command {
        let mut command = self.command();
        command.args(arguments);
        command
    }

    /// The configured client's options.
    #[must_use]
    pub fn options(&self) -> &CliOptions {
        &self.options
    }
}

impl SlackTransport for CliTransport {
    fn open_stream(
        &mut self,
        spec: &SubscribeSpec,
    ) -> Result<Box<dyn EventStream>, TransportError> {
        let mut command = self.command();
        command
            .args(tail_arguments(spec, &self.options.tail))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| {
            TransportError::not_applied(
                "client_unavailable",
                format!("the Slack client could not be started: {error}"),
                true,
            )
        })?;
        let (sender, receiver) = mpsc::channel();
        pump_stdout(
            child.stdout.take().expect("stdout is piped"),
            sender.clone(),
        );
        pump_stderr(
            child.stderr.take().expect("stderr is piped"),
            sender.clone(),
        );
        let stdin = child.stdin.take();
        Ok(Box::new(CliStream {
            child: Some(child),
            _stdin: stdin,
            receiver,
            sender,
            cancelled: false,
            proof: PositionProof::new(spec, self.options.tail.mode.as_deref()),
            proven: None,
        }))
    }

    fn post_message(&mut self, post: &PostMessage<'_>) -> Result<String, TransportError> {
        // Standard input carries the body, so the blocks go in a private file of their own,
        // removed when the run ends. Neither is ever an argument a process listing shows.
        let blocks_file = post
            .blocks
            .map(BlocksFile::create)
            .transpose()
            .map_err(|error| {
                TransportError::not_applied(
                    "blocks_file_failed",
                    format!("the post's blocks could not be written for the client: {error}"),
                    true,
                )
            })?;
        let arguments = post_arguments(
            self.options.identity.app_argument(post.app_id),
            post.channel_id,
            post.thread_ts,
            post.format,
            blocks_file.as_ref().map(|file| file.path.as_path()),
        );
        let converted;
        let body: &[u8] = if post.format == TextFormat::Converted {
            converted = crate::mrkdwn::convert_markdown(post.text);
            converted.as_bytes()
        } else {
            post.text.as_bytes()
        };
        let completed = self
            .run(&arguments, Some(body))
            .map_err(|failure| run_failure(failure, true))?;
        post_outcome(&completed)
    }

    fn add_reaction(
        &mut self,
        app_id: &AppId,
        channel_id: &str,
        message_ts: &str,
        name: &str,
    ) -> Result<bool, TransportError> {
        let mut arguments = vec![
            "slack.reaction".to_owned(),
            "add".to_owned(),
            format!("--channel-id={channel_id}"),
            format!("--message-ts={message_ts}"),
            format!("--name={name}"),
        ];
        arguments.extend(self.options.identity.app_argument(app_id));
        arguments.push("--output=json".to_owned());
        let completed = self
            .run(&arguments, None)
            .map_err(|failure| run_failure(failure, true))?;
        if !completed.status.success() {
            let error = exit_failure(&completed, true);
            if error
                .detail
                .to_ascii_lowercase()
                .contains("already_reacted")
            {
                return Ok(true);
            }
            return Err(error);
        }
        let parsed = first_json_value(&completed.stdout);
        if let Some(value) = &parsed {
            if value.get("status").and_then(Value::as_str) == Some("error") {
                return Err(exit_failure(&completed, true));
            }
        }
        Ok(parsed
            .as_ref()
            .and_then(|value| find_bool(value, "changed", 0))
            .is_some_and(|changed| !changed))
    }

    fn list_messages(
        &mut self,
        query: &ListQuery<'_>,
    ) -> Result<Vec<ListedMessage>, TransportError> {
        let mut arguments = vec![
            "slack.message".to_owned(),
            "list".to_owned(),
            format!("--channel-id={}", query.channel_id),
        ];
        if let Some(thread_ts) = query.thread_ts {
            arguments.push(format!("--thread-ts={thread_ts}"));
        }
        arguments.extend([
            format!("--oldest={}", query.oldest),
            "--limit=20".to_owned(),
            "--columns=message_ts,thread_ts,author_user_id,is_author_bot,content".to_owned(),
            "--content-format=text".to_owned(),
        ]);
        arguments.extend(self.options.identity.app_argument(query.app_id));
        arguments.push("--output=json".to_owned());
        let completed = self
            .run(&arguments, None)
            .map_err(|failure| run_failure(failure, false))?;
        if !completed.status.success() {
            return Err(exit_failure(&completed, false));
        }
        let parsed = first_json_value(&completed.stdout).ok_or_else(|| {
            TransportError::not_applied(
                "listing_unreadable",
                "the client listing was not JSON",
                true,
            )
        })?;
        let rows = parsed
            .get("data")
            .and_then(Value::as_array)
            .or_else(|| parsed.as_array())
            .ok_or_else(|| {
                TransportError::not_applied(
                    "listing_unreadable",
                    "the client listing had no rows",
                    true,
                )
            })?;
        Ok(rows
            .iter()
            .filter_map(|row| {
                let ts = row.get("message_ts").and_then(Value::as_str)?;
                ids::valid_ts(ts).then(|| ListedMessage {
                    ts: ts.to_owned(),
                    thread_ts: text_field(row, "thread_ts"),
                    author_user_id: text_field(row, "author_user_id"),
                    is_author_bot: row.get("is_author_bot").and_then(Value::as_bool) == Some(true),
                    content: row
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                })
            })
            .collect())
    }

    fn scan_messages(&mut self, query: &ScanQuery<'_>) -> Result<ScanPage, TransportError> {
        let completed = self
            .run(&scan_arguments(query), None)
            .map_err(|failure| run_failure(failure, false))?;
        if !completed.status.success() {
            return Err(exit_failure(&completed, false));
        }
        let parsed = first_json_value(&completed.stdout).ok_or_else(|| {
            TransportError::not_applied(
                "listing_unreadable",
                "the client listing was not JSON",
                true,
            )
        })?;
        scan_page(&parsed, query.channel_id)
    }

    fn resolve_sender(&mut self, app_id: &AppId, query: &str) -> Result<String, TransportError> {
        let mut arguments = vec![
            "slack.user".to_owned(),
            "describe".to_owned(),
            format!("--user={query}"),
            "--columns=user_id".to_owned(),
        ];
        arguments.extend(self.options.identity.app_argument(app_id));
        arguments.push("--output=json".to_owned());
        let completed = self
            .run(&arguments, None)
            .map_err(|failure| run_failure(failure, false))?;
        resolved_sender(&directory_document(&completed)?)
    }

    fn sender_is_human(&mut self, app_id: &AppId, sender_id: &str) -> Result<bool, TransportError> {
        let mut arguments = vec![
            "slack.user".to_owned(),
            "describe".to_owned(),
            format!("--user={sender_id}"),
            "--columns=user_id,is_bot".to_owned(),
        ];
        arguments.extend(self.options.identity.app_argument(app_id));
        arguments.push("--output=json".to_owned());
        let completed = self
            .run(&arguments, None)
            .map_err(|failure| run_failure(failure, false))?;
        Ok(exact_human_sender(
            &directory_document(&completed)?,
            sender_id,
        ))
    }

    fn add_rejection(
        &mut self,
        app_id: &AppId,
        channel_id: &str,
        message_ts: &str,
        sender_id: &str,
        name: &str,
        before_write: &mut dyn FnMut() -> Result<(), TransportError>,
    ) -> Result<bool, TransportError> {
        let deadline = Instant::now() + self.options.timeout;
        if !self.sender_is_human(app_id, sender_id)? {
            return Err(crate::transport::unverified_rejection_author());
        }
        before_write()?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(TransportError::not_applied(
                "transport_timeout",
                "the human-author check and durable intent exhausted the rejection deadline; no reaction was applied",
                true,
            ));
        }
        let mut reaction = self.clone();
        reaction.options.timeout = remaining;
        reaction.add_reaction(app_id, channel_id, message_ts, name)
    }

    fn list_members(
        &mut self,
        app_id: &AppId,
        channel_id: &str,
    ) -> Result<Vec<String>, TransportError> {
        let deadline = Instant::now() + self.options.timeout;
        let mut members = BTreeSet::new();
        let mut tokens = BTreeSet::new();
        let mut page_token = None;
        // Even an unhelpful client returning unique tokens and empty pages cannot run forever.
        for _ in 0..=chat_subscription::MAX_ALLOWED_SENDERS {
            let mut arguments = vec![
                "slack.conversation".to_owned(),
                "members".to_owned(),
                format!("--channel-id={channel_id}"),
                "--limit=20".to_owned(),
                "--columns=user_id".to_owned(),
            ];
            if let Some(token) = &page_token {
                arguments.push(format!("--page-token={token}"));
            }
            arguments.extend(self.options.identity.app_argument(app_id));
            arguments.push("--output=json".to_owned());
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(TransportError::not_applied(
                    "transport_timeout",
                    "the member listing could not finish every page before its deadline; no partial listing is returned",
                    true,
                ));
            }
            let completed = run_bounded(self.client_command(&arguments), None, remaining)
                .map_err(|failure| run_failure(failure, false))?;
            let (page, next) = member_page(&directory_document(&completed)?)?;
            members.extend(page);
            if members.len() > chat_subscription::MAX_ALLOWED_SENDERS {
                return Err(TransportError::not_applied(
                    "member_limit_exceeded",
                    format!(
                        "the channel has more than {} members; pass explicit sender IDs instead",
                        chat_subscription::MAX_ALLOWED_SENDERS
                    ),
                    false,
                ));
            }
            let Some(token) = next else {
                return Ok(members.into_iter().collect());
            };
            if !tokens.insert(token.clone()) {
                return Err(directory_unreadable(
                    "the member listing repeated a page token; no partial listing is returned",
                ));
            }
            page_token = Some(token);
        }
        Err(directory_unreadable(
            "the member listing exceeded its page limit; no partial listing is returned",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_message_event() {
        let line = r#"{"seq":1,"ts":1.5,"type":"message.created","source":"slack","iris_seq_id":42,
            "payload":{"event_type":"message.created","container_id":"C0C91RW5FAR",
            "slack_message":{"channel_id":"C0C91RW5FAR","message_ts":"1700000000.000100",
            "thread_ts":"1690000000.000001","slack_user_id":"U012ABCDEF","text":"hi &amp; bye"}}}"#
            .replace('\n', "");
        let StreamRecord::Event {
            sequence,
            event_type,
            message,
        } = parse_tail_line(&line)
        else {
            panic!("not an event");
        };
        assert_eq!(sequence, 42);
        assert_eq!(event_type, "message.created");
        let message = message.expect("message");
        assert_eq!(message.thread_ts.as_deref(), Some("1690000000.000001"));
        assert_eq!(message.user_id.as_deref(), Some("U012ABCDEF"));
        assert_eq!(message.text, "hi &amp; bye");
    }

    #[test]
    fn a_message_event_carries_its_file_ids_in_either_form() {
        for files in [
            r#""F0AAAAAAAA1,F0AAAAAAAA2""#,
            r#"["F0AAAAAAAA1","F0AAAAAAAA2","not-a-file"]"#,
        ] {
            let line = format!(
                r#"{{"type":"message.created","iris_seq_id":7,"payload":{{"event_type":"message.created",
                "slack_message":{{"channel_id":"C0C91RW5FAR","message_ts":"1700000000.000100",
                "slack_user_id":"U012ABCDEF","text":"","file_ids":{files}}}}}}}"#
            )
            .replace('\n', "");
            let StreamRecord::Event { message, .. } = parse_tail_line(&line) else {
                panic!("not an event");
            };
            assert_eq!(message.unwrap().file_ids, ["F0AAAAAAAA1", "F0AAAAAAAA2"]);
        }
    }

    #[test]
    fn rescan_listings_decode_the_clients_rows_and_pagination() {
        let query = ScanQuery {
            channel_id: "C0C91RW5FAR",
            thread_ts: Some("1792836353.598709"),
            oldest: "1792836400.000000",
            page_token: Some("bmV4dA"),
        };
        assert_eq!(
            scan_arguments(&query),
            [
                "slack.message",
                "list",
                "--channel-id=C0C91RW5FAR",
                "--thread-ts=1792836353.598709",
                "--oldest=1792836400.000000",
                "--page-token=bmV4dA",
                "--limit=20",
                "--columns=message_ts,thread_ts,author_user_id,subtype,content,file_ids,reply_count,latest_reply",
                "--content-format=mrkdwn",
                "--output=json",
            ]
        );
        // The shape the client printed on 2026-10-24, content shortened.
        let document: Value = serde_json::from_str(
            r#"{"pagination":{"next_page_token":"bmV4dF90cw","has_next_page":true,"page_size":3},
            "completeness":{"source_complete":true},"dropped_row_count":0,"data":[
            {"message_ts":"1792837627.459099","thread_ts":null,"author_user_id":"U0C23P5291P","subtype":null,"content":"a &amp; b","reply_count":null,"file_ids":"","latest_reply":null},
            {"message_ts":"1792836353.598709","thread_ts":"1792836353.598709","author_user_id":"U0C23P5291P","subtype":null,"content":"root","reply_count":6,"file_ids":"","latest_reply":"1792837507.949079"},
            {"message_ts":"1792836302.768889","thread_ts":null,"author_user_id":"U0C23P5291P","subtype":null,"content":"","reply_count":null,"file_ids":"F0CBYK33RNY","latest_reply":null},
            {"message_ts":"not a ts"}]}"#,
        )
        .unwrap();
        let page = scan_page(&document, "C0C91RW5FAR").unwrap();
        assert_eq!(page.next_page_token.as_deref(), Some("bmV4dF90cw"));
        assert_eq!(page.messages.len(), 3);
        assert_eq!(page.messages[0].message.text, "a &amp; b");
        assert_eq!(page.messages[0].message.thread_ts, None);
        assert_eq!(page.messages[1].reply_count, 6);
        assert_eq!(
            page.messages[1].latest_reply.as_deref(),
            Some("1792837507.949079")
        );
        assert_eq!(page.messages[2].message.file_ids, ["F0CBYK33RNY"]);
        assert_eq!(page.messages[2].message.channel_id, "C0C91RW5FAR");
        let last: Value = serde_json::from_str(
            r#"{"pagination":{"next_page_token":null,"has_next_page":false},"data":[]}"#,
        )
        .unwrap();
        assert_eq!(
            scan_page(&last, "C0C91RW5FAR").unwrap(),
            ScanPage::default()
        );
        let inconsistent: Value =
            serde_json::from_str(r#"{"pagination":{"has_next_page":true},"data":[]}"#).unwrap();
        assert_eq!(
            scan_page(&inconsistent, "C0C91RW5FAR").unwrap_err().code,
            "listing_unreadable"
        );
    }

    #[test]
    fn classifies_control_records() {
        assert_eq!(
            parse_tail_line(r#"{"type":"HEARTBEAT","ts":1.0}"#),
            StreamRecord::Heartbeat
        );
        assert!(matches!(
            parse_tail_line(r#"{"type":"GAP","lost_from_iris_seq_id":1,"lost_to_iris_seq_id":5}"#),
            StreamRecord::Gap(_)
        ));
        assert!(matches!(
            parse_tail_line("Tailing events"),
            StreamRecord::Notice(_)
        ));
        assert!(matches!(
            classify_tail_stderr("queue overflow: the requested position is too far behind"),
            StreamRecord::Gap(_)
        ));
    }

    fn sequence_spec(start: u64) -> SubscribeSpec {
        SubscribeSpec {
            channel_ids: vec!["C0C91RW5FAR".to_owned()],
            catchup: Catchup::Sequence(start),
        }
    }

    // The client's standard error around a catch-up, as captured from a live tail (colour codes
    // included), with its position values as parameters.
    fn catchup_stderr(start: u64, response: &str) -> Vec<String> {
        vec![
            "\u{1b}[2mUsing port: 46507\u{1b}[0m".to_owned(),
            format!("Catching up from {start}..."),
            "Fetched 0 events".to_owned(),
            "Catch-up complete (0 events). Switching to live streaming...".to_owned(),
            "\u{1b}[33mSubscribing to agent events for user 1 (mode: explicit)...\u{1b}[0m"
                .to_owned(),
            format!("\u{1b}[2mSubscribe response: {response}\u{1b}[0m"),
            "\u{1b}[32mTailing events (Ctrl+C to stop)...\u{1b}[0m".to_owned(),
        ]
    }

    const RESPONSE: &str = "SubscribeResponse(seqId=225029, lastIssuedSeqId=225029, \
        syncId=1009015592, requestedPositionSeqId=225029)";

    fn prove(proof: &mut PositionProof, stderr: &[String]) -> Option<u64> {
        let mut proven = None;
        for line in stderr {
            proven = proven.or(proof.stderr(line));
        }
        proven.or_else(|| proof.stdout(&StreamRecord::Heartbeat))
    }

    #[test]
    fn a_completed_catchup_proves_its_subscribe_position_after_a_heartbeat() {
        let mut proof = PositionProof::new(&sequence_spec(223_201), None);
        for line in catchup_stderr(223_201, RESPONSE) {
            assert_eq!(proof.stderr(&line), None, "no heartbeat yet: {line}");
        }
        // An event is not the barrier; only a heartbeat is written strictly after the response.
        assert_eq!(proof.stdout(&parse_tail_line(r#"{"type":"STALL"}"#)), None);
        assert_eq!(proof.stdout(&StreamRecord::Heartbeat), Some(225_029));
        // A position is proven once per stream.
        assert_eq!(proof.stdout(&StreamRecord::Heartbeat), None);
    }

    #[test]
    fn a_heartbeat_read_before_the_response_is_still_the_barrier() {
        let mut proof = PositionProof::new(&sequence_spec(223_201), Some("explicit"));
        assert_eq!(proof.stdout(&StreamRecord::Heartbeat), None);
        let stderr = catchup_stderr(223_201, RESPONSE);
        let proven: Vec<_> = stderr
            .iter()
            .filter_map(|line| proof.stderr(line))
            .collect();
        assert_eq!(proven, [225_029]);
    }

    #[test]
    fn nothing_unproven_becomes_a_position() {
        let ok = catchup_stderr(100, RESPONSE);
        // Not a sequence catch-up, or not the explicit mode.
        for (spec, mode) in [
            (
                SubscribeSpec {
                    catchup: Catchup::SinceSeconds(60),
                    ..sequence_spec(100)
                },
                None,
            ),
            (
                SubscribeSpec {
                    catchup: Catchup::None,
                    ..sequence_spec(100)
                },
                None,
            ),
            (sequence_spec(100), Some("override")),
        ] {
            assert_eq!(prove(&mut PositionProof::new(&spec, mode), &ok), None);
        }
        // The catch-up started elsewhere, failed (no "Fetched"), or came after the response.
        let mut other_start = ok.clone();
        other_start[1] = "Catching up from 99...".to_owned();
        let mut overflow = ok.clone();
        overflow[2] =
            "queue overflow: the requested position is too far behind the current head".to_owned();
        let mut late = ok.clone();
        late.swap(1, 5);
        late.swap(2, 5);
        for stderr in [other_start, overflow, late] {
            assert_eq!(
                prove(&mut PositionProof::new(&sequence_spec(100), None), &stderr),
                None,
                "{stderr:?}"
            );
        }
        // Responses that do not resume where catch-up ended.
        for response in [
            "SubscribeResponse(seqId=225030, lastIssuedSeqId=225030, syncId=1, requestedPositionSeqId=225029)",
            "SubscribeResponse(seqId=225029, lastIssuedSeqId=225028, syncId=1, requestedPositionSeqId=225029)",
            "SubscribeResponse(seqId=99, lastIssuedSeqId=225029, syncId=1, requestedPositionSeqId=99)",
            "SubscribeResponse(seqId=225029, lastIssuedSeqId=225029, syncId=1)",
            "SubscribeResponse(seqId=None, lastIssuedSeqId=225029, syncId=1, requestedPositionSeqId=None)",
            "SubscribeResponse(seqId=1, seqId=225029, lastIssuedSeqId=225029, requestedPositionSeqId=225029)",
            "subscribed",
        ] {
            let stderr = catchup_stderr(100, response);
            assert_eq!(
                prove(&mut PositionProof::new(&sequence_spec(100), None), &stderr),
                None,
                "{response}"
            );
        }
        // Only the first response counts.
        let mut proof = PositionProof::new(&sequence_spec(100), None);
        let mut twice = catchup_stderr(
            100,
            "SubscribeResponse(seqId=1, lastIssuedSeqId=1, syncId=1, requestedPositionSeqId=1)",
        );
        twice.push(format!("Subscribe response: {RESPONSE}"));
        assert_eq!(prove(&mut proof, &twice), None);
    }

    #[test]
    fn tail_arguments_reflect_the_catchup() {
        let spec = SubscribeSpec {
            channel_ids: vec!["C0C91RW5FAR".to_owned()],
            catchup: Catchup::Sequence(77),
        };
        let arguments = tail_arguments(&spec, &TailOptions::default());
        assert!(arguments.contains(&"--catchup=77".to_owned()));
        assert!(arguments.contains(&"--channel-id=C0C91RW5FAR".to_owned()));
        let since = SubscribeSpec {
            catchup: Catchup::SinceSeconds(30),
            ..spec
        };
        assert!(tail_arguments(&since, &TailOptions::default()).contains(&"--since=30s".to_owned()));
    }

    #[test]
    fn bounded_lines() {
        let mut reader = BufReader::new(&b"abc\ndefghij\nk"[..]);
        let mut line = Vec::new();
        assert_eq!(
            read_bounded_line(&mut reader, &mut line, 4).unwrap(),
            Some(false)
        );
        assert_eq!(line, b"abc");
        assert_eq!(
            read_bounded_line(&mut reader, &mut line, 4).unwrap(),
            Some(true)
        );
        assert_eq!(line, b"defg");
        assert_eq!(
            read_bounded_line(&mut reader, &mut line, 4).unwrap(),
            Some(false)
        );
        assert_eq!(line, b"k");
        assert_eq!(read_bounded_line(&mut reader, &mut line, 4).unwrap(), None);
    }
}
