//! Speculative stacking as the default (#1475): a would-be root validates
//! stacked on the candidates ahead of it, landing is a checked
//! fast-forward, and a stacked candidate whose base root fails or turns
//! stale re-validates on the corrected base, never landing on the old
//! validation. `--no-speculative-stacking` restores disjoint roots.

mod support;

use std::path::PathBuf;

use serde_json::Value;
use support::{git, Fixture};

const PASS_UNLESS_B: &str = r#"for t in $(ls tests); do o=pass; if [ "$t" = b ]; then o=fail; fi; printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"; done; exit 0"#;
const ALL_PASS: &str = r#"for t in $(ls tests); do printf '{"id":"%s","outcome":"pass"}\n' "$t" >> "$MQ_RESULTS"; done; exit 0"#;

fn latest_report(fx: &Fixture) -> Value {
    let runs = fx.queue().join(".git/merge-queue/runs");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("report.json").is_file())
        .collect();
    dirs.sort();
    serde_json::from_str(&std::fs::read_to_string(dirs.pop().unwrap().join("report.json")).unwrap())
        .unwrap()
}

fn batch_validations(report: &Value, pr: i64) -> Vec<&Value> {
    report["validations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["purpose"] == "batch" && v["prs"] == serde_json::json!([pr]))
        .collect()
}

fn landed_prs(report: &Value) -> Vec<i64> {
    report["landed"]
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
        .collect()
}

fn ejected_prs(report: &Value) -> Vec<i64> {
    let mut out: Vec<i64> = report["ejected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["pr"].as_i64().unwrap())
        .collect();
    out.sort_unstable();
    out
}

fn seed_three(fx: &Fixture, validator: &str, extra_init: &[&str]) {
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let mut init: Vec<&str> = vec![
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        validator,
    ];
    init.extend_from_slice(extra_init);
    fx.mq_ok(&init);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
}

#[test]
fn stack_of_three_all_green_lands_in_order_by_checked_fast_forward() {
    let fx = Fixture::new("spec-stack-green");
    seed_three(&fx, ALL_PASS, &[]);
    let out = fx.mq(&["run", "--depth", "3", "--max-batch", "1"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report = latest_report(&fx);
    assert_eq!(landed_prs(&report), vec![1, 2, 3], "{report}");
    // Each change validated once, stacked: only the bottom stands on the base.
    for pr in [1, 2, 3] {
        assert_eq!(batch_validations(&report, pr).len(), 1, "{report}");
    }
    assert_eq!(
        batch_validations(&report, 1)[0]["parent"],
        report["base_start"],
        "{report}"
    );
    for pr in [2, 3] {
        assert_ne!(
            batch_validations(&report, pr)[0]["parent"],
            report["base_start"],
            "change {pr} must validate stacked, not as a root: {report}"
        );
    }
    let tree = git(&fx.remote(), &["ls-tree", "-r", "--name-only", "main"]);
    for file in ["tests/a", "tests/b", "tests/c"] {
        assert!(tree.lines().any(|line| line == file), "{tree}");
    }
}

#[test]
fn middle_failure_revalidates_above_and_never_lands_on_the_failed_assumption() {
    let fx = Fixture::new("spec-stack-middle-red");
    seed_three(&fx, PASS_UNLESS_B, &[]);
    let out = fx.mq(&["run", "--depth", "3", "--max-batch", "1"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report = latest_report(&fx);
    assert_eq!(ejected_prs(&report), vec![2], "{report}");
    assert_eq!(landed_prs(&report), vec![1, 3], "{report}");
    // Change 3 first validated on a tree assuming change 2, then again on
    // the corrected base without it: at least two batch validations.
    assert!(
        batch_validations(&report, 3).len() >= 2,
        "change 3 must re-validate after the middle root failed: {report}"
    );
    let tree = git(&fx.remote(), &["ls-tree", "-r", "--name-only", "main"]);
    assert!(tree.lines().any(|line| line == "tests/a"), "{tree}");
    assert!(tree.lines().any(|line| line == "tests/c"), "{tree}");
    assert!(!tree.lines().any(|line| line == "tests/b"), "{tree}");
}

#[test]
fn externally_moved_main_refuses_the_fast_forward_and_replans() {
    let fx = Fixture::new("spec-stack-external-move");
    // The validator moves main externally during the first validation.
    let dev = fx.dev().display().to_string();
    let marker = fx.root.join("moved-once");
    let validator = format!(
        "if [ ! -f {marker} ]; then touch {marker}; echo external > {dev}/external.txt; git -C {dev} add external.txt; git -C {dev} commit -q -m external; git -C {dev} push -q origin HEAD:main; fi; {ALL_PASS}",
        marker = marker.display(),
        dev = dev,
    );
    seed_three(&fx, &validator, &[]);
    let out = fx.mq(&["run", "--depth", "3", "--max-batch", "1"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report = latest_report(&fx);
    assert_eq!(landed_prs(&report), vec![1, 2, 3], "{report}");
    // The bottom re-validated on the moved base instead of landing stale.
    assert!(
        batch_validations(&report, 1).len() >= 2,
        "change 1 must re-validate after main moved externally: {report}"
    );
    let tree = git(&fx.remote(), &["ls-tree", "-r", "--name-only", "main"]);
    for file in ["external.txt", "tests/a", "tests/b", "tests/c"] {
        assert!(tree.lines().any(|line| line == file), "{tree}");
    }
}

#[test]
fn opt_out_restores_the_disjoint_root() {
    let fx = Fixture::new("spec-stack-opt-out");
    seed_three(&fx, ALL_PASS, &["--no-speculative-stacking"]);
    let out = fx.mq(&["run", "--depth", "3", "--max-batch", "1"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report = latest_report(&fx);
    assert_eq!(landed_prs(&report), vec![1, 2, 3], "{report}");
    // With the opt-out the last change rides a root on the base itself.
    assert_eq!(
        batch_validations(&report, 3)[0]["parent"],
        report["base_start"],
        "{report}"
    );
}
