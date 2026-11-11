//! `#591 headless-takeover`: while a headless agent is taken over, nothing else may run its
//! session, and every command says why.

use super::*;
use crate::headless::{events, wrapper};
use crate::subagents::headless::stop_would_end_caller;

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

/// A process whose command line runs Claude Code on `session`, in a Linux session of its own
/// when `own_session` (as an interactive Claude in another terminal is), else in this one.
fn session_writer(session: &str, own_session: bool) -> std::process::Child {
    let mut command = std::process::Command::new("sh");
    command.args(["-c", "sleep 30; :", "claude", "--resume", session]);
    if own_session {
        // SAFETY: setsid is async-signal-safe and touches no memory of this process.
        unsafe {
            std::os::unix::process::CommandExt::pre_exec(&mut command, || {
                libc::setsid();
                Ok(())
            });
        }
    }
    let child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !wrapper::process_runs_session(child.id(), session) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(wrapper::process_runs_session(child.id(), session));
    child
}

fn reap(mut children: Vec<std::process::Child>) {
    for child in &mut children {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[test]
fn a_stopping_wrapper_ends_only_writers_of_its_own_session_that_no_other_record_pins() {
    // #778 agentctl-stop-kills-caller: an interactive Claude resumed on the headless session
    // in another terminal is not the wrapper's to end, nor is another live record's harness.
    let session = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c40";
    let registry = std::env::temp_dir().join(format!("agentctl-778-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&registry);
    let own = registry.join("agents").join("lead");
    std::fs::create_dir_all(&own).unwrap();
    let elsewhere = session_writer(session, true);
    let leftover = session_writer(session, false);
    let pinned = session_writer(session, false);
    let other = registry.join("agents").join("coordinator");
    std::fs::create_dir_all(&other).unwrap();
    let ticks = agent_queue::identity::read_process_stat(u64::from(pinned.id()))
        .unwrap()
        .starttime_ticks;
    std::fs::write(
        other.join("agent.json"),
        json!({"name": "coordinator", "lifecycle": "running",
               "harness_identity": {"pid": pinned.id(), "starttime_ticks": ticks}})
        .to_string(),
    )
    .unwrap();
    let writers = wrapper::own_session_writers(session, &own, "lead");
    assert_eq!(
        writers,
        vec![leftover.id()],
        "elsewhere {} pinned {}",
        elsewhere.id(),
        pinned.id()
    );
    // A pin from an earlier process generation does not protect a different process.
    std::fs::write(
        other.join("agent.json"),
        json!({"name": "coordinator", "lifecycle": "running",
               "harness_identity": {"pid": pinned.id(), "starttime_ticks": ticks + 1}})
        .to_string(),
    )
    .unwrap();
    let mut writers = wrapper::own_session_writers(session, &own, "lead");
    writers.sort_unstable();
    let mut expected = vec![leftover.id(), pinned.id()];
    expected.sort_unstable();
    assert_eq!(writers, expected);
    reap(vec![elsewhere, leftover, pinned]);
    let _ = std::fs::remove_dir_all(&registry);
}

#[test]
fn stop_refuses_when_the_wrapper_would_end_a_process_running_the_command() {
    let session = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c41";
    // The caller is a Claude Code resumed on the headless session in another terminal, as in
    // the incident; the stop command runs below it.
    let caller = session_writer(session, true);
    let chain = wrapper::process_chain(caller.id());
    let fixed = json!({"ok": true, "writer_sweep": wrapper::WRITER_SWEEP});
    assert_eq!(stop_would_end_caller(&fixed, session, &chain), None);
    // A wrapper built before #778 ends every writer of the session on the host.
    let old = json!({"ok": true});
    let reason = stop_would_end_caller(&old, session, &chain).unwrap();
    assert!(reason.contains("older build"), "{reason}");
    assert!(reason.contains(&caller.id().to_string()), "{reason}");
    // Not running the session itself: nothing of the caller is a writer.
    assert_eq!(
        stop_would_end_caller(&old, "0a1b2c3d-4e5f-4a6b-8c7d-000000000000", &chain),
        None
    );
    // A caller in the wrapper's own session (the harness stopping its own agent), or the
    // harness itself, is refused whatever the wrapper's build.
    let sibling = session_writer("0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c42", false);
    let own_chain = wrapper::process_chain(std::process::id());
    let same =
        json!({"ok": true, "wrapper_pid": sibling.id(), "writer_sweep": wrapper::WRITER_SWEEP});
    let reason = stop_would_end_caller(&same, session, &own_chain).unwrap();
    assert!(reason.contains("wrapper's own session"), "{reason}");
    let harness =
        json!({"ok": true, "harness_pid": caller.id(), "writer_sweep": wrapper::WRITER_SWEEP});
    let reason = stop_would_end_caller(&harness, session, &chain).unwrap();
    assert!(reason.contains("own wrapper or harness"), "{reason}");
    reap(vec![caller, sibling]);
}

#[test]
fn stop_of_a_headless_agent_never_asks_the_wrapper_to_stop_when_that_would_end_the_caller() {
    use std::io::{BufRead, Write};
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    let directory = manager.directory("lead").unwrap();
    // A wrapper stand-in in this test's own session: stopping it would end this process.
    let sibling = session_writer("0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c43", false);
    let listener =
        std::os::unix::net::UnixListener::bind(crate::headless::control::socket_path(&directory))
            .unwrap();
    let wrapper_pid = sibling.id();
    let server = std::thread::spawn(move || {
        let mut ops = Vec::new();
        listener.set_nonblocking(false).unwrap();
        for stream in listener.incoming().take(1) {
            let mut stream = stream.unwrap();
            let mut line = String::new();
            std::io::BufReader::new(&stream)
                .read_line(&mut line)
                .unwrap();
            let body: Value = serde_json::from_str(line.trim()).unwrap();
            ops.push(body["op"].as_str().unwrap().to_owned());
            let answer = json!({"ok": true, "wrapper_pid": wrapper_pid,
                                "writer_sweep": wrapper::WRITER_SWEEP});
            writeln!(stream, "{answer}").unwrap();
        }
        ops
    });
    let error = manager.stop("lead").unwrap_err().to_string();
    assert!(error.contains("refusing to stop \"lead\""), "{error}");
    assert!(error.contains("wrapper's own session"), "{error}");
    assert_eq!(
        server.join().unwrap(),
        vec!["status".to_owned()],
        "stop must not be sent"
    );
    assert_eq!(manager.load("lead").unwrap().lifecycle, "running");
    reap(vec![sibling]);
}

#[test]
fn run_headless_spawns_the_launcher_as_given_and_pins_the_post_exec_identity() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    // A fake harness that announces init, then reads input until EOF.
    let fake = fixture.root.join("fake_harness.py");
    fs::write(
        &fake,
        "import json, sys\n\
         sys.stdout.write(json.dumps({\"type\": \"system\", \"subtype\": \"init\", \"claude_code_version\": \"9.9.9\", \"model\": \"fake\", \"permissionMode\": \"dontAsk\", \"session_id\": \"s\", \"tools\": [\"Bash\"]}) + \"\\n\")\n\
         sys.stdout.flush()\n\
         for _line in sys.stdin:\n    pass\n",
    )
    .unwrap();
    // A launcher like the host's: an export, a brief sleep, then exec.
    let launcher = fixture.root.join("claude-launcher");
    fs::write(
        &launcher,
        format!(
            "#!/bin/sh\nexport MCP_TIMEOUT=60000\nsleep 0.3\nexec python3 {} \"$@\"\n",
            fake.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)).unwrap();
    // A previous run's identity must not survive this spawn.
    let mut stale_child = std::process::Command::new("/bin/sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let stale = agent_queue::identity::observe(u64::from(stale_child.id()))
        .unwrap()
        .identity;
    stale_child.kill().unwrap();
    stale_child.wait().unwrap();
    let mut record = manager.load("lead").unwrap();
    record.harness_identity = Some(stale.clone());
    manager.save(&record).unwrap();

    let terminate = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = terminate.clone();
    // No assertion inside the scope before the flag is set and the wrapper
    // thread joined: a panic there would leave the scope waiting on a
    // wrapper that runs forever, masking the real failure as a hang.
    // Liveness and environ are captured while the harness lives, inside
    // the scope but without asserting there.
    let (code, pinned, last, live, environ, harness_pid) = std::thread::scope(|scope| {
        let run = scope
            .spawn(|| manager.run_headless("lead", Some(&launcher), None, None, terminate.clone()));
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut pinned = None;
        let mut last = None;
        let mut live = None;
        let mut environ = Vec::new();
        let mut harness_pid = None;
        while std::time::Instant::now() < deadline {
            let current = manager.load("lead").unwrap().harness_identity;
            if let Some(identity) = &current {
                if *identity != stale {
                    live = Some(agent_queue::identity::liveness(identity));
                    if live == Some(agent_queue::identity::ProcessLiveness::Alive) {
                        environ =
                            fs::read(format!("/proc/{}/environ", identity.pid)).unwrap_or_default();
                        if let Ok(directory) = manager.directory("lead") {
                            harness_pid = fs::read(directory.join("harness.json"))
                                .ok()
                                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                                .and_then(|doc| doc["identity"]["pid"].as_u64());
                        }
                        if harness_pid == Some(identity.pid) {
                            pinned = Some(identity.clone());
                            break;
                        }
                    }
                }
            }
            last = current;
            std::thread::sleep(Duration::from_millis(50));
        }
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        (
            run.join().unwrap(),
            pinned,
            last,
            live,
            environ,
            harness_pid,
        )
    });
    let code = code.unwrap();
    assert_eq!(code, 0);
    let pinned = pinned.unwrap_or_else(|| {
        panic!("post-exec identity was not pinned live; last: {last:?}, last liveness: {live:?}")
    });
    assert_eq!(
        live,
        Some(agent_queue::identity::ProcessLiveness::Alive),
        "pinned identity {pinned:?} must be the live post-exec harness"
    );
    // The launcher's export reached the harness, and the pin names it.
    assert!(
        environ
            .split(|byte| *byte == 0)
            .any(|entry| entry == b"MCP_TIMEOUT=60000"),
        "launcher export must reach the harness"
    );
    // Captured while the harness lived: stopping the wrapper clears it.
    assert_eq!(harness_pid, Some(pinned.pid));
}

#[test]
fn a_raw_bridge_send_to_a_headless_agent_keeps_its_raw_mark() {
    // #1223 bridge-raw-headless: the raw flag must survive the headless
    // enqueue, as the one shared raw-item definition the console uses.
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    let record = manager.load("lead").unwrap();
    let options = DrainOptions {
        raw_prompt: true,
        ready_timeout: Duration::ZERO,
        ..DrainOptions::default()
    };
    manager
        .send_headless_record(&record, "/help", Some("raw-1"), &options)
        .unwrap();
    let queue = manager.queue("lead").unwrap();
    let item = agent_queue::queue::load_message(&queue.join("inbox.v2/raw-1.json")).unwrap();
    assert_eq!(item["raw"], true, "{item:?}");
    assert_eq!(item["text"], "/help", "{item:?}");
    assert!(item.get("opening").is_none(), "{item:?}");
    let shared = crate::agent::enqueued_item("raw-1", "/help", true);
    assert_eq!(item["raw"], shared["raw"]);
    assert_eq!(item["text"], shared["text"]);
}
