//! Bounded, durable feedback for messages the bridge did not admit.

use super::*;

const REJECTION_SCHEMA: &str = "agentctl-chat-rejection-reactions/v1";
const REJECTION_FILE: &str = "rejection-reactions.json";
const MAX_REJECTION_REACTIONS: usize = 256;
const MAX_REJECTION_DOCUMENT_BYTES: usize = 8 * 1_024 * 1_024;
const MAX_REJECTION_ERROR_BYTES: usize = 2_000;

/// Best-effort feedback saved while admitting a batch. None of these failures prevents its
/// cursor from advancing or creates a request for the rejected message.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RejectionSaves {
    /// Durable pending reaction keys, including a pending operation found on replay.
    pub saved: Vec<String>,
    /// New reactions omitted because every retained operation is still pending.
    pub dropped: usize,
    /// Bounded persistence diagnostics; a failed save may have reached disk before reporting
    /// its error, so it is reported separately from a definite capacity drop.
    pub errors: Vec<String>,
}

/// An identity that admission classified as rejected. It carries no text or provider payload,
/// and is never authority to create a coordinator request.
#[derive(Clone, Debug)]
pub(super) struct RejectionCandidate {
    channel_id: String,
    message_id: String,
    sender_id: String,
}

impl RejectionCandidate {
    pub(super) fn new(channel_id: &str, message_id: &str, sender_id: &str) -> Self {
        Self {
            channel_id: channel_id.to_owned(),
            message_id: message_id.to_owned(),
            sender_id: sender_id.to_owned(),
        }
    }
}

fn rejection_request_key(channel_id: &str, message_id: &str) -> Result<String> {
    let identity = serde_json::to_vec(&json!({
        "channel_id": channel_id,
        "message_id": message_id,
    }))?;
    Ok(format!("{:x}", Sha256::digest(identity)))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RejectionPhase {
    Pending,
    Complete,
    Refused,
    Suppressed,
}

/// Successful, finally refused and suppressed operations stay as bounded tombstones. Once one
/// is evicted, replay still reconstructs the same operation UUID, rather than inventing a
/// second operation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RejectionRecord {
    key: String,
    channel_id: String,
    message_id: String,
    sender_id: String,
    emoji: String,
    request_id: String,
    phase: RejectionPhase,
    recorded_at_millis: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attempted_at_millis: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    already_present: Option<bool>,
}

impl RejectionRecord {
    fn validate(&self, provider: &str) -> Result<()> {
        ChannelId::new(self.channel_id.clone())
            .map_err(|error| ChatRuntimeError::invalid(error.to_string()))?;
        chat_subscription::MessageId::new(self.message_id.clone())
            .map_err(|error| ChatRuntimeError::invalid(error.to_string()))?;
        SenderId::new(self.sender_id.clone())
            .map_err(|error| ChatRuntimeError::invalid(error.to_string()))?;
        validate_single_line(&self.emoji, "rejection reaction", 64)?;
        let (key, request_id) = rejection_identity(provider, &self.channel_id, &self.message_id);
        if self.key != key
            || self.request_id != request_id
            || !valid_operation_uuid(&self.request_id)
            || self
                .attempted_at_millis
                .is_some_and(|at| at < self.recorded_at_millis)
            || self
                .error
                .as_ref()
                .is_some_and(|error| error.len() > MAX_REJECTION_ERROR_BYTES)
            || self.error.is_some() && self.attempted_at_millis.is_none()
        {
            return Err(ChatRuntimeError::invalid(
                "saved rejection reaction is inconsistent",
            ));
        }
        let receipt_present = self.reaction_id.is_some() && self.already_present.is_some();
        if self.reaction_id.is_some() != self.already_present.is_some()
            || match self.phase {
                RejectionPhase::Pending => receipt_present,
                RejectionPhase::Complete => {
                    !receipt_present || self.attempted_at_millis.is_none() || self.error.is_some()
                }
                RejectionPhase::Refused => {
                    receipt_present || self.attempted_at_millis.is_none() || self.error.is_none()
                }
                RejectionPhase::Suppressed => receipt_present || self.error.is_some(),
            }
        {
            return Err(ChatRuntimeError::invalid(
                "saved rejection reaction outcome is inconsistent",
            ));
        }
        if let Some(reaction_id) = self.reaction_id.as_deref() {
            validate_single_line(
                reaction_id,
                "provider rejection reaction id",
                chat_subscription::MAX_RESOURCE_ID_BYTES,
            )?;
        }
        Ok(())
    }

    /// Cancel further attempts without inventing a helper outcome. A saved attempt time, if
    /// present, still records a real earlier attempt; a reaction already in flight cannot be
    /// recalled by suppressing its pending record.
    fn suppress(&mut self) {
        self.phase = RejectionPhase::Suppressed;
        self.error = None;
        self.reaction_id = None;
        self.already_present = None;
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RejectionDocument {
    schema: String,
    provider: String,
    records: Vec<RejectionRecord>,
}

impl RejectionDocument {
    fn empty(provider: &str) -> Self {
        Self {
            schema: REJECTION_SCHEMA.to_owned(),
            provider: provider.to_owned(),
            records: Vec::new(),
        }
    }

    fn validate(&self, config: &BridgeConfiguration) -> Result<()> {
        if self.schema != REJECTION_SCHEMA
            || self.provider != config.subscription_plugin
            || self.records.len() > MAX_REJECTION_REACTIONS
        {
            return Err(ChatRuntimeError::invalid(
                "saved rejection reactions exceed their bounds or provider authority",
            ));
        }
        let mut keys = BTreeSet::new();
        for record in &self.records {
            record.validate(&self.provider)?;
            if !config.channel_ids.contains(&record.channel_id) || !keys.insert(record.key.as_str())
            {
                return Err(ChatRuntimeError::invalid(
                    "saved rejection reactions repeat an identity or name an unconfigured channel",
                ));
            }
        }
        if encoded_document_bytes(self)? > MAX_REJECTION_DOCUMENT_BYTES {
            return Err(ChatRuntimeError::invalid(
                "saved rejection reactions exceed their byte bound",
            ));
        }
        Ok(())
    }
}

/// The protocol accepts v4-shaped UUID idempotency keys. A domain-separated digest fills their
/// payload bits here so the operation survives both a restart and bounded tombstone eviction.
/// The UUID is an operation identity, not a random secret. The emoji is deliberately excluded:
/// a configuration change must not invent a second operation for the same provider message.
fn rejection_identity(provider: &str, channel_id: &str, message_id: &str) -> (String, String) {
    let mut digest = Sha256::new();
    digest.update(b"agentctl.chat.rejection-reaction.v1\0");
    for part in [provider, channel_id, message_id] {
        digest.update(u64::try_from(part.len()).unwrap_or(u64::MAX).to_be_bytes());
        digest.update(part.as_bytes());
    }
    let digest = digest.finalize();
    let key = format!("{digest:x}");
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let request_id = format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    );
    (key, request_id)
}

impl BridgeState {
    /// Check request identity under the caller's state lock. Any current request path protects
    /// the message, including an unreadable or unexpected file type; retirement protects its
    /// identity after the active request is removed.
    fn rejection_message_is_admitted_locked(
        &self,
        channel_id: &str,
        message_id: &str,
        admitted_request_keys: &[String],
    ) -> Result<bool> {
        let request_key = rejection_request_key(channel_id, message_id)?;
        if admitted_request_keys.contains(&request_key) {
            return Ok(true);
        }
        match fs::symlink_metadata(self.request_path(&request_key)) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(ChatRuntimeError::Io(error)),
        }
        Ok(self.retired_key(&request_key)?.is_some())
    }

    /// Read the bounded rejection outbox. The caller holds the state lock. Its absence means
    /// no rejection feedback has been saved.
    fn read_rejection_reactions_locked(&self) -> Result<RejectionDocument> {
        let document: RejectionDocument = match read_document(
            &self.root.join(REJECTION_FILE),
            MAX_REJECTION_DOCUMENT_BYTES,
        ) {
            Ok(document) => document,
            Err(ChatRuntimeError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                RejectionDocument::empty(&self.config.subscription_plugin)
            }
            Err(error) => return Err(error),
        };
        document.validate(&self.config)?;
        Ok(document)
    }

    /// Save independently from request admission, before its cursor is published. A crash may
    /// leave feedback for an uncommitted batch, which its replay safely reuses. An unsuccessful
    /// best-effort write is reported without blocking the batch. The caller holds .state.lock.
    pub(super) fn save_rejection_reactions_locked(
        &self,
        candidates: &[RejectionCandidate],
        admitted_request_keys: &[String],
    ) -> RejectionSaves {
        let mut saves = RejectionSaves::default();
        let Some(emoji) = self.config.reject_emoji() else {
            return saves;
        };
        if candidates.is_empty() {
            return saves;
        }
        let mut document = match self.read_rejection_reactions_locked() {
            Ok(document) => document,
            Err(error) => {
                saves.errors.push(bounded_detail(
                    &format!("cannot read rejection reactions: {error}"),
                    MAX_REJECTION_ERROR_BYTES,
                ));
                return saves;
            }
        };
        let mut changed = false;
        let mut seen = BTreeSet::new();
        for candidate in candidates {
            // Feedback is never authority to expand the bridge's configured channels.
            if !self.config.channel_ids.contains(&candidate.channel_id) {
                continue;
            }
            let (key, request_id) = rejection_identity(
                &self.config.subscription_plugin,
                &candidate.channel_id,
                &candidate.message_id,
            );
            if !seen.insert(key.clone()) {
                continue;
            }
            let protected = self.rejection_message_is_admitted_locked(
                &candidate.channel_id,
                &candidate.message_id,
                admitted_request_keys,
            );
            match protected {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => {
                    saves.errors.push(bounded_detail(
                        &format!(
                            "cannot exclude an admitted message from rejection feedback: {error}"
                        ),
                        MAX_REJECTION_ERROR_BYTES,
                    ));
                    continue;
                }
            }
            if let Some(existing) = document.records.iter().find(|record| record.key == key) {
                if existing.sender_id != candidate.sender_id {
                    saves.errors.push(format!(
                        "rejected message {key} changed its authenticated sender; no reaction was queued"
                    ));
                } else if existing.phase == RejectionPhase::Pending {
                    saves.saved.push(key);
                }
                continue;
            }
            let record = RejectionRecord {
                key: key.clone(),
                channel_id: candidate.channel_id.clone(),
                message_id: candidate.message_id.clone(),
                sender_id: candidate.sender_id.clone(),
                emoji: emoji.to_owned(),
                request_id,
                phase: RejectionPhase::Pending,
                recorded_at_millis: unix_millis(),
                attempted_at_millis: None,
                error: None,
                reaction_id: None,
                already_present: None,
            };
            if let Err(error) = record.validate(&self.config.subscription_plugin) {
                saves.errors.push(bounded_detail(
                    &format!("cannot save rejection reaction: {error}"),
                    MAX_REJECTION_ERROR_BYTES,
                ));
                continue;
            }
            if document.records.len() == MAX_REJECTION_REACTIONS {
                let terminal = document
                    .records
                    .iter()
                    .position(|record| record.phase != RejectionPhase::Pending);
                match terminal {
                    Some(index) => {
                        document.records.remove(index);
                    }
                    None => {
                        saves.dropped += 1;
                        continue;
                    }
                }
            }
            document.records.push(record);
            changed = true;
            saves.saved.push(key);
        }
        if changed {
            let written = document
                .validate(&self.config)
                .and_then(|()| write_document(&self.root.join(REJECTION_FILE), &document));
            if let Err(error) = written {
                // The previous outbox remains readable if an atomic write fails before rename;
                // a failure after rename is uncertain and the worker's next scan reconciles it.
                saves.saved.clear();
                saves.errors.push(bounded_detail(
                    &format!("cannot persist rejection reactions: {error}"),
                    MAX_REJECTION_ERROR_BYTES,
                ));
            }
        }
        saves
    }

    /// Cancel pending feedback only after the admission transaction has durably installed its
    /// request records and cursor. This runs even with feedback disabled and no new rejection
    /// candidates. Failures affect feedback only; the dispatch-time guard also protects a
    /// request if this best-effort cancellation is interrupted or cannot be saved.
    pub(super) fn suppress_admitted_rejections_locked(
        &self,
        admitted_request_keys: &[String],
    ) -> Vec<String> {
        if admitted_request_keys.is_empty() {
            return Vec::new();
        }
        let mut document = match self.read_rejection_reactions_locked() {
            Ok(document) => document,
            Err(error) => {
                return vec![bounded_detail(
                    &format!("cannot read rejection reactions: {error}"),
                    MAX_REJECTION_ERROR_BYTES,
                )];
            }
        };
        let suppressed = (|| -> Result<()> {
            let mut changed = false;
            for record in document
                .records
                .iter_mut()
                .filter(|record| record.phase == RejectionPhase::Pending)
            {
                let key = rejection_request_key(&record.channel_id, &record.message_id)?;
                if admitted_request_keys.contains(&key) {
                    record.suppress();
                    changed = true;
                }
            }
            if changed {
                document.validate(&self.config)?;
                write_document(&self.root.join(REJECTION_FILE), &document)?;
            }
            Ok(())
        })();
        match suppressed {
            Ok(()) => Vec::new(),
            Err(error) => vec![bounded_detail(
                &format!("cannot cancel an admitted message's rejection feedback: {error}"),
                MAX_REJECTION_ERROR_BYTES,
            )],
        }
    }

    /// Pending rejection operations, least recently attempted first, with the saved attempt
    /// time used by the service's bounded retry worker. Terminal rows remain as replay
    /// tombstones and are omitted.
    pub(crate) fn rejection_reaction_attempts(&self) -> Result<Vec<(String, Option<u64>)>> {
        if self.config.reject_emoji().is_none() {
            return Ok(Vec::new());
        }
        let _snapshot = self.lock_state_snapshot()?;
        let document = self.read_rejection_reactions_locked()?;
        let mut pending = document
            .records
            .into_iter()
            .filter(|record| record.phase == RejectionPhase::Pending)
            .map(|record| (record.key, record.attempted_at_millis))
            .collect::<Vec<_>>();
        pending.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
        Ok(pending)
    }

    /// Ensure feedback through the dedicated transport path, which verifies that the exact
    /// provider author account is human. No general reaction transport is used as
    /// a fallback. Persist the attempt before transport, and keep the same UUID on uncertainty.
    pub(crate) fn ensure_rejection_reaction(
        &self,
        key: &str,
        transport: &mut dyn ReactionTransport,
    ) -> Result<Option<ReactionReceipt>> {
        if self.config.reject_emoji().is_none() {
            return Ok(None);
        }
        if !valid_key(key) {
            return Err(ChatRuntimeError::invalid(
                "chat rejection key must be 64 lowercase hexadecimal characters",
            ));
        }
        let snapshot = {
            let lock = agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
            lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
            let mut document = self.read_rejection_reactions_locked()?;
            let Some(record) = document
                .records
                .iter_mut()
                .find(|record| record.key == key && record.phase == RejectionPhase::Pending)
            else {
                return Ok(None);
            };
            if self.rejection_message_is_admitted_locked(
                &record.channel_id,
                &record.message_id,
                &[],
            )? {
                record.suppress();
                document.validate(&self.config)?;
                write_document(&self.root.join(REJECTION_FILE), &document)?;
                return Ok(None);
            }
            record.attempted_at_millis = Some(unix_millis().max(record.recorded_at_millis));
            let snapshot = record.clone();
            document.validate(&self.config)?;
            write_document(&self.root.join(REJECTION_FILE), &document)?;
            snapshot
        };
        let ensured = transport.ensure_rejection(RejectionSubmission {
            channel_id: &snapshot.channel_id,
            message_id: &snapshot.message_id,
            sender_id: &snapshot.sender_id,
            emoji: &snapshot.emoji,
            request_id: &snapshot.request_id,
        });
        let outcome = match ensured {
            Ok(receipt) => match validate_single_line(
                &receipt.reaction_id,
                "provider rejection reaction id",
                chat_subscription::MAX_RESOURCE_ID_BYTES,
            ) {
                Ok(()) => Ok(receipt),
                Err(error) => Err(OutboundFailure::unusable_receipt(error.to_string())),
            },
            Err(error) => Err(error),
        };
        let lock = agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
        let mut document = self.read_rejection_reactions_locked()?;
        if let Some(record) = document.records.iter_mut().find(|record| {
            record.key == key
                && record.request_id == snapshot.request_id
                && record.phase == RejectionPhase::Pending
        }) {
            match &outcome {
                Ok(receipt) => {
                    record.phase = RejectionPhase::Complete;
                    record.error = None;
                    record.reaction_id = Some(receipt.reaction_id.clone());
                    record.already_present = Some(receipt.already_present);
                }
                Err(error) => {
                    record.error = Some(bounded_detail(
                        &error.to_string(),
                        MAX_REJECTION_ERROR_BYTES,
                    ));
                    if error.outcome == OutboundOutcome::NotApplied && !error.retryable {
                        record.phase = RejectionPhase::Refused;
                    }
                }
            }
            document.validate(&self.config)?;
            write_document(&self.root.join(REJECTION_FILE), &document)?;
        }
        match outcome {
            Ok(receipt) => Ok(Some(receipt)),
            Err(error) => Err(ChatRuntimeError::invalid(format!(
                "outbound rejection reaction failed: {error}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use chat_subscription::{
        EventSequence, InboundMessage, MessageId, RejectedMessage, RejectionReason, ThreadId,
    };

    use super::*;

    struct Fixture {
        root: PathBuf,
        state: BridgeState,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "agentctl-rejection-{name}-{}-{}",
                std::process::id(),
                random_operation_uuid().expect("fixture identity")
            ));
            let config: BridgeConfiguration = serde_json::from_value(json!({
                "subscription_plugin": "fixture",
                "channel_ids": ["spaces/example"],
                "allowed_senders": ["users/owner"],
                "agent_name": "coordinator",
                "agent_label": "bridge coordinator",
                "ack_reaction": null,
                "receipt_reaction": null,
            }))
            .expect("configuration");
            let state = BridgeState::initialize(&root, config).expect("initialize");
            Self { root, state }
        }

        fn reopen(&self) -> BridgeState {
            BridgeState::open(&self.root).expect("reopen")
        }

        fn document(&self) -> RejectionDocument {
            let _lock = self.state.lock_state_snapshot().expect("snapshot lock");
            self.state
                .read_rejection_reactions_locked()
                .expect("rejection outbox")
        }

        fn set_reaction(&self, setting: ReactionSetting) {
            let lock =
                agent::open_private_lock(&self.root.join(".state.lock"), "fixture state lock")
                    .expect("open lock");
            lock.lock_exclusive().expect("lock");
            let mut config: ConfigurationEnvelope =
                read_document(&self.root.join("bridge.json"), 1 << 20).expect("configuration");
            config.config.reject_reaction = setting;
            config
                .config
                .validate()
                .expect("valid changed configuration");
            write_document(&self.root.join("bridge.json"), &config).expect("save configuration");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn message(index: u64) -> String {
        format!("spaces/example/messages/{index}")
    }

    fn rejected(index: u64, sender: &str, reason: RejectionReason) -> CommittableEvent {
        CommittableEvent::message_rejected(RejectedMessage::new(
            ChannelId::new("spaces/example").expect("channel"),
            MessageId::new(message(index)).expect("message"),
            SenderId::new(sender).expect("sender"),
            reason,
        ))
    }

    fn created(index: u64, sender: &str, text: &str) -> CommittableEvent {
        CommittableEvent::message_created(
            InboundMessage::new(
                ChannelId::new("spaces/example").expect("channel"),
                MessageId::new(message(index)).expect("message"),
                ThreadId::new(format!("spaces/example/threads/{index}")).expect("thread"),
                SenderId::new(sender).expect("sender"),
                text,
                "2026-10-20T12:00:00Z",
                false,
            )
            .expect("normalized message"),
        )
    }

    fn batch(sequence: u64, cursor: &str, events: Vec<CommittableEvent>) -> DeliveryBatch {
        DeliveryBatch::new(
            EventSequence::new(sequence).expect("sequence"),
            ProviderCursor::new(cursor).expect("cursor"),
            DeliveryId::new(format!("delivery-{sequence}")).expect("delivery"),
            events,
        )
        .expect("delivery batch")
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct SentRejection {
        request_id: String,
        channel_id: String,
        message_id: String,
        sender_id: String,
        emoji: String,
    }

    #[derive(Default)]
    struct Transport {
        calls: Vec<SentRejection>,
        applied: BTreeSet<(String, String, String)>,
        outcomes: VecDeque<std::result::Result<(), OutboundFailure>>,
        panic_after_applying_once: bool,
    }

    impl ReactionTransport for Transport {
        fn ensure_reaction(
            &mut self,
            _submission: ReactionSubmission<'_>,
        ) -> std::result::Result<ReactionReceipt, OutboundFailure> {
            panic!("rejected messages must never use the ordinary reaction path");
        }

        fn ensure_rejection(
            &mut self,
            submission: RejectionSubmission<'_>,
        ) -> std::result::Result<ReactionReceipt, OutboundFailure> {
            self.calls.push(SentRejection {
                request_id: submission.request_id.to_owned(),
                channel_id: submission.channel_id.to_owned(),
                message_id: submission.message_id.to_owned(),
                sender_id: submission.sender_id.to_owned(),
                emoji: submission.emoji.to_owned(),
            });
            if let Some(outcome) = self.outcomes.pop_front() {
                outcome?;
            }
            let already_present = !self.applied.insert((
                submission.channel_id.to_owned(),
                submission.message_id.to_owned(),
                submission.emoji.to_owned(),
            ));
            if std::mem::take(&mut self.panic_after_applying_once) {
                panic!("crash after the provider applied the reaction");
            }
            Ok(ReactionReceipt {
                reaction_id: format!("{}/reactions/rejected", submission.message_id),
                already_present,
            })
        }
    }

    #[test]
    fn rejected_human_identity_saves_feedback_without_creating_a_request() {
        let fixture = Fixture::new("unallowed");
        let admission = fixture
            .state
            .admit_batch(&batch(
                1,
                "cursor-1",
                vec![rejected(
                    1,
                    "users/guest",
                    RejectionReason::SenderNotAllowed,
                )],
            ))
            .expect("admission advances despite rejection");
        assert!(admission.new_request_keys.is_empty());
        assert_eq!(admission.rejection_saves.saved.len(), 1);
        assert_eq!(admission.rejection_saves.dropped, 0);
        assert!(admission.rejection_saves.errors.is_empty());
        let checkpoint = fixture.state.read_checkpoint().expect("checkpoint");
        assert_eq!(checkpoint.cursor.as_deref(), Some("cursor-1"));
        assert_eq!(checkpoint.request_count, 0);
        assert!(fixture
            .state
            .request_paths()
            .expect("request files")
            .is_empty());

        let key = &admission.rejection_saves.saved[0];
        let mut transport = Transport::default();
        fixture
            .reopen()
            .ensure_rejection_reaction(key, &mut transport)
            .expect("rejection succeeds")
            .expect("one reaction");
        assert_eq!(transport.calls.len(), 1);
        assert_eq!(transport.calls[0].channel_id, "spaces/example");
        assert_eq!(transport.calls[0].message_id, message(1));
        assert_eq!(transport.calls[0].sender_id, "users/guest");
        assert_eq!(transport.calls[0].emoji, "❌");
        assert!(valid_operation_uuid(&transport.calls[0].request_id));
        assert_eq!(
            fixture.document().records[0].phase,
            RejectionPhase::Complete
        );
        let metadata = fs::metadata(fixture.root.join(REJECTION_FILE)).expect("private outbox");
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(metadata.nlink(), 1);
    }

    #[test]
    fn expiry_and_revocation_of_a_live_provider_sender_save_rejection_feedback() {
        for expired in [false, true] {
            let fixture = Fixture::new(if expired { "expired" } else { "revoked" });
            fixture
                .state
                .grant_senders(&["users/guest".to_owned()], Duration::from_secs(60), None)
                .expect("grant");
            if expired {
                let lock = agent::open_private_lock(
                    &fixture.root.join(".state.lock"),
                    "fixture state lock",
                )
                .expect("lock");
                lock.lock_exclusive().expect("exclusive");
                let path = fixture.root.join("grants.json");
                let mut grants: Value = read_document(&path, 1 << 20).expect("saved grant");
                let now = unix_millis();
                grants["grants"][0]["created_at_millis"] = json!(now - 2_000);
                grants["grants"][0]["expires_at_millis"] = json!(now - 1_000);
                write_document(&path, &grants).expect("expired grant");
            } else {
                fixture
                    .state
                    .revoke_senders(&["users/guest".to_owned()])
                    .expect("revoke");
            }
            let admission = fixture
                .state
                .admit_batch(&batch(
                    1,
                    "cursor-1",
                    vec![created(1, "users/guest", "not admitted")],
                ))
                .expect("a stale provider sender cannot block the cursor");
            assert!(admission.new_request_keys.is_empty());
            assert_eq!(admission.rejection_saves.saved.len(), 1);
            assert!(admission.rejection_saves.errors.is_empty());
            assert_eq!(
                fixture
                    .state
                    .read_checkpoint()
                    .expect("checkpoint")
                    .request_count,
                0
            );
        }
    }

    #[test]
    fn a_new_grant_or_permanent_owner_does_not_get_a_stale_sender_rejection() {
        let fixture = Fixture::new("allowed");
        fixture
            .state
            .grant_senders(&["users/guest".to_owned()], Duration::from_secs(60), None)
            .expect("grant");
        let admission = fixture
            .state
            .admit_batch(&batch(
                1,
                "cursor-1",
                vec![
                    rejected(1, "users/guest", RejectionReason::SenderNotAllowed),
                    rejected(2, "users/owner", RejectionReason::SenderNotAllowed),
                ],
            ))
            .expect("stale denial is not rejection authority");
        assert!(admission.new_request_keys.is_empty());
        assert!(admission.rejection_saves.saved.is_empty());
        assert!(!fixture.root.join(REJECTION_FILE).exists());
    }

    #[test]
    fn ignored_prefixes_get_feedback_without_admission_from_either_event_path() {
        let fixture = Fixture::new("ignored");
        let state = fixture
            .state
            .clone()
            .with_ignored_text_prefixes(vec!["[bridge]".to_owned()])
            .expect("ignore policy");
        let admission = state
            .admit_batch(&batch(
                1,
                "cursor-1",
                vec![
                    created(1, "users/owner", "[bridge] notification"),
                    rejected(2, "users/owner", RejectionReason::IgnoredPrefix),
                ],
            ))
            .expect("ignored messages advance the cursor");
        assert!(admission.new_request_keys.is_empty());
        assert_eq!(admission.rejection_saves.saved.len(), 2);
        assert!(state.request_paths().expect("request files").is_empty());
        assert_eq!(fixture.document().records.len(), 2);
    }

    #[test]
    fn previously_admitted_and_retired_messages_never_get_rejection_feedback() {
        let fixture = Fixture::new("prior-admission");
        fixture
            .state
            .grant_senders(&["users/guest".to_owned()], Duration::from_secs(60), None)
            .expect("grant");
        let admitted = fixture
            .state
            .admit_batch(&batch(
                1,
                "cursor-1",
                vec![created(1, "users/guest", "accepted before revocation")],
            ))
            .expect("first request");
        fixture
            .state
            .confirm_batch_commit(&admitted)
            .expect("provider commit");
        let key = admitted.new_request_keys.first().expect("request");
        fixture
            .state
            .revoke_senders(&["users/guest".to_owned()])
            .expect("revoke");
        let replay = fixture
            .state
            .admit_batch(&batch(
                2,
                "cursor-2",
                vec![rejected(
                    1,
                    "users/guest",
                    RejectionReason::SenderNotAllowed,
                )],
            ))
            .expect("prior request remains admitted");
        assert!(replay.rejection_saves.saved.is_empty());
        fixture
            .state
            .set_delivery_phase(key, RequestPhase::Delivered, None)
            .expect("record confirmed delivery");
        fixture.state.close_replies(key).expect("close and retire");
        assert!(fixture
            .state
            .retired_key(key)
            .expect("retired identity")
            .is_some());
        let retired_replay = fixture
            .state
            .admit_batch(&batch(
                3,
                "cursor-3",
                vec![rejected(
                    1,
                    "users/guest",
                    RejectionReason::SenderNotAllowed,
                )],
            ))
            .expect("retired request stays protected");
        assert!(retired_replay.rejection_saves.saved.is_empty());
        assert!(retired_replay.rejection_saves.errors.is_empty());
        assert!(!fixture.root.join(REJECTION_FILE).exists());
    }

    #[test]
    fn later_admission_suppresses_pending_rejection_without_new_candidates_even_when_disabled() {
        for (ignored_prefix, disabled) in [(false, false), (true, false), (false, true)] {
            let fixture = Fixture::new("later-admission");
            let sender = if ignored_prefix {
                "users/owner"
            } else {
                "users/guest"
            };
            let source = created(1, sender, "[ignore] the same provider message");
            let initial = if ignored_prefix {
                fixture
                    .state
                    .clone()
                    .with_ignored_text_prefixes(vec!["[ignore]".to_owned()])
                    .expect("initial ignore policy")
            } else {
                fixture.state.clone()
            };
            let first_event = if ignored_prefix {
                source.clone()
            } else {
                rejected(1, sender, RejectionReason::SenderNotAllowed)
            };
            let first = initial
                .admit_batch(&batch(1, "cursor-1", vec![first_event]))
                .expect("save initial rejection");
            initial
                .confirm_batch_commit(&first)
                .expect("commit rejection cursor");
            assert_eq!(first.rejection_saves.saved.len(), 1);
            let rejection_key = &first.rejection_saves.saved[0];
            if !ignored_prefix {
                fixture
                    .state
                    .grant_senders(&[sender.to_owned()], Duration::from_secs(60), None)
                    .expect("grant the formerly unallowed sender");
            }
            if disabled {
                fixture.set_reaction(ReactionSetting::Disabled);
            }
            let later = fixture.reopen();
            let admitted = later
                .admit_batch(&batch(2, "cursor-2", vec![source]))
                .expect("admit the same message after policy changes");
            later
                .confirm_batch_commit(&admitted)
                .expect("commit request cursor");
            assert_eq!(admitted.new_request_keys.len(), 1);
            assert!(admitted.rejection_saves.saved.is_empty());
            assert!(admitted.rejection_saves.errors.is_empty());
            let document = fixture.document();
            assert_eq!(document.records.len(), 1);
            assert_eq!(document.records[0].phase, RejectionPhase::Suppressed,
                "request admission must cancel pending feedback without another rejection candidate");
            assert!(document.records[0].attempted_at_millis.is_none());
            assert!(document.records[0].error.is_none());
            assert!(document.records[0].reaction_id.is_none());
            assert!(later
                .rejection_reaction_attempts()
                .expect("suppressed feedback")
                .is_empty());
            let request_path = later.request_path(&admitted.new_request_keys[0]);
            let request_before = fs::read(&request_path).expect("accepted request");
            let mut transport = Transport::default();
            assert!(later
                .ensure_rejection_reaction(rejection_key, &mut transport)
                .expect("suppressed feedback is not sent")
                .is_none());
            assert!(
                transport.calls.is_empty(),
                "an admitted message receives no rejection attempt"
            );
            assert_eq!(
                fs::read(request_path).expect("accepted request preserved"),
                request_before
            );
            assert_eq!(later.cursor().expect("cursor").as_deref(), Some("cursor-2"));
        }
    }

    fn assert_current_or_retired_request_suppresses_restored_pending_feedback(retired: bool) {
        let fixture = Fixture::new(if retired {
            "restored-retired"
        } else {
            "restored-current"
        });
        let first = fixture
            .state
            .admit_batch(&batch(
                1,
                "cursor-1",
                vec![rejected(
                    1,
                    "users/guest",
                    RejectionReason::SenderNotAllowed,
                )],
            ))
            .expect("save pending rejection");
        fixture
            .state
            .confirm_batch_commit(&first)
            .expect("commit rejection cursor");
        let rejection_key = &first.rejection_saves.saved[0];
        let pending = fixture.document();
        assert_eq!(pending.records[0].phase, RejectionPhase::Pending);
        fixture
            .state
            .grant_senders(&["users/guest".to_owned()], Duration::from_secs(60), None)
            .expect("grant sender");
        let admitted = fixture
            .state
            .admit_batch(&batch(
                2,
                "cursor-2",
                vec![created(1, "users/guest", "accepted after the grant")],
            ))
            .expect("admit the same provider identity");
        fixture
            .state
            .confirm_batch_commit(&admitted)
            .expect("commit request cursor");
        assert_eq!(admitted.new_request_keys.len(), 1);
        let request_key = &admitted.new_request_keys[0];
        if retired {
            fixture
                .state
                .set_delivery_phase(request_key, RequestPhase::Delivered, None)
                .expect("verified delivery");
            fixture
                .state
                .close_replies(request_key)
                .expect("retire accepted request");
            assert!(fixture
                .state
                .retired_key(request_key)
                .expect("retired identity")
                .is_some());
        }
        let protected_before = if retired {
            serde_json::to_vec(
                &fixture
                    .state
                    .retired_key(request_key)
                    .expect("retired request"),
            )
            .expect("retired identity bytes")
        } else {
            fs::read(fixture.state.request_path(request_key)).expect("current request bytes")
        };
        {
            // Model a failed best-effort cancellation, or interruption after request admission:
            // the accepted request is durable while its earlier feedback is still pending.
            let lock =
                agent::open_private_lock(&fixture.root.join(".state.lock"), "fixture state lock")
                    .expect("open state lock");
            lock.lock_exclusive().expect("lock state");
            write_document(&fixture.root.join(REJECTION_FILE), &pending)
                .expect("restore only the earlier pending outbox snapshot");
        }
        let reopened = fixture.reopen();
        assert_eq!(
            reopened
                .rejection_reaction_attempts()
                .expect("restored pending work")
                .len(),
            1
        );
        let mut transport = Transport::default();
        let feedback = reopened
            .ensure_rejection_reaction(rejection_key, &mut transport)
            .expect("accepted message suppresses restored pending feedback");
        assert!(
            feedback.is_none(),
            "the snapshot guard must suppress an admitted message before transport"
        );
        assert!(
            transport.calls.is_empty(),
            "the snapshot guard must run before any helper call"
        );
        assert!(reopened
            .rejection_reaction_attempts()
            .expect("suppressed feedback")
            .is_empty());
        let document = fixture.document();
        assert_eq!(document.records[0].phase, RejectionPhase::Suppressed);
        assert!(
            document.records[0].attempted_at_millis.is_none(),
            "suppression is not a helper attempt"
        );
        let protected_after = if retired {
            serde_json::to_vec(
                &reopened
                    .retired_key(request_key)
                    .expect("retired request preserved"),
            )
            .expect("retired identity bytes")
        } else {
            fs::read(reopened.request_path(request_key)).expect("current request preserved")
        };
        assert_eq!(protected_after, protected_before);
    }

    #[test]
    fn the_helper_snapshot_suppresses_restored_pending_feedback_for_a_current_request() {
        assert_current_or_retired_request_suppresses_restored_pending_feedback(false);
    }

    #[test]
    fn the_helper_snapshot_suppresses_restored_pending_feedback_for_a_retired_request() {
        assert_current_or_retired_request_suppresses_restored_pending_feedback(true);
    }

    #[test]
    fn later_admission_preserves_an_earlier_completed_rejection_receipt() {
        let fixture = Fixture::new("completed-before-admission");
        let first = fixture
            .state
            .admit_batch(&batch(
                1,
                "cursor-1",
                vec![rejected(
                    1,
                    "users/guest",
                    RejectionReason::SenderNotAllowed,
                )],
            ))
            .expect("save rejection");
        fixture
            .state
            .confirm_batch_commit(&first)
            .expect("commit rejection cursor");
        let rejection_key = &first.rejection_saves.saved[0];
        let mut transport = Transport::default();
        fixture
            .state
            .ensure_rejection_reaction(rejection_key, &mut transport)
            .expect("feedback before later admission")
            .expect("receipt");
        let completed = serde_json::to_vec(&fixture.document()).expect("completed receipt bytes");
        fixture
            .state
            .grant_senders(&["users/guest".to_owned()], Duration::from_secs(60), None)
            .expect("grant sender");
        let admitted = fixture
            .state
            .admit_batch(&batch(
                2,
                "cursor-2",
                vec![created(
                    1,
                    "users/guest",
                    "accepted after completed feedback",
                )],
            ))
            .expect("later admission");
        assert_eq!(admitted.new_request_keys.len(), 1);
        assert!(admitted.rejection_saves.errors.is_empty());
        assert_eq!(
            serde_json::to_vec(&fixture.document()).expect("historical receipt"),
            completed
        );
        assert!(fixture
            .state
            .ensure_rejection_reaction(rejection_key, &mut transport)
            .expect("no new feedback")
            .is_none());
        assert_eq!(transport.calls.len(), 1);
    }

    #[test]
    fn an_interrupted_admission_keeps_prior_feedback_pending_before_request_installation() {
        let fixture = Fixture::new("interrupted-before-admission");
        let first = fixture
            .state
            .admit_batch(&batch(
                1,
                "cursor-1",
                vec![rejected(
                    1,
                    "users/guest",
                    RejectionReason::SenderNotAllowed,
                )],
            ))
            .expect("save rejection");
        fixture
            .state
            .confirm_batch_commit(&first)
            .expect("commit rejection cursor");
        let rejection_key = &first.rejection_saves.saved[0];
        let pending = serde_json::to_vec(&fixture.document()).expect("pending bytes");
        fixture
            .state
            .grant_senders(&["users/guest".to_owned()], Duration::from_secs(60), None)
            .expect("grant sender");
        *fixture
            .state
            .admission_fault_after
            .lock()
            .expect("admission fault fixture") = Some(0);
        let failure = fixture
            .state
            .admit_batch(&batch(
                2,
                "cursor-2",
                vec![created(
                    1,
                    "users/guest",
                    "admission interrupted before request installation",
                )],
            ))
            .expect_err("injected admission failure");
        assert!(failure
            .to_string()
            .contains("injected admission boundary failure"));
        assert!(fixture
            .state
            .request_paths()
            .expect("no installed request")
            .is_empty());
        assert_eq!(
            fixture.state.cursor().expect("unchanged cursor").as_deref(),
            Some("cursor-1")
        );
        assert_eq!(
            serde_json::to_vec(&fixture.document()).expect("prior feedback"),
            pending,
            "planned request keys must not suppress a valid rejection before admission succeeds"
        );
        let mut transport = Transport::default();
        assert!(fixture
            .state
            .ensure_rejection_reaction(rejection_key, &mut transport)
            .expect("the earlier rejection remains deliverable")
            .is_some());
        assert_eq!(transport.calls.len(), 1);
    }

    #[test]
    fn rejection_outbox_failure_does_not_prevent_cursor_commit_or_other_requests() {
        let fixture = Fixture::new("outbox-corrupt");
        write_document(&fixture.root.join(REJECTION_FILE), &json!({"broken": true}))
            .expect("corrupt outbox fixture");
        let admission = fixture
            .state
            .admit_batch(&batch(
                1,
                "cursor-1",
                vec![
                    rejected(1, "users/guest", RejectionReason::SenderNotAllowed),
                    created(2, "users/owner", "accepted alongside failed feedback"),
                ],
            ))
            .expect("feedback storage is best effort");
        assert_eq!(admission.new_request_keys.len(), 1);
        assert!(admission.rejection_saves.saved.is_empty());
        assert_eq!(admission.rejection_saves.errors.len(), 1);
        fixture
            .state
            .confirm_batch_commit(&admission)
            .expect("confirm cursor");
        let checkpoint = fixture.state.read_checkpoint().expect("checkpoint");
        assert_eq!(checkpoint.cursor.as_deref(), Some("cursor-1"));
        assert_eq!(checkpoint.request_count, 1);
        assert_eq!(
            fixture
                .state
                .current_commit_receipt(&checkpoint)
                .expect("receipt")
                .expect("committed")
                .phase,
            CommitReceiptPhase::Committed
        );
    }

    #[test]
    fn a_failed_rejection_reuses_its_operation_after_restart_and_replay() {
        let fixture = Fixture::new("retry");
        let event = rejected(1, "users/guest", RejectionReason::SenderNotAllowed);
        let admission = fixture
            .state
            .admit_batch(&batch(1, "cursor-1", vec![event.clone()]))
            .expect("admit rejection");
        let key = &admission.rejection_saves.saved[0];
        let mut transport = Transport {
            outcomes: VecDeque::from([Err(OutboundFailure {
                code: "provider_transport".to_owned(),
                detail: "outcome unknown".to_owned(),
                outcome: OutboundOutcome::Unknown,
                retryable: true,
            })]),
            ..Transport::default()
        };
        assert!(fixture
            .state
            .ensure_rejection_reaction(key, &mut transport)
            .is_err());
        let reopened = fixture.reopen();
        let waiting = reopened
            .rejection_reaction_attempts()
            .expect("saved pending work");
        assert_eq!(waiting.len(), 1);
        assert_eq!(&waiting[0].0, key);
        assert!(waiting[0].1.is_some());
        reopened
            .ensure_rejection_reaction(key, &mut transport)
            .expect("retry")
            .expect("one reaction");
        assert_eq!(transport.calls.len(), 2);
        assert_eq!(transport.calls[0], transport.calls[1]);
        let replay = reopened
            .admit_batch(&batch(2, "cursor-1", vec![event]))
            .expect("identical inclusive boundary replay");
        assert!(replay.rejection_saves.saved.is_empty());
        assert!(reopened
            .rejection_reaction_attempts()
            .expect("nothing pending")
            .is_empty());
        assert!(reopened
            .ensure_rejection_reaction(key, &mut transport)
            .expect("already completed")
            .is_none());
        assert_eq!(
            transport.calls.len(),
            2,
            "completed replay must not call the provider again"
        );
    }

    #[test]
    fn a_crash_after_the_provider_effect_keeps_one_stable_reaction_operation() {
        let fixture = Fixture::new("uncertain-effect");
        let admission = fixture
            .state
            .admit_batch(&batch(
                1,
                "cursor-1",
                vec![rejected(
                    1,
                    "users/guest",
                    RejectionReason::SenderNotAllowed,
                )],
            ))
            .expect("save feedback");
        let key = &admission.rejection_saves.saved[0];
        let mut transport = Transport {
            panic_after_applying_once: true,
            ..Transport::default()
        };
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fixture.state.ensure_rejection_reaction(key, &mut transport)
        }));
        assert!(interrupted.is_err());
        assert!(fixture.document().records[0].attempted_at_millis.is_some());
        let receipt = fixture
            .reopen()
            .ensure_rejection_reaction(key, &mut transport)
            .expect("reconcile after crash")
            .expect("receipt");
        assert!(receipt.already_present);
        assert_eq!(transport.calls.len(), 2);
        assert_eq!(transport.calls[0], transport.calls[1]);
        assert_eq!(
            transport.applied.len(),
            1,
            "retry must not invent a second operation"
        );
    }

    #[test]
    fn a_nonhuman_author_refusal_is_terminal_without_poisoning_provider_health() {
        let fixture = Fixture::new("nonhuman");
        let admission = fixture
            .state
            .admit_batch(&batch(
                1,
                "cursor-1",
                vec![rejected(
                    1,
                    "users/guest",
                    RejectionReason::SenderNotAllowed,
                )],
            ))
            .expect("save candidate");
        let key = &admission.rejection_saves.saved[0];
        let mut transport = Transport {
            outcomes: VecDeque::from([Err(OutboundFailure {
                code: "author_not_human".to_owned(),
                detail: "provider identifies this author as an app".to_owned(),
                outcome: OutboundOutcome::NotApplied,
                retryable: false,
            })]),
            ..Transport::default()
        };
        assert!(fixture
            .state
            .ensure_rejection_reaction(key, &mut transport)
            .is_err());
        let reopened = fixture.reopen();
        assert!(reopened
            .rejection_reaction_attempts()
            .expect("terminal refusal")
            .is_empty());
        assert!(reopened
            .ensure_rejection_reaction(key, &mut transport)
            .expect("not retried")
            .is_none());
        assert_eq!(transport.calls.len(), 1);
        assert_eq!(fixture.document().records[0].phase, RejectionPhase::Refused);
        assert!(!fixture.root.join("provider-health.json").exists());
    }

    #[test]
    fn disabling_rejections_leaves_saved_work_dormant_and_keeps_its_original_emoji() {
        let fixture = Fixture::new("disabled");
        let admission = fixture
            .state
            .admit_batch(&batch(
                1,
                "cursor-1",
                vec![rejected(
                    1,
                    "users/guest",
                    RejectionReason::SenderNotAllowed,
                )],
            ))
            .expect("save feedback");
        let key = &admission.rejection_saves.saved[0];
        fixture.set_reaction(ReactionSetting::Disabled);
        let disabled = fixture.reopen();
        assert!(disabled
            .rejection_reaction_attempts()
            .expect("disabled")
            .is_empty());
        let mut transport = Transport::default();
        assert!(disabled
            .ensure_rejection_reaction(key, &mut transport)
            .expect("disabled work")
            .is_none());
        assert!(transport.calls.is_empty());
        let admission = disabled
            .admit_batch(&batch(
                2,
                "cursor-2",
                vec![rejected(
                    2,
                    "users/guest",
                    RejectionReason::SenderNotAllowed,
                )],
            ))
            .expect("disabled rejection advances");
        assert!(admission.rejection_saves.saved.is_empty());
        assert_eq!(fixture.document().records.len(), 1);
        fixture.set_reaction(ReactionSetting::Emoji("🚫".to_owned()));
        fixture
            .reopen()
            .ensure_rejection_reaction(key, &mut transport)
            .expect("resume")
            .expect("receipt");
        assert_eq!(
            transport.calls[0].emoji, "❌",
            "pending feedback keeps its first saved emoji"
        );
    }

    #[test]
    fn a_full_pending_outbox_drops_only_feedback_and_eviction_reuses_stable_ids() {
        let fixture = Fixture::new("bounded");
        let events = (0..MAX_REJECTION_REACTIONS)
            .map(|index| {
                rejected(
                    index as u64,
                    "users/guest",
                    RejectionReason::SenderNotAllowed,
                )
            })
            .collect();
        let full = fixture
            .state
            .admit_batch(&batch(1, "cursor-1", events))
            .expect("fill outbox");
        assert_eq!(full.rejection_saves.saved.len(), MAX_REJECTION_REACTIONS);
        let first_key = full.rejection_saves.saved[0].clone();
        let overflow = fixture
            .state
            .admit_batch(&batch(
                2,
                "cursor-2",
                vec![rejected(
                    1_000,
                    "users/guest",
                    RejectionReason::SenderNotAllowed,
                )],
            ))
            .expect("full feedback does not stop the cursor");
        assert_eq!(overflow.rejection_saves.dropped, 1);
        assert!(overflow.new_request_keys.is_empty());
        assert_eq!(
            fixture
                .state
                .read_checkpoint()
                .expect("checkpoint")
                .cursor
                .as_deref(),
            Some("cursor-2")
        );
        let mut transport = Transport::default();
        fixture
            .state
            .ensure_rejection_reaction(&first_key, &mut transport)
            .expect("complete first")
            .expect("receipt");
        let replacement = fixture
            .state
            .admit_batch(&batch(
                3,
                "cursor-3",
                vec![rejected(
                    1_000,
                    "users/guest",
                    RejectionReason::SenderNotAllowed,
                )],
            ))
            .expect("evict completed tombstone");
        assert_eq!(replacement.rejection_saves.saved.len(), 1);
        assert!(!fixture
            .document()
            .records
            .iter()
            .any(|record| record.key == first_key));
        fixture
            .state
            .ensure_rejection_reaction(&replacement.rejection_saves.saved[0], &mut transport)
            .expect("complete replacement")
            .expect("receipt");
        let replay = fixture
            .reopen()
            .admit_batch(&batch(
                4,
                "cursor-4",
                vec![rejected(
                    0,
                    "users/guest",
                    RejectionReason::SenderNotAllowed,
                )],
            ))
            .expect("replay an evicted message");
        assert_eq!(
            replay.rejection_saves.saved.as_slice(),
            std::slice::from_ref(&first_key)
        );
        let receipt = fixture
            .reopen()
            .ensure_rejection_reaction(&first_key, &mut transport)
            .expect("reconcile evicted operation")
            .expect("receipt");
        assert!(receipt.already_present);
        assert_eq!(transport.calls[0], transport.calls[2]);
        assert_eq!(transport.applied.len(), 2);
        let document = fixture.document();
        assert_eq!(document.records.len(), MAX_REJECTION_REACTIONS);
        assert!(
            encoded_document_bytes(&document).expect("encoded bound")
                <= MAX_REJECTION_DOCUMENT_BYTES
        );
    }

    #[test]
    fn wrong_channels_and_nonmessage_checkpoints_create_no_rejection_work() {
        let fixture = Fixture::new("channel-authority");
        let outside = CommittableEvent::message_rejected(RejectedMessage::new(
            ChannelId::new("spaces/elsewhere").expect("channel"),
            MessageId::new("spaces/elsewhere/messages/one").expect("message"),
            SenderId::new("users/guest").expect("sender"),
            RejectionReason::SenderNotAllowed,
        ));
        let admission = fixture
            .state
            .admit_batch(&batch(
                1,
                "cursor-1",
                vec![outside, CommittableEvent::Checkpoint],
            ))
            .expect("ignored feedback candidate");
        assert!(admission.rejection_saves.saved.is_empty());
        assert!(admission.new_request_keys.is_empty());
        assert!(!fixture.root.join(REJECTION_FILE).exists());
    }
}
