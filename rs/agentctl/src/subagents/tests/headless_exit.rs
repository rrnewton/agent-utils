//! `#2248 agentctl-headless-wrapper-keeps-harness-stderr`: what a headless harness says on
//! stderr before it exits reaches the exit event and `agentctl status`, so an exit-2 usage
//! error is visible without running the harness by hand.

use super::*;
use crate::headless::{events, wrapper};
use std::sync::atomic::AtomicBool;

#[test]
fn a_harness_usage_error_on_stderr_shows_in_the_exit_event_and_status() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    manager
        .start_headless(
            "lead",
            &fixture.root,
            HeadlessStart {
                harness: "claude".to_owned(),
                arguments: vec!["--permission-mode=bypassPermissions".to_owned()],
                launch: false,
                ..HeadlessStart::default()
            },
        )
        .unwrap();
    let directory = manager.directory("lead").unwrap();
    let session = manager.status("lead").unwrap()["session"]
        .as_str()
        .unwrap()
        .to_owned();
    // A fake harness that fails the way `muse serve` did on an exec-only flag (#2247), with
    // a credential on stderr that must not reach the log.
    let mut config = wrapper::WrapperConfig::new("lead", &directory, &fixture.root, &session);
    config.command = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        "printf 'muse serve: --yolo is not a serve option\\n\\nusage: muse serve [OPTIONS]\\nretry with token=hunter2\\n' >&2; exit 2"
            .to_owned(),
    ];
    config.max_restarts = 1;
    config.startup_grace = Duration::ZERO;
    let code = wrapper::run(config, Arc::new(AtomicBool::new(false)), &wrapper::NoHooks).unwrap();
    assert_eq!(code, 1, "the wrapper gives up after its restart limit");
    let expected = json!({
        "lines": [
            "muse serve: --yolo is not a serve option",
            "usage: muse serve [OPTIONS]",
            "retry with token=***",
        ],
        "omitted_lines": 0,
    });
    let exits: Vec<events::Event> = events::read_all(&events::directory(&directory))
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "lifecycle.harness_exited")
        .collect();
    assert_eq!(exits.len(), 2, "{exits:?}");
    for exit in &exits {
        assert_eq!(exit.data["code"], 2, "{exit:?}");
        assert_eq!(exit.data["stderr_tail"], expected, "{exit:?}");
    }
    let status = manager.status("lead").unwrap();
    let last = &status["last_harness_exit"];
    assert_eq!(last["seq"], exits[1].seq, "{status}");
    assert_eq!(last["code"], 2, "{status}");
    assert!(
        last["next"]
            .as_str()
            .unwrap()
            .starts_with("giving up after 1 restarts"),
        "{status}"
    );
    assert_eq!(last["stderr_tail"], expected, "{status}");
}

#[test]
fn status_has_no_harness_exit_before_any_ran() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    manager
        .start_headless(
            "lead",
            &fixture.root,
            HeadlessStart {
                harness: "claude".to_owned(),
                arguments: vec!["--permission-mode=bypassPermissions".to_owned()],
                launch: false,
                ..HeadlessStart::default()
            },
        )
        .unwrap();
    assert_eq!(
        manager.status("lead").unwrap()["last_harness_exit"],
        Value::Null
    );
}
