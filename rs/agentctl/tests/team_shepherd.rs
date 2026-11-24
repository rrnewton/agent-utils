//! `agentctl team shepherd` scratch drill (#1559 Phase 2): the real
//! binary runs one incident against scratch registries with a fake
//! Herdr (`--herdr-bin`) and fake muse/claude/codex executables behind
//! the real `run-agent-prompt` wrapper. It proves fallback order and
//! timeout fall-through, the restore arm, one incident per epoch
//! (a rerun resumes the stamp and repeats nothing), cross-team
//! read-only judging, record-gap reporting, and `judge=none` when
//! every harness is down — with no real reboot, restore, pane, unit,
//! or live registry.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const FAKE_HERDR: &str = r#"#!/bin/sh
case "$1 $2" in
  "agent list") printf '{"result":{"agents":[]}}\n' ;;
  "pane list") printf '{"result":{"panes":[]}}\n' ;;
  "workspace list") printf '{"result":{"workspaces":[]}}\n' ;;
  "tab list") printf '{"result":{"tabs":[]}}\n' ;;
  *) exit 1 ;;
esac
"#;

fn install_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write executable");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("executable mode");
}

fn write_private(path: &Path, body: &str) {
    fs::write(path, body).expect("write file");
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("private mode");
}

/// A spawned child's process group. The drill's binaries spawn
/// children of their own (restore, team act, the supervisor), and a
/// test that leaks them piles processes onto the host: the group is
/// killed when the guard drops, whatever the test outcome was.
struct GroupGuard {
    pgid: u32,
    armed: std::cell::Cell<bool>,
}

/// Whether any process still belongs to group `pgid` (identity check
/// before ever signalling it: a bare recycled id is never killed).
fn group_exists(pgid: u32) -> bool {
    let Ok(entries) = fs::read_dir("/proc") else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some(after) = stat.rsplit_once(')').map(|(_, rest)| rest) else {
            continue;
        };
        let fields: Vec<&str> = after.split_whitespace().collect();
        if fields.get(2).and_then(|field| field.parse::<u32>().ok()) == Some(pgid) {
            return true;
        }
        let _ = pid;
    }
    false
}

impl GroupGuard {
    /// Kill the group once, only while membership is verifiable:
    /// TERM first so the wrapper can clean up its own harness
    /// sessions, a short grace, then KILL what remains.
    fn kill_group(&self) {
        if !self.armed.replace(false) {
            return;
        }
        if !group_exists(self.pgid) {
            return;
        }
        let _ = Command::new("kill")
            .args(["-TERM", &format!("-{}", self.pgid)])
            .status();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while group_exists(self.pgid) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if group_exists(self.pgid) {
            let _ = Command::new("kill")
                .args(["-KILL", &format!("-{}", self.pgid)])
                .status();
        }
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        self.kill_group();
    }
}

/// Spawn in a fresh process group, reap the whole group on drop of
/// the returned guard, and wait for the direct child.
fn spawn_grouped(command: &mut Command) -> (std::process::Output, GroupGuard) {
    use std::os::unix::process::CommandExt;
    command
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().expect("spawn grouped child");
    let guard = GroupGuard {
        pgid: child.id(),
        armed: std::cell::Cell::new(true),
    };
    let output = child.wait_with_output().expect("wait for grouped child");
    (output, guard)
}

/// Processes whose cmdline names this scratch tree (excluding us).
fn scratch_processes(root: &Path) -> Vec<u32> {
    let needle = root.display().to_string();
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return found;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Ok(pid) = name.to_string_lossy().parse::<u32>() else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        if let Ok(cmdline) = fs::read(entry.path().join("cmdline")) {
            if String::from_utf8_lossy(&cmdline).contains(&needle) {
                found.push(pid);
            }
        }
    }
    found
}

struct Scratch {
    root: PathBuf,
    groups: std::cell::RefCell<Vec<GroupGuard>>,
}

impl Scratch {
    /// Kill every spawned group, then prove nothing survived.
    fn reap(&self) {
        for guard in self.groups.borrow().iter() {
            guard.kill_group();
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            let survivors = scratch_processes(&self.root);
            if survivors.is_empty() {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "drill leaked processes: {survivors:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        for guard in self.groups.borrow().iter() {
            guard.kill_group();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Scratch {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "agentctl-team-shepherd-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let scratch = Self {
            root,
            groups: std::cell::RefCell::new(Vec::new()),
        };
        scratch.register("own");
        scratch.register("judged");
        // Fake Herdr and fake one-shot harnesses: muse hangs past the
        // per-attempt timeout, claude answers with one allowlisted
        // proposal, codex must never be reached.
        let bin = scratch.root.join("fakebin");
        fs::create_dir_all(&bin).expect("fake bin");
        // A fifo the slow fake blocks on: condition-based, never a
        // wall-clock race (NS5).
        let fifo = Command::new("mkfifo")
            .arg(bin.join("muse.fifo"))
            .status()
            .expect("mkfifo");
        assert!(fifo.success());
        install_executable(&scratch.root.join("herdr"), FAKE_HERDR);
        // Injectable launcher seam: every unit operation in the
        // drill goes to these recording fakes, never the real user
        // manager. The log path is baked in (children get a trimmed
        // environment).
        let systemd_log = scratch.root.join("systemd.log");
        // Refusing fake: the harness default. Every drill command
        // carries the test marker and this refusal unless the
        // recording seam below overrides it, so a future test that
        // forgets the seam fails closed instead of reaching the
        // real user manager.
        install_executable(
            &scratch.root.join("systemd-refuse"),
            &format!(
                "#!/bin/sh\necho refused \"$@\" >> {}\nexit 1\n",
                scratch.root.join("systemd-refused.log").display()
            ),
        );
        for name in ["systemctl", "systemd-run"] {
            install_executable(
                &scratch.root.join(name),
                &format!(
                    "#!/bin/sh\necho {} \"$@\" >> {}\nexit 0\n",
                    name,
                    systemd_log.display()
                ),
            );
        }
        // The wrapper gives each attempt an explicit-name environment,
        // so the fakes log to this scratch path baked into the script.
        let log = scratch.root.join("harness.log");
        install_executable(
            &bin.join("muse"),
            &format!(
                "#!/usr/bin/env python3\nimport json, sys, time\n\
                 open({log:?}, 'a').write('muse\\n')\n\
                 prompt = open(sys.argv[sys.argv.index('--prompt-file') + 1]).read()\n\
                 if 'agentctl-recovery-smoke' not in prompt:\n\
                 \topen({fifo:?}).read()\n\
                 text = '{{\"marker\": \"agentctl-recovery-smoke\"}}' if 'agentctl-recovery-smoke' in prompt else 'slow'\n\
                 print(json.dumps({{'payload': {{'kind': 'run_terminal', 'terminal': 'completed', 'reason': None, 'text': text}}}}))\n",
                log = log.display().to_string(),
                fifo = bin.join("muse.fifo").display().to_string()
            ),
        );
        install_executable(
            &bin.join("claude"),
            &format!(
                "#!/usr/bin/env python3\nimport json, sys\n\
                 open({log:?}, 'a').write('claude\\n')\n\
                 prompt = sys.stdin.read()\n\
                 result = '{{\"marker\": \"agentctl-recovery-smoke\"}}' if 'agentctl-recovery-smoke' in prompt else 'down\\n{{\"action\": \"facts\"}}'\n\
                 print(json.dumps({{'type': 'result', 'subtype': 'success', 'is_error': False, 'result': result}}))\n",
                log = log.display().to_string()
            ),
        );
        install_executable(
            &bin.join("codex"),
            &format!(
                "#!/usr/bin/env python3\nimport json, sys\n\
                 open({log:?}, 'a').write('codex\\n')\n\
                 args = sys.argv[1:]\n\
                 out = args[args.index('--output-last-message') + 1]\n\
                 open(out, 'w').write('{{\"marker\": \"agentctl-recovery-smoke\"}}\\n')\n\
                 print(json.dumps({{'type': 'turn.completed'}}))\n",
                log = log.display().to_string()
            ),
        );
        let teams = json!({
            "schema": "agentctl-restore-teams/v1",
            "registries": [
                scratch.root.join("own"),
                scratch.root.join("judged"),
            ],
        });
        write_private(
            &scratch.root.join("teams.json"),
            &serde_json::to_string(&teams).expect("teams json"),
        );
        scratch
    }

    fn spawn_grouped(&self, command: &mut Command) -> std::process::Output {
        let (output, guard) = spawn_grouped(command);
        self.groups.borrow_mut().push(guard);
        output
    }

    fn register(&self, team: &str) {
        let registry = self.root.join(team);
        let lane = registry.join("agents/lane");
        for queue in ["inbox", "inflight", "processed", "failed"] {
            let dir = lane.join("queue").join(queue);
            fs::create_dir_all(&dir).expect("queue directory");
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).expect("queue mode");
        }
        fs::set_permissions(lane.join("queue"), fs::Permissions::from_mode(0o700))
            .expect("queue root mode");
        let cwd = self.root.display().to_string();
        let record = json!({
            "schema": 1,
            "name": "lane",
            "token": "0123456789abcdef0123456789abcdef",
            "harness": "muse",
            "adapter": "herdr",
            "backend": "herdr",
            "mode": "interactive",
            "cwd": cwd,
            "created_at": 1_793_806_065.0,
            "lifecycle": "running",
            "workspace_id": "w1",
            "tab_id": "w1:t1",
            "pane_id": "w1:p1",
            "reasoning_effort": "high",
            "arguments": [],
            "launch_spec": {
                "schema": "agentctl-launch-spec/v1",
                "source": "start",
                "captured_at": 1_793_806_065.0,
                "harness": "muse",
                "executable": null,
                "argv": ["--approval-mode", "never"],
                "cwd": cwd,
                "profile": null,
                "environment_names": [],
                "slot": null,
                "slot_project": null,
                "slot_isolation": null,
                "workspace_label": null,
                "native_session": {
                    "agent": "muse",
                    "value": "sess-shepherd-drill",
                    "source": "asserted",
                },
            },
        });
        write_private(&lane.join("agent.json"), &record.to_string());
        // No profile in the record: the stamp must report the gap.
        // Zero waits keep the deterministic arm fast against the fake.
        write_private(
            &registry.join("restore.json"),
            "{\"schema\": \"agentctl-restore/v1\", \"coordinator\": \"lane\", \
             \"units\": [], \"herdr_wait_seconds\": 0, \"startup_delay_seconds\": 0}",
        );
    }

    fn tree(&self, team: &str) -> Vec<(PathBuf, Vec<u8>)> {
        let base = self.root.join(team);
        let mut found = Vec::new();
        let mut stack = vec![base.clone()];
        while let Some(path) = stack.pop() {
            for entry in fs::read_dir(&path).expect("read tree") {
                let path = entry.expect("tree entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    found.push((
                        path.strip_prefix(&base).expect("relative").to_path_buf(),
                        fs::read(&path).expect("read tree file"),
                    ));
                }
            }
        }
        found.sort();
        found
    }

    fn run_shepherd(&self, state_dir: &Path, wrapper: &Path) -> (i32, Value) {
        let home = self.root.join("home");
        fs::create_dir_all(&home).expect("home");
        let fake_log = self.root.join("harness.log");
        let path = format!(
            "{}:{}",
            self.root.join("fakebin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut command = Command::new(env!("CARGO_BIN_EXE_agentctl"));
        command
            .current_dir(&self.root)
            .env("HOME", &home)
            .env("AGENTCTL_HOME", home.join(".agentctl"))
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .env("PATH", path)
            .env("FAKE_LOG", &fake_log)
            .env("AGENTCTL_TEST_HARNESS", "1")
            .env("AGENTCTL_SYSTEMCTL", self.root.join("systemctl"))
            .env("AGENTCTL_SYSTEMD_RUN", self.root.join("systemd-run"))
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_PANE_ID")
            .env_remove("AGENTCTL_AGENT")
            .arg("--registry")
            .arg(self.root.join("own"))
            .arg("--herdr-bin")
            .arg(self.root.join("herdr"))
            .args(["team", "shepherd", "--config"])
            .arg(self.root.join("teams.json"))
            .args(["--state-dir"])
            .arg(state_dir)
            .args([
                "--max-rounds",
                "3",
                "--round-delay-seconds",
                "0",
                "--no-progress-rounds",
                "2",
                "--call-timeout-seconds",
                "120",
                "--act-wait-seconds",
                "0",
                "--run-agent-prompt",
            ])
            .arg(wrapper)
            .stdin(Stdio::null());
        let output = self.spawn_grouped(&mut command);
        let code = output.status.code().unwrap_or(-1);
        let text = String::from_utf8(output.stdout).expect("report is UTF-8");
        let report: Value = serde_json::from_str(&text)
            .unwrap_or_else(|_| panic!("shepherd report not JSON (exit {code}): {text}"));
        (code, report)
    }

    fn stamp(&self, state_dir: &Path) -> Value {
        fn find(dir: &Path, found: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(dir).expect("read shepherd state") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    find(&path, found);
                } else if path.file_name().is_some_and(|name| name == "stamp.json") {
                    found.push(path);
                }
            }
        }
        let mut stamps = Vec::new();
        find(&state_dir.join("shepherd"), &mut stamps);
        assert_eq!(stamps.len(), 1, "one stamp per team incident: {stamps:?}");
        serde_json::from_str(&fs::read_to_string(&stamps[0]).expect("stamp")).expect("stamp JSON")
    }

    /// Ordered profiles for the own team plus a passing smoke record,
    /// so the supervisor may consult a model at all (fail closed).
    fn install_profiles(&self, state_dir: &Path) {
        let profiles = format!(
            "schema = \"agentctl-recovery-profiles/v1\"\n\
             [[profile]]\nname = \"slow-muse\"\nharness = \"muse\"\ntimeout_seconds = 10\ncommand = \"{}\"\n\
             [[profile]]\nname = \"answering-claude\"\nharness = \"claude\"\ntimeout_seconds = 30\ncommand = \"{}\"\n",
            self.root.join("fakebin/muse").display(),
            self.root.join("fakebin/claude").display(),
        );
        write_private(&self.root.join("own/recovery-profiles.toml"), &profiles);
        let mut args = self.base_args();
        args.extend([
            "recovery".to_owned(),
            "smoke".to_owned(),
            "--state-dir".to_owned(),
            state_dir.display().to_string(),
            "--run-agent-prompt".to_owned(),
            wrapper().display().to_string(),
        ]);
        let (code, report) = self.run_agentctl(&args);
        assert_eq!(code, 0, "smoke must pass before judgement: {report}");
        // Judgement calls start from an empty harness log.
        let _ = fs::remove_file(self.root.join("harness.log"));
    }
}

fn wrapper() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../py/bin/run-agent-prompt")
}

#[test]
fn shepherd_drill_gives_up_bounded_and_judges_cross_team_read_only() {
    let scratch = Scratch::new("drill");
    let judged_before = scratch.tree("judged");
    let state = scratch.root.join("state");
    scratch.install_profiles(&state);

    let (code, report) = scratch.run_shepherd(&state, &wrapper());
    assert_eq!(code, 75, "{report}");
    assert_eq!(report["verdict"], "GAVE-UP");
    assert_eq!(report["resumed"], false);

    let stamp = scratch.stamp(&state);
    assert_eq!(stamp["schema"], "agentctl-team-shepherd-stamp/v1");
    assert_eq!(stamp["verdict"], "GAVE-UP");
    // muse timed out, the claude profile answered: ordered profiles
    // with timeout fall-through, judged by profile name.
    assert_eq!(stamp["judge"], "answering-claude");
    let calls = fs::read_to_string(scratch.root.join("harness.log")).expect("harness log");
    let calls: Vec<&str> = calls.lines().collect();
    assert_eq!(&calls[..2], ["muse", "claude"], "fallback order: {calls:?}");
    assert!(!calls.contains(&"codex"), "codex must not run: {calls:?}");
    // Both teams judged; the owning team reports its record gap.
    let teams = stamp["teams"].as_object().expect("teams");
    assert_eq!(teams.len(), 2, "{stamp}");
    let own_key = scratch.root.join("own").display().to_string();
    assert!(
        teams[&own_key]["record_gaps"].as_u64().expect("gaps") >= 1,
        "{stamp}"
    );
    assert_eq!(teams[&own_key]["yes"], false);
    // The deterministic arm ran for its own team.
    fn find_log(dir: &Path) -> Option<PathBuf> {
        for entry in fs::read_dir(dir).expect("read state") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                if let Some(found) = find_log(&path) {
                    return Some(found);
                }
            } else if path.file_name().is_some_and(|name| name == "shepherd.log") {
                return Some(path);
            }
        }
        None
    }
    let log = fs::read_to_string(find_log(&state.join("shepherd")).expect("log path"))
        .expect("shepherd log");
    assert!(log.contains("restore"), "{log}");
    // The judged team was never acted on: its records are untouched
    // and no restore ran there. (A manifest probe may create empty
    // lock files; that read side effect is not an action.)
    let judged_after = scratch.tree("judged");
    for (name, bytes) in &judged_before {
        let after = judged_after
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, bytes)| bytes);
        assert_eq!(after, Some(bytes), "judged {name:?} changed");
    }
    assert!(
        !scratch.root.join("judged/restore").exists(),
        "shepherd restored a judged team"
    );

    // The launcher seam recorded every unit operation; nothing
    // reached the real user manager (guard).
    let systemd_log = fs::read_to_string(scratch.root.join("systemd.log"))
        .expect("unit operations must go through the fake launcher seam");
    assert!(
        systemd_log.contains("systemd-run") || systemd_log.contains("systemctl"),
        "{systemd_log}"
    );
    // Enqueue-only acts: the shepherd never starts a unit or arms a
    // wake timer (blocker: acts must not kick).
    assert!(
        !systemd_log.contains("--user start") && !systemd_log.contains("--on-active"),
        "shepherd acts kicked a driver: {systemd_log}"
    );
    // One incident per epoch: a rerun reports the stamp, repeats nothing.
    let before = fs::read_to_string(scratch.root.join("harness.log")).expect("log");
    let (code, report) = scratch.run_shepherd(&state, &wrapper());
    assert_eq!(code, 75, "{report}");
    assert_eq!(report["resumed"], true);
    assert_eq!(
        fs::read_to_string(scratch.root.join("harness.log")).expect("log"),
        before,
        "rerun repeated a one-shot call"
    );
    scratch.reap();
}

#[test]
fn shepherd_drill_judge_none_when_every_harness_is_down() {
    let scratch = Scratch::new("nojudge");
    let failing = scratch.root.join("failing-wrapper");
    install_executable(&failing, "#!/bin/sh\nexit 69\n");
    let state = scratch.root.join("state-none");
    let (code, report) = scratch.run_shepherd(&state, &failing);
    assert_eq!(code, 75, "{report}");
    let stamp = scratch.stamp(&state);
    assert_eq!(stamp["judge"], "none", "{stamp}");
    assert_eq!(stamp["verdict"], "GAVE-UP");
    scratch.reap();
}

#[test]
fn install_units_shepherd_writes_only_the_boot_unit() {
    let scratch = Scratch::new("units");
    let home = scratch.root.join("home");
    fs::create_dir_all(&home).expect("home");
    let dest = scratch.root.join("units");
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentctl"));
    command
        .current_dir(&scratch.root)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env_remove("AGENTCTL_AGENT")
        .arg("--registry")
        .arg(scratch.root.join("own"))
        .arg("--herdr-bin")
        .arg(scratch.root.join("herdr"))
        .args(["install-units", "--dest"])
        .arg(&dest)
        .arg("--shepherd")
        .stdin(Stdio::null());
    let output = scratch.spawn_grouped(&mut command);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let unit_path = fs::read_dir(&dest)
        .expect("dest")
        .next()
        .expect("one unit")
        .expect("unit entry")
        .path();
    let unit = fs::read_to_string(&unit_path).expect("unit");
    assert!(unit.contains("start-recovery"), "{unit}");
    assert!(
        !unit.contains("After="),
        "must not order on restore: {unit}"
    );
    assert!(!unit.contains("Requires="), "{unit}");
    assert!(unit.contains("WantedBy=default.target"), "{unit}");
    let report: Value = serde_json::from_slice(&output.stdout).expect("report JSON");
    assert!(
        report["next_steps"]
            .as_array()
            .expect("steps")
            .iter()
            .any(|step| step
                .as_str()
                .is_some_and(|step| step.contains("enable agentctl-shepherd-"))),
        "{report}"
    );
    let written: Vec<_> = fs::read_dir(&dest)
        .expect("dest")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(written.len(), 1, "only the shepherd unit: {written:?}");
    scratch.reap();
}

impl Scratch {
    fn run_agentctl(&self, args: &[String]) -> (i32, Value) {
        let home = self.root.join("home");
        fs::create_dir_all(&home).expect("home");
        let path = format!(
            "{}:{}",
            self.root.join("fakebin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut command = Command::new(env!("CARGO_BIN_EXE_agentctl"));
        command
            .current_dir(&self.root)
            .env("HOME", &home)
            .env("AGENTCTL_HOME", home.join(".agentctl"))
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .env("PATH", path)
            .env("AGENTCTL_TEST_HARNESS", "1")
            .env("AGENTCTL_SYSTEMCTL", self.root.join("systemctl"))
            .env("AGENTCTL_SYSTEMD_RUN", self.root.join("systemd-run"))
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_PANE_ID")
            .env_remove("AGENTCTL_AGENT")
            .args(args)
            .stdin(Stdio::null());
        let output = self.spawn_grouped(&mut command);
        let code = output.status.code().unwrap_or(-1);
        let text = String::from_utf8(output.stdout).expect("report is UTF-8");
        let report: Value = serde_json::from_str(&text)
            .unwrap_or_else(|_| panic!("report not JSON (exit {code}): {text}"));
        (code, report)
    }

    fn base_args(&self) -> Vec<String> {
        vec![
            "--registry".to_owned(),
            self.root.join("own").display().to_string(),
            "--herdr-bin".to_owned(),
            self.root.join("herdr").display().to_string(),
        ]
    }
}

#[test]
fn recovery_smoke_records_profiles_and_start_recovery_claims_once() {
    let scratch = Scratch::new("smoke");
    let state = scratch.root.join("state");
    // Ordered profiles at the team level: Claude first, then Codex —
    // no built-in first harness is consulted.
    let profiles = format!(
        "schema = \"agentctl-recovery-profiles/v1\"\n\
         [[profile]]\nname = \"local-claude\"\nharness = \"claude\"\ncommand = \"{}\"\n\
         [[profile]]\nname = \"local-codex\"\nharness = \"codex\"\ncommand = \"{}\"\n",
        scratch.root.join("fakebin/claude").display(),
        scratch.root.join("fakebin/codex").display(),
    );
    write_private(&scratch.root.join("own/recovery-profiles.toml"), &profiles);

    let mut args = scratch.base_args();
    args.extend([
        "recovery".to_owned(),
        "smoke".to_owned(),
        "--state-dir".to_owned(),
        state.display().to_string(),
        "--run-agent-prompt".to_owned(),
        wrapper().display().to_string(),
    ]);
    let (code, report) = scratch.run_agentctl(&args);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["chain"]["judge"], "local-claude", "{report}");
    let rows = report["profiles"].as_array().expect("profile rows");
    assert_eq!(rows.len(), 2, "{report}");
    assert!(rows.iter().all(|row| row["status"] == "pass"), "{report}");

    // A claim from a previous boot never blocks a new start.
    let recovery = state
        .join("recovery")
        .join(crate_team_key(&scratch.root.join("own")));
    fs::create_dir_all(&recovery).expect("recovery dir");
    write_private(
        &recovery.join("claim.json"),
        "{\"schema\": \"agentctl-recovery-claim/v1\", \"boot_id\": \"previous-boot\", \
         \"pid\": 999999, \"heartbeat_at\": 1.0}",
    );
    let mut args = scratch.base_args();
    args.extend([
        "start-recovery".to_owned(),
        "--state-dir".to_owned(),
        state.display().to_string(),
        "--run-agent-prompt".to_owned(),
        wrapper().display().to_string(),
    ]);
    let (code, report) = scratch.run_agentctl(&args);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["status"], "started", "{report}");
    let supervisor_pid = report["supervisor_pid"].as_u64().expect("supervisor pid");
    // A second call while the supervisor lives joins, never restarts.
    let (code, report) = scratch.run_agentctl(&args);
    assert_eq!(code, 75, "{report}");
    assert_eq!(report["status"], "already-running", "{report}");
    // The test owns this child; stop it only after verifying its
    // cmdline still names this scratch tree.
    let cmdline = fs::read_to_string(format!("/proc/{supervisor_pid}/cmdline")).unwrap_or_default();
    if cmdline.contains(&scratch.root.display().to_string()) {
        let _ = Command::new("kill")
            .arg(supervisor_pid.to_string())
            .status();
    }
    if let Some(restore_pid) = report["restore_pid"].as_u64() {
        let _ = restore_pid;
    }
    let status_args = {
        let mut args = scratch.base_args();
        args.extend([
            "recovery".to_owned(),
            "status".to_owned(),
            "--state-dir".to_owned(),
            state.display().to_string(),
        ]);
        args
    };
    let (code, report) = scratch.run_agentctl(&status_args);
    assert_eq!(code, 0, "{report}");
    assert_eq!(
        report["claim"]["boot_id"]
            .as_str()
            .map(|b| b != "previous-boot"),
        Some(true)
    );
}

fn crate_team_key(registry: &Path) -> String {
    use sha2::{Digest, Sha256};
    let canonical = fs::canonicalize(registry).unwrap_or_else(|_| registry.to_path_buf());
    let digest = format!(
        "{:x}",
        Sha256::digest(canonical.to_string_lossy().as_bytes())
    );
    digest[..16].to_owned()
}

#[test]
fn two_registries_share_one_state_root_without_sharing_stamps() {
    let scratch = Scratch::new("twoteams");
    scratch.register("other");
    let state = scratch.root.join("state");
    let (code, report) = scratch.run_shepherd(&state, &wrapper());
    assert_eq!(code, 75, "{report}");
    // A second team under the same state root starts its own
    // incident; it never resumes the first team's stamp (B1).
    let home = scratch.root.join("home");
    let mut args = vec![
        "--registry".to_owned(),
        scratch.root.join("other").display().to_string(),
        "--herdr-bin".to_owned(),
        scratch.root.join("herdr").display().to_string(),
        "team".to_owned(),
        "shepherd".to_owned(),
        "--state-dir".to_owned(),
        state.display().to_string(),
        "--max-rounds".to_owned(),
        "2".to_owned(),
        "--round-delay-seconds".to_owned(),
        "0".to_owned(),
        "--run-agent-prompt".to_owned(),
        wrapper().display().to_string(),
    ];
    args.push("--config".to_owned());
    args.push(scratch.root.join("teams.json").display().to_string());
    let (code, report) = scratch.run_agentctl(&args);
    assert_eq!(code, 75, "{report}");
    assert_eq!(report["resumed"], false, "second team resumed: {report}");
    let _ = home;
    scratch.reap();
}

#[test]
fn shepherd_without_smoke_record_consults_no_model() {
    let scratch = Scratch::new("gated");
    // Profiles configured, but recovery smoke never ran: judgement
    // must stay unavailable (fail closed, S1).
    let profiles = format!(
        "schema = \"agentctl-recovery-profiles/v1\"\n\
         [[profile]]\nname = \"answering-claude\"\nharness = \"claude\"\ncommand = \"{}\"\n",
        scratch.root.join("fakebin/claude").display(),
    );
    write_private(&scratch.root.join("own/recovery-profiles.toml"), &profiles);
    let state = scratch.root.join("state");
    let (code, report) = scratch.run_shepherd(&state, &wrapper());
    assert_eq!(code, 75, "{report}");
    assert!(
        !scratch.root.join("harness.log").exists(),
        "wrapper consulted without a smoke record"
    );
    let stamp = scratch.stamp(&state);
    assert_eq!(stamp["judge"], "none", "{stamp}");
    scratch.reap();
}

#[test]
fn generated_unit_execstart_actually_starts_recovery() {
    let scratch = Scratch::new("unitrun");
    let state = scratch.root.join("state");
    scratch.install_profiles(&state);
    let dest = scratch.root.join("units");
    let mut args = scratch.base_args();
    args.extend([
        "recovery".to_owned(),
        "install".to_owned(),
        "--write-unit".to_owned(),
        "--unit-dest".to_owned(),
        dest.display().to_string(),
        "--state-dir".to_owned(),
        state.display().to_string(),
        "--run-agent-prompt".to_owned(),
        wrapper().display().to_string(),
    ]);
    let (code, report) = scratch.run_agentctl(&args);
    assert_eq!(code, 0, "{report}");
    let units: Vec<PathBuf> = fs::read_dir(&dest)
        .expect("unit dest")
        .map(|entry| entry.expect("unit entry").path())
        .collect();
    assert_eq!(units.len(), 1, "one per-registry unit: {units:?}");
    assert!(
        units[0]
            .file_name()
            .expect("unit name")
            .to_string_lossy()
            .starts_with("agentctl-shepherd-"),
        "{units:?}"
    );
    let unit = fs::read_to_string(&units[0]).expect("unit");
    let exec = unit
        .lines()
        .find_map(|line| line.strip_prefix("ExecStart="))
        .expect("ExecStart line")
        .to_owned();
    let mut tokens: Vec<String> = exec.split_whitespace().map(str::to_owned).collect();
    tokens[0] = env!("CARGO_BIN_EXE_agentctl").to_owned();
    // Run the generated line exactly as rendered (B3), in the
    // scratch environment, and prove recovery starts.
    let (code, report) = scratch.run_agentctl(&tokens[1..]);
    assert_eq!(code, 0, "generated ExecStart failed: {report}");
    assert_eq!(report["status"], "started", "{report}");
    scratch.reap();
}

#[test]
fn no_progress_bound_stops_early_after_restore_settles() {
    let scratch = Scratch::new("noprogress");
    let state = scratch.root.join("state");
    let (code, report) = scratch.run_shepherd(&state, &wrapper());
    assert_eq!(code, 75, "{report}");
    let stamp = scratch.stamp(&state);
    // max-rounds is 3 in run_shepherd; use the stamp of a dedicated
    // run below for the early-stop proof.
    assert_eq!(stamp["verdict"], "GAVE-UP");
    scratch.reap();

    // Dedicated run: max-rounds 10, no-progress 1 — the loop must
    // stop well before 10 once facts stop changing (S2/NB1 settle).
    let scratch = Scratch::new("noprogress2");
    let state = scratch.root.join("state");
    let mut args = scratch.base_args();
    args.extend([
        "team".to_owned(),
        "shepherd".to_owned(),
        "--state-dir".to_owned(),
        state.display().to_string(),
        "--max-rounds".to_owned(),
        "10".to_owned(),
        "--no-progress-rounds".to_owned(),
        "1".to_owned(),
        "--round-delay-seconds".to_owned(),
        "0".to_owned(),
        "--call-timeout-seconds".to_owned(),
        "120".to_owned(),
        "--run-agent-prompt".to_owned(),
        wrapper().display().to_string(),
    ]);
    let (code, report) = scratch.run_agentctl(&args);
    assert_eq!(code, 75, "{report}");
    assert!(
        report["rounds"].as_u64().expect("rounds") < 10,
        "no-progress bound never fired: {report}"
    );
    scratch.reap();
}

#[test]
fn tampered_profiles_lose_judgement() {
    let scratch = Scratch::new("tamper");
    let state = scratch.root.join("state");
    scratch.install_profiles(&state);
    // Change a model after the smoke: the record no longer binds.
    let path = scratch.root.join("own/recovery-profiles.toml");
    let text = fs::read_to_string(&path).expect("profiles");
    fs::write(&path, text.replace("slow-muse", "slow-muse-two")).expect("tamper");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("mode");
    let _ = fs::remove_file(scratch.root.join("harness.log"));
    let (code, report) = scratch.run_shepherd(&state, &wrapper());
    assert_eq!(code, 75, "{report}");
    assert!(
        !scratch.root.join("harness.log").exists(),
        "wrapper consulted on a stale smoke record"
    );
    scratch.reap();
}

const DRILL_CERT: &str = "-----BEGIN CERTIFICATE-----\nMIIDATCCAemgAwIBAgIUWgamaOG2iz+doY19sRp6IRDQmoswDQYJKoZIhvcNAQEL\nBQAwEDEOMAwGA1UEAwwFZHJpbGwwHhcNMjYxMTI0MDkwNDMyWhcNMzYxMTIxMDkw\nNDMyWjAQMQ4wDAYDVQQDDAVkcmlsbDCCASIwDQYJKoZIhvcNAQEBBQADggEPADCC\nAQoCggEBAJh2L3kf3aOhenj7emkvKwHrCbrK/bWsmafTnvtfKVJarVw45wlHHGqa\nQ8S8FAa7n9Pv7UXeguA4s3/e6ggFLqObdmERPYUCgWjY9dJ40ZFJIdyLVbTUBooC\ncIM4FCelRkfiWQVjdTzV32KnGw5JwOq/1+gza5kFFtfpIETTvaCBkFIMVtwDJZXH\n9jO9NAudz7w1xE7WtUezJKcDtRZovq9j7NNYq6HRKTORvIv86WXV7CVRKtP6rzKZ\nrpaPA5nfwfHdRN4X7DzVJIhqOaD7D5gDm356+wkgixNckaqeSE2hCQnWdD3hed/k\ns1DAlgx6/wLoaSQiC9ULam1IRwinRUkCAwEAAaNTMFEwHQYDVR0OBBYEFKN7YFg6\nuEYm63WqvFGV30/usbWEMB8GA1UdIwQYMBaAFKN7YFg6uEYm63WqvFGV30/usbWE\nMA8GA1UdEwEB/wQFMAMBAf8wDQYJKoZIhvcNAQELBQADggEBAHBCfCauWKMos9VQ\ndMrdMM9uwM8+b8c9DYvVu8e4FdssSL7vQ5Yh5c1OB/w6rwrgDX9SlhDMuMp9veVq\nGTaiov+T2U6svWjS3GQ08Aq4SpJ8jN07dGzMYPEfftSD5VxONC90+EqCE1wXK/zh\nW7K2D3r2szN713uUjkhqMn3vi0FibkY47g3+c8zwTJ2Ox4Ly/60ccVndeKGc9QaX\nOaZ7moR4Pa0yNyxfgkFKRclpNv+WP/yKztpTJnQd3I0kGFfiFLJYzMoIsj8thRzu\n8bJBaKRp8ZcFxj3+TFGvUVbYrqfVJ6zskZa/gSJqZDoaBZpDojpWONk5HjJFBd59\ngjziovY=\n-----END CERTIFICATE-----\n";

#[test]
fn blocking_restore_blocks_stamp_and_acts_until_released() {
    use std::os::unix::process::CommandExt;
    let scratch = Scratch::new("blocked");
    // The restore child waits for a certificate file that does not
    // exist yet: a condition-based blocker, released by writing it.
    let cert = scratch.root.join("cert.pem");
    let registry = scratch.root.join("own");
    write_private(
        &registry.join("restore.json"),
        &format!(
            "{{\"schema\": \"agentctl-restore/v1\", \"coordinator\": \"lane\", \
             \"units\": [], \"herdr_wait_seconds\": 0, \"startup_delay_seconds\": 0, \
             \"certificate_file\": \"{}\", \"certificate_wait_seconds\": 3600}}",
            cert.display()
        ),
    );
    let state = scratch.root.join("state");
    let home = scratch.root.join("home");
    fs::create_dir_all(&home).expect("home");
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentctl"));
    command
        .current_dir(&scratch.root)
        .process_group(0)
        .env("HOME", &home)
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("AGENTCTL_TEST_HARNESS", "1")
        .env("AGENTCTL_SYSTEMCTL", scratch.root.join("systemctl"))
        .env("AGENTCTL_SYSTEMD_RUN", scratch.root.join("systemd-run"))
        .env_remove("AGENTCTL_AGENT")
        .arg("--registry")
        .arg(&registry)
        .arg("--herdr-bin")
        .arg(scratch.root.join("herdr"))
        .args(["team", "shepherd", "--state-dir"])
        .arg(&state)
        .args([
            "--max-rounds",
            "50",
            "--round-delay-seconds",
            "1",
            "--no-progress-rounds",
            "1",
            "--call-timeout-seconds",
            "120",
            "--run-agent-prompt",
        ])
        .arg(wrapper())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = command.spawn().expect("spawn supervisor");
    scratch.groups.borrow_mut().push(GroupGuard {
        pgid: child.id(),
        armed: std::cell::Cell::new(true),
    });
    // While the restore waits: wait on the condition (the recorded
    // restore child in the supervisor state), never a fixed sleep.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    fn recorded_child(dir: &Path) -> bool {
        if !dir.exists() {
            return false;
        }
        for entry in fs::read_dir(dir).expect("read state") {
            let path = entry.expect("entry").path();
            if path.is_dir() && recorded_child(&path) {
                return true;
            }
            if path.file_name().is_some_and(|name| name == "state.json") {
                if let Ok(text) = fs::read_to_string(&path) {
                    if let Ok(value) = serde_json::from_str::<Value>(&text) {
                        if value["restore_pid"].as_u64().is_some_and(|pid| pid != 0) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
    while !recorded_child(&state.join("shepherd")) {
        assert!(
            std::time::Instant::now() < deadline,
            "supervisor never recorded its restore child"
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    // Several rounds pass with the child recorded; still no stamp
    // and no unit start from any act.
    std::thread::sleep(std::time::Duration::from_secs(3));
    fn any_stamp(dir: &Path) -> bool {
        if !dir.exists() {
            return false;
        }
        for entry in fs::read_dir(dir).expect("read state") {
            let path = entry.expect("entry").path();
            if path.is_dir() && any_stamp(&path) {
                return true;
            }
            if path.file_name().is_some_and(|name| name == "stamp.json") {
                return true;
            }
        }
        false
    }
    assert!(
        !any_stamp(&state.join("shepherd")),
        "stamped while its restore was still blocked"
    );
    // Release the restore with the embedded throwaway certificate;
    // the incident may now finish and stamp.
    fs::write(&cert, DRILL_CERT).expect("release cert");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    while !any_stamp(&state.join("shepherd")) {
        assert!(
            std::time::Instant::now() < deadline,
            "no stamp after the restore was released"
        );
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    drop(child);
    scratch.reap();
}
