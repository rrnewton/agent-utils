//! Launchable orchestration for the durable chat runtime.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::io;
#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chat_subscription::{
    BackendCapabilities, BackendFailure, ChatSubscription, ChatSubscriptionBackend,
    ChatSubscriptionCancellation, ChatSubscriptionDriver, DeliveryId, SubscribeRequest,
    SubscriptionError, SubscriptionItem,
};
use chat_subscription_plugin::process::ProcessPhaseTimeouts;
use serde::Serialize;
use serde_json::{json, Value};
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use signal_hook::iterator::{Handle as SignalHandle, Signals};

use crate::agent::{AgentError, AgentRuntime, DrainOptions, QueueMessageState};
use crate::chat_events::{self, PaneEvent, PaneEventStream, PaneEventWake};
use crate::chat_runtime::{
    self, AckResult, BridgeConfiguration, BridgeState, ChatRuntimeError, CommandOutboundTransport,
    CoordinatorDeliveryResult, OutboundCancellation, OutboundFailure, ReplyRoute, ReplyRouteEntry,
    RootMessageSubmission, RootMessageTransport, SNAPSHOT_LINES,
};
use crate::client::HerdrClient;
use crate::subagents::{ManagedAgents, ManagedApi};

// The lines of context each output subscription asks for: the rows the service's own reads of
// the pane ask for.
const SNAPSHOT_LINES_U32: u32 = 4_000;
const _: () = assert!(SNAPSHOT_LINES_U32 as usize == SNAPSHOT_LINES);
const MAX_SNAPSHOT_BYTES: usize = 2 * 1_024 * 1_024;
// Notes one pass report keeps, and distinct notes one service process remembers having logged.
const MAX_REPORT_NOTES: usize = 128;
const MAX_LOGGED_NOTES: usize = 4_096;
const MAX_KEYS_PER_PASS: usize = 4;
const MAX_SENDS_PER_PASS: usize = 64;
const PROVIDER_NOTICE_CAPACITY: usize = 64;
// One more than the notice channel holds, so a pass always reaches a closed channel.
const MAX_PROVIDER_NOTICES_PER_PASS: usize = PROVIDER_NOTICE_CAPACITY + 1;
const MAX_DIRECT_REQUEST_KEYS: usize = 2_048;
const ACK_QUEUE_CAPACITY: usize = 64;
const ACK_RETRY_DELAY: Duration = Duration::from_secs(60);
const MAX_IMMEDIATE_BACKLOG_CHUNKS: usize = 64;
const EVENT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const OUTPUT_RETRY_MAX: Duration = Duration::from_secs(60);
// How often the service reads the pane itself while a reply alias keeps an output pattern
// matched, as `RouteCache::saturated_by` describes, and while the agent works by Herdr's report
// or by its screen, as `TurnEvidence` describes.
const SATURATED_POLL_INTERVAL: Duration = Duration::from_secs(2);
const PROVIDER_RETRY_MIN: Duration = Duration::from_secs(1);
const PROVIDER_RETRY_MAX: Duration = Duration::from_secs(60);
// Already reported reply IDs one recovery-scan log line names before it counts the rest.
const ALREADY_REPORTED_LOG_IDS: usize = 8;
// Two seconds are reserved by the process supervisor for forced pidfd reap; the remaining five
// seconds cover host-worker reconciliation and joins. The selected Close + grace is added after
// Hello, while an as-yet-unconnected generation also owns its complete Hello deadline.
const PROCESS_AND_JOIN_MARGIN_SECONDS: u64 = 7;
const SYSTEMD_GRACEFUL_STOP_DIAGNOSTIC_DEADLINE: Duration = Duration::from_secs(70);

/// A service or command failure with its original typed source where available.
#[derive(Debug)]
pub enum ChatServiceError {
    /// Durable bridge state or wire invariants failed.
    Runtime(ChatRuntimeError),
    /// Named-agent resolution or delivery failed.
    Agent(AgentError),
    /// Local service I/O failed.
    Io(io::Error),
    /// A bounded provider or event generation failed.
    Generation(String),
    /// A supervised outbound helper returned bounded provider outcome evidence.
    Outbound(OutboundFailure),
    /// A service worker panicked.
    Worker(String),
}

impl fmt::Display for ChatServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(error) => fmt::Display::fmt(error, formatter),
            Self::Agent(error) => fmt::Display::fmt(error, formatter),
            Self::Io(error) => write!(formatter, "chat service I/O failed: {error}"),
            Self::Generation(detail) | Self::Worker(detail) => formatter.write_str(detail),
            Self::Outbound(error) => write!(formatter, "chat outbound operation failed: {error}"),
        }
    }
}

impl std::error::Error for ChatServiceError {}

impl From<ChatRuntimeError> for ChatServiceError {
    fn from(error: ChatRuntimeError) -> Self {
        Self::Runtime(error)
    }
}

impl From<AgentError> for ChatServiceError {
    fn from(error: AgentError) -> Self {
        Self::Agent(error)
    }
}

impl From<io::Error> for ChatServiceError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<OutboundFailure> for ChatServiceError {
    fn from(error: OutboundFailure) -> Self {
        Self::Outbound(error)
    }
}

/// Bounded delivery settings for one foreground service pass.
#[derive(Clone, Copy, Debug)]
pub struct ServiceOptions {
    /// Coordinator prompt-delivery settings.
    pub delivery: DrainOptions,
    /// Period between disk-backed recovery snapshots.
    pub reconciliation_interval: Duration,
}

/// Machine-readable work completed during one bounded local pass.
#[derive(Clone, Debug, Default, Serialize)]
pub struct CycleReport {
    /// Request keys whose prompts reached or had already reached the coordinator.
    pub delivered: Vec<String>,
    /// Request keys whose delivery remains safely pending.
    pub delivery_pending: Vec<String>,
    /// Request keys whose terminal-delivery outcome is uncertain.
    pub delivery_uncertain: Vec<String>,
    /// Request keys whose configured acknowledgement is reconciled.
    pub acknowledged: Vec<String>,
    /// Newly captured reply ordinals by request key.
    pub captured: Vec<(String, Vec<u32>)>,
    /// Provider message receipts for replies sent during this pass.
    pub sent: Vec<String>,
    /// Whether an observed Herdr snapshot reported truncation.
    pub snapshot_truncated: bool,
    /// Herdr snapshot revision, which is diagnostic and never a replay cursor.
    pub snapshot_revision: Option<u64>,
    /// Bounded per-operation failures retained for explicit retry.
    pub errors: Vec<String>,
    /// Bounded notes about coordinator output that was read but not sent, such as a refused
    /// reply block. They are diagnostics: nothing is retried because of them.
    pub notes: Vec<String>,
    /// More bounded work remains for a later event or recovery pass.
    pub more_work: bool,
    #[serde(skip)]
    deferred_keys: Vec<String>,
    #[serde(skip)]
    recovery_requested: bool,
    #[serde(skip)]
    processed_keys: Vec<String>,
    // The pass asked for a request prompt or a notice to be typed into the pane, so one may have
    // reached it: see `TypingWatch`.
    #[serde(skip)]
    prompt_typed: bool,
}

impl CycleReport {
    /// Whether any operation failed while preserving its durable retry identity.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    fn merge(&mut self, mut other: Self) {
        self.delivered.append(&mut other.delivered);
        self.delivery_pending.append(&mut other.delivery_pending);
        self.delivery_uncertain
            .append(&mut other.delivery_uncertain);
        self.acknowledged.append(&mut other.acknowledged);
        self.captured.append(&mut other.captured);
        self.sent.append(&mut other.sent);
        self.snapshot_truncated |= other.snapshot_truncated;
        self.snapshot_revision = other.snapshot_revision.or(self.snapshot_revision);
        self.errors.append(&mut other.errors);
        self.notes.append(&mut other.notes);
        self.notes.truncate(MAX_REPORT_NOTES);
        self.more_work |= other.more_work;
        self.deferred_keys.append(&mut other.deferred_keys);
        self.recovery_requested |= other.recovery_requested;
        self.processed_keys.append(&mut other.processed_keys);
        self.prompt_typed |= other.prompt_typed;
    }

    fn error(&mut self, operation: &str, key: &str, error: impl fmt::Display) {
        if self.errors.len() < 128 {
            self.errors.push(format!("{operation} {key}: {error}"));
        }
        self.more_work = true;
    }

    fn note(&mut self, note: impl fmt::Display) {
        if self.notes.len() < MAX_REPORT_NOTES {
            self.notes.push(note.to_string());
        }
    }
}

/// Validate all launch authority before creating a bridge state directory.
pub fn initialize<A: ManagedApi + ?Sized>(
    state_root: &Path,
    configuration_path: &Path,
    manager: &ManagedAgents<'_, A>,
) -> Result<Value, ChatServiceError> {
    let configuration = BridgeConfiguration::load_private(configuration_path)?;
    let inventory = crate::plugins::discover();
    let _pinned = chat_runtime::select_plugin(&inventory, &configuration)?;
    let _target = manager.pane_info(&configuration.agent_name)?;
    validate_service_outbound(&configuration)?;
    let _outbound = configuration.outbound_transport()?;
    let state = BridgeState::initialize(state_root, configuration)?;
    Ok(state.status()?)
}

/// Inspect durable state without provider access, Herdr access, or recovery writes.
pub fn status(state_root: &Path) -> Result<Value, ChatServiceError> {
    Ok(BridgeState::inspect(state_root)?.status()?)
}

/// Approve an operator-reviewed checkpoint retry without provider or coordinator access.
pub fn retry_checkpoint_gap(
    state_root: &Path,
    request: &chat_runtime::CheckpointGapRetryApproval<'_>,
) -> Result<Value, ChatServiceError> {
    Ok(BridgeState::inspect(state_root)?.approve_checkpoint_gap_retry(request)?)
}

/// Approve an exact committed-boundary retry without provider or coordinator access.
pub fn retry_boundary_gap(
    state_root: &Path,
    request: &chat_runtime::GapRetryApproval<'_>,
) -> Result<Value, ChatServiceError> {
    Ok(BridgeState::inspect(state_root)?.approve_boundary_gap_retry(request)?)
}

/// Inspect one exact active retained request without writes or external service access.
pub fn inspect_request(state_root: &Path, key: &str) -> Result<Value, ChatServiceError> {
    Ok(BridgeState::inspect(state_root)?.inspect_request(key)?)
}

/// Render one thread's retained messages without writes or external service access.
pub fn thread_history(
    state_root: &Path,
    thread: &str,
    last: u32,
) -> Result<String, ChatServiceError> {
    Ok(BridgeState::inspect(state_root)?.thread_history(thread, last)?)
}

/// Publish one explicit owner/operator root message without mutating durable bridge state.
///
/// The configured channel authority and request are validated before the exact configured helper
/// is pinned or spawned. The caller owns the UUID across any retry whose provider outcome is
/// uncertain; this command deliberately does not create an event-loop reply record.
pub fn publish(
    state_root: &Path,
    channel_id: &str,
    request_id: &str,
    body: &str,
) -> Result<Value, ChatServiceError> {
    let state = BridgeState::inspect(state_root)?;
    let configuration = state.config();
    if !configuration.outbound_enabled {
        return Err(ChatServiceError::Generation(
            "chat publish requires outbound_enabled=true".to_owned(),
        ));
    }
    if configuration.outbound_command.is_none() {
        return Err(ChatServiceError::Generation(
            "chat publish requires an explicit outbound_command helper".to_owned(),
        ));
    }
    if !configuration
        .channel_ids
        .iter()
        .any(|configured| configured == channel_id)
    {
        return Err(ChatServiceError::Generation(
            "chat publish channel is outside the configured channel_ids authority".to_owned(),
        ));
    }
    let submission = RootMessageSubmission {
        channel_id,
        body,
        request_id,
    };
    chat_runtime::validate_root_message_submission(&submission)?;
    let mut transport = state.outbound_transport()?.ok_or_else(|| {
        ChatServiceError::Generation(
            "chat publish requires an explicit outbound_command helper".to_owned(),
        )
    })?;
    let message_id = transport.publish_root(submission)?;
    Ok(json!({
        "version": 1,
        "id": request_id,
        "action": "send",
        "ok": true,
        "receipt": {"message_id": message_id},
    }))
}

/// Close capture for one exact request and durably retire it when terminal and replay-safe.
pub fn close(state_root: &Path, key: &str) -> Result<Value, ChatServiceError> {
    let state = BridgeState::open(state_root)?;
    state.close_replies(key)?;
    Ok(json!({"closed": key}))
}

/// Signal one systemd-supplied main process through an inode-bound pidfd and wait for exact exit.
///
/// The 70-second internal deadline is diagnostic. On expiry this helper deliberately remains in
/// `poll` so systemd's single `TimeoutStopSec` window and `TimeoutStopFailureMode=kill` perform the
/// control-group fallback; returning would incorrectly start a second stop phase.
#[cfg(target_os = "linux")]
pub fn graceful_stop_main(main_pid: u32, main_pidfd_id: u64) -> Result<(), ChatServiceError> {
    let pid = libc::pid_t::try_from(main_pid).map_err(|_| {
        ChatServiceError::Generation("systemd MAINPID does not fit Linux pid_t".to_owned())
    })?;
    if pid <= 1 || main_pid == std::process::id() {
        return Err(ChatServiceError::Generation(
            "graceful stop refuses init, zero, or its own process identity".to_owned(),
        ));
    }
    // SAFETY: pidfd_open accepts scalar arguments and returns a fresh descriptor on success.
    let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0_u32) };
    if descriptor < 0 {
        return Err(ChatServiceError::Io(io::Error::last_os_error()));
    }
    let descriptor = i32::try_from(descriptor).map_err(|_| {
        ChatServiceError::Io(io::Error::other(
            "pidfd_open returned an invalid descriptor",
        ))
    })?;
    // SAFETY: successful pidfd_open returned a fresh descriptor now owned by this function.
    let pidfd = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: pidfd is live and metadata points to writable stat storage.
    if unsafe { libc::fstat(pidfd.as_raw_fd(), metadata.as_mut_ptr()) } < 0 {
        return Err(ChatServiceError::Io(io::Error::last_os_error()));
    }
    // SAFETY: successful fstat initialized the supplied storage.
    let metadata = unsafe { metadata.assume_init() };
    if metadata.st_ino != main_pidfd_id {
        return Err(ChatServiceError::Generation(format!(
            "systemd MAINPIDFDID {main_pidfd_id} does not match pidfd inode {} for MAINPID {main_pid}",
            metadata.st_ino
        )));
    }
    // SAFETY: pidfd_send_signal addresses the stable process identity held by pidfd. Null siginfo
    // and zero flags are the documented ordinary signal form.
    let signal_result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.as_raw_fd(),
            libc::SIGTERM,
            std::ptr::null::<libc::siginfo_t>(),
            0_u32,
        )
    };
    if signal_result < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(ChatServiceError::Io(error));
        }
    }

    let diagnostic_deadline = Instant::now() + SYSTEMD_GRACEFUL_STOP_DIAGNOSTIC_DEADLINE;
    let mut deadline_reported = false;
    loop {
        let mut descriptor = libc::pollfd {
            fd: pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout_millis = if deadline_reported {
            1_000
        } else {
            i32::try_from(
                diagnostic_deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis()
                    .clamp(1, 1_000),
            )
            .expect("bounded poll timeout fits i32")
        };
        // SAFETY: descriptor is one initialized pollfd retained for the duration of the call.
        let result = unsafe { libc::poll(&raw mut descriptor, 1, timeout_millis) };
        if result > 0 && descriptor.revents != 0 {
            return Ok(());
        }
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(ChatServiceError::Io(error));
            }
        }
        if !deadline_reported && Instant::now() >= diagnostic_deadline {
            deadline_reported = true;
            service_log(format_args!(
                "agentctl: graceful MAINPID {main_pid} did not exit within {}s; waiting for systemd's same-window control-group kill",
                SYSTEMD_GRACEFUL_STOP_DIAGNOSTIC_DEADLINE.as_secs()
            ));
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub fn graceful_stop_main(_main_pid: u32, _main_pidfd_id: u64) -> Result<(), ChatServiceError> {
    Err(ChatServiceError::Generation(
        "pidfd-bound graceful service stop requires Linux".to_owned(),
    ))
}

/// Run one bounded recovery/capture/outbound pass and release the runner lease.
pub fn tick<A: ManagedApi + ?Sized>(
    state_root: &Path,
    manager: &ManagedAgents<'_, A>,
    options: ServiceOptions,
) -> Result<CycleReport, ChatServiceError> {
    let state = BridgeState::open(state_root)?;
    let _resume_request = state.subscribe_request()?;
    validate_service_outbound(state.config())?;
    let _lease = state.acquire_runner_lease()?;
    let info = manager.pane_info(&state.config().agent_name)?;
    let snapshot = manager.read_capture(&state.config().agent_name, SNAPSHOT_LINES)?;
    let mut routes = RouteCache::from_entries(state.reply_route_entries()?);
    let mut transport = state.outbound_transport()?;
    let mut control = PassControl {
        transport: &mut transport,
        stop: None,
    };
    // The read is captured before any request is prompted, as at service startup: a prompt
    // written in this pass is newer than the read, so no block in the read answers it, and a
    // reply alias assigned in this pass must not take a block in the read as its reply.
    let mut report = capture_recovery_snapshot(
        &state,
        manager,
        options.delivery,
        &mut routes,
        SnapshotInput {
            text: &snapshot,
            truncated: false,
            revision: None,
        },
        &mut control,
    )?;
    report.merge(recover_pass(
        &state,
        manager,
        options.delivery,
        &mut routes,
        &mut control,
    )?);
    if let Some(problem) = state.reply_alias_problem() {
        report.note(problem);
    }
    if manager.pane_info(&state.config().agent_name)?.pane_id != info.pane_id {
        return Err(ChatServiceError::Generation(
            "coordinator pane moved during the bounded chat tick".to_owned(),
        ));
    }
    Ok(report)
}

/// Own a provider subscription and Herdr output subscription until SIGINT or SIGTERM.
pub fn run<A: ManagedApi + ?Sized>(
    state_root: &Path,
    client: &HerdrClient,
    manager: &ManagedAgents<'_, A>,
    options: ServiceOptions,
) -> Result<Value, ChatServiceError> {
    run_with_ignored_text_prefixes(state_root, client, manager, options, Vec::new())
}

/// Run with process-local admission exclusions; the saved configuration remains unchanged.
pub fn run_with_ignored_text_prefixes<A: ManagedApi + ?Sized>(
    state_root: &Path,
    client: &HerdrClient,
    manager: &ManagedAgents<'_, A>,
    options: ServiceOptions,
    ignored_text_prefixes: Vec<String>,
) -> Result<Value, ChatServiceError> {
    let state = BridgeState::open(state_root)?.with_ignored_text_prefixes(ignored_text_prefixes)?;
    let _resume_request = state.subscribe_request()?;
    validate_service_outbound(state.config())?;
    let outbound_cancellation = OutboundCancellation::new()?;
    let mut outbound = state.outbound_transport()?;
    if let Some(transport) = outbound.as_mut() {
        transport.set_cancellation(outbound_cancellation.clone());
    }
    let ack_cancellation = OutboundCancellation::new()?;
    let mut ack_transport = if state.config().ack_reaction.is_some() {
        outbound
            .as_ref()
            .map(CommandOutboundTransport::try_clone_generation)
            .transpose()?
    } else {
        None
    };
    if let Some(transport) = ack_transport.as_mut() {
        transport.set_cancellation(ack_cancellation.clone());
    }
    let ack_queue = ack_transport
        .as_ref()
        .map(|_| Arc::new(AckQueue::default()));
    let inventory = crate::plugins::discover();
    let pinned = chat_runtime::select_plugin(&inventory, state.config())?;
    let selected_process_timeouts = pinned.process_phase_timeouts();
    let initial_provider_join_timeout = pending_provider_join_timeout(selected_process_timeouts);
    let outbound_join_timeout = configured_outbound_join_timeout(state.config());
    // Preflight owns no generation: release its descriptors and captured environment values
    // before the provider worker independently pins and launches the real generation.
    drop(pinned);
    let _target = manager.pane_info(&state.config().agent_name)?;
    let _lease = state.acquire_runner_lease()?;

    if let Some(queue) = ack_queue.as_ref() {
        queue.enqueue(state.pending_ack_keys()?);
    }
    let stop = Arc::new(StopState {
        ack_queue: ack_queue.clone(),
        ..StopState::default()
    });
    let cancellation: SharedCancellation =
        Arc::new(Mutex::new(ProviderCancellationRegistry::default()));
    let output_wake: SharedWake = Arc::new(Mutex::new(None));
    let overflowed = Arc::new(AtomicBool::new(false));
    let (signal_handle, signal_thread) = spawn_signal_worker(
        Arc::clone(&stop),
        Arc::clone(&cancellation),
        Arc::clone(&output_wake),
        initial_provider_join_timeout,
        outbound_join_timeout,
    )?;
    let ack_worker = if let Some(transport) = ack_transport {
        match spawn_ack_worker(
            state.clone(),
            ack_queue.expect("ACK transport has a queue"),
            transport,
            Arc::clone(&stop),
            Arc::clone(&output_wake),
            |line| service_log(line),
        ) {
            Ok(worker) => Some(worker),
            Err(error) => {
                let deadline = begin_service_stop(
                    &stop,
                    &cancellation,
                    initial_provider_join_timeout,
                    outbound_join_timeout,
                );
                signal_handle.close();
                let _ = join_worker_until(signal_thread, "chat signal", deadline);
                return Err(ChatServiceError::Io(error));
            }
        }
    } else {
        None
    };
    let (notices, notice_receiver) = mpsc::sync_channel(PROVIDER_NOTICE_CAPACITY);
    let provider = match spawn_provider(
        state.clone(),
        Arc::clone(&stop),
        Arc::clone(&cancellation),
        Arc::clone(&output_wake),
        Arc::clone(&overflowed),
        notices,
        selected_process_timeouts,
    ) {
        Ok(provider) => provider,
        Err(error) => {
            let deadline = begin_service_stop(
                &stop,
                &cancellation,
                initial_provider_join_timeout,
                outbound_join_timeout,
            );
            outbound_cancellation.cancel();
            signal_handle.close();
            let _ = join_worker_until(signal_thread, "chat signal", deadline);
            if let Some(worker) = ack_worker {
                if let Err(cleanup) = join_ack_worker_until(worker, &ack_cancellation, deadline) {
                    return Err(ChatServiceError::Worker(format!("{error}; {cleanup}")));
                }
            }
            return Err(error);
        }
    };

    let result = run_owner_loop(
        &state,
        client,
        manager,
        options,
        &stop,
        &cancellation,
        &output_wake,
        &overflowed,
        &notice_receiver,
        &mut outbound,
    );

    begin_service_stop(
        &stop,
        &cancellation,
        initial_provider_join_timeout,
        outbound_join_timeout,
    );
    outbound_cancellation.cancel();
    wake_output(&output_wake);
    if let Err(error) = cancel_provider(&cancellation) {
        stop.record_cleanup_error(error.to_string());
    }
    signal_handle.close();
    let shutdown_deadline = stop
        .shutdown_deadline()
        .expect("run established one shutdown window");
    if let Err(error) = join_worker_until(signal_thread, "chat signal", shutdown_deadline) {
        stop.record_cleanup_error(error.to_string());
    }
    if let Err(error) = join_worker_until(provider, "chat provider", shutdown_deadline) {
        stop.record_cleanup_error(error.to_string());
    }
    if let Some(worker) = ack_worker {
        if let Err(error) = join_ack_worker_until(worker, &ack_cancellation, shutdown_deadline) {
            stop.record_cleanup_error(error.to_string());
        }
    }
    service_outcome(result, stop.take_cleanup_errors())
}

/// What `chat run` returns once shut down: the owner loop's result, with any cleanup errors the
/// shutdown recorded appended to it.
fn service_outcome(
    result: Result<(), ChatServiceError>,
    cleanup_errors: Vec<String>,
) -> Result<Value, ChatServiceError> {
    match (result, cleanup_errors.is_empty()) {
        (Ok(()), true) => Ok(json!({"stopped": true})),
        (Err(error), true) => Err(error),
        (primary, false) => {
            let cleanup = cleanup_errors.join("; ");
            Err(ChatServiceError::Worker(match primary {
                Ok(()) => format!("chat shutdown cleanup is uncertain: {cleanup}"),
                Err(error) => format!("{error}; chat shutdown cleanup is uncertain: {cleanup}"),
            }))
        }
    }
}

fn join_worker_until(
    worker: ServiceWorker,
    label: &str,
    deadline: Instant,
) -> Result<(), ChatServiceError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    match worker.done.recv_timeout(remaining) {
        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
        Err(mpsc::RecvTimeoutError::Timeout) => {
            return Err(ChatServiceError::Worker(format!(
                "{label} worker did not stop before the shared shutdown deadline; cleanup is uncertain"
            )));
        }
    }
    worker
        .handle
        .join()
        .map_err(|_| ChatServiceError::Worker(format!("{label} worker panicked")))
}

struct ServiceWorker {
    handle: thread::JoinHandle<()>,
    done: mpsc::Receiver<()>,
}

#[derive(Default)]
struct AckQueueState {
    pending: VecDeque<String>,
    retained: BTreeSet<String>,
    retry_at: BTreeMap<String, Instant>,
    rescan: bool,
    stopped: bool,
}

#[derive(Default)]
struct AckQueue {
    state: Mutex<AckQueueState>,
    changed: Condvar,
}

enum AckWork {
    Request(String),
    Reconcile,
}

impl AckQueue {
    fn enqueue(&self, keys: impl IntoIterator<Item = String>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.stopped {
            return;
        }
        let now = Instant::now();
        for key in keys {
            if state.retained.contains(&key)
                || state.retry_at.get(&key).is_some_and(|retry| *retry > now)
            {
                continue;
            }
            if state.retained.len() >= ACK_QUEUE_CAPACITY {
                // The request remains durable. The worker scans the bounded pending population
                // after draining this queue; subscription intake never waits for ACK capacity.
                state.rescan = true;
                continue;
            }
            state.retry_at.remove(&key);
            state.retained.insert(key.clone());
            state.pending.push_back(key);
        }
        self.changed.notify_one();
    }

    fn stop(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.stopped = true;
        self.changed.notify_all();
    }

    fn next(&self) -> Option<AckWork> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            // This mutex serializes operation admission with stop. A request returned here is
            // owned until completion; queued requests stay durable when stop wins instead.
            if state.stopped {
                return None;
            }
            if let Some(key) = state.pending.pop_front() {
                return Some(AckWork::Request(key));
            }
            if state.rescan {
                state.rescan = false;
                return Some(AckWork::Reconcile);
            }
            let next_retry = state.retry_at.values().min().copied();
            if let Some(retry) = next_retry {
                let now = Instant::now();
                if retry <= now {
                    state.retry_at.retain(|_, deadline| *deadline > now);
                    return Some(AckWork::Reconcile);
                }
                state = self
                    .changed
                    .wait_timeout(state, retry - now)
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .0;
            } else {
                state = self
                    .changed
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
    }

    fn finished(&self, key: &str, failed: bool) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.retained.remove(key);
        if failed {
            // Failed ACKs cannot retire, so this map is bounded by the durable request limit.
            // Retain an explicit ceiling as well if a caller supplies an invalid request key.
            if state.retry_at.len() < MAX_DIRECT_REQUEST_KEYS {
                state
                    .retry_at
                    .insert(key.to_owned(), Instant::now() + ACK_RETRY_DELAY);
            }
        } else {
            state.retry_at.remove(key);
        }
        self.changed.notify_one();
    }
}

/// Start the ACK worker thread. It writes each failed acknowledgement as one line through `log`;
/// production passes `service_log`.
fn spawn_ack_worker(
    state: BridgeState,
    queue: Arc<AckQueue>,
    mut transport: impl chat_runtime::ReactionTransport + Send + 'static,
    stop: Arc<StopState>,
    output_wake: SharedWake,
    log: impl Fn(fmt::Arguments<'_>) + Send + 'static,
) -> io::Result<ServiceWorker> {
    let (done_sender, done) = mpsc::sync_channel(1);
    let handle = thread::Builder::new()
        .name("agentctl-chat-ack".to_owned())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                while let Some(work) = queue.next() {
                    match work {
                        AckWork::Request(key) => {
                            let result = state.ensure_ack(&key, &mut transport);
                            queue.finished(&key, result.is_err());
                            let mut report = CycleReport::default();
                            record_acknowledgement(&mut report, &key, result);
                            log_report(&report, &log);
                            wake_output(&output_wake);
                        }
                        AckWork::Reconcile => match state.pending_ack_keys() {
                            Ok(keys) => queue.enqueue(keys),
                            Err(error) => {
                                stop.record_cleanup_error(format!("ACK recovery failed: {error}"));
                                stop.stop();
                                wake_output(&output_wake);
                            }
                        },
                    }
                }
            }));
            if let Err(panic) = result {
                stop.record_cleanup_error("ACK worker panicked; operation outcome is uncertain");
                stop.stop();
                wake_output(&output_wake);
                std::panic::resume_unwind(panic);
            }
            let _ = done_sender.send(());
        })?;
    Ok(ServiceWorker { handle, done })
}

fn join_ack_worker_until(
    worker: ServiceWorker,
    cancellation: &OutboundCancellation,
    deadline: Instant,
) -> Result<(), ChatServiceError> {
    // Keep a small part of the existing outer margin for cancellation and process reaping if
    // normal bounded completion unexpectedly stalls. Never cancel an admitted ACK on SIGTERM.
    let remaining = deadline.saturating_duration_since(Instant::now());
    if matches!(
        worker
            .done
            .recv_timeout(remaining.saturating_sub(Duration::from_secs(5))),
        Err(mpsc::RecvTimeoutError::Timeout)
    ) {
        cancellation.cancel();
    }
    join_worker_until(worker, "chat ACK", deadline)
}

fn connected_provider_join_timeout(timeouts: ProcessPhaseTimeouts) -> Duration {
    timeouts
        .close()
        .saturating_add(timeouts.shutdown_grace())
        .saturating_add(Duration::from_secs(PROCESS_AND_JOIN_MARGIN_SECONDS))
}

fn pending_provider_join_timeout(timeouts: ProcessPhaseTimeouts) -> Duration {
    timeouts
        .hello()
        .saturating_add(connected_provider_join_timeout(timeouts))
}

fn configured_outbound_join_timeout(configuration: &BridgeConfiguration) -> Duration {
    configuration
        .outbound_command
        .as_ref()
        .map(|command| {
            Duration::from_millis(command.timeout_millis)
                .saturating_add(Duration::from_millis(command.shutdown_grace_millis))
                .saturating_add(Duration::from_secs(PROCESS_AND_JOIN_MARGIN_SECONDS))
        })
        .unwrap_or(Duration::ZERO)
}

fn service_join_timeout(provider: Duration, outbound: Duration) -> Duration {
    provider.max(outbound)
}

fn validate_selected_process_timeouts(
    selected: ProcessPhaseTimeouts,
    observed: ProcessPhaseTimeouts,
) -> Result<(), ProviderGenerationError> {
    if observed == selected {
        Ok(())
    } else {
        Err(ProviderGenerationError::Fatal(format!(
            "provider process-phase policy changed during one service generation: selected {selected:?}, observed {observed:?}; restart the service to adopt the new policy"
        )))
    }
}

fn connect_provider_unless_stopped<T>(
    stop: &StopState,
    pending_join_timeout: Duration,
    connect: impl FnOnce() -> Result<T, ProviderGenerationError>,
) -> Result<T, ProviderGenerationError> {
    if stop.is_stopped() {
        // A concurrent signal may have observed the prior connected generation immediately before
        // this reservation. Publish the pinned pending-generation window on the same shutdown
        // origin, but do not spawn a generation after stop was already observable here.
        stop.stop_with_timeout(pending_join_timeout);
        return Err(ProviderGenerationError::Cancelled);
    }
    // A stop racing after this check is covered by the already-published reservation and its
    // Hello-inclusive window. `connect` then either installs cancellation or reaches its bounded
    // Hello failure without introducing a second deadline origin.
    connect()
}

fn begin_service_stop(
    stop: &StopState,
    cancellation: &SharedCancellation,
    fallback_provider_join_timeout: Duration,
    outbound_join_timeout: Duration,
) -> Instant {
    let mut state = cancellation
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let provider_join_timeout = match state.stop_join_timeout {
        Some(timeout) => timeout,
        None => {
            let timeout = if state.pending.is_some() || state.active.is_some() {
                state
                    .last_join_timeout
                    .unwrap_or(fallback_provider_join_timeout)
            } else {
                fallback_provider_join_timeout
            };
            state.stopping = true;
            state.stop_join_timeout = Some(timeout);
            timeout
        }
    };
    let deadline = stop.stop_with_timeout(service_join_timeout(
        provider_join_timeout,
        outbound_join_timeout,
    ));
    drop(state);
    deadline
}

fn validate_service_outbound(configuration: &BridgeConfiguration) -> Result<(), ChatServiceError> {
    if configuration.outbound_enabled && configuration.outbound_command.is_none() {
        return Err(ChatServiceError::Generation(
            "outbound-enabled chat service requires an explicit outbound_command helper".to_owned(),
        ));
    }
    Ok(())
}

/// The newest complete lines of a coordinator output snapshot that fit in `MAX_SNAPSHOT_BYTES`,
/// and whether older output was left out. Replies are near the end of the output, and an
/// oversized snapshot must not stop the service.
fn bounded_snapshot(snapshot: &str) -> (&str, bool) {
    if snapshot.len() <= MAX_SNAPSHOT_BYTES {
        return (snapshot, false);
    }
    // A line that starts at or after this byte fits.
    let first_byte = snapshot.len() - MAX_SNAPSHOT_BYTES;
    let kept = snapshot.as_bytes()[first_byte - 1..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or("", |newline| &snapshot[first_byte + newline..]);
    (kept, true)
}

const SNAPSHOT_CUT_NOTE: &str =
    "coordinator output exceeded the snapshot size limit, so only its newest complete lines were read";

fn recover_pass<A: ManagedApi + ?Sized>(
    state: &BridgeState,
    manager: &ManagedAgents<'_, A>,
    delivery: DrainOptions,
    routes: &mut RouteCache,
    control: &mut PassControl<'_>,
) -> Result<CycleReport, ChatServiceError> {
    let keys = state.pending_work_keys()?;
    let mut report = process_keys(state, manager, delivery, &keys, control)?;
    if keys.len() > MAX_KEYS_PER_PASS {
        report.more_work = true;
    }
    *routes = RouteCache::from_entries(state.reply_route_entries()?);
    Ok(report)
}

fn process_keys<A: ManagedApi + ?Sized>(
    state: &BridgeState,
    manager: &ManagedAgents<'_, A>,
    delivery: DrainOptions,
    keys: &[String],
    control: &mut PassControl<'_>,
) -> Result<CycleReport, ChatServiceError> {
    match control.stop {
        Some(stop) => process_keys_with_delivery(
            state,
            &CancellableDelivery {
                manager,
                runtime: StopRuntime::new(stop),
            },
            delivery,
            keys,
            control,
        ),
        None => process_keys_with_delivery(state, manager, delivery, keys, control),
    }
}

fn process_keys_with_delivery(
    state: &BridgeState,
    coordinator: &dyn chat_runtime::CoordinatorDelivery,
    delivery: DrainOptions,
    keys: &[String],
    control: &mut PassControl<'_>,
) -> Result<CycleReport, ChatServiceError> {
    if keys.is_empty() {
        return Ok(CycleReport::default());
    }
    let typed = Cell::new(false);
    let coordinator = &TypingWatch {
        delivery: coordinator,
        typed: &typed,
    };
    let async_ack = control.stop.and_then(|stop| stop.ack_queue.as_ref());
    if let Some(queue) = async_ack {
        queue.enqueue(keys.iter().cloned());
    }
    let mut report = CycleReport {
        deferred_keys: keys.iter().skip(MAX_KEYS_PER_PASS).cloned().collect(),
        more_work: keys.len() > MAX_KEYS_PER_PASS,
        ..CycleReport::default()
    };
    for key in keys.iter().take(MAX_KEYS_PER_PASS) {
        report.processed_keys.push(key.clone());
        if control.stopped() {
            report.more_work = true;
            break;
        }
        let acknowledgement_enabled = state.config().ack_reaction.is_some();
        let concurrent =
            if async_ack.is_none() && acknowledgement_enabled && control.transport.is_some() {
                let transport = control
                    .transport
                    .as_mut()
                    .expect("checked acknowledgement transport");
                Some(thread::scope(|scope| {
                    match thread::Builder::new()
                        .name("agentctl-chat-ack".to_owned())
                        .spawn_scoped(scope, move || state.ensure_ack(key, transport))
                    {
                        Ok(worker) => Ok((
                            chat_runtime::deliver_request_with(state, coordinator, key, delivery),
                            worker.join(),
                        )),
                        Err(error) => Err(error),
                    }
                }))
            } else {
                None
            };

        let delivery_result = match concurrent {
            Some(Ok((delivery_result, acknowledgement_result))) => {
                match acknowledgement_result {
                    Ok(result) => record_acknowledgement(&mut report, key, result),
                    Err(_) => report.error("acknowledge", key, "acknowledgement worker panicked"),
                }
                delivery_result
            }
            Some(Err(spawn_error)) => {
                control.log(format_args!(
                    "agentctl: chat acknowledgement worker could not start for {key}; \
                     running synchronously: {spawn_error}"
                ));
                let delivery_result =
                    chat_runtime::deliver_request_with(state, coordinator, key, delivery);
                let transport = control
                    .transport
                    .as_mut()
                    .expect("spawn failure retained acknowledgement transport");
                let acknowledgement_result = state.ensure_ack(key, transport);
                record_acknowledgement(&mut report, key, acknowledgement_result);
                delivery_result
            }
            None => chat_runtime::deliver_request_with(state, coordinator, key, delivery),
        };
        match delivery_result {
            Ok(
                CoordinatorDeliveryResult::Delivered | CoordinatorDeliveryResult::AlreadyDelivered,
            ) => report.delivered.push(key.clone()),
            Ok(CoordinatorDeliveryResult::Pending(_)) => {
                report.delivery_pending.push(key.clone());
                report.more_work = true;
            }
            Ok(CoordinatorDeliveryResult::Uncertain(_)) => {
                report.delivery_uncertain.push(key.clone());
            }
            Err(error) => report.error("deliver", key, error),
        }
    }
    report.prompt_typed = typed.get();
    if let Some(transport) = control.transport.as_mut() {
        let mut sends = 0_usize;
        for key in keys.iter().take(MAX_KEYS_PER_PASS) {
            while sends < MAX_SENDS_PER_PASS {
                if control.stop.is_some_and(StopState::is_stopped) {
                    report.more_work = true;
                    return Ok(report);
                }
                match state.publish_one(key, transport) {
                    Ok(Some(receipt)) => {
                        report.sent.push(receipt);
                        sends += 1;
                    }
                    Ok(None) => break,
                    Err(error) => {
                        report.error("publish", key, error);
                        break;
                    }
                }
            }
            if sends == MAX_SENDS_PER_PASS {
                report.more_work = true;
                break;
            }
        }
    }
    Ok(report)
}

fn record_acknowledgement(
    report: &mut CycleReport,
    key: &str,
    result: Result<AckResult, ChatRuntimeError>,
) {
    match result {
        Ok(AckResult::Disabled) => {}
        Ok(AckResult::Acked(_)) => report.acknowledged.push(key.to_owned()),
        Err(error) => report.error("acknowledge", key, error),
    }
}

fn capture_recovery_snapshot<A: ManagedApi + ?Sized>(
    state: &BridgeState,
    manager: &ManagedAgents<'_, A>,
    delivery: DrainOptions,
    routes: &mut RouteCache,
    snapshot: SnapshotInput<'_>,
    control: &mut PassControl<'_>,
) -> Result<CycleReport, ChatServiceError> {
    let (text, cut) = bounded_snapshot(snapshot.text);
    let gate = prompt_gate(state, manager, None, control.stop)?;
    let capture = state.capture_snapshot_with_gate(text, &gate)?;
    let route_entries = capture.route_entries.clone();
    let mut report = CycleReport {
        captured: capture.replies.clone(),
        snapshot_truncated: snapshot.truncated,
        snapshot_revision: snapshot.revision,
        ..CycleReport::default()
    };
    if cut {
        report.note(SNAPSHOT_CUT_NOTE);
    }
    if capture.overflowed {
        report.note(format!(
            "coordinator output held more than {} reply blocks, so the oldest were not read",
            chat_runtime::MAX_VISIBLE_MARKERS
        ));
    }
    if capture.inline_overflowed {
        report.note(format!(
            "coordinator output held more than {} reply blocks with a tag that shared its line \
with other text, so the oldest were not read",
            chat_runtime::MAX_VISIBLE_MARKERS
        ));
    }
    for refusal in &capture.refused {
        report.note(refusal);
    }
    // Visible markers the coordinator already received are not reported twice. Log them, since
    // a marker that stays on screen is otherwise silent after its one report.
    if let Some(line) = already_reported_log_line(&capture.suppressed_ids) {
        control.log(line);
    }
    if !capture.unknown_ids.is_empty() || !capture.covered_ids.is_empty() {
        let typed = Cell::new(false);
        let result = deliver_feedback(
            state,
            manager,
            &capture.unknown_ids,
            &capture.covered_ids,
            delivery,
            control.stop,
            &typed,
        );
        report.prompt_typed = typed.get();
        match result {
            Ok(CoordinatorDeliveryResult::Pending(_)) => report.more_work = true,
            Ok(CoordinatorDeliveryResult::Uncertain(_)) => {}
            Ok(
                CoordinatorDeliveryResult::Delivered | CoordinatorDeliveryResult::AlreadyDelivered,
            ) => {}
            Err(error) => report.error("reply-fence-feedback", "coordinator", error),
        }
    }
    let captured_keys = capture
        .replies
        .into_iter()
        .map(|(key, _)| key)
        .collect::<Vec<_>>();
    report.merge(process_keys(
        state,
        manager,
        delivery,
        &captured_keys,
        control,
    )?);
    *routes = RouteCache::from_entries(route_entries);
    Ok(report)
}

fn capture_direct<A: ManagedApi + ?Sized>(
    state: &BridgeState,
    manager: &ManagedAgents<'_, A>,
    delivery: DrainOptions,
    routes: &mut RouteCache,
    identifier: &str,
    snapshot: SnapshotInput<'_>,
    control: &mut PassControl<'_>,
) -> Result<CycleReport, ChatServiceError> {
    let key = match refresh_matched_route(state, routes, identifier)? {
        MatchedRoute::Unknown => {
            return Ok(CycleReport {
                snapshot_truncated: snapshot.truncated,
                snapshot_revision: snapshot.revision,
                recovery_requested: true,
                ..CycleReport::default()
            });
        }
        MatchedRoute::Stale => {
            return Ok(CycleReport {
                snapshot_truncated: snapshot.truncated,
                snapshot_revision: snapshot.revision,
                ..CycleReport::default()
            });
        }
        MatchedRoute::Current(key) => key,
    };
    if control.stopped() {
        return Ok(CycleReport {
            snapshot_truncated: snapshot.truncated,
            snapshot_revision: snapshot.revision,
            more_work: true,
            ..CycleReport::default()
        });
    }
    let (text, cut) = bounded_snapshot(snapshot.text);
    let gate = prompt_gate(state, manager, Some(&key), control.stop)?;
    let capture = state.capture_replies_with_gate(&key, text, &gate)?;
    let mut report = CycleReport {
        snapshot_truncated: snapshot.truncated,
        snapshot_revision: snapshot.revision,
        ..CycleReport::default()
    };
    if cut {
        report.note(SNAPSHOT_CUT_NOTE);
    }
    for refusal in &capture.refused {
        report.note(refusal);
    }
    if !capture.ordinals.is_empty() {
        report.captured.push((key.clone(), capture.ordinals));
        report.merge(process_keys(
            state,
            manager,
            delivery,
            std::slice::from_ref(&key),
            control,
        )?);
    }
    routes.replace(&key, state.next_reply_route(&key)?);
    Ok(report)
}

/// The open requests whose prompt has not reached the coordinator, or may not have, as the queue
/// shows them now, among all of them or only `only`: see [`chat_runtime::PromptGate`]. The pane
/// text a capture scans must be read before this.
fn prompt_gate<A: ManagedApi + ?Sized>(
    state: &BridgeState,
    manager: &ManagedAgents<'_, A>,
    only: Option<&str>,
    stop: Option<&StopState>,
) -> Result<chat_runtime::PromptGate, ChatRuntimeError> {
    match stop {
        Some(stop) => state.prompt_gate(
            &CancellableDelivery {
                manager,
                runtime: StopRuntime::new(stop),
            },
            only,
        ),
        None => state.prompt_gate(manager, only),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum MatchedRoute {
    Unknown,
    Stale,
    Current(String),
}

/// Route one closing marker seen in the output. Any well-formed reply ID of an open request routes
/// to that request: its reply alias, or any `<nonce>_<ordinal>`, not only its next one. A block
/// under an earlier ID may hold new text, and the capture decides by the text whether it does.
fn refresh_matched_route(
    state: &BridgeState,
    routes: &mut RouteCache,
    identifier: &str,
) -> Result<MatchedRoute, ChatRuntimeError> {
    let Some((key, open)) = routes.identifier_route(identifier) else {
        return Ok(MatchedRoute::Unknown);
    };
    let key = key.to_owned();
    let well_formed = chat_runtime::parse_reply_alias(identifier).is_some()
        || chat_runtime::has_reply_ordinal(identifier);
    if !open || !well_formed {
        return Ok(MatchedRoute::Stale);
    }
    let current = state.next_reply_route(&key)?;
    let still_open = current.is_some();
    routes.replace(&key, current);
    Ok(if still_open {
        MatchedRoute::Current(key)
    } else {
        MatchedRoute::Stale
    })
}

/// When to read the pane again because `text` shows a closing line that keeps an output pattern
/// matched, as `RouteCache::saturated_by` describes; `None` when it shows none.
fn next_saturated_poll(routes: &RouteCache, text: &str) -> Option<Instant> {
    routes
        .saturated_by(text)
        .then(|| Instant::now() + SATURATED_POLL_INTERVAL)
}

/// Capture the replies of each open request whose current reply ID has a closing line in
/// `text`, as an output event for that line would. This reports no partial block, so the
/// service can run it while the agent is still writing.
fn capture_visible_current<A: ManagedApi + ?Sized>(
    state: &BridgeState,
    manager: &ManagedAgents<'_, A>,
    delivery: DrainOptions,
    routes: &mut RouteCache,
    text: &str,
    control: &mut PassControl<'_>,
) -> Result<CycleReport, ChatServiceError> {
    let mut report = CycleReport::default();
    for identifier in routes.visible_current_identifiers(text) {
        report.merge(capture_direct(
            state,
            manager,
            delivery,
            routes,
            &identifier,
            SnapshotInput {
                text,
                truncated: false,
                revision: None,
            },
            control,
        )?);
    }
    Ok(report)
}

#[derive(Clone, Copy)]
struct SnapshotInput<'a> {
    text: &'a str,
    truncated: bool,
    revision: Option<u64>,
}

struct PassControl<'a> {
    transport: &'a mut Option<CommandOutboundTransport>,
    stop: Option<&'a StopState>,
}

impl PassControl<'_> {
    fn stopped(&self) -> bool {
        self.stop.is_some_and(StopState::is_stopped)
    }

    /// Write one diagnostic line to standard error. Only `chat run` passes its stop state, and
    /// its standard error is the service log, so there the line starts with the time; `chat tick`
    /// prints it without one.
    fn log(&self, line: impl fmt::Display) {
        self.log_to(&mut io::stderr().lock(), line);
    }

    /// Write one diagnostic line to `out`, as `log` describes.
    fn log_to(&self, out: &mut impl io::Write, line: impl fmt::Display) {
        if self.stop.is_some() {
            write_service_log(out, line);
        } else if let Err(error) = writeln!(out, "{line}") {
            // `chat tick` keeps what the `eprintln!` it always used here does on a failed write.
            panic!("failed printing to stderr: {error}");
        }
    }
}

#[derive(Debug)]
struct RouteCache {
    by_identifier: BTreeMap<String, String>,
    by_key: BTreeMap<String, (String, Option<String>)>,
    by_nonce: BTreeMap<String, String>,
    /// The reply aliases of open and closed requests, like `by_nonce`.
    by_alias: BTreeMap<String, String>,
}

impl RouteCache {
    #[cfg(test)]
    fn new(routes: Vec<ReplyRoute>) -> Self {
        Self::from_entries(
            routes
                .into_iter()
                .map(|route| ReplyRouteEntry {
                    alias: chat_runtime::parse_reply_alias(&route.identifier)
                        .map(|_| route.identifier.clone()),
                    key: route.key,
                    nonce: route.nonce,
                    current_identifier: Some(route.identifier),
                })
                .collect(),
        )
    }

    fn from_entries(entries: Vec<ReplyRouteEntry>) -> Self {
        let mut result = Self {
            by_identifier: BTreeMap::new(),
            by_key: BTreeMap::new(),
            by_nonce: BTreeMap::new(),
            by_alias: BTreeMap::new(),
        };
        for entry in entries {
            if let Some(identifier) = entry.current_identifier.as_ref() {
                result
                    .by_identifier
                    .insert(identifier.clone(), entry.key.clone());
            }
            if let Some(alias) = entry.alias {
                result.by_alias.insert(alias, entry.key.clone());
            }
            result
                .by_nonce
                .insert(entry.nonce.clone(), entry.key.clone());
            result
                .by_key
                .insert(entry.key, (entry.nonce, entry.current_identifier));
        }
        result
    }

    fn replace(&mut self, key: &str, route: Option<ReplyRoute>) {
        let previous = self.by_key.remove(key);
        if let Some((_, Some(identifier))) = previous.as_ref() {
            self.by_identifier.remove(identifier);
        }
        if let Some(route) = route {
            self.by_nonce.insert(route.nonce.clone(), route.key.clone());
            if chat_runtime::parse_reply_alias(&route.identifier).is_some() {
                self.by_alias
                    .insert(route.identifier.clone(), route.key.clone());
            }
            self.by_key.insert(
                route.key.clone(),
                (route.nonce, Some(route.identifier.clone())),
            );
            self.by_identifier.insert(route.identifier, route.key);
        } else if let Some((nonce, _)) = previous {
            self.by_nonce.insert(nonce.clone(), key.to_owned());
            self.by_key.insert(key.to_owned(), (nonce, None));
        }
    }

    #[cfg(test)]
    fn key(&self, identifier: &str) -> Option<&str> {
        self.by_identifier.get(identifier).map(String::as_str)
    }

    #[cfg(test)]
    fn knows_identifier_nonce(&self, identifier: &str) -> bool {
        identifier
            .rsplit_once('_')
            .is_some_and(|(nonce, _)| self.nonce_route(nonce).is_some())
    }

    /// The request that owns a nonce, and whether its capture was open when last read.
    #[cfg(test)]
    fn nonce_route(&self, nonce: &str) -> Option<(&str, bool)> {
        let key = self.by_nonce.get(nonce)?;
        Some((key.as_str(), self.is_open(key)))
    }

    /// The request a reply ID names, and whether its capture was open when last read: the
    /// owner of a reply alias, or of the nonce of `<nonce>_<ordinal>`.
    fn identifier_route(&self, identifier: &str) -> Option<(&str, bool)> {
        let key = if chat_runtime::parse_reply_alias(identifier).is_some() {
            self.by_alias.get(identifier)?
        } else {
            self.by_nonce.get(identifier.rsplit_once('_')?.0)?
        };
        Some((key.as_str(), self.is_open(key)))
    }

    fn is_open(&self, key: &str) -> bool {
        self.by_key
            .get(key)
            .is_some_and(|(_, current)| current.is_some())
    }

    /// Whether some open request has a current reply ID, so that a reply could be stored.
    fn has_current(&self) -> bool {
        !self.by_identifier.is_empty()
    }

    /// The current reply IDs of open requests whose closing line `text` shows, each once, in
    /// the order of the lines.
    fn visible_current_identifiers(&self, text: &str) -> Vec<String> {
        let mut seen = BTreeSet::new();
        text.lines()
            .filter_map(matched_identifier)
            .filter(|identifier| self.by_identifier.contains_key(*identifier))
            .filter(|identifier| seen.insert(*identifier))
            .map(str::to_owned)
            .collect()
    }

    /// Whether `text` shows the closing line of an open request's reply alias. Herdr reports an
    /// output pattern only when it starts to match, and a request keeps its alias for every
    /// reply, so while such a line stays in the pane a later closing line under any ID of the
    /// same pattern raises no event. The service reads the pane itself instead, every
    /// `SATURATED_POLL_INTERVAL`, until no such line is left. A `<nonce>_<ordinal>` ID changes
    /// once its reply is stored, which subscribes a new pattern that starts unmatched. A closing
    /// line under a current `<nonce>_<ordinal>` ID that is not stored, because it has no opening
    /// line or sits inside a prompt echo or tool output, keeps its pattern matched as well but
    /// does not start these reads: a reply under another ID of that pattern then raises no event
    /// and waits for a read of the whole pane, such as the one when the pane settles.
    fn saturated_by(&self, text: &str) -> bool {
        text.lines()
            .filter_map(matched_identifier)
            .any(|identifier| {
                chat_runtime::parse_reply_alias(identifier).is_some()
                    && self.by_identifier.contains_key(identifier)
            })
    }

    fn patterns(&self) -> Result<Vec<String>, ChatServiceError> {
        const PREFIX: &str = r"^[^\S\r\n]*(?:[•⏺●][ \t]+)?</(?:GCHAT|CHAT)_REPLY_";
        const SUFFIX: &str = r">[^\S\r\n]*$";
        let mut patterns = Vec::new();
        let mut alternatives = String::new();
        for identifier in self.by_identifier.keys() {
            if identifier.is_empty()
                || identifier.len() > 29
                || !identifier
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            {
                return Err(ChatServiceError::Generation(
                    "reply route has an invalid output subscription identifier".to_owned(),
                ));
            }
            // Herdr's match stays true while any matching line remains in the pane. Keep current
            // IDs separate from the generic predicate, and rearm them when a route advances.
            // Chunk exact literals so all durable requests fit without broadening the matcher.
            let extra = identifier.len() + usize::from(!alternatives.is_empty());
            if PREFIX.len() + 4 + alternatives.len() + extra + SUFFIX.len()
                > chat_events::MAX_PATTERN_BYTES
            {
                patterns.push(format!("{PREFIX}(?:{alternatives}){SUFFIX}"));
                alternatives.clear();
            }
            if !alternatives.is_empty() {
                alternatives.push('|');
            }
            alternatives.push_str(identifier);
        }
        if !alternatives.is_empty() {
            patterns.push(format!("{PREFIX}(?:{alternatives}){SUFFIX}"));
        }
        patterns.push(format!(r"{PREFIX}[^<>\s]*{SUFFIX}"));
        if patterns.len() > chat_events::MAX_PATTERNS
            || patterns.iter().map(String::len).sum::<usize>()
                > chat_events::MAX_TOTAL_PATTERN_BYTES
        {
            return Err(ChatServiceError::Generation(
                "reply routes exceed the output subscription budget".to_owned(),
            ));
        }
        Ok(patterns)
    }
}

fn matched_identifier(line: &str) -> Option<&str> {
    let stripped = line.trim();
    let stripped = ["• ", "⏺ ", "● "]
        .into_iter()
        .find_map(|prefix| stripped.strip_prefix(prefix))
        .unwrap_or(stripped)
        .trim();
    let body = stripped.strip_prefix("</")?.strip_suffix('>')?;
    body.strip_prefix("CHAT_REPLY_")
        .or_else(|| body.strip_prefix("GCHAT_REPLY_"))
}

/// What the provider worker tells the owner loop. The worker logs its own connects, failures, and
/// reconnect waits as they happen, so those lines keep their order and their times even while
/// the owner loop is busy or this bounded channel is full.
#[derive(Debug)]
enum ProviderNotice {
    Batch(Vec<String>),
    Fatal(String),
}

/// The wait before each provider reconnect. It doubles after each generation that ended sooner
/// than `PROVIDER_RETRY_MAX`, up to that bound. A generation that lasted at least that long was
/// healthy, so its end is a new incident and the wait starts again from `PROVIDER_RETRY_MIN`.
struct ProviderBackoff {
    next: Duration,
}

impl ProviderBackoff {
    fn new() -> Self {
        Self {
            next: PROVIDER_RETRY_MIN,
        }
    }

    /// The wait before reconnecting after a generation that lasted `lasted`.
    fn wait_after(&mut self, lasted: Duration) -> Duration {
        if lasted >= PROVIDER_RETRY_MAX {
            self.next = PROVIDER_RETRY_MIN;
        }
        let wait = self.next;
        self.next = (self.next * 2).min(PROVIDER_RETRY_MAX);
        wait
    }
}

#[derive(Debug)]
enum ProviderGenerationError {
    Cancelled,
    Cleanup(String),
    Retryable(String),
    Fatal(String),
}

#[derive(Default)]
struct StopState {
    stopped: AtomicBool,
    lock: Mutex<()>,
    changed: Condvar,
    cleanup_errors: Mutex<Vec<String>>,
    shutdown_window: Mutex<Option<(Instant, Instant)>>,
    ack_queue: Option<Arc<AckQueue>>,
}

impl StopState {
    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    fn stop(&self) {
        if let Some(queue) = self.ack_queue.as_ref() {
            queue.stop();
        }
        self.stopped.store(true, Ordering::SeqCst);
        self.changed.notify_all();
    }

    fn stop_with_timeout(&self, timeout: Duration) -> Instant {
        let now = Instant::now();
        let mut window = self
            .shutdown_window
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (started, current_deadline) = window.unwrap_or((now, now));
        let candidate = started.checked_add(timeout).unwrap_or(current_deadline);
        let deadline = current_deadline.max(candidate);
        *window = Some((started, deadline));
        drop(window);
        self.stop();
        deadline
    }

    fn shutdown_deadline(&self) -> Option<Instant> {
        self.shutdown_window
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .map(|(_, deadline)| deadline)
    }

    fn wait(&self, duration: Duration) {
        let guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.is_stopped() {
            let _ = self
                .changed
                .wait_timeout(guard, duration)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    fn record_cleanup_error(&self, error: impl Into<String>) {
        self.cleanup_errors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(error.into());
    }

    fn take_cleanup_errors(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .cleanup_errors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

struct StopRuntime<'a> {
    stop: &'a StopState,
    origin: Instant,
}

impl<'a> StopRuntime<'a> {
    fn new(stop: &'a StopState) -> Self {
        Self {
            stop,
            origin: Instant::now(),
        }
    }
}

impl AgentRuntime for StopRuntime<'_> {
    fn monotonic(&self) -> Duration {
        self.origin.elapsed()
    }

    fn sleep(&self, duration: Duration) {
        self.stop.wait(duration);
    }

    fn cancelled(&self) -> bool {
        self.stop.is_stopped()
    }

    fn delivery_wait_chunk(&self) -> Option<Duration> {
        Some(Duration::from_millis(250))
    }
}

struct CancellableDelivery<'a, A: ManagedApi + ?Sized> {
    manager: &'a ManagedAgents<'a, A>,
    runtime: StopRuntime<'a>,
}

impl<A: ManagedApi + ?Sized> chat_runtime::CoordinatorDelivery for CancellableDelivery<'_, A> {
    fn message_state(
        &self,
        agent_name: &str,
        message_id: &str,
    ) -> std::result::Result<Option<QueueMessageState>, String> {
        self.manager
            .message_state_with_runtime(agent_name, message_id, &self.runtime)
            .map_err(|error| error.to_string())
    }

    fn submit(
        &self,
        agent_name: &str,
        prompt: &str,
        message_id: &str,
        options: DrainOptions,
    ) -> std::result::Result<(), String> {
        if self.runtime.cancelled() {
            return Err("chat delivery was cancelled before prompt submission".to_owned());
        }
        self.manager
            .send_identified_with_runtime(agent_name, prompt, options, message_id, &self.runtime)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn drain(&self, agent_name: &str, options: DrainOptions) -> std::result::Result<(), String> {
        if self.runtime.cancelled() {
            return Err("chat delivery was cancelled before queue drain".to_owned());
        }
        self.manager
            .drain_with_runtime(agent_name, options, &self.runtime)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn screen(&self, agent_name: &str) -> std::result::Result<String, String> {
        self.manager
            .peek_capture_with_runtime(agent_name, SNAPSHOT_LINES, &self.runtime)
            .map_err(|error| error.to_string())
    }
}

/// A delivery that sets `typed` once it is asked to type into the pane: `submit` types a prompt
/// and `drain` types the queued ones. Either may have typed even when it fails, so `typed` says
/// that a prompt may have reached the pane, not that one did.
struct TypingWatch<'a> {
    delivery: &'a dyn chat_runtime::CoordinatorDelivery,
    typed: &'a Cell<bool>,
}

impl chat_runtime::CoordinatorDelivery for TypingWatch<'_> {
    fn message_state(
        &self,
        agent_name: &str,
        message_id: &str,
    ) -> std::result::Result<Option<QueueMessageState>, String> {
        self.delivery.message_state(agent_name, message_id)
    }

    fn submit(
        &self,
        agent_name: &str,
        prompt: &str,
        message_id: &str,
        options: DrainOptions,
    ) -> std::result::Result<(), String> {
        self.typed.set(true);
        self.delivery
            .submit(agent_name, prompt, message_id, options)
    }

    fn drain(&self, agent_name: &str, options: DrainOptions) -> std::result::Result<(), String> {
        self.typed.set(true);
        self.delivery.drain(agent_name, options)
    }

    fn screen(&self, agent_name: &str) -> std::result::Result<String, String> {
        self.delivery.screen(agent_name)
    }
}

/// `chat_runtime::deliver_fence_feedback_covering` through the service's delivery, which sets
/// `typed` as `TypingWatch` does.
fn deliver_feedback<A: ManagedApi + ?Sized>(
    state: &BridgeState,
    manager: &ManagedAgents<'_, A>,
    unknown_ids: &[String],
    covered_ids: &[(String, String)],
    options: DrainOptions,
    stop: Option<&StopState>,
    typed: &Cell<bool>,
) -> Result<CoordinatorDeliveryResult, ChatRuntimeError> {
    match stop {
        Some(stop) => chat_runtime::deliver_fence_feedback_covering(
            state,
            &TypingWatch {
                delivery: &CancellableDelivery {
                    manager,
                    runtime: StopRuntime::new(stop),
                },
                typed,
            },
            unknown_ids,
            covered_ids,
            options,
        ),
        None => chat_runtime::deliver_fence_feedback_covering(
            state,
            &TypingWatch {
                delivery: manager,
                typed,
            },
            unknown_ids,
            covered_ids,
            options,
        ),
    }
}

const CANCELLED_PROCESS_RECEIVE_CODE: &str = "plugin_receive_cancelled";
const CANCELLED_PROCESS_START_CODE: &str = "plugin_start_cancelled";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProviderReceiveIdentity(u64);

#[derive(Default)]
struct ReceiveOperationState {
    last_identity: u64,
    active: Option<ProviderReceiveIdentity>,
    failed: Option<ProviderReceiveIdentity>,
}

#[derive(Default)]
struct ReceiveOperationTracker {
    state: Mutex<ReceiveOperationState>,
}

impl ReceiveOperationTracker {
    fn begin(&self) -> std::result::Result<ProviderReceiveIdentity, BackendFailure> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.active.is_some() {
            return Err(BackendFailure::new(
                "host_receive_overlap",
                "provider host attempted overlapping receive operations",
                false,
            )
            .expect("constant backend failure is valid"));
        }
        state.last_identity = state.last_identity.checked_add(1).ok_or_else(|| {
            BackendFailure::new(
                "host_receive_identity_exhausted",
                "provider receive identity space is exhausted",
                false,
            )
            .expect("constant backend failure is valid")
        })?;
        let identity = ProviderReceiveIdentity(state.last_identity);
        state.active = Some(identity);
        state.failed = None;
        Ok(identity)
    }

    fn finish(&self, identity: ProviderReceiveIdentity, failed: bool) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.active == Some(identity) {
            state.active = None;
            state.failed = failed.then_some(identity);
        }
    }

    fn active(&self) -> Option<ProviderReceiveIdentity> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active
    }

    fn failed(&self) -> Option<ProviderReceiveIdentity> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failed
    }
}

struct ReceiveFailureTrackingBackend<B> {
    inner: B,
    receive_tracker: Arc<ReceiveOperationTracker>,
}

impl<B: ChatSubscriptionBackend> ChatSubscriptionBackend for ReceiveFailureTrackingBackend<B> {
    fn cancellation(&self) -> Arc<dyn ChatSubscriptionCancellation> {
        self.inner.cancellation()
    }

    fn capabilities(&self) -> BackendCapabilities {
        self.inner.capabilities()
    }

    fn subscribe(
        &mut self,
        request: &SubscribeRequest,
    ) -> std::result::Result<Box<dyn ChatSubscriptionDriver>, BackendFailure> {
        self.inner.subscribe(request).map(|inner| {
            Box::new(ReceiveFailureTrackingDriver {
                inner,
                receive_tracker: Arc::clone(&self.receive_tracker),
            }) as Box<dyn ChatSubscriptionDriver>
        })
    }
}

struct ReceiveFailureTrackingDriver {
    inner: Box<dyn ChatSubscriptionDriver>,
    receive_tracker: Arc<ReceiveOperationTracker>,
}

impl ChatSubscriptionDriver for ReceiveFailureTrackingDriver {
    fn cancellation(&self) -> Arc<dyn ChatSubscriptionCancellation> {
        self.inner.cancellation()
    }

    fn next_item(&mut self) -> std::result::Result<Option<SubscriptionItem>, BackendFailure> {
        let identity = self.receive_tracker.begin()?;
        let result = self.inner.next_item();
        self.receive_tracker.finish(identity, result.is_err());
        result
    }

    fn acknowledge(&mut self, delivery_id: &DeliveryId) -> std::result::Result<(), BackendFailure> {
        self.inner.acknowledge(delivery_id)
    }

    fn close(&mut self) -> std::result::Result<(), BackendFailure> {
        self.inner.close()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProviderGenerationIdentity(u64);

struct ActiveProviderCancellation {
    identity: ProviderGenerationIdentity,
    authority: Arc<dyn ChatSubscriptionCancellation>,
    receive_tracker: Arc<ReceiveOperationTracker>,
    succeeded: bool,
    cancelled_receive: Option<ProviderReceiveIdentity>,
}

#[derive(Default)]
struct ProviderCancellationRegistry {
    last_identity: u64,
    last_join_timeout: Option<Duration>,
    pending: Option<ProviderGenerationIdentity>,
    active: Option<ActiveProviderCancellation>,
    stopping: bool,
    stop_join_timeout: Option<Duration>,
}

type SharedCancellation = Arc<Mutex<ProviderCancellationRegistry>>;
type SharedWake = Arc<Mutex<Option<PaneEventWake>>>;

struct ProviderCancellationRegistration {
    registry: SharedCancellation,
    identity: ProviderGenerationIdentity,
}

#[derive(Debug, Eq, PartialEq)]
enum ProviderReservationError {
    Stopping,
    Conflict(String),
}

impl ProviderCancellationRegistration {
    fn reserve(
        registry: &SharedCancellation,
        join_timeout: Duration,
    ) -> std::result::Result<Self, ProviderReservationError> {
        let mut state = registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.stopping {
            return Err(ProviderReservationError::Stopping);
        }
        if state.pending.is_some() || state.active.is_some() {
            return Err(ProviderReservationError::Conflict(
                "a prior provider generation remains registered".to_owned(),
            ));
        }
        state.last_identity = state.last_identity.checked_add(1).ok_or_else(|| {
            ProviderReservationError::Conflict(
                "provider generation identity space is exhausted".to_owned(),
            )
        })?;
        let identity = ProviderGenerationIdentity(state.last_identity);
        state.last_join_timeout = Some(join_timeout);
        state.pending = Some(identity);
        drop(state);
        Ok(Self {
            registry: Arc::clone(registry),
            identity,
        })
    }

    #[cfg(test)]
    fn install(
        registry: &SharedCancellation,
        authority: Arc<dyn ChatSubscriptionCancellation>,
        receive_tracker: Arc<ReceiveOperationTracker>,
        join_timeout: Duration,
    ) -> std::result::Result<Self, String> {
        let mut registration =
            Self::reserve(registry, join_timeout).map_err(|error| format!("{error:?}"))?;
        if registration.activate(authority, receive_tracker, join_timeout)? {
            return Err("test installation raced a stopped provider registry".to_owned());
        }
        Ok(registration)
    }

    fn activate(
        &mut self,
        authority: Arc<dyn ChatSubscriptionCancellation>,
        receive_tracker: Arc<ReceiveOperationTracker>,
        join_timeout: Duration,
    ) -> std::result::Result<bool, String> {
        let mut state = self
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.pending != Some(self.identity) || state.active.is_some() {
            return Err("provider generation reservation changed before activation".to_owned());
        }
        state.last_join_timeout = Some(join_timeout);
        state.pending = None;
        state.active = Some(ActiveProviderCancellation {
            identity: self.identity,
            authority,
            receive_tracker,
            succeeded: false,
            cancelled_receive: None,
        });
        Ok(state.stopping)
    }

    fn identity(&self) -> ProviderGenerationIdentity {
        self.identity
    }
}

impl Drop for ProviderCancellationRegistration {
    fn drop(&mut self) {
        let mut state = self
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.identity == self.identity)
        {
            state.active = None;
        }
        if state.pending == Some(self.identity) {
            state.pending = None;
        }
    }
}

fn spawn_provider(
    state: BridgeState,
    stop: Arc<StopState>,
    cancellation: SharedCancellation,
    output_wake: SharedWake,
    overflowed: Arc<AtomicBool>,
    notices: mpsc::SyncSender<ProviderNotice>,
    selected_process_timeouts: ProcessPhaseTimeouts,
) -> Result<ServiceWorker, ChatServiceError> {
    spawn_provider_worker(
        stop,
        notices,
        output_wake,
        move |stop, notices, output_wake| {
            let log = |line: fmt::Arguments<'_>| service_log(line);
            run_provider_worker(
                || {
                    provider_generation(
                        &state,
                        stop,
                        &cancellation,
                        notices,
                        output_wake,
                        &overflowed,
                        selected_process_timeouts,
                        &log,
                    )
                },
                Instant::now,
                &log,
                |delay| stop.wait(delay),
                stop,
                notices,
                output_wake,
            );
        },
    )
    .map_err(ChatServiceError::Io)
}

/// Start the provider worker thread. It runs `body` and then ends as `end_provider_worker`
/// describes, whether `body` returned or panicked.
fn spawn_provider_worker(
    stop: Arc<StopState>,
    notices: mpsc::SyncSender<ProviderNotice>,
    output_wake: SharedWake,
    body: impl FnOnce(&StopState, &mpsc::SyncSender<ProviderNotice>, &SharedWake) + Send + 'static,
) -> io::Result<ServiceWorker> {
    let (done_sender, done) = mpsc::sync_channel(1);
    let handle = thread::Builder::new()
        .name("agentctl-chat-provider".to_owned())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                body(&stop, &notices, &output_wake);
            }));
            end_provider_worker(result, notices, &stop, &output_wake);
            let _ = done_sender.send(());
        })?;
    Ok(ServiceWorker { handle, done })
}

/// Run provider generations until the service stops or a generation ends the worker. After a
/// generation that may be retried, log why it ended and how long the worker waits, then wait.
/// `generation` runs one generation, `now` reads the clock, `log` writes one service log line,
/// and `wait` sleeps for the given time or until the service stops; production passes
/// `provider_generation`, `Instant::now`, `service_log` and `StopState::wait`, and a test can
/// script each of them.
fn run_provider_worker(
    mut generation: impl FnMut() -> Result<(), ProviderGenerationError>,
    now: impl Fn() -> Instant,
    log: &dyn Fn(fmt::Arguments<'_>),
    wait: impl Fn(Duration),
    stop: &StopState,
    notices: &mpsc::SyncSender<ProviderNotice>,
    output_wake: &SharedWake,
) {
    let mut backoff = ProviderBackoff::new();
    while !stop.is_stopped() {
        let started = now();
        let ended = match generation() {
            Ok(()) if stop.is_stopped() => break,
            Ok(()) => "stream ended".to_owned(),
            Err(ProviderGenerationError::Cancelled) if stop.is_stopped() => break,
            Err(ProviderGenerationError::Cancelled) => {
                stop.record_cleanup_error("provider reported cancellation without an owner stop");
                break;
            }
            Err(ProviderGenerationError::Cleanup(error)) => {
                stop.record_cleanup_error(format!("provider shutdown cleanup failed: {error}"));
                break;
            }
            Err(error) if stop.is_stopped() => {
                stop.record_cleanup_error(format!(
                    "provider failed before cancellation was observed: {error:?}"
                ));
                break;
            }
            Err(ProviderGenerationError::Retryable(error)) => error,
            Err(ProviderGenerationError::Fatal(error)) => {
                let _ = notices.send(ProviderNotice::Fatal(error));
                wake_output(output_wake);
                break;
            }
        };
        if stop.is_stopped() {
            break;
        }
        let delay = backoff.wait_after(now().saturating_duration_since(started));
        log(format_args!(
            "agentctl: chat provider: {ended}; reconnecting in {}s",
            delay.as_secs()
        ));
        wait(delay);
    }
}

/// Close the provider worker's notice channel and wake the owner loop, which reads the closed
/// channel as the end of the worker: unless the service is stopping, it stops with an error
/// instead of running on without chat events. A panic also stops the service, as the ACK
/// worker's does, and then unwinds on so that joining the worker reports it.
fn end_provider_worker(
    result: thread::Result<()>,
    notices: mpsc::SyncSender<ProviderNotice>,
    stop: &StopState,
    output_wake: &SharedWake,
) {
    let panic = result.err();
    if panic.is_some() {
        // Record the panic and stop before closing the channel, so an owner loop that finds the
        // channel closed also finds the service stopping, and `chat run` does not also report
        // that the worker stopped unexpectedly. Its error can still start with that of a Herdr
        // call the stop cancelled, when the owner loop was woken just before making one
        // (https://github.com/rrnewton/agent-utils/issues/186).
        stop.record_cleanup_error("chat provider worker panicked; no chat events arrive after it");
        stop.stop();
    }
    // Close the channel before waking the owner loop, so the loop finds it closed.
    drop(notices);
    wake_output(output_wake);
    if let Some(panic) = panic {
        std::panic::resume_unwind(panic);
    }
}

#[allow(clippy::too_many_arguments)]
fn provider_generation(
    state: &BridgeState,
    stop: &StopState,
    cancellation: &SharedCancellation,
    notices: &mpsc::SyncSender<ProviderNotice>,
    output_wake: &SharedWake,
    overflowed: &AtomicBool,
    selected_process_timeouts: ProcessPhaseTimeouts,
    log: &dyn Fn(fmt::Arguments<'_>),
) -> Result<(), ProviderGenerationError> {
    let inventory = crate::plugins::discover();
    let command = chat_runtime::select_plugin(&inventory, state.config())
        .map_err(|error| ProviderGenerationError::Retryable(error.to_string()))?;
    let timeouts = command.process_phase_timeouts();
    validate_selected_process_timeouts(selected_process_timeouts, timeouts)?;
    run_provider_generation(
        state,
        stop,
        cancellation,
        notices,
        output_wake,
        overflowed,
        timeouts,
        || {
            command
                .connect(timeouts)
                .map_err(|error| ProviderGenerationError::Retryable(error.to_string()))
        },
        log,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_provider_generation<B, C>(
    state: &BridgeState,
    stop: &StopState,
    cancellation: &SharedCancellation,
    notices: &mpsc::SyncSender<ProviderNotice>,
    output_wake: &SharedWake,
    overflowed: &AtomicBool,
    timeouts: ProcessPhaseTimeouts,
    connect: impl FnOnce() -> Result<(B, C), ProviderGenerationError>,
    log: &dyn Fn(fmt::Arguments<'_>),
) -> Result<(), ProviderGenerationError>
where
    B: ChatSubscriptionBackend,
    C: ChatSubscriptionCancellation + 'static,
{
    // Publish the complete pre-connection cleanup budget before Hello can block. There is no
    // cancellation authority until Hello succeeds, so a stop in that interval must retain both
    // the remaining Hello bound and the ordinary connected-generation cleanup bound.
    let mut registration = match ProviderCancellationRegistration::reserve(
        cancellation,
        pending_provider_join_timeout(timeouts),
    ) {
        Ok(registration) => registration,
        Err(ProviderReservationError::Stopping) => {
            return Err(ProviderGenerationError::Cancelled);
        }
        Err(ProviderReservationError::Conflict(error)) => {
            return Err(ProviderGenerationError::Cleanup(error));
        }
    };
    let (backend, process_cancellation) =
        connect_provider_unless_stopped(stop, pending_provider_join_timeout(timeouts), connect)?;
    let receive_tracker = Arc::new(ReceiveOperationTracker::default());
    let process_cancellation: Arc<dyn ChatSubscriptionCancellation> =
        Arc::new(process_cancellation);
    // Start occurs in ChatSubscription::open below. Install cancellation before entering it, so
    // Start is interruptible and does not add its independent phase timeout to the join budget.
    let stop_was_pending = registration
        .activate(
            Arc::clone(&process_cancellation),
            Arc::clone(&receive_tracker),
            connected_provider_join_timeout(timeouts),
        )
        .map_err(ProviderGenerationError::Cleanup)?;
    if stop_was_pending || stop.is_stopped() {
        // Stop admission and provider activation use the same registry lock. When stop won during
        // Hello, this sticky result cannot be lost between activation and Start. The potentially
        // bounded cancellation call runs after `activate` released that lock.
        cancel_provider_generation(
            cancellation,
            registration.identity(),
            &process_cancellation,
            &receive_tracker,
        )?;
        return Err(ProviderGenerationError::Cancelled);
    }
    let mut backend = ReceiveFailureTrackingBackend {
        inner: backend,
        receive_tracker: Arc::clone(&receive_tracker),
    };
    let request = state.subscribe_request().map_err(|error| match error {
        ChatRuntimeError::UnresolvedGap(detail) => ProviderGenerationError::Fatal(detail),
        other => ProviderGenerationError::Retryable(other.to_string()),
    })?;
    if stop.is_stopped() {
        cancel_provider_generation(
            cancellation,
            registration.identity(),
            &process_cancellation,
            &receive_tracker,
        )?;
        return Err(ProviderGenerationError::Cancelled);
    }
    let mut subscription = ChatSubscription::open(&mut backend, &request).map_err(|error| {
        classify_provider_start_failure(stop, cancellation, registration.identity(), error)
    })?;
    log(format_args!(
        "agentctl: chat provider: subscribed {}",
        if request.resume_from().is_some() {
            "from the saved cursor"
        } else {
            "without a saved cursor"
        }
    ));
    loop {
        if stop.is_stopped() {
            cancel_provider(cancellation)
                .map_err(|error| ProviderGenerationError::Cleanup(error.to_string()))?;
            return Err(ProviderGenerationError::Cancelled);
        }
        let consumed = match chat_runtime::consume_one(&mut subscription, state) {
            Ok(consumed) => consumed,
            Err(ChatRuntimeError::UnresolvedGap(detail)) => {
                cancel_provider(cancellation).map_err(|error| {
                    ProviderGenerationError::Fatal(format!(
                        "{detail}; provider cancellation cleanup failed: {error}"
                    ))
                })?;
                return Err(ProviderGenerationError::Fatal(detail));
            }
            Err(error) => {
                return Err(classify_provider_receive_failure(
                    stop,
                    cancellation,
                    registration.identity(),
                    &receive_tracker,
                    error,
                ))
            }
        };
        match consumed {
            chat_runtime::ConsumedItem::Heartbeat => {}
            chat_runtime::ConsumedItem::End => return Ok(()),
            chat_runtime::ConsumedItem::Batch(admission) => {
                if let Some(queue) = stop.ack_queue.as_ref() {
                    queue.enqueue(admission.new_request_keys.iter().cloned());
                }
                send_notice(
                    notices,
                    ProviderNotice::Batch(admission.new_request_keys),
                    output_wake,
                    overflowed,
                );
            }
        }
    }
}

fn classify_provider_start_failure(
    stop: &StopState,
    cancellation: &SharedCancellation,
    identity: ProviderGenerationIdentity,
    error: SubscriptionError,
) -> ProviderGenerationError {
    let cancellation_owns_start = cancellation
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .active
        .as_ref()
        .is_some_and(|active| {
            active.identity == identity && active.succeeded && active.cancelled_receive.is_none()
        });
    let cancellation_start_failure = matches!(
        &error,
        SubscriptionError::Backend(failure)
            if failure.code() == CANCELLED_PROCESS_START_CODE && failure.retryable()
    );
    if stop.is_stopped() && cancellation_owns_start && cancellation_start_failure {
        ProviderGenerationError::Cancelled
    } else {
        ProviderGenerationError::Retryable(error.to_string())
    }
}

fn classify_provider_receive_failure(
    stop: &StopState,
    cancellation: &SharedCancellation,
    identity: ProviderGenerationIdentity,
    receive_tracker: &ReceiveOperationTracker,
    error: ChatRuntimeError,
) -> ProviderGenerationError {
    let cancellation_state = cancellation
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .active
        .as_ref()
        .filter(|active| active.identity == identity)
        .map(|active| (active.succeeded, active.cancelled_receive));
    let cancellation_receive_failure = matches!(
        &error,
        ChatRuntimeError::Subscription(SubscriptionError::Backend(failure))
            if failure.code() == CANCELLED_PROCESS_RECEIVE_CODE && failure.retryable()
    );
    let failed_receive = receive_tracker.failed();
    let cancellation_owns_failure = matches!(
        cancellation_state,
        Some((true, Some(cancelled))) if Some(cancelled) == failed_receive
    ) || matches!(cancellation_state, Some((true, None)))
        && failed_receive.is_some();
    if stop.is_stopped() && cancellation_owns_failure && cancellation_receive_failure {
        ProviderGenerationError::Cancelled
    } else {
        ProviderGenerationError::Retryable(error.to_string())
    }
}

fn send_notice(
    notices: &mpsc::SyncSender<ProviderNotice>,
    notice: ProviderNotice,
    output_wake: &SharedWake,
    overflowed: &AtomicBool,
) {
    if notices.try_send(notice).is_err() {
        overflowed.store(true, Ordering::SeqCst);
    }
    wake_output(output_wake);
}

fn cancel_provider(cancellation: &SharedCancellation) -> Result<(), ChatServiceError> {
    let mut state = cancellation
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(active) = state.active.as_mut() else {
        return Ok(());
    };
    if active.succeeded {
        return Ok(());
    }
    let blocked_receive = active.receive_tracker.active();
    active.authority.cancel().map_err(|error| {
        ChatServiceError::Worker(format!("provider cancellation cleanup failed: {error}"))
    })?;
    active.succeeded = true;
    active.cancelled_receive = blocked_receive;
    Ok(())
}

fn cancel_provider_generation(
    cancellation: &SharedCancellation,
    identity: ProviderGenerationIdentity,
    authority: &Arc<dyn ChatSubscriptionCancellation>,
    receive_tracker: &ReceiveOperationTracker,
) -> Result<(), ProviderGenerationError> {
    let blocked_receive = receive_tracker.active();
    authority
        .cancel()
        .map_err(|error| ProviderGenerationError::Cleanup(error.to_string()))?;
    let mut state = cancellation
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let active = state
        .active
        .as_mut()
        .filter(|active| active.identity == identity)
        .ok_or_else(|| {
            ProviderGenerationError::Cleanup(
                "provider generation changed while cancellation completed".to_owned(),
            )
        })?;
    active.succeeded = true;
    active.cancelled_receive = blocked_receive;
    Ok(())
}

fn wake_output(output_wake: &SharedWake) {
    if let Some(wake) = output_wake
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        wake.wake();
    }
}

fn install_output_stream(
    stream: &mut Option<PaneEventStream>,
    output_wake: &SharedWake,
    connected: PaneEventStream,
) -> Result<(), ChatServiceError> {
    let wake = connected
        .wake_handle()
        .map_err(|error| ChatServiceError::Generation(error.to_string()))?;
    *stream = Some(connected);
    let mut published = output_wake
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *published = Some(wake);
    // The notice/signal producer may have observed the empty slot immediately before this
    // publication. Pre-arm the new stream while publication remains serialized: the socket byte
    // is sticky, so the first wait returns and the owner loop rechecks every queued source. The
    // loop also looks Herdr's status up after that wait. If that lookup fails, the loop tries it
    // again after a wait that ends within `SATURATED_POLL_INTERVAL`, or, if this subscription is
    // dropped before then, because that wait fails or the reply patterns change, after the first
    // wait on the next one.
    published
        .as_ref()
        .expect("the output wake was just installed")
        .wake();
    Ok(())
}

fn spawn_signal_worker(
    stop: Arc<StopState>,
    cancellation: SharedCancellation,
    output_wake: SharedWake,
    fallback_provider_join_timeout: Duration,
    outbound_join_timeout: Duration,
) -> Result<(SignalHandle, ServiceWorker), ChatServiceError> {
    let mut signals = Signals::new([SIGINT, SIGTERM])?;
    let handle = signals.handle();
    let (done_sender, done) = mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name("agentctl-chat-signals".to_owned())
        .spawn(move || {
            if signals.forever().next().is_some() {
                begin_service_stop(
                    &stop,
                    &cancellation,
                    fallback_provider_join_timeout,
                    outbound_join_timeout,
                );
                wake_output(&output_wake);
                if let Err(error) = cancel_provider(&cancellation) {
                    stop.record_cleanup_error(error.to_string());
                }
            }
            let _ = done_sender.send(());
        })?;
    Ok((
        handle,
        ServiceWorker {
            handle: worker,
            done,
        },
    ))
}

/// What the owner loop last learned about whether the agent is running a turn.
///
/// Herdr's status is one source, but a status rule can report a working Claude Code pane as idle,
/// so each screen the loop reads itself, or receives with an output event, is checked too, with
/// `submission::running_turn`; the reads a delivery makes to type a prompt are not. A read that
/// cannot tell, because Claude Code's paste hint has taken the place of its status row, leaves
/// the screen's evidence as it was. A pass that asks for a prompt or a notice to be typed, as
/// `CycleReport::prompt_typed` records, counts as the start of a turn until a later read of the
/// loop's own says otherwise. An output event's screen can show a running turn but not end one:
/// the loop handles the events that queued while it typed a prompt after the typing, and Herdr
/// may have read their screens before it. While either source says the agent works and some
/// request has a current reply ID, the loop reads the pane every `SATURATED_POLL_INTERVAL`.
#[derive(Debug, Default)]
struct TurnEvidence {
    // Herdr last reported the pane as `working`.
    herdr: bool,
    // The last read of the loop's own that could tell showed a running turn, or a later pass
    // asked for a prompt or a notice to be typed, or a later output event's screen showed a
    // running turn.
    screen: bool,
}

impl TurnEvidence {
    fn status(&mut self, status: &str) {
        self.herdr = status == "working";
    }

    /// A screen the loop read itself, after every prompt it had asked to be typed.
    fn read(&mut self, screen: &str) {
        if let Some(running) = crate::submission::running_turn(screen) {
            self.screen = running;
        }
    }

    /// The screen an output event carries, which Herdr may have read before a prompt the loop
    /// has since asked to be typed.
    fn event(&mut self, screen: &str) {
        self.screen |= crate::submission::running_turn(screen) == Some(true);
    }

    fn report(&mut self, report: &CycleReport) {
        self.screen |= report.prompt_typed;
    }

    fn working(&self) -> bool {
        self.herdr || self.screen
    }
}

#[allow(clippy::too_many_arguments)]
fn run_owner_loop<A: ManagedApi + ?Sized>(
    state: &BridgeState,
    client: &HerdrClient,
    manager: &ManagedAgents<'_, A>,
    options: ServiceOptions,
    stop: &StopState,
    cancellation: &SharedCancellation,
    output_wake: &SharedWake,
    overflowed: &AtomicBool,
    notices: &mpsc::Receiver<ProviderNotice>,
    transport: &mut Option<CommandOutboundTransport>,
) -> Result<(), ChatServiceError> {
    let owner_runtime = StopRuntime::new(stop);
    let mut routes = RouteCache::from_entries(state.reply_route_entries()?);
    let mut control = PassControl {
        transport,
        stop: Some(stop),
    };
    let mut direct_keys = DirectKeyQueue::default();
    let mut notes = NoteLog::default();
    let mut alias_problem = state.reply_alias_problem();
    if let Some(problem) = &alias_problem {
        service_log(format_args!("agentctl: {problem}"));
    }
    let initial = manager.read_capture_with_runtime(
        &state.config().agent_name,
        SNAPSHOT_LINES,
        &owner_runtime,
    )?;
    let initial_report = capture_recovery_snapshot(
        state,
        manager,
        options.delivery,
        &mut routes,
        SnapshotInput {
            text: &initial,
            truncated: false,
            revision: None,
        },
        &mut control,
    )?;
    enqueue_report_backlog(&mut direct_keys, &initial_report, overflowed);
    notes.log(&initial_report);
    // The loop looks Herdr's status up after each wait for a pane event, and the first wait on a
    // new subscription returns at once, as `install_output_stream` arms it, so the loop learns the
    // status as soon as its first pass subscribes, or, if that lookup fails, from the first of
    // its retries that succeeds, each made within `SATURATED_POLL_INTERVAL` of a failure for as
    // long as the subscription lasts. While no subscription works, the status comes from each
    // reconciliation.
    let mut turn = TurnEvidence::default();
    turn.read(&initial);
    turn.report(&initial_report);
    let recovery = recover_pass(state, manager, options.delivery, &mut routes, &mut control)?;
    enqueue_report_backlog(&mut direct_keys, &recovery, overflowed);
    notes.log(&recovery);
    turn.report(&recovery);
    let mut poll_at = next_saturated_poll(&routes, &initial);
    // Consecutive failed reads of the saturated poll, which logs the first and the recovery.
    let mut poll_failures = 0_u64;
    // Consecutive failed lookups of Herdr's status after a wait that ended with no event, which
    // logs the first and the recovery.
    let mut lookup_failures = 0_u64;
    // When the next wait ends to try a failed lookup again, while one has failed.
    let mut lookup_retry_at: Option<Instant> = None;

    let mut stream: Option<PaneEventStream> = None;
    let mut subscribed_patterns = Vec::new();
    let mut output_retry_at = Instant::now();
    let mut output_retry_delay = Duration::from_secs(1);
    let mut next_reconciliation = Instant::now() + options.reconciliation_interval;
    while !stop.is_stopped() {
        take_provider_notices(notices, stop, &mut direct_keys, overflowed)?;
        let recover = direct_keys.is_empty() && overflowed.swap(false, Ordering::SeqCst);
        if (recover || !direct_keys.is_empty()) && poll_failures == 0 {
            // A rescan may type a prompt, and so may a pass that handles queued request keys,
            // whether the chat provider reported them or an earlier pass left them for later. The
            // prompt's echo and the turn it starts can push a reply block of the turn before off
            // the screen before the pane is read again, and Herdr may not yet have reported that
            // the turn ended, so each such pass reads the pane first, unless a failed read is
            // waiting for its retry.
            poll_at = Some(Instant::now());
        }
        if poll_at.is_some_and(|at| Instant::now() >= at) {
            // This read runs every `SATURATED_POLL_INTERVAL` for as long as the pane shows the
            // closing line, or the agent works while some request has a current reply ID, and at
            // once before each rescan and each pass that handles queued request keys and after a
            // settle the pane has already left, so it saves no snapshot, which would cost a file
            // write and two fsyncs each time, and a failed read is tried again at the next
            // interval. Every other read of the pane still stops the service when it fails.
            match manager.peek_capture_with_runtime(
                &state.config().agent_name,
                SNAPSHOT_LINES,
                &owner_runtime,
            ) {
                Ok(snapshot) => {
                    if poll_failures > 0 {
                        service_log(format_args!(
                            "agentctl: the {}s read of the coordinator's pane works again after {poll_failures} failed reads",
                            SATURATED_POLL_INTERVAL.as_secs()
                        ));
                        poll_failures = 0;
                    }
                    turn.read(&snapshot);
                    let report = capture_visible_current(
                        state,
                        manager,
                        options.delivery,
                        &mut routes,
                        &snapshot,
                        &mut control,
                    )?;
                    enqueue_report_backlog(&mut direct_keys, &report, overflowed);
                    notes.log(&report);
                    turn.report(&report);
                    poll_at = next_saturated_poll(&routes, &snapshot);
                }
                Err(error) if stop.is_stopped() => return Err(error.into()),
                Err(error) => {
                    if poll_failures == 0 {
                        service_log(format_args!(
                            "agentctl: the {0}s read of the coordinator's pane failed; retrying every {0}s: {error}",
                            SATURATED_POLL_INTERVAL.as_secs()
                        ));
                    }
                    poll_failures = poll_failures.saturating_add(1);
                    poll_at = Some(Instant::now() + SATURATED_POLL_INTERVAL);
                }
            }
        }

        if recover {
            let report = recover_pass(state, manager, options.delivery, &mut routes, &mut control)?;
            enqueue_report_backlog(&mut direct_keys, &report, overflowed);
            notes.log(&report);
            turn.report(&report);
        }
        if !direct_keys.is_empty() {
            let report = drain_immediate_backlog(
                state,
                manager,
                options.delivery,
                &mut direct_keys,
                &mut control,
                overflowed,
            )?;
            notes.log(&report);
            turn.report(&report);
            for key in &report.processed_keys {
                routes.replace(key, state.next_reply_route(key)?);
            }
        }

        if Instant::now() >= next_reconciliation {
            let problem = state.reply_alias_problem();
            if problem != alias_problem {
                match &problem {
                    Some(problem) => service_log(format_args!("agentctl: {problem}")),
                    None => service_log("agentctl: the reply alias record is usable again"),
                }
                alias_problem = problem;
            }
            let info =
                manager.pane_info_with_runtime(&state.config().agent_name, &owner_runtime)?;
            turn.status(&info.status);
            if matches!(info.status.as_str(), "idle" | "done") {
                let snapshot = manager.read_capture_with_runtime(
                    &state.config().agent_name,
                    SNAPSHOT_LINES,
                    &owner_runtime,
                )?;
                turn.read(&snapshot);
                let report = capture_recovery_snapshot(
                    state,
                    manager,
                    options.delivery,
                    &mut routes,
                    SnapshotInput {
                        text: &snapshot,
                        truncated: false,
                        revision: None,
                    },
                    &mut control,
                )?;
                enqueue_report_backlog(&mut direct_keys, &report, overflowed);
                notes.log(&report);
                turn.report(&report);
                poll_at = next_saturated_poll(&routes, &snapshot);
            }
            let report = recover_pass(state, manager, options.delivery, &mut routes, &mut control)?;
            enqueue_report_backlog(&mut direct_keys, &report, overflowed);
            notes.log(&report);
            turn.report(&report);
            next_reconciliation = Instant::now() + options.reconciliation_interval;
        }

        let desired_patterns = if state.config().outbound_enabled {
            routes.patterns()?
        } else {
            Vec::new()
        };
        if desired_patterns != subscribed_patterns {
            stream = None;
            *output_wake
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            subscribed_patterns.clear();
            output_retry_at = Instant::now();
            output_retry_delay = Duration::from_secs(1);
        }
        if stream.is_none() && Instant::now() >= output_retry_at {
            match connect_output(
                client,
                manager,
                state,
                desired_patterns.clone(),
                Some(&owner_runtime),
            ) {
                Ok(connected) => {
                    install_output_stream(&mut stream, output_wake, connected)?;
                    subscribed_patterns = desired_patterns;
                    output_retry_delay = Duration::from_secs(1);
                }
                Err(error) => {
                    service_log(format_args!(
                        "agentctl: chat output subscription: {error}; retrying in {}s",
                        output_retry_delay.as_secs()
                    ));
                    output_retry_at = Instant::now() + output_retry_delay;
                    output_retry_delay = (output_retry_delay * 2).min(OUTPUT_RETRY_MAX);
                }
            }
        }

        // A turn can print a reply block and then push it off the screen with more output before
        // the turn ends, and the block's closing line raises no output event while an older
        // closing line keeps its pattern matched. So while the agent works, by Herdr's report or
        // by its screen, and some request has a current reply ID, the pane is read every
        // interval. A request stays open until `chat close`, so with outbound replies on, that is
        // in practice whenever the agent works.
        if turn.working() && poll_at.is_none() && routes.has_current() {
            poll_at = Some(Instant::now() + SATURATED_POLL_INTERVAL);
        }

        if let Some(active) = stream.as_mut() {
            let subscribed_pane = active.pane_id().to_owned();
            let timeout = if direct_keys.is_empty() {
                [poll_at, lookup_retry_at]
                    .into_iter()
                    .flatten()
                    .fold(next_reconciliation, Instant::min)
                    .saturating_duration_since(Instant::now())
            } else {
                Duration::ZERO
            };
            match active.wait(timeout) {
                Ok(events) => {
                    let info = match manager
                        .pane_info_with_runtime(&state.config().agent_name, &owner_runtime)
                    {
                        Ok(info) => {
                            lookup_retry_at = None;
                            if lookup_failures > 0 {
                                service_log(format_args!(
                                    "agentctl: the status lookup of the coordinator's pane works again after {lookup_failures} failed lookups"
                                ));
                                lookup_failures = 0;
                            }
                            info
                        }
                        // A wait that returned no event ended at a timer, as for the read due
                        // every `SATURATED_POLL_INTERVAL` while the agent works, or at a wake, as
                        // for a notice from the chat provider or the first wait on a new
                        // subscription. No event depends on this lookup, so a failed one does
                        // not stop the service. The loop sets a retry time that ends its next
                        // wait within `SATURATED_POLL_INTERVAL`, as a failed read of the saturated
                        // poll is tried again after that interval, and the lookup after that wait
                        // is the retry; if the subscription is dropped before then, because that
                        // wait fails or the reply patterns change, the lookup after the first wait
                        // on the next one is. Until a lookup after a wait succeeds, the loop goes
                        // by the status it last learned; it logs the first failure and that
                        // success. The retry matters most after the first wait on a subscription:
                        // Herdr raises no event for a status that has not changed, so that wait's
                        // lookup is the only one sure to follow the subscription promptly and show
                        // an agent already working when the loop subscribed. A failed lookup after
                        // an event, or at a reconciliation, still stops the service.
                        Err(error) if events.is_empty() && !stop.is_stopped() => {
                            if lookup_failures == 0 {
                                service_log(format_args!(
                                    "agentctl: the status lookup of the coordinator's pane failed; trying again within {}s: {error}",
                                    SATURATED_POLL_INTERVAL.as_secs()
                                ));
                            }
                            lookup_failures = lookup_failures.saturating_add(1);
                            lookup_retry_at = Some(Instant::now() + SATURATED_POLL_INTERVAL);
                            continue;
                        }
                        Err(error) => return Err(error.into()),
                    };
                    if info.pane_id != subscribed_pane {
                        return Err(ChatServiceError::Generation(format!(
                            "coordinator moved from subscribed pane {subscribed_pane:?} to {:?}",
                            info.pane_id
                        )));
                    }
                    turn.status(&info.status);
                    let mut unknown_route_seen = false;
                    for event in events {
                        match event {
                            PaneEvent::Output {
                                matched_line,
                                text,
                                truncated,
                                revision,
                            } => {
                                let identifier = matched_identifier(&matched_line).unwrap_or("");
                                turn.event(&text);
                                let report = capture_direct(
                                    state,
                                    manager,
                                    options.delivery,
                                    &mut routes,
                                    identifier,
                                    SnapshotInput {
                                        text: &text,
                                        truncated,
                                        revision,
                                    },
                                    &mut control,
                                )?;
                                unknown_route_seen |= report.recovery_requested;
                                enqueue_report_backlog(&mut direct_keys, &report, overflowed);
                                notes.log(&report);
                                turn.report(&report);
                                // This capture covers only the event's own reply ID, so an event
                                // never puts off a poll that is already due for the others.
                                poll_at = poll_at.or_else(|| next_saturated_poll(&routes, &text));
                            }
                            PaneEvent::Settled { status }
                                if info.status == status
                                    && matches!(status.as_str(), "idle" | "done") =>
                            {
                                let snapshot = manager.read_capture_with_runtime(
                                    &state.config().agent_name,
                                    SNAPSHOT_LINES,
                                    &owner_runtime,
                                )?;
                                turn.read(&snapshot);
                                let report = capture_recovery_snapshot(
                                    state,
                                    manager,
                                    options.delivery,
                                    &mut routes,
                                    SnapshotInput {
                                        text: &snapshot,
                                        truncated: false,
                                        revision: None,
                                    },
                                    &mut control,
                                )?;
                                enqueue_report_backlog(&mut direct_keys, &report, overflowed);
                                notes.log(&report);
                                turn.report(&report);
                                poll_at = next_saturated_poll(&routes, &snapshot);
                                let recovery = recover_pass(
                                    state,
                                    manager,
                                    options.delivery,
                                    &mut routes,
                                    &mut control,
                                )?;
                                enqueue_report_backlog(&mut direct_keys, &recovery, overflowed);
                                notes.log(&recovery);
                                turn.report(&recovery);
                            }
                            // The agent is no longer in the status Herdr reported, as when a
                            // prompt typed before this event was read has started another turn.
                            // The turn that ended may have left a reply block on the screen,
                            // which the new turn's output can push off, so the pane is read at
                            // once rather than at the next interval, unless a failed read is
                            // waiting for its retry.
                            PaneEvent::Settled { .. } => {
                                if poll_failures == 0 {
                                    poll_at = Some(Instant::now());
                                }
                            }
                            // `turn` took Herdr's status above, from a lookup made after this
                            // event arrived, so the event adds nothing to it.
                            PaneEvent::Working => {}
                        }
                    }
                    if unknown_route_seen {
                        let snapshot = manager.read_capture_with_runtime(
                            &state.config().agent_name,
                            SNAPSHOT_LINES,
                            &owner_runtime,
                        )?;
                        turn.read(&snapshot);
                        let report = capture_recovery_snapshot(
                            state,
                            manager,
                            options.delivery,
                            &mut routes,
                            SnapshotInput {
                                text: &snapshot,
                                truncated: false,
                                revision: None,
                            },
                            &mut control,
                        )?;
                        enqueue_report_backlog(&mut direct_keys, &report, overflowed);
                        notes.log(&report);
                        turn.report(&report);
                        poll_at = next_saturated_poll(&routes, &snapshot);
                    }
                }
                Err(error) => {
                    service_log(format_args!(
                        "agentctl: chat output subscription: {error}; retrying in {}s",
                        output_retry_delay.as_secs()
                    ));
                    stream = None;
                    *output_wake
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                    subscribed_patterns.clear();
                    output_retry_at = Instant::now() + output_retry_delay;
                    output_retry_delay = (output_retry_delay * 2).min(OUTPUT_RETRY_MAX);
                }
            }
        } else {
            let wait_until = next_reconciliation.min(output_retry_at.max(Instant::now()));
            let wait_until = poll_at.map_or(wait_until, |at| at.min(wait_until));
            let timeout = if direct_keys.is_empty() {
                wait_until
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(1))
            } else {
                Duration::ZERO
            };
            match notices.recv_timeout(timeout) {
                Ok(notice) => match notice {
                    ProviderNotice::Batch(keys) => {
                        enqueue_direct_keys(&mut direct_keys, keys, overflowed)
                    }
                    ProviderNotice::Fatal(error) => {
                        return Err(ChatServiceError::Generation(format!(
                            "chat provider stopped on an unrecoverable stream gap: {error}"
                        )));
                    }
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) if stop.is_stopped() => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(ChatServiceError::Worker(
                        "chat provider worker stopped unexpectedly".to_owned(),
                    ));
                }
            }
        }
    }
    cancel_provider(cancellation)?;
    Ok(())
}

#[derive(Debug, Default)]
struct DirectKeyQueue {
    keys: VecDeque<String>,
    members: BTreeSet<String>,
}

impl DirectKeyQueue {
    fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    fn len(&self) -> usize {
        self.keys.len()
    }

    fn take(&mut self, maximum: usize) -> Vec<String> {
        let mut result = Vec::with_capacity(maximum.min(self.keys.len()));
        for _ in 0..maximum.min(self.keys.len()) {
            let key = self
                .keys
                .pop_front()
                .expect("bounded queue length was checked");
            self.members.remove(&key);
            result.push(key);
        }
        result
    }
}

/// Move up to `MAX_PROVIDER_NOTICES_PER_PASS` waiting provider notices into `direct_keys`. A closed
/// channel means the provider worker has ended. Unless the service is stopping, that is an error:
/// no chat event would arrive again, while the bridge still looked healthy. The channel holds at
/// most `PROVIDER_NOTICE_CAPACITY` notices, one fewer than a pass reads, so a pass that starts
/// after the worker has closed it always finds it closed, even when the worker's last wake-up
/// came before the pass and no other one follows.
fn take_provider_notices(
    notices: &mpsc::Receiver<ProviderNotice>,
    stop: &StopState,
    direct_keys: &mut DirectKeyQueue,
    overflowed: &AtomicBool,
) -> Result<(), ChatServiceError> {
    for _ in 0..MAX_PROVIDER_NOTICES_PER_PASS {
        let notice = match notices.try_recv() {
            Ok(notice) => notice,
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) if stop.is_stopped() => break,
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(ChatServiceError::Worker(
                    "chat provider worker stopped unexpectedly".to_owned(),
                ));
            }
        };
        match notice {
            ProviderNotice::Batch(keys) => enqueue_direct_keys(direct_keys, keys, overflowed),
            ProviderNotice::Fatal(error) => {
                return Err(ChatServiceError::Generation(format!(
                    "chat provider stopped on an unrecoverable stream gap: {error}"
                )));
            }
        }
    }
    Ok(())
}

fn enqueue_direct_keys(queued: &mut DirectKeyQueue, keys: Vec<String>, overflowed: &AtomicBool) {
    let available = MAX_DIRECT_REQUEST_KEYS.saturating_sub(queued.len());
    let mut inserted = 0_usize;
    for key in keys {
        if queued.members.contains(&key) {
            continue;
        }
        if inserted == available {
            overflowed.store(true, Ordering::SeqCst);
            break;
        }
        queued.members.insert(key.clone());
        queued.keys.push_back(key);
        inserted = inserted.saturating_add(1);
    }
}

fn enqueue_report_backlog(
    queued: &mut DirectKeyQueue,
    report: &CycleReport,
    overflowed: &AtomicBool,
) {
    if report.more_work && !report.deferred_keys.is_empty() {
        enqueue_direct_keys(queued, report.deferred_keys.clone(), overflowed);
    }
}

fn drain_immediate_backlog<A: ManagedApi + ?Sized>(
    state: &BridgeState,
    manager: &ManagedAgents<'_, A>,
    delivery: DrainOptions,
    queued: &mut DirectKeyQueue,
    control: &mut PassControl<'_>,
    overflowed: &AtomicBool,
) -> Result<CycleReport, ChatServiceError> {
    match control.stop {
        Some(stop) => drain_immediate_backlog_with_delivery(
            state,
            &CancellableDelivery {
                manager,
                runtime: StopRuntime::new(stop),
            },
            delivery,
            queued,
            control,
            overflowed,
        ),
        None => drain_immediate_backlog_with_delivery(
            state, manager, delivery, queued, control, overflowed,
        ),
    }
}

fn drain_immediate_backlog_with_delivery(
    state: &BridgeState,
    coordinator: &dyn chat_runtime::CoordinatorDelivery,
    delivery: DrainOptions,
    queued: &mut DirectKeyQueue,
    control: &mut PassControl<'_>,
    overflowed: &AtomicBool,
) -> Result<CycleReport, ChatServiceError> {
    let mut combined = CycleReport::default();
    for _ in 0..MAX_IMMEDIATE_BACKLOG_CHUNKS {
        if queued.is_empty() || control.stopped() {
            break;
        }
        let keys = queued.take(MAX_KEYS_PER_PASS);
        let report = process_keys_with_delivery(state, coordinator, delivery, &keys, control)?;
        enqueue_report_backlog(queued, &report, overflowed);
        combined.merge(report);
    }
    if !queued.is_empty() {
        combined.more_work = true;
    }
    Ok(combined)
}

fn connect_output<A: ManagedApi + ?Sized>(
    client: &HerdrClient,
    manager: &ManagedAgents<'_, A>,
    state: &BridgeState,
    patterns: Vec<String>,
    runtime: Option<&dyn AgentRuntime>,
) -> Result<PaneEventStream, ChatServiceError> {
    let info = match runtime {
        Some(runtime) => manager.pane_info_with_runtime(&state.config().agent_name, runtime)?,
        None => manager.pane_info(&state.config().agent_name)?,
    };
    let socket = match runtime {
        Some(runtime) => client.event_socket_with_cancellation(&|| runtime.cancelled()),
        None => client.event_socket(),
    }
    .map_err(AgentError::from)
    .map_err(ChatServiceError::Agent)?;
    PaneEventStream::connect(
        &socket,
        &info.pane_id,
        patterns,
        SNAPSHOT_LINES_U32,
        EVENT_CONNECT_TIMEOUT,
    )
    .map_err(|error| ChatServiceError::Generation(error.to_string()))
}

/// Write one service log line to standard error, prefixed with the UTC time it is written. A
/// service manager often appends standard error to a file that records no times of its own.
pub(crate) fn service_log(line: impl fmt::Display) {
    write_service_log(&mut io::stderr().lock(), line);
}

/// Write one service log line to `out`, which the caller has locked, so the time is read after
/// the lock is taken and lines from several threads carry times in the order they are written.
/// The line goes out in one write call, so lines from two processes appending to one file do not
/// interleave unless a write is cut short, as on a full disk; on a pipe, only a line of at most
/// 4,096 bytes is kept whole. A failed write is ignored: `eprintln!` panics instead, and a panic
/// in any thread that writes these lines stops `chat run`, so a full disk or a closed log would
/// stop chat along with its log.
fn write_service_log(out: &mut impl io::Write, line: impl fmt::Display) {
    let line = format!("{} {line}\n", chat_runtime::log_timestamp());
    let _ = out.write_all(line.as_bytes());
}

/// Write each of a report's errors as one line through `log`.
fn log_report(report: &CycleReport, log: &dyn Fn(fmt::Arguments<'_>)) {
    if report.has_errors() {
        for error in &report.errors {
            log(format_args!("agentctl: chat operation: {error}"));
        }
    }
}

/// The notes this service process has logged. A refused block that stays on screen is read
/// again at every capture, so each distinct note is logged once; the oldest are forgotten first.
#[derive(Default)]
struct NoteLog {
    logged: BTreeSet<String>,
    order: VecDeque<String>,
}

impl NoteLog {
    /// Log a report's errors, and each of its notes that this process has not logged yet.
    fn log(&mut self, report: &CycleReport) {
        log_report(report, &|line| service_log(line));
        for note in &report.notes {
            if self.first_time(note) {
                service_log(format_args!("agentctl: chat reply capture: {note}"));
            }
        }
    }

    fn first_time(&mut self, note: &str) -> bool {
        if self.logged.contains(note) {
            return false;
        }
        if self.order.len() == MAX_LOGGED_NOTES {
            if let Some(oldest) = self.order.pop_front() {
                self.logged.remove(&oldest);
            }
        }
        self.logged.insert(note.to_owned());
        self.order.push_back(note.to_owned());
        true
    }
}

/// Name up to `ALREADY_REPORTED_LOG_IDS` already reported reply IDs and count the rest.
fn already_reported_log_line(suppressed_ids: &[String]) -> Option<String> {
    if suppressed_ids.is_empty() {
        return None;
    }
    let hidden = suppressed_ids
        .len()
        .saturating_sub(ALREADY_REPORTED_LOG_IDS);
    Some(format!(
        "agentctl: chat reply fence feedback: already reported, so not repeated: {}{}",
        suppressed_ids
            .iter()
            .take(ALREADY_REPORTED_LOG_IDS)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        if hidden > 0 {
            format!(" and {hidden} more")
        } else {
            String::new()
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::{BufRead, BufReader, Write as _};
    use std::num::NonZeroU16;
    #[cfg(target_os = "linux")]
    use std::os::fd::RawFd;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    #[cfg(target_os = "linux")]
    use std::os::unix::process::CommandExt as _;
    #[cfg(target_os = "linux")]
    use std::os::unix::process::ExitStatusExt;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    use std::sync::Condvar;

    use chat_subscription::{
        BackendCapabilities, BackendFailure, CancellationError, ChannelId, ChatSubscriptionBackend,
        ChatSubscriptionCancellation, ChatSubscriptionDriver, CommittableEvent, DeliveryBatch,
        DeliveryId, EventKind, EventSequence, InboundMessage, MessageId, ProviderCursor,
        ReplaySupport, SenderId, SubscribeRequest, SubscriptionItem, ThreadId,
    };

    static NEXT_STATE: AtomicU64 = AtomicU64::new(1);
    #[cfg(target_os = "linux")]
    const START_CANCEL_PLUGIN_ENV: &str = "AGENTCTL_TEST_START_CANCEL_PLUGIN";
    #[cfg(target_os = "linux")]
    const START_CANCEL_PROTOCOL_FD: RawFd = 3;

    fn state_namespace_snapshot(root: &Path) -> BTreeMap<std::path::PathBuf, (bool, u32, Vec<u8>)> {
        let mut snapshot = BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(path) = pending.pop() {
            let metadata = fs::symlink_metadata(&path).expect("state namespace metadata");
            let relative = path
                .strip_prefix(root)
                .expect("path below state root")
                .to_path_buf();
            if metadata.is_dir() {
                snapshot.insert(relative, (true, metadata.permissions().mode(), Vec::new()));
                let entries = fs::read_dir(&path)
                    .expect("state namespace directory")
                    .map(|entry| entry.expect("state namespace entry").path())
                    .collect::<Vec<_>>();
                pending.extend(entries);
            } else if metadata.is_file() {
                snapshot.insert(
                    relative,
                    (
                        false,
                        metadata.permissions().mode(),
                        fs::read(&path).expect("state namespace file"),
                    ),
                );
            } else {
                panic!("unexpected non-file state path {}", path.display());
            }
        }
        snapshot
    }

    #[derive(Default)]
    struct RecordingDelivery {
        states: Mutex<BTreeMap<String, QueueMessageState>>,
        prompts: Mutex<Vec<String>>,
    }

    struct AckReleasingDelivery {
        states: Mutex<BTreeMap<String, QueueMessageState>>,
        prompts: Mutex<Vec<String>>,
        helper_entered: std::path::PathBuf,
        helper_release: std::path::PathBuf,
        submit_started: AtomicBool,
        submit_thread: Mutex<Option<thread::ThreadId>>,
    }

    struct BlockingFailureBackend {
        gate: Arc<(Mutex<bool>, Condvar)>,
        entered: Arc<(Mutex<bool>, Condvar)>,
        failure_code: &'static str,
    }

    struct BlockingStartBackend {
        gate: Arc<(Mutex<bool>, Condvar)>,
        entered: Arc<(Mutex<bool>, Condvar)>,
    }

    struct GatedProcessStartBackend<B> {
        inner: B,
        gate: Arc<(Mutex<bool>, Condvar)>,
        entered: Arc<(Mutex<bool>, Condvar)>,
    }

    struct BlockingFailureDriver {
        gate: Arc<(Mutex<bool>, Condvar)>,
        entered: Arc<(Mutex<bool>, Condvar)>,
        failure_code: &'static str,
    }

    #[derive(Clone)]
    struct GateCancellation {
        gate: Arc<(Mutex<bool>, Condvar)>,
    }

    struct RegistryCheckingCancellation {
        registry: std::sync::Weak<Mutex<ProviderCancellationRegistry>>,
        gate: Arc<(Mutex<bool>, Condvar)>,
    }

    impl ChatSubscriptionCancellation for GateCancellation {
        fn cancel(&self) -> std::result::Result<(), CancellationError> {
            let (gate_lock, gate_changed) = &*self.gate;
            *gate_lock.lock().expect("gate lock") = true;
            gate_changed.notify_all();
            Ok(())
        }
    }

    impl ChatSubscriptionCancellation for RegistryCheckingCancellation {
        fn cancel(&self) -> std::result::Result<(), CancellationError> {
            let registry = self.registry.upgrade().expect("live provider registry");
            assert!(
                registry.try_lock().is_ok(),
                "bounded provider cancellation ran while holding the registry lock"
            );
            let (gate_lock, gate_changed) = &*self.gate;
            *gate_lock.lock().expect("gate lock") = true;
            gate_changed.notify_all();
            Ok(())
        }
    }

    impl ChatSubscriptionDriver for BlockingFailureDriver {
        fn cancellation(&self) -> Arc<dyn ChatSubscriptionCancellation> {
            Arc::new(GateCancellation {
                gate: Arc::clone(&self.gate),
            })
        }

        fn next_item(&mut self) -> std::result::Result<Option<SubscriptionItem>, BackendFailure> {
            let (entered_lock, entered_changed) = &*self.entered;
            *entered_lock.lock().expect("entered lock") = true;
            entered_changed.notify_all();
            let (gate_lock, gate_changed) = &*self.gate;
            let mut released = gate_lock.lock().expect("gate lock");
            while !*released {
                released = gate_changed.wait(released).expect("gate wait");
            }
            Err(BackendFailure::new(
                self.failure_code,
                "blocked provider read completed with a failure",
                true,
            )
            .expect("backend failure"))
        }

        fn acknowledge(
            &mut self,
            _delivery_id: &DeliveryId,
        ) -> std::result::Result<(), BackendFailure> {
            Ok(())
        }
    }

    impl ChatSubscriptionBackend for BlockingFailureBackend {
        fn cancellation(&self) -> Arc<dyn ChatSubscriptionCancellation> {
            Arc::new(GateCancellation {
                gate: Arc::clone(&self.gate),
            })
        }

        fn capabilities(&self) -> BackendCapabilities {
            BackendCapabilities::new(
                "blocking-fixture",
                ReplaySupport::Cursor,
                true,
                NonZeroU16::new(1).expect("one"),
                vec![
                    EventKind::MessageCreated,
                    EventKind::Checkpoint,
                    EventKind::Gap,
                    EventKind::Heartbeat,
                ],
            )
            .expect("capabilities")
        }

        fn subscribe(
            &mut self,
            _request: &SubscribeRequest,
        ) -> std::result::Result<Box<dyn ChatSubscriptionDriver>, BackendFailure> {
            Ok(Box::new(BlockingFailureDriver {
                gate: Arc::clone(&self.gate),
                entered: Arc::clone(&self.entered),
                failure_code: self.failure_code,
            }))
        }
    }

    impl ChatSubscriptionBackend for BlockingStartBackend {
        fn cancellation(&self) -> Arc<dyn ChatSubscriptionCancellation> {
            Arc::new(GateCancellation {
                gate: Arc::clone(&self.gate),
            })
        }

        fn capabilities(&self) -> BackendCapabilities {
            BackendCapabilities::new(
                "blocking-start-fixture",
                ReplaySupport::Cursor,
                true,
                NonZeroU16::new(1).expect("one"),
                vec![
                    EventKind::MessageCreated,
                    EventKind::Checkpoint,
                    EventKind::Gap,
                    EventKind::Heartbeat,
                ],
            )
            .expect("capabilities")
        }

        fn subscribe(
            &mut self,
            _request: &SubscribeRequest,
        ) -> std::result::Result<Box<dyn ChatSubscriptionDriver>, BackendFailure> {
            let (entered_lock, entered_changed) = &*self.entered;
            *entered_lock.lock().expect("entered lock") = true;
            entered_changed.notify_all();
            let (gate_lock, gate_changed) = &*self.gate;
            let mut released = gate_lock.lock().expect("gate lock");
            while !*released {
                released = gate_changed.wait(released).expect("gate wait");
            }
            Err(BackendFailure::new(
                "plugin_start_cancelled",
                "blocked Start was interrupted by host cancellation",
                true,
            )
            .expect("backend failure"))
        }
    }

    impl<B: ChatSubscriptionBackend> ChatSubscriptionBackend for GatedProcessStartBackend<B> {
        fn cancellation(&self) -> Arc<dyn ChatSubscriptionCancellation> {
            self.inner.cancellation()
        }

        fn capabilities(&self) -> BackendCapabilities {
            let (entered_lock, entered_changed) = &*self.entered;
            *entered_lock.lock().expect("Start admission lock") = true;
            entered_changed.notify_all();
            let (gate_lock, gate_changed) = &*self.gate;
            let mut released = gate_lock.lock().expect("Start gate lock");
            while !*released {
                released = gate_changed.wait(released).expect("Start gate wait");
            }
            self.inner.capabilities()
        }

        fn subscribe(
            &mut self,
            request: &SubscribeRequest,
        ) -> std::result::Result<Box<dyn ChatSubscriptionDriver>, BackendFailure> {
            self.inner.subscribe(request)
        }
    }

    impl chat_runtime::CoordinatorDelivery for RecordingDelivery {
        fn message_state(
            &self,
            _agent_name: &str,
            message_id: &str,
        ) -> std::result::Result<Option<QueueMessageState>, String> {
            Ok(self.states.lock().expect("states").get(message_id).copied())
        }

        fn submit(
            &self,
            _agent_name: &str,
            prompt: &str,
            message_id: &str,
            _options: DrainOptions,
        ) -> std::result::Result<(), String> {
            self.prompts
                .lock()
                .expect("prompts")
                .push(prompt.to_owned());
            self.states
                .lock()
                .expect("states")
                .insert(message_id.to_owned(), QueueMessageState::Processed);
            Ok(())
        }

        fn drain(
            &self,
            _agent_name: &str,
            _options: DrainOptions,
        ) -> std::result::Result<(), String> {
            Ok(())
        }

        fn screen(&self, _agent_name: &str) -> std::result::Result<String, String> {
            Ok(String::new())
        }
    }

    impl chat_runtime::CoordinatorDelivery for AckReleasingDelivery {
        fn message_state(
            &self,
            _agent_name: &str,
            message_id: &str,
        ) -> std::result::Result<Option<QueueMessageState>, String> {
            let observed = self.states.lock().expect("states").get(message_id).copied();
            if observed.is_none() {
                let deadline = Instant::now() + Duration::from_secs(1);
                while !self.helper_entered.exists() {
                    if Instant::now() >= deadline {
                        return Err(
                            "acknowledgement helper did not enter before coordinator delivery"
                                .to_owned(),
                        );
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }
            Ok(observed)
        }

        fn submit(
            &self,
            _agent_name: &str,
            prompt: &str,
            message_id: &str,
            _options: DrainOptions,
        ) -> std::result::Result<(), String> {
            if !self.helper_entered.exists() {
                return Err(
                    "coordinator submit began before acknowledgement helper entered".to_owned(),
                );
            }
            self.submit_started.store(true, Ordering::SeqCst);
            *self.submit_thread.lock().expect("submit thread") = Some(thread::current().id());
            fs::write(&self.helper_release, b"release")
                .map_err(|error| format!("release acknowledgement helper: {error}"))?;
            self.prompts
                .lock()
                .expect("prompts")
                .push(prompt.to_owned());
            self.states
                .lock()
                .expect("states")
                .insert(message_id.to_owned(), QueueMessageState::Processed);
            Ok(())
        }

        fn drain(
            &self,
            _agent_name: &str,
            _options: DrainOptions,
        ) -> std::result::Result<(), String> {
            Ok(())
        }

        fn screen(&self, _agent_name: &str) -> std::result::Result<String, String> {
            Ok(String::new())
        }
    }

    fn state_with_request() -> (BridgeState, String, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "agentctl-chat-service-{}-{}",
            std::process::id(),
            NEXT_STATE.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        fs::create_dir(&root).expect("create fixture root");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("private fixture");
        let state = BridgeState::initialize(
            &root,
            BridgeConfiguration {
                subscription_plugin: "fixture".to_owned(),
                subscription_environment: Vec::new(),
                channel_ids: vec!["spaces/example".to_owned()],
                allowed_senders: vec!["users/owner".to_owned()],
                agent_name: "coordinator".to_owned(),
                agent_label: "coordinator".to_owned(),
                outbound_enabled: true,
                ack_reaction: Some("🤖".to_owned()),
                backend_configuration: None,
                outbound_command: None,
            },
        )
        .expect("initialize state");
        let message = InboundMessage::new(
            ChannelId::new("spaces/example").expect("channel"),
            MessageId::new("spaces/example/messages/one").expect("message"),
            ThreadId::new("spaces/example/threads/one").expect("thread"),
            SenderId::new("users/owner").expect("sender"),
            "request",
            "2026-09-21T12:00:00Z",
            false,
        )
        .expect("inbound message");
        let batch = DeliveryBatch::new(
            EventSequence::new(1).expect("sequence"),
            ProviderCursor::new("cursor").expect("cursor"),
            DeliveryId::new("delivery").expect("delivery"),
            vec![CommittableEvent::message_created(message)],
        )
        .expect("delivery batch");
        let key = state
            .admit_batch(&batch)
            .expect("admit request")
            .new_request_keys
            .remove(0);
        (state, key, root)
    }

    fn admit_more_requests(state: &BridgeState, count: usize) -> Vec<String> {
        let events = (0..count)
            .map(|index| {
                let message = InboundMessage::new(
                    ChannelId::new("spaces/example").expect("channel"),
                    MessageId::new(format!("spaces/example/messages/more-{index}"))
                        .expect("message"),
                    ThreadId::new("spaces/example/threads/one").expect("thread"),
                    SenderId::new("users/owner").expect("sender"),
                    "another request",
                    "2026-09-21T12:00:01Z",
                    false,
                )
                .expect("message");
                CommittableEvent::message_created(message)
            })
            .collect();
        let batch = DeliveryBatch::new(
            EventSequence::new(2).expect("sequence"),
            ProviderCursor::new("cursor-more").expect("cursor"),
            DeliveryId::new("delivery-more").expect("delivery"),
            events,
        )
        .expect("batch");
        let admission = state.admit_batch(&batch).expect("admit more requests");
        admission.new_request_keys
    }

    struct ObservedReactionTransport {
        started: mpsc::Sender<(String, String)>,
        release: Option<mpsc::Receiver<bool>>,
    }

    impl chat_runtime::ReactionTransport for ObservedReactionTransport {
        fn ensure_reaction(
            &mut self,
            submission: chat_runtime::ReactionSubmission<'_>,
        ) -> Result<chat_runtime::ReactionReceipt, OutboundFailure> {
            self.started
                .send((
                    submission.message_id.to_owned(),
                    submission.request_id.to_owned(),
                ))
                .expect("observe ACK admission");
            if self
                .release
                .as_ref()
                .is_some_and(|release| release.recv_timeout(Duration::from_secs(5)) != Ok(true))
            {
                return Err(OutboundFailure {
                    code: "fixture_unknown".to_owned(),
                    detail: "provider result is uncertain".to_owned(),
                    outcome: chat_runtime::OutboundOutcome::Unknown,
                    retryable: true,
                });
            }
            Ok(chat_runtime::ReactionReceipt {
                reaction_id: format!("{}/reactions/ack", submission.message_id),
                already_present: false,
            })
        }
    }

    #[test]
    fn async_ack_does_not_block_later_intake_or_delivery_and_shutdown_owns_inflight() {
        let (state, key, root) = state_with_request();
        let queue = Arc::new(AckQueue::default());
        let stop = Arc::new(StopState {
            ack_queue: Some(Arc::clone(&queue)),
            ..StopState::default()
        });
        let (started, starts) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let worker = spawn_ack_worker(
            state.clone(),
            Arc::clone(&queue),
            ObservedReactionTransport {
                started,
                release: Some(released),
            },
            Arc::clone(&stop),
            Arc::new(Mutex::new(None)),
            captured_service_log,
        )
        .expect("spawn ACK worker");
        queue.enqueue([key.clone()]);
        let first = starts
            .recv_timeout(Duration::from_secs(1))
            .expect("first ACK entered");
        queue.enqueue([key.clone(), key.clone()]);

        // A second durable intake and both pane deliveries complete while the first ACK is held.
        let mut keys = vec![key.clone()];
        keys.extend(admit_more_requests(&state, 1));
        let delivery = RecordingDelivery::default();
        let mut transport = None;
        let report = process_keys_with_delivery(
            &state,
            &delivery,
            DrainOptions::default(),
            &keys,
            &mut PassControl {
                transport: &mut transport,
                stop: Some(&stop),
            },
        )
        .expect("delivery must not wait for ACK");
        assert_eq!(report.delivered, keys);
        assert_eq!(delivery.prompts.lock().expect("prompts").len(), 2);
        assert!(
            starts.try_recv().is_err(),
            "one in-flight ACK owns the deduplicated key"
        );

        stop.stop();
        let (joined, join_result) = mpsc::channel();
        let joiner = thread::spawn(move || {
            let result = join_ack_worker_until(
                worker,
                &OutboundCancellation::new().expect("cancellation"),
                Instant::now() + Duration::from_secs(10),
            );
            joined.send(result).expect("report join");
        });
        assert!(
            matches!(
                join_result.recv_timeout(Duration::from_millis(20)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "shutdown must retain its admitted ACK"
        );
        release.send(true).expect("finish admitted ACK");
        join_result
            .recv_timeout(Duration::from_secs(1))
            .expect("bounded shutdown")
            .expect("join ACK");
        joiner.join().expect("join shutdown owner");
        assert_eq!(first.0, "spaces/example/messages/one");
        assert!(
            starts.try_recv().is_err(),
            "stop must not launch queued ACKs"
        );
        assert_eq!(
            state.inspect_request(&key).expect("first receipt")["acknowledgement"]["phase"],
            "acked"
        );
        assert_eq!(
            state.inspect_request(&keys[1]).expect("queued durable ACK")["acknowledgement"]
                ["phase"],
            "pending"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn async_ack_unknown_outcome_does_not_hot_retry_and_restart_keeps_operation_id() {
        let (state, key, root) = state_with_request();
        let queue = Arc::new(AckQueue::default());
        let stop = Arc::new(StopState {
            ack_queue: Some(Arc::clone(&queue)),
            ..StopState::default()
        });
        let (started, starts) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let worker = spawn_ack_worker(
            state.clone(),
            Arc::clone(&queue),
            ObservedReactionTransport {
                started,
                release: Some(released),
            },
            Arc::clone(&stop),
            Arc::new(Mutex::new(None)),
            captured_service_log,
        )
        .expect("spawn ACK worker");
        queue.enqueue([key.clone()]);
        let first = starts
            .recv_timeout(Duration::from_secs(1))
            .expect("first attempt");
        release.send(false).expect("unknown outcome");
        let (guard, timeout) = queue
            .changed
            .wait_timeout_while(
                queue.state.lock().expect("queue state"),
                Duration::from_secs(1),
                |state| state.retained.contains(&key),
            )
            .expect("await persisted unknown outcome");
        assert!(!timeout.timed_out());
        drop(guard);
        queue.enqueue([key.clone(), key.clone()]);
        assert!(matches!(
            starts.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        let inspection = state.inspect_request(&key).expect("inspect failed ACK");
        assert_eq!(inspection["acknowledgement"]["phase"], "sending");
        assert!(inspection["acknowledgement"]["error"]
            .as_str()
            .expect("error")
            .contains("Unknown"));
        assert!(inspection["timestamps"]["ack_completed_at_millis"].is_null());
        stop.stop();
        join_worker_until(worker, "ACK", Instant::now() + Duration::from_secs(1))
            .expect("join failed generation");

        let reopened = BridgeState::open(&root).expect("restart durable state");
        let queue = Arc::new(AckQueue::default());
        queue.enqueue(reopened.pending_ack_keys().expect("recover ACKs"));
        let stop = Arc::new(StopState {
            ack_queue: Some(Arc::clone(&queue)),
            ..StopState::default()
        });
        let (started, starts) = mpsc::channel();
        let worker = spawn_ack_worker(
            reopened.clone(),
            queue,
            ObservedReactionTransport {
                started,
                release: None,
            },
            Arc::clone(&stop),
            Arc::new(Mutex::new(None)),
            captured_service_log,
        )
        .expect("restart ACK worker");
        let retry = starts
            .recv_timeout(Duration::from_secs(1))
            .expect("reconciliation attempt");
        assert_eq!(retry, first);
        stop.stop();
        join_worker_until(worker, "ACK", Instant::now() + Duration::from_secs(1))
            .expect("join restart");
        assert_eq!(
            reopened.inspect_request(&key).expect("receipt")["acknowledgement"]["phase"],
            "acked"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn async_ack_queue_overflow_recovers_durable_requests_without_blocking_intake() {
        let (state, key, root) = state_with_request();
        let mut keys = vec![key];
        keys.extend(admit_more_requests(&state, ACK_QUEUE_CAPACITY));
        let queue = Arc::new(AckQueue::default());
        queue.enqueue(keys.clone());
        assert_eq!(
            queue.state.lock().expect("queue").retained.len(),
            ACK_QUEUE_CAPACITY
        );
        let stop = Arc::new(StopState {
            ack_queue: Some(Arc::clone(&queue)),
            ..StopState::default()
        });
        let (started, starts) = mpsc::channel();
        let worker = spawn_ack_worker(
            state.clone(),
            queue,
            ObservedReactionTransport {
                started,
                release: None,
            },
            Arc::clone(&stop),
            Arc::new(Mutex::new(None)),
            captured_service_log,
        )
        .expect("spawn ACK worker");
        let mut observed = BTreeSet::new();
        for _ in &keys {
            observed.insert(
                starts
                    .recv_timeout(Duration::from_secs(2))
                    .expect("recovered ACK")
                    .0,
            );
        }
        stop.stop();
        join_worker_until(worker, "ACK", Instant::now() + Duration::from_secs(1))
            .expect("join ACK worker");
        assert_eq!(observed.len(), keys.len());
        assert!(state.pending_ack_keys().expect("remaining ACKs").is_empty());
        fs::remove_dir_all(root).expect("cleanup");
    }

    fn connected_event_stream(
        pattern: &str,
    ) -> (
        PaneEventStream,
        mpsc::Sender<()>,
        thread::JoinHandle<()>,
        std::path::PathBuf,
    ) {
        let root = std::env::temp_dir().join(format!(
            "agentctl-chat-service-events-{}-{}",
            std::process::id(),
            NEXT_STATE.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        fs::create_dir(&root).expect("create event fixture root");
        let socket = root.join("events.sock");
        let listener = UnixListener::bind(&socket).expect("bind event fixture");
        let (release, released) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().expect("accept event client");
            let mut request = String::new();
            BufReader::new(connection.try_clone().expect("clone event connection"))
                .read_line(&mut request)
                .expect("read event subscription");
            let request: Value = serde_json::from_str(&request).expect("decode event subscription");
            let mut acknowledgement = serde_json::to_vec(&json!({
                "id": request["id"],
                "result": {"type": "subscription_started"},
            }))
            .expect("encode event acknowledgement");
            acknowledgement.push(b'\n');
            connection
                .write_all(&acknowledgement)
                .expect("write event acknowledgement");
            released.recv().expect("release event fixture");
        });
        let stream = PaneEventStream::connect(
            &socket,
            "workspace:pane",
            vec![pattern.to_owned()],
            SNAPSHOT_LINES_U32,
            Duration::from_secs(2),
        )
        .expect("connect event fixture");
        (stream, release, server, root)
    }

    #[cfg(target_os = "linux")]
    fn read_process_protocol_frame(reader: &mut impl io::Read) -> Value {
        let mut header = [0_u8; 4];
        reader
            .read_exact(&mut header)
            .expect("read protocol header");
        let length = u32::from_be_bytes(header) as usize;
        assert!(
            (1..=chat_subscription_plugin::MAX_FRAME_BYTES).contains(&length),
            "valid protocol frame length"
        );
        let mut payload = vec![0_u8; length];
        reader
            .read_exact(&mut payload)
            .expect("read protocol payload");
        serde_json::from_slice(&payload).expect("decode protocol frame")
    }

    #[cfg(target_os = "linux")]
    fn write_process_protocol_frame(writer: &mut impl io::Write, value: &Value) {
        let payload = serde_json::to_vec(value).expect("encode protocol frame");
        writer
            .write_all(
                &u32::try_from(payload.len())
                    .expect("protocol frame fits u32")
                    .to_be_bytes(),
            )
            .expect("write protocol header");
        writer.write_all(&payload).expect("write protocol payload");
        writer.flush().expect("flush protocol frame");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn process_plugin_pre_start_close_fixture() {
        if std::env::var_os(START_CANCEL_PLUGIN_ENV).is_none() {
            return;
        }
        let input = io::stdin();
        let mut reader = input.lock();
        let mut writer = fs::OpenOptions::new()
            .write(true)
            .open(format!("/proc/self/fd/{START_CANCEL_PROTOCOL_FD}"))
            .expect("open isolated protocol output");
        assert_eq!(read_process_protocol_frame(&mut reader)["type"], "hello");
        write_process_protocol_frame(
            &mut writer,
            &json!({
                "type": "hello",
                "version": 1,
                "capabilities": {
                    "backend_name": "process-start-cancel-fixture",
                    "replay": "cursor",
                    "full_message_data": true,
                    "max_uncommitted": 1,
                    "event_kinds": ["message_created", "checkpoint", "gap", "heartbeat"],
                },
            }),
        );
        assert_eq!(
            read_process_protocol_frame(&mut reader)["type"],
            "close",
            "graceful cancellation must retire the command port before Start"
        );
    }

    #[cfg(target_os = "linux")]
    fn process_start_cancel_command() -> std::process::Command {
        let mut command = std::process::Command::new(
            std::env::current_exe().expect("resolve current test executable"),
        );
        command
            .arg("--exact")
            .arg("chat_service::tests::process_plugin_pre_start_close_fixture")
            .env(START_CANCEL_PLUGIN_ENV, "1");
        // The libtest harness writes status text to stdout. Preserve the already-configured
        // protocol pipe as descriptor 3, then hide harness text so only fixture frames reach the
        // process host.
        unsafe {
            command.pre_exec(|| {
                if libc::dup2(libc::STDOUT_FILENO, START_CANCEL_PROTOCOL_FD) < 0 {
                    return Err(io::Error::last_os_error());
                }
                let null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
                if null < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::dup2(null, libc::STDOUT_FILENO) < 0 {
                    let error = io::Error::last_os_error();
                    libc::close(null);
                    return Err(error);
                }
                libc::close(null);
                Ok(())
            });
        }
        command
    }

    #[test]
    fn every_output_stream_install_wakes_notices_sent_before_publication() {
        let output_wake: SharedWake = Arc::new(Mutex::new(None));
        let overflowed = AtomicBool::new(false);
        let (notice_sender, notice_receiver) = mpsc::sync_channel(PROVIDER_NOTICE_CAPACITY);
        let mut stream = None;

        for (phase, pattern) in [
            ("startup", "^startup$"),
            ("reconnect", "^startup$"),
            ("pattern-change", "^changed$"),
        ] {
            assert!(stream.is_none(), "prior output stream was retired");
            *output_wake.lock().expect("output wake lock") = None;
            send_notice(
                &notice_sender,
                ProviderNotice::Batch(vec![phase.to_owned()]),
                &output_wake,
                &overflowed,
            );
            assert!(
                output_wake.lock().expect("output wake lock").is_none(),
                "{phase} notice must precede wake publication"
            );

            let (connected, release, server, root) = connected_event_stream(pattern);
            install_output_stream(&mut stream, &output_wake, connected)
                .expect("install production output stream");
            let started = Instant::now();
            assert!(stream
                .as_mut()
                .expect("installed stream")
                .wait(Duration::from_secs(30))
                .expect("pre-armed production wait")
                .is_empty());
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "{phase} install slept despite its pre-publication notice: {:?}",
                started.elapsed()
            );
            match notice_receiver.try_recv().expect("queued provider notice") {
                ProviderNotice::Batch(observed) => assert_eq!(observed, [phase]),
                other => panic!("unexpected notice after {phase}: {other:?}"),
            }
            assert!(!overflowed.load(Ordering::SeqCst));

            drop(stream.take());
            *output_wake.lock().expect("output wake lock") = None;
            release.send(()).expect("release event fixture");
            server.join().expect("join event fixture");
            fs::remove_dir_all(root).expect("remove event fixture");
        }
    }

    #[cfg(target_os = "linux")]
    fn pidfd_inode(process_id: u32) -> u64 {
        // SAFETY: pidfd_open accepts scalar arguments and returns a fresh descriptor on success.
        let descriptor = unsafe {
            libc::syscall(
                libc::SYS_pidfd_open,
                libc::pid_t::try_from(process_id).expect("fixture pid fits pid_t"),
                0_u32,
            )
        };
        assert!(
            descriptor >= 0,
            "open fixture pidfd: {}",
            io::Error::last_os_error()
        );
        // SAFETY: successful pidfd_open returned a fresh descriptor owned by this helper.
        let descriptor = unsafe { OwnedFd::from_raw_fd(i32::try_from(descriptor).unwrap()) };
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::zeroed();
        // SAFETY: descriptor is live and metadata is writable.
        assert_eq!(
            unsafe { libc::fstat(descriptor.as_raw_fd(), metadata.as_mut_ptr()) },
            0
        );
        // SAFETY: successful fstat initialized metadata.
        unsafe { metadata.assume_init() }.st_ino
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn graceful_stop_helper_binds_pidfd_inode_signals_exact_main_and_waits_for_exit() {
        let mut target = std::process::Command::new("/bin/sleep")
            .arg("60")
            .spawn()
            .expect("spawn exact graceful-stop target");
        let process_id = target.id();
        let inode = pidfd_inode(process_id);
        graceful_stop_main(process_id, inode).expect("signal and wait exact pidfd target");
        let status = target.wait().expect("reap graceful-stop target");
        assert_eq!(status.signal(), Some(libc::SIGTERM));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn graceful_stop_helper_refuses_mismatched_systemd_pidfd_inode_without_signalling() {
        let mut target = std::process::Command::new("/bin/sleep")
            .arg("60")
            .spawn()
            .expect("spawn mismatched graceful-stop target");
        let process_id = target.id();
        let inode = pidfd_inode(process_id);
        let error = graceful_stop_main(process_id, inode.wrapping_add(1))
            .expect_err("mismatched pidfd inode must fail closed");
        assert!(error.to_string().contains("does not match pidfd inode"));
        assert!(
            target
                .try_wait()
                .expect("inspect unsignalled target")
                .is_none(),
            "inode mismatch unexpectedly signalled the target"
        );
        target.kill().expect("kill mismatch fixture");
        target.wait().expect("reap mismatch fixture");
    }

    #[test]
    fn publish_refuses_unconfigured_channel_before_helper_open_and_state_mutation() {
        let root = std::env::temp_dir().join(format!(
            "agentctl-chat-publish-authority-{}-{}",
            std::process::id(),
            NEXT_STATE.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        fs::create_dir(&root).expect("create fixture root");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("private fixture");
        BridgeState::initialize(
            &root,
            BridgeConfiguration {
                subscription_plugin: "fixture".to_owned(),
                subscription_environment: Vec::new(),
                channel_ids: vec!["spaces/allowed".to_owned()],
                allowed_senders: vec!["users/owner".to_owned()],
                agent_name: "coordinator".to_owned(),
                agent_label: "coordinator".to_owned(),
                outbound_enabled: true,
                ack_reaction: None,
                backend_configuration: None,
                outbound_command: Some(chat_runtime::OutboundCommandConfiguration {
                    executable: "/definitely/missing/helper".into(),
                    arguments: Vec::new(),
                    environment: Vec::new(),
                    timeout_millis: 1_000,
                    shutdown_grace_millis: 0,
                }),
            },
        )
        .expect("initialize state without opening helper");
        let state_before = state_namespace_snapshot(&root);

        let error = publish(
            &root,
            "spaces/denied",
            "123e4567-e89b-42d3-a456-426614174000",
            "operator message",
        )
        .expect_err("channel outside configured authority");
        assert!(error
            .to_string()
            .contains("configured channel_ids authority"));
        assert_eq!(state_namespace_snapshot(&root), state_before);
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn publish_uses_supervised_null_thread_transport_and_returns_exact_receipt() {
        let fixture = std::env::temp_dir().join(format!(
            "agentctl-chat-publish-success-{}-{}",
            std::process::id(),
            NEXT_STATE.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        fs::create_dir(&fixture).expect("create fixture");
        fs::set_permissions(&fixture, fs::Permissions::from_mode(0o700)).expect("private fixture");
        let helper = fixture.join("helper");
        fs::copy(
            fs::canonicalize("/bin/sh").expect("canonical shell"),
            &helper,
        )
        .expect("copy native helper");
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).expect("private helper");
        let request_id = "123e4567-e89b-42d3-a456-426614174000";
        let response = format!(
            "{{\"version\":1,\"id\":\"{request_id}\",\"action\":\"send\",\"ok\":true,\"receipt\":{{\"message_id\":\"spaces/allowed/messages/root\"}}}}"
        );
        let script = format!(
            "IFS= read -r request || exit 2; case \"$request\" in *'\"thread_id\":null'*) ;; *) exit 3;; esac; printf '%s\\n' '{response}'"
        );
        let root = fixture.join("state");
        BridgeState::initialize(
            &root,
            BridgeConfiguration {
                subscription_plugin: "fixture".to_owned(),
                subscription_environment: Vec::new(),
                channel_ids: vec!["spaces/allowed".to_owned()],
                allowed_senders: vec!["users/owner".to_owned()],
                agent_name: "coordinator".to_owned(),
                agent_label: "coordinator".to_owned(),
                outbound_enabled: true,
                ack_reaction: None,
                backend_configuration: None,
                outbound_command: Some(chat_runtime::OutboundCommandConfiguration {
                    executable: helper,
                    arguments: vec!["-c".to_owned(), script],
                    environment: Vec::new(),
                    timeout_millis: 2_000,
                    shutdown_grace_millis: 50,
                }),
            },
        )
        .expect("initialize state");
        let state_before = state_namespace_snapshot(&root);

        let receipt = publish(&root, "spaces/allowed", request_id, "operator message")
            .expect("publish root message");
        assert_eq!(
            receipt,
            json!({
                "version": 1,
                "id": request_id,
                "action": "send",
                "ok": true,
                "receipt": {"message_id": "spaces/allowed/messages/root"},
            })
        );
        assert_eq!(state_namespace_snapshot(&root), state_before);
        fs::remove_dir_all(fixture).expect("remove fixture");
    }

    fn edge_triggered_output_roundtrip(
        patterns: Vec<String>,
        snapshots: &[&str],
    ) -> Vec<Vec<PaneEvent>> {
        let root = std::env::temp_dir().join(format!(
            "agentctl-chat-output-edges-{}-{}",
            std::process::id(),
            NEXT_STATE.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        fs::create_dir(&root).expect("create edge fixture root");
        let socket = root.join("events.sock");
        let listener = UnixListener::bind(&socket).expect("bind edge fixture");
        let (poll, polls) = mpsc::channel::<String>();
        let (completed, completions) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().expect("accept edge client");
            let mut request = String::new();
            BufReader::new(connection.try_clone().expect("clone edge connection"))
                .read_line(&mut request)
                .expect("read edge subscription");
            let request: Value = serde_json::from_str(&request).expect("decode edge subscription");
            let mut matchers = request["params"]["subscriptions"]
                .as_array()
                .expect("subscriptions")
                .iter()
                .filter(|subscription| subscription["type"] == "pane.output_matched")
                .map(|subscription| {
                    (
                        regex::Regex::new(subscription["match"]["value"].as_str().expect("regex"))
                            .expect("compile output regex"),
                        false,
                    )
                })
                .collect::<Vec<_>>();
            let acknowledgement = json!({
                "id": request["id"],
                "result": {"type": "subscription_started"},
            });
            writeln!(connection, "{acknowledgement}").expect("acknowledge edge subscription");
            while let Ok(text) = polls.recv_timeout(Duration::from_secs(2)) {
                for (matcher, currently_matching) in &mut matchers {
                    // Herdr emits only the first matching line on a false-to-true transition.
                    // A different matching line cannot wake a subscription that remains true.
                    let matched = text.lines().find(|line| matcher.is_match(line));
                    if let Some(line) = matched.filter(|_| !*currently_matching) {
                        let event = json!({
                            "event": "pane.output_matched",
                            "data": {
                                "pane_id": "workspace:pane",
                                "matched_line": line,
                                "read": {
                                    "pane_id": "workspace:pane",
                                    "workspace_id": "workspace",
                                    "tab_id": "workspace:tab",
                                    "source": "recent_unwrapped",
                                    "format": "text",
                                    "text": text,
                                    "revision": 1,
                                    "truncated": false,
                                },
                            },
                        });
                        writeln!(connection, "{event}").expect("write edge event");
                    }
                    *currently_matching = matched.is_some();
                }
                completed.send(()).expect("complete snapshot poll");
            }
        });
        let mut stream = PaneEventStream::connect(
            &socket,
            "workspace:pane",
            patterns,
            SNAPSHOT_LINES_U32,
            Duration::from_secs(2),
        )
        .expect("connect edge fixture");
        let mut observed = Vec::new();
        for snapshot in snapshots {
            poll.send((*snapshot).to_owned()).expect("poll snapshot");
            completions
                .recv_timeout(Duration::from_secs(2))
                .expect("snapshot polled");
            observed.push(
                stream
                    .wait(Duration::from_millis(20))
                    .expect("read edge events"),
            );
        }
        drop(poll);
        server.join().expect("join edge fixture");
        fs::remove_dir_all(root).expect("remove edge fixture");
        observed
    }

    #[test]
    fn current_reply_wakes_survive_retained_closes_and_reconnect() {
        let first_key = "a".repeat(64);
        let second_key = "b".repeat(64);
        let third_key = "c".repeat(64);
        let mut routes = RouteCache::new(vec![
            ReplyRoute {
                key: first_key,
                nonce: "AAAAAAAAAAAAAAAAAAAAAA".to_owned(),
                identifier: "AAAAAAAAAAAAAAAAAAAAAA_2".to_owned(),
            },
            ReplyRoute {
                key: second_key.clone(),
                nonce: "BBBBBBBBBBBBBBBBBBBBBB".to_owned(),
                identifier: "BBBBBBBBBBBBBBBBBBBBBB_1".to_owned(),
            },
            ReplyRoute {
                key: third_key.clone(),
                nonce: "CCCCCCCCCCCCCCCCCCCCCC".to_owned(),
                identifier: "CCCCCCCCCCCCCCCCCCCCCC_1".to_owned(),
            },
        ]);
        let old = "  </CHAT_REPLY_AAAAAAAAAAAAAAAAAAAAAA_1>";
        let two_new = format!(
            "{old}\n<CHAT_REPLY_BBBBBBBBBBBBBBBBBBBBBB_1>\nsecond\n  </CHAT_REPLY_BBBBBBBBBBBBBBBBBBBBBB_1>\n<CHAT_REPLY_CCCCCCCCCCCCCCCCCCCCCC_1>\nthird\n● </CHAT_REPLY_CCCCCCCCCCCCCCCCCCCCCC_1>"
        );
        let identifiers = |events: &[PaneEvent]| {
            events
                .iter()
                .filter_map(|event| match event {
                    PaneEvent::Output { matched_line, .. } => {
                        matched_identifier(matched_line).map(str::to_owned)
                    }
                    PaneEvent::Settled { .. } | PaneEvent::Working => None,
                })
                .collect::<Vec<_>>()
        };
        let first_patterns = routes.patterns().expect("initial patterns");
        let observed =
            edge_triggered_output_roundtrip(first_patterns.clone(), &[old, &two_new, &two_new]);
        assert_eq!(identifiers(&observed[0]), ["AAAAAAAAAAAAAAAAAAAAAA_1"]);
        assert_eq!(
            identifiers(&observed[1]),
            ["BBBBBBBBBBBBBBBBBBBBBB_1"],
            "a consumed closing fence must not mask a newly visible current reply"
        );
        assert!(
            observed[2].is_empty(),
            "fixture must suppress a sustained match"
        );

        routes.replace(
            &second_key,
            Some(ReplyRoute {
                key: second_key.clone(),
                nonce: "BBBBBBBBBBBBBBBBBBBBBB".to_owned(),
                identifier: "BBBBBBBBBBBBBBBBBBBBBB_2".to_owned(),
            }),
        );
        let next_patterns = routes.patterns().expect("advanced patterns");
        assert_ne!(
            next_patterns, first_patterns,
            "route advance must reconnect"
        );
        let observed = edge_triggered_output_roundtrip(next_patterns, &[&two_new]);
        assert_eq!(
            identifiers(&observed[0]),
            ["CCCCCCCCCCCCCCCCCCCCCC_1", "AAAAAAAAAAAAAAAAAAAAAA_1"],
            "reconnecting must immediately find the other current close in the same snapshot"
        );

        routes.replace(&third_key, None);
        let next_reply = format!(
            "{two_new}\n<GCHAT_REPLY_BBBBBBBBBBBBBBBBBBBBBB_2>\nnext\n⏺ </GCHAT_REPLY_BBBBBBBBBBBBBBBBBBBBBB_2>"
        );
        let observed = edge_triggered_output_roundtrip(
            routes.patterns().expect("remaining patterns"),
            &[&two_new, &next_reply],
        );
        assert_eq!(identifiers(&observed[0]), ["AAAAAAAAAAAAAAAAAAAAAA_1"]);
        assert_eq!(identifiers(&observed[1]), ["BBBBBBBBBBBBBBBBBBBBBB_2"]);
    }

    #[test]
    fn route_cache_rearms_current_patterns_and_updates_direct_id_index() {
        let first_key = "a".repeat(64);
        let second_key = "b".repeat(64);
        let mut routes = RouteCache::new(vec![
            ReplyRoute {
                key: first_key.clone(),
                nonce: "AAAAAAAAAAAAAAAAAAAAAA".to_owned(),
                identifier: "AAAAAAAAAAAAAAAAAAAAAA_1".to_owned(),
            },
            ReplyRoute {
                key: second_key,
                nonce: "BBBBBBBBBBBBBBBBBBBBBB".to_owned(),
                identifier: "BBBBBBBBBBBBBBBBBBBBBB_7".to_owned(),
            },
        ]);
        let patterns = routes.patterns().expect("route patterns");
        assert_eq!(patterns.len(), 2);
        assert!(patterns[0].contains("(?:GCHAT|CHAT)_REPLY_"));
        assert_eq!(
            routes.key("AAAAAAAAAAAAAAAAAAAAAA_1"),
            Some(first_key.as_str())
        );

        let key = first_key;
        routes.replace(
            &key,
            Some(ReplyRoute {
                key: key.clone(),
                nonce: "AAAAAAAAAAAAAAAAAAAAAA".to_owned(),
                identifier: "AAAAAAAAAAAAAAAAAAAAAA_2".to_owned(),
            }),
        );
        assert_eq!(routes.key("AAAAAAAAAAAAAAAAAAAAAA_1"), None);
        assert_eq!(routes.key("AAAAAAAAAAAAAAAAAAAAAA_2"), Some(key.as_str()));
        let advanced_patterns = routes.patterns().expect("advanced patterns");
        assert_ne!(advanced_patterns, patterns);
        assert_eq!(advanced_patterns.last(), patterns.last());
        let current = regex::Regex::new(&advanced_patterns[0]).expect("current matcher");
        assert!(!current.is_match("</CHAT_REPLY_AAAAAAAAAAAAAAAAAAAAAA_1>"));
        assert!(current.is_match("</CHAT_REPLY_AAAAAAAAAAAAAAAAAAAAAA_2>"));
        assert!(!current.is_match("</CHAT_REPLY_unknown_1>"));
        routes.replace(
            &"c".repeat(64),
            Some(ReplyRoute {
                key: "c".repeat(64),
                nonce: "CCCCCCCCCCCCCCCCCCCCCC".to_owned(),
                identifier: "CCCCCCCCCCCCCCCCCCCCCC_1".to_owned(),
            }),
        );
        assert_ne!(
            routes.patterns().expect("admitted patterns"),
            advanced_patterns
        );
    }

    #[test]
    fn empty_route_cache_uses_only_the_bounded_unavailable_fence_predicate() {
        let patterns = RouteCache::new(Vec::new())
            .patterns()
            .expect("empty patterns");
        assert_eq!(patterns.len(), 1);
        assert!(patterns[0].contains("(?:GCHAT|CHAT)_REPLY_"));
        assert_eq!(
            matched_identifier("  • </CHAT_REPLY_nonce_4>  "),
            Some("nonce_4")
        );
        assert_eq!(
            matched_identifier("● </CHAT_REPLY_nonce_5>"),
            Some("nonce_5")
        );
        assert_eq!(matched_identifier("```"), None);
        assert_eq!(matched_identifier("<CHAT_REPLY_nonce_4>"), None);
    }

    #[test]
    fn current_reply_patterns_cover_full_durable_request_capacity_within_wire_bounds() {
        let routes = (1..=MAX_DIRECT_REQUEST_KEYS)
            .map(|index| ReplyRoute {
                key: format!("{index:064x}"),
                nonce: format!("{index:022x}"),
                identifier: format!("{index:022x}_999999"),
            })
            .collect::<Vec<_>>();
        let cache = RouteCache::new(routes);
        assert_eq!(cache.by_identifier.len(), 2_048);
        let patterns = cache.patterns().expect("full capacity patterns");
        assert_eq!(patterns.len(), 3);
        assert!(patterns
            .iter()
            .all(|pattern| pattern.len() <= chat_events::MAX_PATTERN_BYTES));
        assert!(
            patterns.iter().map(String::len).sum::<usize>() <= chat_events::MAX_TOTAL_PATTERN_BYTES
        );
        let matchers = patterns[..2]
            .iter()
            .map(|pattern| regex::Regex::new(pattern).expect("compile bounded current matcher"))
            .collect::<Vec<_>>();
        for identifier in cache.by_identifier.keys() {
            let line = format!("  </CHAT_REPLY_{identifier}>");
            assert_eq!(
                matchers
                    .iter()
                    .filter(|matcher| matcher.is_match(&line))
                    .count(),
                1
            );
            let consumed = line.replace("_999999>", "_999998>");
            assert!(matchers.iter().all(|matcher| !matcher.is_match(&consumed)));
        }
        assert!(edge_triggered_output_roundtrip(patterns, &[]).is_empty());
    }

    #[test]
    fn current_reply_patterns_reject_invalid_identifiers_and_excess_budget() {
        let invalid = RouteCache::new(vec![ReplyRoute {
            key: "a".repeat(64),
            nonce: "AAAAAAAAAAAAAAAAAAAAAA".to_owned(),
            identifier: "AAAAAAAAAAAAAAAAAAAAAA_.*".to_owned(),
        }]);
        assert!(invalid.patterns().is_err());
        let over_budget = RouteCache::new(
            (0..4_096)
                .map(|index| ReplyRoute {
                    key: format!("{index:064x}"),
                    nonce: format!("{index:022x}"),
                    identifier: format!("{index:022x}_999999"),
                })
                .collect(),
        );
        assert!(over_budget.patterns().is_err());
    }

    #[test]
    fn provider_hint_queue_is_bounded_and_marks_durable_recovery() {
        let mut queued = DirectKeyQueue::default();
        let overflowed = AtomicBool::new(false);
        enqueue_direct_keys(
            &mut queued,
            (0..=MAX_DIRECT_REQUEST_KEYS)
                .map(|index| format!("{index:064x}"))
                .collect(),
            &overflowed,
        );
        assert_eq!(queued.len(), MAX_DIRECT_REQUEST_KEYS);
        assert!(overflowed.load(Ordering::SeqCst));
    }

    #[test]
    fn restart_drains_full_256_request_batch_without_waiting_for_reconciliation_timer() {
        let root = std::env::temp_dir().join(format!(
            "agentctl-chat-service-backlog-{}-{}",
            std::process::id(),
            NEXT_STATE.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        fs::create_dir(&root).expect("create fixture root");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("private fixture");
        let state = BridgeState::initialize(
            &root,
            BridgeConfiguration {
                subscription_plugin: "fixture".to_owned(),
                subscription_environment: Vec::new(),
                channel_ids: vec!["spaces/example".to_owned()],
                allowed_senders: vec!["users/owner".to_owned()],
                agent_name: "coordinator".to_owned(),
                agent_label: "coordinator".to_owned(),
                outbound_enabled: false,
                ack_reaction: None,
                backend_configuration: None,
                outbound_command: None,
            },
        )
        .expect("initialize state");
        let events = (0..chat_subscription::MAX_BATCH_EVENTS)
            .map(|index| {
                CommittableEvent::message_created(
                    InboundMessage::new(
                        ChannelId::new("spaces/example").expect("channel"),
                        MessageId::new(format!("spaces/example/messages/{index}"))
                            .expect("message"),
                        ThreadId::new("spaces/example/threads/one").expect("thread"),
                        SenderId::new("users/owner").expect("sender"),
                        format!("request {index}"),
                        "2026-09-21T12:00:00Z",
                        false,
                    )
                    .expect("message"),
                )
            })
            .collect::<Vec<_>>();
        let batch = DeliveryBatch::new(
            EventSequence::new(1).expect("sequence"),
            ProviderCursor::new("cursor-256").expect("cursor"),
            DeliveryId::new("delivery-256").expect("delivery"),
            events,
        )
        .expect("batch");
        state.admit_batch(&batch).expect("admit batch");
        drop(state);

        let reopened = BridgeState::open(&root).expect("restart state");
        let mut queued = DirectKeyQueue::default();
        enqueue_direct_keys(
            &mut queued,
            reopened.pending_request_keys().expect("pending keys"),
            &AtomicBool::new(false),
        );
        assert_eq!(queued.len(), chat_subscription::MAX_BATCH_EVENTS);
        let delivery = RecordingDelivery::default();
        let mut transport = None;
        let mut control = PassControl {
            transport: &mut transport,
            stop: None,
        };
        let report = drain_immediate_backlog_with_delivery(
            &reopened,
            &delivery,
            DrainOptions::default(),
            &mut queued,
            &mut control,
            &AtomicBool::new(false),
        )
        .expect("drain causal backlog");
        assert!(queued.is_empty());
        assert_eq!(report.delivered.len(), chat_subscription::MAX_BATCH_EVENTS);
        assert_eq!(
            delivery.prompts.lock().expect("prompts").len(),
            chat_subscription::MAX_BATCH_EVENTS
        );
        assert!(reopened
            .pending_request_keys()
            .expect("durable pending keys")
            .is_empty());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn a_second_reply_under_one_alias_is_read_by_the_saturated_poll() {
        // A request keeps its reply alias for every reply, so once its first closing line is in
        // the pane the output pattern for that alias stays matched, and herdr raises no event for
        // the second closing line. The service reads the pane itself instead, on the saturated
        // poll, while such a line is in view.
        let (state, key, root) = state_with_request();
        let delivery = RecordingDelivery::default();
        chat_runtime::deliver_request_with(&state, &delivery, &key, DrainOptions::default())
            .expect("deliver request");
        assert!(delivery.prompts.lock().expect("prompts")[0]
            .contains("Include the line <CHAT_REPLY_001> at the beginning"));
        let route = state
            .next_reply_route(&key)
            .expect("route")
            .expect("active route");
        assert_eq!(route.identifier, "001");
        let mut routes = RouteCache::new(vec![route]);
        let first = "<CHAT_REPLY_001>\nfirst answer\n  </CHAT_REPLY_001>\n".to_owned();
        let second = format!("{first}<CHAT_REPLY_001>\nsecond answer\n</CHAT_REPLY_001>\n");
        let identifiers = |events: &[PaneEvent]| {
            events
                .iter()
                .filter_map(|event| match event {
                    PaneEvent::Output { matched_line, .. } => {
                        matched_identifier(matched_line).map(str::to_owned)
                    }
                    PaneEvent::Settled { .. } | PaneEvent::Working => None,
                })
                .collect::<Vec<_>>()
        };
        let patterns = routes.patterns().expect("patterns");
        let observed = edge_triggered_output_roundtrip(patterns.clone(), &[&first, &second]);
        // The first closing line raises one event for each pattern it matches: its chunk of exact
        // IDs and the pattern for any ID.
        let first_events = identifiers(&observed[0]);
        assert!(
            !first_events.is_empty() && first_events.iter().all(|id| id == "001"),
            "{first_events:?}"
        );
        assert!(
            identifiers(&observed[1]).is_empty(),
            "herdr raises no event for a second closing line under the same alias"
        );
        assert!(routes.saturated_by(&first) && routes.saturated_by(&second));
        assert!(next_saturated_poll(&routes, &second).is_some());

        let client = HerdrClient::with_executable("direct", Path::new("/missing/herdr"))
            .expect("construct client");
        let manager = ManagedAgents::new(&client, &root.join("registry")).expect("manager");
        let mut transport = None;
        let mut control = PassControl {
            transport: &mut transport,
            stop: None,
        };
        let report = capture_direct(
            &state,
            &manager,
            DrainOptions::default(),
            &mut routes,
            "001",
            SnapshotInput {
                text: &first,
                truncated: false,
                revision: Some(1),
            },
            &mut control,
        )
        .expect("capture the first reply");
        assert_eq!(report.captured, [(key.clone(), vec![1])]);
        // The alias stays the same after a reply, so its pattern and the saturation do too.
        assert_eq!(routes.patterns().expect("patterns"), patterns);
        assert!(next_saturated_poll(&routes, &second).is_some());
        let poll = |routes: &mut RouteCache, control: &mut PassControl<'_>| {
            capture_visible_current(
                &state,
                &manager,
                DrainOptions::default(),
                routes,
                &second,
                control,
            )
            .expect("saturated poll")
        };
        assert_eq!(
            poll(&mut routes, &mut control).captured,
            [(key.clone(), vec![2])]
        );
        assert!(poll(&mut routes, &mut control).captured.is_empty());

        // No poll for a block still being written, for an alias no open request holds, or for a
        // long ID, whose pattern changes once its reply is stored.
        assert!(!routes.saturated_by("<CHAT_REPLY_001>\nstill writing\n"));
        for other in ["000", "002"] {
            let text = format!("<CHAT_REPLY_{other}>\nanswer\n</CHAT_REPLY_{other}>\n");
            assert!(!routes.saturated_by(&text), "{text}");
        }
        let long_id = "BBBBBBBBBBBBBBBBBBBBBB_1";
        let legacy = RouteCache::new(vec![ReplyRoute {
            key: "b".repeat(64),
            nonce: "BBBBBBBBBBBBBBBBBBBBBB".to_owned(),
            identifier: long_id.to_owned(),
        }]);
        let legacy_text = format!("<CHAT_REPLY_{long_id}>\nanswer\n</CHAT_REPLY_{long_id}>\n");
        assert!(next_saturated_poll(&legacy, &legacy_text).is_none());
        // Once the request closes, its alias no longer holds the poll.
        state.close_replies(&key).expect("close replies");
        routes.replace(&key, None);
        assert!(next_saturated_poll(&routes, &second).is_none());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn concurrently_closed_route_is_evicted_without_capture_failure() {
        let (state, key, root) = state_with_request();
        let route = state
            .next_reply_route(&key)
            .expect("read route")
            .expect("active route");
        let mut routes = RouteCache::new(vec![route.clone()]);
        state
            .close_replies(&key)
            .expect("close replies concurrently");
        let client = HerdrClient::with_executable("direct", Path::new("/missing/herdr"))
            .expect("construct client");
        let manager = ManagedAgents::new(&client, &root.join("registry")).expect("manager");
        let mut transport = None;
        let mut control = PassControl {
            transport: &mut transport,
            stop: None,
        };
        let report = capture_direct(
            &state,
            &manager,
            DrainOptions::default(),
            &mut routes,
            &route.identifier,
            SnapshotInput {
                text: &format!("</CHAT_REPLY_{}>", route.identifier),
                truncated: false,
                revision: Some(7),
            },
            &mut control,
        )
        .expect("stale close is a capture no-op");
        assert!(report.captured.is_empty());
        assert_eq!(report.snapshot_revision, Some(7));
        assert_eq!(routes.key(&route.identifier), None);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn sixty_four_unknown_fences_coalesce_to_one_recovery_and_find_late_route() {
        let (state, key, root) = state_with_request();
        let route = state
            .next_reply_route(&key)
            .expect("route")
            .expect("active route");
        let client = HerdrClient::with_executable("direct", Path::new("/missing/herdr"))
            .expect("construct client");
        let manager = ManagedAgents::new(&client, &root.join("registry")).expect("manager");
        let mut routes = RouteCache::from_entries(Vec::new());
        let mut transport = None;
        let mut control = PassControl {
            transport: &mut transport,
            stop: None,
        };
        let mut recovery_requests = 0_usize;
        for index in 0..64 {
            let report = capture_direct(
                &state,
                &manager,
                DrainOptions::default(),
                &mut routes,
                &format!("unknownnonce{index:02}_1"),
                SnapshotInput {
                    text: "bogus terminal marker",
                    truncated: false,
                    revision: Some(index),
                },
                &mut control,
            )
            .expect("unknown route is deferred");
            recovery_requests += usize::from(report.recovery_requested);
        }
        assert_eq!(recovery_requests, 64);
        let rendered = format!(
            "<CHAT_REPLY_{}>\nlate route reply\n</CHAT_REPLY_{}>",
            route.identifier, route.identifier
        );
        let report = capture_recovery_snapshot(
            &state,
            &manager,
            DrainOptions::default(),
            &mut routes,
            SnapshotInput {
                text: &rendered,
                truncated: false,
                revision: Some(65),
            },
            &mut control,
        )
        .expect("one coalesced recovery scan");
        assert_eq!(report.captured.len(), 1);
        assert_eq!(report.captured[0].0, key);
        assert!(routes.knows_identifier_nonce(&route.identifier));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn already_reported_log_line_names_eight_ids_and_counts_the_rest() {
        assert_eq!(already_reported_log_line(&[]), None);
        assert_eq!(
            already_reported_log_line(&["stale_1".to_owned()]).as_deref(),
            Some("agentctl: chat reply fence feedback: already reported, so not repeated: stale_1")
        );
        let many = (1..=10)
            .map(|index| format!("stale_{index}"))
            .collect::<Vec<_>>();
        assert_eq!(
            already_reported_log_line(&many).as_deref(),
            Some(
                "agentctl: chat reply fence feedback: already reported, so not repeated: stale_1, \
stale_2, stale_3, stale_4, stale_5, stale_6, stale_7, stale_8 and 2 more"
            )
        );
    }

    #[test]
    fn provider_backoff_doubles_to_sixty_seconds_and_restarts_after_a_healthy_generation() {
        let short = Duration::from_millis(10);
        let mut backoff = ProviderBackoff::new();
        let waits = (0..8)
            .map(|_| backoff.wait_after(short))
            .collect::<Vec<_>>();
        assert_eq!(
            waits,
            [1, 2, 4, 8, 16, 32, 60, 60].map(Duration::from_secs),
            "generations that fail quickly back off to the longest wait and stay there"
        );
        assert_eq!(
            backoff.wait_after(PROVIDER_RETRY_MAX),
            PROVIDER_RETRY_MIN,
            "a generation that lasted the longest wait ends a new incident"
        );
        assert_eq!(backoff.wait_after(short), Duration::from_secs(2));

        let mut backoff = ProviderBackoff::new();
        assert_eq!(backoff.wait_after(short), Duration::from_secs(1));
        assert_eq!(
            backoff.wait_after(PROVIDER_RETRY_MAX - Duration::from_millis(1)),
            Duration::from_secs(2),
            "a generation just shorter than the longest wait keeps backing off"
        );
        assert_eq!(
            backoff.wait_after(Duration::from_secs(3_600)),
            PROVIDER_RETRY_MIN
        );
    }

    /// Records each write call separately, so a test can tell one write from several.
    #[derive(Default)]
    struct WriteCalls(Vec<Vec<u8>>);

    impl io::Write for WriteCalls {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.push(bytes.to_vec());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Standard error on a full disk (ENOSPC) or a pipe whose reader has exited (EPIPE).
    struct FailingWrites(i32);

    impl io::Write for FailingWrites {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from_raw_os_error(self.0))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::from_raw_os_error(self.0))
        }
    }

    /// A service log for tests. Libtest captures `eprintln!`, also from the threads a test starts,
    /// so a line is shown only when its test fails.
    fn captured_service_log(line: fmt::Arguments<'_>) {
        eprintln!("{line}");
    }

    #[test]
    fn service_log_writes_the_utc_second_and_the_line_in_one_write() {
        let before = chat_runtime::log_timestamp();
        let mut out = WriteCalls::default();
        write_service_log(
            &mut out,
            format_args!(
                "agentctl: chat provider: {}; reconnecting in {}s",
                "stream ended", 1
            ),
        );
        let after = chat_runtime::log_timestamp();
        assert_eq!(out.0.len(), 1, "one line is one write: {:?}", out.0);
        let line = String::from_utf8(out.0.remove(0)).expect("UTF-8 log line");
        let (stamp, text) = line
            .split_once(' ')
            .expect("the time, a space, then the line");
        assert_eq!(
            text,
            "agentctl: chat provider: stream ended; reconnecting in 1s\n"
        );
        let shape = "0000-00-00T00:00:00Z";
        assert_eq!(stamp.len(), shape.len(), "{stamp}");
        assert!(
            stamp
                .chars()
                .zip(shape.chars())
                .all(|(actual, form)| if form == '0' {
                    actual.is_ascii_digit()
                } else {
                    actual == form
                }),
            "{stamp}"
        );
        assert!(
            before.as_str() <= stamp && stamp <= after.as_str(),
            "{before} <= {stamp} <= {after}"
        );
    }

    #[test]
    fn a_service_log_line_that_cannot_be_written_is_dropped_without_a_panic() {
        for error in [libc::ENOSPC, libc::EPIPE] {
            write_service_log(
                &mut FailingWrites(error),
                "agentctl: chat provider: subscribed from the saved cursor",
            );
        }
    }

    #[test]
    fn a_chat_tick_pass_writes_its_diagnostic_line_without_a_time() {
        let mut transport = None;
        let control = PassControl {
            transport: &mut transport,
            stop: None,
        };
        let mut out = WriteCalls::default();
        control.log_to(
            &mut out,
            "agentctl: chat reply fence feedback: already reported, so not repeated: stale_1",
        );
        assert_eq!(
            String::from_utf8(out.0.concat()).expect("UTF-8 line"),
            "agentctl: chat reply fence feedback: already reported, so not repeated: stale_1\n"
        );
    }

    #[test]
    fn a_chat_run_pass_writes_its_diagnostic_line_after_the_utc_second() {
        let stop = StopState::default();
        let mut transport = None;
        let control = PassControl {
            transport: &mut transport,
            stop: Some(&stop),
        };
        let mut out = WriteCalls::default();
        control.log_to(
            &mut out,
            "agentctl: chat reply fence feedback: already reported, so not repeated: stale_1",
        );
        assert_eq!(out.0.len(), 1, "one line is one write: {:?}", out.0);
        let line = String::from_utf8(out.0.remove(0)).expect("UTF-8 log line");
        let (stamp, text) = line
            .split_once(' ')
            .expect("the time, a space, then the line");
        assert_eq!(
            text,
            "agentctl: chat reply fence feedback: already reported, so not repeated: stale_1\n"
        );
        let shape = "0000-00-00T00:00:00Z";
        assert_eq!(stamp.len(), shape.len(), "{stamp}");
        assert!(
            stamp
                .chars()
                .zip(shape.chars())
                .all(|(actual, form)| if form == '0' {
                    actual.is_ascii_digit()
                } else {
                    actual == form
                }),
            "{stamp}"
        );
    }

    #[test]
    fn taking_notices_fails_once_the_provider_worker_ends_unless_the_service_is_stopping() {
        let output_wake: SharedWake = Arc::new(Mutex::new(None));
        let overflowed = AtomicBool::new(false);
        let mut direct_keys = DirectKeyQueue::default();
        let stop = StopState::default();
        let (sender, receiver) = mpsc::sync_channel(PROVIDER_NOTICE_CAPACITY);
        send_notice(
            &sender,
            ProviderNotice::Batch(vec!["key-1".to_owned()]),
            &output_wake,
            &overflowed,
        );
        take_provider_notices(&receiver, &stop, &mut direct_keys, &overflowed)
            .expect("a running worker's notice");
        assert_eq!(direct_keys.take(8), ["key-1"]);
        take_provider_notices(&receiver, &stop, &mut direct_keys, &overflowed)
            .expect("a running worker with nothing to say");

        // A worker that ends without a stop, as after a provider cleanup failure, delivers what it
        // sent first, and then the closed channel is an error, which the owner loop returns.
        send_notice(
            &sender,
            ProviderNotice::Batch(vec!["key-2".to_owned()]),
            &output_wake,
            &overflowed,
        );
        end_provider_worker(Ok(()), sender, &stop, &output_wake);
        assert!(!stop.is_stopped());
        let error = take_provider_notices(&receiver, &stop, &mut direct_keys, &overflowed)
            .expect_err("an ended provider worker");
        assert_eq!(
            error.to_string(),
            "chat provider worker stopped unexpectedly"
        );
        assert_eq!(direct_keys.take(8), ["key-2"]);
        assert!(!overflowed.load(Ordering::SeqCst));

        // During a stop, the worker's end is the expected one.
        let stop = StopState::default();
        let (sender, receiver) = mpsc::sync_channel(PROVIDER_NOTICE_CAPACITY);
        stop.stop();
        end_provider_worker(Ok(()), sender, &stop, &output_wake);
        take_provider_notices(&receiver, &stop, &mut direct_keys, &overflowed)
            .expect("a stopping service");
        assert!(direct_keys.is_empty());
        assert!(stop.take_cleanup_errors().is_empty());
    }

    #[test]
    fn one_pass_finds_an_ended_provider_worker_behind_a_full_notice_channel() {
        // The worker's last wake-up can come before a pass that starts with the channel full, and
        // then no other wake-up follows, so that one pass has to reach the closed channel. Before,
        // a pass stopped after as many notices as the channel holds, and the owner loop slept
        // until its next recovery scan before `chat run` ended.
        let output_wake: SharedWake = Arc::new(Mutex::new(None));
        let overflowed = AtomicBool::new(false);
        let mut direct_keys = DirectKeyQueue::default();
        let stop = StopState::default();
        let (sender, receiver) = mpsc::sync_channel(PROVIDER_NOTICE_CAPACITY);
        for _ in 0..PROVIDER_NOTICE_CAPACITY {
            send_notice(
                &sender,
                ProviderNotice::Batch(Vec::new()),
                &output_wake,
                &overflowed,
            );
        }
        assert!(
            !overflowed.load(Ordering::SeqCst),
            "the channel took every notice"
        );
        end_provider_worker(Ok(()), sender, &stop, &output_wake);
        let error = take_provider_notices(&receiver, &stop, &mut direct_keys, &overflowed)
            .expect_err("an ended provider worker behind a full channel");
        assert_eq!(
            error.to_string(),
            "chat provider worker stopped unexpectedly"
        );
        assert!(direct_keys.is_empty());
        assert!(!overflowed.load(Ordering::SeqCst));
    }

    #[test]
    fn a_fatal_provider_notice_is_an_error_after_the_batches_sent_before_it() {
        let output_wake: SharedWake = Arc::new(Mutex::new(None));
        let overflowed = AtomicBool::new(false);
        let mut direct_keys = DirectKeyQueue::default();
        let stop = StopState::default();
        let (sender, receiver) = mpsc::sync_channel(PROVIDER_NOTICE_CAPACITY);
        send_notice(
            &sender,
            ProviderNotice::Batch(vec!["key-1".to_owned()]),
            &output_wake,
            &overflowed,
        );
        send_notice(
            &sender,
            ProviderNotice::Fatal("cursor 7 is older than the retained events".to_owned()),
            &output_wake,
            &overflowed,
        );
        let error = take_provider_notices(&receiver, &stop, &mut direct_keys, &overflowed)
            .expect_err("a fatal notice");
        assert_eq!(
            error.to_string(),
            "chat provider stopped on an unrecoverable stream gap: cursor 7 is older than the \
retained events"
        );
        assert_eq!(direct_keys.take(8), ["key-1"]);
        assert!(!stop.is_stopped());
    }

    #[test]
    fn an_ended_provider_worker_wakes_a_wait_on_the_output_stream() {
        let output_wake: SharedWake = Arc::new(Mutex::new(None));
        let (connected, release, server, root) = connected_event_stream("^startup$");
        let mut stream = None;
        install_output_stream(&mut stream, &output_wake, connected).expect("install stream");
        let active = stream.as_mut().expect("installed stream");
        assert!(active
            .wait(Duration::from_secs(30))
            .expect("the install's own wake")
            .is_empty());

        let (sender, receiver) = mpsc::sync_channel::<ProviderNotice>(PROVIDER_NOTICE_CAPACITY);
        let stop = StopState::default();
        end_provider_worker(Ok(()), sender, &stop, &output_wake);
        let started = Instant::now();
        assert!(active
            .wait(Duration::from_secs(30))
            .expect("woken output wait")
            .is_empty());
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the output wait slept through the provider worker's end: {:?}",
            started.elapsed()
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));

        drop(stream.take());
        *output_wake.lock().expect("output wake lock") = None;
        release.send(()).expect("release event fixture");
        server.join().expect("join event fixture");
        fs::remove_dir_all(root).expect("remove event fixture");
    }

    #[test]
    fn a_provider_worker_panic_stops_the_service_and_still_unwinds() {
        let output_wake: SharedWake = Arc::new(Mutex::new(None));
        let stop = StopState::default();
        let (sender, receiver) = mpsc::sync_channel::<ProviderNotice>(PROVIDER_NOTICE_CAPACITY);
        let panicked = std::panic::catch_unwind(|| panic!("provider worker fixture panic"));
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            end_provider_worker(panicked, sender, &stop, &output_wake)
        }));
        assert!(
            unwound.is_err(),
            "joining the worker still reports the panic"
        );
        assert!(
            stop.is_stopped(),
            "the service stops rather than running without chat events"
        );
        assert_eq!(
            stop.take_cleanup_errors(),
            ["chat provider worker panicked; no chat events arrive after it"]
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn the_provider_worker_logs_each_reconnect_wait_and_backs_off_until_the_service_stops() {
        let output_wake: SharedWake = Arc::new(Mutex::new(None));
        let stop = Arc::new(StopState::default());
        let (sender, receiver) = mpsc::sync_channel::<ProviderNotice>(PROVIDER_NOTICE_CAPACITY);
        let (record, recorded) = mpsc::channel::<String>();
        let failed = || -> Result<(), ProviderGenerationError> {
            Err(ProviderGenerationError::Retryable("x".to_owned()))
        };
        let quick = Duration::from_millis(10);
        // How long each generation lasts on the scripted clock, and how it ends.
        let mut script = vec![(Duration::ZERO, failed()), (Duration::ZERO, Ok(()))];
        script.extend((0..6).map(|_| (quick, failed())));
        script.push((PROVIDER_RETRY_MAX, Ok(())));
        script.push((quick, failed()));
        let generations = script.len();
        let worker = spawn_provider_worker(
            Arc::clone(&stop),
            sender,
            Arc::clone(&output_wake),
            move |stop, notices, output_wake| {
                let clock = Cell::new(Instant::now());
                let waited = Cell::new(0);
                let mut script = VecDeque::from(script);
                run_provider_worker(
                    || {
                        let (lasted, ended) = script.pop_front().expect("a scripted generation");
                        clock.set(clock.get() + lasted);
                        ended
                    },
                    || clock.get(),
                    &|line| record.send(line.to_string()).expect("record a log line"),
                    |delay| {
                        record
                            .send(format!("wait {delay:?}"))
                            .expect("record a wait");
                        waited.set(waited.get() + 1);
                        if waited.get() == generations {
                            // The owner stops the service during the last reconnect wait.
                            stop.stop();
                        }
                    },
                    stop,
                    notices,
                    output_wake,
                );
            },
        )
        .expect("spawn provider worker");
        join_worker_until(
            worker,
            "chat provider",
            Instant::now() + Duration::from_secs(30),
        )
        .expect("the worker ends once the service stops");
        let expected = [
            ("x", 1),
            ("stream ended", 2),
            ("x", 4),
            ("x", 8),
            ("x", 16),
            ("x", 32),
            ("x", 60),
            ("x", 60),
            // The generation that lasted the longest wait was healthy, so the wait starts again.
            ("stream ended", 1),
            ("x", 2),
        ]
        .into_iter()
        .flat_map(|(ended, seconds)| {
            [
                format!("agentctl: chat provider: {ended}; reconnecting in {seconds}s"),
                format!("wait {seconds}s"),
            ]
        })
        .collect::<Vec<_>>();
        assert_eq!(recorded.try_iter().collect::<Vec<_>>(), expected);
        assert!(stop.take_cleanup_errors().is_empty());
        assert!(
            matches!(receiver.try_recv(), Err(mpsc::TryRecvError::Disconnected)),
            "the ended worker closed its channel"
        );
    }

    #[test]
    fn a_panicking_provider_generation_stops_the_service_before_the_worker_channel_closes() {
        let output_wake: SharedWake = Arc::new(Mutex::new(None));
        let queue = Arc::new(AckQueue::default());
        let stop = Arc::new(StopState {
            ack_queue: Some(Arc::clone(&queue)),
            ..StopState::default()
        });
        let (sender, receiver) = mpsc::sync_channel::<ProviderNotice>(PROVIDER_NOTICE_CAPACITY);
        // `StopState::stop` takes the ACK queue's lock first, so holding that lock holds the worker
        // inside its stop, after it recorded the panic. The test then sees what the owner loop
        // would see at that moment.
        let held = queue.state.lock().expect("ACK queue lock");
        let worker = spawn_provider_worker(
            Arc::clone(&stop),
            sender,
            Arc::clone(&output_wake),
            |stop, notices, output_wake| {
                run_provider_worker(
                    || panic!("provider generation fixture panic"),
                    Instant::now,
                    &|line| panic!("a panicking generation logged: {line}"),
                    |delay| panic!("a panicking generation waited {delay:?}"),
                    stop,
                    notices,
                    output_wake,
                )
            },
        )
        .expect("spawn provider worker");
        let deadline = Instant::now() + Duration::from_secs(30);
        while stop
            .cleanup_errors
            .lock()
            .expect("cleanup errors")
            .is_empty()
            && !worker.handle.is_finished()
        {
            assert!(
                Instant::now() < deadline,
                "the worker neither recorded its panic nor ended"
            );
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            *stop.cleanup_errors.lock().expect("cleanup errors"),
            ["chat provider worker panicked; no chat events arrive after it"]
        );
        assert!(!stop.is_stopped(), "the worker is held inside its stop");
        assert!(
            matches!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "the worker closed its channel before it stopped the service"
        );
        drop(held);
        let error = join_worker_until(
            worker,
            "chat provider",
            Instant::now() + Duration::from_secs(30),
        )
        .expect_err("joining the worker reports the panic");
        assert!(
            stop.is_stopped(),
            "the service stops rather than running without chat events"
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        // The owner loop's check of the channel then finds the service stopping, so the closed
        // channel is not a second error, and the joined panic is the last cleanup error.
        let owner = take_provider_notices(
            &receiver,
            &stop,
            &mut DirectKeyQueue::default(),
            &AtomicBool::new(false),
        );
        stop.record_cleanup_error(error.to_string());
        assert_eq!(
            service_outcome(owner, stop.take_cleanup_errors())
                .expect_err("a panicked worker fails the service")
                .to_string(),
            "chat shutdown cleanup is uncertain: chat provider worker panicked; no chat events \
arrive after it; chat provider worker panicked"
        );
    }

    #[test]
    fn closed_nonce_index_keeps_repeated_old_markers_stale() {
        let (state, _, root) = state_with_request();
        let key = "a".repeat(64);
        let mut routes = RouteCache::from_entries(vec![ReplyRouteEntry {
            key: key.clone(),
            nonce: "AAAAAAAAAAAAAAAAAAAAAA".to_owned(),
            alias: None,
            current_identifier: Some("AAAAAAAAAAAAAAAAAAAAAA_2".to_owned()),
        }]);
        routes.replace(&key, None);
        assert_eq!(
            refresh_matched_route(&state, &mut routes, "AAAAAAAAAAAAAAAAAAAAAA_1")
                .expect("stale route"),
            MatchedRoute::Stale
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn bounded_snapshot_keeps_the_newest_whole_lines_that_fit() {
        let small = "one\ntwo\n";
        assert_eq!(bounded_snapshot(small), (small, false));
        let exact = "x".repeat(MAX_SNAPSHOT_BYTES - 1) + "\n";
        assert_eq!(bounded_snapshot(&exact), (exact.as_str(), false));
        // One 16-byte line more than fits: only the oldest line is left out.
        let line = "0123456789abcde\n";
        let lines = line.repeat(MAX_SNAPSHOT_BYTES / line.len() + 1);
        let (kept, cut) = bounded_snapshot(&lines);
        assert!(cut);
        assert_eq!(kept.len(), MAX_SNAPSHOT_BYTES);
        assert_eq!(&lines[line.len()..], kept);
        // A cut never splits a character or a line.
        let wide = "é".repeat(100) + "\n";
        let text = wide.repeat(MAX_SNAPSHOT_BYTES / wide.len() + 2);
        let (kept, cut) = bounded_snapshot(&text);
        assert!(cut);
        assert!(kept.len() <= MAX_SNAPSHOT_BYTES);
        assert!(kept.len() > MAX_SNAPSHOT_BYTES - wide.len());
        assert!(kept.starts_with(&wide));
        // One line longer than the bound leaves no complete line to read.
        let long = "y".repeat(MAX_SNAPSHOT_BYTES + 1);
        assert_eq!(bounded_snapshot(&long), ("", true));
    }

    #[test]
    fn any_well_formed_reply_id_of_an_open_request_routes_to_it() {
        let (state, key, root) = state_with_request();
        let route = state
            .next_reply_route(&key)
            .expect("route")
            .expect("active route");
        let nonce = route
            .identifier
            .strip_suffix("_1")
            .expect("first ID")
            .to_owned();
        let mut routes = RouteCache::new(vec![route]);
        for ordinal in ["1", "2", "999999"] {
            assert_eq!(
                refresh_matched_route(&state, &mut routes, &format!("{nonce}_{ordinal}"))
                    .expect("route"),
                MatchedRoute::Current(key.clone()),
                "ordinal {ordinal}"
            );
        }
        for ordinal in ["0", "01", "1000000", "x", ""] {
            assert_eq!(
                refresh_matched_route(&state, &mut routes, &format!("{nonce}_{ordinal}"))
                    .expect("route"),
                MatchedRoute::Stale,
                "ordinal {ordinal:?}"
            );
        }
        assert_eq!(
            refresh_matched_route(&state, &mut routes, "unknownnonce_1").expect("route"),
            MatchedRoute::Unknown
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn a_new_answer_under_an_earlier_reply_id_is_captured_directly() {
        let (state, key, root) = state_with_request();
        let route = state
            .next_reply_route(&key)
            .expect("route")
            .expect("active route");
        let identifier = route.identifier.clone();
        let mut routes = RouteCache::new(vec![route]);
        let client = HerdrClient::with_executable("direct", Path::new("/missing/herdr"))
            .expect("construct client");
        let manager = ManagedAgents::new(&client, &root.join("registry")).expect("manager");
        let mut transport = None;
        let mut control = PassControl {
            transport: &mut transport,
            stop: None,
        };
        let first =
            format!("<CHAT_REPLY_{identifier}>\nfirst answer\n</CHAT_REPLY_{identifier}>\n");
        let second = format!(
            "{first}<CHAT_REPLY_{identifier}>\nsecond answer\n</CHAT_REPLY_{identifier}>\n"
        );
        for (revision, text, ordinal) in [(1, &first, 1), (2, &second, 2)] {
            let report = capture_direct(
                &state,
                &manager,
                DrainOptions::default(),
                &mut routes,
                &identifier,
                SnapshotInput {
                    text,
                    truncated: false,
                    revision: Some(revision),
                },
                &mut control,
            )
            .expect("direct capture");
            assert_eq!(report.captured, [(key.clone(), vec![ordinal])]);
            assert!(report.notes.is_empty());
        }
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn a_refused_or_oversized_capture_is_noted_and_the_rest_is_captured() {
        let (state, key, root) = state_with_request();
        let route = state
            .next_reply_route(&key)
            .expect("route")
            .expect("active route");
        let nonce = route.identifier.strip_suffix("_1").expect("first ID");
        let client = HerdrClient::with_executable("direct", Path::new("/missing/herdr"))
            .expect("construct client");
        let manager = ManagedAgents::new(&client, &root.join("registry")).expect("manager");
        let mut routes = RouteCache::from_entries(Vec::new());
        let mut transport = None;
        let mut control = PassControl {
            transport: &mut transport,
            stop: None,
        };
        let block = |ordinal: u32, text: &str| {
            format!("<CHAT_REPLY_{nonce}_{ordinal}>\n{text}\n</CHAT_REPLY_{nonce}_{ordinal}>\n")
        };
        let rendered = [
            block(1, "first"),
            block(2, "second"),
            block(3, &"z".repeat(40_000)),
            block(3, "first"),
        ]
        .concat();
        let report = capture_recovery_snapshot(
            &state,
            &manager,
            DrainOptions::default(),
            &mut routes,
            SnapshotInput {
                text: &rendered,
                truncated: false,
                revision: Some(1),
            },
            &mut control,
        )
        .expect("a refused block does not fail the pass");
        assert_eq!(report.captured, [(key.clone(), vec![1, 2])]);
        assert_eq!(report.notes.len(), 1, "{:?}", report.notes);
        assert!(
            report.notes[0].starts_with(&format!("reply block {nonce}_3 (text "))
                && report.notes[0].contains("was not sent"),
            "{}",
            report.notes[0]
        );
        // Older output past the snapshot bound is left out, and the newest block is still read.
        let oversized = "old output\n".repeat(MAX_SNAPSHOT_BYTES / 10) + &block(3, "third");
        let report = capture_recovery_snapshot(
            &state,
            &manager,
            DrainOptions::default(),
            &mut routes,
            SnapshotInput {
                text: &oversized,
                truncated: false,
                revision: Some(2),
            },
            &mut control,
        )
        .expect("an oversized snapshot does not fail the pass");
        assert_eq!(report.captured, [(key.clone(), vec![3])]);
        assert_eq!(report.notes, [SNAPSHOT_CUT_NOTE]);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn notes_are_bounded_per_report_and_logged_once_per_process() {
        let mut report = CycleReport::default();
        for index in 0..MAX_REPORT_NOTES + 5 {
            report.note(index);
        }
        assert_eq!(report.notes.len(), MAX_REPORT_NOTES);
        let mut other = CycleReport::default();
        other.note("more");
        report.merge(other);
        assert_eq!(report.notes.len(), MAX_REPORT_NOTES);
        let mut log = NoteLog::default();
        assert!(log.first_time("note 0"));
        assert!(!log.first_time("note 0"));
        for index in 1..MAX_LOGGED_NOTES {
            assert!(log.first_time(&format!("note {index}")));
        }
        assert!(!log.first_time("note 0"));
        // Remembering one more distinct note forgets the oldest.
        assert!(log.first_time("one more"));
        assert!(log.first_time("note 0"));
        assert!(!log.first_time("one more"));
    }

    fn blocked_receive_after_owner_stop(failure_code: &'static str) -> ProviderGenerationError {
        let (state, _, root) = state_with_request();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let entered = Arc::new((Mutex::new(false), Condvar::new()));
        let receive_tracker = Arc::new(ReceiveOperationTracker::default());
        let mut backend = ReceiveFailureTrackingBackend {
            inner: BlockingFailureBackend {
                gate: Arc::clone(&gate),
                entered: Arc::clone(&entered),
                failure_code,
            },
            receive_tracker: Arc::clone(&receive_tracker),
        };
        let cancellation: SharedCancellation =
            Arc::new(Mutex::new(ProviderCancellationRegistry::default()));
        let registration = ProviderCancellationRegistration::install(
            &cancellation,
            Arc::new(GateCancellation {
                gate: Arc::clone(&gate),
            }),
            Arc::clone(&receive_tracker),
            Duration::from_secs(17),
        )
        .expect("register cancellation authority");
        let identity = registration.identity();
        let request = state.subscribe_request().expect("subscription request");
        let mut subscription =
            ChatSubscription::open(&mut backend, &request).expect("blocking subscription");
        let stop = Arc::new(StopState::default());
        let worker_stop = Arc::clone(&stop);
        let worker_cancellation = Arc::clone(&cancellation);
        let worker_receive_tracker = Arc::clone(&receive_tracker);
        let worker = std::thread::spawn(move || {
            chat_runtime::consume_one(&mut subscription, &state).map_err(|error| {
                classify_provider_receive_failure(
                    &worker_stop,
                    &worker_cancellation,
                    identity,
                    &worker_receive_tracker,
                    error,
                )
            })
        });
        let (entered_lock, entered_changed) = &*entered;
        let mut is_entered = entered_lock.lock().expect("entered lock");
        while !*is_entered {
            is_entered = entered_changed.wait(is_entered).expect("entered wait");
        }
        drop(is_entered);
        stop.stop();
        cancel_provider(&cancellation).expect("successful provider cancellation");
        let result = worker.join().expect("join blocked receive");
        assert!(stop.take_cleanup_errors().is_empty());
        drop(registration);
        fs::remove_dir_all(root).expect("cleanup");
        result.expect_err("blocked receive must fail")
    }

    #[test]
    fn owner_stop_interrupting_blocked_next_item_is_expected_cancellation_not_cleanup_failure() {
        assert!(matches!(
            blocked_receive_after_owner_stop(CANCELLED_PROCESS_RECEIVE_CODE),
            ProviderGenerationError::Cancelled
        ));
    }

    #[test]
    fn owner_stop_winning_receive_start_race_is_expected_sticky_cancellation() {
        let (state, _, root) = state_with_request();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let entered = Arc::new((Mutex::new(false), Condvar::new()));
        let receive_tracker = Arc::new(ReceiveOperationTracker::default());
        let mut backend = ReceiveFailureTrackingBackend {
            inner: BlockingFailureBackend {
                gate: Arc::clone(&gate),
                entered,
                failure_code: CANCELLED_PROCESS_RECEIVE_CODE,
            },
            receive_tracker: Arc::clone(&receive_tracker),
        };
        let cancellation: SharedCancellation =
            Arc::new(Mutex::new(ProviderCancellationRegistry::default()));
        let registration = ProviderCancellationRegistration::install(
            &cancellation,
            Arc::new(GateCancellation { gate }),
            Arc::clone(&receive_tracker),
            Duration::from_secs(17),
        )
        .expect("register cancellation authority");
        let request = state.subscribe_request().expect("subscription request");
        let mut subscription =
            ChatSubscription::open(&mut backend, &request).expect("blocking subscription");
        let stop = StopState::default();
        stop.stop();
        cancel_provider(&cancellation).expect("pre-receive cancellation succeeds");
        let error = chat_runtime::consume_one(&mut subscription, &state)
            .expect_err("sticky cancellation refuses the racing receive");
        assert!(matches!(
            classify_provider_receive_failure(
                &stop,
                &cancellation,
                registration.identity(),
                &receive_tracker,
                error,
            ),
            ProviderGenerationError::Cancelled
        ));
        drop(registration);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn cancellation_registered_before_start_interrupts_a_blocked_start() {
        let (state, _, root) = state_with_request();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let entered = Arc::new((Mutex::new(false), Condvar::new()));
        let receive_tracker = Arc::new(ReceiveOperationTracker::default());
        let cancellation: SharedCancellation =
            Arc::new(Mutex::new(ProviderCancellationRegistry::default()));
        let registration = ProviderCancellationRegistration::install(
            &cancellation,
            Arc::new(GateCancellation {
                gate: Arc::clone(&gate),
            }),
            Arc::clone(&receive_tracker),
            Duration::from_secs(8),
        )
        .expect("install cancellation before Start");
        let identity = registration.identity();
        let request = state.subscribe_request().expect("subscription request");
        let worker_gate = Arc::clone(&gate);
        let worker_entered = Arc::clone(&entered);
        let worker_stop = Arc::new(StopState::default());
        let classifier_stop = Arc::clone(&worker_stop);
        let worker_cancellation = Arc::clone(&cancellation);
        let worker = thread::spawn(move || {
            let mut backend = ReceiveFailureTrackingBackend {
                inner: BlockingStartBackend {
                    gate: worker_gate,
                    entered: worker_entered,
                },
                receive_tracker,
            };
            match ChatSubscription::open(&mut backend, &request) {
                Err(error) => classify_provider_start_failure(
                    &classifier_stop,
                    &worker_cancellation,
                    identity,
                    error,
                ),
                Ok(_) => panic!("blocked Start unexpectedly succeeded"),
            }
        });
        let (entered_lock, entered_changed) = &*entered;
        let mut is_entered = entered_lock.lock().expect("entered lock");
        while !*is_entered {
            is_entered = entered_changed.wait(is_entered).expect("entered wait");
        }
        drop(is_entered);

        worker_stop.stop();
        cancel_provider(&cancellation).expect("interrupt blocked Start");
        assert!(matches!(
            worker.join().expect("join Start worker"),
            ProviderGenerationError::Cancelled
        ));
        assert!(
            cancellation
                .lock()
                .expect("registry")
                .active
                .as_ref()
                .is_some_and(|active| active.succeeded),
            "the registered authority owns Start cancellation"
        );
        drop(registration);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn provider_generation_classifies_exact_process_start_retirement_as_cancelled() {
        let (state, _, root) = state_with_request();
        let timeouts = ProcessPhaseTimeouts::new(
            Duration::from_secs(2),
            Duration::from_secs(2),
            Duration::from_secs(2),
            Duration::from_secs(2),
            Duration::from_secs(1),
        )
        .expect("valid process fixture timeouts");
        let stop = Arc::new(StopState::default());
        let cancellation: SharedCancellation =
            Arc::new(Mutex::new(ProviderCancellationRegistry::default()));
        let output_wake: SharedWake = Arc::new(Mutex::new(None));
        let overflowed = Arc::new(AtomicBool::new(false));
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let entered = Arc::new((Mutex::new(false), Condvar::new()));

        let worker_state = state.clone();
        let worker_stop = Arc::clone(&stop);
        let worker_cancellation = Arc::clone(&cancellation);
        let worker_output_wake = Arc::clone(&output_wake);
        let worker_overflowed = Arc::clone(&overflowed);
        let worker_gate = Arc::clone(&gate);
        let worker_entered = Arc::clone(&entered);
        let worker = thread::spawn(move || {
            let (notices, _notice_receiver) = mpsc::sync_channel(PROVIDER_NOTICE_CAPACITY);
            run_provider_generation(
                &worker_state,
                &worker_stop,
                &worker_cancellation,
                &notices,
                &worker_output_wake,
                &worker_overflowed,
                timeouts,
                || {
                    let child = chat_subscription_plugin::process::ProcessPluginChild::spawn(
                        process_start_cancel_command(),
                    )
                    .map_err(|error| ProviderGenerationError::Retryable(error.to_string()))?;
                    let (backend, process_cancellation) = child
                        .connect(timeouts)
                        .map_err(|error| ProviderGenerationError::Retryable(error.to_string()))?;
                    Ok((
                        GatedProcessStartBackend {
                            inner: backend,
                            gate: worker_gate,
                            entered: worker_entered,
                        },
                        process_cancellation,
                    ))
                },
                &|line| panic!("a generation whose Start was cancelled logged: {line}"),
            )
        });

        let (entered_lock, entered_changed) = &*entered;
        let entered_state = entered_lock.lock().expect("Start admission lock");
        let (entered_state, wait) = entered_changed
            .wait_timeout_while(entered_state, Duration::from_secs(5), |entered| !*entered)
            .expect("Start admission wait");
        assert!(
            !wait.timed_out() && *entered_state,
            "Start was not admitted"
        );
        drop(entered_state);

        stop.stop();
        let cancellation_result = cancel_provider(&cancellation);
        let (gate_lock, gate_changed) = &*gate;
        *gate_lock.lock().expect("Start gate lock") = true;
        gate_changed.notify_all();
        cancellation_result.expect("semantic pre-Start Close succeeds");
        assert!(matches!(
            worker.join().expect("join provider generation"),
            Err(ProviderGenerationError::Cancelled)
        ));
        assert!(stop.take_cleanup_errors().is_empty());
        assert!(!overflowed.load(Ordering::SeqCst));
        fs::remove_dir_all(root).expect("cleanup");
    }

    /// A provider backend whose stream opens and then ends at once.
    struct EndingBackend;

    /// The stream an `EndingBackend` opens.
    struct EndingDriver;

    fn unused_cancellation() -> GateCancellation {
        GateCancellation {
            gate: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    impl ChatSubscriptionDriver for EndingDriver {
        fn cancellation(&self) -> Arc<dyn ChatSubscriptionCancellation> {
            Arc::new(unused_cancellation())
        }

        fn next_item(&mut self) -> std::result::Result<Option<SubscriptionItem>, BackendFailure> {
            Ok(None)
        }

        fn acknowledge(
            &mut self,
            _delivery_id: &DeliveryId,
        ) -> std::result::Result<(), BackendFailure> {
            Ok(())
        }
    }

    impl ChatSubscriptionBackend for EndingBackend {
        fn cancellation(&self) -> Arc<dyn ChatSubscriptionCancellation> {
            Arc::new(unused_cancellation())
        }

        fn capabilities(&self) -> BackendCapabilities {
            BackendCapabilities::new(
                "ending-fixture",
                ReplaySupport::Cursor,
                true,
                NonZeroU16::new(1).expect("one"),
                vec![
                    EventKind::MessageCreated,
                    EventKind::Checkpoint,
                    EventKind::Gap,
                    EventKind::Heartbeat,
                ],
            )
            .expect("capabilities")
        }

        fn subscribe(
            &mut self,
            _request: &SubscribeRequest,
        ) -> std::result::Result<Box<dyn ChatSubscriptionDriver>, BackendFailure> {
            Ok(Box::new(EndingDriver))
        }
    }

    #[test]
    fn provider_generation_logs_its_subscription_through_the_given_sink() {
        let (state, _, root) = state_with_request();
        assert!(
            state
                .subscribe_request()
                .expect("subscription request")
                .resume_from()
                .is_some(),
            "the fixture state has a saved cursor"
        );
        let timeouts = ProcessPhaseTimeouts::new(
            Duration::from_secs(2),
            Duration::from_secs(2),
            Duration::from_secs(2),
            Duration::from_secs(2),
            Duration::from_secs(1),
        )
        .expect("valid process fixture timeouts");
        let stop = StopState::default();
        let cancellation: SharedCancellation =
            Arc::new(Mutex::new(ProviderCancellationRegistry::default()));
        let output_wake: SharedWake = Arc::new(Mutex::new(None));
        let overflowed = AtomicBool::new(false);
        let (notices, receiver) = mpsc::sync_channel(PROVIDER_NOTICE_CAPACITY);
        let lines = Mutex::new(Vec::new());
        let ended = run_provider_generation(
            &state,
            &stop,
            &cancellation,
            &notices,
            &output_wake,
            &overflowed,
            timeouts,
            || Ok((EndingBackend, unused_cancellation())),
            &|line| lines.lock().expect("log lines").push(line.to_string()),
        );
        assert!(matches!(ended, Ok(())), "{ended:?}");
        assert_eq!(
            *lines.lock().expect("log lines"),
            ["agentctl: chat provider: subscribed from the saved cursor"]
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert!(stop.take_cleanup_errors().is_empty());
        assert!(!overflowed.load(Ordering::SeqCst));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn successful_start_cancel_does_not_hide_unrelated_start_failure() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let cancellation: SharedCancellation =
            Arc::new(Mutex::new(ProviderCancellationRegistry::default()));
        let registration = ProviderCancellationRegistration::install(
            &cancellation,
            Arc::new(GateCancellation { gate }),
            Arc::new(ReceiveOperationTracker::default()),
            Duration::from_secs(8),
        )
        .expect("register cancellation authority");
        let stop = StopState::default();
        stop.stop();
        cancel_provider(&cancellation).expect("record successful cancellation");
        let error = classify_provider_start_failure(
            &stop,
            &cancellation,
            registration.identity(),
            SubscriptionError::Backend(
                BackendFailure::new(
                    "provider_start_failed",
                    "provider rejected Start independently",
                    true,
                )
                .expect("backend failure"),
            ),
        );
        match error {
            ProviderGenerationError::Retryable(detail) => {
                assert!(detail.contains("provider_start_failed"));
            }
            other => panic!("unrelated Start failure was hidden as {other:?}"),
        }
    }

    #[test]
    fn concurrent_unrelated_backend_failure_during_owner_stop_remains_visible() {
        let error = blocked_receive_after_owner_stop("provider_transport_failed");
        match error {
            ProviderGenerationError::Retryable(detail) => {
                assert!(detail.contains("provider_transport_failed"));
            }
            other => panic!("unrelated provider failure was hidden as {other:?}"),
        }
    }

    #[test]
    fn successful_cancel_does_not_hide_invalid_io_or_commit_failures() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let cancellation: SharedCancellation =
            Arc::new(Mutex::new(ProviderCancellationRegistry::default()));
        let receive_tracker = Arc::new(ReceiveOperationTracker::default());
        let registration = ProviderCancellationRegistration::install(
            &cancellation,
            Arc::new(GateCancellation {
                gate: Arc::clone(&gate),
            }),
            Arc::clone(&receive_tracker),
            Duration::from_secs(17),
        )
        .expect("register cancellation authority");
        let stop = StopState::default();
        stop.stop();
        cancel_provider(&cancellation).expect("successful provider cancellation");

        let invalid = classify_provider_receive_failure(
            &stop,
            &cancellation,
            registration.identity(),
            &receive_tracker,
            ChatRuntimeError::Invalid("invalid admitted batch".to_owned()),
        );
        assert!(matches!(invalid, ProviderGenerationError::Retryable(_)));

        let io = classify_provider_receive_failure(
            &stop,
            &cancellation,
            registration.identity(),
            &receive_tracker,
            ChatRuntimeError::Io(io::Error::other("durable write failed")),
        );
        assert!(matches!(io, ProviderGenerationError::Retryable(_)));

        let commit = classify_provider_receive_failure(
            &stop,
            &cancellation,
            registration.identity(),
            &receive_tracker,
            ChatRuntimeError::Subscription(SubscriptionError::Backend(
                BackendFailure::new(
                    CANCELLED_PROCESS_RECEIVE_CODE,
                    "commit receipt was not confirmed",
                    true,
                )
                .expect("backend failure"),
            )),
        );
        assert!(matches!(commit, ProviderGenerationError::Retryable(_)));
    }

    #[test]
    fn acknowledgement_helper_and_coordinator_delivery_run_concurrently() {
        let (state, key, root) = state_with_request();
        let helper = root.join("ack-helper");
        fs::copy(
            fs::canonicalize("/bin/sh").expect("canonical shell"),
            &helper,
        )
        .expect("copy acknowledgement helper");
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).expect("helper mode");
        let helper_entered = root.join("ack-helper-entered");
        let helper_release = root.join("ack-helper-release");
        let script = r#"
IFS= read -r request || exit 2
printf entered > "$1" || exit 3
while [ ! -f "$2" ]; do sleep 0.01; done
case "$request" in
  *'"action":"ensure_reaction"'*) ;;
  *) exit 4 ;;
esac
id=${request#*\"id\":\"}
id=${id%%\"*}
printf '{"version":1,"id":"%s","action":"ensure_reaction","ok":true,"receipt":{"reaction_id":"spaces/example/messages/one/reactions/ack","already_present":false}}\n' "$id"
"#;
        let mut transport = Some(
            CommandOutboundTransport::new(
                helper,
                vec![
                    std::ffi::OsString::from("-c"),
                    std::ffi::OsString::from(script),
                    std::ffi::OsString::from("agentctl-chat-ack-helper"),
                    helper_entered.clone().into_os_string(),
                    helper_release.clone().into_os_string(),
                ],
                &[],
                Duration::from_secs(2),
                Duration::from_millis(50),
            )
            .expect("pin acknowledgement helper"),
        );
        let coordinator = AckReleasingDelivery {
            states: Mutex::new(BTreeMap::new()),
            prompts: Mutex::new(Vec::new()),
            helper_entered: helper_entered.clone(),
            helper_release: helper_release.clone(),
            submit_started: AtomicBool::new(false),
            submit_thread: Mutex::new(None),
        };
        let calling_thread = thread::current().id();
        let mut control = PassControl {
            transport: &mut transport,
            stop: None,
        };

        let report = process_keys_with_delivery(
            &state,
            &coordinator,
            DrainOptions::default(),
            std::slice::from_ref(&key),
            &mut control,
        )
        .expect("concurrent acknowledgement and delivery pass");

        assert!(coordinator.submit_started.load(Ordering::SeqCst));
        assert_eq!(
            *coordinator.submit_thread.lock().expect("submit thread"),
            Some(calling_thread),
            "coordinator submission must remain on the calling thread"
        );
        assert!(helper_entered.exists());
        assert!(helper_release.exists());
        assert_eq!(coordinator.prompts.lock().expect("prompts").len(), 1);
        assert_eq!(report.acknowledged, vec![key.clone()]);
        assert_eq!(report.delivered, vec![key]);
        assert!(
            report.errors.is_empty(),
            "unexpected errors: {:?}",
            report.errors
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn stop_during_ack_waits_for_admitted_ack_and_never_starts_pending_reply() {
        let (state, key, root) = state_with_request();
        let route = state
            .next_reply_route(&key)
            .expect("read reply route")
            .expect("active reply route");
        let fenced = format!(
            "<CHAT_REPLY_{}>\nresponse body\n</CHAT_REPLY_{}>",
            route.identifier, route.identifier
        );
        state
            .capture_snapshot(&fenced)
            .expect("capture one pending response");

        let helper = root.join("ack-and-reply-helper");
        fs::copy(
            fs::canonicalize("/bin/sh").expect("canonical shell"),
            &helper,
        )
        .expect("copy outbound helper");
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).expect("helper mode");
        let ack_entered = root.join("ack-entered");
        let ack_release = root.join("ack-release");
        let reply_started = root.join("reply-started");
        let script = r#"
IFS= read -r request || exit 2
id=${request#*\"id\":\"}
id=${id%%\"*}
case "$request" in
  *'"action":"ensure_reaction"'*)
    printf entered > "$1" || exit 3
    while [ ! -f "$2" ]; do sleep 0.01; done
    printf '{"version":1,"id":"%s","action":"ensure_reaction","ok":true,"receipt":{"reaction_id":"spaces/example/messages/one/reactions/ack","already_present":false}}\n' "$id"
    ;;
  *'"action":"send"'*)
    printf started > "$3" || exit 4
    printf '{"version":1,"id":"%s","action":"send","ok":true,"receipt":{"message_id":"spaces/example/messages/reply"}}\n' "$id"
    ;;
  *) exit 5 ;;
esac
"#;
        let transport = CommandOutboundTransport::new(
            helper,
            vec![
                std::ffi::OsString::from("-c"),
                std::ffi::OsString::from(script),
                std::ffi::OsString::from("agentctl-chat-ack-stop-helper"),
                ack_entered.clone().into_os_string(),
                ack_release.clone().into_os_string(),
                reply_started.clone().into_os_string(),
            ],
            &[],
            Duration::from_secs(2),
            Duration::from_millis(50),
        )
        .expect("pin outbound helper");
        let coordinator = Arc::new(RecordingDelivery::default());
        let stop = Arc::new(StopState::default());
        let worker_state = state.clone();
        let worker_key = key.clone();
        let worker_coordinator = Arc::clone(&coordinator);
        let worker_stop = Arc::clone(&stop);
        let (finished, finished_receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut transport = Some(transport);
            let mut control = PassControl {
                transport: &mut transport,
                stop: Some(&worker_stop),
            };
            let result = process_keys_with_delivery(
                &worker_state,
                worker_coordinator.as_ref(),
                DrainOptions::default(),
                &[worker_key],
                &mut control,
            );
            finished.send(result).expect("publish pass result");
        });

        let progress_deadline = Instant::now() + Duration::from_secs(1);
        while !ack_entered.exists() || coordinator.prompts.lock().expect("prompts").is_empty() {
            assert!(
                Instant::now() < progress_deadline,
                "ack and coordinator delivery did not overlap"
            );
            thread::yield_now();
        }
        assert!(
            !reply_started.exists(),
            "pending reply transport started before ACK reconciliation"
        );
        assert!(
            finished_receiver
                .recv_timeout(Duration::from_millis(50))
                .is_err(),
            "pass returned without joining the admitted ACK"
        );

        stop.stop();
        assert!(
            finished_receiver
                .recv_timeout(Duration::from_millis(50))
                .is_err(),
            "owner stop detached or cancelled the admitted ACK"
        );
        fs::write(&ack_release, b"release").expect("release admitted ACK");
        let report = finished_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("pass returns after ACK reconciliation")
            .expect("stopped pass remains well-formed");
        worker.join().expect("pass worker joins");
        assert_eq!(report.acknowledged, vec![key.clone()]);
        assert_eq!(report.delivered, vec![key]);
        assert!(report.sent.is_empty());
        assert!(report.more_work);
        assert!(
            !reply_started.exists(),
            "stop after admitted ACK must not launch the pending response helper"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn provider_join_ceiling_tracks_selected_manifest_and_connection_phase() {
        let selected = ProcessPhaseTimeouts::new(
            Duration::from_secs(10),
            Duration::from_secs(180),
            Duration::from_secs(30),
            Duration::from_secs(60),
            Duration::from_secs(2),
        )
        .expect("selected timeout fixture");
        assert_eq!(
            connected_provider_join_timeout(selected),
            Duration::from_secs(69)
        );
        assert_eq!(
            pending_provider_join_timeout(selected),
            Duration::from_secs(79)
        );
        assert!(connected_provider_join_timeout(selected) < Duration::from_secs(75));
        assert!(pending_provider_join_timeout(selected) > Duration::from_secs(75));

        let maximum = ProcessPhaseTimeouts::new(
            Duration::from_secs(30),
            Duration::from_secs(180),
            Duration::from_secs(60),
            Duration::from_secs(120),
            Duration::from_secs(10),
        )
        .expect("maximum timeout fixture");
        assert_eq!(
            connected_provider_join_timeout(maximum),
            Duration::from_secs(137)
        );
        assert_eq!(
            pending_provider_join_timeout(maximum),
            Duration::from_secs(167)
        );
    }

    #[test]
    fn service_join_ceiling_includes_the_configured_outbound_operation() {
        let configuration = BridgeConfiguration {
            subscription_plugin: "fixture".to_owned(),
            subscription_environment: Vec::new(),
            channel_ids: vec!["spaces/example".to_owned()],
            allowed_senders: vec!["users/owner".to_owned()],
            agent_name: "coordinator".to_owned(),
            agent_label: "coordinator".to_owned(),
            outbound_enabled: true,
            ack_reaction: Some("🤖".to_owned()),
            backend_configuration: None,
            outbound_command: Some(chat_runtime::OutboundCommandConfiguration {
                executable: "/fixture/outbound-helper".into(),
                arguments: Vec::new(),
                environment: Vec::new(),
                timeout_millis: 30_000,
                shutdown_grace_millis: 30_000,
            }),
        };
        let outbound = configured_outbound_join_timeout(&configuration);
        assert_eq!(outbound, Duration::from_secs(67));

        let fast_provider = ProcessPhaseTimeouts::new(
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::ZERO,
        )
        .expect("fast provider fixture");
        assert_eq!(
            service_join_timeout(pending_provider_join_timeout(fast_provider), outbound),
            outbound,
            "the service must continue owning an admitted outbound operation after provider cleanup"
        );

        let private_provider = ProcessPhaseTimeouts::new(
            Duration::from_secs(10),
            Duration::from_secs(180),
            Duration::from_secs(30),
            Duration::from_secs(60),
            Duration::from_secs(2),
        )
        .expect("selected provider fixture");
        assert_eq!(
            service_join_timeout(pending_provider_join_timeout(private_provider), outbound),
            Duration::from_secs(79),
            "the larger provider window remains authoritative"
        );

        let mut no_outbound = configuration;
        no_outbound.outbound_command = None;
        assert_eq!(
            configured_outbound_join_timeout(&no_outbound),
            Duration::ZERO
        );
    }

    #[test]
    fn provider_generation_policy_is_pinned_exactly_before_reservation() {
        let selected = ProcessPhaseTimeouts::new(
            Duration::from_secs(10),
            Duration::from_secs(20),
            Duration::from_secs(30),
            Duration::from_secs(40),
            Duration::from_secs(2),
        )
        .expect("selected policy");
        validate_selected_process_timeouts(selected, selected).expect("exact policy remains valid");

        let changed = ProcessPhaseTimeouts::new(
            Duration::from_secs(11),
            Duration::from_secs(20),
            Duration::from_secs(30),
            Duration::from_secs(40),
            Duration::from_secs(2),
        )
        .expect("changed policy");
        let error = validate_selected_process_timeouts(selected, changed)
            .expect_err("a live service cannot adopt a changed manifest deadline");
        assert!(matches!(error, ProviderGenerationError::Fatal(_)));
        assert!(format!("{error:?}").contains("restart the service"));
    }

    #[test]
    fn provider_join_ceiling_falls_back_to_pending_between_generations() {
        let cancellation: SharedCancellation =
            Arc::new(Mutex::new(ProviderCancellationRegistry::default()));
        let connected = Duration::from_secs(19);
        let pending = Duration::from_secs(29);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let registration = ProviderCancellationRegistration::install(
            &cancellation,
            Arc::new(GateCancellation {
                gate: Arc::clone(&gate),
            }),
            Arc::new(ReceiveOperationTracker::default()),
            connected,
        )
        .expect("install connected generation");
        drop(registration);
        let stop = StopState::default();
        let deadline = begin_service_stop(&stop, &cancellation, pending, Duration::ZERO);
        let (origin, recorded) = stop
            .shutdown_window
            .lock()
            .expect("shutdown window")
            .expect("stop window");
        assert_eq!(
            recorded.duration_since(origin),
            pending,
            "a stale connected-generation window must not govern the next Hello"
        );
        assert_eq!(deadline, recorded);

        let active_registry: SharedCancellation =
            Arc::new(Mutex::new(ProviderCancellationRegistry::default()));
        let _active = ProviderCancellationRegistration::install(
            &active_registry,
            Arc::new(GateCancellation { gate }),
            Arc::new(ReceiveOperationTracker::default()),
            connected,
        )
        .expect("install second connected generation");
        let active_stop = StopState::default();
        begin_service_stop(&active_stop, &active_registry, pending, Duration::ZERO);
        let (active_origin, active_deadline) = active_stop
            .shutdown_window
            .lock()
            .expect("active shutdown window")
            .expect("active stop window");
        assert_eq!(
            active_deadline.duration_since(active_origin),
            connected,
            "a connected generation retains its selected Close-based window"
        );
    }

    #[test]
    fn stop_and_provider_reservation_share_one_launch_admission_handshake() {
        let cancellation: SharedCancellation =
            Arc::new(Mutex::new(ProviderCancellationRegistry::default()));
        let stop = StopState::default();
        let pending = Duration::from_secs(29);
        let registration = ProviderCancellationRegistration::reserve(&cancellation, pending)
            .expect("provider reserves before stop");
        let stop_deadline = begin_service_stop(
            &stop,
            &cancellation,
            Duration::from_secs(19),
            Duration::ZERO,
        );
        let (stop_origin, recorded_deadline) = stop
            .shutdown_window
            .lock()
            .expect("shutdown window")
            .expect("stop installed shutdown window");
        assert_eq!(stop_deadline, recorded_deadline);
        assert_eq!(recorded_deadline.duration_since(stop_origin), pending);

        let connect_called = AtomicBool::new(false);
        let error = connect_provider_unless_stopped(
            &stop,
            pending,
            || -> Result<(), ProviderGenerationError> {
                connect_called.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .expect_err("a stopped service must not launch the selected plugin");
        assert!(matches!(error, ProviderGenerationError::Cancelled));
        assert!(
            !connect_called.load(Ordering::SeqCst),
            "provider connect (and therefore child launch) ran after stop"
        );
        drop(registration);
        assert!(matches!(
            ProviderCancellationRegistration::reserve(&cancellation, pending),
            Err(ProviderReservationError::Stopping)
        ));

        let stop_first_registry: SharedCancellation =
            Arc::new(Mutex::new(ProviderCancellationRegistry::default()));
        let stop_first = StopState::default();
        begin_service_stop(&stop_first, &stop_first_registry, pending, Duration::ZERO);
        assert!(matches!(
            ProviderCancellationRegistration::reserve(&stop_first_registry, pending),
            Err(ProviderReservationError::Stopping)
        ));
    }

    #[test]
    fn stop_during_hello_keeps_the_published_hello_and_cleanup_budget() {
        let timeouts = ProcessPhaseTimeouts::new(
            Duration::from_secs(30),
            Duration::from_secs(180),
            Duration::from_secs(60),
            Duration::from_secs(1),
            Duration::ZERO,
        )
        .expect("valid slow-Hello fixture");
        let pending = pending_provider_join_timeout(timeouts);
        let connected = connected_provider_join_timeout(timeouts);
        assert_eq!(pending, Duration::from_secs(38));
        assert_eq!(connected, Duration::from_secs(8));

        let cancellation: SharedCancellation =
            Arc::new(Mutex::new(ProviderCancellationRegistry::default()));
        let mut registration = ProviderCancellationRegistration::reserve(&cancellation, pending)
            .expect("publish generation before Hello");
        assert!(
            cancellation.lock().expect("registry").active.is_none(),
            "Hello has no cancellation authority yet"
        );

        let stop = StopState::default();
        let hello_stop_deadline =
            begin_service_stop(&stop, &cancellation, connected, Duration::ZERO);
        let (stop_origin, recorded_deadline) = stop
            .shutdown_window
            .lock()
            .expect("shutdown window")
            .expect("stop installed shutdown window");
        assert_eq!(recorded_deadline.duration_since(stop_origin), pending);
        assert_eq!(
            recorded_deadline.duration_since(stop_origin + timeouts.hello()),
            connected,
            "a valid Hello consuming its full 30 seconds still leaves the complete cleanup budget"
        );
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let authority: Arc<dyn ChatSubscriptionCancellation> =
            Arc::new(RegistryCheckingCancellation {
                registry: Arc::downgrade(&cancellation),
                gate: Arc::clone(&gate),
            });
        let receive_tracker = Arc::new(ReceiveOperationTracker::default());
        let stop_was_pending = registration
            .activate(
                Arc::clone(&authority),
                Arc::clone(&receive_tracker),
                connected,
            )
            .expect("install cancellation after Hello");
        assert!(
            stop_was_pending,
            "stop intent registered during Hello must remain sticky through activation"
        );
        assert_eq!(
            begin_service_stop(&stop, &cancellation, connected, Duration::ZERO),
            hello_stop_deadline,
            "activation cannot shorten or restart the Hello-era absolute deadline"
        );
        cancel_provider_generation(
            &cancellation,
            registration.identity(),
            &authority,
            &receive_tracker,
        )
        .expect("cancel before Start without holding the registry lock");
        assert!(*gate.0.lock().expect("gate"));
    }

    #[test]
    fn repeated_stop_requests_share_one_absolute_shutdown_origin() {
        let stop = StopState::default();
        let first = stop.stop_with_timeout(Duration::from_secs(69));
        thread::sleep(Duration::from_millis(5));
        let repeated = stop.stop_with_timeout(Duration::from_secs(69));
        let shorter = stop.stop_with_timeout(Duration::from_secs(17));
        let extended = stop.stop_with_timeout(Duration::from_secs(137));
        assert_eq!(repeated, first, "repeated stop cannot reset the deadline");
        assert_eq!(
            shorter, first,
            "a shorter later budget cannot reset the deadline"
        );
        assert_eq!(extended.duration_since(first), Duration::from_secs(68));
        assert_eq!(stop.shutdown_deadline(), Some(extended));
    }

    #[test]
    fn stopped_owner_launches_no_further_outbound_helpers() {
        let (state, key, root) = state_with_request();
        let helper = root.join("helper");
        fs::copy(
            fs::canonicalize("/bin/sh").expect("canonical shell"),
            &helper,
        )
        .expect("copy helper");
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).expect("helper mode");
        let marker = root.join("launched");
        let mut transport = Some(
            CommandOutboundTransport::new(
                helper,
                vec![
                    std::ffi::OsString::from("-c"),
                    std::ffi::OsString::from(format!("touch '{}'; exit 9", marker.display())),
                ],
                &[],
                Duration::from_secs(1),
                Duration::ZERO,
            )
            .expect("pin helper"),
        );
        let client = HerdrClient::with_executable("direct", Path::new("/missing/herdr"))
            .expect("construct client");
        let manager = ManagedAgents::new(&client, &root.join("registry")).expect("manager");
        let stop = StopState::default();
        stop.stop();
        let mut control = PassControl {
            transport: &mut transport,
            stop: Some(&stop),
        };
        let report = process_keys(
            &state,
            &manager,
            DrainOptions::default(),
            &[key],
            &mut control,
        )
        .expect("cancelled pass");
        assert!(report.more_work);
        assert!(!marker.exists());
        fs::remove_dir_all(root).expect("cleanup");
    }

    /// The scroll geometry herdr reports for a pane that keeps no scrollback, such as a Claude
    /// Code pane.
    const SCREEN_ONLY: crate::client::PaneScroll = crate::client::PaneScroll {
        offset_from_bottom: 0,
        max_offset_from_bottom: 0,
        viewport_rows: 52,
    };

    /// A new bridge state, in a private directory under the fixture's root, for the fixture's
    /// agent `worker`.
    fn worker_bridge_state(
        fixture: &crate::subagents::tests::Fixture,
        outbound_enabled: bool,
    ) -> (std::path::PathBuf, BridgeState) {
        worker_bridge_state_with(fixture, outbound_enabled, None)
    }

    /// A new bridge state with outbound on for the fixture's agent `worker`, as
    /// `worker_bridge_state` makes one, with the outbound helper that `chat tick` requires. The
    /// helper fails every operation, so a tick that must send nothing never gets to use it.
    fn worker_tick_state(
        fixture: &crate::subagents::tests::Fixture,
    ) -> (std::path::PathBuf, BridgeState) {
        let helper = fixture.root.join("outbound-helper");
        fs::copy(
            fs::canonicalize("/bin/sh").expect("canonical shell"),
            &helper,
        )
        .expect("copy native helper");
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).expect("private helper");
        worker_bridge_state_with(
            fixture,
            true,
            Some(chat_runtime::OutboundCommandConfiguration {
                executable: helper,
                arguments: vec!["-c".to_owned(), "exit 3".to_owned()],
                environment: Vec::new(),
                timeout_millis: 1_000,
                shutdown_grace_millis: 0,
            }),
        )
    }

    fn worker_bridge_state_with(
        fixture: &crate::subagents::tests::Fixture,
        outbound_enabled: bool,
        outbound_command: Option<chat_runtime::OutboundCommandConfiguration>,
    ) -> (std::path::PathBuf, BridgeState) {
        let root = fixture.root.join("chat");
        fs::create_dir(&root).expect("create state root");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("private state");
        let state = BridgeState::initialize(
            &root,
            BridgeConfiguration {
                subscription_plugin: "fixture".to_owned(),
                subscription_environment: Vec::new(),
                channel_ids: vec!["spaces/example".to_owned()],
                allowed_senders: vec!["users/owner".to_owned()],
                agent_name: "worker".to_owned(),
                agent_label: "worker".to_owned(),
                outbound_enabled,
                ack_reaction: None,
                backend_configuration: None,
                outbound_command,
            },
        )
        .expect("initialize state");
        (root, state)
    }

    /// A stand-in for the Herdr binary and server the owner loop talks to. Its
    /// `status server --json` names a socket on which a thread acknowledges each output
    /// subscription, sends it the events `events` builds for the subscribed pane, and keeps the
    /// connection open until `release` is sent or dropped. `subscriptions` counts the
    /// subscriptions it has acknowledged.
    struct OwnerLoopHerdr {
        client: HerdrClient,
        release: mpsc::Sender<()>,
        server: thread::JoinHandle<()>,
        subscriptions: Arc<AtomicU64>,
    }

    fn owner_loop_herdr(root: &Path, name: &str, events: fn(&str) -> Vec<Value>) -> OwnerLoopHerdr {
        owner_loop_herdr_every(root, name, events, None)
    }

    /// `owner_loop_herdr`, which also sends the events again on every open subscription each
    /// `every`, when set.
    fn owner_loop_herdr_every(
        root: &Path,
        name: &str,
        events: fn(&str) -> Vec<Value>,
        every: Option<Duration>,
    ) -> OwnerLoopHerdr {
        owner_loop_herdr_arming(root, name, events, every, None)
    }

    /// `owner_loop_herdr_every`, which also sets `arm`, when given, just before it acknowledges
    /// each subscription. Given the test double's `fail_pane_info_once`, it makes the owner loop's
    /// next `pane_info` call after it subscribes fail.
    fn owner_loop_herdr_arming(
        root: &Path,
        name: &str,
        events: fn(&str) -> Vec<Value>,
        every: Option<Duration>,
        arm: Option<Arc<AtomicBool>>,
    ) -> OwnerLoopHerdr {
        let (socket, client) = herdr_stand_in(root, name);
        let listener = UnixListener::bind(&socket).expect("bind herdr events");
        listener
            .set_nonblocking(true)
            .expect("poll for herdr event clients");
        let (release, released) = mpsc::channel();
        let subscriptions = Arc::new(AtomicU64::new(0));
        let acknowledged = Arc::clone(&subscriptions);
        let server = thread::spawn(move || {
            let mut connections = Vec::<(UnixStream, String)>::new();
            let mut sent_at = Instant::now();
            while released.try_recv() == Err(mpsc::TryRecvError::Empty) {
                if every.is_some_and(|every| sent_at.elapsed() >= every) {
                    // A write fails once the loop has dropped that subscription.
                    for (connection, pane) in &mut connections {
                        for event in events(pane) {
                            let _ = writeln!(connection, "{event}");
                        }
                    }
                    sent_at = Instant::now();
                }
                let mut connection = match listener.accept() {
                    Ok((connection, _)) => connection,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(error) => panic!("accept herdr event client: {error}"),
                };
                connection
                    .set_nonblocking(false)
                    .expect("blocking herdr event connection");
                let mut request = String::new();
                BufReader::new(
                    connection
                        .try_clone()
                        .expect("clone herdr event connection"),
                )
                .read_line(&mut request)
                .expect("read herdr event subscription");
                let request: Value =
                    serde_json::from_str(&request).expect("decode herdr event subscription");
                let pane = request["params"]["subscriptions"][0]["pane_id"]
                    .as_str()
                    .expect("subscribed pane")
                    .to_owned();
                if let Some(arm) = &arm {
                    arm.store(true, AtomicOrdering::SeqCst);
                }
                writeln!(
                    connection,
                    "{}",
                    json!({"id": request["id"], "result": {"type": "subscription_started"}})
                )
                .expect("acknowledge herdr event subscription");
                acknowledged.fetch_add(1, AtomicOrdering::SeqCst);
                for event in events(&pane) {
                    writeln!(connection, "{event}").expect("write herdr event");
                }
                connections.push((connection, pane));
            }
        });
        OwnerLoopHerdr {
            client,
            release,
            server,
            subscriptions,
        }
    }

    /// `owner_loop_herdr` for a Herdr whose socket no server listens on, so every output
    /// subscription the owner loop tries fails to connect.
    fn unreachable_owner_loop_herdr(root: &Path, name: &str) -> OwnerLoopHerdr {
        let (_, client) = herdr_stand_in(root, name);
        let (release, released) = mpsc::channel::<()>();
        OwnerLoopHerdr {
            client,
            release,
            server: thread::spawn(move || {
                let _ = released.recv();
            }),
            subscriptions: Arc::new(AtomicU64::new(0)),
        }
    }

    /// A client for a stand-in Herdr binary named `name` under `root`, whose
    /// `status server --json` names the socket returned with it.
    fn herdr_stand_in(root: &Path, name: &str) -> (std::path::PathBuf, HerdrClient) {
        let socket = root.join(format!("{name}.sock"));
        let executable = root.join(format!("{name}-herdr"));
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{}'\n",
                json!({"running": true, "compatible": true, "socket": socket})
            ),
        )
        .expect("write herdr stand-in");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
            .expect("herdr stand-in mode");
        let client = HerdrClient::with_executable("direct", &executable).expect("herdr client");
        (socket, client)
    }

    /// Admit one request from the owner to `state` and deliver its prompt. Returns the request's
    /// key and the prompt.
    fn delivered_worker_request(state: &BridgeState) -> (String, String) {
        let key = admitted_worker_request(state);
        let delivery = RecordingDelivery::default();
        chat_runtime::deliver_request_with(state, &delivery, &key, DrainOptions::default())
            .expect("deliver request");
        assert!(state.pending_work_keys().expect("pending work").is_empty());
        let prompt = delivery.prompts.lock().expect("prompts").remove(0);
        (key, prompt)
    }

    /// Admit one request from the owner to `state`, and return its key.
    fn admitted_worker_request(state: &BridgeState) -> String {
        admitted_worker_message(state, 1, "one")
    }

    /// Admit the owner's message `name`, in a thread of its own, as provider event `sequence`,
    /// and return the key of its request. `name` and `sequence` must be new to `state`.
    fn admitted_worker_message(state: &BridgeState, sequence: u64, name: &str) -> String {
        let message = InboundMessage::new(
            ChannelId::new("spaces/example").expect("channel"),
            MessageId::new(format!("spaces/example/messages/{name}")).expect("message"),
            ThreadId::new(format!("spaces/example/threads/{name}")).expect("thread"),
            SenderId::new("users/owner").expect("sender"),
            "request",
            "2026-09-30T12:00:00Z",
            false,
        )
        .expect("inbound message");
        let (cursor, delivery) = if sequence == 1 {
            ("cursor".to_owned(), "delivery".to_owned())
        } else {
            (format!("cursor-{sequence}"), format!("delivery-{sequence}"))
        };
        let batch = DeliveryBatch::new(
            EventSequence::new(sequence).expect("sequence"),
            ProviderCursor::new(cursor).expect("cursor"),
            DeliveryId::new(delivery).expect("delivery"),
            vec![CommittableEvent::message_created(message)],
        )
        .expect("delivery batch");
        state
            .admit_batch(&batch)
            .expect("admit request")
            .new_request_keys
            .remove(0)
    }

    /// Run the owner loop for `state`'s agent until the fixture's fake Herdr client has served
    /// `reads` pane reads, let it run on briefly so a further read is recorded too, stop it, and
    /// return the source of every read it made.
    fn owner_loop_read_sources(
        fixture: &crate::subagents::tests::Fixture,
        state: &BridgeState,
        herdr: &OwnerLoopHerdr,
        reconciliation_interval: Duration,
        reads: usize,
    ) -> Vec<String> {
        let read_sources = &fixture.client.read_sources;
        read_sources.lock().expect("read sources").clear();
        run_owner_loop_until(
            fixture,
            state,
            herdr,
            reconciliation_interval,
            Duration::from_secs(10),
            || read_sources.lock().expect("read sources").len() >= reads,
        );
        let sources = read_sources.lock().expect("read sources");
        sources.clone()
    }

    /// Run the owner loop for `state`'s agent until `done`, called every 5 ms from another
    /// thread, returns true or `limit` has passed, let it run on for 300 ms, and stop it.
    fn run_owner_loop_until(
        fixture: &crate::subagents::tests::Fixture,
        state: &BridgeState,
        herdr: &OwnerLoopHerdr,
        reconciliation_interval: Duration,
        limit: Duration,
        mut done: impl FnMut() -> bool + Send,
    ) {
        run_owner_loop_notified(
            fixture,
            state,
            herdr,
            reconciliation_interval,
            limit,
            move |_, _| done(),
        );
    }

    /// `run_owner_loop_until`, which also gives `done` a function that sends the loop a provider
    /// notice as the provider thread of `chat run` does, and one that tells the loop that notices
    /// were lost, as that thread does when the notice channel is full.
    fn run_owner_loop_notified(
        fixture: &crate::subagents::tests::Fixture,
        state: &BridgeState,
        herdr: &OwnerLoopHerdr,
        reconciliation_interval: Duration,
        limit: Duration,
        mut done: impl FnMut(&dyn Fn(ProviderNotice), &dyn Fn()) -> bool + Send,
    ) {
        let manager = fixture.manager();
        let stop = StopState::default();
        let cancellation: SharedCancellation = Arc::default();
        let output_wake: SharedWake = Arc::default();
        let overflowed = AtomicBool::new(false);
        let (notices, notice_receiver) = mpsc::sync_channel(PROVIDER_NOTICE_CAPACITY);
        let mut transport = None;
        thread::scope(|scope| {
            scope.spawn(|| {
                let notify = |notice| send_notice(&notices, notice, &output_wake, &overflowed);
                let overflow = || {
                    overflowed.store(true, AtomicOrdering::SeqCst);
                    wake_output(&output_wake);
                };
                let deadline = Instant::now() + limit;
                while !done(&notify, &overflow) && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(5));
                }
                thread::sleep(Duration::from_millis(300));
                stop.stop();
                wake_output(&output_wake);
            });
            let result = run_owner_loop(
                state,
                &herdr.client,
                &manager,
                ServiceOptions {
                    delivery: DrainOptions::default(),
                    reconciliation_interval,
                },
                &stop,
                &cancellation,
                &output_wake,
                &overflowed,
                &notice_receiver,
                &mut transport,
            );
            // The loop ends at its next check of the stop, or with a cancelled Herdr call when
            // the stop interrupts one, as it does in `chat run`.
            if let Err(error) = result {
                assert!(
                    stop.is_stopped() && error.to_string().contains("cancelled"),
                    "owner loop failed: {error}"
                );
            }
        });
    }

    #[test]
    fn every_recovery_read_of_the_owner_loop_takes_only_the_screen_of_a_pane_without_scrollback() {
        // Herdr answers a read of more rows than a Claude Code pane's screen by scrolling the
        // pane's own view up and joining the screens it sees, so every snapshot read the service
        // makes of such a pane must take the screen. The loop reads at startup, at each
        // reconciliation, when the agent settles, and after an output event names a reply ID it
        // does not know. The fixture's fake client records the source of each read.
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        *fixture.client.scroll.lock().expect("scroll") = Some(SCREEN_ONLY);
        let (_, state) = worker_bridge_state(&fixture, true);
        // One open request gives the loop a reply route to subscribe for. It is delivered
        // first, so the loop has no prompt to deliver: delivery reads the screen for checks of
        // its own, which would hide a snapshot read among them.
        delivered_worker_request(&state);

        // With no reconciliation due within the hour, the loop reads once at startup, once when
        // the agent settles, and once after the unknown reply ID.
        let events = owner_loop_herdr(&fixture.root, "events", |pane| {
            vec![
                json!({
                    "event": "pane.agent_status_changed",
                    "data": {"pane_id": pane, "agent_status": "idle"},
                }),
                json!({
                    "event": "pane.output_matched",
                    "data": {
                        "pane_id": pane,
                        "matched_line": "</CHAT_REPLY_unknownnonce00_1>",
                        "read": {
                            "pane_id": pane,
                            "workspace_id": "workspace",
                            "tab_id": "tab",
                            "source": "recent_unwrapped",
                            "format": "text",
                            "text": "</CHAT_REPLY_unknownnonce00_1>",
                            "revision": 1,
                            "truncated": false,
                        },
                    },
                }),
            ]
        });
        assert_eq!(
            owner_loop_read_sources(&fixture, &state, &events, Duration::from_secs(3_600), 3),
            ["visible"; 3]
        );
        // With no events, every read after the startup one is a reconciliation.
        let quiet = owner_loop_herdr(&fixture.root, "quiet", |_| Vec::new());
        let sources =
            owner_loop_read_sources(&fixture, &state, &quiet, Duration::from_millis(50), 3);
        assert!(
            sources.len() >= 3 && sources.iter().all(|source| source == "visible"),
            "{sources:?}"
        );
        for herdr in [events, quiet] {
            drop(herdr.release);
            herdr.server.join().expect("herdr stand-in");
        }
    }

    #[test]
    fn the_owner_loop_reads_each_further_reply_under_one_alias_on_the_saturated_poll() {
        further_replies_under_one_alias_on_the_saturated_poll(false, SCREEN_ONLY);
    }

    #[test]
    fn after_an_output_event_the_owner_loop_reads_further_replies_on_the_saturated_poll() {
        further_replies_under_one_alias_on_the_saturated_poll(true, SCREEN_ONLY);
    }

    #[test]
    fn the_saturated_poll_reads_a_pane_with_scrollback_as_the_other_reads_of_the_owner_loop_do() {
        further_replies_under_one_alias_on_the_saturated_poll(
            false,
            crate::client::PaneScroll {
                max_offset_from_bottom: 120,
                ..SCREEN_ONLY
            },
        );
    }

    /// Herdr raises an output event only when a pattern starts to match, so once a request's
    /// first closing line is on the screen, a further reply under the same reply ID raises no
    /// event. While such a line is in view the loop reads the pane itself, every
    /// `SATURATED_POLL_INTERVAL`. Here no reconciliation is due within the hour and no event
    /// reports a further reply, so only that poll can read the second and third replies. Each
    /// reply appears on the screen only once the one before it is stored. The first reply is on
    /// the screen when the loop starts, or with `first_from_event`, the screen is empty until
    /// herdr reports the first reply in an output event. Herdr then sends that event again every
    /// 500 ms, as events for other requests' reply IDs could arrive. An event's capture covers
    /// only its own reply ID, so no event may put the poll off. `scroll` is the pane's scroll
    /// geometry. Every read takes what the loop's other reads of that pane take: only the screen
    /// of a pane that keeps no scrollback, whose view herdr would scroll to read more, and recent
    /// rows unwrapped of any other pane.
    fn further_replies_under_one_alias_on_the_saturated_poll(
        first_from_event: bool,
        scroll: crate::client::PaneScroll,
    ) {
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        *fixture.client.scroll.lock().expect("scroll") = Some(scroll);
        let (_, state) = worker_bridge_state(&fixture, true);
        let (key, prompt) = delivered_worker_request(&state);
        assert!(
            prompt.contains("Include the line <CHAT_REPLY_001> at the beginning"),
            "{prompt}"
        );
        let answers = ["first answer", "second answer", "third answer"];
        let show = |count: usize| {
            *fixture.client.screen.lock().expect("screen") = Some(
                answers[..count]
                    .iter()
                    .map(|answer| format!("<CHAT_REPLY_001>\n{answer}\n</CHAT_REPLY_001>\n"))
                    .collect(),
            );
        };
        let stored = || {
            state.inspect_request(&key).expect("inspect request")["replies"]
                .as_array()
                .expect("replies")
                .len()
        };
        let initial = usize::from(!first_from_event);
        show(initial);
        let read_sources = &fixture.client.read_sources;
        read_sources.lock().expect("read sources").clear();
        let events: fn(&str) -> Vec<Value> = if first_from_event {
            |pane| {
                vec![json!({
                    "event": "pane.output_matched",
                    "data": {
                        "pane_id": pane,
                        "matched_line": "</CHAT_REPLY_001>",
                        "read": {
                            "pane_id": pane,
                            "workspace_id": "workspace",
                            "tab_id": "tab",
                            "source": "recent_unwrapped",
                            "format": "text",
                            "text": "<CHAT_REPLY_001>\nfirst answer\n</CHAT_REPLY_001>\n",
                            "revision": 1,
                            "truncated": false,
                        },
                    },
                })]
            }
        } else {
            |_| Vec::new()
        };
        let herdr = owner_loop_herdr_every(
            &fixture.root,
            "herdr",
            events,
            first_from_event.then_some(Duration::from_millis(500)),
        );
        let started = Instant::now();
        run_owner_loop_until(
            &fixture,
            &state,
            &herdr,
            Duration::from_secs(3_600),
            Duration::from_secs(30),
            || {
                let count = stored();
                show(if count == 0 {
                    initial
                } else {
                    (count + 1).min(answers.len())
                });
                count == answers.len()
            },
        );
        let elapsed = started.elapsed();
        assert_eq!(stored(), answers.len());
        // The loop read the pane at startup and then at most once per interval.
        let sources = read_sources.lock().expect("read sources").clone();
        let reads = sources.len();
        let intervals = elapsed.as_millis() / SATURATED_POLL_INTERVAL.as_millis();
        assert!(
            reads >= answers.len() && reads as u128 <= 2 + intervals,
            "{reads} reads in {elapsed:?}"
        );
        let source = if scroll.max_offset_from_bottom == 0 {
            "visible"
        } else {
            "recent-unwrapped"
        };
        assert!(sources.iter().all(|read| read == source), "{sources:?}");
        drop(herdr.release);
        herdr.server.join().expect("herdr stand-in");
    }

    #[test]
    fn a_failed_read_of_the_saturated_poll_is_retried_and_does_not_stop_the_owner_loop() {
        // The poll reads the pane every `SATURATED_POLL_INTERVAL` while a closing line is in view,
        // far more often than the reconciliation read, so a read that fails is logged and tried
        // again at the next interval instead of ending the loop. Here the first reply is on the
        // screen when the loop starts. Once it is stored, the next two reads fail; after that the
        // second reply is on the screen as well. The log line itself goes to standard error and
        // is not checked here.
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        *fixture.client.scroll.lock().expect("scroll") = Some(SCREEN_ONLY);
        let (_, state) = worker_bridge_state(&fixture, true);
        let (key, _) = delivered_worker_request(&state);
        let block = |answer: &str| format!("<CHAT_REPLY_001>\n{answer}\n</CHAT_REPLY_001>\n");
        *fixture.client.screen.lock().expect("screen") = Some(block("first answer"));
        let stored = || {
            state.inspect_request(&key).expect("inspect request")["replies"]
                .as_array()
                .expect("replies")
                .len()
        };
        let read_sources = &fixture.client.read_sources;
        let herdr = owner_loop_herdr(&fixture.root, "herdr", |_| Vec::new());
        // The number of reads made before the reads began to fail, and whether they failed.
        let mut failing_from = None;
        let mut failed = false;
        run_owner_loop_until(
            &fixture,
            &state,
            &herdr,
            Duration::from_secs(3_600),
            Duration::from_secs(30),
            || {
                let count = stored();
                if count == 1 && failing_from.is_none() {
                    fixture
                        .client
                        .fail_read
                        .store(true, AtomicOrdering::Relaxed);
                    failing_from = Some(read_sources.lock().expect("read sources").len());
                }
                if let Some(from) = failing_from.filter(|_| !failed) {
                    if read_sources.lock().expect("read sources").len() >= from + 2 {
                        failed = true;
                        *fixture.client.screen.lock().expect("screen") =
                            Some([block("first answer"), block("second answer")].concat());
                        fixture
                            .client
                            .fail_read
                            .store(false, AtomicOrdering::Relaxed);
                    }
                }
                count == 2
            },
        );
        assert!(failed, "the reads never failed");
        assert_eq!(stored(), 2);
        drop(herdr.release);
        herdr.server.join().expect("herdr stand-in");
    }

    /// The rows of a Claude Code pane's 52-row screen, and the rows of it below the
    /// conversation: a blank row, the input box, and the status row under it.
    const CLAUDE_SCREEN_ROWS: usize = 52;
    const CLAUDE_FOOTER: [&str; 5] = [
        "",
        "────────────────────────────────────────",
        "❯\u{a0}",
        "────────────────────────────────────────",
        "  ⏵⏵ bypass permissions on · 1 shell",
    ];
    /// The rows of that screen that show the conversation.
    const CLAUDE_CONVERSATION_ROWS: usize = CLAUDE_SCREEN_ROWS - CLAUDE_FOOTER.len();

    /// The screen of a Claude Code pane in a turn that answers the request with reply ID 001,
    /// once the turn has printed `below` rows of tool calls, each a blank row, a call row, and an
    /// output row, as Claude Code draws them. With `block`, the turn printed a four-row message
    /// holding a reply block before the tool calls, so the whole message is on the screen while
    /// `below` is at most 43 and none of it once `below` is 47. Earlier rows fill the top of the
    /// screen.
    fn claude_turn_screen(block: bool, below: usize) -> String {
        let mut rows = vec!["● Earlier work.".to_owned()];
        rows.extend((1..=60).map(|line| format!("  earlier line {line}")));
        rows.push(String::new());
        rows.push("❯ The user's request arrived through the configured chat bridge.".to_owned());
        rows.extend((1..=5).map(|line| format!("  Line {line} of the request.")));
        rows.push(String::new());
        if block {
            rows.extend(
                [
                    "● Here is the answer.",
                    "  <CHAT_REPLY_001>",
                    "  first answer",
                    "  </CHAT_REPLY_001>",
                ]
                .map(str::to_owned),
            );
        }
        rows.extend(
            (1..)
                .flat_map(|call| {
                    [
                        String::new(),
                        format!("● Bash(echo step {call})"),
                        format!("  ⎿  step {call}"),
                    ]
                })
                .take(below),
        );
        rows.extend(CLAUDE_FOOTER.map(str::to_owned));
        let mut screen = rows[rows.len() - CLAUDE_SCREEN_ROWS..].join("\n");
        screen.push('\n');
        screen
    }

    /// `claude_turn_screen` as Claude Code draws it while the turn runs, with `esc to interrupt`
    /// at the end of the status row.
    fn claude_running_screen(block: bool, below: usize) -> String {
        let status = CLAUDE_FOOTER[CLAUDE_FOOTER.len() - 1];
        claude_turn_screen(block, below).replace(status, &format!("{status} · esc to interrupt"))
    }

    /// `claude_turn_screen` as Claude Code draws it for a while after a paste, with a hint in
    /// place of the status row, whether or not the turn runs.
    fn claude_pasted_screen(block: bool, below: usize) -> String {
        let status = CLAUDE_FOOTER[CLAUDE_FOOTER.len() - 1];
        claude_turn_screen(block, below).replace(status, "  paste again to expand")
    }

    #[test]
    fn the_poll_stores_a_reply_block_once_wherever_later_tool_calls_have_pushed_it() {
        // https://github.com/rrnewton/agent-utils/issues/201: an agent wrote a complete reply
        // block and then, in the same turn, made tool calls, whose rows appear below the block
        // and push it up the screen. The read the service makes while the agent works stores the
        // block the first time it sees the whole message, at any height, and never again as
        // later reads see it higher up, then without the first row of its message, and then not
        // at all.
        let message_rows = |below: usize| {
            let screen = claude_turn_screen(true, below);
            ["● Here is the answer.", "  <CHAT_REPLY_001>"].map(|row| screen.contains(row))
        };
        assert_eq!(message_rows(43), [true, true]);
        assert_eq!(message_rows(44), [false, true]);
        assert!(!claude_turn_screen(true, CLAUDE_CONVERSATION_ROWS).contains("CHAT_REPLY_001"));
        for first_below in 0..=43 {
            let (state, key, root) = state_with_request();
            let delivery = RecordingDelivery::default();
            chat_runtime::deliver_request_with(&state, &delivery, &key, DrainOptions::default())
                .expect("deliver request");
            let route = state
                .next_reply_route(&key)
                .expect("route")
                .expect("active route");
            assert_eq!(route.identifier, "001");
            let mut routes = RouteCache::new(vec![route]);
            let client = HerdrClient::with_executable("direct", Path::new("/missing/herdr"))
                .expect("construct client");
            let manager = ManagedAgents::new(&client, &root.join("registry")).expect("manager");
            let mut transport = None;
            let mut control = PassControl {
                transport: &mut transport,
                stop: None,
            };
            for below in first_below..=CLAUDE_CONVERSATION_ROWS {
                let report = capture_visible_current(
                    &state,
                    &manager,
                    DrainOptions::default(),
                    &mut routes,
                    &claude_turn_screen(true, below),
                    &mut control,
                )
                .expect("poll");
                let expected = if below == first_below {
                    vec![(key.clone(), vec![1])]
                } else {
                    Vec::new()
                };
                assert_eq!(
                    report.captured, expected,
                    "first read with {first_below} rows below the block, this one with {below}"
                );
            }
            assert_eq!(
                state.inspect_request(&key).expect("inspect request")["replies"]
                    .as_array()
                    .expect("replies")
                    .len(),
                1
            );
            fs::remove_dir_all(root).expect("cleanup");
        }
    }

    /// How many replies are stored for the request `key` of `state`.
    fn stored_replies(state: &BridgeState, key: &str) -> usize {
        state.inspect_request(key).expect("inspect request")["replies"]
            .as_array()
            .expect("replies")
            .len()
    }

    #[test]
    fn while_the_agent_works_the_owner_loop_reads_a_reply_block_before_tool_calls_push_it_off() {
        // https://github.com/rrnewton/agent-utils/issues/201 and
        // https://github.com/rrnewton/agent-utils/issues/202: a turn printed a complete reply
        // block and went on to make tool calls, whose rows pushed the block up and off the
        // screen before the turn ended, and no output event reported the block, as when an older
        // closing line keeps its pattern matched. While the agent works with a request open, the
        // loop reads the pane every `SATURATED_POLL_INTERVAL`. Here the agent works throughout,
        // Herdr sends no event, and no reconciliation is due within the hour, so only that read
        // can see the block. The startup read shows the turn before the block; the next shows
        // the block with 20 rows of tool calls below it, the next without the first row of its
        // message, and every later read without the block.
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        *fixture.client.scroll.lock().expect("scroll") = Some(SCREEN_ONLY);
        *fixture.client.status.lock().expect("status") = Some("working".to_owned());
        let (_, state) = worker_bridge_state(&fixture, true);
        let (key, prompt) = delivered_worker_request(&state);
        assert!(
            prompt.contains("Include the line <CHAT_REPLY_001> at the beginning"),
            "{prompt}"
        );
        fixture.client.screens.lock().expect("screens").extend([
            claude_turn_screen(false, 4),
            claude_turn_screen(true, 20),
            claude_turn_screen(true, 44),
        ]);
        *fixture.client.screen.lock().expect("screen") =
            Some(claude_turn_screen(true, CLAUDE_CONVERSATION_ROWS));
        let read_sources = &fixture.client.read_sources;
        read_sources.lock().expect("read sources").clear();
        let herdr = owner_loop_herdr(&fixture.root, "herdr", |_| Vec::new());
        run_owner_loop_until(
            &fixture,
            &state,
            &herdr,
            Duration::from_secs(3_600),
            Duration::from_secs(15),
            || {
                stored_replies(&state, &key) == 1
                    && read_sources.lock().expect("read sources").len() >= 4
            },
        );
        assert_eq!(stored_replies(&state, &key), 1);
        let reads = read_sources.lock().expect("read sources").len();
        assert!(reads >= 4, "{reads} reads");
        drop(herdr.release);
        herdr.server.join().expect("herdr stand-in");
    }

    #[test]
    fn once_herdr_reports_that_the_agent_works_the_owner_loop_reads_the_pane_while_it_does() {
        // A turn can start without a prompt from the service, as when someone types into the
        // pane, and its screen need not show the running-turn marker, so Herdr's report that the
        // agent works may be the only sign of it. The loop subscribes to that report, so it reads
        // the pane every `SATURATED_POLL_INTERVAL` during such a turn too. Here the agent is idle
        // until the loop has subscribed and works from then on, and Herdr reports that it works
        // every 200 ms; no reconciliation is due within the hour. No read shows the marker; the
        // startup read shows no block, and every later read shows one.
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        *fixture.client.scroll.lock().expect("scroll") = Some(SCREEN_ONLY);
        let (_, state) = worker_bridge_state(&fixture, true);
        let (key, _) = delivered_worker_request(&state);
        fixture
            .client
            .screens
            .lock()
            .expect("screens")
            .push_back(claude_turn_screen(false, 4));
        *fixture.client.screen.lock().expect("screen") = Some(claude_turn_screen(true, 4));
        let herdr = owner_loop_herdr_every(
            &fixture.root,
            "herdr",
            |pane| {
                vec![json!({
                    "event": "pane.agent_status_changed",
                    "data": {"pane_id": pane, "agent_status": "working"},
                })]
            },
            Some(Duration::from_millis(200)),
        );
        let subscriptions = &herdr.subscriptions;
        run_owner_loop_until(
            &fixture,
            &state,
            &herdr,
            Duration::from_secs(3_600),
            Duration::from_secs(15),
            || {
                if subscriptions.load(AtomicOrdering::SeqCst) > 0 {
                    *fixture.client.status.lock().expect("status") = Some("working".to_owned());
                }
                stored_replies(&state, &key) == 1
            },
        );
        assert_eq!(stored_replies(&state, &key), 1);
        drop(herdr.release);
        herdr.server.join().expect("herdr stand-in");
    }

    #[test]
    fn a_settle_the_agent_has_already_left_makes_the_owner_loop_read_the_pane_at_once() {
        // Herdr reports that the agent settled, but by the time the loop asks, Herdr reports
        // another status, as when a prompt typed before the event was read has started another
        // turn. The turn that ended may have left a reply block on the screen, which the new
        // turn's output can push off, so the loop reads the pane at once. Here Herdr reports the
        // agent idle every 200 ms but `done` when asked, so the agent never counts as working,
        // and no reconciliation is due within the hour: only the read for such an event can see
        // the block. The startup read shows no block, and every later read shows one.
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        *fixture.client.scroll.lock().expect("scroll") = Some(SCREEN_ONLY);
        *fixture.client.status.lock().expect("status") = Some("done".to_owned());
        let (_, state) = worker_bridge_state(&fixture, true);
        let (key, _) = delivered_worker_request(&state);
        fixture
            .client
            .screens
            .lock()
            .expect("screens")
            .push_back(claude_turn_screen(false, 4));
        *fixture.client.screen.lock().expect("screen") = Some(claude_turn_screen(true, 4));
        let herdr = owner_loop_herdr_every(
            &fixture.root,
            "herdr",
            |pane| {
                vec![json!({
                    "event": "pane.agent_status_changed",
                    "data": {"pane_id": pane, "agent_status": "idle"},
                })]
            },
            Some(Duration::from_millis(200)),
        );
        run_owner_loop_until(
            &fixture,
            &state,
            &herdr,
            Duration::from_secs(3_600),
            Duration::from_secs(15),
            || stored_replies(&state, &key) == 1,
        );
        assert_eq!(stored_replies(&state, &key), 1);
        drop(herdr.release);
        herdr.server.join().expect("herdr stand-in");
    }

    #[test]
    fn the_owner_loop_reads_the_pane_before_it_types_the_prompt_of_a_new_request() {
        // A typed prompt, and the turn it starts, push the rows above them up, and can push a
        // reply block of the turn before off the screen before the loop reads the pane again,
        // while Herdr has not yet reported that that turn ended
        // (https://github.com/rrnewton/agent-utils/issues/202). So before the loop types the
        // prompt of a request the chat provider reports, it reads the pane.
        reads_the_pane_before_it_types_the_prompt_of_a_new_request(true);
    }

    #[test]
    fn the_owner_loop_reads_the_pane_before_a_rescan_types_the_prompt_of_a_new_request() {
        // When the chat provider finds the notice channel full, it tells the loop that notices
        // were lost, and the loop rescans the requests, which types the prompt of a new one as a
        // notice would. So the loop reads the pane before the rescan too.
        reads_the_pane_before_it_types_the_prompt_of_a_new_request(false);
    }

    /// The owner loop stores a reply block that typing the prompt of a second request pushes off
    /// the screen, as the loop reads the pane before it types that prompt. The second request
    /// arrives once the loop has subscribed, after its startup, and the chat provider sends the
    /// loop its key when `notified` is set, and otherwise only tells the loop that notices were
    /// lost. The agent is idle, Herdr sends no event, and no reconciliation is due within the
    /// hour. The startup read shows the first request's turn before its block, and later reads
    /// show the block until the second request's prompt is typed, which pushes it off the screen.
    fn reads_the_pane_before_it_types_the_prompt_of_a_new_request(notified: bool) {
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        *fixture.client.scroll.lock().expect("scroll") = Some(SCREEN_ONLY);
        let (_, state) = worker_bridge_state(&fixture, true);
        let (first, _) = delivered_worker_request(&state);
        fixture
            .client
            .screens
            .lock()
            .expect("screens")
            .push_back(claude_turn_screen(false, 4));
        *fixture.client.screen.lock().expect("screen") = Some(claude_turn_screen(true, 4));
        *fixture
            .client
            .screen_after_run
            .lock()
            .expect("screen after run") = Some(claude_turn_screen(true, CLAUDE_CONVERSATION_ROWS));
        fixture.client.runs.lock().expect("runs").clear();
        let herdr = owner_loop_herdr(&fixture.root, "herdr", |_| Vec::new());
        let subscriptions = &herdr.subscriptions;
        let mut second = None;
        run_owner_loop_notified(
            &fixture,
            &state,
            &herdr,
            Duration::from_secs(3_600),
            Duration::from_secs(15),
            |notify, overflow| {
                if second.is_none() && subscriptions.load(AtomicOrdering::SeqCst) > 0 {
                    let key = admitted_worker_message(&state, 2, "two");
                    if notified {
                        notify(ProviderNotice::Batch(vec![key.clone()]));
                    } else {
                        overflow();
                    }
                    second = Some(key);
                }
                !fixture.client.runs.lock().expect("runs").is_empty()
            },
        );
        let second = second.expect("the loop never subscribed");
        let prompts = fixture.client.runs.lock().expect("runs").clone();
        assert_eq!(prompts.len(), 1, "{prompts:#?}");
        assert!(
            prompts[0].contains("Include the line <CHAT_REPLY_002> at the beginning"),
            "{}",
            prompts[0]
        );
        assert_eq!(stored_replies(&state, &first), 1);
        assert_eq!(stored_replies(&state, &second), 0);
        drop(herdr.release);
        herdr.server.join().expect("herdr stand-in");
    }

    #[test]
    fn the_owner_loop_reads_the_pane_while_its_screen_shows_a_running_turn() {
        // Herdr can report a working Claude Code pane as idle
        // (https://github.com/rrnewton/agent-utils/issues/179), so the loop also takes a running
        // turn from the screens it reads. Here Herdr reports the agent idle throughout and sends
        // no event, no reconciliation is due within the hour, and every read shows the
        // running-turn marker. The startup read shows the turn before the block; the next shows
        // the block with 20 rows of tool calls below it, the next without the first row of its
        // message, and every later read without the block.
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        *fixture.client.scroll.lock().expect("scroll") = Some(SCREEN_ONLY);
        let (_, state) = worker_bridge_state(&fixture, true);
        let (key, _) = delivered_worker_request(&state);
        fixture.client.screens.lock().expect("screens").extend([
            claude_running_screen(false, 4),
            claude_running_screen(true, 20),
            claude_running_screen(true, 44),
        ]);
        *fixture.client.screen.lock().expect("screen") =
            Some(claude_running_screen(true, CLAUDE_CONVERSATION_ROWS));
        let read_sources = &fixture.client.read_sources;
        read_sources.lock().expect("read sources").clear();
        let herdr = owner_loop_herdr(&fixture.root, "herdr", |_| Vec::new());
        run_owner_loop_until(
            &fixture,
            &state,
            &herdr,
            Duration::from_secs(3_600),
            Duration::from_secs(15),
            || {
                stored_replies(&state, &key) == 1
                    && read_sources.lock().expect("read sources").len() >= 4
            },
        );
        assert_eq!(stored_replies(&state, &key), 1);
        let reads = read_sources.lock().expect("read sources").len();
        assert!(reads >= 4, "{reads} reads");
        drop(herdr.release);
        herdr.server.join().expect("herdr stand-in");
    }

    #[test]
    fn after_typing_a_prompt_the_owner_loop_reads_the_pane_though_herdr_reports_idle() {
        // A typed prompt starts a turn that Herdr can report as idle
        // (https://github.com/rrnewton/agent-utils/issues/179), and the screen need not show the
        // running-turn marker yet. So once the loop has typed a prompt, it reads the pane every
        // `SATURATED_POLL_INTERVAL` until a read shows no running turn. Here Herdr reports the
        // agent idle throughout and sends no event, no reconciliation is due within the hour, and
        // no read shows the marker. The loop types the prompt of the admitted request at startup;
        // reads before that show no block, and every read after it shows one.
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        *fixture.client.scroll.lock().expect("scroll") = Some(SCREEN_ONLY);
        let (_, state) = worker_bridge_state(&fixture, true);
        let key = admitted_worker_request(&state);
        *fixture.client.screen.lock().expect("screen") = Some(claude_turn_screen(false, 4));
        *fixture
            .client
            .screen_after_run
            .lock()
            .expect("screen after run") = Some(claude_turn_screen(true, 4));
        fixture.client.runs.lock().expect("runs").clear();
        let herdr = owner_loop_herdr(&fixture.root, "herdr", |_| Vec::new());
        run_owner_loop_until(
            &fixture,
            &state,
            &herdr,
            Duration::from_secs(3_600),
            Duration::from_secs(15),
            || stored_replies(&state, &key) == 1,
        );
        let prompts = fixture.client.runs.lock().expect("runs").clone();
        assert_eq!(prompts.len(), 1, "{prompts:#?}");
        assert!(
            prompts[0].contains("Include the line <CHAT_REPLY_001> at the beginning"),
            "{}",
            prompts[0]
        );
        assert_eq!(stored_replies(&state, &key), 1);
        drop(herdr.release);
        herdr.server.join().expect("herdr stand-in");
    }

    #[test]
    fn the_owner_loop_keeps_reading_the_pane_while_the_paste_hint_hides_the_running_turn() {
        // For a while after a paste, Claude Code draws a hint in place of the status row that
        // shows a running turn, so a read of that screen cannot tell whether the agent works and
        // leaves what the loop knew. Here Herdr reports the agent idle throughout and sends no
        // event, and no reconciliation is due within the hour. The loop types the prompt of the
        // admitted request at startup. Reads before that show no block and no marker; reads after
        // it show the hint, without a block until one of them has been made, and with a block
        // that answers the request from then on.
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        *fixture.client.scroll.lock().expect("scroll") = Some(SCREEN_ONLY);
        let (_, state) = worker_bridge_state(&fixture, true);
        let key = admitted_worker_request(&state);
        *fixture.client.screen.lock().expect("screen") = Some(claude_turn_screen(false, 4));
        *fixture
            .client
            .screen_after_run
            .lock()
            .expect("screen after run") = Some(claude_pasted_screen(false, 4));
        fixture.client.runs.lock().expect("runs").clear();
        let read_sources = &fixture.client.read_sources;
        let herdr = owner_loop_herdr(&fixture.root, "herdr", |_| Vec::new());
        // The number of reads when the prompt was first seen typed, and whether a later read has
        // been made since.
        let mut typed_at = None;
        let mut hint_read = false;
        run_owner_loop_until(
            &fixture,
            &state,
            &herdr,
            Duration::from_secs(3_600),
            Duration::from_secs(15),
            || {
                let reads = read_sources.lock().expect("read sources").len();
                if typed_at.is_none() && !fixture.client.runs.lock().expect("runs").is_empty() {
                    typed_at = Some(reads);
                }
                if !hint_read && typed_at.is_some_and(|typed| reads > typed) {
                    hint_read = true;
                    *fixture.client.screen.lock().expect("screen") =
                        Some(claude_pasted_screen(true, 4));
                }
                stored_replies(&state, &key) == 1
            },
        );
        assert!(hint_read, "no read after the prompt was typed");
        assert_eq!(fixture.client.runs.lock().expect("runs").len(), 1);
        assert_eq!(stored_replies(&state, &key), 1);
        drop(herdr.release);
        herdr.server.join().expect("herdr stand-in");
    }

    #[test]
    fn a_pass_says_it_asked_for_a_prompt_to_be_typed_only_when_it_did() {
        // The owner loop counts a pass that asked for a prompt to be typed as the start of a
        // turn, so `CycleReport::prompt_typed` must not be set by a pass that typed nothing.
        let pass = |state: &BridgeState, delivery: &RecordingDelivery, key: &str| {
            let mut transport = None;
            process_keys_with_delivery(
                state,
                delivery,
                DrainOptions::default(),
                &[key.to_owned()],
                &mut PassControl {
                    transport: &mut transport,
                    stop: None,
                },
            )
            .expect("pass")
        };
        let (state, key, root) = state_with_request();
        let delivery = RecordingDelivery::default();
        let first = pass(&state, &delivery, &key);
        assert_eq!(first.delivered, std::slice::from_ref(&key));
        assert!(first.prompt_typed);
        // The request is delivered, so the next pass types nothing.
        let second = pass(&state, &delivery, &key);
        assert_eq!(second.delivered, std::slice::from_ref(&key));
        assert!(!second.prompt_typed);
        assert_eq!(delivery.prompts.lock().expect("prompts").len(), 1);
        fs::remove_dir_all(root).expect("cleanup");

        // The queue reports the request's prompt as possibly typed already, so the pass leaves
        // the request uncertain and types nothing.
        let (state, key, root) = state_with_request();
        let message_id = state.inspect_request(&key).expect("inspect request")["delivery"]
            ["message_id"]
            .as_str()
            .expect("queue message ID")
            .to_owned();
        let delivery = RecordingDelivery::default();
        delivery
            .states
            .lock()
            .expect("states")
            .insert(message_id, QueueMessageState::Inflight);
        let report = pass(&state, &delivery, &key);
        assert_eq!(report.delivery_uncertain, std::slice::from_ref(&key));
        assert!(!report.prompt_typed);
        assert!(delivery.prompts.lock().expect("prompts").is_empty());
        fs::remove_dir_all(root).expect("cleanup");

        // The request's prompt waits in the queue's inbox, so the pass drains the queue, which
        // types every prompt there, instead of submitting the prompt itself.
        let (state, key, root) = state_with_request();
        let message_id = state.inspect_request(&key).expect("inspect request")["delivery"]
            ["message_id"]
            .as_str()
            .expect("queue message ID")
            .to_owned();
        let delivery = RecordingDelivery::default();
        delivery
            .states
            .lock()
            .expect("states")
            .insert(message_id, QueueMessageState::Pending);
        let report = pass(&state, &delivery, &key);
        assert!(report.prompt_typed);
        assert!(delivery.prompts.lock().expect("prompts").is_empty());
        fs::remove_dir_all(root).expect("cleanup");
    }

    /// How many times the owner loop reads the pane in the two `SATURATED_POLL_INTERVAL`s after
    /// it subscribes to Herdr, for an agent that Herdr reports as `status` and whose every read
    /// shows `screen`. Outbound replies are on when `outbound` is set, and one delivered request
    /// is open when `request` is set. Herdr sends no event, and no reconciliation is due within
    /// the hour.
    fn owner_loop_polls(status: &str, screen: String, outbound: bool, request: bool) -> usize {
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        *fixture.client.scroll.lock().expect("scroll") = Some(SCREEN_ONLY);
        *fixture.client.status.lock().expect("status") = Some(status.to_owned());
        *fixture.client.screen.lock().expect("screen") = Some(screen);
        let (_, state) = worker_bridge_state(&fixture, outbound);
        if request {
            delivered_worker_request(&state);
        }
        let read_sources = &fixture.client.read_sources;
        let herdr = owner_loop_herdr(&fixture.root, "herdr", |_| Vec::new());
        let subscriptions = &herdr.subscriptions;
        let mut subscribed = None;
        run_owner_loop_until(
            &fixture,
            &state,
            &herdr,
            Duration::from_secs(3_600),
            Duration::from_secs(15),
            || {
                let reads = read_sources.lock().expect("read sources").len();
                if subscribed.is_none() && subscriptions.load(AtomicOrdering::SeqCst) > 0 {
                    subscribed = Some((Instant::now(), reads));
                }
                subscribed.is_some_and(|(at, _): (Instant, usize)| {
                    at.elapsed() > SATURATED_POLL_INTERVAL * 2
                })
            },
        );
        let (_, at_subscription) = subscribed.expect("the loop never subscribed");
        let reads = read_sources.lock().expect("read sources").len();
        drop(herdr.release);
        herdr.server.join().expect("herdr stand-in");
        reads - at_subscription
    }

    #[test]
    fn the_owner_loop_does_not_poll_a_pane_that_neither_herdr_nor_the_screen_reports_working() {
        // No read shows the running-turn marker. Only the last case, where Herdr reports the
        // agent working, is read every `SATURATED_POLL_INTERVAL`.
        let polls = thread::scope(|scope| {
            ["idle", "done", "blocked", "unknown", "working"]
                .map(|status| {
                    scope.spawn(move || {
                        owner_loop_polls(status, claude_turn_screen(false, 4), true, true)
                    })
                })
                .map(|case| case.join().expect("case"))
        });
        assert_eq!(polls[..4], [0; 4], "{polls:?}");
        assert!(polls[4] >= 1, "{polls:?}");
    }

    #[test]
    fn the_owner_loop_does_not_poll_a_working_agent_while_no_request_has_a_current_reply_id() {
        // Herdr and every read report a running turn. With outbound replies off, or no request
        // open, no request has a current reply ID, so no reply can arrive for the poll to read;
        // only the last case has one.
        let polls = thread::scope(|scope| {
            [(true, false), (false, true), (true, true)]
                .map(|(outbound, request)| {
                    scope.spawn(move || {
                        owner_loop_polls(
                            "working",
                            claude_running_screen(false, 4),
                            outbound,
                            request,
                        )
                    })
                })
                .map(|case| case.join().expect("case"))
        });
        assert_eq!(polls[..2], [0; 2], "{polls:?}");
        assert!(polls[2] >= 1, "{polls:?}");
    }

    /// How many times the owner loop reads the pane in the two `SATURATED_POLL_INTERVAL`s after
    /// its first `first` reads, with outbound replies on and one delivered request open, for an
    /// agent that Herdr reports as `status`. The startup read shows `startup`, when set, and every
    /// other read shows `screen`. `herdr` makes the Herdr stand-in under the fixture's root, and
    /// reconciliation is due every `reconciliation_interval`.
    fn owner_loop_reads_after(
        status: &str,
        startup: Option<String>,
        screen: String,
        herdr: fn(&Path) -> OwnerLoopHerdr,
        reconciliation_interval: Duration,
        first: usize,
    ) -> usize {
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        *fixture.client.scroll.lock().expect("scroll") = Some(SCREEN_ONLY);
        *fixture.client.status.lock().expect("status") = Some(status.to_owned());
        *fixture.client.screen.lock().expect("screen") = Some(screen);
        let (_, state) = worker_bridge_state(&fixture, true);
        delivered_worker_request(&state);
        if let Some(startup) = startup {
            fixture
                .client
                .screens
                .lock()
                .expect("screens")
                .push_back(startup);
        }
        let read_sources = &fixture.client.read_sources;
        read_sources.lock().expect("read sources").clear();
        let herdr = herdr(&fixture.root);
        let mut started = None;
        run_owner_loop_until(
            &fixture,
            &state,
            &herdr,
            reconciliation_interval,
            Duration::from_secs(15),
            || {
                let reads = read_sources.lock().expect("read sources").len();
                if started.is_none() && reads >= first {
                    started = Some((Instant::now(), reads));
                }
                started.is_some_and(|(at, _): (Instant, usize)| {
                    at.elapsed() > SATURATED_POLL_INTERVAL * 2
                })
            },
        );
        let (_, at_start) = started.expect("the loop never made its first reads");
        let reads = read_sources.lock().expect("read sources").len();
        drop(herdr.release);
        herdr.server.join().expect("herdr stand-in");
        reads - at_start
    }

    #[test]
    fn while_no_subscription_works_the_owner_loop_takes_herdrs_status_from_reconciliation() {
        // While the loop cannot subscribe to the pane's events, it makes no wait for them and so
        // no status lookup after one, and each reconciliation, which looks the status up too, is
        // its only source of Herdr's report that the agent works. Here no subscription connects,
        // reconciliation is due every 200 ms, and no read shows the running-turn marker. Herdr
        // reports a status that does not settle the agent, so no reconciliation reads the pane,
        // and only the last case, where Herdr reports the agent working, is read every
        // `SATURATED_POLL_INTERVAL` after the startup read.
        let polls = thread::scope(|scope| {
            ["blocked", "unknown", "working"]
                .map(|status| {
                    scope.spawn(move || {
                        owner_loop_reads_after(
                            status,
                            None,
                            claude_turn_screen(false, 4),
                            |root| unreachable_owner_loop_herdr(root, "herdr"),
                            Duration::from_millis(200),
                            1,
                        )
                    })
                })
                .map(|case| case.join().expect("case"))
        });
        assert_eq!(polls[..2], [0; 2], "{polls:?}");
        assert!(polls[2] >= 1, "{polls:?}");
    }

    /// An output event on `pane` for a closing line under a reply ID that no request has, whose
    /// read of the pane's recent rows shows that line above `screen`.
    fn unknown_reply_event(pane: &str, screen: String) -> Vec<Value> {
        let line = "</CHAT_REPLY_unknownnonce00_1>";
        vec![json!({
            "event": "pane.output_matched",
            "data": {
                "pane_id": pane,
                "matched_line": line,
                "read": {
                    "pane_id": pane,
                    "workspace_id": "workspace",
                    "tab_id": "tab",
                    "source": "recent_unwrapped",
                    "format": "text",
                    "text": format!("{line}\n{screen}"),
                    "revision": 1,
                    "truncated": false,
                },
            },
        })]
    }

    #[test]
    fn the_owner_loop_takes_a_running_turn_from_the_rows_an_output_event_carries() {
        // An output event carries the pane's recent rows as Herdr read them when the pattern
        // matched, and the loop takes a running turn from them as from the screens it reads
        // itself, but not the end of one: Herdr may have read them before a prompt the loop has
        // since typed. Here Herdr reports the agent idle throughout
        // (https://github.com/rrnewton/agent-utils/issues/179), no reconciliation is due within
        // the hour, and Herdr sends one event, for a reply ID no request has, so the loop reads
        // the pane after it. By then a paste, as from another sender, has put Claude Code's paste
        // hint in place of the status row, so that read cannot tell whether the agent works. In
        // the first two cases the startup read cannot tell either, and the event's rows show the
        // running-turn marker in the first and not in the second; in the third, the startup read
        // shows the marker and the event's rows do not. The first and third are read every
        // `SATURATED_POLL_INTERVAL` after the read for the event, and the second is not.
        let polls = thread::scope(|scope| {
            let running = scope.spawn(|| {
                owner_loop_reads_after(
                    "idle",
                    None,
                    claude_pasted_screen(false, 4),
                    |root| {
                        owner_loop_herdr(root, "herdr", |pane| {
                            unknown_reply_event(pane, claude_running_screen(false, 4))
                        })
                    },
                    Duration::from_secs(3_600),
                    2,
                )
            });
            let finished = scope.spawn(|| {
                owner_loop_reads_after(
                    "idle",
                    None,
                    claude_pasted_screen(false, 4),
                    |root| {
                        owner_loop_herdr(root, "herdr", |pane| {
                            unknown_reply_event(pane, claude_turn_screen(false, 4))
                        })
                    },
                    Duration::from_secs(3_600),
                    2,
                )
            });
            let running_before = scope.spawn(|| {
                owner_loop_reads_after(
                    "idle",
                    Some(claude_running_screen(false, 4)),
                    claude_pasted_screen(false, 4),
                    |root| {
                        owner_loop_herdr(root, "herdr", |pane| {
                            unknown_reply_event(pane, claude_turn_screen(false, 4))
                        })
                    },
                    Duration::from_secs(3_600),
                    2,
                )
            });
            [running, finished, running_before].map(|case| case.join().expect("case"))
        });
        assert!(polls[0] >= 1, "{polls:?}");
        assert_eq!(polls[1], 0, "{polls:?}");
        assert!(polls[2] >= 1, "{polls:?}");
    }

    #[test]
    fn while_a_failed_read_waits_for_its_retry_settles_and_request_keys_do_not_read_the_pane() {
        // The loop reads the pane at once after a settle the agent has already left and before
        // each pass that handles queued request keys, but not while a failed read waits for the
        // retry the saturated poll makes after `SATURATED_POLL_INTERVAL`, so a pane whose reads
        // fail is not tried again at every event. Here Herdr reports the agent working when
        // asked, so the loop polls, and sends a settle to idle every 200 ms, and no
        // reconciliation is due within the hour. Once the loop has read the pane twice, every
        // read fails, and the chat provider reports the key of the open request every 100 ms,
        // which the loop finds already delivered without touching the pane. The fake records each
        // read before it fails, and in the next five seconds the loop tries the pane at most
        // three times.
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        *fixture.client.scroll.lock().expect("scroll") = Some(SCREEN_ONLY);
        *fixture.client.status.lock().expect("status") = Some("working".to_owned());
        *fixture.client.screen.lock().expect("screen") = Some(claude_turn_screen(false, 4));
        let (_, state) = worker_bridge_state(&fixture, true);
        let (key, _) = delivered_worker_request(&state);
        let read_sources = &fixture.client.read_sources;
        read_sources.lock().expect("read sources").clear();
        let herdr = owner_loop_herdr_every(
            &fixture.root,
            "herdr",
            |pane| {
                vec![json!({
                    "event": "pane.agent_status_changed",
                    "data": {"pane_id": pane, "agent_status": "idle"},
                })]
            },
            Some(Duration::from_millis(200)),
        );
        let reads = || read_sources.lock().expect("read sources").len();
        let mut failing: Option<(Instant, usize)> = None;
        let mut notified_at = Instant::now();
        let mut tried = None;
        run_owner_loop_notified(
            &fixture,
            &state,
            &herdr,
            Duration::from_secs(3_600),
            Duration::from_secs(30),
            |notify, _| {
                let Some((at, before)) = failing else {
                    if reads() >= 2 {
                        fixture.client.fail_read.store(true, AtomicOrdering::SeqCst);
                        failing = Some((Instant::now(), reads()));
                    }
                    return false;
                };
                if at.elapsed() > Duration::from_secs(5) {
                    tried = Some(reads() - before);
                    return true;
                }
                if notified_at.elapsed() >= Duration::from_millis(100) {
                    notify(ProviderNotice::Batch(vec![key.clone()]));
                    notified_at = Instant::now();
                }
                false
            },
        );
        let tried = tried.expect("the loop never read the pane twice");
        assert!((1..=3).contains(&tried), "{tried} tries in five seconds");
        drop(herdr.release);
        herdr.server.join().expect("herdr stand-in");
    }

    #[test]
    fn a_failed_status_lookup_after_a_wait_that_returned_no_event_leaves_the_owner_loop_running() {
        // While the agent works, the loop's wait for pane events ends every
        // `SATURATED_POLL_INTERVAL` for its next read, and the loop looks Herdr's status up after
        // each such wait, so a Herdr call that fails there must not stop the service. Here Herdr
        // reports the agent working and sends no event, and no reconciliation is due within the
        // hour. Once the loop has read the pane twice, every Herdr query, and so every lookup and
        // read, fails for five seconds, which spans at least two such waits. The loop keeps
        // querying Herdr, and once Herdr answers again it reads the pane again; the harness fails
        // the test if the loop stops with an error before it is told to stop. Each failure moves
        // the retry time of its kind, the lookup's or the read's, `SATURATED_POLL_INTERVAL` ahead.
        // So the waits end only a few times in those five seconds, the loop looks the status up
        // after each, and it queries Herdr fewer than 20 times; a retry time that a later failure
        // did not move would make every wait after it end at once.
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        *fixture.client.scroll.lock().expect("scroll") = Some(SCREEN_ONLY);
        *fixture.client.status.lock().expect("status") = Some("working".to_owned());
        *fixture.client.screen.lock().expect("screen") = Some(claude_turn_screen(false, 4));
        let (_, state) = worker_bridge_state(&fixture, true);
        delivered_worker_request(&state);
        let read_sources = &fixture.client.read_sources;
        read_sources.lock().expect("read sources").clear();
        let herdr = owner_loop_herdr(&fixture.root, "herdr", |_| Vec::new());
        let reads = || read_sources.lock().expect("read sources").len();
        let queries = || {
            fixture.client.panes_calls.load(AtomicOrdering::SeqCst)
                + fixture.client.pane_info_calls.load(AtomicOrdering::SeqCst)
        };
        let mut failing: Option<(Instant, u64)> = None;
        let mut answering: Option<(u64, usize)> = None;
        run_owner_loop_until(
            &fixture,
            &state,
            &herdr,
            Duration::from_secs(3_600),
            Duration::from_secs(30),
            || {
                if failing.is_none() && reads() >= 2 {
                    fixture
                        .client
                        .fail_panes
                        .store(true, AtomicOrdering::SeqCst);
                    failing = Some((Instant::now(), queries()));
                }
                match (failing, answering) {
                    (Some((at, queried)), None) if at.elapsed() > Duration::from_secs(5) => {
                        answering = Some((queries() - queried, reads()));
                        fixture
                            .client
                            .fail_panes
                            .store(false, AtomicOrdering::SeqCst);
                    }
                    _ => {}
                }
                answering.is_some_and(|(_, before)| reads() > before)
            },
        );
        let (failed_queries, before) = answering.expect("the loop never read the pane twice");
        assert!(failed_queries >= 2, "{failed_queries} failed Herdr queries");
        assert!(failed_queries < 20, "{failed_queries} failed Herdr queries");
        assert!(reads() > before, "no read after Herdr answered again");
        drop(herdr.release);
        herdr.server.join().expect("herdr stand-in");
    }

    #[test]
    fn a_failed_status_lookup_right_after_subscribing_is_tried_again_within_the_poll_interval() {
        // Herdr raises a `working` event only when the status changes, so the lookup after the
        // first wait on a new subscription is the only lookup sure to follow the subscription
        // promptly and tell the loop that an agent already working then is working. Here Herdr
        // reports the agent working and sends no event, and the next `pane_info` call after it
        // acknowledges the subscription, which that lookup makes, fails. No screen shows a
        // running turn, the loop types no prompt, and no reconciliation is due within the hour,
        // so only a retry of the lookup can start the reads. The loop tries the lookup again
        // `SATURATED_POLL_INTERVAL` later and reads the pane one interval after that: not before
        // three seconds from the subscription, which a lookup that did not fail would allow, and
        // within ten. A loop that kept the retry time after a lookup succeeded would wait no time
        // at all and query Herdr without pause, so Herdr is also queried fewer than 20 times in
        // the three seconds after that read.
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        *fixture.client.scroll.lock().expect("scroll") = Some(SCREEN_ONLY);
        *fixture.client.status.lock().expect("status") = Some("working".to_owned());
        *fixture.client.screen.lock().expect("screen") = Some(claude_turn_screen(false, 4));
        let (_, state) = worker_bridge_state(&fixture, true);
        delivered_worker_request(&state);
        let read_sources = &fixture.client.read_sources;
        let herdr = owner_loop_herdr_arming(
            &fixture.root,
            "herdr",
            |_| Vec::new(),
            None,
            Some(Arc::clone(&fixture.client.fail_pane_info_once)),
        );
        let reads = || read_sources.lock().expect("read sources").len();
        let queries = || {
            fixture.client.panes_calls.load(AtomicOrdering::SeqCst)
                + fixture.client.pane_info_calls.load(AtomicOrdering::SeqCst)
        };
        let mut subscribed: Option<(Instant, usize)> = None;
        let mut first_read: Option<(Duration, Instant, u64)> = None;
        let mut queried = None;
        run_owner_loop_until(
            &fixture,
            &state,
            &herdr,
            Duration::from_secs(3_600),
            Duration::from_secs(30),
            || {
                if subscribed.is_none() && herdr.subscriptions.load(AtomicOrdering::SeqCst) > 0 {
                    subscribed = Some((Instant::now(), reads()));
                }
                let Some((at, before)) = subscribed else {
                    return false;
                };
                if first_read.is_none() && reads() > before {
                    first_read = Some((at.elapsed(), Instant::now(), queries()));
                }
                match first_read {
                    Some((_, read_at, before)) if read_at.elapsed() >= Duration::from_secs(3) => {
                        queried = Some(queries() - before);
                        true
                    }
                    _ => false,
                }
            },
        );
        assert_eq!(herdr.subscriptions.load(AtomicOrdering::SeqCst), 1);
        assert!(
            !fixture
                .client
                .fail_pane_info_once
                .load(AtomicOrdering::SeqCst),
            "the failure armed at the subscription was never used"
        );
        let (after, _, _) = first_read.expect("the loop never read the pane after subscribing");
        assert!(
            after >= Duration::from_secs(3),
            "read {after:?} after subscribing"
        );
        assert!(
            after <= Duration::from_secs(10),
            "read {after:?} after subscribing"
        );
        let queried = queried.expect("the loop stopped within three seconds of its read");
        assert!(queried < 20, "{queried} Herdr queries in three seconds");
        drop(herdr.release);
        herdr.server.join().expect("herdr stand-in");
    }

    #[test]
    fn a_chat_tick_reads_only_the_screen_of_a_pane_without_scrollback() {
        // `agentctl chat tick` makes the service's choice of read source in a second place,
        // `agent::read_capture`. Its outbound stays off, which a tick without an outbound helper
        // requires.
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        let (root, _) = worker_bridge_state(&fixture, false);
        let read_sources = &fixture.client.read_sources;
        let tick_read_sources = |scroll| {
            *fixture.client.scroll.lock().expect("scroll") = scroll;
            read_sources.lock().expect("read sources").clear();
            tick(
                &root,
                &fixture.manager(),
                ServiceOptions {
                    delivery: DrainOptions::default(),
                    reconciliation_interval: Duration::from_secs(1),
                },
            )
            .expect("tick");
            read_sources.lock().expect("read sources").clone()
        };
        assert_eq!(tick_read_sources(Some(SCREEN_ONLY)), ["visible"]);
        // Any other pane is read as `agentctl read` reads it, as recent rows unwrapped; the fake
        // returns some, so there is no fallback read of plain recent rows.
        let with_scrollback = crate::client::PaneScroll {
            max_offset_from_bottom: 120,
            ..SCREEN_ONLY
        };
        for scroll in [Some(with_scrollback), None] {
            assert_eq!(tick_read_sources(scroll), ["recent-unwrapped"]);
        }
    }

    fn tick_options() -> ServiceOptions {
        ServiceOptions {
            delivery: DrainOptions::default(),
            reconciliation_interval: Duration::from_secs(1),
        }
    }

    #[test]
    fn a_chat_tick_takes_no_block_in_its_read_as_the_reply_of_a_request_it_prompts() {
        // `agentctl chat tick` reads the pane once, and prompts pending requests in the same
        // pass. It takes the blocks in its read before any prompt gives a request its reply ID,
        // so a block the read shows under the next number, here one an earlier bridge state left
        // under 001, is reported as naming no open request, and that number is skipped. The read
        // just before the prompt no longer shows the block, as when the agent's output has moved
        // it out of view in between, so here only the order of the tick keeps the block from
        // being taken as the new request's reply.
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        *fixture.client.scroll.lock().expect("scroll") = Some(SCREEN_ONLY);
        let (root, state) = worker_tick_state(&fixture);
        let key = admitted_worker_request(&state);
        fixture.client.screens.lock().expect("screens").push_back(
            "<CHAT_REPLY_001>\nan answer from an earlier bridge state\n</CHAT_REPLY_001>\n"
                .to_owned(),
        );
        *fixture.client.screen.lock().expect("screen") = Some(String::new());
        fixture.client.runs.lock().expect("runs").clear();
        let report = tick(&root, &fixture.manager(), tick_options()).expect("tick");
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.delivered, std::slice::from_ref(&key));
        assert!(report.captured.is_empty(), "{:?}", report.captured);
        assert!(
            state.inspect_request(&key).expect("inspect request")["replies"]
                .as_array()
                .expect("replies")
                .is_empty()
        );
        let prompts = fixture.client.runs.lock().expect("runs").clone();
        assert_eq!(prompts.len(), 2, "{prompts:#?}");
        assert!(
            prompts[0].starts_with("Chat reply routing error: 001 matches no open chat request"),
            "{}",
            prompts[0]
        );
        assert!(
            prompts[1].contains("Include the line <CHAT_REPLY_002> at the beginning"),
            "{}",
            prompts[1]
        );
        assert!(fixture.client.screens.lock().expect("screens").is_empty());
    }

    #[test]
    fn the_reads_before_a_prompt_save_no_snapshot() {
        // A delivery reads the coordinator's pane just before it writes a prompt that gives a
        // reply ID, and before it types queued prompts while such a prompt waits, so these reads
        // come once for each attempt, retry, drain and routing-error prompt. They save no
        // snapshot, which would cost a file write and two fsyncs each time. Both deliveries are
        // checked: the service's, and the one `chat tick` uses.
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        let (_, state) = worker_bridge_state(&fixture, true);
        let snapshot = fixture.root.join("registry/worker/output.json");
        assert!(!snapshot.exists(), "{}", snapshot.display());
        let manager = fixture.manager();
        let stop = StopState::default();
        let service = CancellableDelivery {
            manager: &manager,
            runtime: StopRuntime::new(&stop),
        };
        let mut keys = vec![admitted_worker_request(&state)];
        keys.extend(admit_more_requests(&state, 1));
        let deliveries: [&dyn chat_runtime::CoordinatorDelivery; 2] = [&manager, &service];
        for (index, (key, delivery)) in keys.iter().zip(deliveries).enumerate() {
            fixture
                .client
                .read_sources
                .lock()
                .expect("read sources")
                .clear();
            assert_eq!(
                chat_runtime::deliver_request_with(&state, delivery, key, DrainOptions::default())
                    .expect("deliver request"),
                CoordinatorDeliveryResult::Delivered
            );
            assert!(
                !fixture
                    .client
                    .read_sources
                    .lock()
                    .expect("read sources")
                    .is_empty(),
                "delivery {index} did not read the pane before its prompt"
            );
            assert!(
                !snapshot.exists(),
                "delivery {index} saved a snapshot with its read before the prompt"
            );
        }
        let prompts = fixture.client.runs.lock().expect("runs").clone();
        assert!(
            prompts
                .iter()
                .any(|prompt| prompt.contains("Include the line <CHAT_REPLY_002> at the beginning")),
            "{prompts:#?}"
        );
        // A read that saves one, such as a capture's, shows that the check above can see it.
        manager
            .read_capture_with_runtime("worker", SNAPSHOT_LINES, &StopRuntime::new(&stop))
            .expect("capture read");
        assert!(snapshot.exists(), "{}", snapshot.display());
    }

    #[test]
    fn a_chat_tick_notes_an_unusable_reply_alias_record() {
        // While `reply-aliases.json` cannot be read, prompts give long reply IDs and blocks under
        // short ones are not sent. The service logs that, and a tick reports it among its notes.
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        let (root, state) = worker_tick_state(&fixture);
        let report = tick(&root, &fixture.manager(), tick_options()).expect("tick");
        assert!(report.notes.is_empty(), "{:?}", report.notes);
        fs::write(root.join("reply-aliases.json"), b"{").expect("spoil alias record");
        let report = tick(&root, &fixture.manager(), tick_options()).expect("tick");
        assert_eq!(
            report.notes,
            [state.reply_alias_problem().expect("alias problem")]
        );
        assert!(
            report.notes[0].contains("reply-aliases.json is unusable"),
            "{}",
            report.notes[0]
        );
    }

    #[test]
    fn a_rewrapped_block_with_misplaced_tags_draws_one_notice_as_it_scrolls_away() {
        // A block whose tags share their rows with other text is reported once. A capture of a
        // pane that rewrapped it so its closing tag sits alone on its row has no new block to
        // report, yet still records, with no notice, the entry of the unopened block those rows
        // give without the misplaced opening tag, so once that tag's row scrolls away the rest
        // of the block draws no second notice.
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        let (_root, state) = worker_bridge_state(&fixture, true);
        let (key, prompt) = delivered_worker_request(&state);
        let id = prompt
            .split("<CHAT_REPLY_")
            .nth(1)
            .and_then(|rest| rest.split('>').next())
            .expect("reply ID in the prompt")
            .to_owned();
        let (opening, closing) = (format!("<CHAT_REPLY_{id}>"), format!("</CHAT_REPLY_{id}>"));
        let tail = "remaining reply text. ".repeat(8);
        let screens = [
            format!("⏺ Working.\n❯ question\n⏺ {opening}First line.\n  {tail}{closing}\n❯\u{a0}\n"),
            format!(
                "⏺ Working.\n❯ question\n⏺ {opening}First line.\n  {tail}\n  {closing}\n❯\u{a0}\n"
            ),
            format!("  {tail}\n  {closing}\n❯\u{a0}\n"),
        ];
        let mut routes = RouteCache::new(vec![state
            .next_reply_route(&key)
            .expect("route")
            .expect("open route")]);
        let manager = fixture.manager();
        let mut transport = None;
        let mut control = PassControl {
            transport: &mut transport,
            stop: None,
        };
        fixture.client.runs.lock().expect("runs").clear();
        for (revision, screen) in (1..).zip(&screens) {
            let report = capture_recovery_snapshot(
                &state,
                &manager,
                DrainOptions::default(),
                &mut routes,
                SnapshotInput {
                    text: screen,
                    truncated: false,
                    revision: Some(revision),
                },
                &mut control,
            )
            .expect("capture of a whole read");
            assert!(report.captured.is_empty(), "{:?}", report.captured);
            assert!(report.errors.is_empty(), "{:?}", report.errors);
        }
        let runs = fixture.client.runs.lock().expect("runs").clone();
        assert_eq!(runs.len(), 1, "{runs:?}");
        assert!(
            runs[0].starts_with(&format!(
                "Chat reply not sent: an opening or closing tag shared its line with other text \
in the reply block marked {id}, so that block was not sent."
            )),
            "{}",
            runs[0]
        );
    }

    #[test]
    fn neither_capture_path_sends_a_block_under_the_id_of_a_request_whose_prompt_is_not_typed() {
        // A request's prompt gives it reply ID 001, and nothing types the prompt. A block under
        // 001 cannot be the coordinator's reply to it: the capture after an output event and the
        // capture of a whole read both note the block and send nothing to the request, and the
        // whole read reports the block to the coordinator.
        let fixture = crate::subagents::tests::Fixture::new();
        fixture.start(None);
        let (_root, state) = worker_bridge_state(&fixture, true);
        let key = admitted_worker_request(&state);
        assert!(state
            .prompt(&key, "")
            .expect("prompt")
            .contains("Include the line <CHAT_REPLY_001> at the beginning"));
        let block = "<CHAT_REPLY_001>\nan answer meant for another request\n</CHAT_REPLY_001>\n";
        let mut routes = RouteCache::new(vec![state
            .next_reply_route(&key)
            .expect("route")
            .expect("open route")]);
        let manager = fixture.manager();
        let mut transport = None;
        let mut control = PassControl {
            transport: &mut transport,
            stop: None,
        };
        fixture.client.runs.lock().expect("runs").clear();
        let report = capture_direct(
            &state,
            &manager,
            DrainOptions::default(),
            &mut routes,
            "001",
            SnapshotInput {
                text: block,
                truncated: false,
                revision: Some(1),
            },
            &mut control,
        )
        .expect("capture after an output event");
        assert!(report.captured.is_empty(), "{:?}", report.captured);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.notes.len(), 1, "{:?}", report.notes);
        assert!(
            report.notes[0]
                .contains("was read before its request's prompt reached the coordinator"),
            "{}",
            report.notes[0]
        );
        assert!(fixture.client.runs.lock().expect("runs").is_empty());
        let report = capture_recovery_snapshot(
            &state,
            &manager,
            DrainOptions::default(),
            &mut routes,
            SnapshotInput {
                text: block,
                truncated: false,
                revision: Some(2),
            },
            &mut control,
        )
        .expect("capture of a whole read");
        assert!(report.captured.is_empty(), "{:?}", report.captured);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.notes.len(), 1, "{:?}", report.notes);
        assert!(
            report.notes[0]
                .contains("was read before its request's prompt reached the coordinator"),
            "{}",
            report.notes[0]
        );
        assert_eq!(
            *fixture.client.runs.lock().expect("runs"),
            ["Chat reply not sent: the block marked 001 was written before the chat request with \
that ID was sent to you, so it was not sent, and no block with the same text will be sent to that \
request. No open chat request has been sent to you."]
        );
        assert!(
            state.inspect_request(&key).expect("inspect request")["replies"]
                .as_array()
                .expect("replies")
                .is_empty()
        );

        // While the coordinator's queue cannot be read, both paths hold the block the same way,
        // and report nothing, since a later capture decides.
        let (state, key, root) = state_with_request();
        assert!(state
            .prompt(&key, "")
            .expect("prompt")
            .contains("Include the line <CHAT_REPLY_001> at the beginning"));
        let mut routes = RouteCache::new(vec![state
            .next_reply_route(&key)
            .expect("route")
            .expect("open route")]);
        let client = HerdrClient::with_executable("direct", Path::new("/missing/herdr"))
            .expect("construct client");
        let manager = ManagedAgents::new(&client, &root.join("registry")).expect("manager");
        let mut transport = None;
        let mut control = PassControl {
            transport: &mut transport,
            stop: None,
        };
        let reports = [
            capture_direct(
                &state,
                &manager,
                DrainOptions::default(),
                &mut routes,
                "001",
                SnapshotInput {
                    text: block,
                    truncated: false,
                    revision: Some(1),
                },
                &mut control,
            )
            .expect("capture after an output event"),
            capture_recovery_snapshot(
                &state,
                &manager,
                DrainOptions::default(),
                &mut routes,
                SnapshotInput {
                    text: block,
                    truncated: false,
                    revision: Some(2),
                },
                &mut control,
            )
            .expect("capture of a whole read"),
        ];
        for report in reports {
            assert!(report.captured.is_empty(), "{:?}", report.captured);
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            assert_eq!(report.notes.len(), 1, "{:?}", report.notes);
            assert!(
                report.notes[0].contains("the coordinator's queue could not be read"),
                "{}",
                report.notes[0]
            );
        }
        assert!(
            state.inspect_request(&key).expect("inspect request")["replies"]
                .as_array()
                .expect("replies")
                .is_empty()
        );
        fs::remove_dir_all(root).expect("cleanup");
    }
}
