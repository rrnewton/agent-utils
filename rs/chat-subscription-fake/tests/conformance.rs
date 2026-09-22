//! Shared conformance suite for direct and out-of-process fake backends.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use chat_subscription::{
    BackendConfiguration, ChannelId, ChatSubscription, ChatSubscriptionBackend, CommittableEvent,
    EventKind, ProviderCursor, SenderId, SubscribeRequest, SubscriptionError, SubscriptionItem,
};
use chat_subscription_fake::{FakeBackend, FakeSubscriptionAuthority};
use chat_subscription_plugin::PluginBackend;

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

struct ChildGuard {
    child: Child,
    reaped: bool,
}

impl ChildGuard {
    fn spawn() -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_chat-subscription-fake-plugin"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("fake plugin starts");
        Self {
            child,
            reaped: false,
        }
    }

    fn wait_success(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().expect("query plugin exit") {
                self.reaped = true;
                assert!(status.success(), "fake plugin exited with {status}");
                return;
            }
            assert!(
                Instant::now() < deadline,
                "fake plugin did not exit in time"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        let _ = self.child.kill();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    self.reaped = true;
                    return;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => return,
            }
        }
        eprintln!("fake plugin could not be reaped within two seconds after kill");
    }
}

#[test]
fn out_of_process_plugin_obeys_same_conformance_contract() {
    let mut child = ChildGuard::spawn();
    let stdout = child.child.stdout.take().expect("plugin stdout");
    let stdin = child.child.stdin.take().expect("plugin stdin");
    let mut backend = PluginBackend::connect(stdout, stdin).expect("plugin handshake");
    exercise(&mut backend, "fake-process-plugin");
    drop(backend);
    child.wait_success(Duration::from_secs(5));
}
