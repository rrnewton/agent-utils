//! Transport over an installed Slack command-line client.
//!
//! The operator configures the client's absolute path. The transport uses four of its commands:
//!
//! | Operation | Command |
//! | --- | --- |
//! | inbound stream | `slack.conversation tail --channel-id=C… --event-types=message.created --include-iris-seq-id --output=json --heartbeats --catchup=…` |
//! | post | `slack.message send --channel-id=C… --markdown=file://- [--thread-ts=…] [--as=A…] --output=json` |
//! | reaction | `slack.reaction add --channel-id=C… --message-ts=… --name=… [--as=A…] --output=json` |
//! | listing | `slack.message list --channel-id=C… [--thread-ts=…] --oldest=… [--as=A…] --output=json` |
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
//! "slack_user_id", "bot_id", "app_id", "subtype", "text", "team_id"}}}`, plus
//! `{"type": "HEARTBEAT"}` and `{"type": "GAP", ...}` records.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::ids::{self, AppId};
use crate::transport::{
    Catchup, EventStream, ListQuery, ListedMessage, PostMessage, SlackMessage, SlackTransport,
    StreamCancel, StreamRecord, StreamWait, SubscribeSpec, TextFormat, TransportError,
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
}

impl EventStream for CliStream {
    fn next(&mut self, wait: Duration) -> StreamWait {
        if self.cancelled {
            return StreamWait::Cancelled;
        }
        match self.receiver.recv_timeout(wait) {
            Ok(Pumped::Record(record)) => StreamWait::Record(record),
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
            let record = classify_tail_stderr(&String::from_utf8_lossy(&line));
            if sender.send(Pumped::Record(record)).is_err() {
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
    let parsed = first_json_value(&completed.stdout);
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
    let code = [
        "ratelimited",
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
        RunFailure::Wait(error) => {
            TransportError::unknown("transport_wait_failed", format!("waiting failed: {error}"))
        }
    }
}

/// The client arguments of one post, whose body the client reads from standard input. `app`
/// is the `--as` argument, if the client takes one: see [`ClientIdentity::app_argument`].
#[must_use]
pub(crate) fn post_arguments(
    app: Option<String>,
    channel_id: &str,
    thread_ts: Option<&str>,
    format: TextFormat,
) -> Vec<String> {
    let body_flag = match format {
        TextFormat::Markdown => "--markdown=file://-",
        TextFormat::Text => "--text=file://-",
    };
    let mut arguments = vec![
        "slack.message".to_owned(),
        "send".to_owned(),
        format!("--channel-id={channel_id}"),
        body_flag.to_owned(),
    ];
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
        }))
    }

    fn post_message(&mut self, post: &PostMessage<'_>) -> Result<String, TransportError> {
        let arguments = post_arguments(
            self.options.identity.app_argument(post.app_id),
            post.channel_id,
            post.thread_ts,
            post.format,
        );
        let completed = self
            .run(&arguments, Some(post.text.as_bytes()))
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
