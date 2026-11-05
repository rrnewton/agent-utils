//! The daemon's label sync (`serve --sync-label`) runs beside its loop. Its API call can wait
//! minutes on the request budget, and nothing waits for it: queued changes are planned and
//! validated from the first cycle, and the heartbeat stays fresh, while the listing is still
//! out. A sync that finds a new change while a run holds the queue records the submission, and
//! the run takes it into a free slot of its window.

mod support;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use support::{git, Fixture};

const IDENTITY: [(&str, &str); 7] = [
    ("GIT_AUTHOR_NAME", "Dev"),
    ("GIT_AUTHOR_EMAIL", "dev@example.com"),
    ("GIT_COMMITTER_NAME", "Dev"),
    ("GIT_COMMITTER_EMAIL", "dev@example.com"),
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    // Marks a test run (see support/mod.rs).
    ("TESTTMP", "1"),
];

/// A `gh` whose `pr list` waits (at most 60 s) until `$GH_RELEASE` exists, the way a paced
/// call waits on its request budget, then prints `$GH_LIST`.
const SLOW_GH: &str = r#"#!/bin/sh
echo "$*" >> "$GH_LOG"
case "$1 $2" in
  "pr list")
    i=0; while [ ! -e "$GH_RELEASE" ] && [ $i -lt 1200 ]; do sleep 0.05; i=$((i+1)); done
    cat "$GH_LIST" ;;
esac
"#;

struct Gh {
    stub: PathBuf,
    log: PathBuf,
    list: PathBuf,
    release: PathBuf,
}

impl Gh {
    fn new(fx: &Fixture) -> Self {
        let stub = fx.root.join("gh-slow");
        std::fs::write(&stub, SLOW_GH).unwrap();
        Command::new("chmod")
            .args(["+x", stub.to_str().unwrap()])
            .status()
            .unwrap();
        let gh = Self {
            stub,
            log: fx.root.join("gh.log"),
            list: fx.root.join("gh-list.json"),
            release: fx.root.join("gh-release"),
        };
        std::fs::write(&gh.list, "[]").unwrap();
        gh
    }

    /// The listing `pr list` prints: these pull requests, labelled `mq-ready`.
    fn lists(&self, prs: &[(i64, &str)]) {
        let prs: Vec<Value> = prs
            .iter()
            .map(|(number, head)| {
                serde_json::json!({
                    "number": number, "headRefOid": head, "isDraft": false,
                    "title": format!("change {number}"), "author": {"login": "lane"},
                    "labels": [{"name": "mq-ready"}]
                })
            })
            .collect();
        std::fs::write(&self.list, serde_json::to_string(&prs).unwrap()).unwrap();
    }

    /// Let the waiting `pr list` answer.
    fn release(&self) {
        std::fs::write(&self.release, "").unwrap();
    }
}

/// A pull request: a branch with `files`, pushed as `refs/pull/<number>/head`.
fn pull_request(fx: &Fixture, number: i64, files: &[(&str, &str)]) -> String {
    let head = fx.branch(&format!("pr{number}"), files);
    git(
        &fx.dev(),
        &[
            "push",
            "-q",
            "origin",
            &format!("{head}:refs/pull/{number}/head"),
        ],
    );
    head
}

/// A queue on a stub hosting repository whose validations append their files to `started`
/// and, when `gated`, pass only once the returned gate file exists (at most 60 s).
fn queue(tag: &str, gated: bool) -> (Fixture, Gh, PathBuf, PathBuf) {
    let fx = Fixture::new(tag);
    let gh = Gh::new(&fx);
    let started = fx.root.join("started");
    let gate = fx.root.join("validations-may-pass");
    let wait = gated
        .then_some(gate.as_path())
        .map_or(String::new(), |gate: &Path| {
            format!(
            "i=0; while [ ! -e '{gate}' ] && [ $i -lt 1200 ]; do sleep 0.05; i=$((i+1)); done; ",
            gate = gate.display()
        )
        });
    let validator = format!(
        "echo $(ls) >> '{started}'; {wait}exit 0",
        started = started.display()
    );
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--github-repo",
        "owner/name",
        "--gh",
        gh.stub.to_str().unwrap(),
        "--validator",
        "command",
        "--validator-run",
        &validator,
    ]);
    (fx, gh, started, gate)
}

struct Daemon(Child);

impl Daemon {
    fn start(fx: &Fixture, gh: &Gh) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_merge-queue"));
        command
            .args([
                "serve",
                "--poll-ms",
                "50",
                "--base-poll-secs",
                "1",
                "--sync-label",
                "mq-ready",
            ])
            .current_dir(fx.queue())
            .env("GH_LOG", &gh.log)
            .env("GH_LIST", &gh.list)
            .env("GH_RELEASE", &gh.release)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (key, value) in IDENTITY {
            command.env(key, value);
        }
        command.env_remove("MERGE_QUEUE_STATE");
        Self(command.spawn().expect("spawn serve"))
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for(what: &str, timeout: Duration, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out after {timeout:?} waiting for {what}");
}

fn state_dir(fx: &Fixture) -> PathBuf {
    let common = PathBuf::from(git(&fx.queue(), &["rev-parse", "--git-common-dir"]));
    let common = if common.is_absolute() {
        common
    } else {
        fx.queue().join(common)
    };
    common.join("merge-queue")
}

fn heartbeat(fx: &Fixture) -> Value {
    std::fs::read_to_string(state_dir(fx).join("serve.heartbeat.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

fn item(fx: &Fixture, pr: i64) -> Value {
    std::fs::read_to_string(state_dir(fx).join("items").join(format!("{pr}.json")))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

fn status(fx: &Fixture, pr: i64) -> String {
    item(fx, pr)["status"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[test]
fn a_label_sync_waiting_on_its_budget_never_holds_up_planning_or_the_heartbeat() {
    let (fx, gh, _, _) = queue("serve-sync-slow", false);
    pull_request(&fx, 1, &[("one", "1\n")]);
    fx.mq_ok(&["submit", "1", "--head", "pr1"]);
    let _daemon = Daemon::start(&fx, &gh);
    // The listing is still out (nothing released it), yet the queued change lands.
    wait_for(
        "#1 to land while the label sync waits",
        Duration::from_secs(30),
        || status(&fx, 1) == "landed",
    );
    assert!(!gh.release.exists());
    let log = std::fs::read_to_string(&gh.log).unwrap_or_default();
    assert!(log.contains("pr list"), "the sync never started: {log}");
    // The heartbeat stays fresh and says a sync is in flight.
    wait_for("a fresh heartbeat", Duration::from_secs(10), || {
        let hb = heartbeat(&fx);
        hb["heartbeat_unix"].as_u64().unwrap_or(0) + 2 >= unix_now()
            && hb["cycles"].as_u64().unwrap_or(0) >= 2
    });
    let hb = heartbeat(&fx);
    assert!(
        !hb["sync_started_at"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "{hb}"
    );
    assert!(hb.get("last_sync").is_none(), "{hb}");
    // Once the listing answers, the sync is folded in.
    gh.release();
    wait_for("the sync to finish", Duration::from_secs(30), || {
        heartbeat(&fx)["last_sync"]
            .as_str()
            .is_some_and(|text| text.contains("0 listed"))
    });
    assert!(heartbeat(&fx).get("sync_started_at").is_none());
}

#[test]
fn a_change_a_sync_finds_while_a_run_holds_the_queue_joins_that_run() {
    let (fx, gh, started, gate) = queue("serve-sync-during-run", true);
    pull_request(&fx, 1, &[("one", "1\n")]);
    let h2 = pull_request(&fx, 2, &[("two", "2\n")]);
    fx.mq_ok(&["submit", "1", "--head", "pr1"]);
    gh.lists(&[(2, &h2)]);
    let _daemon = Daemon::start(&fx, &gh);
    // #1's validation is running, and holds the queue's writer lock, before the listing answers.
    wait_for("#1's validation to start", Duration::from_secs(30), || {
        started.exists()
    });
    gh.release();
    // The sync finds #2 while the run holds the lock: it records the submission, and the run
    // takes it into its window beside #1.
    wait_for("#2 to validate beside #1", Duration::from_secs(30), || {
        std::fs::read_to_string(&started)
            .unwrap_or_default()
            .lines()
            .any(|line| line.split_whitespace().any(|file| file == "two"))
    });
    assert_eq!(status(&fx, 1), "queued", "#1 landed before the gate opened");
    std::fs::write(&gate, "").unwrap();
    wait_for("both to land", Duration::from_secs(30), || {
        status(&fx, 1) == "landed" && status(&fx, 2) == "landed"
    });
    let history = item(&fx, 2)["history"].to_string();
    assert!(
        history.contains("while another writer held the queue"),
        "#2 was not recorded for the run: {history}"
    );
    assert_eq!(item(&fx, 2)["source"], "label:mq-ready");
    wait_for("the sync summary", Duration::from_secs(10), || {
        heartbeat(&fx)["last_sync"]
            .as_str()
            .is_some_and(|text| text.contains("recorded [#2]"))
    });
}
