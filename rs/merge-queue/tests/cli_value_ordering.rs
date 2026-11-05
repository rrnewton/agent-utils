//! Value-ordered scheduling: with `value_ordering` on, a waiting batch whose historical
//! green rate beats the deepest in-flight batch's aborts it (the aborted batch is
//! replanned later in the same run) and takes its slot.

mod support;

use std::path::PathBuf;
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
    ("TESTTMP", "1"),
];

struct Daemon(Child);

impl Daemon {
    fn start(fx: &Fixture) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_merge-queue"));
        command
            .args([
                "serve",
                "--once",
                "--poll-ms",
                "50",
                "--base-poll-secs",
                "1",
            ])
            .args(["--depth", "2", "--max-batch", "1"])
            .current_dir(fx.queue())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(fx.root.join("serve.err")).unwrap());
        for (key, value) in IDENTITY {
            command.env(key, value);
        }
        command.env_remove("MERGE_QUEUE_STATE");
        Self(command.spawn().expect("spawn serve"))
    }

    fn exit_code(&mut self, timeout: Duration) -> Option<i32> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if let Ok(Some(status)) = self.0.try_wait() {
                return status.code();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the daemon did not exit within {timeout:?}");
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn started(fx: &Fixture) -> Vec<String> {
    std::fs::read_to_string(fx.root.join("started"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn wait_for(fx: &Fixture, what: &str, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let serve = std::fs::read_to_string(fx.root.join("serve.err")).unwrap_or_default();
    panic!(
        "timed out waiting for {what}; started: {:?}; serve: {serve}",
        started(fx)
    );
}

#[test]
fn a_better_rated_waiting_batch_aborts_the_deepest_in_flight() {
    let fx = Fixture::new("value-ordering");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    std::fs::create_dir_all(fx.root.join("gates")).unwrap();
    // Every validation records its candidate's test files, waits for the gate named
    // after them, then passes everything.
    let validator = format!(
        r#"echo validating $(ls tests) >&2; key=$(ls tests | sort | tr '\n' '_'); echo $key >> '{root}/started'; i=0; while [ ! -e '{root}/gates/'$key ] && [ $i -lt 1200 ]; do sleep 0.05; i=$((i+1)); done; for t in $(ls tests); do printf '{{"id":"%s","outcome":"pass"}}\n' "$t" >> "$MQ_RESULTS"; done"#,
        root = fx.root.display()
    );
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &validator,
        "--value-ordering",
    ]);
    // History: #2 always red, #3 always green (base rates order the window).
    let decisions = fx
        .queue()
        .join(".git/merge-queue/decisions/2026-01-01.jsonl");
    std::fs::create_dir_all(decisions.parent().unwrap()).unwrap();
    let mut history = String::new();
    for _ in 0..3 {
        history.push_str("{\"outcome\":\"red\",\"changes\":[{\"pr\":2}]}\n");
        history.push_str("{\"outcome\":\"green\",\"changes\":[{\"pr\":3}]}\n");
    }
    std::fs::write(&decisions, history).unwrap();

    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let mut daemon = Daemon::start(&fx);
    wait_for(&fx, "the window to fill", || started(&fx).len() >= 2);
    assert_eq!(started(&fx), vec!["a_t0_", "a_b_t0_"]);
    // #3 arrives while the window is full: its rate (1.0) beats #2's (0.0), so #2's
    // validation is aborted and #3 takes the slot on top of #1.
    fx.branch("c", &[("tests/c", "ok\n")]);
    let out = fx.mq_ok(&["submit", "3", "--head", "c"]);
    assert!(out.contains("recorded #3"), "{out}");
    wait_for(&fx, "change 3 to take the aborted slot", || {
        started(&fx).iter().any(|line| line == "a_c_t0_")
    });
    // Let everything finish: #1 lands, then #3 on it, then #2 replanned on the new base.
    for key in ["a_t0_", "a_b_t0_", "a_c_t0_", "a_b_c_t0_"] {
        std::fs::write(fx.root.join("gates").join(key), "").unwrap();
    }
    assert_eq!(daemon.exit_code(Duration::from_secs(60)), Some(0));
    let runs = fx.queue().join(".git/merge-queue/runs");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    let run = dirs.pop().unwrap();
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(run.join("report.json")).unwrap()).unwrap();
    let landed: Vec<Vec<i64>> = report["landed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|batch| {
            batch["prs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pr| pr.as_i64().unwrap())
                .collect()
        })
        .collect();
    assert_eq!(landed, vec![vec![1], vec![3], vec![2]], "{report}");
    let cancelled_two = report["validations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["prs"] == serde_json::json!([2]) && v["cancelled"] == true);
    assert!(cancelled_two, "{report}");
    let events = std::fs::read_to_string(run.join("events.jsonl")).unwrap();
    assert!(events.contains("value-abort"), "{events}");
}
