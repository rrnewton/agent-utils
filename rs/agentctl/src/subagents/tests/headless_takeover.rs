//! `#591 headless-takeover`: while a headless agent is taken over, nothing else may run its
//! session, and every command says why.

use super::*;
use crate::headless::{events, wrapper};

fn headless_agent(fixture: &Fixture) -> ManagedAgents<'_, Fake> {
    let manager = fixture.manager();
    manager
        .start_headless(
            "lead",
            &fixture.root,
            HeadlessStart {
                harness: "claude".to_owned(),
                launch: false,
                ..HeadlessStart::default()
            },
        )
        .unwrap();
    manager
}

/// Mark the record taken over, as `takeover` does before it stops the wrapper.
fn mark_taken_over(manager: &ManagedAgents<'_, Fake>) {
    let mut record = manager.load("lead").unwrap();
    record.extra.get_mut("headless").unwrap()["takeover"] = json!({
        "schema": "agentctl-headless-takeover/v1", "token": "t-1", "state": "interactive",
        "tab": "w9:t9", "pane": "w9:p9",
    });
    manager.save(&record).unwrap();
}

#[test]
fn a_taken_over_agent_refuses_a_wrapper_a_stop_and_a_second_takeover() {
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    mark_taken_over(&manager);
    // A wrapper exits 0 at once (a supervisor must not loop), without starting a harness: the
    // executable named here does not exist, so starting one would fail.
    let terminate = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let code = manager
        .run_headless(
            "lead",
            Some(Path::new("/nonexistent/claude")),
            None,
            terminate,
        )
        .unwrap();
    assert_eq!(code, 0);
    let directory = manager.directory("lead").unwrap();
    assert!(
        !events::directory(&directory).exists(),
        "no wrapper generation was opened"
    );
    let status = manager.status("lead").unwrap();
    assert_eq!(status["agent_status"], "taken-over", "{status}");
    assert_eq!(status["takeover"]["tab"], "w9:t9");
    let error = manager.stop("lead").unwrap_err().to_string();
    assert!(
        error.contains("taken over") && error.contains("handback"),
        "{error}"
    );
    let options = TakeoverOptions {
        workspace: Some("w9".to_owned()),
        interrupt: false,
        wait: Duration::from_secs(1),
        claude_bin: None,
    };
    let error = manager
        .takeover_headless("lead", &options)
        .unwrap_err()
        .to_string();
    assert!(error.contains("already taken over"), "{error}");
}

#[test]
fn handback_waits_for_the_session_lock_and_leaves_the_mark_while_it_is_held() {
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    mark_taken_over(&manager);
    let directory = manager.directory("lead").unwrap();
    // The interactive session holds the lock (here this test process does).
    let held = wrapper::try_lock(&directory).unwrap().expect("lock");
    let error = manager
        .handback_headless("lead", Duration::from_secs(1))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("still running") && error.contains("held by its holder"),
        "{error}"
    );
    assert_eq!(
        manager.status("lead").unwrap()["agent_status"],
        "taken-over"
    );
    drop(held);
}

#[test]
fn takeover_refuses_before_touching_anything_without_a_workspace_or_a_transcript() {
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    let mut options = TakeoverOptions {
        workspace: None,
        interrupt: false,
        wait: Duration::from_secs(1),
        claude_bin: None,
    };
    let error = manager
        .takeover_headless("lead", &options)
        .unwrap_err()
        .to_string();
    assert!(error.contains("workspace"), "{error}");
    options.workspace = Some("w9".to_owned());
    let error = manager
        .takeover_headless("lead", &options)
        .unwrap_err()
        .to_string();
    assert!(error.contains("nothing to resume"), "{error}");
    let status = manager.status("lead").unwrap();
    assert_ne!(status["agent_status"], "taken-over", "{status}");
    assert!(status["takeover"].is_null(), "{status}");
}

#[test]
fn session_processes_finds_a_writer_by_its_command_line_whatever_started_it() {
    let session = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
    assert!(wrapper::session_processes(session).is_empty());
    // A process whose arguments run Claude Code on the session, as a launcher's native child
    // left behind would: `sh -c CMD $0 $1 $2` keeps the extra words in its command line.
    let mut child = std::process::Command::new("sh")
        .args(["-c", "sleep 30; :", "claude", "--resume", session])
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while wrapper::session_processes(session).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(wrapper::session_processes(session), vec![child.id()]);
    assert!(wrapper::session_processes("0a1b2c3d-4e5f-4a6b-8c7d-000000000000").is_empty());
    wrapper::end_processes(&[child.id()], Duration::from_secs(2));
    child.wait().unwrap();
    assert!(wrapper::session_processes(session).is_empty());
}

#[test]
fn argv_names_session_needs_the_flag_and_the_exact_id() {
    let id = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
    assert!(wrapper::argv_names_session(&["claude", "--resume", id], id));
    assert!(wrapper::argv_names_session(
        &["node", "cli.js", "--session-id", id, "-p"],
        id
    ));
    assert!(wrapper::argv_names_session(
        &["claude", &format!("--resume={id}")],
        id
    ));
    assert!(!wrapper::argv_names_session(&["grep", id, "log"], id));
    assert!(!wrapper::argv_names_session(
        &["claude", "--resume", &id[..30]],
        id
    ));
}
