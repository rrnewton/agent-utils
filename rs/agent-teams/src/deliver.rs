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

/// The `agentctl-queue` deliverer: enqueues into the designated
/// coordinator's queue through the shared `agent-queue` crate; that host's
/// drainers type it. The enqueue is unserialized (`serialize: false`):
/// `accept` must never block on the queue's delivery lock (design 9.2),
/// and the ledger claim that precedes every hand-over is what makes this
/// ID single-writer, so the crate's no-replace create is the exact
/// duplicate guard. Team items join the crate's `inbox.v2/` lane, with
/// drain-side `not_after`/tombstone re-checks, when phase P2 lands them
/// in `agent-queue`; until then expiry and supersede are enforced at
/// `accept` and in the ledger.
pub struct AgentctlQueueDeliverer {
    /// The coordinator's queue directory (`REGISTRY/<agent>/queue`).
    pub queue_dir: PathBuf,
    /// Ledger entry path recorded in each item.
    pub ledger_path: PathBuf,
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
        let result = agent_queue::queue::enqueue(
            &self.queue_dir,
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
            Err(e) => match self.contains(id) {
                Some(true) => Handover::Accepted,
                Some(false) => Handover::Refused(format!("queue enqueue failed: {}", e.message())),
                None => Handover::PossiblySubmitted(format!(
                    "queue enqueue failed and presence is unprovable: {}",
                    e.message()
                )),
            },
        }
    }
    fn contains(&self, id: &str) -> Option<bool> {
        use agent_queue::queue::MessageState;
        match agent_queue::queue::message_state(&self.queue_dir, id) {
            Ok(Some(MessageState::Pending | MessageState::Inflight | MessageState::Processed)) => {
                Some(true)
            }
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
            let coordinator =
                crate::proof::registry_coordinator_name(&registry).ok_or_else(|| {
                    crate::jsonfile::DataError::new(
                        "no-coordinator",
                        format!(
                            "registry {} has no coordinator designation",
                            registry.display()
                        ),
                    )
                })?;
            Ok(Box::new(AgentctlQueueDeliverer {
                queue_dir: registry.join(coordinator).join("queue"),
                ledger_path: ledger_path.to_path_buf(),
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
