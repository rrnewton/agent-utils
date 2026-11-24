//! The deterministic per-team restore driver: it interprets the shared
//! `team_model` transition table until the team verifies healthy, hands
//! the remainder to its lead, or escalates to a human.
//!
//! State is its interface. It lives at
//! `<state>/agentctl/team-drivers/<key>/state.json`, schema
//! `agentctl-team-driver/v1`, beside `driver.lock` (the held flock) and
//! `alarm.json` (written only while escalation is undelivered). Readers
//! such as the topology peer adapter need only the top-level `state`
//! (uppercase state name), `incident`, and `held` fields; unknown fields
//! may be added within v1. State is persisted atomically before every
//! pending effect and after each completes, so a crash repeats at most
//! one idempotent effect.
//!
//! The driver and kick run only in the initial (host) PID namespace,
//! where systemd, privileged image mounts, and queue delivery work; a
//! future unit therefore starts on the host and creates its own scopes.
//! The record itself carries no worker PID fields. Any PID text quoted
//! inside `last_verify` is host evidence as the host tools reported it
//! and must never be compared with a namespace PID.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::agent::AgentError;
use crate::restore::{team_driver_dir, team_key, Host, RestoreEpoch};
use crate::team_model::{Abstract, Effect, Env, Mode, State};

/// Schema of the persisted driver record.
pub const DRIVER_STATE_SCHEMA: &str = "agentctl-team-driver/v1";
/// Schema of the `team kick` report wrapping that record.
pub const KICK_SCHEMA: &str = "agentctl-team-kick/v1";
/// Default seconds a sole coordinator acknowledgement may stay pending in
/// VERIFY before it counts as failed; the HANDOFF deadline shares it.
pub const DEFAULT_HANDOFF_TIMEOUT_SECONDS: u64 = 1800;
/// Bounded resident poll while a wake cannot be armed.
pub(crate) const RESIDENT_POLL_SECONDS: u64 = 5;

type Result<T> = std::result::Result<T, AgentError>;

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

/// One queued handoff to the team lead.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct HandoffRecord {
    /// Coordinator name the checklist was queued for.
    pub lead: String,
    /// Deterministic queue message ID.
    pub message_id: String,
    /// Failed check names quoted in the checklist.
    pub checks: Vec<String>,
    /// Unix seconds the queue accepted the checklist, once queued.
    pub sent_at: Option<f64>,
    /// Unix seconds at which HANDOFF expires into ESCALATE.
    pub deadline: Option<f64>,
}

/// Escalation bookkeeping for one incident.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct EscalationRecord {
    /// First successful route delivery, in Unix seconds.
    pub delivered_at: Option<f64>,
    /// Latest delivery attempt, in Unix seconds.
    pub last_attempt_at: Option<f64>,
    /// Earliest next attempt under backoff or the hourly reminder bound.
    pub next_attempt_at: Option<f64>,
    /// Latest delivered reminder after the first delivery.
    pub reminded_at: Option<f64>,
    /// The single resolved notice, once sent.
    pub resolved_at: Option<f64>,
}

/// The re-entry wake armed for the current concrete deadline.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct WakeRecord {
    /// Deadline the wake was armed for, in Unix seconds.
    pub deadline: Option<f64>,
    /// `armed` (timer will fire), `resident` (this process is waiting),
    /// or `none`.
    pub outcome: String,
    /// Transient timer unit when armed through systemd.
    pub unit: Option<String>,
}

/// Who asked for a `team act` request, proved from process ancestry
/// when an agent, never from an environment claim.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RequesterRecord {
    /// `agent` for a proved registered caller, else `other`.
    pub kind: String,
    /// Agent name, or the sender-header other label (`human@host`, ...).
    pub label: String,
    /// `proved` only when the CLI proved the caller from process
    /// ancestry at enqueue; anything else, including a hand-written
    /// request file or a terminal fallback label, is `claimed`.
    #[serde(default = "default_claimed")]
    pub proof: String,
}

fn default_claimed() -> String {
    "claimed".to_owned()
}

/// One allowlisted `team act` action. Typed end to end: no free-form
/// shell, unit, or argument string ever reaches an effect.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum TeamAction {
    /// Rebuild and return the current manifest; no effect.
    Facts,
    /// Revive one desired agent with a recorded native session.
    Revive {
        /// Registered agent to revive.
        agent: String,
    },
    /// Start one configured unit.
    StartUnit {
        /// Configured unit name.
        unit: String,
    },
    /// Reset-failed, then start, one configured unit.
    ResetUnit {
        /// Configured unit name.
        unit: String,
    },
    /// Drain one agent's queued prompts once its pane is at a prompt.
    Drain {
        /// Registered agent whose queue drains.
        agent: String,
    },
    /// Guarded process kill with every identity expectation required.
    KillPid {
        /// Process to signal.
        pid: u32,
        /// Expected command-line substring.
        expect_cmdline: String,
        /// Expected `/proc` start time, exactly as read.
        expect_start: String,
        /// Boot the expectations were read in.
        boot_id: String,
        /// Signal name or number; the effects layer supplies its default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<String>,
    },
    /// Run the full verify and require the explicit YES judgement.
    DeclareDone,
    /// Escalate bounded operator text through the usual routes.
    Escalate {
        /// Operator text, at most 1000 chars.
        text: String,
    },
}

impl TeamAction {
    /// The closed action name used in receipts and details.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Facts => "facts",
            Self::Revive { .. } => "revive",
            Self::StartUnit { .. } => "start-unit",
            Self::ResetUnit { .. } => "reset-unit",
            Self::Drain { .. } => "drain",
            Self::KillPid { .. } => "kill-pid",
            Self::DeclareDone => "declare-done",
            Self::Escalate { .. } => "escalate",
        }
    }
}

/// Schema of one durable `team act` request file.
pub const ACT_REQUEST_SCHEMA: &str = "agentctl-team-act-request/v1";

/// The terminal half of a request file and its state-record mirror.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActionReceipt {
    /// Terminal status: done, failed, refused, or unknown.
    pub status: String,
    /// One closed detail line.
    pub detail: String,
    /// Whole-verify result after the effect, when one ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_ok: Option<bool>,
    /// Explicit per-team YES judgement after the effect, when judged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explicit_yes: Option<bool>,
    /// Unix seconds execution began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<f64>,
    /// Unix seconds execution finished.
    pub finished_at: f64,
    /// Process exit the requesting CLI reports.
    pub exit_code: i32,
    /// Command line the guarded kill verified, for a kill-pid action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_cmdline: Option<String>,
    /// Start ticks the guarded kill verified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_start: Option<u64>,
    /// Working directory the guarded kill verified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_cwd: Option<String>,
    /// Guarded-kill log the performed kill was appended to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kill_log: Option<String>,
}

/// One create-only request under `actions/`, its terminal receipt
/// written back before the state record mirrors it. The files are the
/// durable idempotence ledger if an older writer ever rewrites state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActionRequestFile {
    /// Always [`ACT_REQUEST_SCHEMA`].
    pub schema: String,
    /// Incident the request is bound to.
    pub incident: String,
    /// Driver epoch as recorded in state at enqueue.
    pub epoch: Value,
    /// Caller-chosen idempotence slug, unique per incident.
    pub request_id: String,
    /// The typed action as canonical JSON.
    pub action: Value,
    /// SHA-256 of the canonical request fields.
    pub action_sha256: String,
    /// Validate and record only; run no effect.
    pub dry_run: bool,
    /// Who asked, proved from ancestry when an agent.
    pub requester: RequesterRecord,
    /// Unix seconds the request was enqueued.
    pub requested_at: f64,
    /// Terminal receipt, written back before state mirrors it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<ActionReceipt>,
}

/// One action's state-record mirror: enough to dedupe, resume, and
/// answer a poll without re-reading its request file.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActionRecord {
    /// Caller-chosen idempotence slug, unique per incident.
    pub request_id: String,
    /// Incident the request is bound to.
    pub incident: String,
    /// Driver epoch it was bound to at enqueue.
    pub epoch: Value,
    /// The typed action as canonical JSON.
    pub action: Value,
    /// SHA-256 of the canonical request fields.
    pub action_sha256: String,
    /// Validate and record only; run no effect.
    pub dry_run: bool,
    /// Who asked, proved from ancestry when an agent.
    pub requester: RequesterRecord,
    /// queued, running, done, failed, refused, or unknown.
    pub status: String,
    /// Unix seconds the request was enqueued.
    pub requested_at: f64,
    /// Unix seconds execution began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<f64>,
    /// Unix seconds execution finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<f64>,
    /// One closed detail line for the current status.
    pub detail: String,
    /// Whole-verify result after the effect, when one ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_ok: Option<bool>,
    /// Explicit per-team YES judgement after the effect, when judged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explicit_yes: Option<bool>,
    /// Process exit the requesting CLI reports when terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Command line the guarded kill verified, for a kill-pid action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_cmdline: Option<String>,
    /// Start ticks the guarded kill verified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_start: Option<u64>,
    /// Working directory the guarded kill verified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_cwd: Option<String>,
    /// Guarded-kill log the performed kill was appended to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kill_log: Option<String>,
}

/// What one performed guarded kill verified, for its receipt.
#[derive(Clone, Debug, PartialEq)]
pub struct KillEvidence {
    /// Command line that was verified.
    pub cmdline: String,
    /// Start ticks that were verified.
    pub starttime: u64,
    /// Working directory that was verified.
    pub cwd: Option<String>,
    /// Kill log the performed kill was appended to.
    pub log_path: String,
}

/// One manifest build: the bounded document plus the raw verify report
/// it was projected from (declare-done is judged on the raw rows).
#[derive(Clone, Debug, PartialEq)]
pub struct ManifestOutcome {
    /// The bounded `agentctl-team-manifest/v1` document.
    pub manifest: Value,
    /// The raw `agentctl-team-verify/v1` report inside it.
    pub verify: Value,
}

/// The outcome one concrete action execution returns to the core.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionOutcome {
    /// `done`, `failed`, `refused`, or `unknown`.
    pub status: String,
    /// One closed detail line; no free-form effect output.
    pub detail: String,
    /// Escalation delivery bookkeeping for an escalate action.
    pub escalation: Option<EscalationOutcome>,
    /// Verified identity evidence for a performed kill-pid action.
    pub kill: Option<KillEvidence>,
}

fn done_outcome(detail: &str) -> ActionOutcome {
    ActionOutcome {
        status: "done".to_owned(),
        detail: detail.to_owned(),
        escalation: None,
        kill: None,
    }
}

/// The persisted driver record: model snapshot plus concrete deadlines
/// and receipts the abstract machine deliberately does not carry.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DriverRecord {
    /// Always [`DRIVER_STATE_SCHEMA`].
    pub schema: String,
    /// Canonical registry this record drives.
    pub registry: String,
    /// Per-team key naming this state directory and wake unit.
    pub team_key: String,
    /// Uppercase mirror of `abstract.driver`, the peer-facing field.
    pub state: String,
    /// Peer-facing process mode mirror of `abstract.mode`.
    pub mode: Mode,
    /// The latest verify held a deliberately pinned unit or bridge.
    pub held: bool,
    /// Incident for this epoch at this registry (12 hex).
    pub incident: String,
    /// Restore epoch as recorded in restore reports.
    pub epoch: Value,
    /// Kernel boot identity for this epoch.
    pub boot_id: String,
    /// Model snapshot the next step interprets.
    #[serde(rename = "abstract")]
    pub abstract_state: Abstract,
    /// Effects of the last transition awaiting completion, in order.
    pub pending_effects: Vec<Effect>,
    /// Unix seconds the current driver state was entered.
    pub entered_at: f64,
    /// Concrete deadline for VERIFY ack, HANDOFF, or ESCALATE waits.
    pub deadline: Option<f64>,
    /// Escalation attempts this incident (mirror of the snapshot).
    pub attempts: u8,
    /// Whole latest verify report, when one has run this incident.
    pub last_verify: Value,
    /// Current handoff receipt, once queued.
    pub handoff: Option<HandoffRecord>,
    /// Escalation receipts and schedule.
    pub escalation: EscalationRecord,
    /// Armed or resident wake for `deadline`.
    pub wake: WakeRecord,
    /// Latest effect or load error, for the next reader.
    pub last_error: Option<String>,
    /// The whole latest recovery manifest, once a `team act` validation
    /// or verify-after built one (bounded, token-free by construction).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<Value>,
    /// This incident's `team act` receipts, mirrored from the durable
    /// per-request files under `actions/`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ActionRecord>,
    /// Unix seconds this record was last written.
    pub updated_at: f64,
}

/// Uppercase peer-facing name of a model state.
pub fn state_name(state: State) -> &'static str {
    match state {
        State::Healthy => "HEALTHY",
        State::Detect => "DETECT",
        State::Restore => "RESTORE",
        State::Verify => "VERIFY",
        State::Handoff => "HANDOFF",
        State::Escalate => "ESCALATE",
    }
}

/// One incident id: first 12 hex of the SHA-256 of registry and boot,
/// NUL-separated. Manager identity is deliberately not an input: it can
/// be unreachable from a team PID namespace while the same boot's host
/// side reads it, and keying on it would give one outage two incidents
/// (and two possible escalations). The boot and manager are recorded as
/// fields (`boot_id`, `epoch`) beside the incident; epoch comparison and
/// NewEpoch resets still use the full epoch, while handoff message
/// identity adds its own epoch and checklist discriminators.
pub fn incident_id(registry: &Path, epoch: &RestoreEpoch) -> String {
    let canonical = std::fs::canonicalize(registry).unwrap_or_else(|_| registry.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    hasher.update([0]);
    hasher.update(epoch.boot_id.as_bytes());
    format!("{:x}", hasher.finalize())[..12].to_owned()
}

/// Refuse driver and kick work from a descendant PID namespace, where
/// the user manager is unreachable and restore cannot create wrkslots
/// scopes or mount images. A half-restore there is worse than none: the
/// host-side driver (or its future unit) owns scope creation.
pub fn require_host_namespace(host: &dyn Host) -> Result<()> {
    if host.in_initial_pid_namespace() {
        Ok(())
    } else {
        Err(fail(
            "kick must run in the host namespace; the driver owns scope creation",
        ))
    }
}

impl DriverRecord {
    /// A fresh healthy record for one registry and epoch.
    pub fn new(registry: &Path, epoch: &RestoreEpoch, now: f64) -> Self {
        let canonical = std::fs::canonicalize(registry).unwrap_or_else(|_| registry.to_path_buf());
        let abstract_state = Abstract::initial(Env {
            missing: false,
            lead_up: true,
            route_ok: true,
            fixed: false,
        });
        Self {
            schema: DRIVER_STATE_SCHEMA.to_owned(),
            registry: canonical.display().to_string(),
            team_key: team_key(registry),
            state: state_name(abstract_state.driver).to_owned(),
            mode: abstract_state.mode,
            held: false,
            incident: incident_id(registry, epoch),
            epoch: epoch.as_json(),
            boot_id: epoch.boot_id.clone(),
            abstract_state,
            pending_effects: Vec::new(),
            entered_at: now,
            deadline: None,
            attempts: 0,
            last_verify: Value::Null,
            handoff: None,
            escalation: EscalationRecord::default(),
            wake: WakeRecord {
                outcome: "none".to_owned(),
                ..WakeRecord::default()
            },
            last_error: None,
            manifest: None,
            actions: Vec::new(),
            updated_at: now,
        }
    }

    /// Re-derive the peer-facing mirrors from the model snapshot.
    pub fn sync_mirrors(&mut self, now: f64) {
        self.state = state_name(self.abstract_state.driver).to_owned();
        self.mode = self.abstract_state.mode;
        self.attempts = self.abstract_state.attempts;
        self.updated_at = now;
    }

    fn validate(&self, registry: &Path) -> Result<()> {
        if self.schema != DRIVER_STATE_SCHEMA {
            return Err(fail(format!(
                "driver state schema must be {DRIVER_STATE_SCHEMA}"
            )));
        }
        let canonical = std::fs::canonicalize(registry).unwrap_or_else(|_| registry.to_path_buf());
        if self.registry != canonical.display().to_string() || self.team_key != team_key(registry) {
            return Err(fail("driver state belongs to a different registry"));
        }
        if self.state != state_name(self.abstract_state.driver)
            || self.mode != self.abstract_state.mode
        {
            return Err(fail(
                "driver state mirrors disagree with its model snapshot",
            ));
        }
        let recorded = RestoreEpoch::from_report(&serde_json::json!({
            "boot_id": self.boot_id,
            "epoch": self.epoch,
        }))
        .filter(|found| found.boot_id == self.boot_id)
        .ok_or_else(|| fail("driver state epoch is unreadable or for another boot"))?;
        if self.incident != incident_id(registry, &recorded) {
            return Err(fail("driver state incident does not match its epoch"));
        }
        Ok(())
    }
}

/// The state directory, state file, lock file, and alarm file for one team.
pub fn driver_paths(
    registry: &Path,
    state_dir: Option<&Path>,
) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let directory = team_driver_dir(registry, state_dir)
        .ok_or_else(|| fail("cannot determine the agentctl state directory"))?;
    Ok((
        directory.join("state.json"),
        directory.join("driver.lock"),
        directory.join("alarm.json"),
    ))
}

/// Create the private per-team state directory when absent.
pub fn prepare_driver_dir(registry: &Path, state_dir: Option<&Path>) -> Result<PathBuf> {
    let directory = team_driver_dir(registry, state_dir)
        .ok_or_else(|| fail("cannot determine the agentctl state directory"))?;
    agent_queue::fsio::create_private_directory(
        &directory,
        "team driver state directory",
        true,
        true,
    )
    .map_err(|error| fail(error.to_string()))?;
    Ok(directory)
}

/// Load the driver record when one exists; an unreadable or foreign
/// record is refused, never silently replaced.
pub fn load_record(directory: &Path, registry: &Path) -> Result<Option<DriverRecord>> {
    let path = directory.join("state.json");
    let mut text = String::new();
    let read = File::open(&path).and_then(|file| file.take(1 << 20).read_to_string(&mut text));
    match read {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(fail(format!("cannot read {}: {error}", path.display())));
        }
    }
    let record: DriverRecord = serde_json::from_str(&text)
        .map_err(|error| fail(format!("invalid {}: {error}", path.display())))?;
    record.validate(registry)?;
    Ok(Some(record))
}

/// Persist `record` atomically to its state directory.
pub fn save_record(directory: &Path, record: &mut DriverRecord, now: f64) -> Result<()> {
    record.sync_mirrors(now);
    let value = serde_json::to_value(&record).map_err(|error| fail(error.to_string()))?;
    crate::agent::atomic_json(&directory.join("state.json"), &value)
        .map_err(|error| fail(error.to_string()))
}

/// Whether `request_id` can name one request file: a bounded slug, so
/// it is always one filename component and one poll key.
pub fn valid_request_id(request_id: &str) -> bool {
    !request_id.is_empty()
        && request_id.len() <= 128
        && !matches!(request_id, "." | "..")
        && request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// Whether `incident` has the one shape [`incident_id`] mints:
/// twelve lowercase hex characters naming one request-file prefix.
pub fn valid_incident_id(incident: &str) -> bool {
    incident.len() == 12
        && incident
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Bounds on the free-form fields a typed action carries, checked at
/// enqueue and again at fold time so a hand-written request file cannot
/// smuggle an unbounded payload past the CLI.
fn validate_action_shape(action: &TeamAction) -> Result<()> {
    let bounded = |label: &str, value: &str, max: usize| {
        if value.is_empty() || value.chars().count() > max {
            return Err(fail(format!("{label} must be 1..={max} chars")));
        }
        Ok(())
    };
    match action {
        TeamAction::Facts | TeamAction::DeclareDone => Ok(()),
        TeamAction::Revive { agent } | TeamAction::Drain { agent } => {
            crate::subagents::name(agent).map(|_| ())
        }
        TeamAction::StartUnit { unit } | TeamAction::ResetUnit { unit } => {
            bounded("unit", unit, 256)
        }
        TeamAction::KillPid {
            pid,
            expect_cmdline,
            expect_start,
            boot_id,
            signal,
        } => {
            if *pid == 0 {
                return Err(fail("kill-pid pid must be non-zero"));
            }
            bounded("expect_cmdline", expect_cmdline, 256)?;
            bounded("expect_start", expect_start, 128)?;
            if expect_start.parse::<u64>().is_err() {
                return Err(fail("expect_start must be numeric clock ticks"));
            }
            bounded("boot_id", boot_id, 128)?;
            if let Some(signal) = signal {
                bounded("signal", signal, 32)?;
                if !matches!(
                    signal.as_str(),
                    "TERM" | "KILL" | "INT" | "HUP" | "QUIT" | "USR1" | "USR2" | "CONT"
                ) {
                    return Err(fail(
                        "signal must be one of TERM, KILL, INT, HUP, QUIT, USR1, USR2, CONT",
                    ));
                }
            }
            Ok(())
        }
        TeamAction::Escalate { text } => bounded("escalate text", text, 1000),
    }
}

fn action_sha256(
    incident: &str,
    epoch: &Value,
    request_id: &str,
    action: &Value,
    dry_run: bool,
    requester: &RequesterRecord,
) -> String {
    let document = serde_json::json!({
        "incident": incident,
        "epoch": epoch,
        "request_id": request_id,
        "action": action,
        "dry_run": dry_run,
        "requester": requester,
    });
    format!("{:x}", Sha256::digest(document.to_string().as_bytes()))
}

/// Build one validated request file document for [`enqueue_action`].
pub fn new_action_request(
    incident: &str,
    epoch: Value,
    request_id: &str,
    action: &TeamAction,
    dry_run: bool,
    requester: RequesterRecord,
    requested_at: f64,
) -> Result<ActionRequestFile> {
    if !valid_incident_id(incident) {
        return Err(fail("incident must be twelve lowercase hex characters"));
    }
    if !valid_request_id(request_id) {
        return Err(fail(
            "request id must be a bounded slug (letters, digits, '-', '_', '.')",
        ));
    }
    validate_action_shape(action)?;
    let action = serde_json::to_value(action).map_err(|error| fail(error.to_string()))?;
    let action_sha256 = action_sha256(incident, &epoch, request_id, &action, dry_run, &requester);
    Ok(ActionRequestFile {
        schema: ACT_REQUEST_SCHEMA.to_owned(),
        incident: incident.to_owned(),
        epoch,
        request_id: request_id.to_owned(),
        action,
        action_sha256,
        dry_run,
        requester,
        requested_at,
        receipt: None,
    })
}

fn actions_dir(directory: &Path) -> PathBuf {
    directory.join("actions")
}

fn action_request_path(directory: &Path, incident: &str, request_id: &str) -> PathBuf {
    actions_dir(directory).join(format!("{incident}-{request_id}.json"))
}

fn action_running_marker_path(directory: &Path, incident: &str, request_id: &str) -> PathBuf {
    actions_dir(directory).join(format!("{incident}-{request_id}.running"))
}

fn read_action_request(path: &Path) -> Option<ActionRequestFile> {
    let mut text = String::new();
    File::open(path)
        .and_then(|file| file.take(1 << 20).read_to_string(&mut text))
        .ok()?;
    let file: ActionRequestFile = serde_json::from_str(&text).ok()?;
    if file.schema != ACT_REQUEST_SCHEMA
        || !valid_request_id(&file.request_id)
        || file.action_sha256
            != action_sha256(
                &file.incident,
                &file.epoch,
                &file.request_id,
                &file.action,
                file.dry_run,
                &file.requester,
            )
    {
        return None;
    }
    serde_json::from_value::<TeamAction>(file.action.clone())
        .ok()
        .filter(|action| validate_action_shape(action).is_ok())?;
    Some(file)
}

/// Create one request file, or find the identical one already there.
/// The same request id naming a different action is a conflict, never
/// an overwrite.
pub fn enqueue_action(directory: &Path, request: &ActionRequestFile) -> Result<PathBuf> {
    let typed: TeamAction = serde_json::from_value(request.action.clone())
        .map_err(|error| fail(format!("invalid action: {error}")))?;
    validate_action_shape(&typed)?;
    if request.schema != ACT_REQUEST_SCHEMA
        || !valid_incident_id(&request.incident)
        || !valid_request_id(&request.request_id)
    {
        return Err(fail("invalid team act request"));
    }
    if request.action_sha256
        != action_sha256(
            &request.incident,
            &request.epoch,
            &request.request_id,
            &request.action,
            request.dry_run,
            &request.requester,
        )
    {
        return Err(fail("team act request hash does not match its fields"));
    }
    let dir = actions_dir(directory);
    agent_queue::fsio::create_private_directory(&dir, "team act requests", true, true)
        .map_err(|error| fail(error.to_string()))?;
    let path = action_request_path(directory, &request.incident, &request.request_id);
    let value = serde_json::to_value(request).map_err(|error| fail(error.to_string()))?;
    match agent_queue::fsio::atomic_json_create(&path, &value) {
        Ok(()) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = read_action_request(&path)
                .ok_or_else(|| fail("existing team act request is unreadable"))?;
            if existing.action_sha256 == request.action_sha256
                && existing.incident == request.incident
            {
                Ok(path)
            } else {
                Err(fail(format!(
                    "request id {} already names a different action for incident {}",
                    request.request_id, request.incident
                )))
            }
        }
        Err(error) => Err(fail(error.to_string())),
    }
}

fn is_terminal(status: &str) -> bool {
    matches!(status, "done" | "failed" | "refused" | "unknown")
}

/// The process exit a terminal receipt reports to its `team act` caller:
/// the kick mirror (0 healthy/delivered, 75 still pending, 1 refused).
fn terminal_exit_code(action: &ActionRecord) -> i32 {
    match action.status.as_str() {
        "done" => {
            let name = action.action["action"].as_str().unwrap_or_default();
            if action.dry_run || matches!(name, "facts" | "escalate") {
                0
            } else if name == "declare-done" {
                if action.explicit_yes == Some(true) {
                    0
                } else {
                    1
                }
            } else if action.verify_ok == Some(true) {
                0
            } else {
                75
            }
        }
        _ => 1,
    }
}

fn receipt_of(action: &ActionRecord) -> Option<ActionReceipt> {
    is_terminal(&action.status).then(|| ActionReceipt {
        status: action.status.clone(),
        detail: action.detail.clone(),
        verify_ok: action.verify_ok,
        explicit_yes: action.explicit_yes,
        started_at: action.started_at,
        finished_at: action.finished_at.unwrap_or(action.requested_at),
        exit_code: action
            .exit_code
            .unwrap_or_else(|| terminal_exit_code(action)),
        matched_cmdline: action.matched_cmdline.clone(),
        matched_start: action.matched_start,
        matched_cwd: action.matched_cwd.clone(),
        kill_log: action.kill_log.clone(),
    })
}

fn write_action_receipt(directory: &Path, action: &ActionRecord) -> Result<()> {
    let Some(receipt) = receipt_of(action) else {
        return Ok(());
    };
    let path = action_request_path(directory, &action.incident, &action.request_id);
    let Some(mut file) = read_action_request(&path) else {
        return Ok(());
    };
    file.receipt = Some(receipt);
    let value = serde_json::to_value(&file).map_err(|error| fail(error.to_string()))?;
    crate::agent::atomic_json(&path, &value).map_err(|error| fail(error.to_string()))?;
    let _ = std::fs::remove_file(action_running_marker_path(
        directory,
        &action.incident,
        &action.request_id,
    ));
    Ok(())
}

/// Fold this incident's request files into the record: new files become
/// queued actions; a terminal receipt a state rewrite lost is restored
/// from its file, so no effect ever runs twice for one request id.
fn reconcile_action_requests(directory: &Path, record: &mut DriverRecord) {
    let Ok(entries) = std::fs::read_dir(actions_dir(directory)) else {
        return;
    };
    let mut files: Vec<ActionRequestFile> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| read_action_request(&entry.path()))
        .filter(|file| file.incident == record.incident)
        .collect();
    files.sort_by(|a, b| {
        a.requested_at
            .total_cmp(&b.requested_at)
            .then_with(|| a.request_id.cmp(&b.request_id))
    });
    for mut file in files {
        // A file's proof is its writer's claim, however it was written:
        // the driver never treats a requester from disk as proved.
        file.requester.proof = "claimed".to_owned();
        if let Some(seen) = record
            .actions
            .iter_mut()
            .find(|seen| seen.request_id == file.request_id)
        {
            if file.receipt.is_some() {
                let _ = std::fs::remove_file(action_running_marker_path(
                    directory,
                    &file.incident,
                    &file.request_id,
                ));
            } else if seen.status == "queued"
                && action_running_marker_path(directory, &file.incident, &file.request_id).exists()
            {
                seen.status = "running".to_owned();
            }
            if !is_terminal(&seen.status) {
                if let Some(receipt) = file.receipt {
                    seen.status = receipt.status;
                    seen.detail = receipt.detail;
                    seen.verify_ok = receipt.verify_ok;
                    seen.explicit_yes = receipt.explicit_yes;
                    seen.started_at = receipt.started_at;
                    seen.finished_at = Some(receipt.finished_at);
                    seen.exit_code = Some(receipt.exit_code);
                    seen.matched_cmdline = receipt.matched_cmdline;
                    seen.matched_start = receipt.matched_start;
                    seen.matched_cwd = receipt.matched_cwd;
                    seen.kill_log = receipt.kill_log;
                }
            }
            continue;
        }
        let mut entry = ActionRecord {
            request_id: file.request_id.clone(),
            incident: file.incident.clone(),
            epoch: file.epoch.clone(),
            action: file.action.clone(),
            action_sha256: file.action_sha256.clone(),
            dry_run: file.dry_run,
            requester: file.requester.clone(),
            status: "queued".to_owned(),
            requested_at: file.requested_at,
            started_at: None,
            finished_at: None,
            detail: String::new(),
            verify_ok: None,
            explicit_yes: None,
            exit_code: None,
            matched_cmdline: None,
            matched_start: None,
            matched_cwd: None,
            kill_log: None,
        };
        if let Some(receipt) = file.receipt {
            let _ = std::fs::remove_file(action_running_marker_path(
                directory,
                &file.incident,
                &file.request_id,
            ));
            entry.status = receipt.status;
            entry.detail = receipt.detail;
            entry.verify_ok = receipt.verify_ok;
            entry.explicit_yes = receipt.explicit_yes;
            entry.started_at = receipt.started_at;
            entry.finished_at = Some(receipt.finished_at);
            entry.exit_code = Some(receipt.exit_code);
            entry.matched_cmdline = receipt.matched_cmdline;
            entry.matched_start = receipt.matched_start;
            entry.matched_cwd = receipt.matched_cwd;
            entry.kill_log = receipt.kill_log;
        } else if action_running_marker_path(directory, &file.incident, &file.request_id).exists() {
            entry.status = "running".to_owned();
        }
        record.actions.push(entry);
    }
}

/// Close out actions a dead driver left `running`: re-runnable verbs
/// requeue (their effects are idempotent or already-locked downstream),
/// while drain, kill-pid, and escalate become `unknown` — nobody can
/// prove whether they fired, and they must never fire twice on a guess.
/// A leftover `.running` marker beside the request file is the proof an
/// effect may have started even when state.json was rewritten since.
fn recover_running_actions(directory: &Path, record: &mut DriverRecord, now: f64) -> Result<()> {
    for index in 0..record.actions.len() {
        if record.actions[index].status != "running"
            || record.actions[index].incident != record.incident
        {
            continue;
        }
        let typed: std::result::Result<TeamAction, _> =
            serde_json::from_value(record.actions[index].action.clone());
        match typed {
            Ok(
                TeamAction::Drain { .. } | TeamAction::KillPid { .. } | TeamAction::Escalate { .. },
            ) => {
                finish_action(
                    directory,
                    record,
                    index,
                    "unknown",
                    "driver died while running",
                    None,
                    None,
                    now,
                )?;
            }
            _ => {
                let _ = std::fs::remove_file(action_running_marker_path(
                    directory,
                    &record.actions[index].incident,
                    &record.actions[index].request_id,
                ));
                record.actions[index].status = "queued".to_owned();
                record.actions[index].started_at = None;
            }
        }
    }
    Ok(())
}

/// The one-driver lock, held only while a run can still return it.
/// Its holder unlocks explicitly on drop: releasing by closing the
/// descriptor alone would keep the lock held while any child forked
/// under it sits between fork and exec, and the next kick's try
/// would spuriously find it busy.
pub struct DriverLock {
    _file: agent_queue::fsio::HeldLock,
}

/// Take the one-driver lock without waiting: `None` while another run
/// holds it, which is how a second kick only reports state.
pub fn try_acquire_lock(directory: &Path) -> Result<Option<DriverLock>> {
    let file =
        agent_queue::fsio::open_private_lock(&directory.join("driver.lock"), "team driver lock")
            .map_err(|error| fail(error.to_string()))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(DriverLock {
            _file: agent_queue::fsio::HeldLock::new(file),
        })),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(fail(format!("cannot lock the team driver state: {error}"))),
    }
}

/// Wait used between concrete re-checks while resident; exposed so the
/// CLI help and fakes name the same bound.
pub const RESIDENT_POLL: Duration = Duration::from_secs(RESIDENT_POLL_SECONDS);

/// Whether a run did the work or found it already owned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunAction {
    /// This invocation held the lock and ran the machine.
    Ran,
    /// Another invocation holds the lock; the record is its state.
    AlreadyRunning,
}

/// Process exit for a finished or reported driver state: 0 only for a
/// verified-healthy team or a delivered escalation, 75 while recovery
/// is still pending (the boot-unit retry-later class).
pub fn exit_code(record: &DriverRecord) -> i32 {
    if record.abstract_state.driver == State::Healthy || record.abstract_state.delivered {
        0
    } else {
        75
    }
}

/// Apply one model event to a record: the shared [`crate::team_model::step`]
/// decides, this stamps the concrete deadline and handoff receipt the
/// transition implies. Saving is the caller's job, once per effect.
fn apply_event(
    table: &[crate::team_model::Transition],
    record: &mut DriverRecord,
    event: crate::team_model::Event,
    now: f64,
    options: &DriverOptions,
) -> Result<()> {
    use crate::team_model::Event;
    let from = record.abstract_state.driver;
    let (next, effects) = crate::team_model::step(table, &record.abstract_state, event)
        .ok_or_else(|| fail(format!("no transition from {from:?} on {event:?}")))?;
    record.abstract_state = next;
    record.pending_effects = effects;
    if next.driver != from {
        record.entered_at = now;
    }
    match event {
        Event::VerifyFailLead => {
            let deadline = now + options.handoff_timeout.as_secs_f64();
            record.deadline = Some(deadline);
            record.handoff = Some(HandoffRecord {
                lead: options.lead.clone().unwrap_or_default(),
                message_id: handoff_message_id(record),
                checks: failed_checks(&record.last_verify),
                sent_at: None,
                deadline: Some(deadline),
            });
        }
        Event::Kick | Event::NewEpoch | Event::DetectMissing | Event::DetectClean => {
            record.deadline = None;
        }
        _ => {}
    }
    if next.driver == State::Healthy {
        record.deadline = None;
        record.wake = WakeRecord {
            outcome: "none".to_owned(),
            ..WakeRecord::default()
        };
    }
    record.sync_mirrors(now);
    Ok(())
}

/// Validate one typed action against the fresh manifest and state.
/// Production effects re-check their own domain rules (configured
/// units, pins, bridge targets) before acting.
/// A queued request's expiry: bound to its incident and epoch at
/// enqueue, and refused as expired once it is older than this many
/// seconds, so a later driver never executes a stale revive or
/// start-unit from a dead incident (review 6).
pub const ACT_EXPIRY_SECONDS: f64 = 1800.0;

fn validate_action(
    record: &DriverRecord,
    manifest: &Value,
    action: &TeamAction,
) -> std::result::Result<(), String> {
    validate_action_shape(action).map_err(|error| error.to_string())?;
    let recovery_effect = matches!(
        action,
        TeamAction::Revive { .. }
            | TeamAction::StartUnit { .. }
            | TeamAction::ResetUnit { .. }
            | TeamAction::Drain { .. }
            | TeamAction::KillPid { .. }
    );
    if recovery_effect
        && !matches!(
            record.abstract_state.driver,
            State::Restore | State::Verify | State::Handoff | State::Escalate
        )
    {
        return Err("no recovery action outside an incident state".to_owned());
    }
    let desired: Vec<&str> = manifest["desired"]
        .as_array()
        .map(|rows| rows.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let agent_row = |agent: &str| {
        manifest["agents"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["name"] == agent))
    };
    match action {
        TeamAction::Revive { agent } => {
            if !desired.contains(&agent.as_str()) {
                return Err("agent is not in the desired set".to_owned());
            }
            match agent_row(agent) {
                Some(row) if row["native_session_present"] == serde_json::json!(true) => Ok(()),
                _ => Err("no recorded native session; candidates are report-only".to_owned()),
            }
        }
        TeamAction::Drain { agent } => {
            if !desired.contains(&agent.as_str()) {
                return Err("agent is not in the desired set".to_owned());
            }
            let at_prompt = manifest["verify"]["checks"]
                .as_array()
                .is_some_and(|checks| {
                    checks.iter().any(|row| {
                        row["name"] == "pane-at-prompt"
                            && row["subject"] == agent.as_str()
                            && row["status"] == "pass"
                            && row["evidence"]
                                .as_str()
                                .is_some_and(|text| text.contains("input prompt"))
                    })
                });
            // A pass also covers a working pane with a follow-up editor;
            // the act layer refuses working delivery on every harness.
            let idle = agent_row(agent).is_some_and(|row| {
                matches!(row["agent_status"].as_str(), Some("idle" | "done"))
                    && matches!(
                        row["lifecycle"].as_str(),
                        Some("running" | "starting" | "adopted")
                    )
            });
            if at_prompt && idle {
                Ok(())
            } else if at_prompt {
                Err("pane is working; drain requires an idle composer".to_owned())
            } else {
                Err("pane is not explicitly at a prompt".to_owned())
            }
        }
        TeamAction::StartUnit { unit } | TeamAction::ResetUnit { unit } => {
            let configured = manifest["units"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|row| row["name"] == unit.as_str()));
            if configured {
                Ok(())
            } else {
                Err("unit is not configured for this team".to_owned())
            }
        }
        TeamAction::KillPid { boot_id, .. } => {
            if boot_id == &record.boot_id {
                Ok(())
            } else {
                Err("kill-pid boot id does not match this boot".to_owned())
            }
        }
        TeamAction::Facts | TeamAction::DeclareDone | TeamAction::Escalate { .. } => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_action(
    directory: &Path,
    record: &mut DriverRecord,
    index: usize,
    status: &str,
    detail: &str,
    verify_ok: Option<bool>,
    explicit_yes: Option<bool>,
    now: f64,
) -> Result<()> {
    record.actions[index].status = status.to_owned();
    record.actions[index].detail = detail.to_owned();
    record.actions[index].verify_ok = verify_ok;
    record.actions[index].explicit_yes = explicit_yes;
    record.actions[index].finished_at = Some(now);
    record.actions[index].exit_code = Some(terminal_exit_code(&record.actions[index]));
    let action = record.actions[index].clone();
    write_action_receipt(directory, &action)
}

/// Execute the oldest queued action for this incident. The caller has
/// already applied the model transition persisting RunQueuedAction.
fn run_queued_action<E: DriverEffects>(
    directory: &Path,
    record: &mut DriverRecord,
    effects: &mut E,
    now: f64,
) -> Result<()> {
    let Some(index) = record
        .actions
        .iter()
        .position(|action| action.status == "queued" && action.incident == record.incident)
    else {
        return Ok(());
    };
    let entry = record.actions[index].clone();
    let typed: TeamAction = match serde_json::from_value(entry.action.clone()) {
        Ok(action) if validate_action_shape(&action).is_ok() => action,
        _ => {
            return finish_action(
                directory,
                record,
                index,
                "refused",
                "invalid action",
                None,
                None,
                now,
            );
        }
    };
    if entry.epoch != record.epoch {
        return finish_action(
            directory,
            record,
            index,
            "refused",
            "stale epoch",
            None,
            None,
            now,
        );
    }
    if now - entry.requested_at > ACT_EXPIRY_SECONDS {
        return finish_action(
            directory,
            record,
            index,
            "refused",
            "expired request",
            None,
            None,
            now,
        );
    }
    record.actions[index].status = "running".to_owned();
    record.actions[index].started_at = Some(now);
    // The create-only marker is the crash proof an effect may have
    // started, independent of any later state.json rewrite.
    let marker = action_running_marker_path(directory, &entry.incident, &entry.request_id);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        Ok(mut file) => {
            use std::io::Write;
            let _ = writeln!(file, "{}", entry.request_id);
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return finish_action(
                directory,
                record,
                index,
                "unknown",
                "running marker already present",
                None,
                None,
                now,
            );
        }
        Err(error) => return Err(fail(error.to_string())),
    }
    save_record(directory, record, now)?;
    let pre = match effects.manifest(record) {
        Ok(outcome) => outcome,
        Err(error) => {
            record.last_error = Some(error.to_string());
            return finish_action(
                directory,
                record,
                index,
                "failed",
                "manifest unavailable",
                None,
                None,
                now,
            );
        }
    };
    let manifest = pre.manifest.clone();
    record.manifest = Some(manifest.clone());
    if let Err(detail) = validate_action(record, &manifest, &typed) {
        return finish_action(
            directory,
            record,
            index,
            "refused",
            &detail,
            Some(crate::team_verify::verify_ok(&pre.verify)),
            Some(explicit_yes_of(&manifest, &pre.verify)),
            now,
        );
    }
    if entry.dry_run {
        return finish_action(
            directory,
            record,
            index,
            "done",
            "dry run: validated, no effect run",
            Some(crate::team_verify::verify_ok(&pre.verify)),
            Some(explicit_yes_of(&manifest, &pre.verify)),
            now,
        );
    }
    // An escalate action is recorded as delivered before any route runs,
    // so a crash in the delivery window closes unknown (above) instead
    // of ever sending the same text twice.
    let escalate_was_delivered = record.escalation.delivered_at.is_some();
    let escalate_pre_delivered =
        matches!(typed, TeamAction::Escalate { .. }) && !escalate_was_delivered;
    if escalate_pre_delivered {
        record.escalation.delivered_at = Some(now);
        save_record(directory, record, now)?;
    }
    let outcome = match effects.act(record, &typed, &manifest, &entry.requester) {
        Ok(outcome) => outcome,
        Err(error) => {
            record.last_error = Some(error.to_string());
            return finish_action(
                directory,
                record,
                index,
                "failed",
                "effect failed",
                None,
                None,
                now,
            );
        }
    };
    if let Some(kill) = &outcome.kill {
        record.actions[index].matched_cmdline = Some(kill.cmdline.clone());
        record.actions[index].matched_start = Some(kill.starttime);
        record.actions[index].matched_cwd = kill.cwd.clone();
        record.actions[index].kill_log = Some(kill.log_path.clone());
    }
    if let Some(escalation) = &outcome.escalation {
        record.escalation.last_attempt_at = Some(now);
        match escalation {
            EscalationOutcome::Delivered => {
                if escalate_was_delivered {
                    record.escalation.reminded_at = Some(now);
                }
                record.escalation.next_attempt_at = Some(now + 3600.0);
            }
            EscalationOutcome::Failed(detail) => {
                record.last_error = Some(detail.clone());
                if escalate_pre_delivered {
                    record.escalation.delivered_at = None;
                }
                record.escalation.next_attempt_at = Some(
                    now + crate::team_escalate::backoff_seconds(record.abstract_state.attempts)
                        as f64,
                );
            }
            EscalationOutcome::Unconfigured => {
                if escalate_pre_delivered {
                    record.escalation.delivered_at = None;
                }
                record.escalation.next_attempt_at = Some(
                    now + crate::team_escalate::backoff_seconds(record.abstract_state.attempts)
                        as f64,
                );
            }
        }
    }
    if outcome.status != "done" {
        return finish_action(
            directory,
            record,
            index,
            &outcome.status,
            &outcome.detail,
            None,
            None,
            now,
        );
    }
    let after = match effects.manifest(record) {
        Ok(manifest) => manifest,
        Err(error) => {
            record.last_error = Some(error.to_string());
            return finish_action(
                directory,
                record,
                index,
                "unknown",
                "verify-after unavailable",
                None,
                None,
                now,
            );
        }
    };
    record.manifest = Some(after.manifest.clone());
    record.last_verify = after.verify.clone();
    record.held = report_held(&record.last_verify);
    let explicit = explicit_yes_of(&after.manifest, &after.verify);
    let (status, detail) = if matches!(typed, TeamAction::DeclareDone) && !explicit {
        ("failed", "explicit YES checks did not all pass".to_owned())
    } else {
        ("done", outcome.detail.clone())
    };
    finish_action(
        directory,
        record,
        index,
        status,
        &detail,
        Some(crate::team_verify::verify_ok(&after.verify)),
        Some(explicit),
        now,
    )
}

fn explicit_yes_of(manifest: &Value, verify: &Value) -> bool {
    manifest["coordinator"]
        .as_str()
        .filter(|coordinator| !coordinator.is_empty())
        .is_some_and(|coordinator| crate::team_verify::explicit_yes(verify, coordinator))
}

/// The wake request for the record's concrete deadline.
fn wake_request(
    record: &DriverRecord,
    options: &DriverOptions,
) -> Result<crate::restore::WakeRequest> {
    let deadline = record
        .deadline
        .ok_or_else(|| fail("cannot arm a wake without a deadline"))?;
    Ok(crate::restore::WakeRequest {
        team_key: record.team_key.clone(),
        registry: PathBuf::from(&record.registry),
        deadline_unix: deadline,
        argv: options.wake_argv.clone(),
    })
}

enum Resident {
    Wake,
    ActQueued,
    Terminated,
}

/// Wait out a concrete deadline in-process when no timer could be
/// armed, re-verifying on the poll so an early fix wakes the machine
/// at once. The polls are observations only: no model event is fed
/// until the wake, so the abstract trace stays exactly the table's.
fn resident_wait<E: DriverEffects>(
    directory: &Path,
    record: &mut DriverRecord,
    host: &dyn Host,
    effects: &mut E,
    options: &DriverOptions,
) -> Result<Resident> {
    loop {
        if host.terminated() {
            return Ok(Resident::Terminated);
        }
        let now = host.now();
        let Some(deadline) = record.deadline else {
            return Ok(Resident::Wake);
        };
        if now >= deadline {
            return Ok(Resident::Wake);
        }
        reconcile_action_requests(directory, record);
        if record
            .actions
            .iter()
            .any(|action| action.status == "queued" && action.incident == record.incident)
        {
            // Not WakeFired: the deadline has not expired, and feeding
            // WakeFired would mark it expired and page a human early.
            return Ok(Resident::ActQueued);
        }
        host.sleep(Duration::from_secs_f64(
            (deadline - now).min(RESIDENT_POLL.as_secs_f64()),
        ));
        if host.terminated() {
            return Ok(Resident::Terminated);
        }
        if record.abstract_state.mode == Mode::Running {
            continue;
        }
        let outcome = effects.verify(record)?;
        record.held = report_held(&outcome.report);
        record.last_verify = outcome.report;
        save_record(directory, record, host.now())?;
        if crate::team_verify::verify_ok(&record.last_verify) {
            return Ok(Resident::Wake);
        }
        // In VERIFY an ack that is no longer merely pending (expired or
        // joined by other failures) also ends the wait early.
        if record.abstract_state.driver == State::Verify
            && !(failed_checks(&record.last_verify) == ["coordinator-ack"]
                && outcome.ack_deadline.is_some_and(|due| due > host.now()))
        {
            return Ok(Resident::Wake);
        }
        let _ = options;
    }
}

/// Run the machine for one registry until it may exit: healthy, a
/// delivered escalation, or a dormant deadline with its wake armed (or
/// waited out resident and then woken). The lock is held throughout;
/// a concurrent invocation gets the current record back untouched.
pub fn run_with_effects<E: DriverEffects>(
    registry: &Path,
    host: &dyn Host,
    effects: &mut E,
    options: &DriverOptions,
) -> Result<(DriverRecord, RunAction)> {
    use crate::team_model::{Effect, Event, Mode, State};
    require_host_namespace(host)?;
    let directory = prepare_driver_dir(registry, options.state_dir.as_deref())?;
    let epoch = host.epoch()?;
    let Some(lock) = try_acquire_lock(&directory)? else {
        let record = load_record(&directory, registry)?
            .unwrap_or_else(|| DriverRecord::new(registry, &epoch, host.now()));
        return Ok((record, RunAction::AlreadyRunning));
    };
    let _lock = lock;
    let table = crate::team_model::transitions();
    let mut record = match load_record(&directory, registry)? {
        Some(record) => record,
        None => {
            let mut fresh = DriverRecord::new(registry, &epoch, host.now());
            save_record(&directory, &mut fresh, host.now())?;
            fresh
        }
    };
    let recorded = RestoreEpoch::from_report(&serde_json::json!({
        "boot_id": record.boot_id,
        "epoch": record.epoch,
    }));
    if recorded.as_ref() != Some(&epoch) {
        // Close out the old epoch's queued actions under their receipts,
        // then prune them: request files stay as the durable audit, and
        // an action bound to a dead epoch never runs in the new one.
        let new_incident = incident_id(registry, &epoch);
        for index in 0..record.actions.len() {
            if record.actions[index].incident == new_incident {
                continue;
            }
            if record.actions[index].status == "queued" {
                finish_action(
                    &directory,
                    &mut record,
                    index,
                    "refused",
                    "stale epoch",
                    None,
                    None,
                    host.now(),
                )?;
            } else if record.actions[index].status == "running" {
                finish_action(
                    &directory,
                    &mut record,
                    index,
                    "unknown",
                    "epoch changed while running",
                    None,
                    None,
                    host.now(),
                )?;
            }
        }
        record
            .actions
            .retain(|action| action.incident == new_incident);
        // A new boot or user-manager restart: the model resets the
        // incident facts; the concrete receipts reset with them. The
        // incident id itself is boot-scoped and recomputed either way.
        record.epoch = epoch.as_json();
        record.boot_id = epoch.boot_id.clone();
        record.incident = incident_id(registry, &epoch);
        record.handoff = None;
        record.escalation = EscalationRecord::default();
        record.last_verify = Value::Null;
        record.held = false;
        record.deadline = None;
        record.wake = WakeRecord {
            outcome: "none".to_owned(),
            ..WakeRecord::default()
        };
        record.last_error = None;
        record.manifest = None;
        apply_event(&table, &mut record, Event::NewEpoch, host.now(), options)?;
        save_record(&directory, &mut record, host.now())?;
    }
    reconcile_action_requests(&directory, &mut record);
    recover_running_actions(&directory, &mut record, host.now())?;
    save_record(&directory, &mut record, host.now())?;
    let mut first_healthy_kick = true;
    for _ in 0..10_000 {
        let now = host.now();
        if record.deadline.is_some_and(|deadline| now >= deadline)
            && matches!(
                record.abstract_state.driver,
                State::Verify | State::Handoff | State::Escalate
            )
        {
            record.abstract_state.deadline_expired = true;
        }
        let actions_before = record.actions.clone();
        reconcile_action_requests(&directory, &mut record);
        if record.actions != actions_before {
            save_record(&directory, &mut record, now)?;
        }
        if let Some(effect) = record.pending_effects.first().copied() {
            match effect {
                Effect::LaunchRestore => {
                    let report = effects.restore(&record)?;
                    if let Some(error) = report["error"].as_str() {
                        record.last_error = Some(error.to_owned());
                    }
                    record.pending_effects.remove(0);
                    apply_event(&table, &mut record, Event::RestoreDone, now, options)?;
                }
                Effect::RunVerify => {
                    let outcome = effects.verify(&record)?;
                    record.held = report_held(&outcome.report);
                    record.last_verify = outcome.report;
                    record.abstract_state.env.lead_up = outcome.lead_up;
                    let state = record.abstract_state.driver;
                    let event = if crate::team_verify::verify_ok(&record.last_verify) {
                        record.abstract_state.env.fixed = true;
                        if state == State::Handoff {
                            Event::Progress
                        } else {
                            Event::VerifyPass
                        }
                    } else {
                        record.abstract_state.env.fixed = false;
                        let ack_only =
                            failed_checks(&record.last_verify) == ["coordinator-ack".to_owned()];
                        if state == State::Verify
                            && ack_only
                            && outcome.ack_deadline.is_some_and(|due| due > now)
                        {
                            record.deadline = outcome.ack_deadline;
                            Event::VerifyAckPending
                        } else if state == State::Verify {
                            if outcome.lead_up {
                                Event::VerifyFailLead
                            } else {
                                Event::VerifyFailNoLead
                            }
                        } else if state == State::Handoff && !outcome.lead_up {
                            Event::LeadGone
                        } else {
                            Event::DeadlineReached
                        }
                    };
                    record.pending_effects.remove(0);
                    apply_event(&table, &mut record, event, now, options)?;
                }
                Effect::QueueLeadChecklist => {
                    let Some(handoff) = record.handoff.clone() else {
                        record.pending_effects.remove(0);
                        save_record(&directory, &mut record, now)?;
                        continue;
                    };
                    if handoff.sent_at.is_none() {
                        let text = handoff_text(&record);
                        match effects.queue_handoff(&record, &text, &handoff.message_id) {
                            Ok(()) => {
                                if let Some(sent) = record.handoff.as_mut() {
                                    sent.sent_at = Some(now);
                                }
                                record.pending_effects.remove(0);
                            }
                            Err(error) => {
                                // The Exit effect has not run: this
                                // process is still the actor, so the loss
                                // of the lead is a running transition.
                                record.last_error = Some(error.to_string());
                                record.pending_effects.clear();
                                record.abstract_state.mode = Mode::Running;
                                record.abstract_state.env.lead_up = false;
                                apply_event(&table, &mut record, Event::LeadGone, now, options)?;
                            }
                        }
                    } else {
                        record.pending_effects.remove(0);
                    }
                }
                Effect::DeliverEscalation => {
                    let text = escalation_text(&record, false);
                    match effects.escalate(&record, &text, false)? {
                        EscalationOutcome::Delivered => {
                            if record.abstract_state.delivered {
                                record.escalation.reminded_at = Some(now);
                            } else {
                                record.escalation.delivered_at = Some(now);
                            }
                            record.escalation.last_attempt_at = Some(now);
                            let next = now + 3600.0;
                            record.escalation.next_attempt_at = Some(next);
                            record.deadline = Some(next);
                            record.abstract_state.env.route_ok = true;
                            record.pending_effects.remove(0);
                            apply_event(
                                &table,
                                &mut record,
                                Event::EscalateDelivered,
                                now,
                                options,
                            )?;
                        }
                        failed @ (EscalationOutcome::Failed(_)
                        | EscalationOutcome::Unconfigured) => {
                            let detail = match failed {
                                EscalationOutcome::Failed(detail) => detail,
                                _ => "no escalation routes are configured".to_owned(),
                            };
                            record.last_error = Some(detail);
                            record.escalation.last_attempt_at = Some(now);
                            let next = now
                                + crate::team_escalate::backoff_seconds(
                                    record.abstract_state.attempts,
                                ) as f64;
                            record.escalation.next_attempt_at = Some(next);
                            record.deadline = Some(next);
                            record.abstract_state.env.route_ok = false;
                            record.pending_effects.remove(0);
                            apply_event(&table, &mut record, Event::EscalateFailed, now, options)?;
                        }
                    }
                }
                Effect::ArmWake => {
                    if options.wake_argv.is_empty() {
                        record.wake = WakeRecord {
                            deadline: record.deadline,
                            outcome: "resident".to_owned(),
                            unit: None,
                        };
                    } else {
                        match host.arm_wake(&wake_request(&record, options)?) {
                            crate::restore::WakeOutcome::Armed { unit } => {
                                record.wake = WakeRecord {
                                    deadline: record.deadline,
                                    outcome: "armed".to_owned(),
                                    unit: Some(unit),
                                };
                            }
                            crate::restore::WakeOutcome::Failed { detail } => {
                                record.last_error = Some(format!("wake arming failed: {detail}"));
                                record.wake = WakeRecord {
                                    deadline: record.deadline,
                                    outcome: "resident".to_owned(),
                                    unit: None,
                                };
                            }
                            crate::restore::WakeOutcome::NoManager
                            | crate::restore::WakeOutcome::Unsupported => {
                                record.wake = WakeRecord {
                                    deadline: record.deadline,
                                    outcome: "resident".to_owned(),
                                    unit: None,
                                };
                            }
                        }
                    }
                    record.pending_effects.remove(0);
                }
                Effect::WriteAlarm => {
                    let incident = crate::team_escalate::Incident {
                        host: options.hostname.clone(),
                        team: record.registry.clone(),
                        incident: record.incident.clone(),
                        text: escalation_text(&record, false),
                    };
                    crate::team_escalate::write_alarm(
                        &directory,
                        &incident,
                        record
                            .last_error
                            .as_deref()
                            .unwrap_or("escalation undelivered"),
                        now,
                    )?;
                    record.pending_effects.remove(0);
                }
                Effect::ClearAlarm => {
                    crate::team_escalate::clear_alarm(&directory)?;
                    record.pending_effects.remove(0);
                }
                Effect::SendResolved => {
                    if record.escalation.delivered_at.is_some()
                        && record.escalation.resolved_at.is_none()
                    {
                        let text = escalation_text(&record, true);
                        match effects.escalate(&record, &text, true)? {
                            EscalationOutcome::Delivered => {
                                record.escalation.resolved_at = Some(now);
                            }
                            EscalationOutcome::Failed(detail) => {
                                record.last_error = Some(detail);
                            }
                            EscalationOutcome::Unconfigured => {
                                record.last_error =
                                    Some("no escalation routes are configured".to_owned());
                            }
                        }
                    }
                    record.pending_effects.remove(0);
                }
                Effect::RunQueuedAction => {
                    run_queued_action(&directory, &mut record, effects, now)?;
                    record.pending_effects.remove(0);
                }
                Effect::Exit => {
                    // Never exit past queued work: drain every queued
                    // current-incident action in this pass, one model
                    // self-loop at a time, before any exit is honoured.
                    if record.actions.iter().any(|action| {
                        action.status == "queued" && action.incident == record.incident
                    }) {
                        record.pending_effects.remove(0);
                        apply_event(&table, &mut record, Event::ActRequested, now, options)?;
                    } else if record.abstract_state.driver == State::Healthy
                        || record.wake.outcome == "armed"
                    {
                        record.pending_effects.remove(0);
                        save_record(&directory, &mut record, now)?;
                        return Ok((record, RunAction::Ran));
                    } else {
                        match resident_wait(&directory, &mut record, host, effects, options)? {
                            Resident::Terminated => {
                                save_record(&directory, &mut record, host.now())?;
                                return Ok((record, RunAction::Ran));
                            }
                            Resident::ActQueued => {
                                record.pending_effects.remove(0);
                                apply_event(
                                    &table,
                                    &mut record,
                                    Event::ActRequested,
                                    host.now(),
                                    options,
                                )?;
                            }
                            Resident::Wake => {
                                record.pending_effects.remove(0);
                                apply_event(
                                    &table,
                                    &mut record,
                                    Event::WakeFired,
                                    host.now(),
                                    options,
                                )?;
                            }
                        }
                    }
                }
            }
            save_record(&directory, &mut record, host.now())?;
            continue;
        }
        // No pending effect: a queued action pre-empts the next
        // observation, exactly one per loop pass.
        if record
            .actions
            .iter()
            .any(|action| action.status == "queued" && action.incident == record.incident)
        {
            apply_event(&table, &mut record, Event::ActRequested, now, options)?;
            save_record(&directory, &mut record, host.now())?;
            continue;
        }
        // No pending effect: choose the next observation or transition.
        match (record.abstract_state.driver, record.abstract_state.mode) {
            (State::Healthy, Mode::Dormant) if first_healthy_kick => {
                first_healthy_kick = false;
                apply_event(&table, &mut record, Event::Kick, now, options)?;
            }
            (_, Mode::Dormant) => {
                if record.deadline.is_some_and(|due| now >= due)
                    && matches!(
                        record.abstract_state.driver,
                        State::Verify | State::Handoff | State::Escalate
                    )
                {
                    apply_event(&table, &mut record, Event::WakeFired, now, options)?;
                } else {
                    save_record(&directory, &mut record, now)?;
                    return Ok((record, RunAction::Ran));
                }
            }
            (State::Detect, Mode::Running) => {
                let outcome = effects.detect(&record)?;
                record.abstract_state.env.missing = outcome.missing;
                record.abstract_state.env.lead_up = outcome.lead_up;
                if outcome.missing && !outcome.detail.is_empty() {
                    record.last_error = Some(outcome.detail.clone());
                }
                apply_event(
                    &table,
                    &mut record,
                    if outcome.missing {
                        Event::DetectMissing
                    } else {
                        Event::DetectClean
                    },
                    now,
                    options,
                )?;
            }
            (State::Restore, Mode::Running) => {
                record.pending_effects = vec![Effect::LaunchRestore];
            }
            (State::Verify | State::Handoff | State::Escalate, Mode::Running) => {
                record.pending_effects = vec![Effect::RunVerify];
            }
            (State::Healthy, Mode::Running) => {
                record.abstract_state.mode = Mode::Dormant;
            }
        }
        save_record(&directory, &mut record, host.now())?;
    }
    record.last_error = Some("driver step bound exhausted".to_owned());
    save_record(&directory, &mut record, host.now())?;
    Err(fail("team driver step bound exhausted"))
}

/// Production effects over the real client, restore, verify, queue, and
/// escalation routes. Constructed by the CLI and by `run_driver`.
pub struct ProductionEffects<'a, A: crate::subagents::ManagedApi + ?Sized> {
    client: &'a A,
    registry: PathBuf,
    host: &'a dyn Host,
    config: crate::restore::RestoreConfig,
    startup_timeout: Duration,
    wrkslots: Option<PathBuf>,
    herdr_bin: PathBuf,
    verify_options: crate::team_verify::VerifyOptions,
    escalation: crate::team_escalate::EscalationConfig,
    hostname: String,
}

impl<'a, A: crate::subagents::ManagedApi + ?Sized> ProductionEffects<'a, A> {
    fn manager(&self) -> Result<crate::subagents::ManagedAgents<'a, A>> {
        crate::subagents::ManagedAgents::new(self.client, &self.registry)
            .map_err(|error| fail(error.to_string()))
    }

    fn coordinator_live(&self, statuses: &[(String, std::result::Result<Value, String>)]) -> bool {
        let Some(lead) = self.config.coordinator.as_deref() else {
            return false;
        };
        crate::team_verify::desired_live_rows(&[lead.to_owned()], statuses)
            .first()
            .is_some_and(|row| row["status"] == "pass")
    }
}

impl<A: crate::subagents::ManagedApi + ?Sized> DriverEffects for ProductionEffects<'_, A> {
    fn detect(&mut self, _record: &DriverRecord) -> Result<DetectOutcome> {
        let manager = self.manager()?;
        let desired = crate::team_verify::desired_set(&self.registry, &self.config)?;
        let statuses: Vec<(String, std::result::Result<Value, String>)> = desired
            .names
            .iter()
            .map(|name| {
                (
                    name.clone(),
                    manager.status(name).map_err(|error| error.to_string()),
                )
            })
            .collect();
        let rows = crate::team_verify::desired_live_rows(&desired.names, &statuses);
        let lead_up = self.coordinator_live(&statuses);
        let mut missing = !desired.undecidable.is_empty()
            || rows.iter().any(|row| row["status"] == "fail")
            || self.client.panes().is_err();
        let pins = crate::team_verify::read_pins(
            &self.registry,
            &self.config.units,
            self.verify_options.state_dir.as_deref(),
        )
        .unwrap_or_default();
        let mut details: Vec<String> = rows
            .iter()
            .filter(|row| row["status"] == "fail")
            .filter_map(|row| row["evidence"].as_str().map(str::to_owned))
            .collect();
        for unit in &self.config.units {
            if pins.reason_for(unit).is_some() {
                continue;
            }
            let outcome = self.host.run(
                &[
                    crate::restore::systemctl_program(),
                    "--user".to_owned(),
                    "is-active".to_owned(),
                    unit.clone(),
                ],
                None,
                Duration::from_secs(30),
            );
            if crate::team_verify::unit_row(unit, &outcome, None)["status"] == "fail" {
                missing = true;
                details.push(format!("{unit} not active"));
            }
        }
        Ok(DetectOutcome {
            missing,
            lead_up,
            detail: details.join("; "),
        })
    }

    fn restore(&mut self, _record: &DriverRecord) -> Result<Value> {
        // A restore that ran in any class is an observation for VERIFY,
        // never a driver abort: synthesize the incomplete report an
        // early refusal would have written.
        Ok(crate::restore::restore(
            self.client,
            &self.registry,
            self.host,
            &self.config,
            &crate::restore::RestoreOptions {
                force: true,
                dry_run: false,
                startup_timeout: self.startup_timeout,
                wrkslots: self.wrkslots.clone(),
                herdr_bin: self.herdr_bin.clone(),
            },
        )
        .unwrap_or_else(|error| serde_json::json!({"complete": false, "error": error.to_string()})))
    }

    fn verify(&mut self, record: &DriverRecord) -> Result<VerifyOutcome> {
        let report = crate::team_verify::verify_team(
            self.client,
            &self.registry,
            self.host,
            &self.config,
            &self.verify_options,
        )
        .unwrap_or_else(|error| {
            serde_json::json!({
                "schema": crate::team_verify::VERIFY_SCHEMA,
                "ok": false,
                "checks": [{
                    "name": "desired-live", "status": "fail", "subject": null,
                    "evidence": format!("verify itself failed: {error}"),
                    "fix": "read the verify error and run agentctl team verify",
                }],
            })
        });
        // Ack deadline and lead liveness are read directly, never parsed
        // back out of evidence text.
        let mut ack_deadline = None;
        let mut lead_up = false;
        if let Ok(manager) = self.manager() {
            if let Some(lead) = self.config.coordinator.as_deref() {
                let status = manager.status(lead).map_err(|error| error.to_string());
                lead_up = self.coordinator_live(&[(lead.to_owned(), status)]);
                let notice_id =
                    format!("restore-{}", &record.boot_id[..8.min(record.boot_id.len())]);
                let pending = matches!(
                    manager.message_state(lead, &notice_id),
                    Ok(Some(
                        crate::agent::QueueMessageState::Pending
                            | crate::agent::QueueMessageState::Inflight
                    ))
                );
                if pending {
                    let epoch = RestoreEpoch::from_report(&serde_json::json!({
                        "boot_id": record.boot_id,
                        "epoch": record.epoch,
                    }));
                    if let Some(epoch) = epoch {
                        let shared_dir = crate::restore::default_report_dir();
                        let finished = crate::restore::candidate_report_paths(
                            &self.registry,
                            &self.config,
                            &epoch.boot_id,
                        )
                        .into_iter()
                        .find_map(|path| {
                            let report = crate::restore::read_report(&path)?;
                            crate::restore::report_counts_for_registry(
                                &self.registry,
                                &self.config,
                                &path,
                                &report,
                                shared_dir.as_deref(),
                            )
                            .then_some(report)
                        })
                        .and_then(|found| found["finished_at"].as_f64());
                        ack_deadline = finished
                            .map(|when| when + self.verify_options.ack_timeout_seconds as f64);
                    }
                }
            }
        }
        Ok(VerifyOutcome {
            report,
            lead_up,
            ack_deadline,
        })
    }

    fn queue_handoff(&mut self, record: &DriverRecord, text: &str, message_id: &str) -> Result<()> {
        let lead = record
            .handoff
            .as_ref()
            .map(|handoff| handoff.lead.clone())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| fail("no lead is configured for the handoff"))?;
        let manager = self.manager()?;
        let stamped = crate::sender_header::stamp(
            text,
            &crate::sender_header::Sender::Other("agentctl-team-driver".to_owned()),
            "agentctl",
        );
        if manager.is_cloud(&lead).unwrap_or(false) || manager.is_headless(&lead).unwrap_or(false) {
            manager
                .send_identified(
                    &lead,
                    &stamped,
                    crate::agent::DrainOptions::default(),
                    Some(message_id),
                    None,
                )
                .map_err(|error| fail(error.to_string()))?;
        } else {
            manager
                .queue_identified(
                    &lead,
                    &stamped,
                    None,
                    Some("team-handoff"),
                    Some(message_id),
                )
                .map_err(|error| fail(error.to_string()))?;
        }
        Ok(())
    }

    fn escalate(
        &mut self,
        record: &DriverRecord,
        text: &str,
        _resolved: bool,
    ) -> Result<EscalationOutcome> {
        let incident = crate::team_escalate::Incident {
            host: self.hostname.clone(),
            team: record.registry.clone(),
            incident: record.incident.clone(),
            text: text.to_owned(),
        };
        Ok(
            match crate::team_escalate::deliver(self.host, &self.escalation, &incident) {
                crate::team_escalate::Delivery::Delivered { .. } => EscalationOutcome::Delivered,
                crate::team_escalate::Delivery::Failed { detail } => {
                    EscalationOutcome::Failed(detail)
                }
                crate::team_escalate::Delivery::Unconfigured => EscalationOutcome::Unconfigured,
            },
        )
    }

    fn manifest(&mut self, _record: &DriverRecord) -> Result<ManifestOutcome> {
        let options = crate::team_verify::VerifyOptions {
            dismiss_allowlisted: false,
            ..self.verify_options.clone()
        };
        let (manifest, verify) = crate::team_manifest::manifest_team_with_verify(
            self.client,
            &self.registry,
            self.host,
            &self.config,
            &options,
        )
        .map_err(|error| fail(error.to_string()))?;
        Ok(ManifestOutcome { manifest, verify })
    }

    fn act(
        &mut self,
        record: &DriverRecord,
        action: &TeamAction,
        manifest: &Value,
        requester: &RequesterRecord,
    ) -> Result<ActionOutcome> {
        let refused = |detail: &str| ActionOutcome {
            status: "refused".to_owned(),
            detail: detail.to_owned(),
            escalation: None,
            kill: None,
        };
        match action {
            TeamAction::Facts => Ok(done_outcome("facts recorded")),
            TeamAction::DeclareDone => Ok(done_outcome("verify requested")),
            TeamAction::Revive { agent } => {
                let manager = self.manager()?;
                // Recorded-only: revive can never discover or bind a
                // candidate from the manifest at act time.
                match manager.revive_recorded_only(agent, self.startup_timeout) {
                    Ok(_) => Ok(done_outcome("revive completed")),
                    Err(_) => Ok(ActionOutcome {
                        status: "unknown".to_owned(),
                        detail: "revive outcome unknown".to_owned(),
                        escalation: None,
                        kill: None,
                    }),
                }
            }
            TeamAction::StartUnit { unit } | TeamAction::ResetUnit { unit } => {
                let reset = matches!(action, TeamAction::ResetUnit { .. });
                if !self.config.units.iter().any(|found| found == unit) {
                    return Ok(refused("unit is not configured for this team"));
                }
                let pins = match crate::team_verify::read_pins(
                    &self.registry,
                    &self.config.units,
                    self.verify_options.state_dir.as_deref(),
                ) {
                    Ok(pins) => pins,
                    Err(_) => return Ok(refused("turnover pin unreadable")),
                };
                if pins.reason_for(unit).is_some() {
                    return Ok(refused("unit held by turnover pin"));
                }
                if unit.starts_with("agentctl-bridge-") {
                    let coordinator = manifest["coordinator"].as_str().unwrap_or_default();
                    let live = !coordinator.is_empty()
                        && manifest["verify"]["checks"]
                            .as_array()
                            .is_some_and(|checks| {
                                checks.iter().any(|row| {
                                    row["name"] == "desired-live"
                                        && row["subject"] == coordinator
                                        && row["status"] == "pass"
                                })
                            });
                    if !live {
                        return Ok(refused("bridge target not live"));
                    }
                }
                if reset {
                    let outcome = self.host.run(
                        &[
                            crate::restore::systemctl_program(),
                            "--user".to_owned(),
                            "reset-failed".to_owned(),
                            unit.clone(),
                        ],
                        None,
                        Duration::from_secs(30),
                    );
                    if outcome.status != Some(0) {
                        return Ok(ActionOutcome {
                            status: "failed".to_owned(),
                            detail: "unit reset-failed failed".to_owned(),
                            escalation: None,
                            kill: None,
                        });
                    }
                }
                let outcome = self.host.run(
                    &[
                        crate::restore::systemctl_program(),
                        "--user".to_owned(),
                        "start".to_owned(),
                        unit.clone(),
                    ],
                    None,
                    Duration::from_secs(120),
                );
                if outcome.status == Some(0) {
                    Ok(done_outcome(if reset {
                        "unit reset and started"
                    } else {
                        "unit started"
                    }))
                } else {
                    Ok(ActionOutcome {
                        status: "failed".to_owned(),
                        detail: "unit start failed".to_owned(),
                        escalation: None,
                        kill: None,
                    })
                }
            }
            TeamAction::Drain { agent } => {
                const BOUNDS: &str = "bounds ready_timeout=30s deadline=60s lock_timeout=10s max_attempts=1 max_items=1 idle_only=true deliver_while_working=false";
                let manager = self.manager()?;
                let options = crate::agent::DrainOptions {
                    ready_timeout: Duration::from_secs(30),
                    deadline: Some(Duration::from_secs(60)),
                    lock_timeout: Some(Duration::from_secs(10)),
                    max_attempts: 1,
                    max_items: Some(1),
                    idle_only: true,
                    deliver_while_working: false,
                    ..crate::agent::DrainOptions::default()
                };
                match manager.drain(agent, options) {
                    Ok(result) => Ok(match result.outcome {
                        crate::agent::QueueOutcome::Delivered => {
                            done_outcome(&format!("queue drained ({BOUNDS})"))
                        }
                        crate::agent::QueueOutcome::Pending => ActionOutcome {
                            status: "failed".to_owned(),
                            detail: format!("drain pending ({BOUNDS})"),
                            escalation: None,
                            kill: None,
                        },
                        crate::agent::QueueOutcome::PossiblySubmitted => ActionOutcome {
                            status: "unknown".to_owned(),
                            detail: format!("drain quarantined ({BOUNDS})"),
                            escalation: None,
                            kill: None,
                        },
                    }),
                    Err(_) => Ok(ActionOutcome {
                        status: "unknown".to_owned(),
                        detail: format!("drain outcome unknown ({BOUNDS})"),
                        escalation: None,
                        kill: None,
                    }),
                }
            }
            TeamAction::KillPid {
                pid,
                expect_cmdline,
                expect_start,
                signal,
                ..
            } => {
                let Ok(expect_start) = expect_start.parse::<u64>() else {
                    return Ok(refused("invalid expect_start"));
                };
                let signal = signal.as_deref().unwrap_or("TERM");
                if !crate::kill_pid::SIGNALS
                    .iter()
                    .any(|(name, _)| *name == signal)
                {
                    return Ok(refused("invalid signal"));
                }
                let actor = if requester.proof == "proved" {
                    requester.label.clone()
                } else {
                    format!("{} (claimed)", requester.label)
                };
                match crate::kill_pid::run_kill_pid_as(
                    u64::from(*pid),
                    expect_cmdline,
                    expect_start,
                    signal,
                    &self.registry,
                    Some(actor.as_str()),
                ) {
                    Ok(report) => Ok(ActionOutcome {
                        status: "done".to_owned(),
                        detail: "process signalled".to_owned(),
                        escalation: None,
                        kill: Some(KillEvidence {
                            cmdline: report.cmdline,
                            starttime: report.starttime,
                            cwd: report.cwd,
                            log_path: report.log_path.display().to_string(),
                        }),
                    }),
                    Err(refusal) if refusal.reason == "log-failed" => Ok(ActionOutcome {
                        status: "unknown".to_owned(),
                        detail: "kill outcome unknown (log-failed)".to_owned(),
                        escalation: None,
                        kill: None,
                    }),
                    Err(refusal) => Ok(ActionOutcome {
                        status: "refused".to_owned(),
                        detail: format!("kill refused ({})", refusal.reason),
                        escalation: None,
                        kill: None,
                    }),
                }
            }
            TeamAction::Escalate { text } => {
                // New text is always delivered; an earlier delivery this
                // incident never makes this text count as sent.
                let outcome = self.escalate(record, text, false)?;
                let (status, detail) = match &outcome {
                    EscalationOutcome::Delivered => ("done", "escalation delivered"),
                    EscalationOutcome::Failed(_) => ("failed", "escalation failed"),
                    EscalationOutcome::Unconfigured => ("failed", "escalation unconfigured"),
                };
                Ok(ActionOutcome {
                    status: status.to_owned(),
                    detail: detail.to_owned(),
                    escalation: Some(outcome),
                    kill: None,
                })
            }
        }
    }
}

/// Inputs the CLI gathers once (designation-resolved config, resolved
/// binaries) for one production driver or kick run.
pub struct ProductionInputs {
    /// Config after following the registry's coordinator designation.
    pub config: crate::restore::RestoreConfig,
    /// Startup deadline handed to restore.
    pub startup_timeout: Duration,
    /// Slot manager for restore, when the project configures one.
    pub wrkslots: Option<PathBuf>,
    /// Absolute Herdr binary for restore and wake argv.
    pub herdr_bin: PathBuf,
    /// Escalation routes already loaded (missing file is empty).
    pub escalation: crate::team_escalate::EscalationConfig,
}

fn production_effects<'a, A: crate::subagents::ManagedApi + ?Sized>(
    client: &'a A,
    registry: &Path,
    host: &'a dyn Host,
    inputs: &'a ProductionInputs,
    options: &DriverOptions,
) -> ProductionEffects<'a, A> {
    ProductionEffects {
        client,
        registry: registry.to_path_buf(),
        host,
        config: inputs.config.clone(),
        startup_timeout: inputs.startup_timeout,
        wrkslots: inputs.wrkslots.clone(),
        herdr_bin: inputs.herdr_bin.clone(),
        verify_options: crate::team_verify::VerifyOptions {
            state_dir: options.state_dir.clone(),
            ack_timeout_seconds: options.ack_timeout.as_secs(),
            dismiss_allowlisted: true,
        },
        escalation: inputs.escalation.clone(),
        hostname: options.hostname.clone(),
    }
}

/// Run the production driver for one registry to its next exit point.
pub fn run_driver<A: crate::subagents::ManagedApi + ?Sized>(
    client: &A,
    registry: &Path,
    host: &dyn Host,
    inputs: &ProductionInputs,
    options: &DriverOptions,
) -> Result<(DriverRecord, RunAction)> {
    let mut effects = production_effects(client, registry, host, inputs, options);
    run_with_effects(registry, host, &mut effects, options)
}

/// The kick report for one registry, as printed.
fn kick_report(action: &str, record: &DriverRecord, directory: &Path) -> Value {
    serde_json::json!({
        "schema": KICK_SCHEMA,
        "action": action,
        "registry": record.registry,
        "state": serde_json::to_value(record).unwrap_or(Value::Null),
        "alarm": crate::restore::read_report(&directory.join("alarm.json")).unwrap_or(Value::Null),
        "exit_code": exit_code(record),
    })
}

/// Kick one registry: through its driver unit when a user manager
/// answers and starts it, in the foreground otherwise. Returns the
/// report to print and the process exit.
pub fn kick<A: crate::subagents::ManagedApi + ?Sized>(
    client: &A,
    registry: &Path,
    host: &dyn Host,
    inputs: &ProductionInputs,
    options: &DriverOptions,
) -> Result<(Value, i32)> {
    require_host_namespace(host)?;
    let directory = prepare_driver_dir(registry, options.state_dir.as_deref())?;
    let probe = host.run(
        &[
            crate::restore::systemctl_program(),
            "--user".to_owned(),
            "show-environment".to_owned(),
        ],
        None,
        Duration::from_secs(15),
    );
    if probe.status == Some(0) {
        let unit = format!("agentctl-team@{}.service", team_key(registry));
        let started = host.run(
            &[
                crate::restore::systemctl_program(),
                "--user".to_owned(),
                "start".to_owned(),
                unit,
            ],
            None,
            Duration::from_secs(30),
        );
        if started.status == Some(0) {
            let epoch = host.epoch()?;
            let record = load_record(&directory, registry)?
                .unwrap_or_else(|| DriverRecord::new(registry, &epoch, host.now()));
            let current = RestoreEpoch::from_report(&serde_json::json!({
                "boot_id": record.boot_id,
                "epoch": record.epoch,
            }))
            .is_some_and(|recorded| recorded == epoch);
            let code = if current { exit_code(&record) } else { 75 };
            let mut report = kick_report("started-unit", &record, &directory);
            report["exit_code"] = serde_json::json!(code);
            return Ok((report, code));
        }
    }
    let (record, action) = run_driver(client, registry, host, inputs, options)?;
    let action = match action {
        RunAction::Ran => "ran",
        RunAction::AlreadyRunning => "already-running",
    };
    let code = exit_code(&record);
    Ok((kick_report(action, &record, &directory), code))
}

/// How one driver invocation is configured. Timeouts are concrete
/// deadlines around the abstract machine, never test budgets.
#[derive(Clone, Debug)]
pub struct DriverOptions {
    /// Agentctl state root holding `team-drivers/`; `None` derives the
    /// XDG state home the watch state also uses.
    pub state_dir: Option<PathBuf>,
    /// Seconds HANDOFF gets before ESCALATE.
    pub handoff_timeout: Duration,
    /// Seconds a sole coordinator acknowledgement may stay pending in
    /// VERIFY before it counts as failed.
    pub ack_timeout: Duration,
    /// The team lead's agent name, when the registry names one.
    pub lead: Option<String>,
    /// This host's name for escalation substitution and state readers.
    pub hostname: String,
    /// Argv a wake runs to re-enter this driver (absolute executable
    /// first); the CLI builds its own `team kick` invocation.
    pub wake_argv: Vec<String>,
}

impl Default for DriverOptions {
    fn default() -> Self {
        Self {
            state_dir: None,
            handoff_timeout: Duration::from_secs(DEFAULT_HANDOFF_TIMEOUT_SECONDS),
            ack_timeout: Duration::from_secs(DEFAULT_HANDOFF_TIMEOUT_SECONDS),
            lead: None,
            hostname: "unknown".to_owned(),
            wake_argv: Vec::new(),
        }
    }
}

/// What DETECT observed: enough to choose its model event.
#[derive(Clone, Debug, PartialEq)]
pub struct DetectOutcome {
    /// A desired agent or configured unit is missing or not live.
    pub missing: bool,
    /// The configured lead is live enough to hand a checklist to.
    pub lead_up: bool,
    /// One line of evidence for the state record on refusal paths.
    pub detail: String,
}

/// What one verify run found, beyond its printed report.
#[derive(Clone, Debug, PartialEq)]
pub struct VerifyOutcome {
    /// The whole `agentctl-team-verify/v1` report.
    pub report: Value,
    /// The configured lead is live enough to hand a checklist to.
    pub lead_up: bool,
    /// When the only failure is an unexpired coordinator ack, the Unix
    /// time it expires; `None` for every other outcome.
    pub ack_deadline: Option<f64>,
}

/// How one escalation attempt ended, before the model records it.
#[derive(Clone, Debug, PartialEq)]
pub enum EscalationOutcome {
    /// Some route delivered.
    Delivered,
    /// Every route failed; says how, for the alarm and state.
    Failed(String),
    /// No route is configured: an attempt that cannot deliver.
    Unconfigured,
}

/// The driver's concrete effects, seamed so fixture runs script every
/// outcome and never touch a live registry, unit, or binary. Each call
/// is one pending model effect (or the DETECT observation), already
/// persisted by the core before it runs, so the core may repeat it
/// after a crash: every implementation must stay idempotent.
pub trait DriverEffects {
    /// Observe desired agents and configured units for DETECT.
    fn detect(&mut self, record: &DriverRecord) -> Result<DetectOutcome>;
    /// Run one restore for the record's registry. An `Err` is a crash
    /// or refusal before a report exists; a restore that ran and wrote
    /// any report class returns `Ok(report)`.
    fn restore(&mut self, record: &DriverRecord) -> Result<Value>;
    /// Run the whole-team checks once.
    fn verify(&mut self, record: &DriverRecord) -> Result<VerifyOutcome>;
    /// Queue `text` for the lead under the deterministic `message_id`.
    fn queue_handoff(&mut self, record: &DriverRecord, text: &str, message_id: &str) -> Result<()>;
    /// Try to deliver `text` (or the resolved notice) to a human.
    fn escalate(
        &mut self,
        record: &DriverRecord,
        text: &str,
        resolved: bool,
    ) -> Result<EscalationOutcome>;
    /// Build the current recovery manifest (facts validation and the
    /// verify-after snapshot for one queued action).
    fn manifest(&mut self, record: &DriverRecord) -> Result<ManifestOutcome>;
    /// Execute one validated queued action. The core has persisted it
    /// running; retry after a crash is governed by the action's status.
    fn act(
        &mut self,
        record: &DriverRecord,
        action: &TeamAction,
        manifest: &Value,
        requester: &RequesterRecord,
    ) -> Result<ActionOutcome>;
}

/// The failed-check names in a verify report, in report order.
fn failed_checks(report: &Value) -> Vec<String> {
    report["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["status"] == "fail")
        .filter_map(|row| row["name"].as_str().map(str::to_owned))
        .collect()
}

/// Whether any row of a verify report is a deliberate hold.
fn report_held(report: &Value) -> bool {
    report["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|row| row["status"] == "held")
}

/// The deterministic handoff message ID for one checklist version: the
/// incident, an epoch discriminator (so one boot with two managers
/// never collides in the queue), and a hash of the quoted rows.
fn handoff_message_id(record: &DriverRecord) -> String {
    let mut hasher = Sha256::new();
    if let Some(rows) = record.last_verify["checks"].as_array() {
        for row in rows.iter().filter(|row| row["status"] == "fail") {
            hasher.update(row.to_string().as_bytes());
            hasher.update([0]);
        }
    }
    let manager = record.epoch["user_manager"]
        .as_str()
        .unwrap_or("managerless");
    let manager_hash = format!("{:x}", Sha256::digest(manager.as_bytes()));
    format!(
        "team-handoff-{}-{}-{}",
        record.incident,
        &manager_hash[..8],
        &format!("{:x}", hasher.finalize())[..8]
    )
}

/// Checklist text for the lead: the failed rows, quoted with their fix.
fn handoff_text(record: &DriverRecord) -> String {
    let mut lines = vec![format!(
        "Team restore needs you: verification failed for incident {} at state {}.",
        record.incident, record.state
    )];
    if let Some(rows) = record.last_verify["checks"].as_array() {
        for row in rows.iter().filter(|row| row["status"] == "fail") {
            lines.push(format!(
                "- {} ({}): {} Fix: {}",
                row["name"].as_str().unwrap_or("check"),
                row["subject"].as_str().unwrap_or("team"),
                row["evidence"].as_str().unwrap_or_default(),
                row["fix"].as_str().unwrap_or("see the verify report")
            ));
        }
    }
    lines.push(
        "The deterministic driver keeps re-verifying and escalates when this \
         handoff deadline passes; `agentctl team verify` shows the same rows live."
            .to_owned(),
    );
    lines.join("\n")
}

/// Escalation and alarm text for `resolved` or the current failure.
fn escalation_text(record: &DriverRecord, resolved: bool) -> String {
    if resolved {
        return format!(
            "Team restore incident {} at {} resolved: the team verifies healthy.",
            record.incident, record.registry
        );
    }
    let mut lines = vec![format!(
        "Team restore incident {} at {} is unrecovered at state {}.",
        record.incident, record.registry, record.state
    )];
    if let Some(rows) = record.last_verify["checks"].as_array() {
        for row in rows.iter().filter(|row| row["status"] == "fail") {
            lines.push(format!(
                "- {} ({}): {}",
                row["name"].as_str().unwrap_or("check"),
                row["subject"].as_str().unwrap_or("team"),
                row["evidence"].as_str().unwrap_or_default()
            ));
        }
    }
    if let Some(error) = record.last_error.as_deref() {
        lines.push(format!("Latest driver error: {error}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn temp_root(tag: &str) -> PathBuf {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let directory = std::env::temp_dir().join(format!(
            "team-driver-{}-{tag}-{sequence}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(directory.join("registry")).unwrap();
        directory
    }

    fn epoch(boot: &str, manager: Option<&str>) -> RestoreEpoch {
        RestoreEpoch {
            boot_id: boot.to_owned(),
            user_manager: manager.map(str::to_owned),
        }
    }

    #[test]
    fn state_contract_round_trips_and_lock_is_single() {
        let root = temp_root("contract");
        let registry = root.join("registry");
        let state_root = root.join("state");
        let directory = prepare_driver_dir(&registry, Some(&state_root)).unwrap();
        let mut record =
            DriverRecord::new(&registry, &epoch("boot-a", Some("invocation:m1")), 100.0);
        save_record(&directory, &mut record, 101.0).unwrap();
        let text = fs::read_to_string(directory.join("state.json")).unwrap();
        let document: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(document["schema"], "agentctl-team-driver/v1");
        assert_eq!(document["state"], "HEALTHY");
        assert_eq!(document["mode"], "dormant");
        assert_eq!(document["held"], false);
        assert_eq!(document["incident"].as_str().unwrap().len(), 12);
        assert_eq!(document["abstract"]["driver"], "healthy");
        assert_eq!(document["pending_effects"], json!([]));
        // Empty landing-2 fields are omitted, so an untouched record is
        // byte-identical for older readers; absent fields load as empty.
        assert!(document.get("manifest").is_none());
        assert!(document.get("actions").is_none());
        assert_eq!(load_record(&directory, &registry).unwrap(), Some(record));
        // Manager identity is recorded beside the incident, never keyed
        // into it: both sides of a namespace boundary derive one id.
        assert_eq!(
            incident_id(&registry, &epoch("boot-a", Some("invocation:m1"))),
            incident_id(&registry, &epoch("boot-a", Some("invocation:m2")))
        );
        assert_eq!(
            incident_id(&registry, &epoch("boot-a", None)),
            incident_id(&registry, &epoch("boot-a", None))
        );
        let first = try_acquire_lock(&directory).unwrap().expect("first lock");
        assert!(try_acquire_lock(&directory).unwrap().is_none());
        drop(first);
        assert!(try_acquire_lock(&directory).unwrap().is_some());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn action_ledger_dedupes_conflicts_stale_and_recovers_running() {
        let root = temp_root("actions");
        let registry = root.join("registry");
        let state_root = root.join("state");
        let directory = prepare_driver_dir(&registry, Some(&state_root)).unwrap();
        let mut record = DriverRecord::new(&registry, &epoch("boot-a", None), 100.0);
        save_record(&directory, &mut record, 100.0).unwrap();
        let requester = RequesterRecord {
            kind: "agent".to_owned(),
            label: "test".to_owned(),
            proof: "claimed".to_owned(),
        };
        let facts = new_action_request(
            &record.incident,
            record.epoch.clone(),
            "facts-1",
            &TeamAction::Facts,
            false,
            requester.clone(),
            100.0,
        )
        .unwrap();
        let path = enqueue_action(&directory, &facts).unwrap();
        assert_eq!(enqueue_action(&directory, &facts).unwrap(), path);
        let conflict = new_action_request(
            &record.incident,
            record.epoch.clone(),
            "facts-1",
            &TeamAction::DeclareDone,
            false,
            requester.clone(),
            100.0,
        )
        .unwrap();
        assert!(enqueue_action(&directory, &conflict).is_err());

        let host = FakeHost::new(100.0);
        let mut fake = Fake::default();
        let record = run(&root, &host, &mut fake).unwrap();
        assert_eq!(record.actions.len(), 1);
        assert_eq!(record.actions[0].status, "done");
        assert_eq!(record.actions[0].exit_code, Some(0));
        assert_eq!(fake.acted, [TeamAction::Facts]);
        assert_eq!(
            read_action_request(&path).unwrap().receipt.unwrap().status,
            "done"
        );
        // A second run replays nothing for the same request id.
        let record = run(&root, &host, &mut fake).unwrap();
        assert_eq!(fake.acted.len(), 1);

        // Recovery verbs refuse outside an incident state (this team is
        // Healthy), while an escalate delivers without setting a deadline.
        for (id, action) in [
            (
                "revive-healthy",
                TeamAction::Revive {
                    agent: "lane".to_owned(),
                },
            ),
            (
                "escalate-healthy",
                TeamAction::Escalate {
                    text: "owner note".to_owned(),
                },
            ),
        ] {
            let request = new_action_request(
                &record.incident,
                record.epoch.clone(),
                id,
                &action,
                false,
                requester.clone(),
                100.0,
            )
            .unwrap();
            enqueue_action(&directory, &request).unwrap();
        }
        // One pass drains every queued action before any exit is honoured.
        let record = run(&root, &host, &mut fake).unwrap();
        let by_id = |id: &str| {
            record
                .actions
                .iter()
                .find(|action| action.request_id == id)
                .unwrap()
        };
        assert_eq!(by_id("revive-healthy").status, "refused");
        assert_eq!(by_id("escalate-healthy").status, "done");
        assert_eq!(record.deadline, None);
        assert!(record.escalation.delivered_at.is_some());
        assert_eq!(fake.acted.len(), 2, "only the escalate ran");

        // Declare-done judges the raw verify: the bounded projection says
        // only "no bridge evidence", the raw report names the bridgeless
        // skip, and only the raw row can pass explicit YES.
        let mut bounded = Fake::fake_manifest(&record, true);
        bounded["verify"]["checks"][1] = json!({
            "name": "units-bridges",
            "status": "skipped",
            "subject": null,
            "evidence": "no bridge evidence",
        });
        let mut raw = bounded["verify"].clone();
        raw["checks"][1]["evidence"] = json!("no bridge state directories under the registry");
        fake.manifests.push_back(bounded.clone());
        fake.manifests.push_back(bounded);
        fake.manifest_verifies.push_back(raw.clone());
        fake.manifest_verifies.push_back(raw);
        let declare = new_action_request(
            &record.incident,
            record.epoch.clone(),
            "declare-1",
            &TeamAction::DeclareDone,
            false,
            requester.clone(),
            100.0,
        )
        .unwrap();
        enqueue_action(&directory, &declare).unwrap();
        let record = run(&root, &host, &mut fake).unwrap();
        let declared = record
            .actions
            .iter()
            .find(|action| action.request_id == "declare-1")
            .unwrap();
        assert_eq!(declared.status, "done", "{declared:?}");
        assert_eq!(declared.explicit_yes, Some(true));

        // A request bound to another manager epoch never executes.
        let stale = new_action_request(
            &record.incident,
            epoch("boot-a", Some("invocation:m2")).as_json(),
            "facts-2",
            &TeamAction::Facts,
            false,
            requester.clone(),
            100.0,
        )
        .unwrap();
        enqueue_action(&directory, &stale).unwrap();
        let record = run(&root, &host, &mut fake).unwrap();
        let stale_row = record
            .actions
            .iter()
            .find(|action| action.request_id == "facts-2")
            .unwrap();
        assert_eq!(stale_row.status, "refused");
        assert_eq!(fake.acted.len(), 3, "facts, escalate, and declare-done ran");

        // A driver that died mid-action requeues a re-runnable verb and
        // closes a non-idempotent one unknown without firing it again.
        for (id, action) in [
            ("facts-3", TeamAction::Facts),
            (
                "escalate-crash",
                TeamAction::Escalate {
                    text: "crash window".to_owned(),
                },
            ),
            (
                "kill-1",
                TeamAction::KillPid {
                    pid: 4242,
                    expect_cmdline: "fixture-proc".to_owned(),
                    expect_start: "99".to_owned(),
                    boot_id: "boot-a".to_owned(),
                    signal: None,
                },
            ),
        ] {
            let request = new_action_request(
                &record.incident,
                record.epoch.clone(),
                id,
                &action,
                false,
                requester.clone(),
                100.0,
            )
            .unwrap();
            enqueue_action(&directory, &request).unwrap();
        }
        let mut crashed = load_record(&directory, &registry).unwrap().unwrap();
        reconcile_action_requests(&directory, &mut crashed);
        for action in crashed.actions.iter_mut().filter(|action| {
            matches!(
                action.request_id.as_str(),
                "facts-3" | "kill-1" | "escalate-crash"
            )
        }) {
            action.status = "running".to_owned();
            action.started_at = Some(100.0);
        }
        save_record(&directory, &mut crashed, 100.0).unwrap();
        let record = run(&root, &host, &mut fake).unwrap();
        let by_id = |id: &str| {
            record
                .actions
                .iter()
                .find(|action| action.request_id == id)
                .unwrap()
        };
        assert_eq!(by_id("facts-3").status, "done");
        assert_eq!(by_id("kill-1").status, "unknown");
        assert_eq!(by_id("escalate-crash").status, "unknown");
        assert_eq!(fake.acted.len(), 4, "only the requeued facts also ran");

        // An older writer rewrote state.json from a snapshot taken
        // before this action folded: the request file and its running
        // marker survive, the state entry does not. The marker alone
        // must close it unknown, never re-run it.
        let lost = new_action_request(
            &record.incident,
            record.epoch.clone(),
            "kill-lost",
            &TeamAction::KillPid {
                pid: 4242,
                expect_cmdline: "fixture-proc".to_owned(),
                expect_start: "99".to_owned(),
                boot_id: "boot-a".to_owned(),
                signal: None,
            },
            false,
            RequesterRecord {
                kind: "agent".to_owned(),
                label: "forged".to_owned(),
                proof: "proved".to_owned(),
            },
            100.0,
        )
        .unwrap();
        enqueue_action(&directory, &lost).unwrap();
        fs::write(
            action_running_marker_path(&directory, &record.incident, "kill-lost"),
            "kill-lost\n",
        )
        .unwrap();
        let record = run(&root, &host, &mut fake).unwrap();
        let lost_row = record
            .actions
            .iter()
            .find(|action| action.request_id == "kill-lost")
            .unwrap();
        assert_eq!(lost_row.status, "unknown");
        assert_eq!(lost_row.requester.proof, "claimed");
        assert_eq!(fake.acted.len(), 4, "the lost kill never re-ran");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn resident_act_never_expires_the_deadline_it_woke_inside() {
        for (driver, delivered) in [(State::Handoff, false), (State::Escalate, true)] {
            let root = temp_root("resident-act");
            let registry = root.join("registry");
            let state_root = root.join("state");
            let directory = prepare_driver_dir(&registry, Some(&state_root)).unwrap();
            let mut record = DriverRecord::new(&registry, &epoch("boot-a", None), 100.0);
            record.abstract_state.driver = driver;
            record.abstract_state.mode = Mode::Dormant;
            record.abstract_state.wake_armed = true;
            record.abstract_state.delivered = delivered;
            record.abstract_state.attempts = 1;
            record.abstract_state.env.missing = false;
            record.abstract_state.env.lead_up = true;
            record.abstract_state.env.route_ok = true;
            record.abstract_state.env.fixed = true;
            record.deadline = Some(130.0);
            record.pending_effects = vec![Effect::Exit];
            save_record(&directory, &mut record, 100.0).unwrap();
            let request = new_action_request(
                &record.incident,
                record.epoch.clone(),
                "resident-facts",
                &TeamAction::Facts,
                false,
                RequesterRecord {
                    kind: "agent".to_owned(),
                    label: "test".to_owned(),
                    proof: "claimed".to_owned(),
                },
                100.0,
            )
            .unwrap();
            enqueue_action(&directory, &request).unwrap();
            let mut host = FakeHost::new(100.0);
            host.armed = false;
            let mut fake = Fake {
                verifies: [vpass(true), vpass(true), vpass(true)].into(),
                ..Fake::default()
            };
            let record = run(&root, &host, &mut fake).unwrap();
            assert_eq!(record.abstract_state.driver, State::Healthy, "{record:?}");
            assert!(
                fake.escalate_calls.is_empty(),
                "act paged early from {driver:?}"
            );
            assert_eq!(fake.acted, [TeamAction::Facts]);
            assert_eq!(record.actions[0].status, "done");
            fs::remove_dir_all(&root).unwrap();
        }
    }

    #[test]
    fn foreign_or_mismatched_state_is_refused_not_replaced() {
        let root = temp_root("foreign");
        let registry = root.join("registry");
        let state_root = root.join("state");
        let directory = prepare_driver_dir(&registry, Some(&state_root)).unwrap();
        let mut record = DriverRecord::new(&registry, &epoch("boot-a", None), 100.0);
        record.state = "VERIFY".to_owned();
        save_record(&directory, &mut record, 100.0).unwrap();
        // save_record re-derives mirrors, so corrupt the file by hand.
        let path = directory.join("state.json");
        fs::write(
            &path,
            fs::read_to_string(&path)
                .unwrap()
                .replace("HEALTHY", "VERIFY"),
        )
        .unwrap();
        assert!(load_record(&directory, &registry).is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    use crate::restore::{CommandOutcome, WakeOutcome, WakeRequest};
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;

    struct FakeHost {
        now: Cell<f64>,
        armed: bool,
        wakes: RefCell<Vec<WakeRequest>>,
        initial_namespace: bool,
    }
    impl FakeHost {
        fn new(now: f64) -> Self {
            Self {
                now: Cell::new(now),
                armed: true,
                wakes: RefCell::new(Vec::new()),
                initial_namespace: true,
            }
        }
    }
    impl Host for FakeHost {
        fn boot_id(&self) -> crate::agent::AgentResult<String> {
            Ok("boot-a".to_owned())
        }
        fn boot_time(&self) -> Option<u64> {
            Some(0)
        }
        fn now(&self) -> f64 {
            self.now.get()
        }
        fn sleep(&self, duration: Duration) {
            self.now.set(self.now.get() + duration.as_secs_f64());
        }
        fn run(&self, _argv: &[String], _cwd: Option<&Path>, _timeout: Duration) -> CommandOutcome {
            CommandOutcome::default()
        }
        fn arm_wake(&self, request: &WakeRequest) -> WakeOutcome {
            self.wakes.borrow_mut().push(request.clone());
            if self.armed {
                WakeOutcome::Armed {
                    unit: format!("agentctl-team-wake@{}.timer", request.team_key),
                }
            } else {
                WakeOutcome::Unsupported
            }
        }
        fn in_initial_pid_namespace(&self) -> bool {
            self.initial_namespace
        }
    }

    #[derive(Default)]
    struct Fake {
        missing: bool,
        lead_up: bool,
        detect_calls: usize,
        restore_calls: usize,
        verifies: VecDeque<VerifyOutcome>,
        queued: Vec<(String, String)>,
        queue_fails: bool,
        escalations: VecDeque<EscalationOutcome>,
        escalate_calls: Vec<bool>,
        manifests: VecDeque<Value>,
        manifest_verifies: VecDeque<Value>,
        act_outcomes: VecDeque<ActionOutcome>,
        acted: Vec<TeamAction>,
        crash: Option<&'static str>,
    }
    impl Fake {
        fn fake_manifest(record: &DriverRecord, ok: bool) -> Value {
            json!({
                "schema": "agentctl-team-manifest/v1",
                "registry": record.registry,
                "coordinator": "coord",
                "desired": ["coord", "lane"],
                "ok": ok,
                "agents": [
                    {"name": "coord", "native_session_present": true},
                    {"name": "lane", "native_session_present": true},
                ],
                "units": [{"name": "bridge.service", "active": ok, "held": false}],
                "verify": {
                    "schema": "agentctl-team-verify/v1",
                    "ok": ok,
                    "desired": ["coord", "lane"],
                    "checks": [
                        {"name": "desired-live", "status": if ok { "pass" } else { "fail" }, "subject": "coord"},
                        {"name": "units-bridges", "status": if ok { "pass" } else { "fail" }, "subject": "bridge chat"},
                        {"name": "pane-at-prompt", "status": "pass", "subject": "lane"},
                        {"name": "coordinator-ack", "status": if ok { "pass" } else { "fail" }, "subject": "coord"},
                    ],
                },
            })
        }

        fn maybe_crash(&mut self, name: &'static str) -> Result<()> {
            if self.crash == Some(name) {
                self.crash = None;
                return Err(fail("simulated crash"));
            }
            Ok(())
        }
    }
    impl DriverEffects for Fake {
        fn detect(&mut self, _record: &DriverRecord) -> Result<DetectOutcome> {
            self.detect_calls += 1;
            Ok(DetectOutcome {
                missing: self.missing,
                lead_up: self.lead_up,
                detail: if self.missing {
                    "missing".to_owned()
                } else {
                    String::new()
                },
            })
        }
        fn restore(&mut self, _record: &DriverRecord) -> Result<Value> {
            self.maybe_crash("restore")?;
            self.restore_calls += 1;
            self.missing = false;
            Ok(json!({"complete": true}))
        }
        fn verify(&mut self, _record: &DriverRecord) -> Result<VerifyOutcome> {
            self.maybe_crash("verify")?;
            Ok(self.verifies.pop_front().expect("scripted verify outcome"))
        }
        fn queue_handoff(&mut self, _record: &DriverRecord, text: &str, id: &str) -> Result<()> {
            self.maybe_crash("queue")?;
            if self.queue_fails {
                return Err(fail("queue refused"));
            }
            self.queued.push((id.to_owned(), text.to_owned()));
            Ok(())
        }
        fn escalate(
            &mut self,
            _record: &DriverRecord,
            _text: &str,
            resolved: bool,
        ) -> Result<EscalationOutcome> {
            self.maybe_crash("escalate")?;
            self.escalate_calls.push(resolved);
            Ok(self
                .escalations
                .pop_front()
                .unwrap_or(EscalationOutcome::Delivered))
        }
        fn manifest(&mut self, record: &DriverRecord) -> Result<ManifestOutcome> {
            self.maybe_crash("manifest")?;
            let manifest = self
                .manifests
                .pop_front()
                .unwrap_or_else(|| Self::fake_manifest(record, !self.missing));
            let verify = self
                .manifest_verifies
                .pop_front()
                .unwrap_or_else(|| manifest["verify"].clone());
            Ok(ManifestOutcome { manifest, verify })
        }
        fn act(
            &mut self,
            _record: &DriverRecord,
            action: &TeamAction,
            _manifest: &Value,
            _requester: &RequesterRecord,
        ) -> Result<ActionOutcome> {
            self.maybe_crash("act")?;
            self.acted.push(action.clone());
            Ok(self
                .act_outcomes
                .pop_front()
                .unwrap_or_else(|| ActionOutcome {
                    status: "done".to_owned(),
                    detail: "fake action done".to_owned(),
                    escalation: None,
                    kill: None,
                }))
        }
    }

    fn vreport(fails: &[&str], held: bool) -> Value {
        let mut checks: Vec<Value> = fails
            .iter()
            .map(|name| json!({"name": name, "status": "fail", "subject": "lane", "evidence": "bad", "fix": "fix it"}))
            .collect();
        if held {
            checks.push(json!({"name": "units-bridges", "status": "held", "subject": "unit", "evidence": "held (pin: window)", "fix": null}));
        }
        json!({"schema": "agentctl-team-verify/v1", "ok": fails.is_empty(), "checks": checks})
    }
    fn vpass(lead_up: bool) -> VerifyOutcome {
        VerifyOutcome {
            report: vreport(&[], false),
            lead_up,
            ack_deadline: None,
        }
    }
    fn vfail(names: &[&str], lead_up: bool, ack_deadline: Option<f64>) -> VerifyOutcome {
        VerifyOutcome {
            report: vreport(names, false),
            lead_up,
            ack_deadline,
        }
    }
    fn test_options(root: &Path) -> DriverOptions {
        DriverOptions {
            state_dir: Some(root.join("state")),
            lead: Some("coord".to_owned()),
            hostname: "host-a".to_owned(),
            wake_argv: vec![
                "/bin/agentctl".to_owned(),
                "team".to_owned(),
                "kick".to_owned(),
            ],
            ..DriverOptions::default()
        }
    }
    fn run(root: &Path, host: &FakeHost, fake: &mut Fake) -> Result<DriverRecord> {
        // A run that finds the driver lock still held reports the stale
        // record and consumes no effects, so retrying until this run
        // actually drove the machine is safe — and necessary when a
        // just-released lock is not yet visible as free. The guard only
        // reports a hang; having run is the condition.
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        loop {
            let (record, action) =
                run_with_effects(&root.join("registry"), host, fake, &test_options(root))?;
            if action == RunAction::Ran || std::time::Instant::now() >= deadline {
                assert_eq!(action, RunAction::Ran, "the run never drove the machine");
                return Ok(record);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn driver_healthy_noop_then_happy_restore() {
        let root = temp_root("happy");
        let registry = root.join("registry");
        let host = FakeHost::new(1000.0);
        let mut fake = Fake {
            lead_up: true,
            ..Fake::default()
        };
        let record = run(&root, &host, &mut fake).unwrap();
        assert_eq!(record.state, "HEALTHY");
        assert_eq!(fake.restore_calls, 0);
        fake.missing = true;
        fake.verifies.push_back(vpass(true));
        let record = run(&root, &host, &mut fake).unwrap();
        assert_eq!(record.state, "HEALTHY");
        assert_eq!(fake.restore_calls, 1);
        assert_eq!(exit_code(&record), 0);
        let _ = registry;
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_released_driver_lock_is_free_though_a_descriptor_copy_survives() {
        // A child forked while the driver holds its lock shares the
        // lock's open file description until its exec, exactly as a
        // dup'd descriptor does here. Releasing by closing the
        // holder's descriptor alone would keep the flock held for as
        // long as that copy lives, and this test's second try would
        // find it busy; the explicit unlock on drop frees it at once.
        use std::os::fd::AsRawFd;

        let root = temp_root("lock-release");
        let directory = root.join("registry");
        let lock = try_acquire_lock(&directory)
            .expect("acquire")
            .expect("the lock is free");
        let copy = unsafe { libc::dup(lock._file.as_raw_fd()) };
        assert!(copy >= 0, "dup the lock descriptor");
        drop(lock);
        let again = try_acquire_lock(&directory).expect("try again");
        assert!(
            again.is_some(),
            "a surviving descriptor copy still holds the released lock"
        );
        drop(again);
        unsafe { libc::close(copy) };
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn driver_ack_waits_expires_hands_off_then_progresses() {
        let root = temp_root("ack");
        let host = FakeHost::new(1000.0);
        let mut fake = Fake {
            missing: true,
            lead_up: true,
            verifies: VecDeque::from([
                vfail(&["coordinator-ack"], true, Some(2800.0)),
                vfail(&["coordinator-ack"], true, None),
                vpass(true),
                vpass(true),
            ]),
            ..Fake::default()
        };
        let record = run(&root, &host, &mut fake).unwrap();
        assert_eq!(record.state, "VERIFY");
        assert_eq!(record.deadline, Some(2800.0));
        assert_eq!(exit_code(&record), 75);
        host.now.set(2800.0);
        let record = run(&root, &host, &mut fake).unwrap();
        assert_eq!(record.state, "HANDOFF");
        assert_eq!(record.deadline, Some(4600.0));
        assert_eq!(fake.queued.len(), 1);
        host.now.set(4600.0);
        let record = run(&root, &host, &mut fake).unwrap();
        assert_eq!(record.state, "HEALTHY");
        assert_eq!(fake.queued.len(), 1, "one checklist per incident version");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn driver_no_lead_escalates_then_resolves() {
        let root = temp_root("escalate");
        let host = FakeHost::new(1000.0);
        let mut fake = Fake {
            missing: true,
            lead_up: false,
            verifies: VecDeque::from([vfail(&["desired-live"], false, None), vpass(false)]),
            escalations: VecDeque::from([
                EscalationOutcome::Delivered,
                EscalationOutcome::Delivered,
            ]),
            ..Fake::default()
        };
        let record = run(&root, &host, &mut fake).unwrap();
        assert_eq!(record.state, "ESCALATE");
        assert!(record.escalation.delivered_at.is_some());
        host.now.set(record.deadline.unwrap());
        let record = run(&root, &host, &mut fake).unwrap();
        assert_eq!(record.state, "HEALTHY");
        assert!(record.escalation.resolved_at.is_some());
        assert_eq!(fake.escalate_calls, [false, true]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn driver_handoff_deadline_escalates_with_alarm() {
        let root = temp_root("alarm");
        let host = FakeHost::new(1000.0);
        let mut fake = Fake {
            missing: true,
            lead_up: true,
            verifies: VecDeque::from([
                vfail(&["desired-live"], true, None),
                vfail(&["desired-live"], true, None),
                vfail(&["desired-live"], true, None),
            ]),
            escalations: VecDeque::from([
                EscalationOutcome::Failed("route down".to_owned()),
                EscalationOutcome::Delivered,
            ]),
            ..Fake::default()
        };
        let record = run(&root, &host, &mut fake).unwrap();
        assert_eq!(record.state, "HANDOFF");
        host.now.set(record.deadline.unwrap());
        let record = run(&root, &host, &mut fake).unwrap();
        assert_eq!(record.state, "ESCALATE");
        let directory =
            prepare_driver_dir(&root.join("registry"), Some(&root.join("state"))).unwrap();
        assert!(
            directory.join("alarm.json").exists(),
            "undelivered writes the alarm"
        );
        host.now.set(record.deadline.unwrap());
        let record = run(&root, &host, &mut fake).unwrap();
        assert_eq!(record.state, "ESCALATE");
        assert!(record.escalation.delivered_at.is_some());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn driver_crash_resumes_and_second_runner_only_reports() {
        let root = temp_root("crash");
        let registry = root.join("registry");
        let host = FakeHost::new(1000.0);
        let options = test_options(&root);
        let directory = prepare_driver_dir(&registry, options.state_dir.as_deref()).unwrap();
        // A held lock makes the invocation a pure state report.
        let fresh = DriverRecord::new(&registry, &epoch("boot-a", None), 1000.0);
        let mut fresh = fresh;
        save_record(&directory, &mut fresh, 1000.0).unwrap();
        let held = try_acquire_lock(&directory).unwrap().unwrap();
        let mut fake = Fake::default();
        let (reported, action) = run_with_effects(&registry, &host, &mut fake, &options).unwrap();
        assert_eq!(action, RunAction::AlreadyRunning);
        assert_eq!(fake.detect_calls, 0);
        drop(held);
        // Crash inside restore: the persisted pending launch resumes.
        let mut fake = Fake {
            missing: true,
            lead_up: true,
            verifies: VecDeque::from([vpass(true)]),
            crash: Some("restore"),
            ..Fake::default()
        };
        assert!(run(&root, &host, &mut fake).is_err());
        let saved = load_record(&directory, &registry).unwrap().unwrap();
        assert_eq!(saved.state, "RESTORE");
        assert_eq!(saved.pending_effects, [Effect::LaunchRestore]);
        let record = run(&root, &host, &mut fake).unwrap();
        assert_eq!(record.state, "HEALTHY");
        assert_eq!(fake.restore_calls, 1);
        let _ = reported;
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn driver_refuses_a_descendant_pid_namespace() {
        let root = temp_root("namespace");
        let host = FakeHost {
            initial_namespace: false,
            ..FakeHost::new(1000.0)
        };
        let mut fake = Fake::default();
        let error = run(&root, &host, &mut fake).unwrap_err().to_string();
        assert!(error.contains("host namespace"), "{error}");
        assert!(!root.join("state/team-drivers").exists());
        fs::remove_dir_all(&root).unwrap();
    }
}
