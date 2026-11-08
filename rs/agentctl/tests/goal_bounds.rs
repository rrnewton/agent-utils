//! `agentctl goal` never waits without bound and never outlives itself (#1075
//! agentctl-goal-wait-unbounded).
//!
//! A consumer found an `agentctl goal` process parked in `pause()` for 32 hours, deaf to SIGTERM,
//! after its target had been archived. That process was the atomic supervisor of a Herdr control
//! command, which carries its host's command line: its host was killed mid-command and, before
//! #1061 chat-launch-outer-kill-leak, nothing took the supervisor down. These tests drive the real
//! binary against a scratch registry and a stand-in `herdr`, so nothing live is touched, and kill
//! only the process IDs they started or recorded.

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    /// A registry holding one running Claude Code agent, `lane`, and a stand-in `herdr` running
    /// `herdr_body`.
    fn new(label: &str, herdr_body: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "agentctl-goal-bounds-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        for directory in ["registry/agents/lane", "home"] {
            fs::create_dir_all(root.join(directory)).expect("create scratch directory");
        }
        for directory in ["", "registry", "registry/agents", "registry/agents/lane"] {
            fs::set_permissions(root.join(directory), fs::Permissions::from_mode(0o700))
                .expect("make scratch directory private");
        }
        let herdr = root.join("herdr");
        fs::write(&herdr, format!("#!/bin/sh\n{herdr_body}\n")).expect("write stand-in herdr");
        fs::set_permissions(&herdr, fs::Permissions::from_mode(0o700)).expect("herdr mode");
        let record = serde_json::json!({
            "schema": 1,
            "name": "lane",
            "token": "00000000000000000000000000000002",
            "harness": "claude",
            "adapter": "herdr",
            "backend": "herdr",
            "mode": "interactive",
            "cwd": root,
            "created_at": 1_793_806_065.0,
            "lifecycle": "running",
            "workspace_id": "w1",
            "tab_id": "w1:t1",
            "pane_id": "w1:p-lane",
            "arguments": [],
        });
        let path = root.join("registry/agents/lane/agent.json");
        fs::write(&path, record.to_string()).expect("write agent record");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("record mode");
        Self { root }
    }

    fn goal(&self, arguments: &[&str]) -> Child {
        let home = self.root.join("home");
        Command::new(env!("CARGO_BIN_EXE_agentctl"))
            .current_dir(&self.root)
            .env("HOME", &home)
            .env("AGENTCTL_HOME", home.join(".agentctl"))
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .env("AGENTCTL_AGENT", "coordinator")
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_PANE_ID")
            .env_remove("AGENTCLOUD_SESSION_ID")
            .arg("--registry")
            .arg(self.root.join("registry"))
            .arg("--herdr-bin")
            .arg(self.root.join("herdr"))
            .arg("goal")
            .arg("lane")
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run agentctl goal")
    }

    /// Every live process whose command line names this scratch directory: the command, and
    /// any supervisor that inherited its command line.
    fn survivors(&self) -> Vec<i32> {
        let marker = self.root.to_string_lossy().into_owned();
        let mut found = Vec::new();
        for entry in fs::read_dir("/proc").expect("list /proc").flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
                continue;
            };
            let Ok(cmdline) = fs::read(entry.path().join("cmdline")) else {
                continue;
            };
            if String::from_utf8_lossy(&cmdline).contains(&marker) && alive(pid) {
                found.push(pid);
            }
        }
        found
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Live and not a zombie.
fn alive(pid: i32) -> bool {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_some_and(|state| state != "Z"),
        Err(_) => false,
    }
}

fn wait_for(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < limit {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    condition()
}

/// Kills exactly the recorded process IDs if a test fails part-way.
struct Reap(Vec<i32>);

impl Drop for Reap {
    fn drop(&mut self) {
        for &pid in &self.0 {
            if alive(pid) {
                // SAFETY: kill only sends a signal to a process ID this test recorded.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }
}

fn recorded_pids(path: &Path) -> Vec<i32> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

#[test]
fn a_goal_behind_a_held_agent_lock_gives_up_at_its_deadline() {
    let scratch = Scratch::new("deadline", "exit 1");
    let locks = scratch.root.join("registry/locks");
    fs::create_dir_all(&locks).expect("create locks directory");
    fs::set_permissions(&locks, fs::Permissions::from_mode(0o700)).expect("locks mode");
    // What a stop, an archive or another send holds while it works on the agent.
    let held = File::create(locks.join("lane.lock")).expect("create the agent lock");
    fs::set_permissions(locks.join("lane.lock"), fs::Permissions::from_mode(0o600))
        .expect("lock mode");
    // SAFETY: flock on a descriptor this test owns.
    assert_eq!(unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX) }, 0);

    let started = Instant::now();
    let child = scratch.goal(&["--deadline", "2", "finish the review"]);
    let _reap = Reap(vec![child.id() as i32]);
    let output = child.wait_with_output().expect("wait for agentctl goal");
    let elapsed = started.elapsed();
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.status.code(), Some(75), "{report}");
    assert!(
        elapsed < Duration::from_secs(10),
        "took {elapsed:?}: {report}"
    );
    assert!(report.contains("send deadline"), "{report}");
    let record = fs::read_to_string(scratch.root.join("registry/agents/lane/agent.json"))
        .expect("read agent record");
    assert!(
        !record.contains("finish the review"),
        "nothing was recorded: {record}"
    );
}

#[test]
fn a_goal_whose_agent_is_archived_while_it_waits_exits_with_a_clear_error() {
    let scratch = Scratch::new("archived", "exit 1");
    let locks = scratch.root.join("registry/locks");
    fs::create_dir_all(&locks).expect("create locks directory");
    fs::set_permissions(&locks, fs::Permissions::from_mode(0o700)).expect("locks mode");
    let held = File::create(locks.join("lane.lock")).expect("create the agent lock");
    fs::set_permissions(locks.join("lane.lock"), fs::Permissions::from_mode(0o600))
        .expect("lock mode");
    // SAFETY: flock on a descriptor this test owns.
    assert_eq!(unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX) }, 0);

    let mut child = scratch.goal(&["--deadline", "60", "finish the review"]);
    let _reap = Reap(vec![child.id() as i32]);
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        child.try_wait().expect("poll agentctl goal").is_none(),
        "goal did not wait"
    );
    // What an archive does while it holds the lock: the agent's record leaves the registry.
    fs::rename(
        scratch.root.join("registry/agents/lane"),
        scratch.root.join("archived-lane"),
    )
    .expect("archive the agent");
    let released = Instant::now();
    drop(held);
    let output = child.wait_with_output().expect("wait for agentctl goal");
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(released.elapsed() < Duration::from_secs(5), "{report}");
    assert!(!output.status.success(), "{report}");
    assert!(report.contains("unknown agent \"lane\""), "{report}");
    assert!(
        !scratch.root.join("registry/agents/lane").exists(),
        "goal recreated the archived agent: {report}"
    );
}

#[test]
fn goal_help_documents_the_deadline_and_its_default() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentctl"))
        .args(["goal", "--help"])
        .output()
        .expect("run agentctl goal --help");
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{help}");
    assert!(help.contains("--deadline <SECONDS>"), "{help}");
    assert!(help.contains("send_deadline_seconds"), "{help}");
    assert!(help.contains("else 600"), "{help}");
}

/// The stand-in `herdr` records its process ID and then hangs, so `goal` is inside a Herdr
/// control command when the signal arrives.
fn hung_herdr_survivors_after(signal: libc::c_int) {
    let label = format!("signal-{signal}");
    let scratch_root = std::env::temp_dir().join(format!(
        "agentctl-goal-bounds-{label}-{}",
        std::process::id()
    ));
    let pids = scratch_root.join("herdr.pids");
    let scratch = Scratch::new(
        &label,
        &format!("echo $$ >> '{}'\nexec sleep 300", pids.display()),
    );
    let mut child = scratch.goal(&["--deadline", "120", "finish the review"]);
    let host = child.id() as i32;
    let mut reap = Reap(vec![host]);
    assert!(
        wait_for(Duration::from_secs(30), || !recorded_pids(&pids).is_empty()),
        "goal never ran its Herdr command"
    );
    reap.0.extend(recorded_pids(&pids));
    let supervisors: Vec<i32> = scratch
        .survivors()
        .into_iter()
        .filter(|&pid| pid != host)
        .collect();
    reap.0.extend(&supervisors);

    // SAFETY: kill sends a signal to the child this test spawned.
    assert_eq!(unsafe { libc::kill(host, signal) }, 0);
    assert!(
        wait_for(Duration::from_secs(2), || child
            .try_wait()
            .ok()
            .flatten()
            .is_some()),
        "agentctl goal outlived signal {signal} by 2 s"
    );
    let gone = wait_for(Duration::from_secs(3), || {
        scratch.survivors().is_empty() && recorded_pids(&pids).iter().all(|&pid| !alive(pid))
    });
    assert!(
        gone,
        "after signal {signal}: processes naming the scratch registry {:?}, herdr {:?}",
        scratch.survivors(),
        recorded_pids(&pids)
            .into_iter()
            .filter(|&pid| alive(pid))
            .collect::<Vec<_>>()
    );
}

#[test]
fn sigterm_during_a_herdr_command_ends_goal_and_everything_it_started() {
    hung_herdr_survivors_after(libc::SIGTERM);
}

#[test]
fn sigkill_during_a_herdr_command_leaves_no_supervisor_behind() {
    hung_herdr_survivors_after(libc::SIGKILL);
}
