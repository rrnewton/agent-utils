//! `agentctl turnover` end to end (#686 agent-turnover): a headless agent with a fake
//! stream-json `claude` on PATH is turned over three ways (mechanically during a running turn,
//! after a handoff that came too late, and with the agent's own handoff), and once abandoned.
//! Each new session must read the handoff first, then the messages that waited for it; the
//! record keeps its name and token and changes only its session; the lineage shows every step.
//! Skips (passes) when python3 is not installed. Linux only: the wrapper reads `/proc`.

#![cfg(target_os = "linux")]

use std::fs::{self, DirBuilder};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::Value;

/// A stand-in for `claude -p` stream-json, enough for the wrapper: each input is acknowledged,
/// started, written to the transcript and answered. `SLOW` makes a turn last five seconds. A
/// turnover handoff request is answered after `FAKE_HANDOFF_DELAY` seconds by writing the file it
/// names, ending with its nonce line. Every input is logged to `FAKE_INPUTS` with its session.
const FAKE_CLAUDE: &str = r##"#!/usr/bin/env python3
import json, os, re, sys, threading, time, queue
args = sys.argv[1:]
session = args[args.index("--session-id") + 1] if "--session-id" in args else args[args.index("--resume") + 1]
config = os.environ["CLAUDE_CONFIG_DIR"]
project = re.sub(r"[^A-Za-z0-9]", "-", os.getcwd())
transcript = os.path.join(config, "projects", project, session + ".jsonl")
inputs = queue.Queue()
def reader():
    for line in sys.stdin:
        inputs.put(json.loads(line))
    inputs.put(None)
threading.Thread(target=reader, daemon=True).start()
def out(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()
def life(uuid, state):
    out({"type": "command_lifecycle", "command_uuid": uuid, "state": state})
def handoff(text):
    found = re.search(r"handoff for your successor to this file:\n\n(\S+)\n", text)
    nonce = re.search(r"<!-- agentctl-turnover \S+ -->", text)
    if not found or not nonce:
        return
    time.sleep(float(os.environ.get("FAKE_HANDOFF_DELAY", "0")))
    with open(found.group(1), "w") as handle:
        handle.write("# Handoff from the fake\n\nPurpose: test the turnover.\n\n" + nonce.group(0) + "\n")
while True:
    message = inputs.get()
    if message is None:
        break
    if message.get("type") == "control_request":
        out({"type": "control_response", "response": {"subtype": "success",
             "request_id": message["request_id"], "response": {"still_queued": []}}})
        continue
    uuid = message["uuid"]
    text = message["message"]["content"]
    with open(os.environ["FAKE_INPUTS"], "a") as log:
        log.write(json.dumps({"session": session, "text": text}) + "\n")
    life(uuid, "queued")
    life(uuid, "started")
    os.makedirs(os.path.dirname(transcript), exist_ok=True)
    with open(transcript, "a") as handle:
        handle.write(json.dumps({"type": "user", "uuid": uuid}) + "\n")
    out({"type": "system", "subtype": "init", "claude_code_version": "9.9.9", "model": "fake",
         "permissionMode": "dontAsk", "session_id": session, "tools": ["Bash"]})
    if "SLOW" in text:
        time.sleep(5)
    if "agentctl-turnover" in text and "Write ONE self-contained handoff" in text:
        handoff(text)
    out({"type": "assistant", "parent_tool_use_id": None, "message": {"content": [
        {"type": "text", "text": "echo: " + text[:40]}]}})
    out({"type": "result", "subtype": "success", "duration_ms": 5,
         "usage": {"input_tokens": 1, "output_tokens": 2}, "permission_denials": []})
    life(uuid, "completed")
"##;

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
    bin: PathBuf,
    config: PathBuf,
    inputs: PathBuf,
}

impl Fixture {
    fn new() -> Option<Self> {
        executable("python3")?;
        let root = std::env::temp_dir().join(format!(
            "agentctl-turnover-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|now| now.as_nanos())
                .unwrap_or(0)
        ));
        let work = root.join("work");
        let bin = root.join("bin");
        let config = root.join("claude-config");
        for directory in [&root, &work, &bin, &config] {
            DirBuilder::new()
                .mode(0o700)
                .recursive(true)
                .create(directory)
                .unwrap();
        }
        let fake = bin.join("claude");
        fs::write(&fake, FAKE_CLAUDE).unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        Some(Self {
            registry: root.join("registry"),
            inputs: root.join("inputs.log"),
            root,
            work,
            bin,
            config,
        })
    }

    fn agentctl(&self, args: &[&str]) -> Output {
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Command::new(env!("CARGO_BIN_EXE_agentctl"))
            .arg("--registry")
            .arg(&self.registry)
            .args(args)
            .env("PATH", path)
            .env("CLAUDE_CONFIG_DIR", &self.config)
            .env("FAKE_INPUTS", &self.inputs)
            .env("FAKE_HANDOFF_DELAY", "3")
            .env_remove("AGENTCTL_AGENT")
            .env_remove("HERDR_WORKSPACE_ID")
            .output()
            .expect("run agentctl")
    }

    fn json(&self, args: &[&str]) -> Value {
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

    fn inputs(&self) -> Vec<(String, String)> {
        fs::read_to_string(&self.inputs)
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let value: Value = serde_json::from_str(line).unwrap();
                (
                    value["session"].as_str().unwrap().to_owned(),
                    value["text"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    fn wait_for(&self, what: &str, mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !check() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn session(&self) -> String {
        let record: Value = serde_json::from_slice(
            &fs::read(self.registry.join("agents/to-turn/agent.json")).unwrap(),
        )
        .unwrap();
        record["headless"]["session"].as_str().unwrap().to_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.agentctl(&["stop", "to-turn"]);
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn first_input_of(inputs: &[(String, String)], session: &str) -> Vec<String> {
    inputs
        .iter()
        .filter(|(owner, _)| owner == session)
        .map(|(_, text)| text.clone())
        .collect()
}

#[test]
fn a_headless_agent_turns_over_to_a_fresh_session_that_reads_the_handoff_first() {
    let Some(fixture) = Fixture::new() else {
        eprintln!("python3 not installed; skipping");
        return;
    };
    let work = fixture.work.display().to_string();
    fixture.json(&[
        "start",
        "to-turn",
        "--mode",
        "headless",
        "--harness",
        "claude",
        "--cwd",
        &work,
    ]);
    let token = |fixture: &Fixture| -> String {
        let record: Value = serde_json::from_slice(
            &fs::read(fixture.registry.join("agents/to-turn/agent.json")).unwrap(),
        )
        .unwrap();
        record["token"].as_str().unwrap().to_owned()
    };
    let first_token = token(&fixture);
    let first = fixture.session();

    // 1. Mechanical, while a turn runs: the message queued behind it waits for the new session.
    fixture.json(&["send", "to-turn", "SLOW one"]);
    let queued = fixture.agentctl(&["send", "to-turn", "queued two", "--ready-timeout", "0"]);
    assert!(queued.status.code().is_some(), "{queued:?}");
    let turned = fixture.json(&["turnover", "to-turn", "--no-ask", "--wait", "60"]);
    assert_eq!(turned["turned_over"], true, "{turned:#}");
    assert_eq!(turned["handoff_kind"], "mechanical");
    let second = fixture.session();
    assert_ne!(second, first);
    assert_eq!(turned["new_session"], second.as_str());
    assert_eq!(token(&fixture), first_token, "the record keeps its token");
    fixture.wait_for("the new session's inputs", || {
        first_input_of(&fixture.inputs(), &second).len() >= 2
    });
    let seen = first_input_of(&fixture.inputs(), &second);
    assert!(
        seen[0].starts_with("[from agentctl-turnover via agentctl]")
            && seen[0].contains("Mechanical handoff for to-turn")
            && seen[0].contains(&format!("previous session ({first})")),
        "{}",
        seen[0]
    );
    assert!(seen[1].ends_with("queued two"), "{}", seen[1]);
    assert!(first_input_of(&fixture.inputs(), &first)
        .iter()
        .all(|text| !text.ends_with("queued two")));
    assert!(!fixture
        .registry
        .join(".lineage/to-turn/turnover.json")
        .exists());

    // 2. A handoff that comes too late: exit 75, nothing changed, the journal waits.
    let late = fixture.agentctl(&["turnover", "to-turn", "--handoff-timeout", "1"]);
    assert_eq!(late.status.code(), Some(75), "{late:?}");
    let report: Value = serde_json::from_slice(&late.stdout).unwrap();
    assert_eq!(report["timed_out"], true);
    assert_eq!(fixture.session(), second);
    let journal: Value = serde_json::from_slice(
        &fs::read(fixture.registry.join(".lineage/to-turn/turnover.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(journal["phase"], "asking");

    // 3. Rerun: the same request's handoff arrives, and the new session reads it first.
    let turned = fixture.json(&["turnover", "to-turn", "--handoff-timeout", "60"]);
    assert_eq!(turned["handoff_kind"], "agent", "{turned:#}");
    let third = fixture.session();
    assert_ne!(third, second);
    fixture.wait_for("the third session's first input", || {
        !first_input_of(&fixture.inputs(), &third).is_empty()
    });
    let seen = first_input_of(&fixture.inputs(), &third);
    assert!(seen[0].contains("# Handoff from the fake"), "{}", seen[0]);
    assert!(!seen[0].contains("<!-- agentctl-turnover"), "{}", seen[0]);
    let requests = first_input_of(&fixture.inputs(), &second)
        .iter()
        .filter(|text| text.contains("Write ONE self-contained handoff"))
        .count();
    assert_eq!(requests, 1, "a rerun waits for the request it already made");
    let handoff = fs::read_to_string(turned["handoff"].as_str().unwrap()).unwrap();
    assert!(handoff.ends_with(" -->\n"));

    // 4. Abandoned while asking: the agent keeps its session.
    let late = fixture.agentctl(&["turnover", "to-turn", "--handoff-timeout", "1"]);
    assert_eq!(late.status.code(), Some(75));
    let aborted = fixture.json(&["turnover", "to-turn", "--abort"]);
    assert_eq!(aborted["aborted"], true);
    assert_eq!(fixture.session(), third);
    assert!(!fixture
        .registry
        .join(".lineage/to-turn/turnover.json")
        .exists());

    // The lineage shows both turnovers, each with its handoff.
    let history = fixture.json(&["history", "to-turn", "--json"]);
    let steps: Vec<(String, String, String)> = history["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["kind"].as_str().unwrap().to_owned(),
                entry
                    .get("relation")
                    .or_else(|| entry.get("reason"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                entry["session"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let expected = [
        ("generation", "start", first.as_str()),
        ("ended", "turned-over", first.as_str()),
        ("generation", "turnover", second.as_str()),
        ("ended", "turned-over", second.as_str()),
        ("generation", "turnover", third.as_str()),
    ]
    .map(|(kind, detail, session)| (kind.to_owned(), detail.to_owned(), session.to_owned()));
    assert_eq!(steps, expected, "{history:#}");
    let entries = history["entries"].as_array().unwrap();
    assert_eq!(entries[4]["status"], "live");
    assert_eq!(entries[2]["status"], "turned-over");
    assert_eq!(entries[2]["handoff"]["kind"], "mechanical");
    assert!(Path::new(entries[4]["handoff"]["path"].as_str().unwrap()).is_file());
}
