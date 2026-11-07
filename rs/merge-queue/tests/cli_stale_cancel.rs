//! A donor job cancelled as stale after the base moves is recomposed on
//! the new base, never retried as the identical candidate: the stale
//! sweep and the run's own stale path agree.

mod support;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
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

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_merge-queue")
}

fn command(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Command {
    let mut command = Command::new(bin());
    command.args(args).current_dir(dir);
    for (key, value) in IDENTITY.iter().chain(env) {
        command.env(key, value);
    }
    command.env("MERGE_QUEUE_ALLOW_DIRTY", "1");
    command.env_remove("MERGE_QUEUE_STATE");
    command
}

fn donor_clone(fx: &Fixture, name: &str) -> PathBuf {
    git(&fx.root, &["clone", "-q", "remote.git", name]);
    fx.root.join(name)
}

fn route(fx: &Fixture) -> String {
    format!("{} --repo {}", bin(), fx.queue().display())
}

fn donor_run(fx: &Fixture, clone: &Path, name: &str, poll_secs: &str) -> Output {
    let route = route(fx);
    let clone_text = clone.display().to_string();
    command(
        &fx.root,
        &[
            "donor",
            "run",
            "--queue",
            &route,
            "--clone",
            &clone_text,
            "--name",
            name,
            "--poll-secs",
            poll_secs,
            "--until-idle",
        ],
        &[],
    )
    .output()
    .unwrap()
}

fn spawn_run(fx: &Fixture, extra: &[&str]) -> Child {
    let mut args = vec!["run", "--json"];
    args.extend_from_slice(extra);
    command(&fx.queue(), &args, &[])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn finish(run: Child) -> (Option<i32>, Value, String) {
    let out = run.wait_with_output().unwrap();
    let report = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (
        out.status.code(),
        report,
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn jobs(fx: &Fixture) -> Vec<Value> {
    let out = fx.mq_ok(&["donor", "status", "--json"]);
    serde_json::from_str::<Value>(&out)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

fn wait_for(what: &str, timeout: Duration, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out waiting for {what}");
}

fn item_status(fx: &Fixture, pr: i64) -> String {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["pr"] == pr && i["status"] != "pending-intake")
        .map(|i| i["status"].as_str().unwrap().to_owned())
        .unwrap_or_else(|| "missing".to_owned())
}

fn set_config(fx: &Fixture, edit: impl FnOnce(&mut Value)) {
    let path = fx.queue().join(".git/merge-queue/config.json");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    edit(&mut config);
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
}

fn job_path(fx: &Fixture, id: &str) -> PathBuf {
    fx.queue()
        .join(".git")
        .join("merge-queue")
        .join("donor")
        .join("jobs")
        .join(format!("{id}.json"))
}

/// Rewrite donor job `id`'s state as a donor holding (or no longer holding) its lease would.
fn set_lease(fx: &Fixture, id: &str, held: bool) {
    let path = job_path(fx, id);
    let mut job: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    if held {
        job["state"] = Value::String("leased".to_owned());
        job["lease"] = serde_json::json!({
            "donor": "holder", "host": "elsewhere", "token": "held-by-the-test",
            "claimed_unix": 0, "expires_unix": u64::MAX / 2,
        });
    } else {
        job["state"] = Value::String("queued".to_owned());
        job["lease"] = Value::Null;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&job).unwrap()).unwrap();
}

/// Which change of `changes` the donor job `job` validates (one change per batch).
fn change_of(fx: &Fixture, job: &Value, changes: &[(&str, i64)]) -> Option<i64> {
    let commit = job["commit"].as_str().unwrap();
    let added = git(
        &fx.queue(),
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            &format!("{commit}^"),
            commit,
        ],
    );
    changes
        .iter()
        .find(|(file, _)| added.lines().any(|line| line.trim() == *file))
        .map(|(_, pr)| *pr)
}

#[test]
fn a_stale_cancelled_root_is_recomposed_on_the_new_base_not_retried() {
    // Three disjoint roots on one base. a lands and moves the base while
    // b still runs and c's job sits queued, held out of donors' reach.
    // When c's job is given back, the claim-time stale sweep cancels it:
    // the run must recompose c on the new base (one job for the stale
    // candidate, no identical retries), not republish it as a refusal.
    let fx = Fixture::new("stale-cancel-recompose");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let runner = r#"if [ -f tests/b ]; then sleep 8; fi; for t in $(ls tests); do printf '{"id":"%s","outcome":"pass"}\n' "$t" >> "$MQ_RESULTS"; done; exit 0"#;
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        runner,
        "--offload",
        "decide",
        "--refusal-max-retries",
        "3",
        "--refusal-backoff-secs",
        "0",
    ]);
    set_config(&fx, |config| {
        config["depth"] = serde_json::json!(3);
        config["roots"] = serde_json::json!(3);
        config["keep_stale_roots"] = serde_json::json!(true);
        config["max_batch"] = serde_json::json!(1);
    });
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let run = spawn_run(&fx, &[]);
    wait_for("three published root jobs", Duration::from_secs(60), || {
        jobs(&fx).len() >= 3
    });
    let changes = [("tests/a", 1), ("tests/b", 2), ("tests/c", 3)];
    let first = jobs(&fx);
    let job_of = |pr: i64| {
        first
            .iter()
            .find(|job| change_of(&fx, job, &changes) == Some(pr))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (job_a, job_b, job_c) = (job_of(1), job_of(2), job_of(3));
    let stale_commit = first
        .iter()
        .find(|job| job["id"] == job_c.as_str())
        .unwrap()["commit"]
        .as_str()
        .unwrap()
        .to_owned();
    // Hold c's job out of reach while the donors take a and b, then give
    // it back queued, as a job no donor has started.
    set_lease(&fx, &job_c, true);
    let clones = [
        donor_clone(&fx, "donor-host-1"),
        donor_clone(&fx, "donor-host-2"),
    ];
    let done = std::sync::atomic::AtomicBool::new(false);
    let (code, report, stderr) = std::thread::scope(|scope| {
        for (n, clone) in clones.iter().enumerate() {
            let (fx, done) = (&fx, &done);
            let name = format!("d{}", n + 1);
            let clone_text = clone.display().to_string();
            scope.spawn(move || {
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    let route = route(fx);
                    let _ = command(
                        &fx.root,
                        &[
                            "donor",
                            "run",
                            "--queue",
                            &route,
                            "--clone",
                            &clone_text,
                            "--name",
                            &name,
                            "--poll-secs",
                            "1",
                            "--until-idle",
                        ],
                        &[],
                    )
                    .output();
                    std::thread::sleep(Duration::from_millis(200));
                }
            });
        }
        wait_for(
            "both donors running a and b",
            Duration::from_secs(60),
            || {
                let now = jobs(&fx);
                [&job_a, &job_b].iter().all(|id| {
                    now.iter()
                        .any(|job| job["id"] == id.as_str() && job["state"] != "queued")
                })
            },
        );
        // a is landing (or landed): give c's job back, so the next claim
        // sweeps it as stale on the moved base.
        set_lease(&fx, &job_c, false);
        let outcome = finish(run);
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        outcome
    });
    assert_eq!(code, Some(0), "{stderr}");
    for pr in [1, 2, 3] {
        assert_eq!(item_status(&fx, pr), "landed", "change {pr}: {report}");
    }
    let after = jobs(&fx);
    // Exactly one donor job was ever published for the stale candidate:
    // no identical-candidate retries.
    let stale_jobs: Vec<&Value> = after
        .iter()
        .filter(|job| job["commit"] == stale_commit.as_str())
        .collect();
    assert_eq!(
        stale_jobs.len(),
        1,
        "the stale candidate was republished: {after:?}"
    );
    assert_eq!(stale_jobs[0]["state"], "cancelled", "{}", stale_jobs[0]);
    // Its replacement was built on the new base and done.
    let rebuilt: Vec<&Value> = after
        .iter()
        .filter(|job| job["id"] != job_c.as_str() && change_of(&fx, job, &changes) == Some(3))
        .collect();
    assert!(
        rebuilt.iter().any(|job| job["state"] == "done"),
        "change 3 was not validated again on the new base: {after:?}"
    );
    // No validation of the stale commit was recorded as refused.
    assert!(
        !report["validations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["commit"] == stale_commit && v["refused"] == true),
        "{report}"
    );
    let _ = donor_run;
}

#[test]
fn a_stale_cancelled_probe_abandons_the_search_and_the_batch_is_recomposed() {
    // A red batch's culprit search runs its probe as a donor job. When
    // that job is cancelled as stale (here written directly, standing in
    // for the sweep), the probe has observed nothing: the search must be
    // abandoned and the batch recomposed, never counted as an
    // inconclusive search that stops the run.
    let fx = Fixture::new("stale-probe");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    // The first probe run sleeps, so the test can cancel its job
    // mid-flight; later probes answer at once.
    let probes = fx.root.join("probe-runs");
    let runner = format!(
        r#"if [ "$MQ_SELECTION" = tests ]; then
  n=$(cat '{probes}' 2>/dev/null || echo 0); n=$((n+1)); echo $n > '{probes}'
  if [ "$n" -lt 2 ]; then sleep 30; fi
  set -- $MQ_TESTS
else
  set -- $(ls tests)
fi
rc=0
for t in "$@"; do
  if [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then o=fail; rc=1; else o=pass; fi
  printf '{{"id":"%s","outcome":"%s"}}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#,
        probes = probes.display()
    );
    let script = fx.root.join("validator.sh");
    std::fs::write(&script, runner).unwrap();
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &format!("sh '{}'", script.display()),
        "--offload",
        "decide",
        "--confirm-reruns",
    ]);
    fx.branch("a", &[("tests/a", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx, &[]);
    let done = std::sync::atomic::AtomicBool::new(false);
    let (code, report, stderr) = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = donor_run(&fx, &clone, "d1", "1");
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        // The batch job validates red, then its probe job is published.
        wait_for("the probe job", Duration::from_secs(60), || {
            jobs(&fx).len() >= 2
        });
        let probe = jobs(&fx)
            .into_iter()
            .max_by_key(|job| job["id"].as_str().unwrap().to_owned())
            .unwrap();
        cancel_job_as_stale(&fx, probe["id"].as_str().unwrap());
        let outcome = finish(run);
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        outcome
    });
    assert_eq!(code, Some(0), "{stderr}");
    assert!(!stderr.contains("culprit search inconclusive"), "{stderr}");
    assert!(
        report["stopped"].is_null(),
        "the stale probe stopped the run: {report}"
    );
    // The abandoned search does not decide the batch: the red batch is
    // validated again as a recomposed batch, searched again, and only
    // that second, complete search ejects it.
    let batch_validations = report["validations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["purpose"] == "batch" && v["prs"] == serde_json::json!([1]))
        .count();
    assert_eq!(
        batch_validations, 2,
        "the red batch was not recomposed and validated again: {report}"
    );
    let probe_validations = report["validations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["purpose"] == "probe")
        .count();
    assert!(
        probe_validations >= 2,
        "the recomposed batch was not searched again: {report}"
    );
    assert_eq!(item_status(&fx, 1), "ejected", "{report}");
    let runs = fx.queue().join(".git/merge-queue/runs");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("events.jsonl").is_file())
        .collect();
    dirs.sort();
    let events = std::fs::read_to_string(dirs.pop().unwrap().join("events.jsonl")).unwrap();
    assert!(events.contains("stale-probe"), "{events}");
    assert!(
        events.contains("recomposed on the current base"),
        "{events}"
    );
}

fn cancel_job_as_stale(fx: &Fixture, id: &str) {
    let path = job_path(fx, id);
    let mut job: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    job["state"] = Value::String("cancelled".to_owned());
    job["lease"] = Value::Null;
    job["history"] = serde_json::json!([
        "cancelled: stale: the base moved to deadbeef, which its candidate does not contain"
    ]);
    std::fs::write(&path, serde_json::to_string_pretty(&job).unwrap()).unwrap();
}

#[test]
fn a_stale_outside_cancellation_is_recomposed_not_refused() {
    // A donor job cancelled as stale (the sweep's reason, written here
    // directly as in an_outside_cancellation_refuses_the_batch) is not a
    // refusal: with refusal retries armed, the identical candidate must
    // not be republished; the batch is recomposed and a donor validates
    // the recomposed candidate to a landing.
    let fx = Fixture::new("stale-outside-cancel");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
        "--offload",
        "decide",
        "--refusal-max-retries",
        "3",
        "--refusal-backoff-secs",
        "0",
    ]);
    fx.branch("a", &[("a.txt", "a\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let first_id = jobs(&fx)[0]["id"].as_str().unwrap().to_owned();
    cancel_job_as_stale(&fx, &first_id);
    // The recomposed candidate is published as a new job; a donor takes
    // it and the change lands in the same run.
    wait_for("the recomposed job", Duration::from_secs(60), || {
        jobs(&fx).len() >= 2
    });
    let out = donor_run(&fx, &clone, "d1", "1");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (code, report, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(item_status(&fx, 1), "landed", "{report}");
    let validations = report["validations"].as_array().unwrap();
    assert!(
        validations
            .iter()
            .any(|v| v["cancelled"] == true && v["refused"] != true),
        "the stale cancellation was not recorded as cancelled: {report}"
    );
    assert!(
        !validations.iter().any(|v| v["refused"] == true),
        "a stale cancellation was retried as a refusal: {report}"
    );
    assert!(
        report["deferred"].as_array().is_none_or(Vec::is_empty),
        "{report}"
    );
}
