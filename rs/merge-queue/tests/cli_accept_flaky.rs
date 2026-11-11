//! Rulings on batches that passed only after retries (GREEN_FLAKY). The known-flaky list is a
//! standing ruling: a batch whose retried tests are all on it lands at once. `accept-flaky` is a
//! ruling on one held batch: a run lands exactly the commit that was validated, without
//! validating it again, if the base has not moved; otherwise the batch is validated again.

mod support;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use fs2::FileExt;
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

/// A queue whose validator passes every test file, appends a line to `count` per validation,
/// and, while the file `flaky` exists, says the tests it names passed only after a retry.
/// Returns the fixture, `count` and `flaky`.
fn setup(tag: &str, extra: &[&str]) -> (Fixture, PathBuf, PathBuf) {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let count = fx.root.join("count");
    let flaky = fx.root.join("flaky");
    let validator = format!(
        r#"echo run >> '{count}'
for t in $(ls tests); do printf '{{"id":"%s","outcome":"pass"}}\n' "$t" >> "$MQ_RESULTS"; done
if [ -e '{flaky}' ]; then echo "GREEN_FLAKY: $(cat '{flaky}')"; fi
exit 0"#,
        count = count.display(),
        flaky = flaky.display()
    );
    let mut args = vec![
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &validator,
    ];
    args.extend_from_slice(extra);
    fx.mq_ok(&args);
    (fx, count, flaky)
}

fn validations(count: &Path) -> usize {
    std::fs::read_to_string(count)
        .unwrap_or_default()
        .lines()
        .count()
}

fn state_dir(fx: &Fixture) -> PathBuf {
    fx.queue().join(".git/merge-queue")
}

fn run(fx: &Fixture) -> Value {
    let out = fx.mq(&["run", "--json"]);
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap_or_else(|error| {
        panic!(
            "bad report ({error}): {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn item(fx: &Fixture, pr: i64) -> Value {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["pr"] == pr && item["status"] != "pending-intake")
        .cloned()
        .unwrap_or(Value::Null)
}

fn history_kinds(item: &Value) -> Vec<String> {
    item["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["kind"].as_str().unwrap().to_owned())
        .collect()
}

fn rulings(fx: &Fixture) -> Vec<Value> {
    std::fs::read_to_string(state_dir(fx).join("decisions/rulings.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn landed(report: &Value) -> Vec<Vec<i64>> {
    report["landed"]
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
        .collect()
}

/// Submit change 1 (`tests/a`), have its validation pass only after a retry of `a`, and return
/// the commit the hold records and the base it was built on.
fn hold_one(fx: &Fixture, flaky: &Path) -> (String, String) {
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    std::fs::write(flaky, "a").unwrap();
    let report = run(fx);
    assert!(landed(&report).is_empty(), "{report}");
    let held = item(fx, 1);
    assert_eq!(held["status"], "queued", "{held}");
    let candidate = &held["held_candidate"];
    let tip = candidate["tip"].as_str().unwrap().to_owned();
    let parent = candidate["parent"].as_str().unwrap().to_owned();
    assert_eq!(parent, fx.remote_main());
    assert_eq!(candidate["changes"][0]["pr"], 1, "{held}");
    (tip, parent)
}

#[test]
fn a_batch_whose_retried_tests_are_all_known_flaky_lands_without_a_hold() {
    let (fx, count, flaky) = setup("known-flaky-lands", &[]);
    let added = fx.mq_ok(&[
        "known-flaky",
        "add",
        "a",
        "--issue",
        "https://example.invalid/issues/1",
        "--by",
        "owner",
    ]);
    assert!(added.contains("a is on the known-flaky list"), "{added}");
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    // The validator names the test in one of its cells; the entry covers its cells.
    std::fs::write(&flaky, "a[fast/debug]").unwrap();
    let report = run(&fx);
    assert_eq!(landed(&report), vec![vec![1]], "{report}");
    assert_eq!(
        report["landed"][0]["flaky"],
        serde_json::json!(["a[fast/debug]"]),
        "{report}"
    );
    assert_eq!(report["landed"][0]["tip"], fx.remote_main().as_str());
    assert_eq!(validations(&count), 1);
    let landed_item = item(&fx, 1);
    assert_eq!(landed_item["status"], "landed", "{landed_item}");
    assert!(
        landed_item.get("flaky_hold_until").is_none(),
        "{landed_item}"
    );
    let rulings = rulings(&fx);
    assert_eq!(rulings.len(), 1, "{rulings:?}");
    assert_eq!(rulings[0]["ruling"], "known-flaky");
    assert_eq!(rulings[0]["outcome"], "landed");
    assert_eq!(rulings[0]["flaky"], serde_json::json!(["a[fast/debug]"]));
    assert_eq!(rulings[0]["commit"], fx.remote_main().as_str());
    let reason = rulings[0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("a[fast/debug] as a: https://example.invalid/issues/1"),
        "{reason}"
    );
    // Records beside the validation records do not break the metrics read from them.
    std::fs::write(
        state_dir(&fx).join("decisions/skips.jsonl"),
        r#"{"decision":"skip","pr":9,"reason":"recount-pins refused","run":"r","at":"2026-11-04T00:00:00Z"}
"#,
    )
    .unwrap();
    let stats: Value = serde_json::from_str(&fx.mq_ok(&["stats", "--json"])).unwrap();
    assert_eq!(stats["landed"], 1, "{stats}");
    // The list, as listed.
    let listed: Value =
        serde_json::from_str(&fx.mq_ok(&["known-flaky", "list", "--json"])).unwrap();
    assert_eq!(listed["entries"][0]["test"], "a", "{listed}");
    assert_eq!(listed["entries"][0]["by"], "owner", "{listed}");
    fx.mq_ok(&["known-flaky", "remove", "a", "--by", "owner"]);
    assert!(fx
        .mq_ok(&["known-flaky", "list"])
        .contains("the known-flaky list is empty"));
}

#[test]
fn a_retried_test_not_on_the_list_still_holds_the_batch() {
    let (fx, _count, flaky) = setup("known-flaky-partial", &[]);
    fx.mq_ok(&[
        "known-flaky",
        "add",
        "a",
        "--issue",
        "tracker-1",
        "--by",
        "owner",
    ]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let before = fx.remote_main();
    // `a-2` is not a cell or a case of `a`.
    std::fs::write(&flaky, "a,a-2").unwrap();
    let report = run(&fx);
    assert!(landed(&report).is_empty(), "{report}");
    assert!(
        report["stopped"].as_str().unwrap().contains("GREEN_FLAKY"),
        "{report}"
    );
    assert_eq!(fx.remote_main(), before);
    let held = item(&fx, 1);
    assert_eq!(
        held["flaky_nodes"],
        serde_json::json!(["a", "a-2"]),
        "{held}"
    );
    assert!(rulings(&fx).is_empty());
}

#[test]
fn accept_flaky_lands_exactly_the_held_commit_without_validating_again() {
    let (fx, count, flaky) = setup("accept-flaky-lands", &[]);
    let (tip, parent) = hold_one(&fx, &flaky);
    assert_eq!(validations(&count), 1);
    // The plan names the ruling that lands it.
    let plan = fx.mq_ok(&["plan"]);
    assert!(
        plan.contains("merge-queue accept-flaky 1 --by WHO --reason WHY"),
        "{plan}"
    );
    let out = fx.mq_ok(&[
        "accept-flaky",
        "1",
        "--by",
        "owner",
        "--reason",
        "a known flake",
    ]);
    assert!(
        out.contains("accepted the GREEN_FLAKY passes of #1"),
        "{out}"
    );
    let accepted = item(&fx, 1);
    assert_eq!(accepted["accepted"]["by"], "owner", "{accepted}");
    let report = run(&fx);
    assert_eq!(landed(&report), vec![vec![1]], "{report}");
    assert_eq!(report["landed"][0]["tip"], tip.as_str(), "{report}");
    assert_eq!(report["landed"][0]["flaky"], serde_json::json!(["a"]));
    assert!(
        report["landed"][0]["note"]
            .as_str()
            .unwrap()
            .contains("accepted by owner: a known flake"),
        "{report}"
    );
    // Not validated again.
    assert!(
        report["validations"].as_array().unwrap().is_empty(),
        "{report}"
    );
    assert_eq!(validations(&count), 1);
    assert_eq!(fx.remote_main(), tip);
    let landed_item = item(&fx, 1);
    assert_eq!(landed_item["status"], "landed", "{landed_item}");
    assert_eq!(landed_item["landed_as"], tip.as_str());
    assert!(landed_item.get("held_candidate").is_none(), "{landed_item}");
    assert!(landed_item.get("accepted").is_none(), "{landed_item}");
    let kinds = history_kinds(&landed_item);
    assert!(kinds.contains(&"accepted-flaky".to_owned()), "{kinds:?}");
    let rulings = rulings(&fx);
    assert_eq!(rulings.len(), 1, "{rulings:?}");
    assert_eq!(rulings[0]["ruling"], "accept-flaky");
    assert_eq!(rulings[0]["outcome"], "landed");
    assert_eq!(rulings[0]["by"], "owner");
    assert_eq!(rulings[0]["reason"], "a known flake");
    assert_eq!(rulings[0]["commit"], tip.as_str());
    assert_eq!(rulings[0]["base"], parent.as_str());
}

#[test]
fn accept_flaky_after_the_base_moved_validates_the_batch_again() {
    let (fx, count, flaky) = setup("accept-flaky-moved", &[]);
    let (tip, parent) = hold_one(&fx, &flaky);
    // Someone else lands first.
    fx.write("other.txt", "elsewhere\n");
    fx.commit_all("other: change");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let moved = fx.remote_main();
    assert_ne!(moved, parent);
    fx.mq_ok(&[
        "accept-flaky",
        "1",
        "--by",
        "owner",
        "--reason",
        "a known flake",
    ]);
    std::fs::remove_file(&flaky).unwrap();
    let report = run(&fx);
    // Validated again on the new base, and landed on that validation.
    assert_eq!(landed(&report), vec![vec![1]], "{report}");
    let landed_tip = report["landed"][0]["tip"].as_str().unwrap();
    assert_ne!(landed_tip, tip);
    assert_eq!(
        report["validations"].as_array().unwrap().len(),
        1,
        "{report}"
    );
    assert_eq!(validations(&count), 2);
    let kinds = history_kinds(&item(&fx, 1));
    assert!(
        kinds.contains(&"accepted-flaky-requeued".to_owned()),
        "{kinds:?}"
    );
    let rulings = rulings(&fx);
    assert_eq!(rulings.len(), 1, "{rulings:?}");
    assert_eq!(rulings[0]["outcome"], "requeued");
    let detail = rulings[0]["detail"].as_str().unwrap();
    assert_eq!(
        detail,
        format!(
            "the base moved from {} to {} since {} was validated",
            &parent[..10],
            &moved[..10],
            &tip[..10]
        )
    );
}

#[test]
fn accept_flaky_names_the_whole_held_batch_and_only_held_changes() {
    let (fx, _count, flaky) = setup("accept-flaky-refusals", &[]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    std::fs::write(&flaky, "a").unwrap();
    let report = run(&fx);
    assert!(landed(&report).is_empty(), "{report}");
    let (one, two) = (item(&fx, 1), item(&fx, 2));
    assert_eq!(one["held_candidate"], two["held_candidate"]);
    assert_eq!(
        one["held_candidate"]["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|change| change["pr"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let refused = |args: &[&str], expected: &str| {
        let out = fx.mq(args);
        assert!(!out.status.success(), "{args:?} was accepted");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
    };
    refused(
        &["accept-flaky", "1", "--by", "owner", "--reason", "flake"],
        "the held batch is #1, #2",
    );
    refused(
        &["accept-flaky", "3", "--by", "owner", "--reason", "flake"],
        "#3 is not held for a ruling",
    );
    refused(
        &[
            "accept-flaky",
            "1",
            "2",
            "3",
            "--by",
            "owner",
            "--reason",
            "flake",
        ],
        "#3 is not held for a ruling",
    );
    refused(
        &["accept-flaky", "1", "2", "--by", " ", "--reason", "flake"],
        "names who gives it",
    );
    assert!(item(&fx, 1).get("accepted").is_none());
    // In any order, the whole batch.
    fx.mq_ok(&[
        "accept-flaky",
        "2",
        "1",
        "--by",
        "owner",
        "--reason",
        "flake",
    ]);
    assert_eq!(item(&fx, 1)["accepted"]["by"], "owner");
    assert_eq!(item(&fx, 2)["accepted"]["by"], "owner");
}

#[test]
fn accept_flaky_while_the_lock_is_held_is_recorded_and_applied_by_the_next_run() {
    let (fx, count, flaky) = setup("accept-flaky-intake", &[]);
    let (tip, _) = hold_one(&fx, &flaky);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state_dir(&fx).join("lock"))
        .unwrap();
    lock.try_lock_exclusive().unwrap();
    let out = fx.mq_ok(&[
        "accept-flaky",
        "1",
        "--by",
        "owner",
        "--reason",
        "a known flake",
    ]);
    assert!(out.contains("accept-flaky #1: recorded"), "{out}");
    let rows: Value = serde_json::from_str(&fx.mq_ok(&["list", "--json"])).unwrap();
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|row| row["status"] == "pending-intake" && row["op"] == "accept-flaky"),
        "{rows}"
    );
    assert!(item(&fx, 1).get("accepted").is_none());
    drop(lock);
    let report = run(&fx);
    assert_eq!(landed(&report), vec![vec![1]], "{report}");
    assert_eq!(report["landed"][0]["tip"], tip.as_str());
    assert_eq!(validations(&count), 1);
}

#[test]
fn with_confirmation_a_rerun_that_retried_only_known_flaky_tests_lands() {
    // Same-commit confirmation on: the batch fails `a` once, and the rerun of `a` on the same
    // commit passes only after a retry. `a` is known flaky, so that is a plain flaky pass.
    let fx = Fixture::new("known-flaky-rerun");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let seen = fx.root.join("seen");
    let validator = format!(
        r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests); else set -- $MQ_TESTS; fi
if [ ! -e '{seen}' ]; then
  touch '{seen}'
  for t in "$@"; do o=pass; [ "$t" = a ] && o=fail; printf '{{"id":"%s","outcome":"%s"}}\n' "$t" "$o" >> "$MQ_RESULTS"; done
  exit 1
fi
for t in "$@"; do printf '{{"id":"%s","outcome":"pass"}}\n' "$t" >> "$MQ_RESULTS"; done
echo "GREEN_FLAKY: a"
exit 0"#,
        seen = seen.display()
    );
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &validator,
        "--confirm-reruns",
    ]);
    fx.mq_ok(&[
        "known-flaky",
        "add",
        "a",
        "--issue",
        "tracker-2",
        "--by",
        "owner",
    ]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let report = run(&fx);
    assert_eq!(landed(&report), vec![vec![1]], "{report}");
    assert_eq!(report["landed"][0]["flaky"], serde_json::json!(["a"]));
    assert_eq!(item(&fx, 1)["status"], "landed");
}

struct Daemon(Child);

impl Daemon {
    /// One run (`--once`), two batches of one change each in flight at most.
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

fn wait_for(what: &str, timeout: Duration, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

/// #1272 held-dependent-after: B (--after held A) is never validated on
/// a candidate without A while A is held GREEN_FLAKY.
#[test]
fn a_dependent_of_a_held_change_is_not_validated_without_it() {
    let (fx, count, flaky) = setup("held-dependent-after", &[]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    std::fs::write(&flaky, "a").unwrap();
    let report = run(&fx);
    assert!(landed(&report).is_empty(), "{report}");
    assert_eq!(item(&fx, 1)["status"], "queued");
    // B depends on A and stacks on its branch.
    fx.branch_from("b", "a", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b", "--after", "1"]);
    let before = validations(&count);
    let report = run(&fx);
    assert!(landed(&report).is_empty(), "{report}");
    assert_eq!(item(&fx, 1)["status"], "queued", "{report}");
    assert_eq!(item(&fx, 2)["status"], "queued", "{report}");
    let validated_alone = report["validations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["prs"] == serde_json::json!([2]));
    assert!(
        !validated_alone,
        "change 2 validated without change 1: {report}"
    );
    let _ = before;
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_ruling_taken_in_mid_run_cancels_the_batch_in_flight_and_lands_first() {
    // Each validation appends its candidate's test files to `started` and waits (at most 60 s)
    // for the gate named after them; while the file `flaky/<files>` exists, it passes only
    // after a retry of the test named there.
    let fx = Fixture::new("accept-flaky-mid-run");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let started = fx.root.join("started");
    let gates = fx.root.join("gates");
    let flaky = fx.root.join("flaky");
    std::fs::create_dir_all(&gates).unwrap();
    std::fs::create_dir_all(&flaky).unwrap();
    let validator = format!(
        r#"set=$(ls tests | tr '\n' _); echo "$set" >> '{started}'
i=0; while [ ! -e '{gates}'/"$set" ] && [ $i -lt 1200 ]; do sleep 0.05; i=$((i+1)); done
[ -e '{gates}'/"$set" ] || exit 1
for t in $(ls tests); do printf '{{"id":"%s","outcome":"pass"}}\n' "$t" >> "$MQ_RESULTS"; done
if [ -e '{flaky}'/"$set" ]; then echo "GREEN_FLAKY: $(cat '{flaky}'/"$set")"; fi
exit 0"#,
        started = started.display(),
        gates = gates.display(),
        flaky = flaky.display()
    );
    // In a file: the daemon's preflight reads the tools a validator command names.
    let script = fx.root.join("validator.sh");
    std::fs::write(&script, validator).unwrap();
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &format!("sh '{}'", script.display()),
    ]);
    // Change 1 is held after passing only after a retry.
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    std::fs::write(gates.join("a_t0_"), "").unwrap();
    std::fs::write(flaky.join("a_t0_"), "a").unwrap();
    let report = run(&fx);
    assert!(landed(&report).is_empty(), "{report}");
    let tip = item(&fx, 1)["held_candidate"]["tip"]
        .as_str()
        .unwrap()
        .to_owned();
    // Change 2 validates (and waits at its gate) when the ruling on change 1 is given.
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let mut daemon = Daemon::start(&fx);
    let deadline = Instant::now() + Duration::from_secs(30);
    while lines(&started).len() < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        lines(&started).len() >= 2,
        "change 2 never validated; serve said:\n{}",
        std::fs::read_to_string(fx.root.join("serve.err")).unwrap_or_default()
    );
    assert_eq!(
        lines(&started),
        vec!["a_t0_".to_owned(), "b_t0_".to_owned()]
    );
    let out = fx.mq_ok(&[
        "accept-flaky",
        "1",
        "--by",
        "owner",
        "--reason",
        "a known flake",
    ]);
    assert!(out.contains("recorded"), "{out}");
    // The run takes the ruling in, cancels change 2 (built on the base without change 1),
    // lands change 1 as validated, and validates change 2 again on top of it.
    wait_for(
        "change 2 to validate again",
        Duration::from_secs(30),
        || lines(&started).len() >= 3,
    );
    assert_eq!(fx.remote_main(), tip);
    std::fs::write(gates.join("a_b_t0_"), "").unwrap();
    assert_eq!(daemon.exit_code(Duration::from_secs(60)), Some(0));
    assert_eq!(
        lines(&started),
        vec!["a_t0_".to_owned(), "b_t0_".to_owned(), "a_b_t0_".to_owned()]
    );
    assert_eq!(item(&fx, 1)["status"], "landed");
    assert_eq!(item(&fx, 1)["landed_as"], tip.as_str());
    assert_eq!(item(&fx, 2)["status"], "landed");
    let rulings = rulings(&fx);
    assert_eq!(rulings.len(), 1, "{rulings:?}");
    assert_eq!(rulings[0]["outcome"], "landed");
}
