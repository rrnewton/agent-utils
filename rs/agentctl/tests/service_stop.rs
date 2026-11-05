//! `agentctl watch-agents` exits within seconds of SIGTERM (#870 agentctl-service-slow-stop):
//! from the sleep between passes, with its state saved, and from a pass stuck in a Herdr call,
//! which it abandons. Before the fix, the watcher's handler turned SIGTERM into a flag that its
//! loop never read, so every stop waited out systemd's 90 s stop timeout.
//!
//! This drives the real binary as a subprocess in its own process group, against a scratch
//! registry, scratch home directories and a stand-in `herdr` that never answers, so nothing live
//! is touched. The `chat run` cases live in the chat launch integration tests, which have a fake
//! provider plugin.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// How long a stop may take.
const STOP_LIMIT: Duration = Duration::from_secs(5);

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "agentctl-service-stop-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        for directory in ["registry", "state", "home"] {
            fs::create_dir_all(root.join(directory)).expect("create scratch directory");
        }
        for directory in [root.clone(), root.join("registry"), root.join("state")] {
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                .expect("make scratch directory private");
        }
        // A stand-in Herdr that records each call and never answers, as a server stuck behind a
        // busy pane does.
        let herdr = root.join("herdr");
        fs::write(
            &herdr,
            "#!/bin/sh\necho \"$@\" >> \"$(dirname \"$0\")/calls\"\nexec sleep 600\n",
        )
        .expect("write stand-in herdr");
        fs::set_permissions(&herdr, fs::Permissions::from_mode(0o700)).expect("herdr mode");
        Self { root }
    }

    /// Register one running agent, so that a pass asks Herdr about it.
    fn register_agent(&self) {
        let directory = self.root.join("registry/agents/lane");
        fs::create_dir_all(&directory).expect("create agent directory");
        fs::set_permissions(
            self.root.join("registry/agents"),
            fs::Permissions::from_mode(0o700),
        )
        .expect("agents directory mode");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .expect("agent directory mode");
        let record = serde_json::json!({
            "schema": 1,
            "name": "lane",
            "token": "0123456789abcdef0123456789abcdef",
            "harness": "claude",
            "adapter": "herdr",
            "backend": "herdr",
            "mode": "interactive",
            "cwd": self.root,
            "created_at": 1_793_806_065.0,
            "lifecycle": "running",
            "workspace_id": "w1",
            "tab_id": "w1:t1",
            "pane_id": "w1:p1",
            "arguments": [],
        });
        let path = directory.join("agent.json");
        fs::write(&path, record.to_string()).expect("write agent record");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("record mode");
    }

    fn watch(&self) -> Child {
        let home = self.root.join("home");
        Command::new(env!("CARGO_BIN_EXE_agentctl"))
            .current_dir(&self.root)
            .env("HOME", &home)
            .env("AGENTCTL_HOME", home.join(".agentctl"))
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .env_remove("HERDR_SOCKET_PATH")
            .arg("--registry")
            .arg(self.root.join("registry"))
            .arg("--herdr-bin")
            .arg(self.root.join("herdr"))
            .args(["watch-agents", "--interval", "600", "--state-dir"])
            .arg(self.root.join("state"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(self.root.join("stderr")).expect("stderr file"))
            .process_group(0)
            .spawn()
            .expect("start watch-agents")
    }

    fn stderr(&self) -> String {
        fs::read_to_string(self.root.join("stderr")).unwrap_or_default()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn wait_for(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    condition()
}

/// Send SIGTERM to `child` and wait for it for up to `limit`; how long it took and how it ended.
/// Its whole process group is killed afterwards, stand-in Herdr calls included.
fn terminate(child: &mut Child, limit: Duration) -> (Duration, Option<ExitStatus>) {
    let pid = i32::try_from(child.id()).expect("pid fits");
    let started = Instant::now();
    // SAFETY: kill takes scalar arguments and signals the child this test started.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    let mut status = None;
    while started.elapsed() < limit * 4 {
        if let Some(done) = child.try_wait().expect("poll watch-agents") {
            status = Some(done);
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let took = started.elapsed();
    // SAFETY: killpg takes scalar arguments; the group is the one this test's child leads.
    unsafe { libc::killpg(pid, libc::SIGKILL) };
    if status.is_none() {
        let _ = child.wait();
    }
    (took, status)
}

fn calls(root: &Path) -> String {
    fs::read_to_string(root.join("calls")).unwrap_or_default()
}

#[test]
fn watch_agents_stops_from_its_sleep_between_passes_within_seconds_of_sigterm() {
    let scratch = Scratch::new("sleep");
    let mut child = scratch.watch();
    // The first pass has run, and saved its report, once this file is there; the watcher then
    // sleeps until the next pass, 10 minutes off.
    assert!(
        wait_for(Duration::from_secs(60), || scratch
            .root
            .join("state/agent-watch.json")
            .exists()),
        "the first pass never finished: {}",
        scratch.stderr()
    );
    thread::sleep(Duration::from_millis(300));
    let (took, status) = terminate(&mut child, STOP_LIMIT);
    let status = status.expect("watch-agents exited");
    assert!(took < STOP_LIMIT, "took {took:?}: {}", scratch.stderr());
    assert!(status.success(), "{status:?}: {}", scratch.stderr());
    assert!(
        scratch.stderr().contains("stopping: termination requested"),
        "{}",
        scratch.stderr()
    );
}

#[test]
fn watch_agents_abandons_a_pass_stuck_in_herdr_within_seconds_of_sigterm() {
    let scratch = Scratch::new("mid-pass");
    scratch.register_agent();
    let mut child = scratch.watch();
    assert!(
        wait_for(Duration::from_secs(60), || !calls(&scratch.root).is_empty()),
        "the pass never asked Herdr about the agent: {}",
        scratch.stderr()
    );
    thread::sleep(Duration::from_millis(300));
    let (took, status) = terminate(&mut child, STOP_LIMIT);
    let status = status.expect("watch-agents exited");
    assert!(took < STOP_LIMIT, "took {took:?}: {}", scratch.stderr());
    // Ended as SIGTERM's default action ends a process, which systemd counts as a clean stop.
    assert_eq!(status.signal(), Some(libc::SIGTERM), "{}", scratch.stderr());
    assert!(
        scratch
            .stderr()
            .contains("abandoning what is still in flight"),
        "{}",
        scratch.stderr()
    );
}
