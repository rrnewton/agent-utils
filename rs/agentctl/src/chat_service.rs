//! Launchable orchestration for the durable chat runtime.

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
use crate::chat_events::{PaneEvent, PaneEventStream, PaneEventWake};
use crate::chat_runtime::{
    self, AckResult, BridgeConfiguration, BridgeState, ChatRuntimeError, CommandOutboundTransport,
    CoordinatorDeliveryResult, OutboundCancellation, OutboundFailure, ReplyRoute, ReplyRouteEntry,
    RootMessageSubmission, RootMessageTransport,
};
use crate::client::HerdrClient;
use crate::subagents::{ManagedAgents, ManagedApi};

const SNAPSHOT_LINES: usize = 4_000;
const SNAPSHOT_LINES_U32: u32 = 4_000;
const MAX_SNAPSHOT_BYTES: usize = 2 * 1_024 * 1_024;
const MAX_KEYS_PER_PASS: usize = 4;
const MAX_SENDS_PER_PASS: usize = 64;
const PROVIDER_NOTICE_CAPACITY: usize = 64;
const MAX_PROVIDER_NOTICES_PER_PASS: usize = 64;
const MAX_DIRECT_REQUEST_KEYS: usize = 2_048;
const MAX_IMMEDIATE_BACKLOG_CHUNKS: usize = 64;
const EVENT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const OUTPUT_RETRY_MAX: Duration = Duration::from_secs(60);
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
    /// More bounded work remains for a later event or recovery pass.
    pub more_work: bool,
    #[serde(skip)]
    deferred_keys: Vec<String>,
    #[serde(skip)]
    recovery_requested: bool,
    #[serde(skip)]
    processed_keys: Vec<String>,
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
        self.more_work |= other.more_work;
        self.deferred_keys.append(&mut other.deferred_keys);
        self.recovery_requested |= other.recovery_requested;
        self.processed_keys.append(&mut other.processed_keys);
    }

    fn error(&mut self, operation: &str, key: &str, error: impl fmt::Display) {
        if self.errors.len() < 128 {
            self.errors.push(format!("{operation} {key}: {error}"));
        }
        self.more_work = true;
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

/// Inspect one exact active retained request without writes or external service access.
pub fn inspect_request(state_root: &Path, key: &str) -> Result<Value, ChatServiceError> {
    Ok(BridgeState::inspect(state_root)?.inspect_request(key)?)
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
            eprintln!(
                "agentctl: graceful MAINPID {main_pid} did not exit within {}s; waiting for systemd's same-window control-group kill",
                SYSTEMD_GRACEFUL_STOP_DIAGNOSTIC_DEADLINE.as_secs()
            );
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
    let snapshot = manager.read(&state.config().agent_name, SNAPSHOT_LINES)?;
    validate_snapshot(&snapshot)?;
    let mut routes = RouteCache::from_entries(state.reply_route_entries()?);
    let mut transport = state.outbound_transport()?;
    let mut control = PassControl {
        transport: &mut transport,
        stop: None,
    };
    let mut report = recover_pass(&state, manager, options.delivery, &mut routes, &mut control)?;
    report.merge(capture_recovery_snapshot(
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
    )?);
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
    let state = BridgeState::open(state_root)?;
    let _resume_request = state.subscribe_request()?;
    validate_service_outbound(state.config())?;
    let outbound_cancellation = OutboundCancellation::new()?;
    let mut outbound = state.outbound_transport()?;
    if let Some(transport) = outbound.as_mut() {
        transport.set_cancellation(outbound_cancellation.clone());
    }
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

    let stop = Arc::new(StopState::default());
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
    let cleanup_errors = stop.take_cleanup_errors();
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

fn validate_snapshot(snapshot: &str) -> Result<(), ChatServiceError> {
    if snapshot.len() > MAX_SNAPSHOT_BYTES {
        return Err(ChatServiceError::Generation(format!(
            "retained coordinator output exceeds {MAX_SNAPSHOT_BYTES} bytes"
        )));
    }
    Ok(())
}

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
        let concurrent = if acknowledgement_enabled && control.transport.is_some() {
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
                eprintln!(
                    "agentctl: chat acknowledgement worker could not start for {key}; \
                     running synchronously: {spawn_error}"
                );
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
    validate_snapshot(snapshot.text)?;
    let capture = state.capture_snapshot(snapshot.text)?;
    let route_entries = capture.route_entries.clone();
    let mut report = CycleReport {
        captured: capture.replies.clone(),
        snapshot_truncated: snapshot.truncated,
        snapshot_revision: snapshot.revision,
        ..CycleReport::default()
    };
    if !capture.unknown_ids.is_empty() {
        match deliver_feedback(state, manager, &capture.unknown_ids, delivery, control.stop) {
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
    validate_snapshot(snapshot.text)?;
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
    let capture = state.capture_replies(&key, snapshot.text)?;
    let mut report = CycleReport {
        snapshot_truncated: snapshot.truncated,
        snapshot_revision: snapshot.revision,
        ..CycleReport::default()
    };
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

#[derive(Clone, Debug, Eq, PartialEq)]
enum MatchedRoute {
    Unknown,
    Stale,
    Current(String),
}

fn refresh_matched_route(
    state: &BridgeState,
    routes: &mut RouteCache,
    identifier: &str,
) -> Result<MatchedRoute, ChatRuntimeError> {
    let Some(key) = routes.key(identifier).map(str::to_owned) else {
        return Ok(if routes.knows_identifier_nonce(identifier) {
            MatchedRoute::Stale
        } else {
            MatchedRoute::Unknown
        });
    };
    let current = state.next_reply_route(&key)?;
    if current.as_ref().map(|route| route.identifier.as_str()) == Some(identifier) {
        Ok(MatchedRoute::Current(key))
    } else {
        routes.replace(&key, current);
        Ok(MatchedRoute::Stale)
    }
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
}

#[derive(Debug)]
struct RouteCache {
    by_identifier: BTreeMap<String, String>,
    by_key: BTreeMap<String, (String, Option<String>)>,
    by_nonce: BTreeMap<String, String>,
}

impl RouteCache {
    #[cfg(test)]
    fn new(routes: Vec<ReplyRoute>) -> Self {
        Self::from_entries(
            routes
                .into_iter()
                .filter_map(|route| {
                    let nonce = route.identifier.rsplit_once('_')?.0.to_owned();
                    Some(ReplyRouteEntry {
                        key: route.key,
                        nonce,
                        current_identifier: Some(route.identifier),
                    })
                })
                .collect(),
        )
    }

    fn from_entries(entries: Vec<ReplyRouteEntry>) -> Self {
        let mut result = Self {
            by_identifier: BTreeMap::new(),
            by_key: BTreeMap::new(),
            by_nonce: BTreeMap::new(),
        };
        for entry in entries {
            if let Some(identifier) = entry.current_identifier.as_ref() {
                result
                    .by_identifier
                    .insert(identifier.clone(), entry.key.clone());
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
            if let Some((nonce, _)) = route.identifier.rsplit_once('_') {
                let nonce = nonce.to_owned();
                self.by_nonce.insert(nonce.clone(), route.key.clone());
                self.by_key
                    .insert(route.key.clone(), (nonce, Some(route.identifier.clone())));
                self.by_identifier.insert(route.identifier, route.key);
            }
        } else if let Some((nonce, _)) = previous {
            self.by_nonce.insert(nonce.clone(), key.to_owned());
            self.by_key.insert(key.to_owned(), (nonce, None));
        }
    }

    fn key(&self, identifier: &str) -> Option<&str> {
        self.by_identifier.get(identifier).map(String::as_str)
    }

    fn knows_identifier_nonce(&self, identifier: &str) -> bool {
        identifier
            .rsplit_once('_')
            .is_some_and(|(nonce, _)| self.by_nonce.contains_key(nonce))
    }

    fn patterns(&self) -> Vec<String> {
        vec![r"^[^\S\r\n]*(?:[•⏺●][ \t]+)?</(?:GCHAT|CHAT)_REPLY_[^<>\s]*>[^\S\r\n]*$".to_owned()]
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

#[derive(Debug)]
enum ProviderNotice {
    Batch(Vec<String>),
    Error(String),
    Fatal(String),
    End,
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
}

impl StopState {
    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    fn stop(&self) {
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
}

fn deliver_feedback<A: ManagedApi + ?Sized>(
    state: &BridgeState,
    manager: &ManagedAgents<'_, A>,
    unknown_ids: &[String],
    options: DrainOptions,
    stop: Option<&StopState>,
) -> Result<CoordinatorDeliveryResult, ChatRuntimeError> {
    match stop {
        Some(stop) => chat_runtime::deliver_fence_feedback_with(
            state,
            &CancellableDelivery {
                manager,
                runtime: StopRuntime::new(stop),
            },
            unknown_ids,
            options,
        ),
        None => chat_runtime::deliver_fence_feedback(state, manager, unknown_ids, options),
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
    let (done_sender, done) = mpsc::sync_channel(1);
    let handle = thread::Builder::new()
        .name("agentctl-chat-provider".to_owned())
        .spawn(move || {
            let mut delay = Duration::from_secs(1);
            while !stop.is_stopped() {
                match provider_generation(
                    &state,
                    &stop,
                    &cancellation,
                    &notices,
                    &output_wake,
                    &overflowed,
                    selected_process_timeouts,
                ) {
                    Ok(()) if stop.is_stopped() => break,
                    Ok(()) => {
                        send_notice(&notices, ProviderNotice::End, &output_wake, &overflowed);
                    }
                    Err(ProviderGenerationError::Cancelled) if stop.is_stopped() => break,
                    Err(ProviderGenerationError::Cancelled) => {
                        stop.record_cleanup_error(
                            "provider reported cancellation without an owner stop",
                        );
                        break;
                    }
                    Err(ProviderGenerationError::Cleanup(error)) => {
                        stop.record_cleanup_error(format!(
                            "provider shutdown cleanup failed: {error}"
                        ));
                        break;
                    }
                    Err(error) if stop.is_stopped() => {
                        stop.record_cleanup_error(format!(
                            "provider failed before cancellation was observed: {error:?}"
                        ));
                        break;
                    }
                    Err(ProviderGenerationError::Retryable(error)) => send_notice(
                        &notices,
                        ProviderNotice::Error(error),
                        &output_wake,
                        &overflowed,
                    ),
                    Err(ProviderGenerationError::Fatal(error)) => {
                        let _ = notices.send(ProviderNotice::Fatal(error));
                        wake_output(&output_wake);
                        break;
                    }
                }
                if stop.is_stopped() {
                    break;
                }
                stop.wait(delay);
                delay = (delay * 2).min(Duration::from_secs(60));
            }
            let _ = done_sender.send(());
        })
        .map_err(ChatServiceError::Io)?;
    Ok(ServiceWorker { handle, done })
}

fn provider_generation(
    state: &BridgeState,
    stop: &StopState,
    cancellation: &SharedCancellation,
    notices: &mpsc::SyncSender<ProviderNotice>,
    output_wake: &SharedWake,
    overflowed: &AtomicBool,
    selected_process_timeouts: ProcessPhaseTimeouts,
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
    // is sticky, so the first wait returns and the owner loop rechecks every queued source.
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
    let initial =
        manager.read_with_runtime(&state.config().agent_name, SNAPSHOT_LINES, &owner_runtime)?;
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
    log_report(&initial_report);
    let recovery = recover_pass(state, manager, options.delivery, &mut routes, &mut control)?;
    enqueue_report_backlog(&mut direct_keys, &recovery, overflowed);
    log_report(&recovery);

    let mut stream: Option<PaneEventStream> = None;
    let mut subscribed_patterns = Vec::new();
    let mut output_retry_at = Instant::now();
    let mut output_retry_delay = Duration::from_secs(1);
    let mut next_reconciliation = Instant::now() + options.reconciliation_interval;
    while !stop.is_stopped() {
        for _ in 0..MAX_PROVIDER_NOTICES_PER_PASS {
            let Ok(notice) = notices.try_recv() else {
                break;
            };
            match notice {
                ProviderNotice::Batch(keys) => {
                    enqueue_direct_keys(&mut direct_keys, keys, overflowed)
                }
                ProviderNotice::Error(error) => eprintln!("agentctl: chat provider: {error}"),
                ProviderNotice::Fatal(error) => {
                    return Err(ChatServiceError::Generation(format!(
                        "chat provider stopped on an unrecoverable stream gap: {error}"
                    )));
                }
                ProviderNotice::End => {
                    eprintln!("agentctl: chat provider stream ended; reconnecting")
                }
            }
        }
        if direct_keys.is_empty() && overflowed.swap(false, Ordering::SeqCst) {
            let report = recover_pass(state, manager, options.delivery, &mut routes, &mut control)?;
            enqueue_report_backlog(&mut direct_keys, &report, overflowed);
            log_report(&report);
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
            log_report(&report);
            for key in &report.processed_keys {
                routes.replace(key, state.next_reply_route(key)?);
            }
        }

        if Instant::now() >= next_reconciliation {
            let info =
                manager.pane_info_with_runtime(&state.config().agent_name, &owner_runtime)?;
            if matches!(info.status.as_str(), "idle" | "done") {
                let snapshot = manager.read_with_runtime(
                    &state.config().agent_name,
                    SNAPSHOT_LINES,
                    &owner_runtime,
                )?;
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
                log_report(&report);
            }
            let report = recover_pass(state, manager, options.delivery, &mut routes, &mut control)?;
            enqueue_report_backlog(&mut direct_keys, &report, overflowed);
            log_report(&report);
            next_reconciliation = Instant::now() + options.reconciliation_interval;
        }

        let desired_patterns = if state.config().outbound_enabled {
            routes.patterns()
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
                    eprintln!("agentctl: chat output subscription: {error}");
                    output_retry_at = Instant::now() + output_retry_delay;
                    output_retry_delay = (output_retry_delay * 2).min(OUTPUT_RETRY_MAX);
                }
            }
        }

        if let Some(active) = stream.as_mut() {
            let subscribed_pane = active.pane_id().to_owned();
            let timeout = if direct_keys.is_empty() {
                next_reconciliation.saturating_duration_since(Instant::now())
            } else {
                Duration::ZERO
            };
            match active.wait(timeout) {
                Ok(events) => {
                    let info = manager
                        .pane_info_with_runtime(&state.config().agent_name, &owner_runtime)?;
                    if info.pane_id != subscribed_pane {
                        return Err(ChatServiceError::Generation(format!(
                            "coordinator moved from subscribed pane {subscribed_pane:?} to {:?}",
                            info.pane_id
                        )));
                    }
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
                                log_report(&report);
                            }
                            PaneEvent::Settled { status }
                                if info.status == status
                                    && matches!(status.as_str(), "idle" | "done") =>
                            {
                                let snapshot = manager.read_with_runtime(
                                    &state.config().agent_name,
                                    SNAPSHOT_LINES,
                                    &owner_runtime,
                                )?;
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
                                log_report(&report);
                                let recovery = recover_pass(
                                    state,
                                    manager,
                                    options.delivery,
                                    &mut routes,
                                    &mut control,
                                )?;
                                enqueue_report_backlog(&mut direct_keys, &recovery, overflowed);
                                log_report(&recovery);
                            }
                            PaneEvent::Settled { .. } => {}
                        }
                    }
                    if unknown_route_seen {
                        let snapshot = manager.read_with_runtime(
                            &state.config().agent_name,
                            SNAPSHOT_LINES,
                            &owner_runtime,
                        )?;
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
                        log_report(&report);
                    }
                }
                Err(error) => {
                    eprintln!("agentctl: chat output subscription: {error}");
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
                    ProviderNotice::Error(error) => {
                        eprintln!("agentctl: chat provider: {error}")
                    }
                    ProviderNotice::Fatal(error) => {
                        return Err(ChatServiceError::Generation(format!(
                            "chat provider stopped on an unrecoverable stream gap: {error}"
                        )));
                    }
                    ProviderNotice::End => {}
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

fn log_report(report: &CycleReport) {
    if report.has_errors() {
        for error in &report.errors {
            eprintln!("agentctl: chat operation: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::{BufRead, BufReader, Write as _};
    use std::num::NonZeroU16;
    #[cfg(target_os = "linux")]
    use std::os::fd::RawFd;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
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
                ProviderNotice::Error(phase.to_owned()),
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
                ProviderNotice::Error(observed) => assert_eq!(observed, phase),
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

    #[test]
    fn route_cache_uses_one_generic_line_pattern_and_updates_direct_id_index() {
        let first_key = "a".repeat(64);
        let second_key = "b".repeat(64);
        let mut routes = RouteCache::new(vec![
            ReplyRoute {
                key: first_key.clone(),
                identifier: "AAAAAAAAAAAAAAAAAAAAAA_1".to_owned(),
            },
            ReplyRoute {
                key: second_key,
                identifier: "BBBBBBBBBBBBBBBBBBBBBB_7".to_owned(),
            },
        ]);
        let patterns = routes.patterns();
        assert_eq!(patterns.len(), 1);
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
                identifier: "AAAAAAAAAAAAAAAAAAAAAA_2".to_owned(),
            }),
        );
        assert_eq!(routes.key("AAAAAAAAAAAAAAAAAAAAAA_1"), None);
        assert_eq!(routes.key("AAAAAAAAAAAAAAAAAAAAAA_2"), Some(key.as_str()));
    }

    #[test]
    fn empty_route_cache_uses_only_the_bounded_unavailable_fence_predicate() {
        let patterns = RouteCache::new(Vec::new()).patterns();
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
    fn one_generic_closing_predicate_routes_full_provider_batch_capacity() {
        let routes = (1..=chat_subscription::MAX_BATCH_EVENTS)
            .map(|index| ReplyRoute {
                key: format!("{index:064x}"),
                identifier: format!("AAAAAAAAAAAAAAAAAAAAAA_{index}"),
            })
            .collect::<Vec<_>>();
        let cache = RouteCache::new(routes);
        assert_eq!(cache.by_identifier.len(), 256);
        let patterns = cache.patterns();
        assert_eq!(patterns.len(), 1);
        assert!(patterns[0].starts_with("^[^\\S\\r\\n]*"));
        assert!(patterns[0].contains("</(?:GCHAT|CHAT)_REPLY_"));
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
    fn closed_nonce_index_keeps_repeated_old_markers_stale() {
        let (state, _, root) = state_with_request();
        let key = "a".repeat(64);
        let mut routes = RouteCache::from_entries(vec![ReplyRouteEntry {
            key: key.clone(),
            nonce: "AAAAAAAAAAAAAAAAAAAAAA".to_owned(),
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
}
