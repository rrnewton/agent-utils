//! Durable, serialized messaging for interactive agents hosted by Herdr.
//!
//! This module owns durable FIFO files, target validation, idle/done readiness, atomic multiline
//! submission, working-state confirmation, and at-most-once quarantine after an ambiguous pane
//! injection. Queue and target locks use the command's stable, package-independent disk format.

use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::Serialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::client::{AgentPaneInfo, HerdrClient, Pane};
use crate::error::{HerdrRunError, EXIT_BUSY, EXIT_TIMEOUT};

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const MESSAGE_ID_MAX: usize = 255;
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Identity assertions for one already-running interactive Herdr agent.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Target {
    /// Exact pane identifier, when known.
    pub pane_id: Option<String>,
    /// Agent name associated with the stable session value, when known.
    pub session_agent: Option<String>,
    /// Stable interactive-agent session value, when known.
    pub session_value: Option<String>,
    /// Expected live agent implementation.
    pub expected_agent: Option<String>,
    /// Expected live workspace label.
    pub expected_workspace: Option<String>,
    /// Expected live working directory.
    pub expected_cwd: Option<PathBuf>,
}

/// Overall state produced by one queue drain.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueOutcome {
    /// Every valid prompt examined by this drain was confirmed delivered.
    Delivered,
    /// At least one prompt remains safely queued before injection.
    Pending,
    /// At least one prompt may have been injected and was quarantined rather than retried.
    PossiblySubmitted,
}

impl QueueOutcome {
    /// Return the stable machine-readable spelling used by both packages.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::Pending => "pending",
            Self::PossiblySubmitted => "possibly_submitted",
        }
    }
}

/// Structured outcome of one durable queue send or drain operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QueueResult {
    /// Identifier of the prompt created by `send`, or an empty string for a plain drain.
    pub message_id: String,
    /// Prompt identifiers whose working transition was confirmed.
    pub delivered: Vec<String>,
    /// Prompt identifiers retained in `failed` because retrying could be unsafe.
    pub quarantined: Vec<String>,
    /// Prompt identifiers that remain safe in `inbox`.
    pub pending: Vec<String>,
    /// Readiness or identity failure that stopped FIFO progress.
    pub blocked: Option<String>,
    /// Machine-readable aggregate state.
    pub outcome: QueueOutcome,
}

/// Validated live-agent identity plus observational queue state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QueueStatus {
    /// Resolved live pane identifier.
    pub pane_id: String,
    /// Live interactive-agent implementation, when reported.
    pub agent: Option<String>,
    /// Native live Herdr agent state.
    pub agent_status: String,
    /// Agent name from the stable session identity, when present.
    pub session_agent: Option<String>,
    /// Stable session value, when present.
    pub session_value: Option<String>,
    /// Owning workspace identifier.
    pub workspace_id: String,
    /// Live pane working directory.
    pub cwd: String,
    /// Prompt identifiers waiting safely before injection.
    pub pending: Vec<String>,
    /// Prompt identifiers behind the durable injection-intent barrier.
    pub inflight: Vec<String>,
    /// Prompt identifiers retained after malformed or ambiguous delivery.
    pub failed: Vec<String>,
}

/// One durable prompt that was not confirmed delivered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UndeliveredMessage {
    /// Human-readable diagnostic.
    pub message: String,
    /// Stable prompt identifier.
    pub message_id: String,
    /// Durable artifact holding the prompt.
    pub artifact: PathBuf,
}

/// Typed failure outcomes for interactive-agent delivery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentError {
    /// Herdr itself or its control protocol is unavailable.
    Client(HerdrRunError),
    /// Delivery could not proceed, with no more specific send outcome.
    Delivery(String),
    /// Nothing was injected; the durable prompt remains safe to retry.
    Pending(UndeliveredMessage),
    /// Injection may have succeeded; an automatic retry could duplicate a turn.
    PossiblySubmitted(UndeliveredMessage),
}

impl AgentError {
    fn delivery(message: impl Into<String>) -> Self {
        Self::Delivery(message.into())
    }

    /// Return the process status assigned to this failure.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Client(error) => error.exit_code(),
            Self::Delivery(_) | Self::Pending(_) => EXIT_BUSY,
            Self::PossiblySubmitted(_) => EXIT_TIMEOUT,
        }
    }

    /// Return the durable message details for typed send outcomes.
    #[must_use]
    pub const fn undelivered(&self) -> Option<&UndeliveredMessage> {
        match self {
            Self::Pending(message) | Self::PossiblySubmitted(message) => Some(message),
            Self::Client(_) | Self::Delivery(_) => None,
        }
    }

    /// Return the stable machine-readable outcome for typed send failures.
    #[must_use]
    pub const fn outcome(&self) -> Option<QueueOutcome> {
        match self {
            Self::Client(_) | Self::Delivery(_) => None,
            Self::Pending(_) => Some(QueueOutcome::Pending),
            Self::PossiblySubmitted(_) => Some(QueueOutcome::PossiblySubmitted),
        }
    }

    /// Report whether repeating the same send is known to be safe.
    #[must_use]
    pub const fn safe_to_retry(&self) -> bool {
        matches!(self, Self::Pending(_))
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Client(error) => fmt::Display::fmt(error, formatter),
            Self::Delivery(message) => formatter.write_str(message),
            Self::Pending(message) | Self::PossiblySubmitted(message) => {
                formatter.write_str(&message.message)
            }
        }
    }
}

impl std::error::Error for AgentError {}

/// Result type used by durable interactive-agent operations.
pub type AgentResult<T> = std::result::Result<T, AgentError>;

/// Herdr operations required by the durable interactive-agent transport.
pub trait AgentApi: Send + Sync {
    /// List every live pane.
    fn panes(&self) -> crate::error::Result<Vec<Pane>>;
    /// Read validated identity and readiness fields for one pane.
    fn pane_info(&self, pane_id: &str) -> crate::error::Result<AgentPaneInfo>;
    /// Resolve one exact workspace identifier to its live label.
    fn workspace_label(&self, workspace_id: &str) -> crate::error::Result<String>;
    /// Atomically type and submit one prompt.
    fn run(&self, pane_id: &str, text: &str) -> crate::error::Result<()>;
    /// Wait for a native agent-state transition.
    fn wait_agent_status(
        &self,
        pane_id: &str,
        status: &str,
        timeout_ms: u64,
    ) -> crate::error::Result<()>;
    /// Read rendered terminal text.
    fn read(
        &self,
        pane_id: &str,
        source: &str,
        lines: Option<usize>,
    ) -> crate::error::Result<String>;
}

impl AgentApi for HerdrClient {
    fn panes(&self) -> crate::error::Result<Vec<Pane>> {
        HerdrClient::panes(self, None)
    }

    fn pane_info(&self, pane_id: &str) -> crate::error::Result<AgentPaneInfo> {
        HerdrClient::pane_info(self, pane_id)
    }

    fn workspace_label(&self, workspace_id: &str) -> crate::error::Result<String> {
        HerdrClient::workspace_label(self, workspace_id)
    }

    fn run(&self, pane_id: &str, text: &str) -> crate::error::Result<()> {
        HerdrClient::run(self, pane_id, text)
    }

    fn wait_agent_status(
        &self,
        pane_id: &str,
        status: &str,
        timeout_ms: u64,
    ) -> crate::error::Result<()> {
        HerdrClient::wait_agent_status(self, pane_id, status, timeout_ms)
    }

    fn read(
        &self,
        pane_id: &str,
        source: &str,
        lines: Option<usize>,
    ) -> crate::error::Result<String> {
        HerdrClient::read(self, pane_id, source, lines)
    }
}

/// Timing and retry controls for one queue drain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DrainOptions {
    /// Maximum pre-injection wait for idle/done readiness.
    pub ready_timeout: Duration,
    /// Maximum post-injection wait for the native working transition.
    pub working_timeout: Duration,
    /// Maximum recorded delivery-attempt count accepted for a prompt.
    pub max_attempts: u64,
}

impl Default for DrainOptions {
    fn default() -> Self {
        Self {
            ready_timeout: Duration::from_secs(900),
            working_timeout: Duration::from_secs(30),
            max_attempts: 3,
        }
    }
}

/// Injectable clock used to make readiness polling deterministic in tests and embedders.
pub trait AgentRuntime {
    /// Return a monotonic duration from an arbitrary fixed origin.
    fn monotonic(&self) -> Duration;
    /// Pause for at most `duration`.
    fn sleep(&self, duration: Duration);
}

/// Production wall-clock runtime for readiness polling.
#[derive(Clone, Copy, Debug)]
pub struct SystemRuntime {
    origin: Instant,
}

impl Default for SystemRuntime {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl AgentRuntime for SystemRuntime {
    fn monotonic(&self) -> Duration {
        self.origin.elapsed()
    }

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

/// Resolve by stable session when supplied, then revalidate every asserted field.
pub fn resolve_target<A: AgentApi + ?Sized>(
    client: &A,
    target: &Target,
) -> AgentResult<AgentPaneInfo> {
    validate_target_authority(target)?;
    let asserted_pane = target.pane_id.as_deref();
    let mut pane_id = target.pane_id.clone();
    if let Some(session_value) = target.session_value.as_deref() {
        let mut matches = Vec::new();
        for pane in client.panes().map_err(client_error)? {
            let info = client.pane_info(&pane.pane_id).map_err(client_error)?;
            if info.session_value.as_deref() == Some(session_value)
                && target
                    .session_agent
                    .as_deref()
                    .is_none_or(|agent| info.session_agent.as_deref() == Some(agent))
            {
                matches.push(pane.pane_id);
            }
        }
        if matches.len() != 1 {
            return Err(AgentError::delivery(format!(
                "expected exactly one live pane for session {session_value:?}, found {}",
                matches.len()
            )));
        }
        let resolved = matches.remove(0);
        if asserted_pane.is_some_and(|asserted| asserted != resolved) {
            return Err(AgentError::delivery(format!(
                "refusing session target pane {resolved:?}: expected exact pane {asserted_pane:?}"
            )));
        }
        pane_id = Some(resolved);
    }
    let pane_id = pane_id
        .ok_or_else(|| AgentError::delivery("target needs --pane or a stable session value"))?;
    let info = client.pane_info(&pane_id).map_err(client_error)?;
    validate_target(client, &info, target)?;
    Ok(info)
}

fn validate_target<A: AgentApi + ?Sized>(
    client: &A,
    info: &AgentPaneInfo,
    target: &Target,
) -> AgentResult<()> {
    let mut failures = Vec::new();
    if let Some(expected) = target.expected_agent.as_deref() {
        if info.agent.as_deref() != Some(expected) {
            failures.push(format!("agent is {:?}, expected {expected:?}", info.agent));
        }
    }
    if let Some(expected) = target.session_agent.as_deref() {
        if info.session_agent.as_deref() != Some(expected) {
            failures.push(format!(
                "session agent is {:?}, expected {expected:?}",
                info.session_agent
            ));
        }
    }
    if let Some(expected) = target.session_value.as_deref() {
        if info.session_value.as_deref() != Some(expected) {
            failures.push(format!(
                "session is {:?}, expected {expected:?}",
                info.session_value
            ));
        }
    }
    if let Some(expected) = target.expected_workspace.as_deref() {
        let actual = client
            .workspace_label(&info.workspace_id)
            .map_err(client_error)?;
        if actual != expected {
            failures.push(format!("workspace is {actual:?}, expected {expected:?}"));
        }
    }
    if let Some(expected) = target.expected_cwd.as_deref() {
        let actual = real_path(Path::new(&info.cwd))?;
        let expected = real_path(expected)?;
        if actual != expected {
            failures.push(format!(
                "cwd is {:?}, expected {:?}",
                info.cwd,
                expected.display().to_string()
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(AgentError::delivery(format!(
            "refusing pane {}: {}",
            info.pane_id,
            failures.join("; ")
        )))
    }
}

/// Persist one prompt before any readiness or transport operation.
pub fn enqueue(root: &Path, text: &str, message_id: Option<&str>) -> AgentResult<String> {
    enqueue_internal(root, text, message_id, true)
}

fn enqueue_internal(
    root: &Path,
    text: &str,
    message_id: Option<&str>,
    serialize: bool,
) -> AgentResult<String> {
    if text.is_empty() {
        return Err(AgentError::delivery("message must not be empty"));
    }
    let directories = prepare(root)?;
    let identifier = message_id.map_or_else(generated_message_id, str::to_owned);
    validate_message_id(&identifier)?;
    let filename = format!("{identifier}.json");
    let path = directories.inbox.join(&filename);
    let _lock = if serialize {
        let lock_path = root.join(".delivery.lock");
        let lock = open_private_lock(&lock_path, "queue delivery lock")?;
        FileExt::lock_exclusive(&lock)
            .map_err(|error| io_error("lock queue delivery", &lock_path, error))?;
        Some(lock)
    } else {
        None
    };
    if directories
        .all()
        .iter()
        .any(|directory| fs::symlink_metadata(directory.join(&filename)).is_ok())
    {
        return Err(AgentError::delivery(format!(
            "message id already exists: {identifier}"
        )));
    }
    let document = json!({
        "id": identifier,
        "text": text,
        "queued_at": unix_seconds(),
        "delivery_attempts": 0,
    });
    atomic_json_create(&path, &document).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            AgentError::delivery(format!("message id already exists: {identifier}"))
        } else {
            io_error("persist queued message", &path, error)
        }
    })?;
    Ok(identifier)
}

/// Serialize and drain a FIFO with production timing.
pub fn drain<A: AgentApi + ?Sized>(
    client: &A,
    target: &Target,
    root: &Path,
    options: DrainOptions,
) -> AgentResult<QueueResult> {
    drain_with_runtime(client, target, root, options, &SystemRuntime::default())
}

/// Serialize and drain a FIFO using an injected runtime.
pub fn drain_with_runtime<A: AgentApi + ?Sized, R: AgentRuntime + ?Sized>(
    client: &A,
    target: &Target,
    root: &Path,
    options: DrainOptions,
    runtime: &R,
) -> AgentResult<QueueResult> {
    bind_queue(root, target)?;
    let directories = prepare(root)?;
    let queue_lock_path = root.join(".delivery.lock");
    let queue_lock = open_private_lock(&queue_lock_path, "queue delivery lock")?;
    FileExt::lock_exclusive(&queue_lock)
        .map_err(|error| io_error("lock queue delivery", &queue_lock_path, error))?;
    let mut delivered = Vec::new();
    let mut quarantined = recover_inflight(&directories)?;
    let mut blocked = None;
    let mut target_lock: Option<File> = None;
    let mut locked_pane_id: Option<String> = None;
    let mut initial_info: Option<AgentPaneInfo> = None;
    for path in json_paths(&directories.inbox)? {
        let (mut document, attempts) = match load_message(&path).and_then(|document| {
            let attempts = delivery_attempts(&document, &path)?;
            Ok((document, attempts))
        }) {
            Ok(loaded) => loaded,
            Err(error) => {
                let identifier = quarantine_raw(
                    &path,
                    &directories.failed,
                    "invalid_message",
                    &error.to_string(),
                )?;
                quarantined.push(identifier);
                continue;
            }
        };
        let filename_id = message_id_from_path(&path)?;
        let identifier = document
            .get("id")
            .and_then(Value::as_str)
            .map_or(filename_id.clone(), str::to_owned);
        if attempts >= options.max_attempts {
            let detail = format!(
                "message {identifier} reached the maximum delivery-attempt count ({attempts} >= {}); retained pending",
                options.max_attempts
            );
            document.insert("delivery_state".to_owned(), json!("pending"));
            document.insert("delivery_error".to_owned(), json!(detail));
            document.insert("delivery_blocked_at".to_owned(), json!(unix_seconds()));
            atomic_json(&path, &Value::Object(document))?;
            blocked = Some(detail);
            break;
        }

        let readiness = (|| -> AgentResult<AgentPaneInfo> {
            if target_lock.is_none() {
                let (lock, info) = lock_resolved_target(client, target)?;
                target_lock = Some(lock);
                locked_pane_id = Some(info.pane_id.clone());
                initial_info = Some(info);
            }
            wait_ready(
                client,
                target,
                options.ready_timeout,
                runtime,
                locked_pane_id
                    .as_deref()
                    .expect("target lock always records its pane"),
                initial_info.take(),
            )
        })();
        let info = match readiness {
            Ok(info) => info,
            Err(error) => {
                let detail = error.to_string();
                document.insert("delivery_state".to_owned(), json!("pending"));
                document.insert("delivery_error".to_owned(), json!(detail));
                document.insert("delivery_blocked_at".to_owned(), json!(unix_seconds()));
                atomic_json(&path, &Value::Object(document))?;
                blocked = Some(detail);
                break;
            }
        };

        let inflight_path = directories.inflight.join(path.file_name().ok_or_else(|| {
            AgentError::delivery(format!(
                "queued message has no filename: {}",
                path.display()
            ))
        })?);
        transition(&path, &inflight_path)?;
        document.insert("possibly_submitted".to_owned(), Value::Bool(true));
        document.insert("delivery_state".to_owned(), json!("inflight"));
        document.insert("inflight_at".to_owned(), json!(unix_seconds()));
        atomic_json(&inflight_path, &Value::Object(document.clone()))?;

        match deliver_one(
            client,
            &info,
            &message_text(&document)?,
            options.working_timeout,
        ) {
            Ok(()) => {
                document.insert("delivery_state".to_owned(), json!("processed"));
                document.insert("confirmed_at".to_owned(), json!(unix_seconds()));
                atomic_json(&inflight_path, &Value::Object(document))?;
                transition(
                    &inflight_path,
                    &directories
                        .processed
                        .join(path.file_name().expect("validated queued filename")),
                )?;
                delivered.push(identifier);
            }
            Err(error) => {
                let attempts = attempts.saturating_add(1);
                let detail = error.to_string();
                document.insert("delivery_attempts".to_owned(), json!(attempts));
                document.insert("tui_delivery_attempts".to_owned(), json!(attempts));
                document.insert("delivery_error".to_owned(), json!(detail));
                document.insert("possibly_submitted".to_owned(), Value::Bool(true));
                document.insert("delivery_failed_at".to_owned(), json!(unix_seconds()));
                atomic_json(&inflight_path, &Value::Object(document))?;
                let failed_path = directories
                    .failed
                    .join(path.file_name().expect("validated queued filename"));
                transition(&inflight_path, &failed_path)?;
                failed_metadata(&failed_path, "possibly_submitted", &detail)?;
                quarantined.push(identifier);
            }
        }
    }

    let pending = identifiers(&directories.inbox)?;
    let outcome = if blocked.is_some() {
        QueueOutcome::Pending
    } else if quarantined.is_empty() {
        QueueOutcome::Delivered
    } else {
        QueueOutcome::PossiblySubmitted
    };
    Ok(QueueResult {
        message_id: String::new(),
        delivered,
        quarantined,
        pending,
        blocked,
        outcome,
    })
}

/// Durably enqueue one prompt, drain its bound FIFO, and require confirmed delivery.
pub fn send<A: AgentApi + ?Sized>(
    client: &A,
    target: &Target,
    root: &Path,
    text: &str,
    options: DrainOptions,
) -> AgentResult<QueueResult> {
    send_with_runtime(
        client,
        target,
        root,
        text,
        options,
        &SystemRuntime::default(),
    )
}

/// Send one prompt using an injected readiness runtime.
pub fn send_with_runtime<A: AgentApi + ?Sized, R: AgentRuntime + ?Sized>(
    client: &A,
    target: &Target,
    root: &Path,
    text: &str,
    options: DrainOptions,
    runtime: &R,
) -> AgentResult<QueueResult> {
    bind_queue(root, target)?;
    // Generated identifiers plus no-replace creation make persistence safe without waiting behind
    // a long-running drain. The drain and terminal-artifact inspection below resolve any message
    // consumed by another sender between these phases.
    let identifier = enqueue_internal(root, text, None, false)?;
    let result = drain_with_runtime(client, target, root, options, runtime)?;
    let filename = format!("{identifier}.json");
    let failed_path = root.join("failed").join(&filename);
    if result.quarantined.contains(&identifier) || fs::symlink_metadata(&failed_path).is_ok() {
        let detail = load_message(&failed_path)
            .ok()
            .and_then(|document| {
                document
                    .get("delivery_error")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "unknown delivery failure".to_owned());
        return Err(AgentError::PossiblySubmitted(UndeliveredMessage {
            message: format!(
                "message {identifier} has an ambiguous outcome after one injection: {detail}; it is retained under {}/failed",
                root.display()
            ),
            message_id: identifier,
            artifact: failed_path,
        }));
    }
    let inflight_path = root.join("inflight").join(&filename);
    if fs::symlink_metadata(&inflight_path).is_ok() {
        return Err(AgentError::PossiblySubmitted(UndeliveredMessage {
            message: format!(
                "message {identifier} remains behind the durable inflight barrier; automatic resubmission is unsafe"
            ),
            message_id: identifier,
            artifact: inflight_path,
        }));
    }
    let inbox_path = root.join("inbox").join(&filename);
    if result.pending.contains(&identifier) || fs::symlink_metadata(&inbox_path).is_ok() {
        return Err(AgentError::Pending(UndeliveredMessage {
            message: format!(
                "message {identifier} remains pending without consuming a retry attempt: {}",
                result
                    .blocked
                    .as_deref()
                    .unwrap_or("unknown readiness failure")
            ),
            message_id: identifier.clone(),
            artifact: inbox_path,
        }));
    }
    let processed_path = root.join("processed").join(&filename);
    if !result.delivered.contains(&identifier) && fs::symlink_metadata(&processed_path).is_err() {
        return Err(AgentError::delivery(format!(
            "message {identifier} disappeared without a durable terminal artifact"
        )));
    }
    let mut delivered = result.delivered;
    if !delivered.contains(&identifier) {
        delivered.push(identifier.clone());
    }
    Ok(QueueResult {
        message_id: identifier,
        delivered,
        quarantined: result.quarantined,
        pending: result.pending,
        blocked: result.blocked,
        outcome: QueueOutcome::Delivered,
    })
}

/// Read validated live-agent and queue state without creating or changing queue files.
pub fn status<A: AgentApi + ?Sized>(
    client: &A,
    target: &Target,
    root: &Path,
) -> AgentResult<QueueStatus> {
    validate_existing_queue(root)?;
    validate_existing_binding(root, target)?;
    let info = resolve_target(client, target)?;
    let directories = QueueDirectories::new(root);
    Ok(QueueStatus {
        pane_id: info.pane_id,
        agent: info.agent,
        agent_status: info.status,
        session_agent: info.session_agent,
        session_value: info.session_value,
        workspace_id: info.workspace_id,
        cwd: info.cwd,
        pending: identifiers_if_directory(&directories.inbox)?,
        inflight: identifiers_if_directory(&directories.inflight)?,
        failed: identifiers_if_directory(&directories.failed)?,
    })
}

/// Read recent terminal output from a validated interactive-agent target.
pub fn read<A: AgentApi + ?Sized>(
    client: &A,
    target: &Target,
    lines: usize,
) -> AgentResult<String> {
    let info = resolve_target(client, target)?;
    let text = client
        .read(&info.pane_id, "recent-unwrapped", Some(lines))
        .map_err(client_error)?;
    if text.is_empty() {
        client
            .read(&info.pane_id, "recent", Some(lines))
            .map_err(client_error)
    } else {
        Ok(text)
    }
}

/// Return the stable SHA-256 lock identity for one resolved live pane.
#[must_use]
pub fn pane_lock_digest(pane_id: &str) -> String {
    let identity = json!({"kind": "pane", "pane_id": pane_id});
    let encoded = serde_json::to_vec(&identity).expect("JSON value serialization cannot fail");
    format!("{:x}", Sha256::digest(encoded))
}

fn wait_ready<A: AgentApi + ?Sized, R: AgentRuntime + ?Sized>(
    client: &A,
    target: &Target,
    timeout: Duration,
    runtime: &R,
    locked_pane_id: &str,
    mut initial_info: Option<AgentPaneInfo>,
) -> AgentResult<AgentPaneInfo> {
    let deadline = runtime.monotonic().saturating_add(timeout);
    loop {
        let info = match initial_info.take() {
            Some(info) => info,
            None => resolve_target(client, target)?,
        };
        if info.pane_id != locked_pane_id {
            return Err(AgentError::delivery(format!(
                "target moved from locked pane {locked_pane_id:?} to {:?}",
                info.pane_id
            )));
        }
        if matches!(info.status.as_str(), "idle" | "done") {
            return Ok(info);
        }
        if info.status == "blocked" {
            return Err(AgentError::delivery(format!(
                "pane {} is blocked; resolve its visible prompt",
                info.pane_id
            )));
        }
        let now = runtime.monotonic();
        if now >= deadline {
            return Err(AgentError::delivery(format!(
                "pane {} did not become idle/done within {}s; last status={}",
                info.pane_id,
                timeout.as_secs_f64(),
                info.status
            )));
        }
        runtime.sleep(POLL_INTERVAL.min(deadline.saturating_sub(runtime.monotonic())));
    }
}

fn deliver_one<A: AgentApi + ?Sized>(
    client: &A,
    info: &AgentPaneInfo,
    text: &str,
    working_timeout: Duration,
) -> AgentResult<()> {
    client.run(&info.pane_id, text).map_err(|error| {
        AgentError::delivery(format!(
            "pane {} pane-run outcome is unknown; prompt may have been submitted: {error}",
            info.pane_id
        ))
    })?;
    let millis = working_timeout.as_millis().clamp(1, u64::MAX.into()) as u64;
    client
        .wait_agent_status(&info.pane_id, "working", millis)
        .map_err(|error| {
            AgentError::delivery(format!(
                "pane {} did not confirm idle/done -> working submission: {error}",
                info.pane_id
            ))
        })
}

#[derive(Clone, Debug)]
struct QueueDirectories {
    inbox: PathBuf,
    inflight: PathBuf,
    processed: PathBuf,
    failed: PathBuf,
}

impl QueueDirectories {
    fn new(root: &Path) -> Self {
        Self {
            inbox: root.join("inbox"),
            inflight: root.join("inflight"),
            processed: root.join("processed"),
            failed: root.join("failed"),
        }
    }

    fn all(&self) -> [&Path; 4] {
        [&self.inbox, &self.inflight, &self.processed, &self.failed]
    }
}

fn prepare(root: &Path) -> AgentResult<QueueDirectories> {
    let root_existed = fs::symlink_metadata(root).is_ok();
    create_private_directory(root, "queue directory", true, true)?;
    if !root_existed {
        if let Some(parent) = root.parent().filter(|path| !path.as_os_str().is_empty()) {
            sync_directory(parent)?;
        }
    }
    let directories = QueueDirectories::new(root);
    for path in directories.all() {
        create_private_directory(path, "queue state directory", true, true)?;
    }
    sync_directory(root)?;
    Ok(directories)
}

fn create_private_directory(
    path: &Path,
    purpose: &str,
    recursive: bool,
    tighten: bool,
) -> AgentResult<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(recursive).mode(0o700);
    if let Err(error) = builder.create(path) {
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(io_error(&format!("prepare {purpose}"), path, error));
        }
    }
    validate_private_directory(path, purpose, tighten)
}

fn validate_private_directory(path: &Path, purpose: &str, tighten: bool) -> AgentResult<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| io_error(&format!("inspect {purpose}"), path, error))?;
    let uid = unsafe { libc::getuid() };
    if !metadata.file_type().is_dir() || metadata.uid() != uid {
        return Err(AgentError::delivery(format!(
            "unsafe {purpose}: {}",
            path.display()
        )));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        if !tighten {
            return Err(AgentError::delivery(format!(
                "{purpose} is not private: {}",
                path.display()
            )));
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| io_error(&format!("make {purpose} private"), path, error))?;
    }
    Ok(())
}

fn open_private_lock(path: &Path, purpose: &str) -> AgentResult<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| io_error(&format!("open {purpose}"), path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| io_error(&format!("inspect {purpose}"), path, error))?;
    let uid = unsafe { libc::getuid() };
    if !metadata.file_type().is_file()
        || metadata.uid() != uid
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(AgentError::delivery(format!(
            "unsafe {purpose}: {}",
            path.display()
        )));
    }
    Ok(file)
}

fn binding(target: &Target) -> AgentResult<Value> {
    let expected_cwd = target
        .expected_cwd
        .as_deref()
        .map(real_path)
        .transpose()?
        .map(|path| path.display().to_string());
    if let Some(value) = target.session_value.as_deref() {
        Ok(json!({
            "kind": "session",
            "agent": target.session_agent,
            "value": value,
            "expected_agent": target.expected_agent,
            "expected_workspace": target.expected_workspace,
            "expected_cwd": expected_cwd,
        }))
    } else {
        Ok(json!({
            "kind": "pane",
            "pane_id": target.pane_id,
            "expected_agent": target.expected_agent,
            "expected_workspace": target.expected_workspace,
            "expected_cwd": expected_cwd,
        }))
    }
}

fn bind_queue(root: &Path, target: &Target) -> AgentResult<()> {
    validate_target_authority(target)?;
    prepare(root)?;
    let lock_path = root.join(".binding.lock");
    let binding_path = root.join("target.json");
    let lock = open_private_lock(&lock_path, "queue binding lock")?;
    FileExt::lock_exclusive(&lock)
        .map_err(|error| io_error("lock queue binding", &lock_path, error))?;
    let expected = binding(target)?;
    match fs::symlink_metadata(&binding_path) {
        Ok(_) => {
            let actual = read_private_json(&binding_path)?;
            if actual != expected {
                return Err(AgentError::delivery(format!(
                    "queue {} is bound to {actual}, refusing different target {expected}",
                    root.display()
                )));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            atomic_json(&binding_path, &expected)?;
        }
        Err(error) => {
            return Err(io_error(
                "inspect queue target binding",
                &binding_path,
                error,
            ))
        }
    }
    Ok(())
}

fn validate_existing_binding(root: &Path, target: &Target) -> AgentResult<()> {
    let binding_path = root.join("target.json");
    let actual = match fs::symlink_metadata(&binding_path) {
        Ok(_) => read_private_json(&binding_path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(io_error(
                "inspect queue target binding",
                &binding_path,
                error,
            ))
        }
    };
    let expected = binding(target)?;
    if actual != expected {
        return Err(AgentError::delivery(format!(
            "queue {} is bound to {actual}, refusing different target {expected}",
            root.display()
        )));
    }
    Ok(())
}

fn validate_existing_queue(root: &Path) -> AgentResult<()> {
    match fs::symlink_metadata(root) {
        Ok(_) => validate_private_directory(root, "queue directory", false)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error("inspect queue directory", root, error)),
    }
    for directory in QueueDirectories::new(root).all() {
        match fs::symlink_metadata(directory) {
            Ok(_) => validate_private_directory(directory, "queue state directory", false)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("inspect queue state directory", directory, error)),
        }
    }
    Ok(())
}

fn validate_target_authority(target: &Target) -> AgentResult<()> {
    let pane = target
        .pane_id
        .as_deref()
        .is_some_and(|value| !value.is_empty());
    let session = target
        .session_value
        .as_deref()
        .is_some_and(|value| !value.is_empty());
    if pane || session {
        Ok(())
    } else {
        Err(AgentError::delivery(
            "target needs --pane or a stable session value",
        ))
    }
}

fn target_lock_path(pane_id: &str) -> AgentResult<PathBuf> {
    let root = Path::new("/tmp").join(format!("herdr-agent-target-locks-{}", unsafe {
        libc::getuid()
    }));
    create_private_directory(&root, "host-wide target lock directory", false, false)?;
    Ok(root.join(format!("{}.lock", pane_lock_digest(pane_id))))
}

fn lock_resolved_target<A: AgentApi + ?Sized>(
    client: &A,
    target: &Target,
) -> AgentResult<(File, AgentPaneInfo)> {
    let initial = resolve_target(client, target)?;
    let lock_path = target_lock_path(&initial.pane_id)?;
    let lock = open_private_lock(&lock_path, "host-wide target lock")?;
    FileExt::lock_exclusive(&lock)
        .map_err(|error| io_error("lock interactive-agent target", &lock_path, error))?;
    let confirmed = resolve_target(client, target)?;
    if confirmed.pane_id != initial.pane_id {
        return Err(AgentError::delivery(format!(
            "target moved from pane {:?} to {:?} while waiting for its host-wide lock",
            initial.pane_id, confirmed.pane_id
        )));
    }
    Ok((lock, confirmed))
}

fn atomic_json(path: &Path, value: &Value) -> AgentResult<()> {
    let parent = path.parent().ok_or_else(|| {
        AgentError::delivery(format!("JSON path has no parent: {}", path.display()))
    })?;
    let (temporary, mut file) = temporary_file(parent)?;
    let write_result = (|| -> io::Result<()> {
        {
            let mut writer = BufWriter::new(&mut file);
            serde_json::to_writer_pretty(&mut writer, value).map_err(io::Error::other)?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        sync_directory_io(parent)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result.map_err(|error| io_error("write durable JSON", path, error))
}

fn atomic_json_create(path: &Path, value: &Value) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("JSON path has no parent"))?;
    let (temporary, mut file) = temporary_file_io(parent)?;
    let result = (|| {
        {
            let mut writer = BufWriter::new(&mut file);
            serde_json::to_writer_pretty(&mut writer, value).map_err(io::Error::other)?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
        file.sync_all()?;
        fs::hard_link(&temporary, path)?;
        fs::remove_file(&temporary)?;
        sync_directory_io(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn temporary_file(parent: &Path) -> AgentResult<(PathBuf, File)> {
    temporary_file_io(parent)
        .map_err(|error| io_error("create durable JSON temporary", parent, error))
}

fn temporary_file_io(parent: &Path) -> io::Result<(PathBuf, File)> {
    for _ in 0..100 {
        let path = parent.join(format!(
            ".message.{}.{}",
            std::process::id(),
            TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique JSON temporary file",
    ))
}

fn read_json(path: &Path) -> AgentResult<Value> {
    read_json_with_policy(path, false)
}

fn read_private_json(path: &Path) -> AgentResult<Value> {
    read_json_with_policy(path, true)
}

fn read_json_with_policy(path: &Path, require_private: bool) -> AgentResult<Value> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| io_error("read JSON", path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| io_error("inspect JSON", path, error))?;
    if !metadata.file_type().is_file()
        || metadata.uid() != unsafe { libc::getuid() }
        || require_private && metadata.permissions().mode() & 0o077 != 0
    {
        return Err(AgentError::delivery(format!(
            "unsafe JSON artifact: {}",
            path.display()
        )));
    }
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|error| io_error("read JSON", path, error))?;
    serde_json::from_str(&contents).map_err(|error| {
        AgentError::delivery(format!("cannot read JSON {}: {error}", path.display()))
    })
}

fn load_message(path: &Path) -> AgentResult<Map<String, Value>> {
    let value = read_json(path).map_err(|error| {
        AgentError::delivery(format!(
            "cannot read queued message {}: {error}",
            path.display()
        ))
    })?;
    let document = value.as_object().cloned().ok_or_else(|| {
        AgentError::delivery(format!(
            "queued message {} has no string text field",
            path.display()
        ))
    })?;
    if document
        .get("text")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err(AgentError::delivery(format!(
            "queued message {} must have a nonempty string text field",
            path.display()
        )));
    }
    if document.get("id").is_some_and(|value| !value.is_string()) {
        return Err(AgentError::delivery(format!(
            "queued message {} has a non-string id field",
            path.display()
        )));
    }
    Ok(document)
}

fn delivery_attempts(document: &Map<String, Value>, path: &Path) -> AgentResult<u64> {
    let (key, value) = if let Some(value) = document.get("delivery_attempts") {
        ("delivery_attempts", Some(value))
    } else {
        (
            "tui_delivery_attempts",
            document.get("tui_delivery_attempts"),
        )
    };
    match value {
        None => Ok(0),
        Some(value) => value.as_u64().ok_or_else(|| {
            AgentError::delivery(format!(
                "queued message {} has an invalid nonnegative integer {key} field",
                path.display()
            ))
        }),
    }
}

fn message_text(document: &Map<String, Value>) -> AgentResult<String> {
    document
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| AgentError::delivery("queued message has no string text field"))
}

fn transition(source: &Path, destination: &Path) -> AgentResult<()> {
    match fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(AgentError::delivery(format!(
                "refusing to replace durable queue artifact {}",
                destination.display()
            )))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(io_error(
                "inspect durable queue destination",
                destination,
                error,
            ))
        }
    }
    fs::rename(source, destination)
        .map_err(|error| io_error("transition durable queue artifact", source, error))?;
    let destination_parent = destination.parent().expect("queue destination has parent");
    sync_directory(destination_parent)?;
    let source_parent = source.parent().expect("queue source has parent");
    if source_parent != destination_parent {
        sync_directory(source_parent)?;
    }
    Ok(())
}

fn failed_metadata(failed_path: &Path, outcome: &str, error: &str) -> AgentResult<()> {
    let filename = failed_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AgentError::delivery("failed artifact name is not valid UTF-8"))?;
    atomic_json(
        &failed_path.with_file_name(format!("{filename}.error")),
        &json!({
            "artifact": filename,
            "outcome": outcome,
            "error": error,
            "failed_at": unix_seconds(),
        }),
    )
}

fn quarantine_raw(path: &Path, failed: &Path, outcome: &str, error: &str) -> AgentResult<String> {
    let filename = path.file_name().ok_or_else(|| {
        AgentError::delivery(format!(
            "queued message has no filename: {}",
            path.display()
        ))
    })?;
    let destination = failed.join(filename);
    transition(path, &destination)?;
    failed_metadata(&destination, outcome, error)?;
    message_id_from_path(path)
}

fn recover_inflight(directories: &QueueDirectories) -> AgentResult<Vec<String>> {
    let mut recovered = Vec::new();
    for path in json_paths(&directories.inflight)? {
        let filename = path.file_name().expect("listed path has filename");
        let destination = directories.failed.join(filename);
        transition(&path, &destination)?;
        failed_metadata(
            &destination,
            "possibly_submitted",
            "recovered an inflight prompt after process restart; refusing automatic resubmission",
        )?;
        recovered.push(message_id_from_path(&path)?);
    }
    Ok(recovered)
}

fn json_paths(directory: &Path) -> AgentResult<Vec<PathBuf>> {
    let entries = fs::read_dir(directory)
        .map_err(|error| io_error("list durable queue directory", directory, error))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|error| io_error("read durable queue directory entry", directory, error))?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.ends_with(".json"))
        {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

fn identifiers(directory: &Path) -> AgentResult<Vec<String>> {
    json_paths(directory)?
        .iter()
        .map(|path| message_id_from_path(path))
        .collect()
}

fn identifiers_if_directory(directory: &Path) -> AgentResult<Vec<String>> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.file_type().is_dir() => identifiers(directory),
        Ok(_) => Err(AgentError::delivery(format!(
            "unsafe queue state directory: {}",
            directory.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(io_error("inspect queue state directory", directory, error)),
    }
}

fn message_id_from_path(path: &Path) -> AgentResult<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".json"))
        .map(str::to_owned)
        .ok_or_else(|| {
            AgentError::delivery(format!(
                "queued message name is not valid UTF-8 JSON: {}",
                path.display()
            ))
        })
}

fn sync_directory(path: &Path) -> AgentResult<()> {
    sync_directory_io(path).map_err(|error| io_error("sync durable queue directory", path, error))
}

fn sync_directory_io(path: &Path) -> io::Result<()> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)?
        .sync_all()
}

fn validate_message_id(identifier: &str) -> AgentResult<()> {
    let bytes = identifier.as_bytes();
    let first_ok = bytes.first().is_some_and(u8::is_ascii_alphanumeric);
    let rest_ok = bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if !first_ok || !rest_ok || bytes.len() > MESSAGE_ID_MAX {
        return Err(AgentError::delivery(
            "message id must be 1-255 ASCII letters, digits, dots, underscores, or hyphens and must start with a letter or digit",
        ));
    }
    Ok(())
}

fn generated_message_id() -> String {
    let nanoseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{nanoseconds:020}-{}", std::process::id())
}

fn unix_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn real_path(path: &Path) -> AgentResult<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                AgentError::delivery(format!("cannot read current directory: {error}"))
            })?
            .join(path)
    };
    Ok(fs::canonicalize(&absolute).unwrap_or_else(|_| lexical_normalize(&absolute)))
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn client_error(error: HerdrRunError) -> AgentError {
    AgentError::Client(error)
}

fn io_error(action: &str, path: &Path, error: io::Error) -> AgentError {
    AgentError::delivery(format!("cannot {action} {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Instant;

    use super::*;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "herdr-agent-rust-{label}-{}-{}",
                std::process::id(),
                TEST_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed)
            ));
            fs::create_dir(&path).expect("create test directory");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .expect("set test directory mode");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Default)]
    struct FakeState {
        infos: BTreeMap<String, AgentPaneInfo>,
        states: VecDeque<String>,
        last_state: String,
        runs: Vec<String>,
        waits: Vec<(String, String, u64)>,
        reads: Vec<String>,
        fail_run: bool,
        fail_wait: bool,
        unwrapped_empty: bool,
    }

    struct FakeAgent {
        panes: Vec<Pane>,
        workspace_label: String,
        state: Mutex<FakeState>,
        run_gate: Option<Arc<RunGate>>,
    }

    impl FakeAgent {
        fn new(states: &[&str]) -> Self {
            let mut infos = BTreeMap::new();
            infos.insert("w1:p1".to_owned(), default_info("w1:p1", "session-1"));
            Self {
                panes: vec![Pane {
                    pane_id: "w1:p1".to_owned(),
                    tab_id: "w1:t1".to_owned(),
                    workspace_id: "w1".to_owned(),
                }],
                workspace_label: "deepscry".to_owned(),
                state: Mutex::new(FakeState {
                    infos,
                    states: states.iter().map(|state| (*state).to_owned()).collect(),
                    last_state: states.last().copied().unwrap_or("idle").to_owned(),
                    ..FakeState::default()
                }),
                run_gate: None,
            }
        }

        fn with_run_gate(states: &[&str], run_gate: Arc<RunGate>) -> Self {
            let mut fake = Self::new(states);
            fake.run_gate = Some(run_gate);
            fake
        }

        fn with_duplicate_session() -> Self {
            let mut fake = Self::new(&["idle"]);
            fake.panes.push(Pane {
                pane_id: "w1:p2".to_owned(),
                tab_id: "w1:t1".to_owned(),
                workspace_id: "w1".to_owned(),
            });
            fake.state
                .lock()
                .expect("fake state")
                .infos
                .insert("w1:p2".to_owned(), default_info("w1:p2", "session-1"));
            fake
        }

        fn runs(&self) -> Vec<String> {
            self.state.lock().expect("fake state").runs.clone()
        }

        fn waits(&self) -> Vec<(String, String, u64)> {
            self.state.lock().expect("fake state").waits.clone()
        }
    }

    impl AgentApi for FakeAgent {
        fn panes(&self) -> crate::error::Result<Vec<Pane>> {
            Ok(self.panes.clone())
        }

        fn pane_info(&self, pane_id: &str) -> crate::error::Result<AgentPaneInfo> {
            let mut state = self.state.lock().expect("fake state");
            let mut info = state
                .infos
                .get(pane_id)
                .cloned()
                .ok_or_else(|| HerdrRunError::unavailable("missing pane"))?;
            if let Some(next) = state.states.pop_front() {
                state.last_state = next;
            }
            info.status.clone_from(&state.last_state);
            Ok(info)
        }

        fn workspace_label(&self, _workspace_id: &str) -> crate::error::Result<String> {
            Ok(self.workspace_label.clone())
        }

        fn run(&self, _pane_id: &str, text: &str) -> crate::error::Result<()> {
            if let Some(gate) = &self.run_gate {
                gate.enter_and_wait();
            }
            let mut state = self.state.lock().expect("fake state");
            state.runs.push(text.to_owned());
            if state.fail_run {
                Err(HerdrRunError::unavailable(
                    "connection vanished after write",
                ))
            } else {
                Ok(())
            }
        }

        fn wait_agent_status(
            &self,
            pane_id: &str,
            status: &str,
            timeout_ms: u64,
        ) -> crate::error::Result<()> {
            let mut state = self.state.lock().expect("fake state");
            state
                .waits
                .push((pane_id.to_owned(), status.to_owned(), timeout_ms));
            if state.fail_wait {
                Err(HerdrRunError::unavailable("no working transition"))
            } else {
                Ok(())
            }
        }

        fn read(
            &self,
            _pane_id: &str,
            source: &str,
            _lines: Option<usize>,
        ) -> crate::error::Result<String> {
            let mut state = self.state.lock().expect("fake state");
            state.reads.push(source.to_owned());
            if source == "recent-unwrapped" && state.unwrapped_empty {
                Ok(String::new())
            } else {
                Ok("agent transcript\n".to_owned())
            }
        }
    }

    #[derive(Default)]
    struct RunGate {
        state: Mutex<(usize, bool)>,
        changed: Condvar,
    }

    impl RunGate {
        fn enter_and_wait(&self) {
            let mut state = self.state.lock().expect("run gate");
            state.0 += 1;
            self.changed.notify_all();
            while !state.1 {
                state = self.changed.wait(state).expect("run gate wait");
            }
        }

        fn wait_for_entries(&self, expected: usize, timeout: Duration) -> bool {
            let deadline = Instant::now() + timeout;
            let mut state = self.state.lock().expect("run gate");
            while state.0 < expected {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return false;
                }
                let (next, timed) = self
                    .changed
                    .wait_timeout(state, remaining)
                    .expect("run gate timed wait");
                state = next;
                if timed.timed_out() && state.0 < expected {
                    return false;
                }
            }
            true
        }

        fn release(&self) {
            let mut state = self.state.lock().expect("run gate");
            state.1 = true;
            self.changed.notify_all();
        }
    }

    #[derive(Default)]
    struct FakeRuntime {
        millis: AtomicU64,
    }

    impl AgentRuntime for FakeRuntime {
        fn monotonic(&self) -> Duration {
            Duration::from_millis(self.millis.load(AtomicOrdering::SeqCst))
        }

        fn sleep(&self, duration: Duration) {
            self.millis.fetch_add(
                u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                AtomicOrdering::SeqCst,
            );
        }
    }

    fn default_info(pane_id: &str, session: &str) -> AgentPaneInfo {
        AgentPaneInfo {
            pane_id: pane_id.to_owned(),
            workspace_id: "w1".to_owned(),
            cwd: "/work/mtg".to_owned(),
            agent: Some("codex".to_owned()),
            status: "idle".to_owned(),
            session_agent: Some("codex".to_owned()),
            session_value: Some(session.to_owned()),
        }
    }

    fn target() -> Target {
        Target {
            pane_id: Some("w1:p1".to_owned()),
            session_agent: Some("codex".to_owned()),
            session_value: Some("session-1".to_owned()),
            expected_agent: Some("codex".to_owned()),
            expected_workspace: Some("deepscry".to_owned()),
            expected_cwd: Some(PathBuf::from("/work/mtg")),
        }
    }

    #[test]
    fn resolved_pane_lock_digest_is_byte_for_byte_compatible() {
        assert_eq!(
            pane_lock_digest("w1:p1"),
            "a176b65b6a799f512519f02899c894b47e31ae17567009e92b90383974dcd38c"
        );
    }

    #[test]
    fn session_resolution_refuses_ambiguity_and_contradictory_exact_pane() {
        let fake = FakeAgent::new(&["idle"]);
        let empty = Target {
            pane_id: Some(String::new()),
            ..Target::default()
        };
        let error = resolve_target(&fake, &empty).unwrap_err();
        assert!(error.to_string().contains("target needs"));

        let ambiguous = FakeAgent::with_duplicate_session();
        let error = resolve_target(&ambiguous, &target()).unwrap_err();
        assert!(error.to_string().contains("found 2"));

        let mut contradiction = target();
        contradiction.pane_id = Some("w1:p9".to_owned());
        let error = resolve_target(&fake, &contradiction).unwrap_err();
        assert!(error.to_string().contains("expected exact pane"));
    }

    #[test]
    fn multiline_busy_then_idle_is_atomic_and_confirmed() {
        let directory = TestDirectory::new("multiline");
        let fake = FakeAgent::new(&["working", "working", "idle"]);
        let runtime = FakeRuntime::default();
        let text = "first line\nsecond line\nthird line";
        let result = send_with_runtime(
            &fake,
            &target(),
            directory.path(),
            text,
            DrainOptions {
                ready_timeout: Duration::from_secs(1),
                ..DrainOptions::default()
            },
            &runtime,
        )
        .unwrap();
        assert_eq!(fake.runs(), [text]);
        assert_eq!(
            fake.waits(),
            [("w1:p1".to_owned(), "working".to_owned(), 30_000)]
        );
        assert_eq!(result.delivered, [result.message_id]);
    }

    #[test]
    fn done_is_submit_safe_and_zero_working_timeout_still_waits_one_millisecond() {
        let directory = TestDirectory::new("done");
        let fake = FakeAgent::new(&["done"]);
        let result = send(
            &fake,
            &target(),
            directory.path(),
            "from done",
            DrainOptions {
                working_timeout: Duration::ZERO,
                ..DrainOptions::default()
            },
        )
        .unwrap();
        assert_eq!(result.outcome, QueueOutcome::Delivered);
        assert_eq!(fake.runs(), ["from done"]);
        assert_eq!(fake.waits()[0].2, 1);
    }

    #[test]
    fn concurrent_senders_serialize_within_and_across_queue_roots() {
        for distinct_roots in [false, true] {
            let parent = TestDirectory::new(if distinct_roots {
                "distinct-roots"
            } else {
                "same-root"
            });
            let root_a = parent.path().join("queue-a");
            let root_b = if distinct_roots {
                parent.path().join("queue-b")
            } else {
                root_a.clone()
            };
            let gate = Arc::new(RunGate::default());
            let fake = Arc::new(FakeAgent::with_run_gate(&["idle"], gate.clone()));

            let first_fake = fake.clone();
            let first_target = target();
            let first = thread::spawn(move || {
                send(
                    first_fake.as_ref(),
                    &first_target,
                    &root_a,
                    "first",
                    DrainOptions::default(),
                )
            });
            assert!(gate.wait_for_entries(1, Duration::from_secs(5)));

            let second_fake = fake.clone();
            let second_target = target();
            let second = thread::spawn(move || {
                send(
                    second_fake.as_ref(),
                    &second_target,
                    &root_b,
                    "second",
                    DrainOptions::default(),
                )
            });
            assert!(
                !gate.wait_for_entries(2, Duration::from_millis(100)),
                "a second sender reached pane.run before the first released its lock"
            );
            assert!(fake.runs().is_empty());
            gate.release();
            first.join().expect("first sender").unwrap();
            second.join().expect("second sender").unwrap();
            assert_eq!(fake.runs(), ["first", "second"]);
        }
    }

    #[test]
    fn readiness_timeout_is_typed_pending_and_consumes_no_attempt() {
        let directory = TestDirectory::new("pending");
        let fake = FakeAgent::new(&["working"]);
        let error = send_with_runtime(
            &fake,
            &target(),
            directory.path(),
            "poll artifact survives",
            DrainOptions {
                ready_timeout: Duration::ZERO,
                ..DrainOptions::default()
            },
            &FakeRuntime::default(),
        )
        .unwrap_err();
        assert_eq!(error.exit_code(), 75);
        assert_eq!(error.outcome(), Some(QueueOutcome::Pending));
        assert!(error.safe_to_retry());
        let artifact = &error.undelivered().expect("pending details").artifact;
        let document = read_json(artifact).unwrap();
        assert_eq!(document["text"], "poll artifact survives");
        assert_eq!(document["delivery_attempts"], 0);
        assert!(json_paths(&directory.path().join("failed"))
            .unwrap()
            .is_empty());
        assert!(fake.runs().is_empty());
    }

    #[test]
    fn blocked_agent_is_pending_without_injection() {
        let directory = TestDirectory::new("blocked");
        let fake = FakeAgent::new(&["blocked"]);
        let error = send(
            &fake,
            &target(),
            directory.path(),
            "do not type",
            DrainOptions::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("visible prompt"));
        assert!(matches!(error, AgentError::Pending(_)));
        assert!(fake.runs().is_empty());
    }

    #[test]
    fn post_injection_failures_are_quarantined_once_as_possibly_submitted() {
        for failure in ["run", "wait"] {
            let directory = TestDirectory::new(failure);
            let fake = FakeAgent::new(&["idle"]);
            {
                let mut state = fake.state.lock().unwrap();
                state.fail_run = failure == "run";
                state.fail_wait = failure == "wait";
            }
            let error = send(
                &fake,
                &target(),
                directory.path(),
                "only once",
                DrainOptions::default(),
            )
            .unwrap_err();
            assert_eq!(error.exit_code(), 76);
            assert_eq!(error.outcome(), Some(QueueOutcome::PossiblySubmitted));
            assert!(!error.safe_to_retry());
            let artifact = &error.undelivered().expect("ambiguous details").artifact;
            let document = read_json(artifact).unwrap();
            assert_eq!(document["possibly_submitted"], true);
            assert_eq!(document["delivery_attempts"], 1);
            assert_eq!(fake.runs(), ["only once"]);
            assert!(json_paths(&directory.path().join("inflight"))
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn restart_quarantines_inflight_without_resubmission() {
        let directory = TestDirectory::new("restart");
        let identifier = enqueue(directory.path(), "at most once", Some("000000000007")).unwrap();
        fs::rename(
            directory.path().join("inbox/000000000007.json"),
            directory.path().join("inflight/000000000007.json"),
        )
        .unwrap();
        let fake = FakeAgent::new(&["idle"]);
        let result = drain(&fake, &target(), directory.path(), DrainOptions::default()).unwrap();
        assert_eq!(result.outcome, QueueOutcome::PossiblySubmitted);
        assert_eq!(result.quarantined, [identifier]);
        assert!(fake.runs().is_empty());
        assert!(directory
            .path()
            .join("failed/000000000007.json.error")
            .is_file());
    }

    #[test]
    fn malformed_fifo_head_preserves_raw_and_does_not_block_valid_prompt() {
        let directory = TestDirectory::new("malformed");
        prepare(directory.path()).unwrap();
        let raw = b"{not json\n";
        fs::write(directory.path().join("inbox/000000000001.json"), raw).unwrap();
        fs::write(
            directory.path().join("inbox/000000000002.json"),
            br#"{"id":[],"text":"invalid id"}
"#,
        )
        .unwrap();
        fs::write(
            directory.path().join("inbox/000000000003.json"),
            br#"{"id":"good","text":"deliver me"}
"#,
        )
        .unwrap();
        let fake = FakeAgent::new(&["idle"]);
        let result = drain(&fake, &target(), directory.path(), DrainOptions::default()).unwrap();
        assert_eq!(
            fs::read(directory.path().join("failed/000000000001.json")).unwrap(),
            raw
        );
        let metadata = read_json(&directory.path().join("failed/000000000001.json.error")).unwrap();
        assert_eq!(metadata["outcome"], "invalid_message");
        assert!(directory.path().join("failed/000000000002.json").is_file());
        assert_eq!(result.quarantined, ["000000000001", "000000000002"]);
        assert_eq!(result.delivered, ["good"]);
        assert_eq!(fake.runs(), ["deliver me"]);
    }

    #[test]
    fn queue_binding_refuses_a_different_session_without_moving_prompt() {
        let directory = TestDirectory::new("binding");
        let fake = FakeAgent::new(&["working"]);
        let first = send(
            &fake,
            &target(),
            directory.path(),
            "bound prompt",
            DrainOptions {
                ready_timeout: Duration::ZERO,
                ..DrainOptions::default()
            },
        );
        assert!(matches!(first, Err(AgentError::Pending(_))));
        let mut different = target();
        different.session_value = Some("different".to_owned());
        let error =
            drain(&fake, &different, directory.path(), DrainOptions::default()).unwrap_err();
        assert!(error.to_string().contains("bound to"));
        let pending = json_paths(&directory.path().join("inbox")).unwrap();
        assert_eq!(pending.len(), 1);
        assert!(fs::read_to_string(&pending[0])
            .unwrap()
            .contains("bound prompt"));
    }

    #[test]
    fn status_is_read_only_and_read_falls_back_from_empty_unwrapped_source() {
        let parent = TestDirectory::new("status");
        let root = parent.path().join("absent");
        let fake = FakeAgent::new(&["idle"]);
        fake.state.lock().unwrap().unwrapped_empty = true;
        let snapshot = status(&fake, &target(), &root).unwrap();
        assert!(snapshot.pending.is_empty());
        assert!(snapshot.inflight.is_empty());
        assert!(snapshot.failed.is_empty());
        assert!(!root.exists());
        assert_eq!(read(&fake, &target(), 17).unwrap(), "agent transcript\n");
        assert_eq!(
            fake.state.lock().unwrap().reads,
            ["recent-unwrapped", "recent"]
        );
    }

    #[test]
    fn status_rejects_unsafe_existing_queue_without_mutating_it() {
        let parent = TestDirectory::new("status-safety");
        let root = parent.path().join("queue");
        prepare(&root).unwrap();
        bind_queue(&root, &target()).unwrap();
        let fake = FakeAgent::new(&["idle"]);

        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        let error = status(&fake, &target(), &root).unwrap_err();
        assert!(error.to_string().contains("not private"));
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o755
        );

        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(root.join("target.json"), fs::Permissions::from_mode(0o644)).unwrap();
        let error = status(&fake, &target(), &root).unwrap_err();
        assert!(error.to_string().contains("unsafe JSON artifact"));

        fs::set_permissions(root.join("target.json"), fs::Permissions::from_mode(0o600)).unwrap();
        let link = parent.path().join("queue-link");
        symlink(&root, &link).unwrap();
        let error = status(&fake, &target(), &link).unwrap_err();
        assert!(error.to_string().contains("unsafe queue directory"));
    }

    #[test]
    fn existing_python_subagent_message_shape_is_accepted() {
        let directory = TestDirectory::new("legacy");
        prepare(directory.path()).unwrap();
        fs::write(
            directory.path().join("inbox/000000000007.json"),
            br#"{"seq":7,"text":"legacy fifo","tui_delivery_attempts":0}
"#,
        )
        .unwrap();
        let fake = FakeAgent::new(&["idle"]);
        let result = drain(&fake, &target(), directory.path(), DrainOptions::default()).unwrap();
        assert_eq!(result.delivered, ["000000000007"]);
        assert_eq!(fake.runs(), ["legacy fifo"]);
        assert!(directory
            .path()
            .join("processed/000000000007.json")
            .is_file());
    }

    #[test]
    fn message_ids_and_queue_files_cannot_escape_the_private_root() {
        let directory = TestDirectory::new("ids");
        for identifier in ["../escape", "/absolute", "bad space", "é", "", ".hidden"] {
            let error = enqueue(directory.path(), "message", Some(identifier)).unwrap_err();
            assert!(error.to_string().contains("message id must"));
        }
        let identifier = "a".repeat(256);
        assert!(enqueue(directory.path(), "message", Some(&identifier)).is_err());
        assert!(!directory.0.parent().unwrap().join("escape.json").exists());

        let accepted = enqueue(directory.path(), "message", Some("A.good_id-7")).unwrap();
        assert_eq!(accepted, "A.good_id-7");
        assert!(enqueue(directory.path(), "other", Some("A.good_id-7"))
            .unwrap_err()
            .to_string()
            .contains("already exists"));
    }

    #[test]
    fn queue_directories_tighten_legacy_modes_but_reject_symlinks() {
        let parent = TestDirectory::new("directory-safety");
        let root = parent.path().join("queue");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        enqueue(&root, "legacy", Some("legacy")).unwrap();
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for directory in QueueDirectories::new(&root).all() {
            assert_eq!(
                fs::metadata(directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }

        let symlink_root = parent.path().join("queue-link");
        symlink(&root, &symlink_root).unwrap();
        let error = enqueue(&symlink_root, "unsafe", Some("unsafe")).unwrap_err();
        assert!(error.to_string().contains("unsafe queue directory"));
    }

    #[test]
    fn planted_delivery_lock_symlink_is_refused_without_touching_target() {
        let directory = TestDirectory::new("lock-symlink");
        enqueue(directory.path(), "queued", Some("queued")).unwrap();
        let victim = directory.path().join("victim");
        fs::write(&victim, b"unchanged").unwrap();
        fs::remove_file(directory.path().join(".delivery.lock")).unwrap();
        symlink(&victim, directory.path().join(".delivery.lock")).unwrap();
        let fake = FakeAgent::new(&["idle"]);
        let error = drain(&fake, &target(), directory.path(), DrainOptions::default()).unwrap_err();
        assert!(error.to_string().contains("queue delivery lock"));
        assert_eq!(fs::read(&victim).unwrap(), b"unchanged");
        assert!(fake.runs().is_empty());
    }

    #[test]
    fn invalid_json_domains_and_empty_text_are_quarantined() {
        for (name, payload) in [
            ("nan", r#"{"id":"bad","text":NaN}"#),
            (
                "bool-attempts",
                r#"{"id":"bad","text":"prompt","delivery_attempts":true}"#,
            ),
            ("empty", r#"{"id":"bad","text":""}"#),
        ] {
            let directory = TestDirectory::new(name);
            let directories = prepare(directory.path()).unwrap();
            fs::write(directories.inbox.join("bad.json"), payload).unwrap();
            let result = drain(
                &FakeAgent::new(&["idle"]),
                &target(),
                directory.path(),
                DrainOptions::default(),
            )
            .unwrap();
            assert_eq!(result.quarantined, ["bad"]);
        }
    }

    #[test]
    fn exhausted_head_is_pending_and_missing_target_mutates_nothing() {
        let directory = TestDirectory::new("exhausted");
        let directories = prepare(directory.path()).unwrap();
        fs::write(
            directories.inbox.join("exhausted.json"),
            r#"{"id":"exhausted","text":"keep me","delivery_attempts":1}"#,
        )
        .unwrap();
        let result = drain(
            &FakeAgent::new(&["idle"]),
            &target(),
            directory.path(),
            DrainOptions {
                max_attempts: 1,
                ..DrainOptions::default()
            },
        )
        .unwrap();
        assert_eq!(result.outcome, QueueOutcome::Pending);
        assert_eq!(result.pending, ["exhausted"]);
        assert!(result
            .blocked
            .as_deref()
            .is_some_and(|value| value.contains("maximum delivery-attempt")));

        let missing = directory.path().join("missing-target");
        let error = send(
            &FakeAgent::new(&["idle"]),
            &Target::default(),
            &missing,
            "do not strand",
            DrainOptions::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("target needs"));
        assert!(!missing.exists());
    }
}
