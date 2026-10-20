//! The warm outbound worker: a Slack client already started for a reply that is likely soon.
//!
//! One run of the Slack command-line client spends most of its time starting up. Measured on a
//! production host, a post took 1.2 to 1.7 seconds, of which about a second passed before the
//! client read its standard input; the rest was authorisation and the Slack call. The client has
//! no mode that keeps one process for many operations, so the outbound helper cannot reuse one.
//!
//! The warm worker is a long-lived process (`agentctl-slack-outbound --role=warm-worker`) that
//! starts the client's `slack.message send` for a thread *before* the reply exists, with the body
//! to come on standard input, and holds it, waiting on that input, until the reply arrives or a
//! time to live passes. A post through such a spare skips the start-up.
//!
//! The one-shot helper talks to the worker over a Unix socket (`--warm-socket=PATH`):
//!
//! * When the helper adds a reaction to a message, it first asks the worker to `prepare` a spare
//!   for a reply in that message's thread, `C…/threads/<message ts>`, which is where the reply to
//!   a top-level message goes. The reaction is usually the request's acknowledgement, made as the
//!   request is admitted, so the spare starts while the agent works. A reply to a message inside
//!   a thread goes to the thread root instead, so that spare goes unused until it expires.
//! * When the helper posts a reply, it asks the worker to `post` it through the spare for exactly
//!   that app, client, format, channel and thread. The worker takes the spare and answers
//!   `ready`, or answers `no_spare`; it writes the body to the spare only after the helper then
//!   sends `commit`. So until the helper has sent `commit`, the worker provably has posted
//!   nothing, and the helper posts with its own client run, as it does without a worker, after
//!   `no_spare`, after any failure, and when no `ready` comes within 2 seconds (a stopped or hung
//!   worker). A worker that then gets no `commit` discards the spare. After `commit`, a failure
//!   the worker proves was not applied is retried with the helper's own run if it is retryable,
//!   is not a rate limit, and came within 4 seconds; any other failure, including a connection
//!   that breaks, is reported with an unknown outcome, so the helper's ledger reconciles it and
//!   the reply is never posted twice by a fallback.
//!
//! A spare that is not used within `--warm-ttl-seconds` (default 300) is killed, as is the oldest
//! once `--warm-max-spares` (default 2) are held. A waiting client is killed before its input is
//! closed, and the client refuses an empty body in any case, so a spare that is discarded posts
//! nothing. The body is written with one `write` into an empty pipe that holds it whole, and each
//! spare is started with a parent-death signal, so a worker that dies while writing cannot leave
//! a client holding part of a body.
//!
//! Each spare costs what one waiting client costs: on the measured host about 225 to 310 MB of
//! private memory and half a percent of one core, for at most the time to live. The worker
//! itself blocks in `accept` and on a condition variable; it polls nothing.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::cli::{self, CliTransport, ClientIdentity, Completed};
use crate::diag::Diagnostics;
use crate::ids::{self, AppId};
use crate::transport::{
    bounded, EventStream, ListQuery, ListedMessage, Outcome, PostMessage, SlackTransport,
    SubscribeSpec, TextFormat, TransportError,
};

/// Wire version of the socket protocol.
// 2 added the client identity and prefix arguments to every request, so a worker started for
// another identity than the helper's holds no spare for it (#383 slack-crewmate-outbound).
const PROTOCOL_VERSION: u64 = 2;
/// Largest request line the worker reads: a body of at most 12,000 bytes, JSON-escaped.
const MAX_REQUEST_LINE_BYTES: usize = 256 * 1024;
/// Largest response line the helper reads.
const MAX_RESPONSE_LINE_BYTES: usize = 16 * 1024;
/// How long the helper waits for the worker to accept a `prepare` and answer it.
const PREPARE_TIMEOUT: Duration = Duration::from_secs(1);
/// How long the helper waits for the worker to say it holds a spare for a post (`ready`), and
/// how long the worker then waits for the helper's `commit`. Until the worker reads `commit` it
/// writes nothing to the spare, so a helper that gives up first may post with its own client run.
const READY_TIMEOUT: Duration = Duration::from_secs(2);
/// A post the worker refused as not applied is retried with the helper's own client run only if
/// the refusal came this soon, so the helper still answers within the bridge's deadline.
const FALLBACK_WITHIN: Duration = Duration::from_secs(4);
/// How long the worker waits for a request line after accepting a connection.
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Connections the worker serves at once; more are refused.
const MAX_CONNECTIONS: usize = 16;
/// The longest path a Unix socket address holds on Linux.
pub const MAX_SOCKET_PATH_BYTES: usize = 107;

/// The worker's settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WarmSettings {
    /// The worker's Unix socket.
    pub socket: PathBuf,
    /// The most spares held at once.
    pub max_spares: usize,
    /// How long an unused spare is kept.
    pub ttl: Duration,
}

/// What one helper needs to ask the worker for a spare: the identity every spare must match.
#[derive(Clone, Debug)]
pub struct WarmClient {
    socket: PathBuf,
    cli: PathBuf,
    app_id: AppId,
    format: TextFormat,
    identity: ClientIdentity,
    prefix_arguments: Vec<String>,
    timeout: Duration,
    diagnostics: Diagnostics,
}

/// The worker's answer to a `post`.
#[derive(Debug, PartialEq, Eq)]
pub enum WarmPost {
    /// Nothing was attempted: no matching spare, or no worker. The reason is a diagnostic.
    NoSpare(String),
    /// Posted; the message `ts`.
    Posted(String),
    /// The post failed, with application evidence.
    Failed(TransportError),
}

fn identity_name(identity: ClientIdentity) -> &'static str {
    match identity {
        ClientIdentity::User => "user",
        ClientIdentity::Crewmate => "crewmate",
    }
}

fn format_name(format: TextFormat) -> &'static str {
    match format {
        TextFormat::Markdown => "markdown",
        TextFormat::Text => "text",
    }
}

impl WarmClient {
    /// A client of the worker at `socket` for spares of this client, app and format.
    #[must_use]
    pub fn new(
        socket: PathBuf,
        cli: PathBuf,
        app_id: AppId,
        format: TextFormat,
        timeout: Duration,
        diagnostics: Diagnostics,
    ) -> Self {
        Self {
            socket,
            cli,
            app_id,
            format,
            identity: ClientIdentity::User,
            prefix_arguments: Vec::new(),
            timeout,
            diagnostics,
        }
    }

    /// The same client, run with `prefix_arguments` before its subcommand and acting as
    /// `identity`; a spare must match both.
    #[must_use]
    pub fn with_client(mut self, identity: ClientIdentity, prefix_arguments: Vec<String>) -> Self {
        self.identity = identity;
        self.prefix_arguments = prefix_arguments;
        self
    }

    fn request(&self, op: &str, channel_id: &str, thread_ts: Option<&str>) -> Value {
        json!({
            "version": PROTOCOL_VERSION,
            "op": op,
            "cli": self.cli,
            "app_id": self.app_id.as_str(),
            "format": format_name(self.format),
            "client_identity": identity_name(self.identity),
            "cli_arguments": self.prefix_arguments,
            "channel_id": channel_id,
            "thread_ts": thread_ts,
        })
    }

    /// Ask the worker to start a spare for a reply in `thread_ts`. Best effort: a worker that is
    /// not running, or any failure, is logged and otherwise ignored.
    pub fn prepare(&self, channel_id: &str, thread_ts: &str) {
        let request = self.request("prepare", channel_id, Some(thread_ts));
        match exchange(&self.socket, &request, PREPARE_TIMEOUT) {
            Ok(Exchange::Answered(response)) => {
                let spare = response
                    .get("spare")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                self.diagnostics
                    .log(&format!("warm prepare {channel_id} {thread_ts}: {spare}"));
            }
            Ok(Exchange::NotSent(reason)) | Err(reason) => {
                self.diagnostics
                    .log(&format!("warm prepare {channel_id} {thread_ts}: {reason}"));
            }
        }
    }

    /// Ask the worker to post `post` through a matching spare.
    #[must_use]
    pub fn post(&self, post: &PostMessage<'_>) -> WarmPost {
        if post.app_id != &self.app_id || post.format != self.format {
            return WarmPost::NoSpare("the post does not match this worker client".to_owned());
        }
        let Some(thread_ts) = post.thread_ts else {
            return WarmPost::NoSpare("only thread replies have spares".to_owned());
        };
        let mut request = self.request("post", post.channel_id, Some(thread_ts));
        request["text"] = Value::String(post.text.to_owned());
        // First the request. The worker answers `ready` once it holds a spare for this post,
        // having written nothing to it, or `no_spare`. Anything else, including no answer within
        // `READY_TIMEOUT`, leaves the worker unable to post: it writes the body only after it
        // reads `commit`.
        let mut stream = match send_request(&self.socket, &request, READY_TIMEOUT) {
            Ok(stream) => stream,
            Err(reason) => return WarmPost::NoSpare(reason),
        };
        let ready = match read_answer(&mut stream) {
            Ok(answer) => answer,
            Err(reason) => return WarmPost::NoSpare(format!("no ready answer: {reason}")),
        };
        match ready.get("result").and_then(Value::as_str) {
            Some("ready")
                if ready.get("version").and_then(Value::as_u64) == Some(PROTOCOL_VERSION) => {}
            Some("no_spare") => return parse_post_response(&ready),
            _ => return WarmPost::NoSpare("the warm worker did not take the post".to_owned()),
        }
        // Then `commit`. A failed write sends nothing of this one short line, so the worker never
        // reads it and discards the spare unused. Once it is sent, the worker may post.
        let mut commit = serde_json::to_vec(&json!({"version": PROTOCOL_VERSION, "op": "commit"}))
            .unwrap_or_default();
        commit.push(b'\n');
        if let Err(error) = stream
            .set_read_timeout(Some(self.timeout + Duration::from_secs(1)))
            .and_then(|()| stream.write_all(&commit))
        {
            return WarmPost::NoSpare(format!("the commit did not reach the warm worker: {error}"));
        }
        let _ = stream.shutdown(std::net::Shutdown::Write);
        match read_answer(&mut stream) {
            Ok(answer) => parse_post_response(&answer),
            Err(reason) => WarmPost::Failed(TransportError::unknown(
                "warm_worker_response_unknown",
                format!("the warm worker took the post but its answer was lost: {reason}"),
            )),
        }
    }
}

fn parse_post_response(response: &Value) -> WarmPost {
    if response.get("version").and_then(Value::as_u64) != Some(PROTOCOL_VERSION) {
        return WarmPost::Failed(TransportError::unknown(
            "warm_worker_protocol",
            "the warm worker answered with another protocol version",
        ));
    }
    match response.get("result").and_then(Value::as_str) {
        Some("no_spare") => WarmPost::NoSpare(
            response
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("no spare")
                .to_owned(),
        ),
        Some("posted") => match response.get("ts").and_then(Value::as_str) {
            Some(ts) if ids::valid_ts(ts) => WarmPost::Posted(ts.to_owned()),
            _ => WarmPost::Failed(TransportError::unknown(
                "receipt_invalid",
                "the warm worker reported a post without a valid ts",
            )),
        },
        Some("failed") => {
            let code = response
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("warm_worker_failed");
            let detail = response
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("no detail")
                .to_owned();
            let retryable = response
                .get("retryable")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            WarmPost::Failed(match response.get("outcome").and_then(Value::as_str) {
                Some("not_applied") => TransportError::not_applied(code, detail, retryable),
                _ => TransportError::unknown(code, detail),
            })
        }
        _ => WarmPost::Failed(TransportError::unknown(
            "warm_worker_protocol",
            "the warm worker's answer names no result",
        )),
    }
}

enum Exchange {
    /// The request line never reached the worker whole, so it did nothing.
    NotSent(String),
    /// The worker's answer.
    Answered(Value),
}

/// Connect and send one request line, with `timeout` on both directions. An error means the
/// worker cannot have acted on the request: it reads a request only once its closing newline has
/// arrived, a failed write call sends nothing, and the newline is the last byte.
fn send_request(socket: &Path, request: &Value, timeout: Duration) -> Result<UnixStream, String> {
    let mut stream =
        UnixStream::connect(socket).map_err(|error| format!("no warm worker: {error}"))?;
    let mut line = serde_json::to_vec(request).unwrap_or_default();
    line.push(b'\n');
    stream
        .set_write_timeout(Some(timeout))
        .and_then(|()| stream.set_read_timeout(Some(timeout)))
        .map_err(|error| format!("the warm worker connection could not be set up: {error}"))?;
    stream
        .write_all(&line)
        .and_then(|()| stream.flush())
        .map_err(|error| format!("the request did not reach the warm worker: {error}"))?;
    Ok(stream)
}

/// Read one answer line from the worker, at most `MAX_RESPONSE_LINE_BYTES`, byte by byte so
/// nothing after it is consumed.
fn read_answer(stream: &mut UnixStream) -> Result<Value, String> {
    let mut answer = Vec::new();
    let mut byte = [0_u8; 1];
    while answer.len() <= MAX_RESPONSE_LINE_BYTES {
        match stream.read(&mut byte) {
            Ok(0) => return Err("the warm worker closed without a whole answer".to_owned()),
            Ok(_) => {
                answer.push(byte[0]);
                if byte[0] == b'\n' {
                    return serde_json::from_slice(&answer)
                        .map_err(|_| "the warm worker's answer is not JSON".to_owned());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                return Err(format!("reading the warm worker's answer failed: {error}"));
            }
        }
    }
    Err("the warm worker's answer is too long".to_owned())
}

/// Send one request line and read one answer line, for a request that changes nothing the helper
/// relies on (`prepare`). `Ok(NotSent)` means the worker cannot have acted on the request; `Err`
/// means it was sent but no answer was read.
fn exchange(socket: &Path, request: &Value, timeout: Duration) -> Result<Exchange, String> {
    let mut stream = match send_request(socket, request, timeout) {
        Ok(stream) => stream,
        Err(reason) => return Ok(Exchange::NotSent(reason)),
    };
    let _ = stream.shutdown(std::net::Shutdown::Write);
    read_answer(&mut stream).map(Exchange::Answered)
}

/// A [`SlackTransport`] that posts thread replies through the warm worker's spares when it has
/// one, asks it to prepare a spare before each reaction, and does everything else, including
/// every post the worker does not take, with `inner`.
pub struct WarmTransport {
    inner: Box<dyn SlackTransport>,
    client: WarmClient,
}

impl WarmTransport {
    /// Wrap `inner`.
    #[must_use]
    pub fn new(inner: Box<dyn SlackTransport>, client: WarmClient) -> Self {
        Self { inner, client }
    }
}

impl SlackTransport for WarmTransport {
    fn open_stream(
        &mut self,
        spec: &SubscribeSpec,
    ) -> Result<Box<dyn EventStream>, TransportError> {
        self.inner.open_stream(spec)
    }

    fn post_message(&mut self, post: &PostMessage<'_>) -> Result<String, TransportError> {
        let started = Instant::now();
        match self.client.post(post) {
            WarmPost::Posted(ts) => {
                self.client.diagnostics.log(&format!(
                    "warm post {} {}: posted {ts}",
                    post.channel_id,
                    post.thread_ts.unwrap_or("-")
                ));
                Ok(ts)
            }
            WarmPost::NoSpare(reason) => {
                self.client.diagnostics.log(&format!(
                    "warm post {} {}: {reason}; posting with a new client run",
                    post.channel_id,
                    post.thread_ts.unwrap_or("-")
                ));
                self.inner.post_message(post)
            }
            // Proven not to have happened, so a new client run cannot post it twice. A refusal
            // that repeating cannot fix, a rate limit, or one that came late is returned as it is.
            WarmPost::Failed(error)
                if error.outcome == Outcome::NotApplied
                    && error.retryable
                    && error.code != "ratelimited"
                    && started.elapsed() < FALLBACK_WITHIN =>
            {
                self.client.diagnostics.log(&format!(
                    "warm post {} {}: not applied ({}); posting with a new client run",
                    post.channel_id,
                    post.thread_ts.unwrap_or("-"),
                    error.code
                ));
                self.inner.post_message(post)
            }
            WarmPost::Failed(error) => Err(error),
        }
    }

    fn add_reaction(
        &mut self,
        app_id: &AppId,
        channel_id: &str,
        message_ts: &str,
        name: &str,
    ) -> Result<bool, TransportError> {
        if app_id == &self.client.app_id {
            self.client.prepare(channel_id, message_ts);
        }
        self.inner
            .add_reaction(app_id, channel_id, message_ts, name)
    }

    fn list_messages(
        &mut self,
        query: &ListQuery<'_>,
    ) -> Result<Vec<ListedMessage>, TransportError> {
        self.inner.list_messages(query)
    }

    fn resolve_sender(&mut self, app_id: &AppId, query: &str) -> Result<String, TransportError> {
        self.inner.resolve_sender(app_id, query)
    }

    fn list_members(
        &mut self,
        app_id: &AppId,
        channel_id: &str,
    ) -> Result<Vec<String>, TransportError> {
        self.inner.list_members(app_id, channel_id)
    }

    fn sender_is_human(&mut self, app_id: &AppId, sender_id: &str) -> Result<bool, TransportError> {
        self.inner.sender_is_human(app_id, sender_id)
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
        self.inner.add_rejection(
            app_id,
            channel_id,
            message_ts,
            sender_id,
            name,
            before_write,
        )
    }
}

// ---------------------------------------------------------------------------------------------
// The worker.

#[derive(Clone, Debug, PartialEq, Eq)]
struct SpareKey {
    channel_id: String,
    thread_ts: String,
}

/// One waiting client.
struct Spare {
    key: SpareKey,
    child: Child,
    stdin: ChildStdin,
    stdout: thread::JoinHandle<Vec<u8>>,
    stderr: thread::JoinHandle<Vec<u8>>,
    expires: Instant,
}

impl Spare {
    /// Close the input of a client that has already exited and been reaped, and collect its
    /// output threads. Its process group may be gone, so it is not signalled.
    fn release_reaped(self) {
        drop(self.stdin);
        let _ = self.stdout.join();
        let _ = self.stderr.join();
    }

    /// Kill the waiting client and its process group, then close its input and reap it. The
    /// client dies before it can read the end of its input, so it posts nothing.
    fn discard(mut self) {
        kill_group(&self.child);
        let _ = self.child.kill();
        let _ = self.child.wait();
        drop(self.stdin);
        let _ = self.stdout.join();
        let _ = self.stderr.join();
    }
}

#[derive(Default)]
struct PoolState {
    spares: Vec<Spare>,
    /// Threads whose spare was used, until they expire: a later `prepare` for one of them, such
    /// as the receipt reaction's, starts no new spare.
    used: VecDeque<(SpareKey, Instant)>,
    /// Threads whose spare is being started.
    starting: Vec<SpareKey>,
}

struct Pool {
    state: Mutex<PoolState>,
    changed: Condvar,
}

impl Pool {
    fn lock(&self) -> std::sync::MutexGuard<'_, PoolState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// What the worker needs to start and use spares.
struct Worker {
    transport: CliTransport,
    cli: PathBuf,
    app_id: AppId,
    format: TextFormat,
    channels: Vec<String>,
    timeout: Duration,
    settings: WarmSettings,
    pool: Pool,
    spawner: Mutex<mpsc::Sender<SpawnJob>>,
    connections: AtomicUsize,
    diagnostics: Diagnostics,
}

/// Releases one of the worker's `MAX_CONNECTIONS` slots when the connection's thread ends,
/// however it ends.
struct ConnectionSlot<'a>(&'a AtomicUsize);

impl Drop for ConnectionSlot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct SpawnJob {
    key: SpareKey,
    done: mpsc::Sender<io::Result<Spare>>,
}

/// Run the worker until it fails. It holds an exclusive lock on `<socket>.lock` for its whole
/// life, so a second worker for the same socket refuses to start.
///
/// # Errors
///
/// Returns a description when the socket cannot be served.
#[allow(clippy::too_many_arguments)]
pub fn serve(
    transport: CliTransport,
    cli: PathBuf,
    app_id: AppId,
    format: TextFormat,
    channels: Vec<String>,
    timeout: Duration,
    settings: WarmSettings,
    diagnostics: Diagnostics,
) -> Result<(), String> {
    let lock = lock_socket(&settings.socket)?;
    restrict_umask();
    let listener = bind(&settings.socket)?;
    let (spawn_sender, spawn_receiver) = mpsc::channel::<SpawnJob>();
    let worker = Arc::new(Worker {
        transport,
        cli,
        app_id,
        format,
        channels,
        timeout,
        settings,
        pool: Pool {
            state: Mutex::new(PoolState::default()),
            changed: Condvar::new(),
        },
        spawner: Mutex::new(spawn_sender),
        connections: AtomicUsize::new(0),
        diagnostics,
    });
    // Spares are started from this one long-lived thread, because a parent-death signal fires
    // when the thread that started the child exits, not only the process.
    let spawning = Arc::clone(&worker);
    thread::Builder::new()
        .name("slack-warm-spawn".to_owned())
        .spawn(move || {
            for job in spawn_receiver {
                let _ = job.done.send(spawning.start_spare(job.key));
            }
        })
        .map_err(|error| format!("the spare thread could not start: {error}"))?;
    let expiring = Arc::clone(&worker);
    thread::Builder::new()
        .name("slack-warm-expiry".to_owned())
        .spawn(move || expiring.expire_forever())
        .map_err(|error| format!("the expiry thread could not start: {error}"))?;
    worker.diagnostics.log(&format!(
        "warm worker serving {} (at most {} spares, {} s each)",
        worker.settings.socket.display(),
        worker.settings.max_spares,
        worker.settings.ttl.as_secs()
    ));
    for connection in listener.incoming() {
        let stream = match connection {
            Ok(stream) => stream,
            Err(error) => {
                worker
                    .diagnostics
                    .log(&format!("warm worker accept failed: {error}"));
                // A failure that repeats, such as running out of descriptors, must not spin.
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        if worker.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            worker.connections.fetch_sub(1, Ordering::SeqCst);
            let _ = respond(
                &stream,
                &json!({"version": PROTOCOL_VERSION, "result": "no_spare", "detail": "the warm worker is busy"}),
            );
            continue;
        }
        // Kept so a connection whose thread cannot start is still told that nothing was done.
        let refusal = stream.try_clone();
        let serving = Arc::clone(&worker);
        let spawned = thread::Builder::new()
            .name("slack-warm-conn".to_owned())
            .spawn(move || {
                let _counted = ConnectionSlot(&serving.connections);
                // A panic here closes the connection: before `commit` the helper then posts with
                // its own client run, and after it the outcome is unknown, as it should be.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    serving.serve_connection(stream);
                }));
            });
        if spawned.is_err() {
            worker.connections.fetch_sub(1, Ordering::SeqCst);
            if let Ok(refusal) = refusal {
                let _ = respond(
                    &refusal,
                    &no_spare("the warm worker could not serve the connection"),
                );
            }
        }
    }
    drop(lock);
    Err("the warm worker socket stopped accepting connections".to_owned())
}

#[allow(unsafe_code)]
fn lock_socket(socket: &Path) -> Result<File, String> {
    let mut path = socket.as_os_str().to_owned();
    path.push(".lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .map_err(|error| format!("the warm worker lock file cannot be opened: {error}"))?;
    // SAFETY: `flock` takes a descriptor this function owns and flags; it reads no memory.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(format!(
            "another warm worker holds {}: {}",
            Path::new(&path).display(),
            io::Error::last_os_error()
        ));
    }
    Ok(file)
}

/// Make every file this process creates private from its creation, so the socket is never
/// reachable by another user between `bind` and its `chmod`.
#[allow(unsafe_code)]
fn restrict_umask() {
    // SAFETY: `umask` only sets this process's file-creation mask and returns the old one.
    unsafe {
        libc::umask(0o077);
    }
}

fn bind(socket: &Path) -> Result<UnixListener, String> {
    if socket.as_os_str().len() > MAX_SOCKET_PATH_BYTES {
        return Err(format!(
            "--warm-socket is longer than {MAX_SOCKET_PATH_BYTES} bytes"
        ));
    }
    // The lock is held, so a socket file left here belongs to a worker that has exited.
    match fs::symlink_metadata(socket) {
        Ok(metadata) => {
            use std::os::unix::fs::FileTypeExt;
            if !metadata.file_type().is_socket() {
                return Err(format!("{} exists and is not a socket", socket.display()));
            }
            fs::remove_file(socket).map_err(|error| {
                format!("the old warm worker socket cannot be removed: {error}")
            })?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("the warm worker socket cannot be read: {error}")),
    }
    let listener = UnixListener::bind(socket)
        .map_err(|error| format!("the warm worker socket cannot be bound: {error}"))?;
    fs::set_permissions(socket, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("the warm worker socket mode cannot be set: {error}"))?;
    Ok(listener)
}

/// Read one line of at most `limit` bytes, newline included; `None` at end of input, on an
/// error or timeout, or when the line is longer.
fn read_line(reader: &mut BufReader<UnixStream>, limit: usize) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    match reader
        .by_ref()
        .take(u64::try_from(limit).unwrap_or(u64::MAX))
        .read_until(b'\n', &mut line)
    {
        Ok(_) if line.last() == Some(&b'\n') => Some(line),
        _ => None,
    }
}

fn respond(mut stream: &UnixStream, response: &Value) -> io::Result<()> {
    let mut line = serde_json::to_vec(response).unwrap_or_default();
    line.push(b'\n');
    stream.write_all(&line)
}

fn no_spare(detail: &str) -> Value {
    json!({"version": PROTOCOL_VERSION, "result": "no_spare", "detail": detail})
}

fn failed(error: &TransportError) -> Value {
    json!({
        "version": PROTOCOL_VERSION,
        "result": "failed",
        "code": error.code,
        "detail": bounded(&error.detail),
        "outcome": match error.outcome {
            Outcome::NotApplied => "not_applied",
            Outcome::Unknown => "unknown",
        },
        "retryable": error.retryable,
    })
}

impl Worker {
    fn serve_connection(&self, stream: UnixStream) {
        let _ = stream.set_read_timeout(Some(REQUEST_READ_TIMEOUT));
        let mut reader = match stream.try_clone() {
            Ok(clone) => BufReader::new(clone),
            Err(_) => {
                let _ = respond(&stream, &no_spare("the connection could not be read"));
                return;
            }
        };
        let Some(line) = read_line(&mut reader, MAX_REQUEST_LINE_BYTES) else {
            // Only a whole line is a request: a helper that stopped part-way asked for nothing,
            // and is told so in case it is still listening.
            let _ = respond(&stream, &no_spare("no whole request line arrived"));
            return;
        };
        let response = match serde_json::from_slice::<Value>(&line) {
            Ok(request) => self.handle(&request, &stream, &mut reader),
            Err(_) => no_spare("the request is not JSON"),
        };
        let _ = respond(&stream, &response);
    }

    /// Check that a request is for this worker's client, app and format and an allowed channel,
    /// and return its spare key.
    fn key(&self, request: &Value) -> Result<SpareKey, String> {
        if request.get("version").and_then(Value::as_u64) != Some(PROTOCOL_VERSION) {
            return Err("unsupported protocol version".to_owned());
        }
        if request.get("cli").and_then(Value::as_str) != self.cli.to_str()
            || request.get("app_id").and_then(Value::as_str) != Some(self.app_id.as_str())
            || request.get("format").and_then(Value::as_str) != Some(format_name(self.format))
        {
            return Err("the request is for another client, app or format".to_owned());
        }
        let options = self.transport.options();
        if request.get("client_identity").and_then(Value::as_str)
            != Some(identity_name(options.identity))
            || request.get("cli_arguments") != Some(&json!(options.prefix_arguments))
        {
            return Err(
                "the request is for another client identity or other client arguments".to_owned(),
            );
        }
        let channel_id = request
            .get("channel_id")
            .and_then(Value::as_str)
            .filter(|channel| self.channels.iter().any(|allowed| allowed == channel))
            .ok_or("the channel is not in this worker's --channel list")?;
        let thread_ts = request
            .get("thread_ts")
            .and_then(Value::as_str)
            .filter(|ts| ids::valid_ts(ts))
            .ok_or("the request names no valid thread ts")?;
        Ok(SpareKey {
            channel_id: channel_id.to_owned(),
            thread_ts: thread_ts.to_owned(),
        })
    }

    fn handle(
        &self,
        request: &Value,
        stream: &UnixStream,
        reader: &mut BufReader<UnixStream>,
    ) -> Value {
        let key = match self.key(request) {
            Ok(key) => key,
            Err(detail) => return no_spare(&detail),
        };
        match request.get("op").and_then(Value::as_str) {
            Some("prepare") => self.prepare(key),
            Some("post") => match request.get("text").and_then(Value::as_str) {
                Some(text) => self.post(&key, text, stream, reader),
                None => no_spare("a post names no text"),
            },
            _ => no_spare("unknown op"),
        }
    }

    fn prepare(&self, key: SpareKey) -> Value {
        let answer = |spare: &str| json!({"version": PROTOCOL_VERSION, "ok": true, "spare": spare});
        let evicted = {
            let mut state = self.pool.lock();
            let now = Instant::now();
            if state.starting.contains(&key)
                || state
                    .spares
                    .iter()
                    .any(|spare| spare.key == key && spare.expires > now)
            {
                return answer("present");
            }
            if state
                .used
                .iter()
                .any(|(used, until)| *used == key && *until > now)
                || state.starting.len() >= self.settings.max_spares
            {
                return answer("declined");
            }
            state.starting.push(key.clone());
            if state.spares.len() + state.starting.len() > self.settings.max_spares {
                let oldest = state
                    .spares
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, spare)| spare.expires)
                    .map(|(index, _)| index);
                oldest.map(|index| state.spares.remove(index))
            } else {
                None
            }
        };
        if let Some(spare) = evicted {
            self.diagnostics.log(&format!(
                "warm spare {} {} evicted for a newer one",
                spare.key.channel_id, spare.key.thread_ts
            ));
            spare.discard();
        }
        let (done, started) = mpsc::channel();
        let sent = self
            .spawner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .send(SpawnJob {
                key: key.clone(),
                done,
            });
        let started = match sent {
            Ok(()) => started
                .recv()
                .unwrap_or_else(|_| Err(io::Error::other("the spare thread stopped"))),
            Err(_) => Err(io::Error::other("the spare thread stopped")),
        };
        let mut state = self.pool.lock();
        state.starting.retain(|starting| *starting != key);
        match started {
            Ok(spare) => {
                state.spares.push(spare);
                self.pool.changed.notify_all();
                drop(state);
                self.diagnostics.log(&format!(
                    "warm spare {} {} started",
                    key.channel_id, key.thread_ts
                ));
                answer("started")
            }
            Err(error) => {
                drop(state);
                self.diagnostics.log(&format!(
                    "warm spare {} {} could not start: {error}",
                    key.channel_id, key.thread_ts
                ));
                json!({"version": PROTOCOL_VERSION, "ok": false, "spare": "failed"})
            }
        }
    }

    /// Start one client waiting for a body on its standard input. Runs on the spare thread.
    #[allow(unsafe_code)]
    fn start_spare(&self, key: SpareKey) -> io::Result<Spare> {
        let arguments = cli::post_arguments(
            self.transport.options().identity.app_argument(&self.app_id),
            &key.channel_id,
            Some(&key.thread_ts),
            self.format,
        );
        let mut command = self.transport.client_command(&arguments);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        // SAFETY: `prctl` is async-signal-safe and touches no memory of this process; it only
        // asks the kernel to kill the child when the thread that started it exits.
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("stdin is piped");
        // Room for any body in one write: a pipe that holds the body whole takes it in one
        // `write`, which a death of this process cannot cut short.
        set_pipe_capacity(&stdin, 64 * 1024);
        let stdout = cli::collect(
            child.stdout.take().expect("stdout is piped"),
            cli::MAX_STDOUT_BYTES,
        );
        let stderr = cli::collect(
            child.stderr.take().expect("stderr is piped"),
            cli::MAX_STDERR_BYTES,
        );
        Ok(Spare {
            key,
            child,
            stdin,
            stdout,
            stderr,
            expires: Instant::now() + self.settings.ttl,
        })
    }

    /// Post `text` through the spare for `key`: answer `ready`, wait for the helper's `commit`,
    /// and only then write the body. Returns the final answer.
    fn post(
        &self,
        key: &SpareKey,
        text: &str,
        stream: &UnixStream,
        reader: &mut BufReader<UnixStream>,
    ) -> Value {
        let taken = {
            let mut state = self.pool.lock();
            let now = Instant::now();
            let index = state
                .spares
                .iter()
                .position(|spare| spare.key == *key && spare.expires > now);
            let taken = index.map(|index| state.spares.remove(index));
            if taken.is_some() {
                state.used.push_back((key.clone(), now + self.settings.ttl));
                self.pool.changed.notify_all();
            }
            taken
        };
        let Some(mut spare) = taken else {
            return no_spare("no spare for this thread");
        };
        match spare.child.try_wait() {
            Ok(None) => {}
            Ok(Some(status)) => {
                self.diagnostics.log(&format!(
                    "warm spare {} {} had exited with {status}",
                    key.channel_id, key.thread_ts
                ));
                spare.release_reaped();
                return no_spare("the spare had exited");
            }
            Err(error) => {
                spare.discard();
                return no_spare(&format!("the spare could not be checked: {error}"));
            }
        }
        if text.is_empty() || pipe_capacity(&spare.stdin).is_none_or(|room| text.len() > room) {
            spare.discard();
            return no_spare("the body does not fit the spare's input pipe");
        }
        // Nothing is written to the spare until the helper commits: a helper that has given up
        // waiting for `ready`, and posted with its own client run, never sends `commit`.
        let committed = respond(
            stream,
            &json!({"version": PROTOCOL_VERSION, "result": "ready"}),
        )
        .is_ok()
            && stream.set_read_timeout(Some(READY_TIMEOUT)).is_ok()
            && read_line(reader, 1_024)
                .and_then(|line| serde_json::from_slice::<Value>(&line).ok())
                .is_some_and(|commit| {
                    commit.get("op").and_then(Value::as_str) == Some("commit")
                        && commit.get("version").and_then(Value::as_u64) == Some(PROTOCOL_VERSION)
                });
        if !committed {
            self.diagnostics.log(&format!(
                "warm post {} {}: no commit; the spare is discarded unused",
                key.channel_id, key.thread_ts
            ));
            spare.discard();
            return no_spare("no commit arrived");
        }
        let started = Instant::now();
        let Spare {
            mut child,
            mut stdin,
            stdout,
            stderr,
            ..
        } = spare;
        if let Err(error) = stdin.write_all(text.as_bytes()) {
            // The client closed its input before reading it all, so it never saw the end of the
            // body, which it waits for before posting.
            kill_group(&child);
            let _ = child.kill();
            let _ = child.wait();
            drop(stdin);
            let _ = stdout.join();
            let _ = stderr.join();
            if error.kind() == io::ErrorKind::BrokenPipe {
                return no_spare("the spare closed its input");
            }
            return failed(&TransportError::unknown(
                "warm_spare_write_failed",
                format!("writing the body to the spare failed: {error}"),
            ));
        }
        drop(stdin);
        let status = match cli::wait_until(&mut child, started + self.timeout) {
            Ok(Some(status)) => status,
            Ok(None) => {
                kill_group(&child);
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout.join();
                let _ = stderr.join();
                return failed(&TransportError::unknown(
                    "transport_timeout",
                    "the Slack client did not finish before its deadline",
                ));
            }
            Err(error) => {
                kill_group(&child);
                let _ = child.kill();
                let _ = child.wait();
                return failed(&TransportError::unknown(
                    "transport_wait_failed",
                    format!("waiting failed: {error}"),
                ));
            }
        };
        let completed = Completed {
            status,
            stdout: stdout.join().unwrap_or_default(),
            stderr: stderr.join().unwrap_or_default(),
        };
        let elapsed = started.elapsed().as_millis();
        match cli::post_outcome(&completed) {
            Ok(ts) => {
                self.diagnostics.log(&format!(
                    "warm post {} {}: posted {ts} in {elapsed} ms",
                    key.channel_id, key.thread_ts
                ));
                json!({"version": PROTOCOL_VERSION, "result": "posted", "ts": ts})
            }
            Err(error) => {
                self.diagnostics.log(&format!(
                    "warm post {} {}: failed in {elapsed} ms: {error}",
                    key.channel_id, key.thread_ts
                ));
                failed(&error)
            }
        }
    }

    /// Discard each spare whose time to live has passed, and forget used threads likewise,
    /// sleeping on the pool's condition variable until the next expiry.
    fn expire_forever(&self) {
        let mut state = self.pool.lock();
        loop {
            let now = Instant::now();
            state.used.retain(|(_, until)| *until > now);
            let mut expired = Vec::new();
            let mut index = 0;
            while index < state.spares.len() {
                if state.spares[index].expires <= now {
                    expired.push(state.spares.remove(index));
                } else {
                    index += 1;
                }
            }
            if !expired.is_empty() {
                drop(state);
                for spare in expired {
                    self.diagnostics.log(&format!(
                        "warm spare {} {} expired unused",
                        spare.key.channel_id, spare.key.thread_ts
                    ));
                    spare.discard();
                }
                state = self.pool.lock();
                continue;
            }
            let next = state
                .spares
                .iter()
                .map(|spare| spare.expires)
                .chain(state.used.iter().map(|(_, until)| *until))
                .min();
            state = match next {
                Some(next) => {
                    self.pool
                        .changed
                        .wait_timeout(state, next.saturating_duration_since(now))
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0
                }
                None => self
                    .pool
                    .changed
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            };
        }
    }
}

#[allow(unsafe_code)]
fn kill_group(child: &Child) {
    let Ok(process_id) = libc::pid_t::try_from(child.id()) else {
        return;
    };
    // SAFETY: `kill` has no memory-safety preconditions. The child was started as the leader of
    // its own process group and has not been reaped, so the group ID is still its own.
    unsafe {
        libc::kill(-process_id, libc::SIGKILL);
    }
}

#[allow(unsafe_code)]
fn set_pipe_capacity(pipe: &ChildStdin, bytes: i32) {
    // SAFETY: `fcntl` with F_SETPIPE_SZ takes a descriptor this process owns and an integer; it
    // reads no memory. A refusal leaves the pipe as it was, which `pipe_capacity` then reports.
    unsafe {
        libc::fcntl(pipe.as_raw_fd(), libc::F_SETPIPE_SZ, bytes);
    }
}

#[allow(unsafe_code)]
fn pipe_capacity(pipe: &ChildStdin) -> Option<usize> {
    // SAFETY: as for `set_pipe_capacity`; F_GETPIPE_SZ returns the capacity or -1.
    let capacity = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETPIPE_SZ) };
    usize::try_from(capacity).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_post_responses() {
        assert_eq!(
            parse_post_response(
                &json!({"version": PROTOCOL_VERSION, "result": "posted", "ts": "1700000000.000100"})
            ),
            WarmPost::Posted("1700000000.000100".to_owned())
        );
        assert!(matches!(
            parse_post_response(
                &json!({"version": PROTOCOL_VERSION, "result": "no_spare", "detail": "x"})
            ),
            WarmPost::NoSpare(_)
        ));
        let WarmPost::Failed(error) = parse_post_response(&json!({
            "version": PROTOCOL_VERSION, "result": "failed", "code": "ratelimited", "detail": "slow down",
            "outcome": "not_applied", "retryable": true
        })) else {
            panic!("a failure");
        };
        assert_eq!(error.outcome, Outcome::NotApplied);
        // Anything the helper cannot read is an unknown outcome, never a reason to post again.
        for response in [
            json!({"version": PROTOCOL_VERSION, "result": "posted", "ts": "nope"}),
            json!({"version": PROTOCOL_VERSION + 1, "result": "no_spare"}),
            json!({"version": PROTOCOL_VERSION, "result": "failed", "code": "x", "outcome": "maybe"}),
            json!({"version": 1}),
        ] {
            let WarmPost::Failed(error) = parse_post_response(&response) else {
                panic!("{response} must fail");
            };
            assert_eq!(error.outcome, Outcome::Unknown, "{response}");
        }
    }

    static STAND_INS: AtomicUsize = AtomicUsize::new(0);

    /// Remove a stand-in's socket directory.
    fn remove_stand_in(socket: &Path) {
        if let Some(directory) = socket.parent() {
            let _ = fs::remove_dir_all(directory);
        }
    }

    /// What a stand-in worker does with one connection, after reading its request line.
    #[derive(Clone, Debug)]
    enum Script {
        /// Answer this line at once.
        Answer(Value),
        /// Answer `ready`, read the helper's `commit` line, then answer this, or close without an
        /// answer when `None`.
        Commit(Option<Value>),
        /// Answer nothing for this long, then close, as a stopped or hung worker does.
        Silent(Duration),
    }

    /// A stand-in worker on a fresh socket: it plays the next of `scripts` on each connection,
    /// and records each line it read (requests and commits).
    fn stand_in(
        name: &str,
        scripts: Vec<Script>,
    ) -> (PathBuf, Arc<Mutex<Vec<Value>>>, thread::JoinHandle<()>) {
        let directory = std::env::temp_dir().join(format!(
            "slack-warm-{name}-{}-{}",
            std::process::id(),
            STAND_INS.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("socket directory");
        let socket = directory.join("w.sock");
        let listener = UnixListener::bind(&socket).expect("bind");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        let server = thread::spawn(move || {
            for script in scripts {
                let (stream, _) = listener.accept().expect("accept");
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                let record = |reader: &mut BufReader<UnixStream>| {
                    let line = read_line(reader, MAX_REQUEST_LINE_BYTES).expect("a whole line");
                    recorded
                        .lock()
                        .expect("seen")
                        .push(serde_json::from_slice::<Value>(&line).expect("JSON line"));
                };
                record(&mut reader);
                match script {
                    Script::Answer(answer) => respond(&stream, &answer).expect("answer"),
                    Script::Commit(answer) => {
                        respond(
                            &stream,
                            &json!({"version": PROTOCOL_VERSION, "result": "ready"}),
                        )
                        .expect("ready");
                        record(&mut reader);
                        if let Some(answer) = answer {
                            respond(&stream, &answer).expect("answer");
                        }
                    }
                    Script::Silent(duration) => thread::sleep(duration),
                }
            }
        });
        (socket, seen, server)
    }

    fn warm_transport(socket: PathBuf, fake: &crate::fake::FakeTransport) -> WarmTransport {
        WarmTransport::new(
            Box::new(fake.clone()),
            WarmClient::new(
                socket,
                PathBuf::from("/usr/bin/slack"),
                AppId::new("A0TESTAPP01").expect("app"),
                TextFormat::Markdown,
                Duration::from_secs(5),
                Diagnostics::new(None, "test"),
            ),
        )
    }

    fn reply(app: &AppId) -> PostMessage<'_> {
        PostMessage {
            app_id: app,
            channel_id: "C0C91RW5FAR",
            thread_ts: Some("1700000000.000100"),
            text: "hello",
            format: TextFormat::Markdown,
        }
    }

    #[test]
    fn a_post_the_worker_makes_is_not_made_again() {
        let (socket, seen, server) = stand_in(
            "posted",
            vec![Script::Commit(Some(
                json!({"version": PROTOCOL_VERSION, "result": "posted", "ts": "1700000001.000200"}),
            ))],
        );
        let fake = crate::fake::FakeTransport::new();
        let app = AppId::new("A0TESTAPP01").expect("app");
        let ts = warm_transport(socket.clone(), &fake)
            .post_message(&reply(&app))
            .expect("posted");
        server.join().expect("stand-in");
        remove_stand_in(&socket);
        assert_eq!(ts, "1700000001.000200");
        assert!(fake.state().posts.is_empty(), "no second post");
        let seen = seen.lock().expect("seen");
        let request = &seen[0];
        assert_eq!(request["op"], "post");
        assert_eq!(request["thread_ts"], "1700000000.000100");
        assert_eq!(request["text"], "hello");
        assert_eq!(request["app_id"], "A0TESTAPP01");
        assert_eq!(seen[1]["op"], "commit");
    }

    #[test]
    fn no_spare_or_a_failure_proven_not_applied_posts_with_the_inner_transport() {
        // A worker that has no spare, one that refuses as not applied, and one that never
        // answers the request at all (stopped or hung) cannot have posted, so the helper posts
        // with its own client run. The silent worker costs the helper `READY_TIMEOUT`.
        for script in [
            Script::Answer(
                json!({"version": PROTOCOL_VERSION, "result": "no_spare", "detail": "none"}),
            ),
            Script::Commit(Some(
                json!({"version": PROTOCOL_VERSION, "result": "failed", "code": "invalid_auth",
                "detail": "x", "outcome": "not_applied", "retryable": true}),
            )),
            Script::Silent(READY_TIMEOUT + Duration::from_millis(500)),
        ] {
            let (socket, _, server) = stand_in("fallback", vec![script.clone()]);
            let fake = crate::fake::FakeTransport::new();
            let app = AppId::new("A0TESTAPP01").expect("app");
            let started = Instant::now();
            let ts = warm_transport(socket.clone(), &fake)
                .post_message(&reply(&app))
                .expect("posted by the inner transport");
            let waited = started.elapsed();
            server.join().expect("stand-in");
            remove_stand_in(&socket);
            let posts = fake.state().posts.clone();
            assert_eq!(posts.len(), 1, "{script:?}");
            assert_eq!(posts[0].ts, ts);
            if matches!(script, Script::Silent(_)) {
                assert!(waited >= READY_TIMEOUT, "{waited:?}");
            }
        }
    }

    #[test]
    fn a_refusal_that_cannot_succeed_again_is_not_retried() {
        for answer in [
            json!({"version": PROTOCOL_VERSION, "result": "failed", "code": "msg_too_long", "detail": "x",
                "outcome": "not_applied", "retryable": false}),
            json!({"version": PROTOCOL_VERSION, "result": "failed", "code": "ratelimited", "detail": "x",
                "outcome": "not_applied", "retryable": true}),
        ] {
            let (socket, _, server) =
                stand_in("refused", vec![Script::Commit(Some(answer.clone()))]);
            let fake = crate::fake::FakeTransport::new();
            let app = AppId::new("A0TESTAPP01").expect("app");
            let error = warm_transport(socket.clone(), &fake)
                .post_message(&reply(&app))
                .expect_err("refused");
            server.join().expect("stand-in");
            remove_stand_in(&socket);
            assert_eq!(error.outcome, Outcome::NotApplied, "{answer}");
            assert!(fake.state().posts.is_empty(), "{answer}");
        }
    }

    #[test]
    fn an_unknown_worker_outcome_is_never_posted_again() {
        // After `commit`, an answer of unknown outcome, and a worker that closes without
        // answering, both leave the post possibly made, so the inner transport is not used.
        for answer in [
            Some(
                json!({"version": PROTOCOL_VERSION, "result": "failed", "code": "transport_timeout",
                "detail": "x", "outcome": "unknown", "retryable": true}),
            ),
            None,
        ] {
            let (socket, _, server) = stand_in("unknown", vec![Script::Commit(answer.clone())]);
            let fake = crate::fake::FakeTransport::new();
            let app = AppId::new("A0TESTAPP01").expect("app");
            let error = warm_transport(socket.clone(), &fake)
                .post_message(&reply(&app))
                .expect_err("unknown");
            server.join().expect("stand-in");
            remove_stand_in(&socket);
            assert_eq!(error.outcome, Outcome::Unknown, "{answer:?}");
            assert!(fake.state().posts.is_empty(), "{answer:?}");
        }
    }

    #[test]
    fn a_reaction_asks_for_a_spare_in_its_message_thread_first() {
        let (socket, seen, server) = stand_in(
            "prepare",
            vec![Script::Answer(
                json!({"version": PROTOCOL_VERSION, "ok": true, "spare": "started"}),
            )],
        );
        let fake = crate::fake::FakeTransport::new();
        let app = AppId::new("A0TESTAPP01").expect("app");
        warm_transport(socket.clone(), &fake)
            .add_reaction(&app, "C0C91RW5FAR", "1700000000.000100", "eyes")
            .expect("reaction");
        server.join().expect("stand-in");
        remove_stand_in(&socket);
        assert_eq!(fake.state().reactions.len(), 1);
        let request = &seen.lock().expect("seen")[0];
        assert_eq!(request["op"], "prepare");
        assert_eq!(request["channel_id"], "C0C91RW5FAR");
        assert_eq!(request["thread_ts"], "1700000000.000100");
    }

    #[test]
    fn a_missing_worker_is_no_spare() {
        let client = WarmClient::new(
            PathBuf::from("/nonexistent/agentctl-slack-warm.sock"),
            PathBuf::from("/usr/bin/slack"),
            AppId::new("A0TESTAPP01").expect("app"),
            TextFormat::Markdown,
            Duration::from_secs(5),
            Diagnostics::new(None, "test"),
        );
        let app = AppId::new("A0TESTAPP01").expect("app");
        let post = PostMessage {
            app_id: &app,
            channel_id: "C0C91RW5FAR",
            thread_ts: Some("1700000000.000100"),
            text: "hello",
            format: TextFormat::Markdown,
        };
        assert!(matches!(client.post(&post), WarmPost::NoSpare(_)));
        let other = AppId::new("A0OTHERAPP1").expect("app");
        assert!(matches!(
            client.post(&PostMessage {
                app_id: &other,
                ..post
            }),
            WarmPost::NoSpare(_)
        ));
    }
}
