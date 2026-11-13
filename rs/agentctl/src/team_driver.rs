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
        apply_event(&table, &mut record, Event::NewEpoch, host.now(), options)?;
        save_record(&directory, &mut record, host.now())?;
    }
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
                Effect::Exit => {
                    if record.abstract_state.driver == State::Healthy
                        || record.wake.outcome == "armed"
                    {
                        record.pending_effects.remove(0);
                        save_record(&directory, &mut record, now)?;
                        return Ok((record, RunAction::Ran));
                    }
                    match resident_wait(&directory, &mut record, host, effects, options)? {
                        Resident::Terminated => {
                            save_record(&directory, &mut record, host.now())?;
                            return Ok((record, RunAction::Ran));
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
                    "systemctl".to_owned(),
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
                        let finished = crate::restore::candidate_report_paths(
                            &self.registry,
                            &self.config,
                            &epoch.boot_id,
                        )
                        .into_iter()
                        .find_map(|path| crate::restore::read_report(&path))
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
            "systemctl".to_owned(),
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
                "systemctl".to_owned(),
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
        crash: Option<&'static str>,
    }
    impl Fake {
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
        run_with_effects(&root.join("registry"), host, fake, &test_options(root))
            .map(|(record, _)| record)
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
