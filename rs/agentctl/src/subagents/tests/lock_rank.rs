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
        std::thread::sleep(Duration::from_millis(100));
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
        std::thread::sleep(Duration::from_millis(125));

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
