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
