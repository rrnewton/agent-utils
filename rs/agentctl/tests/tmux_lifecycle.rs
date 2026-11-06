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

static FIXTURE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
        // Every fixture owns its own tmux server (#923): a socket named
        // only by pid is shared by every test in this binary, so one
        // test's teardown kill-server used to kill another test's
        // server mid-flight, and a leaked server pinned its slot.
        let socket = format!(
            "agentctl-tmux-test-{}-{}",
            std::process::id(),
            FIXTURE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        );
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
        // Start this fixture's server now, from the fixture's temp root
        // (outside any slot): a tmux server keeps the cwd of whoever
        // first spawns it, and a server started from inside a slot pins
        // that slot for as long as it lives (#923).
        let started = Command::new(&tmux)
            .args(["-L", &socket, "start-server"])
            .current_dir(&root)
            .output()
            .expect("start tmux server");
        assert!(
            started.status.success(),
            "start tmux server: {}",
            String::from_utf8_lossy(&started.stderr)
        );
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

    fn agentctl_without_unknown_bin(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_agentctl"))
            .arg("--registry")
            .arg(&self.registry)
            .args(["--term-backend", "tmux", "--tmux-socket", &self.socket])
            .args(args)
            .env_remove("AGENTCTL_UNKNOWN_BIN")
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

/// The socket file tmux creates for a `-L` name: `$TMUX_TMPDIR` or
/// `/tmp`, under `tmux-<uid>`.
fn tmux_socket_path(socket: &str) -> PathBuf {
    let uid = fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("Uid:"))
                .and_then(|fields| fields.split_whitespace().next().map(str::to_owned))
        })
        .unwrap_or_else(|| "0".to_string());
    let base = std::env::var_os("TMUX_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join(format!("tmux-{uid}")).join(socket)
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Kill this fixture's own server and remove its socket, on
        // success, failure and panic unwind alike (#923).
        let _ = self.tmux(&["kill-server"]);
        let _ = fs::remove_file(tmux_socket_path(&self.socket));
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn tmux_server_dies_with_its_fixture_even_on_panic() {
    // #923: a fixture's private tmux server and socket must not
    // outlive the test, including when the test panics mid-way.
    let Some(fixture) = Fixture::new() else {
        eprintln!("tmux or python3 not installed; skipping");
        return;
    };
    let work = fixture.work.display().to_string();
    let created = fixture.tmux(&["new-session", "-d", "-s", "guard-probe", "-c", &work]);
    assert!(created.status.success(), "new-session failed");
    assert!(
        fixture.tmux(&["list-sessions"]).status.success(),
        "server running before the panic"
    );
    let socket = fixture.socket.clone();
    let tmux = fixture.tmux.clone();
    let root = fixture.root.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _fixture = fixture;
        panic!("forced panic to prove the tmux guard cleans up");
    }));
    assert!(result.is_err(), "the forced panic was caught");
    let listed = Command::new(&tmux)
        .args(["-L", &socket, "list-sessions"])
        .output()
        .expect("list sessions after panic");
    assert!(
        !listed.status.success(),
        "tmux server {socket} outlived its test"
    );
    assert!(
        !tmux_socket_path(&socket).exists(),
        "tmux socket {} left behind",
        tmux_socket_path(&socket).display()
    );
    assert!(!root.exists(), "fixture root removed by drop");
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

    // Revive fresh (#686 agent-turnover): kill the revived harness, then start a new
    // conversation under the same name instead of resuming the recorded one.
    let revived_status = fixture.agentctl_json(&["status", "worker"]);
    let revived_pid = revived_status["harness_identity"]["pid"]
        .as_u64()
        .expect("revived harness pid");
    let cmdline =
        fs::read_to_string(format!("/proc/{revived_pid}/cmdline")).expect("harness cmdline");
    assert!(cmdline.contains(&fixture.fake.display().to_string()));
    assert!(Command::new("kill")
        .arg(revived_pid.to_string())
        .status()
        .unwrap()
        .success());
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let output = fixture.agentctl(&["revive", "worker", "--dry-run"]);
        if serde_json::from_slice::<Value>(&output.stdout)
            .is_ok_and(|plan| plan["action"] == "revive")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no revive plan after the second death"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    let fresh = fixture.agentctl_json(&["revive", "worker", "--fresh", "--startup-timeout", "30"]);
    assert_eq!(fresh["revived"], json!(true), "{fresh:#}");
    let fresh_status = fixture.agentctl_json(&["status", "worker"]);
    let fresh_session = fresh_status["native_session"]["value"]
        .as_str()
        .expect("fresh session")
        .to_owned();
    assert_ne!(fresh_session, session);
    let argv_log = fs::read_to_string(&fixture.argv_file).expect("fake argv log");
    let last = argv_log.lines().last().unwrap();
    assert!(
        last.contains(&format!("--session-id {fresh_session}")) && !last.contains("--resume"),
        "fresh revive argv: {last}"
    );

    // Stop closes the owned pane and archives the record.
    let stopped = fixture.agentctl_json(&["stop", "worker"]);
    assert_eq!(stopped["pane_closed"], json!(true));
    assert!(!fixture.registry.join("agents/worker").exists());
    assert!(!fixture.registry.join("worker").exists());
    let list = fixture.agentctl(&["list"]);
    assert!(list.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&list.stdout).unwrap(),
        json!([])
    );

    // Every step above extended the session lineage (#686 agent-turnover).
    let history = fixture.agentctl_json(&["history", "worker", "--json"]);
    let steps: Vec<(String, String)> = history["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| {
            let detail = entry
                .get("relation")
                .or_else(|| entry.get("reason"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            (
                entry["kind"].as_str().unwrap().to_owned(),
                detail.to_owned(),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = [
        ("generation", "start"),
        ("ended", "died"),
        ("generation", "resume"),
        ("ended", "died"),
        ("generation", "fresh"),
        ("ended", "stopped"),
    ]
    .iter()
    .map(|(kind, detail)| ((*kind).to_owned(), (*detail).to_owned()))
    .collect();
    assert_eq!(steps, expected, "{history:#}");
    let entries = history["entries"].as_array().unwrap();
    assert!(entries[..4]
        .iter()
        .all(|entry| entry["session"] == json!(session)));
    assert!(entries[4..]
        .iter()
        .all(|entry| entry["session"] == json!(fresh_session)));
    assert!(entries.iter().all(|entry| entry["line"] == history["line"]));
    assert_eq!(history["sessions"], 2);
    let adopted = fixture.agentctl_json(&["history", "manual-agent", "--json"]);
    let kinds: Vec<&str> = adopted["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry.get("relation").or_else(|| entry.get("reason")))
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(kinds, ["adopt", "stopped"], "{adopted:#}");
    let owners = fixture.agentctl_json(&["history", "--session", &fresh_session, "--json"]);
    assert!(owners["owners"]
        .as_array()
        .unwrap()
        .iter()
        .all(|owner| owner["name"] == "worker"));
}

/// #913: an unrecognised TUI adopts under `--harness unknown` with no
/// AGENTCTL_UNKNOWN_BIN: the record pins the pane's actual foreground
/// program (path plus process generation), so a different program later
/// started in the same pane never counts as the same agent alive.
#[test]
fn tmux_unknown_harness_pins_actual_foreground_program() {
    let Some(fixture) = Fixture::new() else {
        eprintln!("tmux or python3 not installed; skipping");
        return;
    };
    let work = fixture.work.display().to_string();
    // A fake TUI whose name matches no harness kind and no fixed
    // install location.
    let fake_tui = fixture.root.join("bin/fake-tui");
    fs::write(&fake_tui, FAKE_CLAUDE).unwrap();
    fs::set_permissions(&fake_tui, fs::Permissions::from_mode(0o700)).unwrap();
    let other_tui = fixture.root.join("bin/fake-other");
    fs::write(&other_tui, FAKE_CLAUDE).unwrap();
    fs::set_permissions(&other_tui, fs::Permissions::from_mode(0o700)).unwrap();

    let created = fixture.tmux(&[
        "new-session",
        "-d",
        "-P",
        "-F",
        "#{pane_id}",
        "-s",
        "unknown-manual",
        "-c",
        &work,
    ]);
    assert!(created.status.success());
    let pane = String::from_utf8_lossy(&created.stdout).trim().to_owned();
    let launch = fake_tui.display().to_string();
    assert!(fixture
        .tmux(&["send-keys", "-t", &pane, "-l", "--", &launch])
        .status
        .success());
    assert!(fixture
        .tmux(&["send-keys", "-t", &pane, "Enter"])
        .status
        .success());
    fixture.wait_for("fake-tui screen", Duration::from_secs(30), || {
        let output = fixture.tmux(&["capture-pane", "-p", "-t", &pane]);
        String::from_utf8_lossy(&output.stdout).contains("fake harness ready")
    });

    // Adopt with no AGENTCTL_UNKNOWN_BIN anywhere in the environment.
    let output = fixture.agentctl_without_unknown_bin(&[
        "adopt",
        "unknown-agent",
        "--pane",
        &pane,
        "--workspace",
        "unknown-manual",
        "--cwd",
        &work,
        "--harness",
        "unknown",
    ]);
    assert!(
        output.status.success(),
        "adopt unknown failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let adopted: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(adopted["adapter"], "tmux-foreign");
    assert_eq!(adopted["harness"], "unknown");
    let pinned_pid = adopted["harness_identity"]["pid"]
        .as_u64()
        .expect("pinned foreground pid");
    assert!(pinned_pid > 0);
    // The pin is the actual program: its /proc cmdline names fake-tui,
    // and the recorded launch spec holds the same executable.
    let cmdline = fs::read_to_string(format!("/proc/{pinned_pid}/cmdline")).expect("cmdline");
    assert!(
        cmdline.contains(&fake_tui.display().to_string()),
        "pinned pid {pinned_pid} is not the fake-tui: {cmdline:?}"
    );
    // The recorded launch names the actual program: a script TUI's
    // kernel image is its interpreter, so the script path is in argv.
    let launch_argv = adopted["launch_spec"]["argv"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    assert!(
        launch_argv.contains(&fake_tui.display().to_string())
            || adopted["launch_spec"]["executable"] == json!(fake_tui.display().to_string()),
        "launch spec does not name the fake-tui: {:#}",
        adopted["launch_spec"]
    );

    // It takes work through the generic submission path.
    let sent =
        fixture.agentctl_without_unknown_bin(&["send", "unknown-agent", "hello unknown world"]);
    assert!(
        sent.status.success(),
        "send: {}",
        String::from_utf8_lossy(&sent.stderr)
    );
    let sent_json: Value = serde_json::from_slice(&sent.stdout).unwrap();
    assert_eq!(sent_json["outcome"], "delivered");

    // Replacement: kill the pinned program and start a different one
    // in the same pane. It must never count as the adopted agent
    // alive, and a send must not be delivered to it.
    let cmdline = fs::read_to_string(format!("/proc/{pinned_pid}/cmdline")).expect("cmdline");
    assert!(cmdline.contains(&fake_tui.display().to_string()));
    assert!(Command::new("kill")
        .arg(pinned_pid.to_string())
        .status()
        .unwrap()
        .success());
    let pane_command = |fixture: &Fixture| {
        let output = fixture.tmux(&[
            "list-panes",
            "-t",
            "unknown-manual",
            "-F",
            "#{pane_current_command}",
        ]);
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    fixture.wait_for(
        "pane shell after fake-tui death",
        Duration::from_secs(30),
        || pane_command(&fixture) == "bash",
    );
    let launch_other = other_tui.display().to_string();
    assert!(fixture
        .tmux(&["send-keys", "-t", &pane, "-l", "--", &launch_other])
        .status
        .success());
    assert!(fixture
        .tmux(&["send-keys", "-t", &pane, "Enter"])
        .status
        .success());
    fixture.wait_for(
        "replacement program running",
        Duration::from_secs(30),
        || pane_command(&fixture) != "bash",
    );
    let status = fixture.agentctl_without_unknown_bin(&["status", "unknown-agent"]);
    if status.status.success() {
        let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
        assert_ne!(
            status_json["agent_status"], "idle",
            "replacement program counted as the adopted agent: {status_json:#}"
        );
    }
    let sent = fixture.agentctl_without_unknown_bin(&[
        "send",
        "unknown-agent",
        "must not reach the replacement",
        "--ready-timeout",
        "5",
    ]);
    let delivered = sent.status.success()
        && serde_json::from_slice::<Value>(&sent.stdout)
            .is_ok_and(|value| value["outcome"] == "delivered");
    assert!(!delivered, "send reached the replacement program");

    // Stop requires the recorded idle shell: exit the replacement
    // first (its own die-now prompt), then stop unregisters without
    // closing the foreign pane.
    assert!(fixture
        .tmux(&["send-keys", "-t", &pane, "-l", "--", "die-now"])
        .status
        .success());
    assert!(fixture
        .tmux(&["send-keys", "-t", &pane, "Enter"])
        .status
        .success());
    fixture.wait_for(
        "pane shell after replacement exit",
        Duration::from_secs(30),
        || pane_command(&fixture) == "bash",
    );
    let stopped = fixture.agentctl_without_unknown_bin(&["stop", "unknown-agent"]);
    assert!(
        stopped.status.success(),
        "stop: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    assert!(fixture
        .tmux(&["kill-session", "-t", "unknown-manual"])
        .status
        .success());
}
