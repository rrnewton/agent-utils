//! A setup step that times out before any test runs is could-not-run,
//! not a red verdict (#1119): the batch is refused, never landed,
//! strikes no change, and repeated could-not-runs escalate at the
//! deferral bound with the setup reason named.

mod support;

use std::path::PathBuf;

use serde_json::Value;
use support::{git, Fixture};

/// A fake dagrun whose setup step always ends on a wall timeout and
/// whose selected step therefore never runs.
fn setup_timeout(tag: &str) -> Fixture {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let script = fx.root.join("fake-dagrun");
    std::fs::write(
        &script,
        "#!/bin/sh\n\
         echo '{\"event\":\"step_end\",\"step\":\"setup.fetch\",\"ok\":false,\"reason\":\"TIMEOUT >60s\"}' >> \"$DAGRUN_LOG_DIR/journal.jsonl\"\n\
         echo '{\"event\":\"step_skip\",\"step\":\"t0\",\"reason\":\"setup.fetch failed\"}' >> \"$DAGRUN_LOG_DIR/journal.jsonl\"\n\
         exit 1\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "dagrun",
        "--dag",
        "validation.dag.yaml",
        "--dagrun",
        &script.display().to_string(),
        "--refusal-max-retries",
        "0",
        "--refusal-backoff-secs",
        "0",
        "--max-deferrals",
        "3",
    ]);
    fx
}

fn report(stdout: &str) -> Value {
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("bad report JSON ({e}):\n{stdout}"))
}

fn items(fx: &Fixture) -> Vec<Value> {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items.as_array().unwrap().clone()
}

fn item(fx: &Fixture, pr: i64) -> Value {
    items(fx)
        .into_iter()
        .find(|i| i["pr"] == pr && i["status"] != "pending-intake")
        .unwrap_or(Value::Null)
}

fn state_dir(fx: &Fixture) -> PathBuf {
    fx.queue().join(".git").join("merge-queue")
}

/// The outcomes recorded for this queue, newest files read whole.
fn decision_outcomes(fx: &Fixture) -> Vec<String> {
    let dir = state_dir(fx).join("decisions");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(text) = std::fs::read_to_string(entry.path()) {
                for line in text.lines() {
                    if let Ok(row) = serde_json::from_str::<Value>(line) {
                        if let Some(outcome) = row["outcome"].as_str() {
                            out.push(outcome.to_owned());
                        }
                    }
                }
            }
        }
    }
    out
}

/// One setup timeout: refused, queued, not red anywhere.
#[test]
fn a_setup_timeout_with_no_tests_counted_is_could_not_run_not_red() {
    let fx = setup_timeout("stackgate-setup-timeout");
    let before = fx.remote_main();
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let out = fx.mq(&["run", "--json"]);
    // A run that stops on a refusal exits 3, like any stopped run.
    assert_eq!(
        out.status.code(),
        Some(3),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let r = report(&String::from_utf8_lossy(&out.stdout));
    let validations = r["validations"].as_array().unwrap();
    assert_eq!(validations.len(), 1, "{r}");
    assert_eq!(validations[0]["refused"], true, "{r}");
    assert!(r["landed"].as_array().unwrap().is_empty(), "{r}");
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    let deferred = r["deferred"].as_array().unwrap();
    assert_eq!(deferred.len(), 1, "{r}");
    assert!(
        deferred[0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("setup.fetch") && reason.contains("TIMEOUT")),
        "{r}"
    );
    assert_eq!(item(&fx, 1)["status"], "queued");
    assert_eq!(fx.remote_main(), before, "a could-not-run landed");
    assert_eq!(decision_outcomes(&fx), vec!["refused".to_owned()]);
}

/// The same revision timing out in setup on every run escalates at
/// the bound (K = 3) with the repeated could-not-run reason on the
/// item and in plan/status, instead of retrying silently forever.
/// It is never counted as a pass and never lands.
#[test]
fn repeated_setup_timeouts_escalate_with_the_reason_and_never_land() {
    let fx = setup_timeout("stackgate-setup-timeout-k");
    let before = fx.remote_main();
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    for run_no in 1..=3 {
        let out = fx.mq(&["run", "--json"]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        // Each run stops on the refusal (exit 3), never a pass.
        assert_eq!(out.status.code(), Some(3), "run {run_no}: {stderr}");
        if run_no < 3 {
            assert!(!stderr.contains("ALARM"), "run {run_no}: {stderr}");
        }
    }
    let out = fx.mq(&["run", "--json"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    // Held at the bound: the fourth run validates nothing.
    assert!(r["validations"].as_array().unwrap().is_empty(), "{r}");
    let it = item(&fx, 1);
    assert_eq!(it["status"], "queued", "{it}");
    let escalated = it["escalated"].as_str().unwrap_or_default().to_owned();
    assert!(
        escalated.contains("did not run 3 time(s)") && escalated.contains("TIMEOUT"),
        "escalated reason does not name the repeated could-not-run: {it}"
    );
    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    assert_eq!(plan["held"][0]["kind"], "escalated", "{plan}");
    assert!(
        plan["held"][0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("TIMEOUT")),
        "{plan}"
    );
    assert_eq!(fx.remote_main(), before, "a could-not-run landed");
    assert_eq!(
        decision_outcomes(&fx),
        vec!["refused".to_owned(); 3],
        "setup timeouts must never record green or red"
    );
}
