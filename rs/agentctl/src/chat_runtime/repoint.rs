//! Repointing a bridge at another agent without restarting it (#677 chat-bridge-repoint).
//!
//! `chat repoint` records a request in `target.json` under the exclusive state lock and wakes the
//! running service. The service applies it between two owner loop passes, when no prompt is being
//! typed: it validates the new agent, settles every request whose prompt has not reached the old
//! agent (taking queued prompts back out of the old agent's queue so they move to the new one, and
//! marking a prompt that may already have reached the old agent as uncertain rather than typing it
//! twice), then rewrites `agent_name` in `bridge.json`, so a later restart keeps the new target,
//! and records the outcome next to the request. With no service running, the command applies the
//! request itself while it holds the runner lease. A request is applied once: its generation is
//! recorded with its outcome, and a newer request supersedes an older one still waiting.

use super::*;

/// The document in the state directory holding the latest repoint request and recent outcomes.
pub(crate) const TARGET_DOCUMENT: &str = "target.json";
/// Outcomes `target.json` keeps, newest last.
const MAX_REPOINT_HISTORY: usize = 16;
/// Request keys one outcome lists in each of its lists.
const MAX_LISTED_KEYS: usize = 256;
const MAX_TARGET_DOCUMENT_BYTES: usize = 1 << 20;
const MAX_REQUESTED_BY_BYTES: usize = 256;

/// Checks that an agent can be the bridge's target and gives the pane it is in, or why not: see
/// [`BridgeState::apply_repoint`].
pub(crate) type ValidateTarget<'a> =
    dyn Fn(&str, Option<&str>) -> std::result::Result<Option<String>, String> + 'a;

/// One request to point the bridge at another agent, as `chat repoint` records it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepointRequest {
    /// Increases by one with each request; an outcome names the request it settled.
    pub generation: u64,
    /// The registered agent the bridge should type into.
    pub agent_name: String,
    /// The pane the agent must be in, when the requester named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    /// The agent the bridge served when the request was made.
    pub previous_agent: String,
    /// Who asked: an agent name, or `harness:PID@host`, `human@host` or `unknown@host`.
    pub requested_by: String,
    /// Unix milliseconds.
    pub requested_at_millis: u64,
}

/// One request whose prompt may already have reached the previous agent, so it is not typed into
/// the new one: an operator settles it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UncertainRequest {
    /// The request key.
    pub key: String,
    /// Why its prompt is not typed into the new agent.
    pub detail: String,
}

/// What applying one [`RepointRequest`] did.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepointOutcome {
    /// The generation of the request this settled.
    pub generation: u64,
    /// The agent the request named.
    pub agent_name: String,
    /// The pane that agent was found in when the request was applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    /// The agent the bridge served before.
    pub previous_agent: String,
    /// Who asked, as the request records it.
    pub requested_by: String,
    /// When the request was made, in Unix milliseconds.
    pub requested_at_millis: u64,
    /// When it was applied, in Unix milliseconds.
    pub applied_at_millis: u64,
    /// `service` when the running `chat run` applied it, `command` when `chat repoint` did
    /// because no service was running.
    pub applied_by: String,
    /// Why the bridge kept its target, when the request was refused; nothing moved then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// Requests whose prompts had not reached the previous agent and now go to the new one,
    /// including prompts taken back out of the previous agent's queue.
    #[serde(default)]
    pub moved: Vec<String>,
    /// Requests whose prompts the previous agent's queue had already typed when the repoint was
    /// applied; they are recorded as delivered and stay with that agent.
    #[serde(default)]
    pub delivered_to_previous: Vec<String>,
    /// Requests delivered to the previous agent that have no reply yet. The previous agent can
    /// still answer them with `agentctl chat reply`; the bridge no longer reads its screen.
    #[serde(default)]
    pub awaiting_reply_from_previous: Vec<String>,
    /// Requests whose prompts may have reached the previous agent, and those already uncertain.
    #[serde(default)]
    pub uncertain: Vec<UncertainRequest>,
}

impl RepointOutcome {
    /// Whether the bridge's target changed.
    #[must_use]
    pub fn changed_target(&self) -> bool {
        self.refused.is_none() && self.agent_name != self.previous_agent
    }

    /// One line for the service log, and the body of the channel notice.
    #[must_use]
    pub fn summary(&self) -> String {
        if let Some(reason) = &self.refused {
            return format!(
                "[chat-bridge] repoint to {} refused, still typing into {}: {reason} (asked by {})",
                self.agent_name, self.previous_agent, self.requested_by
            );
        }
        let pane = self
            .pane_id
            .as_deref()
            .map(|pane| format!(" (pane {pane})"))
            .unwrap_or_default();
        let mut text = if self.agent_name == self.previous_agent {
            format!(
                "[chat-bridge] repoint by {}: already typing into {}{pane}",
                self.requested_by, self.agent_name
            )
        } else {
            format!(
                "[chat-bridge] now typing into {}{pane}, was {}; repointed by {}",
                self.agent_name, self.previous_agent, self.requested_by
            )
        };
        let mut parts = Vec::new();
        if !self.moved.is_empty() {
            parts.push(format!("{} waiting request(s) moved", self.moved.len()));
        }
        if !self.awaiting_reply_from_previous.is_empty() {
            parts.push(format!(
                "{} delivered to {} still unanswered",
                self.awaiting_reply_from_previous.len(),
                self.previous_agent
            ));
        }
        if !self.uncertain.is_empty() {
            parts.push(format!(
                "{} uncertain, not retyped (see agentctl chat health)",
                self.uncertain.len()
            ));
        }
        if !parts.is_empty() {
            text.push_str(". ");
            text.push_str(&parts.join("; "));
        }
        text
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetDocument {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request: Option<RepointRequest>,
    #[serde(default)]
    applied: Vec<RepointOutcome>,
}

impl TargetDocument {
    fn latest_generation(&self) -> u64 {
        let requested = self
            .request
            .as_ref()
            .map_or(0, |request| request.generation);
        let applied = self.applied.last().map_or(0, |outcome| outcome.generation);
        requested.max(applied)
    }

    fn pending(&self) -> Option<&RepointRequest> {
        let applied = self.applied.last().map_or(0, |outcome| outcome.generation);
        self.request
            .as_ref()
            .filter(|request| request.generation > applied)
    }
}

fn validate_requested_by(requested_by: &str) -> Result<()> {
    validate_single_line(requested_by, "repoint requester", MAX_REQUESTED_BY_BYTES)
}

fn listed(mut keys: Vec<String>) -> Vec<String> {
    keys.truncate(MAX_LISTED_KEYS);
    keys
}

impl BridgeState {
    fn target_path(&self) -> PathBuf {
        self.root.join(TARGET_DOCUMENT)
    }

    /// Caller holds the shared or exclusive state lock. Absent, no repoint was ever requested.
    fn read_target_locked(&self) -> Result<TargetDocument> {
        match read_document::<TargetDocument>(&self.target_path(), MAX_TARGET_DOCUMENT_BYTES) {
            Ok(document) if document.version == STATE_VERSION => Ok(document),
            Ok(document) => Err(ChatRuntimeError::invalid(format!(
                "{TARGET_DOCUMENT} has unsupported version {}",
                document.version
            ))),
            Err(ChatRuntimeError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                Ok(TargetDocument {
                    version: STATE_VERSION,
                    ..TargetDocument::default()
                })
            }
            Err(error) => Err(error),
        }
    }

    /// Record a request to point the bridge at `agent_name`, superseding any request still
    /// waiting. The caller validated the agent; the applier validates it again.
    pub fn request_repoint(
        &self,
        agent_name: &str,
        pane_id: Option<&str>,
        requested_by: &str,
    ) -> Result<RepointRequest> {
        validate_slug(agent_name, "agent name", MAX_AGENT_NAME_BYTES)?;
        if let Some(pane) = pane_id {
            validate_single_line(pane, "pane ID", 256)?;
        }
        validate_requested_by(requested_by)?;
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        FileExt::lock_exclusive(&state_lock).map_err(ChatRuntimeError::Io)?;
        let envelope = self.read_grant_configuration_locked()?;
        let mut document = self.read_target_locked()?;
        let request = RepointRequest {
            generation: document.latest_generation().saturating_add(1),
            agent_name: agent_name.to_owned(),
            pane_id: pane_id.map(str::to_owned),
            previous_agent: envelope.config.agent_name,
            requested_by: requested_by.to_owned(),
            requested_at_millis: unix_millis(),
        };
        document.request = Some(request.clone());
        write_document(&self.target_path(), &document)?;
        Ok(request)
    }

    /// The repoint request still waiting to be applied, if any.
    pub fn pending_repoint(&self) -> Result<Option<RepointRequest>> {
        if fs::symlink_metadata(self.target_path()).is_err() {
            return Ok(None);
        }
        let _snapshot = self.lock_state_snapshot()?;
        Ok(self.read_target_locked()?.pending().cloned())
    }

    /// The outcome recorded for request `generation`, once it has been applied or superseded by
    /// a newer applied request.
    pub fn repoint_outcome(&self, generation: u64) -> Result<Option<RepointOutcome>> {
        let _snapshot = self.lock_state_snapshot()?;
        let document = self.read_target_locked()?;
        Ok(document
            .applied
            .iter()
            .find(|outcome| outcome.generation == generation)
            .or_else(|| {
                document
                    .applied
                    .iter()
                    .find(|outcome| outcome.generation > generation)
            })
            .cloned())
    }

    /// `chat status`'s `repoint`: the request still waiting and the latest outcome, each
    /// `null` when there is none, or `unreadable` with the error. Caller holds the state lock.
    pub(super) fn repoint_status_locked(&self) -> Value {
        match self.read_target_locked() {
            Ok(document) => json!({
                "pending": document.pending(),
                "last": document.applied.last(),
            }),
            Err(error) => json!({ "unreadable": error.to_string() }),
        }
    }

    /// The recent repoint outcomes, newest last.
    pub fn repoint_history(&self) -> Result<Vec<RepointOutcome>> {
        let _snapshot = self.lock_state_snapshot()?;
        Ok(self.read_target_locked()?.applied)
    }

    /// When the bridge last changed its target, if it ever did: a request typed before then went
    /// to the previous agent, and the bridge no longer reads that agent's screen.
    pub(crate) fn retargeted_at_millis(&self) -> Option<u64> {
        if fs::symlink_metadata(self.target_path()).is_err() {
            return None;
        }
        let _snapshot = self.lock_state_snapshot().ok()?;
        self.read_target_locked()
            .ok()?
            .applied
            .iter()
            .rev()
            .find(|outcome| outcome.changed_target())
            .map(|outcome| outcome.applied_at_millis)
    }

    /// This state, serving `agent_name` instead of the agent it was opened with. Process-local
    /// settings are kept.
    #[must_use]
    pub fn retargeted(&self, agent_name: &str) -> Self {
        let mut state = self.clone();
        state.config.agent_name = agent_name.to_owned();
        state
    }

    /// Apply `request` to this state, whose agent is the previous one. `validate` checks the new
    /// agent (registered, pane alive, harness detected, and in `request.pane_id` when given) and
    /// gives the pane it is in, or why it cannot be the target. Only one applier may run: the
    /// service's owner loop between passes, or a command holding the runner lease. Returns the
    /// recorded outcome; the caller retargets the in-memory state when it changed the target.
    pub(crate) fn apply_repoint(
        &self,
        request: &RepointRequest,
        delivery: &dyn CoordinatorDelivery,
        validate: &ValidateTarget<'_>,
        applied_by: &str,
    ) -> Result<RepointOutcome> {
        let previous = self.config.agent_name.clone();
        let mut outcome = RepointOutcome {
            generation: request.generation,
            agent_name: request.agent_name.clone(),
            pane_id: request.pane_id.clone(),
            previous_agent: previous.clone(),
            requested_by: request.requested_by.clone(),
            requested_at_millis: request.requested_at_millis,
            applied_at_millis: 0,
            applied_by: applied_by.to_owned(),
            refused: None,
            moved: Vec::new(),
            delivered_to_previous: Vec::new(),
            awaiting_reply_from_previous: Vec::new(),
            uncertain: Vec::new(),
        };
        match validate(&request.agent_name, request.pane_id.as_deref()) {
            Ok(pane) => outcome.pane_id = pane.or(outcome.pane_id),
            Err(reason) => {
                outcome.refused = Some(reason);
                return self.record_repoint(outcome);
            }
        }
        if request.agent_name != previous {
            self.move_waiting_requests(&previous, &request.agent_name, delivery, &mut outcome)?;
        }
        // What stays with the previous agent, and what was already uncertain, for the report.
        let records = {
            let _snapshot = self.lock_state_snapshot()?;
            self.request_records()?
        };
        let mut awaiting = Vec::new();
        for (record, _) in records {
            match record.phase {
                RequestPhase::Delivered
                    if request.agent_name != previous
                        && record.reply_count == 0
                        && !record.reply_closed =>
                {
                    awaiting.push(record.key);
                }
                RequestPhase::DeliveryUncertain
                    if !outcome
                        .uncertain
                        .iter()
                        .any(|entry| entry.key == record.key) =>
                {
                    outcome.uncertain.push(UncertainRequest {
                        key: record.key,
                        detail: record
                            .delivery_error
                            .unwrap_or_else(|| "delivery is uncertain".to_owned()),
                    });
                }
                _ => {}
            }
        }
        outcome.awaiting_reply_from_previous = listed(awaiting);
        outcome.uncertain.truncate(MAX_LISTED_KEYS);
        self.record_repoint(outcome)
    }

    /// Settle each request whose prompt the previous agent has not been recorded as typing, so
    /// that none is typed into both agents and none is lost. The previous agent's queue decides:
    /// a prompt it never held, or set aside untyped, moves; one waiting in its inbox is taken back
    /// out and moves; one it typed is recorded as delivered; any other state, or a queue that
    /// cannot be read, leaves the request uncertain for an operator.
    fn move_waiting_requests(
        &self,
        previous: &str,
        next: &str,
        delivery: &dyn CoordinatorDelivery,
        outcome: &mut RepointOutcome,
    ) -> Result<()> {
        let waiting: Vec<RequestRecord> = {
            let _snapshot = self.lock_state_snapshot()?;
            self.request_records()?
                .into_iter()
                .map(|(record, _)| record)
                .filter(|record| {
                    matches!(
                        record.phase,
                        RequestPhase::Pending | RequestPhase::Submitting
                    )
                })
                .collect()
        };
        let moved_detail = format!("moved from {previous} to {next} by a repoint");
        for record in waiting {
            let key = record.key.clone();
            let message_id = &record.delivery_message_id;
            // A request whose delivery never started was never queued anywhere: it moves without
            // asking the previous agent's queue, which may be gone with its registration.
            let observed = if record.delivery_started_at_millis.is_none() {
                Ok(None)
            } else {
                delivery.message_state(previous, message_id)
            };
            let settled = match observed {
                Ok(None | Some(QueueMessageState::Cancelled)) => Settled::Moved,
                Ok(Some(QueueMessageState::Processed)) => Settled::Typed,
                Ok(Some(QueueMessageState::Pending)) => {
                    match delivery.withdraw(previous, message_id) {
                        Ok(true) => Settled::Moved,
                        withdrawn => match delivery.message_state(previous, message_id) {
                            Ok(Some(QueueMessageState::Processed)) => Settled::Typed,
                            Ok(None | Some(QueueMessageState::Cancelled)) => Settled::Moved,
                            state => Settled::Uncertain(format!(
                            "repointed from {previous} to {next} while its prompt was queued for \
                             {previous}, and it could not be taken back ({}; queue now {}); it may \
                             reach {previous}, so it is not typed into {next}",
                            match withdrawn {
                                Ok(_) => "the queue had begun to type it".to_owned(),
                                Err(error) => error,
                            },
                            queue_state_text(&state)
                        )),
                        },
                    }
                }
                Ok(Some(QueueMessageState::Inflight | QueueMessageState::Failed)) => {
                    Settled::Uncertain(format!(
                        "repointed from {previous} to {next} while {previous}'s queue was typing \
                         its prompt; it may have reached {previous}, so it is not typed into \
                         {next}"
                    ))
                }
                Err(error) => Settled::Uncertain(format!(
                    "repointed from {previous} to {next}, but {previous}'s queue could not be \
                     read ({error}); its prompt may be queued there, so it is not typed into \
                     {next}"
                )),
            };
            let result = match &settled {
                Settled::Moved => {
                    self.set_waiting_phase(&key, RequestPhase::Pending, Some(&moved_detail))
                }
                Settled::Typed => self.note_started(delivery, message_id).set_waiting_phase(
                    &key,
                    RequestPhase::Delivered,
                    None,
                ),
                Settled::Uncertain(detail) => {
                    self.set_waiting_phase(&key, RequestPhase::DeliveryUncertain, Some(detail))
                }
            };
            match result {
                Ok(true) => match settled {
                    Settled::Moved => outcome.moved.push(key),
                    Settled::Typed => outcome.delivered_to_previous.push(key),
                    Settled::Uncertain(detail) => {
                        outcome.uncertain.push(UncertainRequest { key, detail });
                    }
                },
                // Retired or settled meanwhile.
                Ok(false) => {}
                Err(error) => return Err(error),
            }
        }
        outcome.moved = listed(std::mem::take(&mut outcome.moved));
        outcome.delivered_to_previous = listed(std::mem::take(&mut outcome.delivered_to_previous));
        Ok(())
    }

    /// Set `key`'s phase if its prompt still waits (`pending` or `submitting`); `false` when it
    /// no longer does or the request was retired.
    fn set_waiting_phase(
        &self,
        key: &str,
        phase: RequestPhase,
        detail: Option<&str>,
    ) -> Result<bool> {
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        state_lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
        let record = match self.read_request(key) {
            Ok(record) => record,
            Err(ChatRuntimeError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        if !matches!(
            record.phase,
            RequestPhase::Pending | RequestPhase::Submitting
        ) {
            return Ok(false);
        }
        self.set_delivery_phase_locked(record, phase, detail)?;
        Ok(true)
    }

    /// Save `outcome` and, when it changes the target, `agent_name` in `bridge.json`, under one
    /// exclusive state lock. A request newer than the outcome's stays pending.
    fn record_repoint(&self, mut outcome: RepointOutcome) -> Result<RepointOutcome> {
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        FileExt::lock_exclusive(&state_lock).map_err(ChatRuntimeError::Io)?;
        outcome.applied_at_millis = unix_millis();
        if outcome.changed_target() {
            let mut envelope = self.read_grant_configuration_locked()?;
            envelope.config.agent_name = outcome.agent_name.clone();
            envelope.config.validate()?;
            write_document(&self.root.join("bridge.json"), &envelope)?;
        }
        let mut document = self.read_target_locked()?;
        document.applied.push(outcome.clone());
        let excess = document.applied.len().saturating_sub(MAX_REPOINT_HISTORY);
        document.applied.drain(..excess);
        write_document(&self.target_path(), &document)?;
        Ok(outcome)
    }
}

enum Settled {
    Moved,
    Typed,
    Uncertain(String),
}

fn queue_state_text(state: &std::result::Result<Option<QueueMessageState>, String>) -> String {
    match state {
        Ok(None) => "absent".to_owned(),
        Ok(Some(QueueMessageState::Pending)) => "pending".to_owned(),
        Ok(Some(QueueMessageState::Inflight)) => "inflight".to_owned(),
        Ok(Some(QueueMessageState::Processed)) => "processed".to_owned(),
        Ok(Some(QueueMessageState::Failed)) => "failed".to_owned(),
        Ok(Some(QueueMessageState::Cancelled)) => "cancelled".to_owned(),
        Err(error) => format!("unreadable: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    use chat_subscription::{
        ChannelId, CommittableEvent, DeliveryBatch, DeliveryId, EventSequence, InboundMessage,
        MessageId, ProviderCursor, SenderId, ThreadId,
    };

    use super::*;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
        state: BridgeState,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn fixture(name: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "agentctl-repoint-{name}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let configuration = serde_json::from_value(json!({
            "subscription_plugin": "fixture",
            "channel_ids": ["spaces/example"],
            "allowed_senders": ["users/owner"],
            "agent_name": "old-agent",
            "agent_label": "coordinator",
            "outbound_enabled": true,
            "ack_reaction": null
        }))
        .expect("configuration");
        let state = BridgeState::initialize(&root, configuration).expect("initialize state");
        Fixture { root, state }
    }

    fn admit(state: &BridgeState, index: u64) -> String {
        let message = InboundMessage::new(
            ChannelId::new("spaces/example").expect("channel"),
            MessageId::new(format!("spaces/example/messages/{index}")).expect("message"),
            ThreadId::new(format!("spaces/example/threads/{index}")).expect("thread"),
            SenderId::new("users/owner").expect("sender"),
            format!("request {index}"),
            "2026-10-14T12:00:00Z",
            false,
        )
        .expect("message");
        let batch = DeliveryBatch::new(
            EventSequence::new(index).expect("sequence"),
            ProviderCursor::new(format!("cursor-{index}")).expect("cursor"),
            DeliveryId::new(format!("delivery-{index}")).expect("delivery"),
            vec![CommittableEvent::message_created(message)],
        )
        .expect("batch");
        state
            .admit_batch(&batch)
            .expect("admit")
            .new_request_keys
            .remove(0)
    }

    /// Per-agent queues: each `(agent, message ID)` has its own state. A prompt in an inbox is
    /// taken back out unless `stuck` names it.
    #[derive(Default)]
    struct Queues {
        states: Mutex<BTreeMap<(String, String), QueueMessageState>>,
        stuck: Mutex<BTreeSet<String>>,
        unreadable: Mutex<BTreeSet<String>>,
        asked: Mutex<Vec<String>>,
    }

    impl Queues {
        fn set(&self, agent: &str, key: &str, state: QueueMessageState) {
            self.states
                .lock()
                .expect("queue lock")
                .insert((agent.to_owned(), format!("chat-{key}")), state);
        }
    }

    impl CoordinatorDelivery for Queues {
        fn message_state(
            &self,
            agent_name: &str,
            message_id: &str,
        ) -> std::result::Result<Option<QueueMessageState>, String> {
            self.asked
                .lock()
                .expect("asked lock")
                .push(agent_name.to_owned());
            if self.unreadable.lock().expect("lock").contains(message_id) {
                return Err("queue artifact cannot be read".to_owned());
            }
            Ok(self
                .states
                .lock()
                .expect("queue lock")
                .get(&(agent_name.to_owned(), message_id.to_owned()))
                .copied())
        }

        fn submit(
            &self,
            _agent_name: &str,
            _prompt: &str,
            _message_id: &str,
            _options: DrainOptions,
        ) -> std::result::Result<(), String> {
            panic!("a repoint never types a prompt");
        }

        fn drain(
            &self,
            _agent_name: &str,
            _options: DrainOptions,
        ) -> std::result::Result<(), String> {
            panic!("a repoint never drains a queue");
        }

        fn screen(&self, _agent_name: &str) -> std::result::Result<String, String> {
            panic!("a repoint never reads a pane");
        }

        fn withdraw(
            &self,
            agent_name: &str,
            message_id: &str,
        ) -> std::result::Result<bool, String> {
            if self.stuck.lock().expect("lock").contains(message_id) {
                return Ok(false);
            }
            let mut states = self.states.lock().expect("queue lock");
            let entry = states.get_mut(&(agent_name.to_owned(), message_id.to_owned()));
            match entry {
                Some(state) if *state == QueueMessageState::Pending => {
                    *state = QueueMessageState::Cancelled;
                    Ok(true)
                }
                _ => Ok(false),
            }
        }
    }

    fn accept(_agent: &str, pane: Option<&str>) -> std::result::Result<Option<String>, String> {
        Ok(Some(pane.unwrap_or("w1:p2").to_owned()))
    }

    fn phase(state: &BridgeState, key: &str) -> (RequestPhase, Option<String>) {
        let record = state.read_request(key).expect("request");
        (record.phase, record.delivery_error)
    }

    #[test]
    fn a_repoint_moves_waiting_requests_and_survives_a_reopen() {
        let fixture = fixture("moves");
        let state = &fixture.state;
        let never_queued = admit(state, 1);
        let queued = admit(state, 2);
        let typed = admit(state, 3);
        let set_aside = admit(state, 4);
        for key in [&queued, &typed, &set_aside] {
            state
                .set_delivery_phase(key, RequestPhase::Submitting, None)
                .expect("submitting");
        }
        let queues = Queues::default();
        queues.set("old-agent", &queued, QueueMessageState::Pending);
        queues.set("old-agent", &typed, QueueMessageState::Processed);
        queues.set("old-agent", &set_aside, QueueMessageState::Cancelled);

        assert_eq!(state.pending_repoint().expect("pending"), None);
        let request = state
            .request_repoint("new-agent", None, "cl-test")
            .expect("request");
        assert_eq!(request.generation, 1);
        assert_eq!(request.previous_agent, "old-agent");
        assert_eq!(
            state.pending_repoint().expect("pending"),
            Some(request.clone())
        );

        let outcome = state
            .apply_repoint(&request, &queues, &accept, "service")
            .expect("apply");
        assert!(outcome.changed_target());
        assert_eq!(outcome.pane_id.as_deref(), Some("w1:p2"));
        let mut moved = outcome.moved.clone();
        moved.sort();
        let mut expected = vec![never_queued.clone(), queued.clone(), set_aside.clone()];
        expected.sort();
        assert_eq!(moved, expected);
        assert_eq!(outcome.delivered_to_previous, vec![typed.clone()]);
        assert!(outcome.uncertain.is_empty());
        // Only the previous agent's queue was consulted.
        assert!(queues
            .asked
            .lock()
            .expect("lock")
            .iter()
            .all(|agent| agent == "old-agent"));
        // The queued prompt was taken back out of the previous agent's queue.
        assert_eq!(
            queues.message_state("old-agent", &format!("chat-{queued}")),
            Ok(Some(QueueMessageState::Cancelled))
        );
        for key in [&never_queued, &queued, &set_aside] {
            let (phase, detail) = phase(state, key);
            assert_eq!(phase, RequestPhase::Pending);
            assert!(detail
                .expect("detail")
                .contains("moved from old-agent to new-agent"));
        }
        assert_eq!(phase(state, &typed).0, RequestPhase::Delivered);
        assert_eq!(state.pending_repoint().expect("pending"), None);
        assert!(outcome
            .summary()
            .contains("now typing into new-agent (pane w1:p2), was old-agent"));
        assert!(outcome.summary().contains("repointed by cl-test"));
        assert!(outcome.summary().contains("3 waiting request(s) moved"));

        // The new target is durable: a reopened state serves it, and the outcome is recorded.
        let reopened = BridgeState::inspect(&fixture.root).expect("reopen");
        assert_eq!(reopened.config().agent_name, "new-agent");
        assert_eq!(
            reopened.repoint_outcome(1).expect("outcome"),
            Some(outcome.clone())
        );
        assert_eq!(reopened.repoint_history().expect("history"), vec![outcome]);
        assert_eq!(
            state.retargeted("new-agent").config().agent_name,
            reopened.config().agent_name
        );
    }

    #[test]
    fn a_prompt_that_may_have_reached_the_previous_agent_is_uncertain_not_moved() {
        let fixture = fixture("uncertain");
        let state = &fixture.state;
        let stuck = admit(state, 1);
        let inflight = admit(state, 2);
        let unreadable = admit(state, 3);
        let earlier = admit(state, 4);
        for key in [&stuck, &inflight, &unreadable] {
            state
                .set_delivery_phase(key, RequestPhase::Submitting, None)
                .expect("submitting");
        }
        state
            .set_delivery_phase(
                &earlier,
                RequestPhase::DeliveryUncertain,
                Some("typed, then lost"),
            )
            .expect("uncertain before the repoint");
        let queues = Queues::default();
        queues.set("old-agent", &stuck, QueueMessageState::Pending);
        queues
            .stuck
            .lock()
            .expect("lock")
            .insert(format!("chat-{stuck}"));
        queues.set("old-agent", &inflight, QueueMessageState::Inflight);
        queues
            .unreadable
            .lock()
            .expect("lock")
            .insert(format!("chat-{unreadable}"));
        let request = state
            .request_repoint("new-agent", None, "cl-test")
            .expect("request");
        let outcome = state
            .apply_repoint(&request, &queues, &accept, "service")
            .expect("apply");
        assert!(outcome.moved.is_empty());
        let uncertain: BTreeMap<_, _> = outcome
            .uncertain
            .iter()
            .map(|entry| (entry.key.clone(), entry.detail.clone()))
            .collect();
        assert_eq!(uncertain.len(), 4, "{uncertain:?}");
        assert!(uncertain[&stuck].contains("could not be taken back"));
        assert!(uncertain[&inflight].contains("was typing its prompt"));
        assert!(uncertain[&unreadable].contains("could not be read"));
        assert_eq!(uncertain[&earlier], "typed, then lost");
        for key in [&stuck, &inflight, &unreadable] {
            assert_eq!(phase(state, key).0, RequestPhase::DeliveryUncertain);
        }
        assert!(outcome.summary().contains("4 uncertain, not retyped"));
    }

    #[test]
    fn a_refused_target_changes_nothing_and_settles_the_request() {
        let fixture = fixture("refused");
        let state = &fixture.state;
        let waiting = admit(state, 1);
        let request = state
            .request_repoint("gone", None, "cl-test")
            .expect("request");
        let refuse = |agent: &str, _pane: Option<&str>| {
            Err::<Option<String>, String>(format!("{agent}'s pane is not alive"))
        };
        let outcome = state
            .apply_repoint(&request, &Queues::default(), &refuse, "command")
            .expect("apply");
        assert_eq!(outcome.refused.as_deref(), Some("gone's pane is not alive"));
        assert!(!outcome.changed_target());
        assert_eq!(phase(state, &waiting), (RequestPhase::Pending, None));
        assert_eq!(
            BridgeState::inspect(&fixture.root)
                .expect("reopen")
                .config()
                .agent_name,
            "old-agent"
        );
        assert_eq!(state.pending_repoint().expect("pending"), None);
        assert_eq!(state.retargeted_at_millis(), None);
        assert!(outcome
            .summary()
            .contains("refused, still typing into old-agent"));
    }

    #[test]
    fn a_newer_request_supersedes_one_not_yet_applied() {
        let fixture = fixture("supersede");
        let state = &fixture.state;
        let first = state
            .request_repoint("agent-a", None, "one")
            .expect("first");
        let second = state
            .request_repoint("agent-b", Some("w2:p1"), "two")
            .expect("second");
        assert_eq!((first.generation, second.generation), (1, 2));
        let pending = state
            .pending_repoint()
            .expect("pending")
            .expect("a request");
        assert_eq!(pending, second);
        let outcome = state
            .apply_repoint(&pending, &Queues::default(), &accept, "service")
            .expect("apply");
        assert_eq!(outcome.agent_name, "agent-b");
        assert_eq!(outcome.pane_id.as_deref(), Some("w2:p1"));
        // The first request's outcome is the one that superseded it.
        assert_eq!(
            state
                .repoint_outcome(first.generation)
                .expect("outcome")
                .map(|o| o.generation),
            Some(2)
        );
        let third = state
            .request_repoint("agent-c", None, "three")
            .expect("third");
        assert_eq!(third.generation, 3);
        assert_eq!(third.previous_agent, "agent-b");
    }

    #[test]
    fn requests_typed_before_a_repoint_are_left_to_the_previous_agent() {
        let fixture = fixture("awaiting");
        let state = &fixture.state;
        let answered_elsewhere = admit(state, 1);
        state
            .set_delivery_phase(&answered_elsewhere, RequestPhase::Delivered, None)
            .expect("delivered");
        assert_eq!(
            state
                .reply_reminder_candidates()
                .expect("candidates")
                .candidates
                .len(),
            1
        );
        let request = state
            .request_repoint("new-agent", None, "cl-test")
            .expect("request");
        let outcome = state
            .apply_repoint(&request, &Queues::default(), &accept, "service")
            .expect("apply");
        assert_eq!(
            outcome.awaiting_reply_from_previous,
            vec![answered_elsewhere]
        );
        assert!(outcome
            .summary()
            .contains("1 delivered to old-agent still unanswered"));
        assert!(state.retargeted_at_millis().is_some());
        // The new agent is never reminded of a request it did not receive.
        assert!(state
            .reply_reminder_candidates()
            .expect("candidates")
            .candidates
            .is_empty());
    }

    #[test]
    fn a_request_never_queued_moves_even_when_the_previous_queue_is_gone() {
        let fixture = fixture("gone");
        let state = &fixture.state;
        let never_started = admit(state, 1);
        let started = admit(state, 2);
        state
            .set_delivery_phase(&started, RequestPhase::Submitting, None)
            .expect("submitting");
        let queues = Queues::default();
        // The previous agent's registration is gone: its queue cannot be read at all.
        queues
            .unreadable
            .lock()
            .expect("lock")
            .extend([format!("chat-{never_started}"), format!("chat-{started}")]);
        let request = state
            .request_repoint("new-agent", None, "cl-test")
            .expect("request");
        let outcome = state
            .apply_repoint(&request, &queues, &accept, "command")
            .expect("apply");
        assert_eq!(outcome.moved, vec![never_started.clone()]);
        assert_eq!(phase(state, &never_started).0, RequestPhase::Pending);
        // One whose prompt may be queued there cannot be proved untyped.
        assert_eq!(
            outcome
                .uncertain
                .iter()
                .map(|entry| entry.key.clone())
                .collect::<Vec<_>>(),
            vec![started]
        );
    }

    #[test]
    fn each_request_reports_its_own_outcome() {
        let fixture = fixture("own");
        let state = &fixture.state;
        for (agent, generation) in [("agent-a", 1), ("agent-b", 2)] {
            let request = state
                .request_repoint(agent, None, "cl-test")
                .expect("request");
            assert_eq!(request.generation, generation);
            state
                .apply_repoint(&request, &Queues::default(), &accept, "service")
                .expect("apply");
        }
        let first = state.repoint_outcome(1).expect("outcome").expect("first");
        assert_eq!(
            (first.generation, first.agent_name.as_str()),
            (1, "agent-a")
        );
        assert_eq!(state.repoint_outcome(3).expect("outcome"), None);
    }

    #[test]
    fn the_history_is_bounded() {
        let fixture = fixture("history");
        let state = &fixture.state;
        for index in 0..(MAX_REPOINT_HISTORY + 3) {
            let request = state
                .request_repoint(&format!("agent-{index}"), None, "cl-test")
                .expect("request");
            state
                .apply_repoint(&request, &Queues::default(), &accept, "service")
                .expect("apply");
        }
        let history = state.repoint_history().expect("history");
        assert_eq!(history.len(), MAX_REPOINT_HISTORY);
        assert_eq!(
            history.last().map(|outcome| outcome.agent_name.as_str()),
            Some(format!("agent-{}", MAX_REPOINT_HISTORY + 2).as_str())
        );
    }
}
