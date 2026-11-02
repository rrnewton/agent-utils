//! Regression tests for adversarial-review findings: each scenario once produced a wrong outcome.

mod support;

use serde_json::Value;
use support::{git, Fixture};

const RUNNER: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests); else set -- $MQ_TESTS; fi
rc=0
for t in "$@"; do
  o=pass
  if [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then o=fail; fi
  if [ "$t" = c ] && [ -f tests/b ] && [ ! -f tests/c ] && [ -n "$FLAKY_MARKER" ] && [ ! -e "$FLAKY_MARKER" ]; then o=fail; touch "$FLAKY_MARKER"; fi
  if [ "$t" = b ] && [ -f tests/b ] && grep -q NEEDFIX tests/b && [ ! -f fixed ]; then o=fail; fi
  [ $o = fail ] && rc=1
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
case "$MQ_RESULTS" in *remainder*) if [ -n "$FIXSHA" ] && [ ! -e "$FIXMARK" ]; then touch "$FIXMARK"; git -C "$REMOTE" update-ref refs/heads/main "$FIXSHA"; fi;; esac
exit $rc"#;

const SELECT: &str = "git diff --name-only \"$MQ_BASE\" HEAD -- tests | sed 's|^tests/||'";

fn setup(tag: &str, extra: &[&str]) -> Fixture {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let mut args = vec![
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
    ];
    args.extend_from_slice(extra);
    fx.mq_ok(&args);
    fx
}

fn report(stdout: &str) -> Value {
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("bad report JSON ({e}):\n{stdout}"))
}

fn status(fx: &Fixture, pr: i64) -> String {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["pr"] == pr)
        .map(|i| i["status"].as_str().unwrap().to_owned())
        .unwrap()
}

/// X: two commits (edit shared.txt line 3, then revert) plus tests/x. Net diff never touches
/// shared.txt, so the plan's pairwise probe sees no conflict with B, but replaying X's first
/// commit onto B conflicts at build time.
fn conflict_fixture(tag: &str) -> Fixture {
    let fx = setup(tag, &[]);
    fx.branch("b", &[("shared.txt", "1\n2\nB\n4\n5\n")]);
    git(&fx.dev(), &["checkout", "-q", "-B", "x", "main"]);
    fx.write("shared.txt", "1\n2\nX\n4\n5\n");
    fx.commit_all("x1");
    fx.write("shared.txt", "1\n2\n3\n4\n5\n");
    fx.write("tests/x", "ok\n");
    fx.commit_all("x2");
    git(&fx.dev(), &["push", "-q", "-f", "origin", "HEAD:x"]);
    git(&fx.dev(), &["checkout", "-q", "main"]);
    fx.branch("y", &[("tests/y", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "x"]);
    fx.mq_ok(&["submit", "3", "--head", "y", "--after", "2"]);
    fx
}

#[test]
fn dependent_of_conflict_ejected_change_is_requeued_not_landed() {
    let fx = conflict_fixture("rv-dep");
    let out = fx.mq(&["run", "--json"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(status(&fx, 2), "ejected");
    // #3 must land after #2, which was ejected: it goes back to the queue instead.
    assert_eq!(status(&fx, 3), "queued");
    assert_eq!(status(&fx, 1), "landed");
    assert_eq!(r["requeued"], serde_json::json!([3]));
}

#[test]
fn dry_run_changes_no_item() {
    let fx = conflict_fixture("rv-dry");
    fx.mq_ok(&["run", "--dry-run", "--json"]);
    assert_eq!(status(&fx, 2), "queued");
    assert_eq!(status(&fx, 3), "queued");
}

#[test]
fn flake_during_the_search_does_not_eject_an_innocent_change() {
    let fx = setup("rv-flake", &["--max-batch", "4", "--depth", "2"]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.branch("c", &[("tests/c", "FAIL\n")]);
    fx.branch("d", &[("tests/d", "ok\n")]);
    for (pr, name) in [(1, "a"), (2, "b"), (3, "c"), (4, "d")] {
        fx.mq_ok(&["submit", &pr.to_string(), "--head", name]);
    }
    let marker = fx.root.join("flaky-marker");
    let out = fx.mq_env(
        &["run", "--json"],
        &[("FLAKY_MARKER", marker.to_str().unwrap())],
    );
    let r = report(&String::from_utf8_lossy(&out.stdout));
    let ejected: Vec<i64> = r["ejected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["pr"].as_i64().unwrap())
        .collect();
    assert_eq!(ejected, vec![3]);
    for pr in [1, 2, 4] {
        assert_eq!(status(&fx, pr), "landed");
    }
}

#[test]
fn base_moved_after_ejection_keeps_the_culprit_out() {
    let fx = setup("rv-moved", &["--max-batch", "4"]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "NEEDFIX\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    // A commit on top of main that the runner pushes during the remainder validation.
    let fix = fx.branch("fix", &[("fixed", "1\n")]);
    for (pr, name) in [(1, "a"), (2, "b"), (3, "c")] {
        fx.mq_ok(&["submit", &pr.to_string(), "--head", name]);
    }
    let mark = fx.root.join("fix-marker");
    let remote = fx.remote();
    let out = fx.mq_env(
        &["run", "--json"],
        &[
            ("FIXSHA", &fix),
            ("FIXMARK", mark.to_str().unwrap()),
            ("REMOTE", remote.to_str().unwrap()),
        ],
    );
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(r["ejected"].as_array().unwrap().len(), 1);
    assert_eq!(r["ejected"][0]["pr"], 2);
    assert_eq!(status(&fx, 2), "ejected");
    assert_eq!(status(&fx, 1), "landed");
    assert_eq!(status(&fx, 3), "landed");
}

#[test]
fn shadow_widen_leaves_real_obligations_alone() {
    let fx = setup("rv-shadow-widen", &[]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["run", "--json"]);
    let open: Value = serde_json::from_str(&fx.mq_ok(&["obligations", "--json"])).unwrap();
    assert_eq!(open.as_array().unwrap().len(), 1);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["run", "--shadow", "--widen", "--json"]);
    let open: Value = serde_json::from_str(&fx.mq_ok(&["obligations", "--json"])).unwrap();
    assert_eq!(open.as_array().unwrap().len(), 1, "{open}");
}

/// `build` fails when tests/build says FAIL; `x` is then reported skipped, else it fails when
/// tests/x says FAIL.
const SKIPPING_RUNNER: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- build x; else set -- $MQ_TESTS; fi
rc=0
broken=no; grep -q FAIL tests/build 2>/dev/null && broken=yes
for t in "$@"; do
  o=pass
  case $t in
    build) [ $broken = yes ] && o=fail ;;
    x) if [ $broken = yes ]; then o=skip; elif grep -q FAIL tests/x 2>/dev/null; then o=fail; fi ;;
  esac
  [ $o = fail ] && rc=1
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#;

#[test]
fn a_skipped_test_is_rerun_on_the_remainder_not_carried_forward() {
    let fx = Fixture::new("rv-skip");
    fx.write("tests/build", "ok\n");
    fx.write("tests/x", "ok\n");
    fx.commit_all("seed");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        SKIPPING_RUNNER,
        "--select",
        SELECT,
        "--policy",
        "path-routing",
        "--max-batch",
        "2",
    ]);
    fx.branch("c", &[("tests/build", "FAIL\n")]);
    fx.branch("d", &[("tests/x", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "c"]);
    fx.mq_ok(&["submit", "2", "--head", "d"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    // #1 breaks `build`, which hides #2 breaking `x` (skipped). The remainder must rerun `x`.
    let ejected: Vec<i64> = r["ejected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["pr"].as_i64().unwrap())
        .collect();
    assert_eq!(ejected, vec![1, 2]);
    assert!(r["landed"].as_array().unwrap().is_empty());
}

#[test]
fn a_red_complete_run_keeps_obligations_open() {
    let fx = setup("rv-red-widen", &[]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["run", "--json"]);
    // Break a test the focused landing never ran.
    git(&fx.dev(), &["pull", "-q", "--rebase", "origin", "main"]);
    fx.write("tests/t0", "FAIL\n");
    fx.commit_all("break t0 behind the queue's back");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let out = fx.mq(&["widen", "--json"]);
    assert_eq!(out.status.code(), Some(3));
    let open: Value = serde_json::from_str(&fx.mq_ok(&["obligations", "--json"])).unwrap();
    let open = open.as_array().unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0]["failed_runs"][0]["green"], false);
}
