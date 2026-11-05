//! Early red: a candidate whose runner streams a deterministic failure is decided at once.
//! The run stops the candidate's validation, releases its slot, cancels the batch built on
//! it, rebuilds that batch on the base and decides it too -- all without waiting for either
//! runner to finish the sleep it started after reporting the failure.

mod support;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::Value;
use support::{git, Fixture};

#[test]
fn a_streamed_deterministic_red_releases_the_window_at_once() {
    let fx = Fixture::new("early-red");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    // Every candidate streams a deterministic failure for its own test file, then sleeps:
    // only the early-red watch can decide it before the sleep ends.
    let validator = r#"failed=0; for t in $(ls tests); do if [ "$t" != t0 ]; then echo "RERUN-EXEMPT: $t"; printf '{"id":"%s","outcome":"fail"}\n' "$t" >> "$MQ_RESULTS"; failed=1; fi; done; if [ $failed -eq 1 ]; then sleep 30; fi; exit $failed"#;
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        validator,
    ]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let started = Instant::now();
    let _ = fx.mq(&["run", "--depth", "2", "--max-batch", "1"]);
    assert!(
        started.elapsed() < Duration::from_secs(25),
        "the run waited for a sleeping runner: {:?}",
        started.elapsed()
    );
    let runs = fx.queue().join(".git/merge-queue/runs");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    let run = dirs.pop().expect("a run");
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(run.join("report.json")).unwrap()).unwrap();
    let validations = report["validations"].as_array().unwrap();
    let early_batches = validations
        .iter()
        .filter(|v| v["purpose"] == "batch" && v["early_red"] == true)
        .count();
    assert!(early_batches >= 2, "{report}");
    let mut ejected: Vec<i64> = report["ejected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["pr"].as_i64().unwrap())
        .collect();
    ejected.sort_unstable();
    assert_eq!(ejected, vec![1, 2], "{report}");
}
