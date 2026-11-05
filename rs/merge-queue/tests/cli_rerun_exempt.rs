//! A red run whose every failure the validator names a deterministic check (`RERUN-EXEMPT:`) is
//! confirmed on that one sighting: a rerun of the same tree cannot change its answer, so the
//! queue ejects without one. Any other failure still needs its second sighting. `validator.
//! fail_fast` hands candidate validations `MQ_FAIL_FAST=1`.

mod support;

use serde_json::Value;
use support::{git, Fixture};

/// Each file under `tests/` is one test, failing when it contains FAIL. A failing test named
/// `check.*` is a deterministic check: the runner names the failed checks on a `RERUN-EXEMPT:`
/// line, whatever else failed (the queue decides what that covers). With `$FF_LOG` set, each
/// run appends what `$MQ_FAIL_FAST` and `$MQ_SELECTION` were.
const RUNNER: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests); else set -- $MQ_TESTS; fi
[ -n "$FF_LOG" ] && echo "ff=${MQ_FAIL_FAST:-} sel=$MQ_SELECTION" >> "$FF_LOG"
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
exit $rc"#;

/// Path-routing policy: the tests whose files changed.
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
        "--trust-base",
    ];
    args.extend_from_slice(extra);
    fx.mq_ok(&args);
    fx
}

fn report(fx: &Fixture, env: &[(&str, &str)]) -> Value {
    let out = fx.mq_env(&["run", "--json"], env);
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap_or_else(|e| {
        panic!(
            "bad report JSON ({e}): {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn purposes(report: &Value) -> Vec<String> {
    report["validations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["purpose"].as_str().unwrap().to_owned())
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
        .unwrap_or_default()
}

#[test]
fn a_deterministic_check_red_once_is_ejected_without_a_rerun() {
    let fx = setup("exempt-single", &["--max-batch", "1"]);
    fx.branch("a", &[("tests/check.paths", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let r = report(&fx, &[]);
    // One validation: the batch. No same-commit confirmation was spent on it.
    assert_eq!(purposes(&r), vec!["batch"], "{r}");
    let ejected = &r["ejected"][0];
    assert_eq!(ejected["pr"], 1, "{r}");
    assert_eq!(ejected["kind"], "failed-tests");
    assert_eq!(ejected["failed"], serde_json::json!(["check.paths"]));
    let reason = ejected["reason"].as_str().unwrap();
    assert!(
        reason.contains(
            "(seen once: a deterministic check, which a rerun of the same tree cannot change)"
        ),
        "{reason}"
    );
    assert_eq!(status(&fx, 1), "ejected");
}

#[test]
fn a_failure_outside_the_named_checks_still_needs_its_second_sighting() {
    let fx = setup("exempt-mixed", &["--max-batch", "1"]);
    fx.branch(
        "a",
        &[("tests/check.paths", "FAIL\n"), ("tests/unit_a", "FAIL\n")],
    );
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let r = report(&fx, &[]);
    // `unit_a` is not a named check: the batch's red is confirmed by a rerun of the same commit.
    assert_eq!(purposes(&r), vec!["batch", "probe"], "{r}");
    let validations = r["validations"].as_array().unwrap();
    assert_eq!(validations[0]["commit"], validations[1]["commit"]);
    let ejected = &r["ejected"][0];
    assert_eq!(ejected["pr"], 1, "{r}");
    let reason = ejected["reason"].as_str().unwrap();
    assert!(reason.contains("(seen twice)"), "{reason}");
    assert_eq!(status(&fx, 1), "ejected");
}

#[test]
fn a_deterministic_culprit_in_a_batch_needs_no_confirming_probe() {
    let fx = setup("exempt-batch", &["--max-batch", "3"]);
    fx.branch("a", &[("tests/unit_a", "ok\n")]);
    fx.branch("b", &[("tests/check.paths", "FAIL\n")]);
    fx.branch("c", &[("tests/unit_c", "ok\n")]);
    for (pr, name) in [(1, "a"), (2, "b"), (3, "c")] {
        fx.mq_ok(&["submit", &pr.to_string(), "--head", name]);
    }
    let r = report(&fx, &[]);
    let landed: Vec<Value> = r["landed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["prs"].clone())
        .collect();
    assert_eq!(landed, vec![serde_json::json!([1, 3])], "{r}");
    let ejected = &r["ejected"][0];
    assert_eq!(ejected["pr"], 2, "{r}");
    assert!(
        ejected["reason"]
            .as_str()
            .unwrap()
            .contains("seen once: a deterministic check"),
        "{r}"
    );
    // One round of probes (two at once: the probe width is the window): #1 alone passes, and
    // #1+#2 fails the check once, which confirms it. Without the marker, a second round probes
    // #1+#2 again to see the failure twice. The full candidate is never probed again.
    assert_eq!(
        purposes(&r),
        vec!["batch", "probe", "probe", "remainder"],
        "{r}"
    );
    let validations = r["validations"].as_array().unwrap();
    let batch_commit = &validations[0]["commit"];
    assert!(
        validations
            .iter()
            .filter(|v| v["purpose"] == "probe")
            .all(|v| &v["commit"] != batch_commit),
        "{r}"
    );
    assert_eq!(status(&fx, 2), "ejected");
    assert_eq!(status(&fx, 3), "landed");
}

fn config_validator(fx: &Fixture) -> Value {
    let common = git(&fx.queue(), &["rev-parse", "--git-common-dir"]);
    let common = if std::path::Path::new(&common).is_absolute() {
        std::path::PathBuf::from(common)
    } else {
        fx.queue().join(common)
    };
    let text = std::fs::read_to_string(common.join("merge-queue").join("config.json")).unwrap();
    serde_json::from_str::<Value>(&text).unwrap()["validator"].clone()
}

#[test]
fn fail_fast_reaches_candidate_validations_only_when_configured() {
    // Off (the default): the key is not even written, so the stored config is unchanged.
    let off = setup("fail-fast-off", &["--max-batch", "1"]);
    assert!(config_validator(&off).get("fail_fast").is_none());
    off.branch("a", &[("tests/unit_a", "ok\n")]);
    off.mq_ok(&["submit", "1", "--head", "a"]);
    let log = off.root.join("ff.log");
    let r = report(&off, &[("FF_LOG", log.to_str().unwrap())]);
    assert_eq!(r["landed"][0]["prs"], serde_json::json!([1]), "{r}");
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "ff= sel=tests\n");

    // On: every candidate validation gets MQ_FAIL_FAST=1; the complete run on the base does not.
    let on = setup("fail-fast-on", &["--max-batch", "1", "--fail-fast"]);
    assert_eq!(config_validator(&on)["fail_fast"], true);
    on.branch("a", &[("tests/unit_a", "ok\n")]);
    on.mq_ok(&["submit", "1", "--head", "a"]);
    let log = on.root.join("ff.log");
    let r = report(&on, &[("FF_LOG", log.to_str().unwrap())]);
    assert_eq!(r["landed"][0]["prs"], serde_json::json!([1]), "{r}");
    let out = on.mq_env(&["widen", "--json"], &[("FF_LOG", log.to_str().unwrap())]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "ff=1 sel=tests\nff= sel=all\n"
    );
}
