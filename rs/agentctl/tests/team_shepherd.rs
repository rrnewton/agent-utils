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

struct Scratch {
    root: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
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
        let scratch = Self { root };
        scratch.register("own");
        scratch.register("judged");
        // Fake Herdr and fake one-shot harnesses: muse hangs past the
        // per-attempt timeout, claude answers with one allowlisted
        // proposal, codex must never be reached.
        let bin = scratch.root.join("fakebin");
        fs::create_dir_all(&bin).expect("fake bin");
        install_executable(&scratch.root.join("herdr"), FAKE_HERDR);
        // The wrapper gives each attempt an explicit-name environment,
        // so the fakes log to this scratch path baked into the script.
        let log = scratch.root.join("harness.log");
        install_executable(
            &bin.join("muse"),
            &format!("#!/bin/sh\necho muse >> {}\nsleep 10\n", log.display()),
        );
        install_executable(
            &bin.join("claude"),
            &format!(
                "#!/bin/sh\necho claude >> {}\ncat > /dev/null\nprintf '%s\\n' \
                 '{{\"type\": \"result\", \"subtype\": \"success\", \"is_error\": false, \
                 \"result\": \"down\\n{{\\\"action\\\": \\\"facts\\\"}}\"}}'\n",
                log.display()
            ),
        );
        install_executable(
            &bin.join("codex"),
            &format!("#!/bin/sh\necho codex >> {}\n", log.display()),
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
        let output = Command::new(env!("CARGO_BIN_EXE_agentctl"))
            .current_dir(&self.root)
            .env("HOME", &home)
            .env("AGENTCTL_HOME", home.join(".agentctl"))
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .env("PATH", path)
            .env("FAKE_LOG", &fake_log)
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
                "1",
                "--act-wait-seconds",
                "0",
                "--run-agent-prompt",
            ])
            .arg(wrapper)
            .stdin(Stdio::null())
            .output()
            .expect("run agentctl team shepherd");
        let code = output.status.code().unwrap_or(-1);
        let text = String::from_utf8(output.stdout).expect("report is UTF-8");
        let report: Value = serde_json::from_str(&text)
            .unwrap_or_else(|_| panic!("shepherd report not JSON (exit {code}): {text}"));
        (code, report)
    }

    fn stamp(&self, state_dir: &Path) -> Value {
        let boots = fs::read_dir(state_dir.join("shepherd"))
            .expect("shepherd state")
            .map(|entry| entry.expect("boot entry").path())
            .collect::<Vec<_>>();
        assert_eq!(boots.len(), 1, "one incident per boot: {boots:?}");
        serde_json::from_str(&fs::read_to_string(boots[0].join("stamp.json")).expect("stamp"))
            .expect("stamp JSON")
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

    let (code, report) = scratch.run_shepherd(&state, &wrapper());
    assert_eq!(code, 75, "{report}");
    assert_eq!(report["verdict"], "GAVE-UP");
    assert_eq!(report["resumed"], false);

    let stamp = scratch.stamp(&state);
    assert_eq!(stamp["schema"], "agentctl-team-shepherd-stamp/v1");
    assert_eq!(stamp["verdict"], "GAVE-UP");
    // muse timed out, claude answered: fallback order with fall-through.
    assert_eq!(stamp["judge"], "claude");
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
    // The deterministic arm ran (dry run, then real) for its own team.
    let boots = fs::read_dir(state.join("shepherd")).expect("boots");
    let boot = boots
        .map(|entry| entry.expect("boot").path())
        .next()
        .expect("one boot");
    let log = fs::read_to_string(boot.join("shepherd.log")).expect("shepherd log");
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
}

#[test]
fn install_units_shepherd_writes_only_the_boot_unit() {
    let scratch = Scratch::new("units");
    let home = scratch.root.join("home");
    fs::create_dir_all(&home).expect("home");
    let dest = scratch.root.join("units");
    let output = Command::new(env!("CARGO_BIN_EXE_agentctl"))
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
        .stdin(Stdio::null())
        .output()
        .expect("run install-units --shepherd");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let unit = fs::read_to_string(dest.join("agentctl-shepherd.service")).expect("unit");
    assert!(unit.contains("team shepherd"), "{unit}");
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
                .is_some_and(|step| step.contains("enable agentctl-shepherd.service"))),
        "{report}"
    );
    let written: Vec<_> = fs::read_dir(&dest)
        .expect("dest")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(written.len(), 1, "only the shepherd unit: {written:?}");
}
