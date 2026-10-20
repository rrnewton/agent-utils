use super::*;

use std::fs;
use std::num::NonZeroU16;
use std::sync::atomic::AtomicU64;

use chat_runtime::{
    OutboundOutcome, ReactionReceipt, ReactionSubmission, ReactionTransport, RejectionSubmission,
};
use chat_subscription::{
    CancellationError, ChannelId, CommittableEvent, DeliveryBatch, EventKind, EventSequence,
    InboundMessage, MessageId, ProviderCursor, RejectedMessage, RejectionReason, ReplaySupport,
    SenderId, ThreadId,
};
use fs2::FileExt;

const HANG_GUARD: Duration = Duration::from_secs(5);
const CHANNEL: &str = "spaces/example";
const OWNER: &str = "users/owner";
const GUEST: &str = "users/guest";
static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

fn configuration() -> Value {
    json!({
        "subscription_plugin": "fixture",
        "channel_ids": [CHANNEL],
        "allowed_senders": [OWNER],
        "agent_name": "coordinator",
        "agent_label": "coordinator",
        "outbound_enabled": true,
        "ack_reaction": null
    })
}

struct Fixture {
    root: PathBuf,
    state: BridgeState,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from("/tmp").join(format!(
            "agentctl-chat-rejection-service-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        let state = BridgeState::initialize(
            &root,
            serde_json::from_value(configuration()).expect("bridge configuration"),
        )
        .expect("initialize private state");
        Self { root, state }
    }

    fn age_uncertain_attempt(&self) {
        // Age only this scratch fixture's persisted retry evidence, without a minute-long wait.
        let lock = crate::agent::open_private_lock(
            &self.root.join(".state.lock"),
            "rejection retry fixture lock",
        )
        .expect("state lock");
        lock.lock_exclusive().expect("lock private rejection state");
        let path = self.root.join("rejection-reactions.json");
        let mut document: Value =
            serde_json::from_slice(&fs::read(&path).expect("saved rejection"))
                .expect("rejection document");
        let records = document["records"].as_array_mut().expect("records");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["phase"], "pending");
        assert!(records[0]["attempted_at_millis"].is_u64());
        records[0]["recorded_at_millis"] = json!(1);
        records[0]["attempted_at_millis"] = json!(1);
        crate::agent::atomic_json(&path, &document).expect("age scratch retry evidence");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove private state fixture");
    }
}

fn rejected() -> CommittableEvent {
    CommittableEvent::message_rejected(RejectedMessage::new(
        ChannelId::new(CHANNEL).expect("channel"),
        MessageId::new(format!("{CHANNEL}/messages/rejected")).expect("message"),
        SenderId::new(GUEST).expect("sender"),
        RejectionReason::SenderNotAllowed,
    ))
}

fn owner_message(name: &str) -> CommittableEvent {
    CommittableEvent::message_created(
        InboundMessage::new(
            ChannelId::new(CHANNEL).expect("channel"),
            MessageId::new(format!("{CHANNEL}/messages/{name}")).expect("message"),
            ThreadId::new(format!("{CHANNEL}/threads/{name}")).expect("thread"),
            SenderId::new(OWNER).expect("sender"),
            format!("request {name}"),
            "2026-10-20T12:00:00Z",
            false,
        )
        .expect("message"),
    )
}

fn batch(sequence: u64, cursor: &str, events: Vec<CommittableEvent>) -> SubscriptionItem {
    SubscriptionItem::Batch(
        DeliveryBatch::new(
            EventSequence::new(sequence).expect("sequence"),
            ProviderCursor::new(cursor).expect("cursor"),
            DeliveryId::new(format!("delivery-{sequence}-{cursor}")).expect("delivery"),
            events,
        )
        .expect("bounded batch"),
    )
}

#[derive(Clone)]
struct Cancellation {
    cancelled: Arc<AtomicBool>,
    input: mpsc::Sender<Option<SubscriptionItem>>,
}

impl ChatSubscriptionCancellation for Cancellation {
    fn cancel(&self) -> Result<(), CancellationError> {
        self.cancelled.store(true, Ordering::SeqCst);
        let _ = self.input.send(None);
        Ok(())
    }
}

struct Backend {
    cancellation: Cancellation,
    input: Option<mpsc::Receiver<Option<SubscriptionItem>>>,
    started: mpsc::Sender<SubscribeRequest>,
    acknowledged: mpsc::Sender<String>,
}

struct Driver {
    cancellation: Cancellation,
    input: mpsc::Receiver<Option<SubscriptionItem>>,
    acknowledged: mpsc::Sender<String>,
}

impl ChatSubscriptionBackend for Backend {
    fn cancellation(&self) -> Arc<dyn ChatSubscriptionCancellation> {
        Arc::new(self.cancellation.clone())
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::new(
            "rejection-fixture",
            ReplaySupport::Cursor,
            false,
            NonZeroU16::new(1).expect("one"),
            vec![
                EventKind::MessageCreated,
                EventKind::MessageRejected,
                EventKind::Checkpoint,
                EventKind::Heartbeat,
            ],
        )
        .expect("capabilities")
    }

    fn subscribe(
        &mut self,
        request: &SubscribeRequest,
    ) -> Result<Box<dyn ChatSubscriptionDriver>, BackendFailure> {
        self.started.send(request.clone()).expect("observe Start");
        Ok(Box::new(Driver {
            cancellation: self.cancellation.clone(),
            input: self.input.take().expect("one subscription"),
            acknowledged: self.acknowledged.clone(),
        }))
    }
}

impl ChatSubscriptionDriver for Driver {
    fn cancellation(&self) -> Arc<dyn ChatSubscriptionCancellation> {
        Arc::new(self.cancellation.clone())
    }

    fn next_item(&mut self) -> Result<Option<SubscriptionItem>, BackendFailure> {
        if self.cancellation.cancelled.load(Ordering::SeqCst) {
            return Ok(None);
        }
        self.input.recv_timeout(HANG_GUARD).map_err(|error| {
            BackendFailure::new("fixture_timeout", error.to_string(), false)
                .expect("bounded fixture error")
        })
    }

    fn acknowledge(&mut self, receipt: &DeliveryId) -> Result<(), BackendFailure> {
        self.acknowledged
            .send(receipt.as_str().to_owned())
            .expect("observe provider ACK");
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Submitted {
    channel_id: String,
    message_id: String,
    sender_id: String,
    emoji: String,
    request_id: String,
}

enum HelperOutcome {
    Complete,
    Unknown,
    Refused,
}

struct GuardedTransport {
    submitted: mpsc::Sender<Submitted>,
    release: mpsc::Receiver<HelperOutcome>,
}

impl ReactionTransport for GuardedTransport {
    fn ensure_reaction(
        &mut self,
        _submission: ReactionSubmission<'_>,
    ) -> Result<ReactionReceipt, OutboundFailure> {
        panic!("rejection feedback must never use the unguarded reaction operation");
    }

    fn ensure_rejection(
        &mut self,
        submission: RejectionSubmission<'_>,
    ) -> Result<ReactionReceipt, OutboundFailure> {
        self.submitted
            .send(Submitted {
                channel_id: submission.channel_id.to_owned(),
                message_id: submission.message_id.to_owned(),
                sender_id: submission.sender_id.to_owned(),
                emoji: submission.emoji.to_owned(),
                request_id: submission.request_id.to_owned(),
            })
            .expect("observe guarded operation");
        match self.release.recv_timeout(HANG_GUARD) {
            Ok(HelperOutcome::Complete) => Ok(ReactionReceipt {
                reaction_id: format!("{}/reactions/rejected", submission.message_id),
                already_present: false,
            }),
            Ok(HelperOutcome::Refused) => Err(OutboundFailure {
                code: "rejection_refused".to_owned(),
                detail: "fixture human-author verification refused feedback".to_owned(),
                outcome: OutboundOutcome::NotApplied,
                retryable: false,
            }),
            Ok(HelperOutcome::Unknown) | Err(_) => Err(OutboundFailure {
                code: "fixture_unknown".to_owned(),
                detail: "fixture operation may have been applied".to_owned(),
                outcome: OutboundOutcome::Unknown,
                retryable: true,
            }),
        }
    }
}

struct Live {
    provider: Option<ServiceWorker>,
    feedback: Option<ServiceWorker>,
    stop: Arc<StopState>,
    cancellation: SharedCancellation,
    queue: Arc<AckQueue>,
    input: mpsc::Sender<Option<SubscriptionItem>>,
    started: mpsc::Receiver<SubscribeRequest>,
    acknowledged: mpsc::Receiver<String>,
    notices: mpsc::Receiver<ProviderNotice>,
    results: mpsc::Receiver<Result<(), ProviderGenerationError>>,
    overflowed: Arc<AtomicBool>,
}

impl Live {
    fn start(state: &BridgeState) -> Self {
        let queue = Arc::new(AckQueue::default());
        let stop = Arc::new(StopState {
            ack_queue: Some(Arc::clone(&queue)),
            ..StopState::default()
        });
        let cancellation = Arc::new(Mutex::new(ProviderCancellationRegistry::default()));
        let overflowed = Arc::new(AtomicBool::new(false));
        let (input, receive) = mpsc::channel();
        let control = Cancellation {
            cancelled: Arc::new(AtomicBool::new(false)),
            input: input.clone(),
        };
        let (started, starts) = mpsc::channel();
        let (acknowledged, acknowledgements) = mpsc::channel();
        let backend = Backend {
            cancellation: control.clone(),
            input: Some(receive),
            started,
            acknowledged,
        };
        let (notices, received) = mpsc::sync_channel(PROVIDER_NOTICE_CAPACITY);
        let (completed, results) = mpsc::channel();
        let worker_state = state.clone();
        let worker_cancellation = Arc::clone(&cancellation);
        let worker_overflowed = Arc::clone(&overflowed);
        let timeouts = ProcessPhaseTimeouts::new(
            Duration::from_secs(2),
            Duration::from_secs(2),
            Duration::from_secs(2),
            Duration::from_secs(2),
            Duration::from_secs(1),
        )
        .expect("bounded fixture phases");
        let provider = spawn_provider_worker(
            Arc::clone(&stop),
            notices,
            Arc::new(Mutex::new(None)),
            move |stop, notices, wake| {
                let result = run_provider_generation(
                    &worker_state,
                    stop,
                    &worker_cancellation,
                    notices,
                    wake,
                    &worker_overflowed,
                    timeouts,
                    || Ok((backend, control)),
                    &|_| {},
                );
                let _ = completed.send(result);
            },
        )
        .expect("spawn real provider generation");
        Self {
            provider: Some(provider),
            feedback: None,
            stop,
            cancellation,
            queue,
            input,
            started: starts,
            acknowledged: acknowledgements,
            notices: received,
            results,
            overflowed,
        }
    }

    fn start_feedback(
        &mut self,
        state: &BridgeState,
    ) -> (mpsc::Receiver<Submitted>, mpsc::Sender<HelperOutcome>) {
        assert!(self.feedback.is_none());
        queue_saved_rejections(
            &self.queue,
            state.rejection_reaction_attempts().expect("saved feedback"),
        );
        let (submitted, submissions) = mpsc::channel();
        let (release, released) = mpsc::channel();
        self.feedback = Some(
            spawn_ack_worker(
                AckLane::Rejections,
                state.clone(),
                Arc::clone(&self.queue),
                GuardedTransport {
                    submitted,
                    release: released,
                },
                Arc::clone(&self.stop),
                Arc::new(Mutex::new(None)),
                |_| {},
            )
            .expect("spawn actual rejection lane"),
        );
        (submissions, release)
    }

    fn committed(&self, state: &BridgeState, item: SubscriptionItem) -> Vec<String> {
        let SubscriptionItem::Batch(ref delivery) = item else {
            panic!("fixture commits batches");
        };
        let receipt = delivery.delivery_id().as_str().to_owned();
        let cursor = delivery.cursor().as_str().to_owned();
        self.input.send(Some(item)).expect("send provider item");
        assert_eq!(self.acknowledged.recv_timeout(HANG_GUARD), Ok(receipt));
        let keys = match self
            .notices
            .recv_timeout(HANG_GUARD)
            .expect("provider batch notice")
        {
            ProviderNotice::Batch(keys) => keys,
            _ => panic!("rejection feedback must not end the provider"),
        };
        assert_eq!(state.cursor().expect("durable cursor"), Some(cursor));
        self.assert_live();
        keys
    }

    fn assert_live(&self) {
        assert!(
            !self.stop.is_stopped(),
            "feedback does not stop the service"
        );
        assert!(!self
            .provider
            .as_ref()
            .expect("provider")
            .handle
            .is_finished());
        assert!(
            self.results.try_recv().is_err(),
            "the same provider generation remains live"
        );
        assert!(!self.overflowed.load(Ordering::SeqCst));
    }

    fn finished_feedback(&self, key: &str) {
        let deadline = Instant::now() + HANG_GUARD;
        let mut state = self.queue.state.lock().expect("reaction queue");
        while state.rejections_retained.contains(key) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "rejection worker did not publish its outcome"
            );
            let (next, _) = self
                .queue
                .changed
                .wait_timeout(state, remaining)
                .expect("reaction completion");
            state = next;
        }
    }

    fn finish(mut self) {
        self.stop.stop();
        cancel_provider(&self.cancellation).expect("cancel blocked provider");
        if let Some(worker) = self.feedback.take() {
            join_worker_until(worker, "rejection fixture", Instant::now() + HANG_GUARD)
                .expect("join rejection lane");
        }
        join_worker_until(
            self.provider.take().expect("provider worker"),
            "rejection fixture provider",
            Instant::now() + HANG_GUARD,
        )
        .expect("join provider");
        assert!(self.stop.take_cleanup_errors().is_empty());
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.stop.stop();
        let _ = cancel_provider(&self.cancellation);
        for worker in [self.feedback.take(), self.provider.take()]
            .into_iter()
            .flatten()
        {
            let _ = join_worker_until(
                worker,
                "rejection fixture cleanup",
                Instant::now() + HANG_GUARD,
            );
        }
    }
}

#[test]
fn a_blocked_or_failing_rejection_helper_does_not_hold_cursor_or_later_requests() {
    let fixture = Fixture::new();
    let mut live = Live::start(&fixture.state);
    let start = live
        .started
        .recv_timeout(HANG_GUARD)
        .expect("provider Start");
    assert!(start.resume_from().is_none());
    let (submissions, release) = live.start_feedback(&fixture.state);
    assert!(live
        .committed(
            &fixture.state,
            batch(
                1,
                "rejected",
                vec![CommittableEvent::Checkpoint, rejected()]
            )
        )
        .is_empty());
    let submission = submissions
        .recv_timeout(HANG_GUARD)
        .expect("rejection helper is blocked");
    assert_eq!(submission.sender_id, GUEST);
    assert_eq!(submission.emoji, "❌");
    let attempts = fixture
        .state
        .rejection_reaction_attempts()
        .expect("in-flight feedback");
    assert_eq!(attempts.len(), 1);
    let key = &attempts[0].0;

    let admitted = live.committed(
        &fixture.state,
        batch(
            2,
            "owner-while-blocked",
            vec![owner_message("while-blocked")],
        ),
    );
    assert_eq!(
        admitted.len(),
        1,
        "later allowed messages are admitted while feedback is blocked"
    );
    assert_eq!(
        fixture
            .state
            .inspect_request(&admitted[0])
            .expect("request")["source"]["sender_id"],
        OWNER
    );
    release
        .send(HelperOutcome::Refused)
        .expect("release failing helper");
    live.finished_feedback(key);
    assert!(fixture
        .state
        .rejection_reaction_attempts()
        .expect("permanent refusal is settled")
        .is_empty());
    let after = live.committed(
        &fixture.state,
        batch(
            3,
            "owner-after-failure",
            vec![owner_message("after-failure")],
        ),
    );
    assert_eq!(
        after.len(),
        1,
        "a failed reaction never stops request intake"
    );
    assert_eq!(fixture.state.request_keys().expect("requests").len(), 2);
    assert!(
        submissions.try_recv().is_err(),
        "one rejected message produced one guarded operation"
    );
    live.finish();
}

#[test]
fn rejection_worker_recovers_the_same_operation_across_restart_and_replay() {
    let fixture = Fixture::new();
    let mut first = Live::start(&fixture.state);
    first.started.recv_timeout(HANG_GUARD).expect("first Start");
    let (first_submissions, first_release) = first.start_feedback(&fixture.state);
    assert!(first
        .committed(
            &fixture.state,
            batch(
                1,
                "rejected",
                vec![CommittableEvent::Checkpoint, rejected()]
            )
        )
        .is_empty());
    let uncertain = first_submissions
        .recv_timeout(HANG_GUARD)
        .expect("first guarded operation");
    let attempts = fixture
        .state
        .rejection_reaction_attempts()
        .expect("pending feedback");
    assert_eq!(attempts.len(), 1);
    let key = attempts[0].0.clone();
    first_release
        .send(HelperOutcome::Unknown)
        .expect("ambiguous first result");
    first.finished_feedback(&key);
    assert_eq!(
        fixture
            .state
            .rejection_reaction_attempts()
            .expect("uncertain operation retained")
            .len(),
        1
    );
    first.finish();
    fixture.age_uncertain_attempt();

    let reopened = BridgeState::inspect(&fixture.root).expect("restart opens durable state");
    let mut second = Live::start(&reopened);
    let start = second
        .started
        .recv_timeout(HANG_GUARD)
        .expect("restart Start");
    assert_eq!(
        start.resume_from().map(ProviderCursor::as_str),
        Some("rejected")
    );
    let (second_submissions, second_release) = second.start_feedback(&reopened);
    let recovered = second_submissions
        .recv_timeout(HANG_GUARD)
        .expect("saved feedback is recovered");
    assert_eq!(
        recovered, uncertain,
        "recovery reuses the exact guarded operation UUID and authority"
    );
    assert!(second
        .committed(
            &reopened,
            batch(
                1,
                "rejected",
                vec![CommittableEvent::Checkpoint, rejected()]
            )
        )
        .is_empty());
    assert_eq!(
        reopened
            .rejection_reaction_attempts()
            .expect("single retained operation")
            .len(),
        1
    );
    second_release
        .send(HelperOutcome::Complete)
        .expect("finish recovered operation");
    second.finished_feedback(&key);
    assert!(reopened
        .rejection_reaction_attempts()
        .expect("completed feedback")
        .is_empty());
    assert!(second
        .committed(&reopened, batch(2, "redelivered", vec![rejected()]))
        .is_empty());
    second.finished_feedback(&key);
    assert!(
        second_submissions.try_recv().is_err(),
        "replay and redelivery must not create a second operation"
    );
    assert!(
        reopened.request_keys().expect("requests").is_empty(),
        "feedback never creates a request"
    );
    second.finish();
}

#[test]
fn rejection_configuration_defaults_disables_and_customizes_feedback() {
    let config: BridgeConfiguration =
        serde_json::from_value(configuration()).expect("default config");
    assert_eq!(config.reject_emoji(), Some("❌"));
    for disabled in [Value::Null, json!(""), json!(false)] {
        let mut value = configuration();
        value["reject_reaction"] = disabled;
        let config: BridgeConfiguration = serde_json::from_value(value).expect("disabled config");
        assert_eq!(config.reject_emoji(), None);
    }
    let mut custom = configuration();
    custom["reject_reaction"] = json!("⛔");
    let config: BridgeConfiguration = serde_json::from_value(custom).expect("custom config");
    assert_eq!(config.reject_emoji(), Some("⛔"));
    let mut outbound_disabled = configuration();
    outbound_disabled["outbound_enabled"] = json!(false);
    let config: BridgeConfiguration =
        serde_json::from_value(outbound_disabled).expect("outbound disabled");
    assert_eq!(config.reject_emoji(), None);
}

#[test]
fn guarded_rejection_wire_binds_action_uuid_and_message_and_never_uses_plain_reactions() {
    let submission = RejectionSubmission {
        channel_id: CHANNEL,
        message_id: "spaces/example/messages/rejected",
        sender_id: GUEST,
        emoji: "❌",
        request_id: "123e4567-e89b-42d3-a456-426614174000",
    };
    let encoded = chat_runtime::encode_rejection_request(&submission).expect("guarded request");
    assert_eq!(
        serde_json::from_slice::<Value>(&encoded).expect("request JSON"),
        json!({
            "version": 1,
            "action": "ensure_rejection",
            "id": submission.request_id,
            "channel_id": CHANNEL,
            "message_id": submission.message_id,
            "sender_id": GUEST,
            "emoji": "❌",
        })
    );
    let response = json!({
        "version": 1,
        "action": "ensure_rejection",
        "id": submission.request_id,
        "ok": true,
        "receipt": {"reaction_id": "spaces/example/messages/rejected/reactions/feedback", "already_present": true},
    });
    assert!(
        chat_runtime::decode_rejection_response(
            &serde_json::to_vec(&response).expect("response"),
            &submission
        )
        .expect("exact bound response")
        .already_present
    );
    for (pointer, value) in [
        ("/action", json!("ensure_reaction")),
        ("/id", json!("123e4567-e89b-42d3-a456-426614174001")),
        (
            "/receipt/reaction_id",
            json!("spaces/example/messages/other/reactions/feedback"),
        ),
    ] {
        let mut wrong = response.clone();
        *wrong.pointer_mut(pointer).expect("response field") = value;
        let error = chat_runtime::decode_rejection_response(
            &serde_json::to_vec(&wrong).expect("wrong response"),
            &submission,
        )
        .expect_err("guarded receipt must match its exact operation and message");
        assert_eq!(error.outcome, OutboundOutcome::Unknown);
    }
    struct Unsupported {
        plain_calls: usize,
    }
    impl ReactionTransport for Unsupported {
        fn ensure_reaction(
            &mut self,
            _submission: ReactionSubmission<'_>,
        ) -> Result<ReactionReceipt, OutboundFailure> {
            self.plain_calls += 1;
            panic!("unsupported guarded feedback cannot fall back to an ordinary reaction");
        }
    }
    let mut transport = Unsupported { plain_calls: 0 };
    let error = transport
        .ensure_rejection(submission)
        .expect_err("unsupported guarded operation refuses safely");
    assert_eq!(error.code, "rejection_unsupported");
    assert_eq!(error.outcome, OutboundOutcome::NotApplied);
    assert!(!error.retryable);
    assert_eq!(transport.plain_calls, 0);
}
