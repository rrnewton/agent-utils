//! Tests beyond the complete suite: a select command that prints `all` and then test IDs asks
//! for the complete suite plus those tests. The runner gets `MQ_SELECTION=all` and the IDs in
//! `MQ_EXTRA_TESTS`; a failing one makes the batch red like any test, and one the runner never
//! reports is missing, never a pass.

mod support;

use serde_json::Value;
use support::{git, Fixture};

/// Records what it was asked to run, then runs it: a test `t` is the file `tests/t`, an extra
/// test `extra.x` the file `extras/x`, and either fails when its file says FAIL. With
/// `ignore-extras` in the tree it never reports an extra test.
const RUNNER: &str = r#"echo "selection=$MQ_SELECTION extra=${MQ_EXTRA_TESTS:-} tests=${MQ_TESTS:-}" >> "$CALLS"
if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests) ${MQ_EXTRA_TESTS:-}; else set -- $MQ_TESTS; fi
rc=0
for t in "$@"; do
  case $t in
    extra.*) [ -e ignore-extras ] && continue; f="extras/${t#extra.}" ;;
    *) f="tests/$t" ;;
  esac
  o=pass
  if [ -f "$f" ] && grep -q FAIL "$f"; then o=fail; rc=1; fi
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#;

/// The complete suite, plus `extra.<name>` for every file the change adds under `extras/`.
const SELECT: &str =
    r#"echo all; git diff --name-only "$MQ_BASE" HEAD -- extras | sed 's|^extras/|extra.|'"#;

fn setup(tag: &str) -> (Fixture, std::path::PathBuf) {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let calls = fx.root.join("calls");
    let runner = fx.root.join("runner.sh");
    std::fs::write(&runner, format!("CALLS='{}'\n{RUNNER}\n", calls.display())).unwrap();
    let run = format!("sh '{}'", runner.display());
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &run,
        "--select",
        SELECT,
    ]);
    (fx, calls)
}

fn calls(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn status(fx: &Fixture, pr: i64) -> String {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["pr"] == pr)
        .map(|item| item["status"].as_str().unwrap().to_owned())
        .unwrap_or_else(|| "missing".to_owned())
}

fn events(fx: &Fixture, report: &Value) -> Vec<(String, String)> {
    let run = report["run"].as_str().unwrap();
    let text = std::fs::read_to_string(
        fx.queue()
            .join(".git/merge-queue/runs")
            .join(run)
            .join("events.jsonl"),
    )
    .unwrap();
    text.lines()
        .map(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            (
                event["kind"].as_str().unwrap().to_owned(),
                event["detail"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

#[test]
fn a_select_command_can_ask_for_tests_beyond_the_complete_suite() {
    let (fx, calls_file) = setup("extra-tests-green");
    fx.branch("a", &[("extras/x", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--solo"]);
    fx.mq_ok(&["submit", "2", "--head", "b", "--solo"]);
    let report: Value = serde_json::from_str(&fx.mq_ok(&["run", "--json"])).unwrap();
    assert_eq!(status(&fx, 1), "landed", "{report}");
    assert_eq!(status(&fx, 2), "landed", "{report}");
    // The change that adds extras/x gets the suite plus extra.x; the other gets the suite.
    let seen = calls(&calls_file);
    assert!(
        seen.contains(&"selection=all extra=extra.x tests=".to_owned()),
        "{seen:?}"
    );
    assert!(
        seen.contains(&"selection=all extra= tests=".to_owned()),
        "{seen:?}"
    );
    let validated: Vec<String> = events(&fx, &report)
        .into_iter()
        .filter(|(kind, _)| kind == "validate")
        .map(|(_, detail)| detail)
        .collect();
    assert!(
        validated
            .iter()
            .any(|d| d.contains("#1") && d.ends_with(": all + extra.x (select-command)")),
        "{validated:?}"
    );
    // The validation record names the extra tests it ran.
    let runs = fx
        .queue()
        .join(".git/merge-queue/runs")
        .join(report["run"].as_str().unwrap());
    let records: Vec<Value> = std::fs::read_dir(runs.join("validations"))
        .unwrap()
        .flatten()
        .map(|entry| {
            serde_json::from_str(
                &std::fs::read_to_string(entry.path().join("record.json")).unwrap(),
            )
            .unwrap()
        })
        .collect();
    assert!(
        records
            .iter()
            .any(|r| r["extra"] == serde_json::json!(["extra.x"])),
        "{records:?}"
    );
    assert!(
        records.iter().any(|r| r.get("extra").is_none()),
        "{records:?}"
    );
}

#[test]
fn a_failing_extra_test_is_red_and_its_change_is_ejected() {
    let (fx, _) = setup("extra-tests-red");
    fx.branch("a", &[("extras/x", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let report: Value = serde_json::from_str(&fx.mq_ok(&["run", "--json"])).unwrap();
    assert_eq!(status(&fx, 1), "ejected", "{report}");
    assert_eq!(
        report["ejected"][0]["failed"],
        serde_json::json!(["extra.x"]),
        "{report}"
    );
}

#[test]
fn an_extra_test_the_runner_never_reports_is_missing_not_a_pass() {
    let (fx, _) = setup("extra-tests-missing");
    fx.branch("a", &[("extras/x", "ok\n"), ("ignore-extras", "\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let report: Value = serde_json::from_str(&fx.mq_ok(&["run", "--json"])).unwrap();
    assert_ne!(status(&fx, 1), "landed", "{report}");
    let validated: Vec<(String, String)> = events(&fx, &report)
        .into_iter()
        .filter(|(kind, _)| kind == "result")
        .collect();
    assert!(
        validated
            .iter()
            .any(|(_, d)| d.starts_with("v1 red") && d.contains("extra.x")),
        "{validated:?}"
    );
}
