//! Launchable orchestration for the durable chat runtime.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::io;
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
    CoordinatorDeliveryResult, OutboundCancellation, ReplyRoute, ReplyRouteEntry,
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
const PROVIDER_JOIN_TIMEOUT: Duration = Duration::from_secs(45);
const SIGNAL_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

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

/// Close capture for one exact request and durably retire it when terminal and replay-safe.
pub fn close(state_root: &Path, key: &str) -> Result<Value, ChatServiceError> {
    let state = BridgeState::open(state_root)?;
    state.close_replies(key)?;
    Ok(json!({"closed": key}))
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
    let _pinned = chat_runtime::select_plugin(&inventory, state.config())?;
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
        outbound_cancellation.clone(),
    )?;
    let (notices, notice_receiver) = mpsc::sync_channel(PROVIDER_NOTICE_CAPACITY);
    let provider = match spawn_provider(
        state.clone(),
        Arc::clone(&stop),
        Arc::clone(&cancellation),
        Arc::clone(&output_wake),
        Arc::clone(&overflowed),
        notices,
    ) {
        Ok(provider) => provider,
        Err(error) => {
            stop.stop();
            outbound_cancellation.cancel();
            signal_handle.close();
            let _ = join_worker(signal_thread, "chat signal", SIGNAL_JOIN_TIMEOUT);
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

    stop.stop();
    outbound_cancellation.cancel();
    wake_output(&output_wake);
    if let Err(error) = cancel_provider(&cancellation) {
        stop.record_cleanup_error(error.to_string());
    }
    signal_handle.close();
    if let Err(error) = join_worker(signal_thread, "chat signal", SIGNAL_JOIN_TIMEOUT) {
        stop.record_cleanup_error(error.to_string());
    }
    if let Err(error) = join_worker(provider, "chat provider", PROVIDER_JOIN_TIMEOUT) {
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

fn join_worker(
    worker: thread::JoinHandle<()>,
    label: &str,
    timeout: Duration,
) -> Result<(), ChatServiceError> {
    let deadline = Instant::now() + timeout;
    while !worker.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if !worker.is_finished() {
        return Err(ChatServiceError::Worker(format!(
            "{label} worker did not stop within {}s; cleanup is uncertain",
            timeout.as_secs_f64()
        )));
    }
    worker
        .join()
        .map_err(|_| ChatServiceError::Worker(format!("{label} worker panicked")))
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
        if let Some(transport) = control.transport.as_mut() {
            match state.ensure_ack(key, transport) {
                Ok(AckResult::Disabled) => {}
                Ok(AckResult::Acked(_)) => report.acknowledged.push(key.clone()),
                Err(error) => report.error("acknowledge", key, error),
            }
        }
        if control.stopped() {
            report.more_work = true;
            break;
        }
        match chat_runtime::deliver_request_with(state, coordinator, key, delivery) {
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
        vec![r"^[^\S\r\n]*(?:[•⏺][ \t]+)?</(?:GCHAT|CHAT)_REPLY_[^<>\s]*>[^\S\r\n]*$".to_owned()]
    }
}

fn matched_identifier(line: &str) -> Option<&str> {
    let stripped = line.trim();
    let stripped = ["• ", "⏺ "]
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
}

impl StopState {
    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.changed.notify_all();
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

const CANCELLED_PROCESS_RECEIVE_CODE: &str = "plugin_worker_stopped";

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
    active: Option<ActiveProviderCancellation>,
}

type SharedCancellation = Arc<Mutex<ProviderCancellationRegistry>>;
type SharedWake = Arc<Mutex<Option<PaneEventWake>>>;

struct ProviderCancellationRegistration {
    registry: SharedCancellation,
    identity: ProviderGenerationIdentity,
}

impl ProviderCancellationRegistration {
    fn install(
        registry: &SharedCancellation,
        authority: Arc<dyn ChatSubscriptionCancellation>,
        receive_tracker: Arc<ReceiveOperationTracker>,
    ) -> std::result::Result<Self, String> {
        let mut state = registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.active.is_some() {
            return Err("a prior provider cancellation authority remains registered".to_owned());
        }
        state.last_identity = state
            .last_identity
            .checked_add(1)
            .ok_or_else(|| "provider generation identity space is exhausted".to_owned())?;
        let identity = ProviderGenerationIdentity(state.last_identity);
        state.active = Some(ActiveProviderCancellation {
            identity,
            authority,
            receive_tracker,
            succeeded: false,
            cancelled_receive: None,
        });
        drop(state);
        Ok(Self {
            registry: Arc::clone(registry),
            identity,
        })
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
    }
}

fn spawn_provider(
    state: BridgeState,
    stop: Arc<StopState>,
    cancellation: SharedCancellation,
    output_wake: SharedWake,
    overflowed: Arc<AtomicBool>,
    notices: mpsc::SyncSender<ProviderNotice>,
) -> Result<thread::JoinHandle<()>, ChatServiceError> {
    thread::Builder::new()
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
        })
        .map_err(ChatServiceError::Io)
}

fn provider_generation(
    state: &BridgeState,
    stop: &StopState,
    cancellation: &SharedCancellation,
    notices: &mpsc::SyncSender<ProviderNotice>,
    output_wake: &SharedWake,
    overflowed: &AtomicBool,
) -> Result<(), ProviderGenerationError> {
    let inventory = crate::plugins::discover();
    let command = chat_runtime::select_plugin(&inventory, state.config())
        .map_err(|error| ProviderGenerationError::Retryable(error.to_string()))?;
    let timeouts = ProcessPhaseTimeouts::new(
        Duration::from_secs(10),
        Duration::from_secs(30),
        Duration::from_secs(30),
        Duration::from_secs(10),
        Duration::from_secs(2),
    )
    .map_err(|error| ProviderGenerationError::Retryable(error.to_string()))?;
    let (backend, process_cancellation) = command
        .connect(timeouts)
        .map_err(|error| ProviderGenerationError::Retryable(error.to_string()))?;
    let receive_tracker = Arc::new(ReceiveOperationTracker::default());
    let registration = ProviderCancellationRegistration::install(
        cancellation,
        Arc::new(process_cancellation),
        Arc::clone(&receive_tracker),
    )
    .map_err(ProviderGenerationError::Cleanup)?;
    let mut backend = ReceiveFailureTrackingBackend {
        inner: backend,
        receive_tracker: Arc::clone(&receive_tracker),
    };
    if stop.is_stopped() {
        cancel_provider(cancellation)
            .map_err(|error| ProviderGenerationError::Cleanup(error.to_string()))?;
        return Err(ProviderGenerationError::Cancelled);
    }
    let request = state.subscribe_request().map_err(|error| match error {
        ChatRuntimeError::UnresolvedGap(detail) => ProviderGenerationError::Fatal(detail),
        other => ProviderGenerationError::Retryable(other.to_string()),
    })?;
    let mut subscription = ChatSubscription::open(&mut backend, &request)
        .map_err(|error| ProviderGenerationError::Retryable(error.to_string()))?;
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

fn classify_provider_receive_failure(
    stop: &StopState,
    cancellation: &SharedCancellation,
    identity: ProviderGenerationIdentity,
    receive_tracker: &ReceiveOperationTracker,
    error: ChatRuntimeError,
) -> ProviderGenerationError {
    let cancelled_receive = cancellation
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .active
        .as_ref()
        .filter(|active| active.identity == identity && active.succeeded)
        .and_then(|active| active.cancelled_receive);
    let cancellation_receive_failure = matches!(
        &error,
        ChatRuntimeError::Subscription(SubscriptionError::Backend(failure))
            if failure.code() == CANCELLED_PROCESS_RECEIVE_CODE && failure.retryable()
    );
    if stop.is_stopped()
        && cancelled_receive.is_some()
        && receive_tracker.failed() == cancelled_receive
        && cancellation_receive_failure
    {
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

fn wake_output(output_wake: &SharedWake) {
    if let Some(wake) = output_wake
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        wake.wake();
    }
}

fn spawn_signal_worker(
    stop: Arc<StopState>,
    cancellation: SharedCancellation,
    output_wake: SharedWake,
    outbound_cancellation: OutboundCancellation,
) -> Result<(SignalHandle, thread::JoinHandle<()>), ChatServiceError> {
    let mut signals = Signals::new([SIGINT, SIGTERM])?;
    let handle = signals.handle();
    let worker = thread::Builder::new()
        .name("agentctl-chat-signals".to_owned())
        .spawn(move || {
            if signals.forever().next().is_some() {
                stop.stop();
                outbound_cancellation.cancel();
                wake_output(&output_wake);
                if let Err(error) = cancel_provider(&cancellation) {
                    stop.record_cleanup_error(error.to_string());
                }
            }
        })?;
    Ok((handle, worker))
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
                    let wake = connected
                        .wake_handle()
                        .map_err(|error| ChatServiceError::Generation(error.to_string()))?;
                    *output_wake
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(wake);
                    subscribed_patterns = desired_patterns;
                    stream = Some(connected);
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
    use std::fs;
    use std::num::NonZeroU16;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    use std::sync::Condvar;

    use chat_subscription::{
        BackendCapabilities, BackendFailure, CancellationError, ChannelId, ChatSubscriptionBackend,
        ChatSubscriptionCancellation, ChatSubscriptionDriver, CommittableEvent, DeliveryBatch,
        DeliveryId, EventKind, EventSequence, InboundMessage, MessageId, ProviderCursor,
        ReplaySupport, SenderId, SubscribeRequest, SubscriptionItem, ThreadId,
    };

    static NEXT_STATE: AtomicU64 = AtomicU64::new(1);

    #[derive(Default)]
    struct RecordingDelivery {
        states: Mutex<BTreeMap<String, QueueMessageState>>,
        prompts: Mutex<Vec<String>>,
    }

    struct BlockingFailureBackend {
        gate: Arc<(Mutex<bool>, Condvar)>,
        entered: Arc<(Mutex<bool>, Condvar)>,
        failure_code: &'static str,
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

    impl ChatSubscriptionCancellation for GateCancellation {
        fn cancel(&self) -> std::result::Result<(), CancellationError> {
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
