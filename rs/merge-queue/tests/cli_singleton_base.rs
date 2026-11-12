//! Singleton ejection needs base evidence (#1187 mq-singleton-eject): a
//! `trust_base` pass is assumed, not measured, so on a base the queue never
//! landed (a direct push) the base is probed before a singleton is blamed,
//! while a base the queue itself landed stays trusted.

mod support;

use serde_json::Value;
use support::{git, Fixture};

/// Each file under `tests/` is one test, failing when it contains FAIL.
const RUNNER: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests); else set -- $MQ_TESTS; fi
rc=0
for t in "$@"; do
  o=pass
  if [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then o=fail; rc=1; fi
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#;

/// Path-routing policy: the tests whose files changed.
const SELECT: &str = "git diff --name-only \"$MQ_BASE\" HEAD -- tests | sed 's|^tests/||'";

fn setup(tag: &str) -> Fixture {
    setup_with(tag, &[])
}

fn setup_with(tag: &str, extra: &[&str]) -> Fixture {
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
        "--max-batch",
        "1",
        "--trust-base",
    ];
    args.extend_from_slice(extra);
    fx.mq_ok(&args);
    fx
}

/// Like [`RUNNER`], but a targeted run whose selected tests all passed
/// still exits 4 (its receipt did not qualify), and a failing `check.*`
/// test is named deterministic on a `RERUN-EXEMPT:` line.
const RUNNER_UNQUALIFIED: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests); else set -- $MQ_TESTS; fi
rc=0
exempt=
for t in "$@"; do
  o=pass
  if [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then
    o=fail
    rc=1
    case "$t" in check.*) exempt="$exempt,$t" ;; esac
  fi
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
[ -n "$exempt" ] && echo "RERUN-EXEMPT: ${exempt#,}"
if [ $rc -eq 0 ] && [ "$MQ_SELECTION" != all ]; then exit 4; fi
exit $rc"#;

/// Like [`RUNNER`], but a selected test with no file at this commit is
/// reported skipped (so a base probe of it observes nothing conclusive).
const RUNNER_MISSING: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests); else set -- $MQ_TESTS; fi
rc=0
for t in "$@"; do
  if [ ! -f "tests/$t" ]; then
    printf '{"id":"%s","outcome":"skip"}\n' "$t" >> "$MQ_RESULTS"
    continue
  fi
  o=pass
  if grep -q FAIL "tests/$t"; then o=fail; rc=1; fi
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#;

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
        .unwrap_or_else(|| panic!("no item for #{pr}"))
}

#[test]
fn a_red_direct_push_base_blames_no_singleton() {
    // The base itself fails `tbad` and reached main by direct push: the
    // queue never validated it. Two disjoint singletons on it fail the same
    // test; neither is ejected, and the base is probed.
    let fx = setup("singleton-base-red");
    fx.write("tests/tbad", "FAIL\n");
    fx.commit_all("direct push: break tbad on the base");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let base = fx.remote_main();
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let out = fx.mq(&["run", "--json"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert!(
        r["ejected"].as_array().unwrap().is_empty(),
        "no singleton is blamed for the base red: {r}"
    );
    assert_eq!(status(&fx, 1), "queued");
    assert_eq!(status(&fx, 2), "queued");
    let probed_base = r["validations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["purpose"] == "probe" && v["commit"] == base);
    let stopped_on_base = r["stopped"].as_str().unwrap_or_default().contains("base");
    assert!(
        probed_base || stopped_on_base,
        "the base was probed or the run stopped on it: {r}"
    );
}

#[test]
fn deterministic_singleton_ejects_on_unqualified_base_probe() {
    // #1379: the singleton's only test is a deterministic check that
    // exists only in its own diff. The base probe passes it (absent at
    // the base, so reported passing) but exits 4; the singleton is
    // ejected, with the unqualified exit named, instead of the search
    // going inconclusive.
    let fx = Fixture::new("singleton-unqualified-probe");
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
        RUNNER_UNQUALIFIED,
        "--max-batch",
        "1",
        "--trust-base",
        "--select",
        SELECT,
        "--policy",
        "path-routing",
    ]);
    fx.branch("bad", &[("tests/check.bad", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "bad"]);
    let out = fx.mq(&["run", "--json"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    let ejected = r["ejected"].as_array().unwrap();
    assert_eq!(ejected.len(), 1, "{r}");
    assert_eq!(ejected[0]["pr"], 1, "{r}");
    assert!(
        ejected[0]["reason"]
            .as_str()
            .unwrap()
            .contains("passed every selected test but exited 4"),
        "the ejection names the unqualified probe exit: {r}"
    );
    assert_eq!(status(&fx, 1), "ejected");
}

#[test]
fn inconclusive_batch_holds_and_disjoint_root_lands() {
    // #1379: change 1's culprit search is inconclusive (its base probe
    // observes nothing). Only change 1 is held; the disjoint green
    // change 2 lands in the same run.
    let fx = Fixture::new("singleton-inconclusive-hold");
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
        RUNNER_MISSING,
    ]);
    fx.branch("a", &[("tests/a", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let out = fx.mq(&["run", "--json", "--depth", "3", "--max-batch", "1"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert!(
        r["ejected"].as_array().unwrap().is_empty(),
        "an inconclusive search ejects nobody: {r}"
    );
    let landed: Vec<i64> = r["landed"]
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
    assert!(landed.contains(&3), "the disjoint green lands: {r}");
    assert_eq!(status(&fx, 1), "queued");
    assert_eq!(status(&fx, 3), "landed");
}

#[test]
fn a_queue_landed_base_stays_trusted() {
    // One run lands the good change first: the bad singleton behind it
    // stands on a validated queue landing, so it is ejected on the trusted
    // base with no base probe, and the reason says the pass was assumed.
    // Path routing keeps the landing focused, so no recorded green covers
    // the bad test at the base and the pass really is the synthetic one.
    let fx = setup_with(
        "singleton-base-landed",
        &["--select", SELECT, "--policy", "path-routing"],
    );
    fx.branch("good", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "good"]);
    fx.branch("bad", &[("tests/b", "FAIL\n")]);
    fx.mq_ok(&["submit", "2", "--head", "bad"]);
    let out = fx.mq(&["run", "--json"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(r["landed"].as_array().unwrap().len(), 1, "{r}");
    let ejected = r["ejected"].as_array().unwrap();
    assert_eq!(ejected.len(), 1, "{r}");
    assert_eq!(ejected[0]["pr"], 2, "{r}");
    assert!(
        ejected[0]["reason"]
            .as_str()
            .unwrap()
            .contains("pass without it (assumed: trust_base)"),
        "the trusted pass is named as assumed: {r}"
    );
    assert!(
        r["validations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["purpose"] != "probe"),
        "no base probe runs on a queue landing: {r}"
    );
}
