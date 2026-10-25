//! Deliverers: what the receiving host hands an accepted message to
//! (design section 4.4). `accept` never types; the local deliverer does.
//!
//! Delivery is behind the [`Deliverer`] trait so agent-teams stays
//! independent of Herdr and agentctl. The `agentctl-queue` deliverer
//! enqueues through the shared `agent-queue` crate (the same queue format
//! and duplicate check agentctl's drainers use), and tests use the
//! [`FakeDeliverer`].

use crate::jsonfile::DataResult;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Outcome of one hand-over to a deliverer.
#[derive(Debug, Clone, PartialEq)]
pub enum Handover {
    /// The message is with the deliverer (queued or in the hook inbox).
    Accepted,
    /// The hand-over was refused before anything was delivered.
    Refused(String),
    /// The hand-over may or may not have happened; never retried blind.
    PossiblySubmitted(String),
}

/// Where a ledger claim says its hand-over went (design section 9.2).
#[derive(Debug, Clone)]
pub struct HandoverTarget {
    /// Deliverer kind.
    pub kind: String,
    /// For `agentctl-queue`: the queue directory.
    pub queue_dir: Option<PathBuf>,
    /// For `hook-inbox`: the inbox directory.
    pub inbox_dir: Option<PathBuf>,
}

/// A deliverer: the receiving host's final hop for one team.
pub trait Deliverer {
    /// Deliverer kind name as in the declaration.
    fn kind(&self) -> &str;
    /// The hand-over target recorded in the ledger claim before handing over.
    fn target(&self) -> HandoverTarget;
    /// Hand the rendered message over. Must be idempotent per ID where the
    /// underlying store allows it (queue lane, hook inbox).
    fn handover(&self, id: &str, envelope: &Value, rendered: &str) -> Handover;
    /// Whether the ID is provably with this deliverer (used by recovery).
    fn contains(&self, id: &str) -> Option<bool>;
    /// Wakes an idle agent? Shown to senders (`hook-only` when false).
    fn wakes_idle(&self) -> bool {
        true
    }
    /// Nudge delivery of an already-accepted ID that may still be waiting
    /// (a retry's known-accepted answer, recovery). Default: nothing.
    fn nudge(&self, _id: &str) {}
}

/// This binary's path for spawning our own helper processes
/// (`$AGENT_TEAMS_SELF_BIN`, else the running executable).
pub fn self_binary() -> PathBuf {
    std::env::var_os("AGENT_TEAMS_SELF_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_exe().unwrap_or_else(|_| PathBuf::from("agent-teams")))
}

/// Write `<dir>/<id>.json` by temp file plus `link()`; fails if present.
fn link_into(dir: &Path, id: &str, contents: &[u8]) -> std::io::Result<bool> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".tmp-{id}-{}", std::process::id()));
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
    }
    let target = dir.join(format!("{id}.json"));
    match std::fs::hard_link(&tmp, &target) {
        Ok(()) => {
            let _ = std::fs::remove_file(&tmp);
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_file(&tmp);
            Ok(false)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn queue_item(envelope: &Value, rendered: &str, ledger_path: &Path) -> Value {
    json!({
        "id": envelope.get("id"),
        "text": rendered,
        "not_after": envelope.get("not_after"),
        "topic": envelope.get("topic"),
        "external_delivery_record": ledger_path.to_string_lossy(),
        "envelope": envelope,
    })
}

/// The `hook-inbox` deliverer: a per-team directory a harness hook drains at
/// the next turn boundary. Never wakes an idle agent.
pub struct HookInboxDeliverer {
    /// Inbox directory.
    pub inbox_dir: PathBuf,
    /// Ledger entry path recorded in each item.
    pub ledger_path: PathBuf,
}

impl Deliverer for HookInboxDeliverer {
    fn kind(&self) -> &str {
        "hook-inbox"
    }
    fn target(&self) -> HandoverTarget {
        HandoverTarget {
            kind: self.kind().to_string(),
            queue_dir: None,
            inbox_dir: Some(self.inbox_dir.clone()),
        }
    }
    fn handover(&self, id: &str, envelope: &Value, rendered: &str) -> Handover {
        let item = queue_item(envelope, rendered, &self.ledger_path);
        match link_into(&self.inbox_dir, id, item.to_string().as_bytes()) {
            Ok(_) => Handover::Accepted,
            Err(e) => Handover::Refused(format!("hook inbox write failed: {e}")),
        }
    }
    fn contains(&self, id: &str) -> Option<bool> {
        Some(
            self.inbox_dir.join(format!("{id}.json")).exists()
                || self
                    .inbox_dir
                    .join("claimed")
                    .join(format!("{id}.json"))
                    .exists(),
        )
    }
    fn wakes_idle(&self) -> bool {
        false
    }
}

/// Delivery on arrival for the `agentctl-queue` deliverer: after a
/// successful enqueue, start one bounded `agentctl drain` (in a detached
/// helper process, so `accept` never waits for typing and the drain
/// survives the one-shot serve process) instead of leaving the item
/// for some later drain. On by default; a declaration turns it off with
/// `"auto_drain": false`.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoDrain {
    /// Start a drain after each accepted hand-over.
    pub enabled: bool,
    /// `--ready-timeout` for the drain, in seconds.
    pub ready_timeout_seconds: u64,
}

impl Default for AutoDrain {
    fn default() -> Self {
        AutoDrain {
            enabled: true,
            ready_timeout_seconds: 120,
        }
    }
}

/// The `agentctl-queue` deliverer: enqueues into the designated
/// coordinator's queue through the shared `agent-queue` crate; that host's
/// drainers type it. The enqueue is unserialized (`serialize: false`):
/// `accept` must never block on the queue's delivery lock (design 9.2),
/// and the ledger claim that precedes every hand-over is what makes this
/// ID single-writer, so the crate's no-replace create is the exact
/// duplicate guard. Team items join the crate's `inbox.v2/` lane, which only
/// drainers that re-check `not_after` and tombstones just before typing
/// read; older drainers never see them.
pub struct AgentctlQueueDeliverer {
    /// The coordinator's queue directory (`REGISTRY/<agent>/queue`).
    pub queue_dir: PathBuf,
    /// Ledger entry path recorded in each item.
    pub ledger_path: PathBuf,
    /// Registry the queue belongs to (for the drain command).
    pub registry: PathBuf,
    /// Coordinator agent name in that registry.
    pub agent: String,
    /// Delivery-on-arrival settings.
    pub auto_drain: AutoDrain,
    /// State directory holding the drain lock and `team-log.jsonl`;
    /// `None` disables auto-drain (tests, embedding).
    pub drain_state_dir: Option<PathBuf>,
}

impl AgentctlQueueDeliverer {
    /// The queue item: the crate's plain item plus the team fields a
    /// reader may ignore until P2 teaches drainers to honour them.
    fn document(&self, id: &str, envelope: &Value, rendered: &str) -> Value {
        let mut item = agent_queue::queue::plain_item(id, rendered);
        if let Some(obj) = item.as_object_mut() {
            if let Some(ms) = envelope
                .get("not_after")
                .and_then(Value::as_str)
                .and_then(crate::timeutil::parse_rfc3339_ms)
            {
                obj.insert("not_after".to_string(), json!(ms));
            }
            obj.insert(
                "topic".to_string(),
                envelope.get("topic").cloned().unwrap_or(Value::Null),
            );
            obj.insert(
                "external_delivery_record".to_string(),
                json!(self.ledger_path.to_string_lossy()),
            );
            obj.insert("envelope".to_string(), envelope.clone());
        }
        item
    }

    /// Start the bounded delivery-on-arrival drain for this item, if
    /// enabled, as a detached `drain-one` helper process holding a
    /// per-agent lock (never more than one of our drains per queue). A
    /// drain that finds the item already processed does nothing. Logs are
    /// content-free (IDs and outcomes only).
    fn start_drain(&self, id: &str) {
        if !self.auto_drain.enabled {
            return;
        }
        let Some(state) = self.drain_state_dir.clone() else {
            return;
        };
        // A detached helper PROCESS, not a thread: serve is one-shot and
        // exits right after answering, which would kill a thread mid-drain
        // (live-trial finding, 2026-10-25). The helper (`drain-one`)
        // survives us, holds the per-agent lock, and logs content-free.
        let spawned = std::process::Command::new(self_binary())
            .args([
                "drain-one",
                "--queue-dir",
                &self.queue_dir.to_string_lossy(),
                "--registry",
                &self.registry.to_string_lossy(),
                "--agent",
                &self.agent,
                "--id",
                id,
                "--ready-timeout",
                &self.auto_drain.ready_timeout_seconds.to_string(),
                "--state-dir",
                &state.to_string_lossy(),
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        let outcome = if spawned.is_ok() {
            "spawned"
        } else {
            "spawn-failed"
        };
        crate::sender::log_event(
            &state,
            &json!({"event": "auto-drain", "id": id, "agent": self.agent, "outcome": outcome}),
        );
    }

    /// Whether the item already in the queue under `id` is this envelope.
    fn existing_is_ours(&self, id: &str, envelope: &Value) -> Option<bool> {
        let directories = agent_queue::queue::QueueDirectories::new(&self.queue_dir);
        for (dir, _state) in directories.states() {
            let path = dir.join(format!("{id}.json"));
            if path.exists() {
                let doc = agent_queue::queue::load_message(&path).ok()?;
                return Some(
                    doc.get("envelope").and_then(|e| e.get("body_sha256"))
                        == envelope.get("body_sha256"),
                );
            }
        }
        Some(false)
    }
}

impl Deliverer for AgentctlQueueDeliverer {
    fn kind(&self) -> &str {
        "agentctl-queue"
    }
    fn target(&self) -> HandoverTarget {
        HandoverTarget {
            kind: self.kind().to_string(),
            queue_dir: Some(self.queue_dir.clone()),
            inbox_dir: None,
        }
    }
    fn handover(&self, id: &str, envelope: &Value, rendered: &str) -> Handover {
        let outcome = self.enqueue(id, envelope, rendered);
        if outcome == Handover::Accepted {
            self.start_drain(id);
        }
        outcome
    }
    fn contains(&self, id: &str) -> Option<bool> {
        self.queue_contains(id)
    }
    fn nudge(&self, id: &str) {
        // Only a queued, not-yet-processed item needs a drain; the helper
        // re-checks under its lock before typing anything.
        if self.queue_contains(id) == Some(true) {
            self.start_drain(id);
        }
    }
}

/// One bounded delivery-on-arrival drain, run by the detached
/// `drain-one` helper process `start_drain` spawns. Holds the per-agent
/// drain lock for its whole run, so our drains never overlap on one queue;
/// the queue's own delivery lock inside `agentctl drain` remains the
/// typing serialization.
pub fn run_auto_drain(
    state: &Path,
    queue_dir: &Path,
    registry: &Path,
    agent: &str,
    id: &str,
    ready_timeout_seconds: u64,
) {
    use fs2::FileExt;
    let log = |outcome: &str| {
        crate::sender::log_event(
            state,
            &json!({"event": "auto-drain", "id": id, "agent": agent, "outcome": outcome}),
        );
    };
    let lock_path = state.join(format!("auto-drain-{agent}.lock"));
    let Ok(lock_file) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
    else {
        log("lock-open-failed");
        return;
    };
    if lock_file.lock_exclusive().is_err() {
        log("lock-failed");
        return;
    }
    // A drain we queued behind may already have typed this item.
    if let Ok(Some(agent_queue::queue::MessageState::Processed)) =
        agent_queue::queue::message_state(queue_dir, id)
    {
        log("already-processed");
        return;
    }
    log("started");
    let spawned = crate::proof::agentctl_command()
        .arg("--registry")
        .arg(registry)
        .args([
            "drain",
            agent,
            "--deliver-while-working",
            "--ready-timeout",
            &ready_timeout_seconds.to_string(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            crate::sender::log_event(
                state,
                &json!({"event": "auto-drain", "id": id, "agent": agent,
                        "outcome": "spawn-failed", "error": error.to_string()}),
            );
            return;
        }
    };
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(ready_timeout_seconds + 180);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    log("done");
                } else {
                    crate::sender::log_event(
                        state,
                        &json!({"event": "auto-drain", "id": id, "agent": agent,
                                "outcome": "failed", "exit_code": status.code()}),
                    );
                }
                return;
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                log("timed-out");
                return;
            }
            Err(_) => {
                log("wait-failed");
                return;
            }
        }
    }
}

impl AgentctlQueueDeliverer {
    fn enqueue(&self, id: &str, envelope: &Value, rendered: &str) -> Handover {
        let result = agent_queue::queue::enqueue_into(
            &self.queue_dir,
            agent_queue::queue::Lane::InboxV2,
            Some(id),
            false,
            &agent_queue::fsio::Blocking,
            |identifier| self.document(identifier, envelope, rendered),
        );
        match result {
            Ok(_) => Handover::Accepted,
            Err(e) if e.kind() == agent_queue::ErrorKind::Duplicate => {
                match self.existing_is_ours(id, envelope) {
                    Some(true) => Handover::Accepted,
                    Some(false) => Handover::PossiblySubmitted(
                        "queue already holds a different item under this ID".to_string(),
                    ),
                    None => Handover::PossiblySubmitted(
                        "queue holds this ID but its item could not be read".to_string(),
                    ),
                }
            }
            Err(e) => match self.queue_contains(id) {
                Some(true) => Handover::Accepted,
                Some(false) => Handover::Refused(format!("queue enqueue failed: {}", e.message())),
                None => Handover::PossiblySubmitted(format!(
                    "queue enqueue failed and presence is unprovable: {}",
                    e.message()
                )),
            },
        }
    }
    fn queue_contains(&self, id: &str) -> Option<bool> {
        use agent_queue::queue::MessageState;
        match agent_queue::queue::message_state(&self.queue_dir, id) {
            // Set aside untyped (a tombstone or expiry) still means the hand-over happened, so
            // recovery never hands the ID over again.
            Ok(Some(
                MessageState::Pending
                | MessageState::Inflight
                | MessageState::Processed
                | MessageState::Cancelled,
            )) => Some(true),
            // Quarantined: the hand-over happened but delivery is unsettled,
            // so recovery must settle it as possibly_submitted, not accepted.
            Ok(Some(MessageState::Failed)) => None,
            Ok(None) => Some(false),
            Err(_) => None,
        }
    }
}

/// The `command` deliverer: runs an owner-supplied argv with the rendered
/// message on stdin. Not idempotent; recovery records `possibly_submitted`.
pub struct CommandDeliverer {
    /// Argv to run.
    pub argv: Vec<String>,
}

impl Deliverer for CommandDeliverer {
    fn kind(&self) -> &str {
        "command"
    }
    fn target(&self) -> HandoverTarget {
        HandoverTarget {
            kind: self.kind().to_string(),
            queue_dir: None,
            inbox_dir: None,
        }
    }
    fn handover(&self, _id: &str, _envelope: &Value, rendered: &str) -> Handover {
        let Some((program, args)) = self.argv.split_first() else {
            return Handover::Refused("command deliverer has no argv".to_string());
        };
        let spawned = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(e) => return Handover::Refused(format!("command deliverer failed to start: {e}")),
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(rendered.as_bytes());
        }
        match child.wait() {
            Ok(status) if status.success() => Handover::Accepted,
            Ok(status) => Handover::PossiblySubmitted(format!("command deliverer exited {status}")),
            Err(e) => Handover::PossiblySubmitted(format!("command deliverer wait failed: {e}")),
        }
    }
    fn contains(&self, _id: &str) -> Option<bool> {
        None
    }
}

/// A fake deliverer for tests: counts pastes per ID.
#[derive(Default)]
pub struct FakeDeliverer {
    /// Rendered texts handed over, in order.
    pub pastes: Mutex<Vec<(String, String)>>,
}

impl Deliverer for FakeDeliverer {
    fn kind(&self) -> &str {
        "fake"
    }
    fn target(&self) -> HandoverTarget {
        HandoverTarget {
            kind: self.kind().to_string(),
            queue_dir: None,
            inbox_dir: None,
        }
    }
    fn handover(&self, id: &str, _envelope: &Value, rendered: &str) -> Handover {
        self.pastes
            .lock()
            .expect("fake deliverer lock")
            .push((id.to_string(), rendered.to_string()));
        Handover::Accepted
    }
    fn contains(&self, id: &str) -> Option<bool> {
        Some(
            self.pastes
                .lock()
                .expect("fake deliverer lock")
                .iter()
                .any(|(existing, _)| existing == id),
        )
    }
}

impl FakeDeliverer {
    /// How many times this ID was handed over (must never exceed one).
    pub fn paste_count(&self, id: &str) -> usize {
        self.pastes
            .lock()
            .expect("fake deliverer lock")
            .iter()
            .filter(|(existing, _)| existing == id)
            .count()
    }
}

/// Build the deliverer a declaration names, rooted at the team's state dir.
pub fn deliverer_for(
    declaration: &crate::model::Declaration,
    team_state_dir: &Path,
    ledger_path: &Path,
) -> DataResult<Box<dyn Deliverer>> {
    match declaration.deliverer_kind.as_str() {
        "hook-inbox" => Ok(Box::new(HookInboxDeliverer {
            inbox_dir: declaration
                .deliverer
                .get("inbox_dir")
                .and_then(Value::as_str)
                .map(crate::jsonfile::expand_home)
                .unwrap_or_else(|| team_state_dir.join("hook-inbox")),
            ledger_path: ledger_path.to_path_buf(),
        })),
        "agentctl-queue" => {
            let registry = declaration
                .deliverer
                .get("registry")
                .and_then(Value::as_str)
                .map(crate::jsonfile::expand_home)
                .ok_or_else(|| {
                    crate::jsonfile::DataError::new(
                        "declaration",
                        "agentctl-queue deliverer needs a registry path",
                    )
                })?;
            // Until the registry carries its own coordinator designation
            // (phase P0b), a declaration may name the agent explicitly.
            let coordinator = match declaration.deliverer.get("agent").and_then(Value::as_str) {
                Some(agent) => agent.to_string(),
                None => crate::proof::registry_coordinator_name(&registry).ok_or_else(|| {
                    crate::jsonfile::DataError::new(
                        "no-coordinator",
                        format!(
                            "registry {} has no coordinator designation",
                            registry.display()
                        ),
                    )
                })?,
            };
            let auto_drain = AutoDrain {
                enabled: declaration
                    .deliverer
                    .get("auto_drain")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                ready_timeout_seconds: declaration
                    .deliverer
                    .get("auto_drain_ready_timeout_seconds")
                    .and_then(Value::as_u64)
                    .unwrap_or(120),
            };
            // team_state_dir is <state>/teams.d/<team>; the drain lock and
            // content-free log belong to the state root.
            let drain_state_dir = team_state_dir
                .parent()
                .and_then(|teams_d| teams_d.parent())
                .map(|state| state.to_path_buf());
            Ok(Box::new(AgentctlQueueDeliverer {
                queue_dir: registry.join(&coordinator).join("queue"),
                ledger_path: ledger_path.to_path_buf(),
                registry,
                agent: coordinator,
                auto_drain,
                drain_state_dir,
            }))
        }
        "command" => {
            let argv: Vec<String> = declaration
                .deliverer
                .get("argv")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            Ok(Box::new(CommandDeliverer { argv }))
        }
        other => Err(crate::jsonfile::DataError::new(
            "declaration",
            format!("unknown deliverer kind {other:?}"),
        )),
    }
}
