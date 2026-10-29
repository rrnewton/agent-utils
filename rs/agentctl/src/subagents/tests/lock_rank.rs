//! #556 chat-delivery-latency: a priority manager (the chat bridge) takes an agent's lifecycle
//! lock before ordinary senders that were already waiting, and every waiter is visible.

use super::*;
use std::sync::Mutex as StdMutex;

/// Real time, but polling at `every`, so a slow poller can only win by rank.
struct Paced {
    every: Duration,
}

impl agent::AgentRuntime for Paced {
    fn monotonic(&self) -> Duration {
        Duration::ZERO
    }
    fn sleep(&self, _duration: Duration) {
        std::thread::sleep(self.every);
    }
}

/// Wait (at most 30 s) until `count` senders wait for `worker`'s lifecycle lock.
fn waiters_reach(registry: &Path, count: usize) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while crate::lock_wait::contention(registry, "worker")["waiters"]
        .as_array()
        .map_or(0, Vec::len)
        < count
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_priority_manager_takes_a_contended_lifecycle_lock_first_and_waiters_are_visible() {
    let fixture = Fixture::new();
    let registry = fixture.root.join("registry");
    let holder = fixture
        .manager()
        .lock("worker")
        .expect("first lock is free");

    let order = StdMutex::new(Vec::new());
    std::thread::scope(|scope| {
        for name in ["agent-a", "agent-b", "agent-c"] {
            let (fixture, order) = (&fixture, &order);
            scope.spawn(move || {
                let fast = Paced {
                    every: Duration::from_millis(10),
                };
                let _lock = fixture
                    .manager()
                    .lock_with_runtime("worker", &fast)
                    .unwrap();
                order.lock().unwrap().push(name);
                std::thread::sleep(Duration::from_millis(30));
            });
        }
        // Wait until every ordinary sender is visibly waiting, however loaded the host is.
        waiters_reach(&registry, 3);
        let (fixture, order) = (&fixture, &order);
        scope.spawn(move || {
            let slow = Paced {
                every: Duration::from_millis(50),
            };
            let _lock = fixture
                .manager()
                .with_lock_priority()
                .lock_with_runtime("worker", &slow)
                .unwrap();
            order.lock().unwrap().push("bridge");
            std::thread::sleep(Duration::from_millis(30));
        });
        waiters_reach(&registry, 4);

        let report = crate::lock_wait::contention(&registry, "worker");
        let waiters = report["waiters"].as_array().expect("waiters");
        assert_eq!(waiters.len(), 4, "{report}");
        assert_eq!(
            waiters
                .iter()
                .filter(|waiter| waiter["priority"] == true)
                .count(),
            1
        );
        assert_eq!(report["holder"]["pid"], json!(std::process::id()));
        drop(holder);
    });
    let order = order.into_inner().unwrap();
    assert_eq!(order.first(), Some(&"bridge"), "{order:?}");
    assert_eq!(order.len(), 4);
    assert_eq!(
        crate::lock_wait::contention(&registry, "worker")["waiters"],
        json!([])
    );
}

#[test]
fn a_read_back_asks_a_pane_that_keeps_no_scrollback_for_its_visible_rows_only() {
    let scroll = |max_offset_from_bottom, viewport_rows| {
        Some(crate::client::PaneScroll {
            offset_from_bottom: 0,
            max_offset_from_bottom,
            viewport_rows,
        })
    };
    assert_eq!(scrollback_lines(scroll(0, 89)), 89);
    assert_eq!(scrollback_lines(scroll(0, 1_000)), SCROLLBACK_LINES);
    // Scrollback can grow between `pane get` and the read: the full window.
    assert_eq!(scrollback_lines(scroll(12, 89)), SCROLLBACK_LINES);
    assert_eq!(scrollback_lines(scroll(0, 0)), SCROLLBACK_LINES);
    assert_eq!(scrollback_lines(None), SCROLLBACK_LINES);
}

/// #702 send-hard-timeout: a send whose agent stays locked by another operation for its whole
/// deadline gives up with exit 75, queues nothing, and holds no lock afterwards.
#[test]
fn a_send_gives_up_at_its_deadline_while_the_agent_stays_locked_and_queues_nothing() {
    let fixture = Fixture::new();
    fixture.start(None);
    let manager = fixture.manager();
    let queued_before = agent::queued_prompts(&manager.queue("worker").unwrap()).unwrap();
    let holder = manager.lock("worker").expect("first lock is free");
    let started = std::time::Instant::now();
    let error = manager
        .send_identified(
            "worker",
            "never typed",
            DrainOptions {
                deadline: Some(Duration::from_millis(300)),
                ..DrainOptions::default()
            },
            Some("hard-deadline-lock"),
        )
        .unwrap_err();
    let elapsed = started.elapsed();
    assert!(matches!(error, AgentError::Pending(_)), "{error}");
    assert_eq!(error.exit_code(), 75);
    assert!(error.to_string().contains("nothing was queued"), "{error}");
    assert!(error.to_string().contains("send deadline"), "{error}");
    assert!(elapsed < Duration::from_secs(20), "{elapsed:?}");
    assert_eq!(
        agent::queued_prompts(&manager.queue("worker").unwrap()).unwrap(),
        queued_before
    );
    drop(holder);
    // The send that gave up holds nothing: the lock is free at once.
    let system = agent::SystemRuntime::default();
    let free = agent::UntilRuntime::after(&system, Duration::from_secs(5));
    let _free = manager
        .lock_with_runtime("worker", &free)
        .expect("lock free after the send gave up");
}

/// #702 send-hard-timeout: a send whose pane never becomes ready gives up at its deadline, not
/// at its much longer readiness timeout; the prompt stays pending, safe to send again, and every
/// lock is released, so the next send is not blocked.
#[test]
fn a_send_whose_pane_never_becomes_ready_gives_up_at_its_deadline_and_releases_every_lock() {
    let fixture = Fixture::new();
    fixture.start(None);
    *fixture.client.status.lock().unwrap() = Some("working".to_owned());
    let manager = fixture.manager();
    let started = std::time::Instant::now();
    // A slash command waits for an idle pane, even for a Codex that takes follow-ups.
    let error = manager
        .send_identified(
            "worker",
            "/status",
            DrainOptions {
                ready_timeout: Duration::from_secs(900),
                deadline: Some(Duration::from_millis(400)),
                ..DrainOptions::default()
            },
            Some("hard-deadline-ready"),
        )
        .unwrap_err();
    let elapsed = started.elapsed();
    assert!(matches!(error, AgentError::Pending(_)), "{error}");
    assert_eq!(error.exit_code(), 75);
    assert!(error.to_string().contains("the send deadline"), "{error}");
    assert!(error.to_string().contains("holding no lock"), "{error}");
    assert!(elapsed < Duration::from_secs(20), "{elapsed:?}");
    let queue = manager.queue("worker").unwrap();
    assert_eq!(
        agent::message_state(&queue, "hard-deadline-ready").unwrap(),
        Some(agent::QueueMessageState::Pending)
    );
    assert!(fixture.client.runs.lock().unwrap().is_empty());
    let system = agent::SystemRuntime::default();
    let free = agent::UntilRuntime::after(&system, Duration::from_secs(5));
    let _free = manager
        .lock_with_runtime("worker", &free)
        .expect("agent lock free after the deadline");
    let queue_lock = agent::open_private_lock(&queue.join(".delivery.lock"), "queue lock").unwrap();
    assert!(
        fs2::FileExt::try_lock_exclusive(&queue_lock).is_ok(),
        "queue delivery lock free after the deadline"
    );
}

/// #702 send-hard-timeout: the deadline is the call's own when it sets one, else the registry's
/// settings.json `send_deadline_seconds`, else 10 minutes.
#[test]
fn the_send_deadline_is_the_calls_then_the_registrys_then_ten_minutes() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    let registry = fixture.root.join("registry");
    assert_eq!(manager.send_deadline(), Duration::from_secs(600));
    assert_eq!(
        manager.with_send_deadline(DrainOptions::default()).deadline,
        Some(Duration::from_secs(600))
    );
    fs::create_dir_all(&registry).unwrap();
    fs::write(
        registry.join("settings.json"),
        r#"{"send_deadline_seconds": 300}"#,
    )
    .unwrap();
    assert_eq!(
        manager.with_send_deadline(DrainOptions::default()).deadline,
        Some(Duration::from_secs(300))
    );
    let call = DrainOptions {
        deadline: Some(Duration::from_secs(5)),
        ..DrainOptions::default()
    };
    assert_eq!(
        manager.with_send_deadline(call).deadline,
        Some(Duration::from_secs(5))
    );
}

/// #703 send-lock-free-wait: a send waiting for its pane to become ready holds no lock, so
/// another operation (here a plain lifecycle lock, as `stop` or the chat bridge takes) gets the
/// agent meanwhile; once the pane is idle the waiting send types its prompt.
#[test]
fn a_send_waiting_for_readiness_holds_no_lock_and_types_once_the_pane_is_idle() {
    let fixture = Fixture::new();
    fixture.start(None);
    *fixture.client.status.lock().unwrap() = Some("working".to_owned());
    let runs_before = fixture.client.runs.lock().unwrap().len();
    std::thread::scope(|scope| {
        let sender = scope.spawn(|| {
            fixture.manager().send_identified(
                "worker",
                "/status",
                DrainOptions {
                    ready_timeout: Duration::from_secs(60),
                    working_timeout: Duration::from_millis(50),
                    deadline: Some(Duration::from_secs(60)),
                    ..DrainOptions::default()
                },
                Some("lock-free-wait"),
            )
        });
        // Wait until the prompt is queued and the first probe has found the pane busy.
        let queue = fixture.manager().queue("worker").unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while agent::message_state(&queue, "lock-free-wait").unwrap()
            != Some(agent::QueueMessageState::Pending)
            && std::time::Instant::now() < until
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(100));
        let system = agent::SystemRuntime::default();
        let bounded = agent::UntilRuntime::after(&system, Duration::from_secs(5));
        let other = fixture
            .manager()
            .lock_with_runtime("worker", &bounded)
            .expect("the waiting send holds no lock");
        drop(other);
        assert!(!sender.is_finished());
        *fixture.client.status.lock().unwrap() = Some("idle".to_owned());
        let result = sender.join().unwrap().expect("typed once the pane is idle");
        assert_eq!(result.message_id, "lock-free-wait");
        assert!(result.delivered.contains(&"lock-free-wait".to_owned()));
    });
    assert!(fixture.client.runs.lock().unwrap().len() > runs_before);
}
