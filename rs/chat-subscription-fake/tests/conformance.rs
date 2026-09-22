//! Shared conformance suite for direct and out-of-process fake backends.

use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use chat_subscription::{
    BackendConfiguration, BackendFailure, ChannelId, ChatSubscription, ChatSubscriptionBackend,
    CommittableEvent, EventKind, ProviderCursor, SenderId, SubscribeRequest, SubscriptionError,
    SubscriptionItem,
};
use chat_subscription_fake::{FakeBackend, FakeSubscriptionAuthority};
use chat_subscription_plugin::process::{ProcessPhaseTimeouts, ProcessPluginChild};

fn request() -> SubscribeRequest {
    SubscribeRequest::new(
        vec![ChannelId::new("channels/conformance").expect("fixture channel")],
        vec![SenderId::new("users/owner").expect("fixture sender")],
        Some(ProviderCursor::new("durable-before").expect("fixture cursor")),
    )
    .expect("fixture authority")
    .with_backend_configuration(
        BackendConfiguration::new(
            "fixture.config.v1",
            serde_json::Map::from_iter([(
                "subscription".to_owned(),
                serde_json::Value::String("subscriptions/conformance".to_owned()),
            )]),
        )
        .expect("fixture configuration"),
    )
}

fn maximum_authority_request() -> SubscribeRequest {
    let channels = (0..chat_subscription::MAX_SUBSCRIPTION_CHANNELS)
        .map(|index| {
            ChannelId::new(format!(
                "{index:02}{}",
                "\0".repeat(chat_subscription::MAX_RESOURCE_ID_BYTES - 2)
            ))
            .expect("maximum channel")
        })
        .collect();
    let senders = (0..chat_subscription::MAX_ALLOWED_SENDERS)
        .map(|index| {
            SenderId::new(format!(
                "{index:03}{}",
                "\0".repeat(chat_subscription::MAX_SENDER_ID_BYTES - 3)
            ))
            .expect("maximum sender")
        })
        .collect();
    SubscribeRequest::new(
        channels,
        senders,
        Some(ProviderCursor::new("durable-before").expect("fixture cursor")),
    )
    .expect("maximum authority remains valid")
}

fn exercise(backend: &mut dyn ChatSubscriptionBackend, expected_name: &str) {
    let capabilities = backend.capabilities();
    assert_eq!(capabilities.backend_name(), expected_name);
    assert!(capabilities.full_message_data());
    assert_eq!(capabilities.max_uncommitted().get(), 1);
    assert!(capabilities.supports(EventKind::Gap));

    let mut subscription =
        ChatSubscription::open(backend, &request()).expect("conformance subscription opens");
    let first = match subscription.next_item().expect("first item") {
        Some(SubscriptionItem::Batch(delivery)) => delivery,
        other => panic!("expected first delivery, found {other:?}"),
    };
    assert!(matches!(
        first.events(),
        [CommittableEvent::MessageCreated(message)]
            if message.message_id().as_str() == "messages/one"
    ));
    assert_eq!(first.cursor().as_str(), "cursor-1");
    assert_eq!(first.delivery_id().as_str(), "delivery-1");
    assert_eq!(
        subscription.next_item(),
        Err(SubscriptionError::OutstandingDelivery {
            sequence: first.sequence()
        })
    );
    subscription
        .commit_durable(&first)
        .expect("first durable commit is acknowledged");

    let second = match subscription.next_item().expect("second item") {
        Some(SubscriptionItem::Batch(delivery)) => delivery,
        other => panic!("expected checkpoint delivery, found {other:?}"),
    };
    assert_eq!(second.sequence().get(), 2);
    assert!(matches!(second.events(), [CommittableEvent::Checkpoint]));
    assert_eq!(
        subscription.commit_durable(&first),
        Err(SubscriptionError::CommitMismatch {
            expected: second.sequence(),
            observed: first.sequence(),
        })
    );
    subscription
        .commit_durable(&second)
        .expect("checkpoint commits");

    let gap = match subscription.next_item().expect("gap item") {
        Some(SubscriptionItem::Batch(delivery)) => delivery,
        other => panic!("expected gap delivery, found {other:?}"),
    };
    assert!(gap
        .events()
        .iter()
        .any(CommittableEvent::requires_reconciliation));
    subscription.commit_durable(&gap).expect("gap commits");
    assert!(matches!(
        subscription.next_item().expect("heartbeat item"),
        Some(SubscriptionItem::Heartbeat(heartbeat)) if heartbeat.sequence().get() == 4
    ));
    assert_eq!(subscription.next_item().expect("clean end"), None);
    assert_eq!(subscription.next_item().expect("stable clean end"), None);
}

#[test]
fn statically_linked_backend_obeys_conformance_contract() {
    let mut backend = FakeBackend::conformance("fake-static");
    exercise(&mut backend, "fake-static");
    let observation = backend.observation();
    assert_eq!(observation.receive_calls, 5);
    assert_eq!(
        observation.acknowledged_delivery_ids,
        ["delivery-1", "delivery-2", "delivery-3"]
    );
    assert_eq!(
        observation.subscriptions,
        [FakeSubscriptionAuthority {
            channel_ids: vec!["channels/conformance".to_owned()],
            allowed_senders: vec!["users/owner".to_owned()],
            resume_from: Some("durable-before".to_owned()),
            backend_configuration: request().backend_configuration().cloned(),
        }]
    );
}

fn timeouts() -> ProcessPhaseTimeouts {
    ProcessPhaseTimeouts::new(
        Duration::from_millis(500),
        Duration::from_millis(200),
        Duration::from_millis(200),
        Duration::from_millis(200),
        Duration::from_millis(200),
    )
    .expect("fixture deadlines are valid")
}

fn spawn(stall: Option<&str>) -> ProcessPluginChild {
    let mut command = Command::new(env!("CARGO_BIN_EXE_chat-subscription-fake-plugin"));
    if let Some(mode) = stall {
        command.env("CHAT_SUBSCRIPTION_FAKE_STALL", mode);
    }
    ProcessPluginChild::spawn(command).expect("fake plugin starts")
}

fn assert_reaped(process_id: u32) {
    assert!(
        !PathBuf::from(format!("/proc/{process_id}")).exists(),
        "plugin leader {process_id} was not reaped"
    );
}

fn assert_bounded(started: Instant) {
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "bounded process operation took {:?}",
        started.elapsed()
    );
}

fn backend_failure_code(error: SubscriptionError) -> String {
    backend_failure(error).code().to_owned()
}

fn backend_failure(error: SubscriptionError) -> BackendFailure {
    match error {
        SubscriptionError::Backend(error) => error,
        other => panic!("expected backend failure, found {other:?}"),
    }
}

#[test]
fn out_of_process_plugin_obeys_same_conformance_contract() {
    let child = spawn(None);
    let process_id = child.id();
    let (mut backend, cancellation) = child.connect(timeouts()).expect("plugin handshake");
    exercise(&mut backend, "fake-process-plugin");
    drop(backend);
    let status = cancellation.cancel().expect("plugin is joined and reaped");
    assert!(status.success(), "fake plugin exited with {status}");
    assert_reaped(process_id);
}

#[test]
fn stalled_hello_is_bounded_and_reaped() {
    let child = spawn(Some("hello"));
    let process_id = child.id();
    let started = Instant::now();
    let error = match child.connect(timeouts()) {
        Ok(_) => panic!("a plugin that never answers Hello must time out"),
        Err(error) => error,
    };
    assert_eq!(error.code(), "plugin_hello_timeout");
    assert_bounded(started);
    assert_reaped(process_id);
}

#[test]
fn stalled_start_is_bounded_and_reaped() {
    let child = spawn(Some("start"));
    let process_id = child.id();
    let (mut backend, cancellation) = child.connect(timeouts()).expect("Hello succeeds");
    let started = Instant::now();
    let error = ChatSubscription::open(&mut backend, &maximum_authority_request())
        .err()
        .expect("a plugin that never reads the maximum Start frame must time out");
    assert_eq!(backend_failure_code(error), "plugin_start_timeout");
    assert_bounded(started);
    assert!(!cancellation
        .cancel()
        .expect("stalled plugin is reaped")
        .success());
    assert_reaped(process_id);
}

#[test]
fn stalled_commit_is_bounded_and_reaped() {
    let child = spawn(Some("commit"));
    let process_id = child.id();
    let (mut backend, cancellation) = child.connect(timeouts()).expect("Hello succeeds");
    let mut subscription =
        ChatSubscription::open(&mut backend, &request()).expect("Start succeeds");
    let delivery = match subscription.next_item().expect("fixture item") {
        Some(SubscriptionItem::Batch(delivery)) => delivery,
        other => panic!("expected a delivery, found {other:?}"),
    };
    let started = Instant::now();
    let error = subscription
        .commit_durable(&delivery)
        .expect_err("a plugin that never confirms Commit must time out");
    assert_eq!(backend_failure_code(error), "commit_outcome_unknown");
    assert_bounded(started);
    drop(subscription);
    assert!(!cancellation
        .cancel()
        .expect("stalled plugin is reaped")
        .success());
    assert_reaped(process_id);
}

#[test]
fn protocol_end_with_nonzero_exit_is_failure_and_reaped() {
    let child = spawn(Some("end-exit-7"));
    let process_id = child.id();
    let (mut backend, cancellation) = child.connect(timeouts()).expect("Hello succeeds");
    let mut subscription =
        ChatSubscription::open(&mut backend, &request()).expect("Start succeeds");
    let started = Instant::now();
    let error = subscription
        .next_item()
        .expect_err("End followed by exit 7 is not clean termination");
    let error = backend_failure(error);
    assert_eq!(error.code(), "plugin_unclean_end");
    assert!(error.detail().contains("exit status: 7"), "{error}");
    assert_bounded(started);
    assert!(!cancellation
        .cancel()
        .expect("nonzero plugin is reaped")
        .success());
    assert_reaped(process_id);
}

#[test]
fn protocol_end_requiring_forced_kill_is_failure_and_reaped() {
    let child = spawn(Some("end-stall"));
    let process_id = child.id();
    let (mut backend, cancellation) = child.connect(timeouts()).expect("Hello succeeds");
    let mut subscription =
        ChatSubscription::open(&mut backend, &request()).expect("Start succeeds");
    let started = Instant::now();
    let error = subscription
        .next_item()
        .expect_err("End from a plugin that stays alive is not clean termination");
    let error = backend_failure(error);
    assert_eq!(error.code(), "plugin_unclean_end");
    assert!(error.detail().contains("signal: 9"), "{error}");
    assert_bounded(started);
    assert!(!cancellation
        .cancel()
        .expect("forced plugin is reaped")
        .success());
    assert_reaped(process_id);
}

#[test]
fn stalled_close_is_bounded_and_reaped() {
    let child = spawn(Some("close"));
    let process_id = child.id();
    let (mut backend, cancellation) = child.connect(timeouts()).expect("Hello succeeds");
    let mut subscription =
        ChatSubscription::open(&mut backend, &request()).expect("Start succeeds");
    let started = Instant::now();
    let error = subscription
        .close()
        .expect_err("a plugin that ignores Close must be forcibly stopped");
    assert_eq!(backend_failure_code(error), "plugin_forced_shutdown");
    assert_bounded(started);
    assert!(!cancellation
        .cancel()
        .expect("stalled plugin is reaped")
        .success());
    assert_reaped(process_id);
}

#[test]
fn stalled_close_before_start_is_bounded_and_reaped() {
    let child = spawn(Some("close-before-start"));
    let process_id = child.id();
    let (mut backend, cancellation) = child.connect(timeouts()).expect("Hello succeeds");
    let started = Instant::now();
    let error = backend
        .close()
        .expect_err("a negotiated plugin that ignores Close must be forcibly stopped");
    assert_eq!(error.code(), "plugin_forced_shutdown");
    assert_bounded(started);
    assert!(!cancellation
        .cancel()
        .expect("stalled plugin is reaped")
        .success());
    assert_reaped(process_id);
}

#[test]
fn independent_cancellation_interrupts_blocking_next_item_and_joins_worker() {
    let child = spawn(Some("close"));
    let process_id = child.id();
    let (mut backend, cancellation) = child.connect(timeouts()).expect("Hello succeeds");
    let mut subscription =
        ChatSubscription::open(&mut backend, &request()).expect("Start succeeds");
    let (result_sender, result_receiver) = mpsc::channel();
    let receiver_thread = thread::spawn(move || {
        let result = subscription.next_item();
        let _ = result_sender.send(result);
    });
    thread::sleep(Duration::from_millis(50));
    let started = Instant::now();
    let status = cancellation
        .cancel()
        .expect("independent cancellation joins and reaps");
    assert!(!status.success());
    let result = result_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("blocked next_item returns after cancellation");
    assert!(result.is_err());
    receiver_thread.join().expect("receiver thread joins");
    assert_bounded(started);
    assert_reaped(process_id);
}
