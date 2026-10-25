//! End-to-end agent lifecycle on the tmux backend against a real tmux
//! server on a private `-L` socket and a fake Claude-shaped harness:
//! start, send (idle and deliver-while-working), wait, read, a child
//! started from inside the agent (created_by/managed_by edges), adopt of
//! a manually started pane, harness death, revive, and stop. Skips
//! (passes) when tmux or python3 is not installed. Linux only: the
//! tmux process proofs read `/proc`.

#![cfg(target_os = "linux")]

use std::fs::{self, DirBuilder};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const FAKE_CLAUDE: &str = include_str!("fixtures/fake_claude.txt");

fn executable(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| {
            fs::metadata(candidate)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

struct Fixture {
    root: PathBuf,
    registry: PathBuf,
    work: PathBuf,
    socket: String,
    tmux: PathBuf,
    fake: PathBuf,
    argv_file: PathBuf,
}

impl Fixture {
    fn new() -> Option<Self> {
        let tmux = executable("tmux")?;
        executable("python3")?;
        let root = std::env::temp_dir().join(format!(
            "agentctl-tmux-lifecycle-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|now| now.as_nanos())
                .unwrap_or(0)
        ));
        let work = root.join("work");
        let bin = root.join("bin");
        for directory in [&root, &work, &bin] {
            DirBuilder::new()
                .mode(0o700)
                .create(directory)
                .unwrap_or_else(|error| panic!("create {}: {error}", directory.display()));
        }
        let fake = bin.join("claude");
        let socket = format!("agentctl-tmux-test-{}", std::process::id());
        // The started agent itself starts a child on the "spawn-child"
        // prompt, which is how created_by/managed_by get proved on this
        // backend. The command is baked into the fake (not passed with
        // start --env, whose values revive will not replay).
        let registry = root.join("registry");
        let spawn_command = format!(
            "AGENTCTL_CLAUDE_BIN={} {} --registry {} --term-backend tmux --tmux-socket {} \
             start child --cwd {} --harness claude --startup-timeout 30 >> {} 2>&1",
            fake.display(),
            env!("CARGO_BIN_EXE_agentctl"),
            registry.display(),
            socket,
            work.display(),
            root.join("spawn.log").display()
        );
        let argv_file = root.join("fake-argv.log");
        fs::write(
            &fake,
            FAKE_CLAUDE
                .replace("__SPAWN_CMD__", &spawn_command)
                .replace("__ARGV_FILE__", &argv_file.display().to_string()),
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        Some(Self {
            registry,
            root,
            work,
            socket,
            tmux,
            fake,
            argv_file,
        })
    }

    fn agentctl(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_agentctl"))
            .arg("--registry")
            .arg(&self.registry)
            .args(["--term-backend", "tmux", "--tmux-socket", &self.socket])
            .args(args)
            .env("AGENTCTL_CLAUDE_BIN", &self.fake)
            .env_remove("AGENTCTL_TERM_BACKEND")
            .env_remove("HERDR_WORKSPACE_ID")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .output()
            .expect("run agentctl")
    }

    fn agentctl_json(&self, args: &[&str]) -> Value {
        let output = self.agentctl(args);
        assert!(
            output.status.success(),
            "agentctl {args:?} failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("agentctl {args:?} output is not JSON: {error}"))
    }

    fn tmux(&self, args: &[&str]) -> Output {
        Command::new(&self.tmux)
            .args(["-L", &self.socket])
            .args(args)
            .output()
            .expect("run tmux")
    }

    fn wait_for(&self, what: &str, timeout: Duration, mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if check() {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!("timed out waiting for {what}");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.tmux(&["kill-server"]);
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn tmux_backend_runs_the_full_agent_lifecycle() {
    let Some(fixture) = Fixture::new() else {
        eprintln!("tmux or python3 not installed; skipping");
        return;
    };
    let work = fixture.work.display().to_string();

    // Start: a tmux record, an anchored harness process, an idle screen.
    let started = fixture.agentctl_json(&[
        "start",
        "worker",
        "--cwd",
        &work,
        "--harness",
        "claude",
        "--startup-timeout",
        "30",
    ]);
    assert_eq!(started["adapter"], "tmux");
    assert_eq!(started["backend"], "tmux");
    assert_eq!(started["agent_status"], "idle");
    assert_eq!(started["probe_error"], Value::Null);
    let pane_id = started["pane_id"].as_str().expect("pane id").to_owned();
    assert!(pane_id.starts_with('%'), "tmux pane id: {pane_id}");
    let harness_pid = started["harness_identity"]["pid"]
        .as_u64()
        .expect("anchored harness pid");
    assert!(harness_pid > 0);

    // Send while idle, then while working. The work window is long and
    // polled for, so a loaded host cannot race it shut; the fake ends it
    // early on the finish-now follow-up.
    let sent = fixture.agentctl_json(&["send", "worker", "hello idle world"]);
    assert_eq!(sent["outcome"], "delivered");
    let sent = fixture.agentctl_json(&["send", "worker", "work:120 start the long task"]);
    assert_eq!(sent["outcome"], "delivered");
    fixture.wait_for("worker to report working", Duration::from_secs(60), || {
        let output = fixture.agentctl(&["status", "worker"]);
        output.status.success()
            && serde_json::from_slice::<Value>(&output.stdout)
                .is_ok_and(|status| status["agent_status"] == "working")
    });
    let sent = fixture.agentctl_json(&[
        "send",
        "worker",
        "followup while busy",
        "--deliver-while-working",
    ]);
    assert_eq!(sent["outcome"], "delivered");
    let sent = fixture.agentctl_json(&["send", "worker", "finish-now", "--deliver-while-working"]);
    assert_eq!(sent["outcome"], "delivered");
    let waited = fixture.agentctl_json(&["wait", "worker"]);
    assert_eq!(waited["agent_status"], "idle");
    let read = fixture.agentctl(&["read", "worker"]);
    let screen = String::from_utf8_lossy(&read.stdout);
    assert!(screen.contains("hello idle world"), "read: {screen}");
    assert!(screen.contains("followup while busy"), "read: {screen}");
    assert!(screen.contains("finished"), "read: {screen}");

    // A child the worker starts from inside its own pane records the
    // worker as its creator and manager, as on Herdr.
    let sent = fixture.agentctl_json(&["send", "worker", "spawn-child"]);
    assert_eq!(sent["outcome"], "delivered");
    fixture.wait_for("child agent to register", Duration::from_secs(60), || {
        fixture.agentctl(&["status", "child"]).status.success()
    });
    let child = fixture.agentctl_json(&["status", "child"]);
    assert_eq!(child["adapter"], "tmux");
    assert_eq!(child["created_by"]["agent"], json!("worker"));
    assert_eq!(child["managed_by"]["agent"], json!("worker"));
    let stopped = fixture.agentctl_json(&["stop", "child"]);
    assert_eq!(stopped["pane_closed"], json!(true));

    // Adopt: a harness started by hand in a plain tmux session registers
    // as tmux-foreign, takes sends, and stop unregisters without closing
    // its pane.
    let created = fixture.tmux(&[
        "new-session",
        "-d",
        "-P",
        "-F",
        "#{pane_id}",
        "-s",
        "manual",
        "-c",
        &work,
    ]);
    assert!(created.status.success());
    let manual_pane = String::from_utf8_lossy(&created.stdout).trim().to_owned();
    let launch = format!(
        "{} --session-id 11111111-2222-3333-4444-555555555555",
        fixture.fake.display()
    );
    assert!(fixture
        .tmux(&["send-keys", "-t", &manual_pane, "-l", "--", &launch])
        .status
        .success());
    assert!(fixture
        .tmux(&["send-keys", "-t", &manual_pane, "Enter"])
        .status
        .success());
    fixture.wait_for("manual harness screen", Duration::from_secs(30), || {
        let output = fixture.tmux(&["capture-pane", "-p", "-t", &manual_pane]);
        String::from_utf8_lossy(&output.stdout).contains("fake harness ready")
    });
    let adopted = fixture.agentctl_json(&[
        "adopt",
        "manual-agent",
        "--pane",
        &manual_pane,
        "--workspace",
        "manual",
        "--cwd",
        &work,
        "--harness",
        "claude",
    ]);
    assert_eq!(adopted["adapter"], "tmux-foreign");
    assert_eq!(adopted["backend"], "tmux");
    let sent = fixture.agentctl_json(&["send", "manual-agent", "hello adopted agent"]);
    assert_eq!(sent["outcome"], "delivered");
    let stopped = fixture.agentctl_json(&["stop", "manual-agent"]);
    assert_eq!(stopped["pane_closed"], json!(false));
    assert_eq!(stopped["runtime_preserved"], json!(true));
    let panes = fixture.tmux(&["list-panes", "-a", "-F", "#{pane_id}"]);
    assert!(
        String::from_utf8_lossy(&panes.stdout)
            .lines()
            .any(|line| line == manual_pane),
        "adopted pane must survive stop"
    );
    assert!(fixture
        .tmux(&["kill-session", "-t", "manual"])
        .status
        .success());

    // Revive: kill the worker's harness process, prove the pane is back
    // at its idle shell, and revive the same conversation in a new pane.
    // Re-verify the pid is still this fixture's fake first: the value was
    // captured many steps ago and a recycled pid must never be signalled.
    let fresh = fixture.agentctl_json(&["status", "worker"]);
    assert_eq!(fresh["harness_identity"]["pid"], json!(harness_pid));
    let session = fresh["native_session"]["value"]
        .as_str()
        .expect("native session")
        .to_owned();
    let cmdline =
        fs::read_to_string(format!("/proc/{harness_pid}/cmdline")).expect("harness cmdline");
    assert!(
        cmdline.contains(&fixture.fake.display().to_string()),
        "pid {harness_pid} is no longer the fake harness"
    );
    assert!(Command::new("kill")
        .arg(harness_pid.to_string())
        .status()
        .unwrap()
        .success());
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let output = fixture.agentctl(&["revive", "worker", "--dry-run"]);
        if output.status.success()
            && serde_json::from_slice::<Value>(&output.stdout)
                .is_ok_and(|plan| plan["action"] == "revive")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "revive dry-run never planned a revive: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    let revived = fixture.agentctl_json(&["revive", "worker", "--startup-timeout", "30"]);
    assert_eq!(revived["revived"], json!(true));
    assert_eq!(revived["agent_status"], "idle");
    assert_ne!(revived["pane_id"], json!(pane_id));
    assert_eq!(revived["adapter"], "tmux");
    // The replacement launch must resume the recorded conversation, not
    // start a new one: the fake logs every invocation's argv.
    let argv_log = fs::read_to_string(&fixture.argv_file).expect("fake argv log");
    let invocations: Vec<&str> = argv_log.lines().collect();
    assert!(
        invocations.len() >= 2,
        "expected start and revive launches, got: {argv_log}"
    );
    assert!(
        invocations[0].contains(&format!("--session-id {session}")),
        "first launch argv: {}",
        invocations[0]
    );
    assert!(
        invocations
            .last()
            .unwrap()
            .contains(&format!("--resume {session}")),
        "revive launch argv: {}",
        invocations.last().unwrap()
    );
    let sent = fixture.agentctl_json(&["send", "worker", "hello after revive"]);
    assert_eq!(sent["outcome"], "delivered");

    // Stop closes the owned pane and archives the record.
    let stopped = fixture.agentctl_json(&["stop", "worker"]);
    assert_eq!(stopped["pane_closed"], json!(true));
    assert!(!fixture.registry.join("worker").exists());
    let list = fixture.agentctl(&["list"]);
    assert!(list.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&list.stdout).unwrap(),
        json!([])
    );
}
