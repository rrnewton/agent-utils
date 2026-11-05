//! The disjoint second root: with a window of at least two, one slot validates a batch
//! built on the base itself, disjoint from the stack. When the stack's head goes red,
//! the root is not cancelled with it: it lands on its one validation, while the stacked
//! batch behind the red head is rebuilt and validated again.

mod support;

use std::path::PathBuf;

use serde_json::Value;
use support::{git, Fixture};

#[test]
fn a_disjoint_root_survives_a_red_stack_head() {
    let fx = Fixture::new("disjoint-root");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    // Red exactly for candidates containing change 1's or change 2's test file: the
    // whole stacked chain is bad, and only the disjoint root can land.
    let validator = r#"for t in $(ls tests); do o=pass; if [ "$t" = a ] || [ "$t" = b ]; then o=fail; fi; printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"; done; exit 0"#;
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        validator,
    ]);
    fx.branch("a", &[("tests/a", "bad\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let out = fx.mq(&["run", "--depth", "3", "--max-batch", "1"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let runs = fx.queue().join(".git/merge-queue/runs");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    let report: Value = serde_json::from_str(
        &std::fs::read_to_string(dirs.pop().unwrap().join("report.json")).unwrap(),
    )
    .unwrap();
    let mut ejected: Vec<i64> = report["ejected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["pr"].as_i64().unwrap())
        .collect();
    ejected.sort_unstable();
    assert_eq!(ejected, vec![1, 2], "{report}");
    let mut landed: Vec<i64> = report["landed"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|b| {
            b["prs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pr| pr.as_i64().unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    landed.sort_unstable();
    assert_eq!(landed, vec![3], "{report}");
    // Change 3 rode the disjoint root: one batch validation, built on the base itself.
    let root_runs: Vec<&Value> = report["validations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["purpose"] == "batch" && v["prs"] == serde_json::json!([3]))
        .collect();
    assert_eq!(root_runs.len(), 1, "{report}");
    assert_eq!(root_runs[0]["parent"], report["base_start"], "{report}");
    // Change 2 was stacked on the red head: cancelled and validated again on the base.
    let stacked_runs: Vec<&Value> = report["validations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["purpose"] == "batch" && v["prs"] == serde_json::json!([2]))
        .collect();
    assert!(stacked_runs.len() >= 2, "{report}");
}
