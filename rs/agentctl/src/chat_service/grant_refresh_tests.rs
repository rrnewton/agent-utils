use super::*;

use std::fs;
use std::num::NonZeroU16;
use std::sync::atomic::AtomicU64;

use chat_subscription::{
    CancellationError, ChannelId, CommittableEvent, DeliveryBatch, EventKind, EventSequence,
    Heartbeat, InboundMessage, MessageId, ProviderCursor, ReplaySupport, SenderId, ThreadId,
};
use fs2::FileExt;

const HANG_GUARD: Duration = Duration::from_secs(5);
const OWNER: &str = "users/owner";
const GUEST: &str = "users/guest";
static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    state: BridgeState,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from("/tmp").join(format!(
            "agentctl-chat-grant-refresh-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        let config = serde_json::from_value(json!({
            "subscription_plugin": "fixture",
            "channel_ids": ["spaces/example"],
            "allowed_senders": [OWNER],
            "agent_name": "coordinator",
            "agent_label": "coordinator",
            "outbound_enabled": false,
            "ack_reaction": null
        }))
        .expect("bridge configuration");
        let state = BridgeState::initialize(&root, config).expect("initialize private state");
        Self { root, state }
    }

    fn grant_guest(&self) {
        BridgeState::inspect(&self.root)
            .expect("CLI opens its own state object")
            .grant_senders(&[GUEST.to_owned()], Duration::from_secs(3_600), None)
            .expect("grant guest for an hour");
    }

    fn expire_guest(&self) {
        // Change only this test's private durable clock evidence, without sleeping for expiry.
        let lock = crate::agent::open_private_lock(
            &self.root.join(".state.lock"),
            "grant expiry fixture lock",
        )
        .expect("open state lock");
        FileExt::lock_exclusive(&lock).expect("hold state lock while expiring grant");
        let path = self.root.join("grants.json");
        let mut document: Value = serde_json::from_slice(&fs::read(&path).expect("durable grants"))
            .expect("grant document");
        let grants = document["grants"].as_array_mut().expect("grant array");
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0]["sender_id"], GUEST);
        grants[0]["created_at_millis"] = json!(1);
        grants[0]["expires_at_millis"] = json!(2);
        crate::agent::atomic_json(&path, &document).expect("publish expired scratch grant");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove private state fixture");
    }
}

fn message(sender: &str, name: &str) -> CommittableEvent {
    CommittableEvent::message_created(
        InboundMessage::new(
            ChannelId::new("spaces/example").expect("channel"),
            MessageId::new(format!("spaces/example/messages/{name}")).expect("message"),
            ThreadId::new(format!("spaces/example/threads/{name}")).expect("thread"),
            SenderId::new(sender).expect("sender"),
            format!("request {name}"),
            "2026-10-20T12:00:00Z",
            false,
        )
        .expect("inbound message"),
    )
}

fn batch(sequence: u64, cursor: &str, events: Vec<CommittableEvent>) -> SubscriptionItem {
    SubscriptionItem::Batch(
        DeliveryBatch::new(
            EventSequence::new(sequence).expect("sequence"),
            ProviderCursor::new(cursor).expect("cursor"),
            DeliveryId::new(format!("delivery-{cursor}")).expect("delivery ID"),
            events,
        )
        .expect("delivery batch"),
    )
}

fn heartbeat(sequence: u64) -> SubscriptionItem {
    SubscriptionItem::Heartbeat(Heartbeat::new(
        EventSequence::new(sequence).expect("heartbeat sequence"),
    ))
}

struct Started {
    generation: usize,
    request: SubscribeRequest,
    items: mpsc::Sender<Option<SubscriptionItem>>,
}

#[derive(Clone)]
struct ScriptSignals {
    starts: mpsc::Sender<Started>,
    polls: mpsc::Sender<(usize, usize)>,
    acknowledgements: mpsc::Sender<(usize, String)>,
    closes: mpsc::Sender<usize>,
}

struct ScriptProbe {
    starts: mpsc::Receiver<Started>,
    polls: mpsc::Receiver<(usize, usize)>,
    acknowledgements: mpsc::Receiver<(usize, String)>,
    closes: mpsc::Receiver<usize>,
}

impl ScriptProbe {
    fn new() -> (ScriptSignals, Self) {
        let (starts, started) = mpsc::channel();
        let (polls, polled) = mpsc::channel();
        let (acknowledgements, acknowledged) = mpsc::channel();
        let (closes, closed) = mpsc::channel();
        (
            ScriptSignals {
                starts,
                polls,
                acknowledgements,
                closes,
            },
            Self {
                starts: started,
                polls: polled,
                acknowledgements: acknowledged,
                closes: closed,
            },
        )
    }

    fn started(&self, generation: usize) -> Started {
        let started = self
            .starts
            .recv_timeout(HANG_GUARD)
            .expect("provider Start");
        assert_eq!(started.generation, generation);
        started
    }

    fn polled(&self, generation: usize, item: usize) {
        assert_eq!(
            self.polls
                .recv_timeout(HANG_GUARD)
                .expect("blocked next_item"),
            (generation, item)
        );
    }
}

#[derive(Clone)]
struct ScriptCancellation {
    cancelled: Arc<AtomicBool>,
    items: mpsc::Sender<Option<SubscriptionItem>>,
}

impl ChatSubscriptionCancellation for ScriptCancellation {
    fn cancel(&self) -> Result<(), CancellationError> {
        if !self.cancelled.swap(true, Ordering::SeqCst) {
            // A sticky flag covers a future receive; this item releases an already blocked one.
            let _ = self.items.send(None);
        }
        Ok(())
    }
}

struct ScriptBackend {
    generation: usize,
    items: Option<mpsc::Receiver<Option<SubscriptionItem>>>,
    cancellation: ScriptCancellation,
    signals: ScriptSignals,
}

struct ScriptDriver {
    generation: usize,
    received: usize,
    items: mpsc::Receiver<Option<SubscriptionItem>>,
    cancellation: ScriptCancellation,
    signals: ScriptSignals,
}

fn script_backend(
    generation: usize,
    signals: ScriptSignals,
) -> (ScriptBackend, ScriptCancellation) {
    let (items, receiver) = mpsc::channel();
    let cancellation = ScriptCancellation {
        cancelled: Arc::new(AtomicBool::new(false)),
        items,
    };
    (
        ScriptBackend {
            generation,
            items: Some(receiver),
            cancellation: cancellation.clone(),
            signals,
        },
        cancellation,
    )
}

impl ChatSubscriptionBackend for ScriptBackend {
    fn cancellation(&self) -> Arc<dyn ChatSubscriptionCancellation> {
        Arc::new(self.cancellation.clone())
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::new(
            "grant-refresh-fixture",
            ReplaySupport::Cursor,
            false,
            NonZeroU16::new(1).expect("one"),
            vec![
                EventKind::MessageCreated,
                EventKind::Checkpoint,
                EventKind::Heartbeat,
            ],
        )
        .expect("provider capabilities")
    }

    fn subscribe(
        &mut self,
        request: &SubscribeRequest,
    ) -> Result<Box<dyn ChatSubscriptionDriver>, BackendFailure> {
        self.signals
            .starts
            .send(Started {
                generation: self.generation,
                request: request.clone(),
                items: self.cancellation.items.clone(),
            })
            .expect("Start observer");
        Ok(Box::new(ScriptDriver {
            generation: self.generation,
            received: 0,
            items: self.items.take().expect("one stream per backend"),
            cancellation: self.cancellation.clone(),
            signals: self.signals.clone(),
        }))
    }
}

impl ChatSubscriptionDriver for ScriptDriver {
    fn cancellation(&self) -> Arc<dyn ChatSubscriptionCancellation> {
        Arc::new(self.cancellation.clone())
    }

    fn next_item(&mut self) -> Result<Option<SubscriptionItem>, BackendFailure> {
        if self.cancellation.cancelled.load(Ordering::SeqCst) {
            return Ok(None);
        }
        self.signals
            .polls
            .send((self.generation, self.received))
            .expect("receive observer");
        self.received += 1;
        self.items.recv_timeout(HANG_GUARD).map_err(|error| {
            BackendFailure::new("fixture_receive_timeout", error.to_string(), false)
                .expect("bounded fixture failure")
        })
    }

    fn acknowledge(&mut self, delivery_id: &DeliveryId) -> Result<(), BackendFailure> {
        self.signals
            .acknowledgements
            .send((self.generation, delivery_id.as_str().to_owned()))
            .expect("commit observer");
        Ok(())
    }

    fn close(&mut self) -> Result<(), BackendFailure> {
        self.signals
            .closes
            .send(self.generation)
            .expect("close observer");
        Ok(())
    }
}

fn committed(
    stream: &mut ChatSubscription,
    state: &BridgeState,
    started: &Started,
    probe: &ScriptProbe,
    item: SubscriptionItem,
    cursor: &str,
) -> Vec<String> {
    started.items.send(Some(item)).expect("emit provider batch");
    let chat_runtime::ConsumedItem::Batch(admission) =
        chat_runtime::consume_one(stream, state).expect("commit batch through stale subscription")
    else {
        panic!("expected a committed batch");
    };
    assert_eq!(
        state.cursor().expect("durable cursor"),
        Some(cursor.to_owned())
    );
    assert_eq!(
        probe
            .acknowledgements
            .recv_timeout(HANG_GUARD)
            .expect("provider ACK after durable commit"),
        (started.generation, format!("delivery-{cursor}"))
    );
    admission.new_request_keys
}

fn refuses_new_guest_messages_after_changing_authority(expire: bool) {
    let fixture = Fixture::new();
    fixture.grant_guest();
    let (signals, probe) = ScriptProbe::new();
    let (mut backend, _cancellation) = script_backend(0, signals);
    let request = fixture
        .state
        .subscribe_request()
        .expect("active grant in Start");
    let mut stream = ChatSubscription::open(&mut backend, &request).expect("open old subscription");
    let started = probe.started(0);
    assert_eq!(
        started
            .request
            .allowed_senders()
            .iter()
            .map(SenderId::as_str)
            .collect::<Vec<_>>(),
        [OWNER, GUEST]
    );
    let initial = committed(
        &mut stream,
        &fixture.state,
        &started,
        &probe,
        batch(
            1,
            "before-policy-change",
            vec![CommittableEvent::Checkpoint, message(GUEST, "before")],
        ),
        "before-policy-change",
    );
    assert_eq!(initial.len(), 1, "the guest was allowed before the change");
    if expire {
        fixture.expire_guest();
    } else {
        let removed = BridgeState::inspect(&fixture.root)
            .expect("independent revoke command")
            .revoke_senders(&[GUEST.to_owned()])
            .expect("revoke guest");
        assert_eq!(removed, [GUEST]);
    }
    // The stream still carries the original owner + guest list. Admission must independently
    // read current authority instead of trusting this now-stale provider allowlist.
    assert_eq!(started.request.allowed_senders().len(), 2);
    let refused = committed(
        &mut stream,
        &fixture.state,
        &started,
        &probe,
        batch(2, "after-policy-change", vec![message(GUEST, "after")]),
        "after-policy-change",
    );
    assert!(refused.is_empty(), "guest messages stop creating requests");
    let owners = committed(
        &mut stream,
        &fixture.state,
        &started,
        &probe,
        batch(
            3,
            "owner-still-allowed",
            vec![message(OWNER, "owner-after")],
        ),
        "owner-still-allowed",
    );
    assert_eq!(owners.len(), 1, "the original owner stays permanent");
    let reopened = BridgeState::inspect(&fixture.root).expect("reopen durable bridge");
    assert_eq!(
        reopened
            .inspect_request(&initial[0])
            .expect("retained guest request")["source"]["sender_id"],
        GUEST
    );
    assert_eq!(
        reopened.inspect_request(&owners[0]).expect("owner request")["source"]["sender_id"],
        OWNER
    );
    assert_eq!(reopened.request_keys().expect("retained requests").len(), 2);
    let saved: Value = serde_json::from_slice(
        &fs::read(
            fixture
                .root
                .join("requests")
                .join(format!("{}.json", initial[0])),
        )
        .expect("saved admitted request"),
    )
    .expect("request JSON");
    assert_eq!(saved["sender_grant"]["sender_id"], GUEST);
}

#[test]
fn an_expired_grant_refuses_new_messages_and_preserves_cursor_owners_and_admitted_requests() {
    refuses_new_guest_messages_after_changing_authority(true);
}

#[test]
fn a_revoked_grant_refuses_new_messages_and_preserves_cursor_owners_and_admitted_requests() {
    refuses_new_guest_messages_after_changing_authority(false);
}

struct LiveProvider {
    worker: Option<ServiceWorker>,
    stop: Arc<StopState>,
    cancellation: SharedCancellation,
    probe: ScriptProbe,
    notices: mpsc::Receiver<ProviderNotice>,
    lines: Arc<Mutex<Vec<String>>>,
    failures: Arc<Mutex<Vec<String>>>,
    delays: Arc<Mutex<Vec<Duration>>>,
    overflowed: Arc<AtomicBool>,
}

impl LiveProvider {
    fn start(state: &BridgeState) -> Self {
        let stop = Arc::new(StopState::default());
        let cancellation: SharedCancellation =
            Arc::new(Mutex::new(ProviderCancellationRegistry::default()));
        let output_wake: SharedWake = Arc::new(Mutex::new(None));
        let overflowed = Arc::new(AtomicBool::new(false));
        let (notices, receiver) = mpsc::sync_channel(PROVIDER_NOTICE_CAPACITY);
        let (signals, probe) = ScriptProbe::new();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let failures = Arc::new(Mutex::new(Vec::new()));
        let delays = Arc::new(Mutex::new(Vec::new()));
        let worker_state = state.clone();
        let worker_cancellation = Arc::clone(&cancellation);
        let worker_overflowed = Arc::clone(&overflowed);
        let worker_lines = Arc::clone(&lines);
        let worker_failures = Arc::clone(&failures);
        let worker_delays = Arc::clone(&delays);
        let timeouts = ProcessPhaseTimeouts::new(
            Duration::from_secs(2),
            Duration::from_secs(2),
            Duration::from_secs(2),
            Duration::from_secs(2),
            Duration::from_secs(1),
        )
        .expect("fixture process bounds");
        let worker = spawn_provider_worker(
            Arc::clone(&stop),
            notices,
            output_wake,
            move |stop, notices, wake| {
                let mut generation = 0;
                let log = |line: fmt::Arguments<'_>| {
                    worker_lines
                        .lock()
                        .expect("service logs")
                        .push(line.to_string());
                };
                run_provider_worker(
                    || {
                        let current = generation;
                        generation += 1;
                        run_provider_generation(
                            &worker_state,
                            stop,
                            &worker_cancellation,
                            notices,
                            wake,
                            &worker_overflowed,
                            timeouts,
                            || Ok(script_backend(current, signals.clone())),
                            &log,
                        )
                    },
                    Instant::now,
                    &log,
                    &|error| {
                        worker_failures
                            .lock()
                            .expect("provider failures")
                            .push(error.to_owned());
                        worker_state
                            .note_subscription_down(error)
                            .expect("record provider down");
                    },
                    |delay| {
                        worker_delays.lock().expect("retry delays").push(delay);
                        // A policy refresh must never back off; stop an incorrect worker so a
                        // failed regression remains bounded instead of entering a retry loop.
                        stop.stop();
                    },
                    stop,
                    notices,
                    wake,
                );
            },
        )
        .expect("spawn real provider worker");
        Self {
            worker: Some(worker),
            stop,
            cancellation,
            probe,
            notices: receiver,
            lines,
            failures,
            delays,
            overflowed,
        }
    }

    fn assert_healthy(&self, state: &BridgeState) {
        assert!(
            !self.stop.is_stopped(),
            "the same service worker is still live"
        );
        assert!(!self.worker.as_ref().expect("worker").handle.is_finished());
        assert!(self.failures.lock().expect("provider failures").is_empty());
        assert!(self.delays.lock().expect("retry delays").is_empty());
        assert!(
            state.status().expect("provider health")["provider_health"]["subscription"].is_null()
        );
        assert!(!self.overflowed.load(Ordering::SeqCst));
        assert!(!self
            .lines
            .lock()
            .expect("logs")
            .iter()
            .any(|line| line.contains("reconnecting in")));
    }

    fn finish(mut self) {
        self.stop.stop();
        cancel_provider(&self.cancellation).expect("release blocked fixture receive");
        join_worker_until(
            self.worker.take().expect("live worker"),
            "grant refresh fixture provider",
            Instant::now() + HANG_GUARD,
        )
        .expect("join stopped provider worker");
        assert!(self.stop.take_cleanup_errors().is_empty());
    }
}

impl Drop for LiveProvider {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.stop.stop();
            let _ = cancel_provider(&self.cancellation);
            let _ = join_worker_until(
                worker,
                "grant refresh fixture cleanup",
                Instant::now() + HANG_GUARD,
            );
        }
    }
}

#[test]
fn a_running_provider_worker_picks_up_a_grant_without_restart_or_provider_failure() {
    let fixture = Fixture::new();
    let live = LiveProvider::start(&fixture.state);
    let first = live.probe.started(0);
    assert_eq!(
        first
            .request
            .allowed_senders()
            .iter()
            .map(SenderId::as_str)
            .collect::<Vec<_>>(),
        [OWNER]
    );
    live.probe.polled(0, 0);
    fixture.grant_guest();
    first
        .items
        .send(Some(heartbeat(1)))
        .expect("release old generation with heartbeat");
    assert_eq!(live.probe.closes.recv_timeout(HANG_GUARD), Ok(0));
    let second = live.probe.started(1);
    assert_eq!(
        second
            .request
            .allowed_senders()
            .iter()
            .map(SenderId::as_str)
            .collect::<Vec<_>>(),
        [OWNER, GUEST]
    );
    live.probe.polled(1, 0);
    second
        .items
        .send(Some(batch(
            1,
            "live-grant-cursor",
            vec![CommittableEvent::Checkpoint, message(GUEST, "live-grant")],
        )))
        .expect("emit newly granted sender message");
    let ProviderNotice::Batch(keys) = live
        .notices
        .recv_timeout(HANG_GUARD)
        .expect("admission notice")
    else {
        panic!("grant refresh must not produce a fatal notice");
    };
    assert_eq!(keys.len(), 1, "the new sender reaches admission");
    assert_eq!(
        fixture
            .state
            .inspect_request(&keys[0])
            .expect("new guest request")["source"]["sender_id"],
        GUEST
    );
    assert_eq!(
        fixture.state.cursor().expect("durable live cursor"),
        Some("live-grant-cursor".to_owned())
    );
    assert_eq!(
        live.probe.acknowledgements.recv_timeout(HANG_GUARD),
        Ok((1, "delivery-live-grant-cursor".to_owned()))
    );
    live.probe.polled(1, 1);
    live.assert_healthy(&fixture.state);
    live.finish();
}

#[test]
fn a_grant_while_receive_is_blocked_replays_the_filtered_message_without_advancing_its_cursor() {
    let fixture = Fixture::new();
    let live = LiveProvider::start(&fixture.state);
    let first = live.probe.started(0);
    assert_eq!(
        first
            .request
            .allowed_senders()
            .iter()
            .map(SenderId::as_str)
            .collect::<Vec<_>>(),
        [OWNER]
    );
    live.probe.polled(0, 0);
    first
        .items
        .send(Some(batch(
            1,
            "saved-before-grant",
            vec![CommittableEvent::Checkpoint],
        )))
        .expect("establish a durable resume cursor");
    assert_eq!(
        live.probe.acknowledgements.recv_timeout(HANG_GUARD),
        Ok((0, "delivery-saved-before-grant".to_owned()))
    );
    let ProviderNotice::Batch(keys) = live
        .notices
        .recv_timeout(HANG_GUARD)
        .expect("initial checkpoint notice")
    else {
        panic!("the initial checkpoint must not stop the worker");
    };
    assert!(keys.is_empty());
    // The worker already checked its old sender list and is now waiting inside the driver.
    live.probe.polled(0, 1);
    fixture.grant_guest();
    first
        .items
        .send(Some(batch(
            2,
            "first-guest-after-grant",
            vec![CommittableEvent::Checkpoint],
        )))
        .expect("old provider filters the first guest message to a checkpoint");
    assert_eq!(live.probe.closes.recv_timeout(HANG_GUARD), Ok(0));
    let second = live.probe.started(1);
    assert!(
        matches!(
            live.probe.acknowledgements.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ),
        "the stale checkpoint must remain unacknowledged so the guest message can replay"
    );
    assert!(
        matches!(live.notices.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "the filtered guest message must not produce a durable batch notice"
    );
    assert_eq!(
        fixture.state.cursor().expect("unchanged durable cursor"),
        Some("saved-before-grant".to_owned())
    );
    assert!(fixture
        .state
        .request_keys()
        .expect("no lost request")
        .is_empty());
    assert_eq!(
        second.request.resume_from().map(ProviderCursor::as_str),
        Some("saved-before-grant")
    );
    assert_eq!(
        second
            .request
            .allowed_senders()
            .iter()
            .map(SenderId::as_str)
            .collect::<Vec<_>>(),
        [OWNER, GUEST]
    );
    live.probe.polled(1, 0);
    second
        .items
        .send(Some(batch(
            1,
            "first-guest-after-grant",
            vec![message(GUEST, "first-after-grant")],
        )))
        .expect("replay the uncommitted provider event under the refreshed sender list");
    let ProviderNotice::Batch(keys) = live
        .notices
        .recv_timeout(HANG_GUARD)
        .expect("replayed guest admission notice")
    else {
        panic!("the replayed message must not stop the worker");
    };
    assert_eq!(
        keys.len(),
        1,
        "the first newly granted message is preserved"
    );
    assert_eq!(
        fixture
            .state
            .inspect_request(&keys[0])
            .expect("replayed guest request")["source"]["sender_id"],
        GUEST
    );
    assert_eq!(
        fixture.state.cursor().expect("replayed guest cursor"),
        Some("first-guest-after-grant".to_owned())
    );
    assert_eq!(
        live.probe.acknowledgements.recv_timeout(HANG_GUARD),
        Ok((1, "delivery-first-guest-after-grant".to_owned()))
    );
    live.probe.polled(1, 1);
    live.assert_healthy(&fixture.state);
    live.finish();
}

#[test]
fn an_expired_grant_is_logged_once_across_items_and_provider_restarts() {
    let fixture = Fixture::new();
    fixture.grant_guest();
    fixture.expire_guest();
    let live = LiveProvider::start(&fixture.state);
    let first = live.probe.started(0);
    live.probe.polled(0, 0);
    for sequence in 1..=2 {
        first
            .items
            .send(Some(heartbeat(sequence)))
            .expect("emit heartbeat");
        live.probe.polled(0, sequence as usize);
    }
    let expiry_prefix = format!("agentctl: chat grant expired: {GUEST} ");
    assert_eq!(
        live.lines
            .lock()
            .expect("service log")
            .iter()
            .filter(|line| line.starts_with(&expiry_prefix))
            .count(),
        1
    );
    live.assert_healthy(&fixture.state);
    live.finish();
    let reopened = BridgeState::inspect(&fixture.root).expect("reopen same bridge state");
    let restarted = LiveProvider::start(&reopened);
    let _first = restarted.probe.started(0);
    restarted.probe.polled(0, 0);
    assert!(!restarted
        .lines
        .lock()
        .expect("restarted service log")
        .iter()
        .any(|line| line.starts_with(&expiry_prefix)));
    restarted.assert_healthy(&reopened);
    restarted.finish();
}
