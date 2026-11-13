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
    /// Effects of the last transition not yet completed, in order.
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

/// The one-driver lock, held only while a run can still return it.
pub struct DriverLock {
    _file: File,
}

/// Take the one-driver lock without waiting: `None` while another run
/// holds it, which is how a second kick only reports state.
pub fn try_acquire_lock(directory: &Path) -> Result<Option<DriverLock>> {
    let file =
        agent_queue::fsio::open_private_lock(&directory.join("driver.lock"), "team driver lock")
            .map_err(|error| fail(error.to_string()))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(DriverLock { _file: file })),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(fail(format!("cannot lock the team driver state: {error}"))),
    }
}

/// Wait used between concrete re-checks while resident; exposed so the
/// CLI help and fakes name the same bound.
pub const RESIDENT_POLL: Duration = Duration::from_secs(RESIDENT_POLL_SECONDS);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn temp_root(tag: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("team-driver-{}-{tag}", std::process::id()));
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
}
