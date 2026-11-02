//! Pipelined planning end to end: the first batch validates before the
//! completed plan is consumed, and the run still lands the whole queue.

mod support;

use serde_json::Value;
use support::{git, Fixture};

/// Same tiny runner as the `run` suite: one file per test under `tests/`.
const RUNNER: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests); else set -- $MQ_TESTS; fi
rc=0
for t in "$@"; do
  o=pass
  if [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then o=fail; fi
  [ $o = fail ] && rc=1
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
sleep 0.5
exit $rc"#;

/// Path-routing policy: the tests whose files changed.
const SELECT: &str = "git diff --name-only \"$MQ_BASE\" HEAD -- tests | sed 's|^tests/||'";

fn setup(tag: &str) -> Fixture {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        RUNNER,
        "--select",
        SELECT,
        "--policy",
        "path-routing",
    ]);
    fx
}

#[test]
fn first_batch_validates_before_completed_plan_is_consumed() {
    let fx = setup("pipeline-first");
    for pr in 1..=12_i64 {
        let name = format!("p{pr}");
        fx.branch(&name, &[(&format!("tests/{name}"), "ok\n")]);
        fx.mq_ok(&["submit", &pr.to_string(), "--head", &name]);
    }
    // Depth 1 keeps the window at the first batch until it lands, so the
    // completed plan can only be consumed afterwards; the event order
    // then shows the overlap without relying on wall-clock timing.
    let out = fx.mq_ok(&[
        "run",
        "--shadow",
        "--depth",
        "1",
        "--max-batch",
        "2",
        "--json",
    ]);
    let report: Value = serde_json::from_str(&out).unwrap();
    let landed: usize = report["landed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|batch| batch["prs"].as_array().unwrap().len())
        .sum();
    assert_eq!(landed, 12, "{report}");
    assert_eq!(report["validations"][0]["prs"], serde_json::json!([1, 2]));

    let run_id = report["run"].as_str().unwrap();
    let events_path = fx
        .queue()
        .join(".git/merge-queue/runs")
        .join(run_id)
        .join("events.jsonl");
    let text = std::fs::read_to_string(&events_path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", events_path.display()));
    let kinds: Vec<String> = text
        .lines()
        .map(|line| {
            let value: Value = serde_json::from_str(line).unwrap();
            value["kind"].as_str().unwrap().to_owned()
        })
        .collect();
    let position = |kind: &str| {
        kinds
            .iter()
            .position(|seen| seen == kind)
            .unwrap_or_else(|| panic!("no {kind} event in {kinds:?}"))
    };
    // The first batch was emitted, then validated, and only then was the
    // completed plan consumed for the remaining batches.
    assert!(position("plan-first") < position("validate"), "{kinds:?}");
    assert!(
        position("validate") < position("plan-complete"),
        "{kinds:?}"
    );
    // The completed plan in the report covers the whole queue.
    let planned: usize = report["plan"]["batches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|batch| batch["prs"].as_array().unwrap().len())
        .sum();
    assert_eq!(planned, 12, "{report}");
}
