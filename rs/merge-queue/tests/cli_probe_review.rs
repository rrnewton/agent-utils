//! Regression tests from an adversarial review of probe caching and staged planning: a cold
//! cache once changed outcomes (ejecting what the plan holds, landing a child of a held parent),
//! and the cache key once missed repository attributes.

mod support;

use serde_json::Value;
use support::{git, Fixture};

const RUNNER: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests); else set -- $MQ_TESTS; fi
rc=0
for t in "$@"; do
  o=pass
  if [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then o=fail; fi
  [ $o = fail ] && rc=1
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#;

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

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("bad JSON ({e}):\n{text}"))
}

fn ejected(report: &Value) -> Vec<(i64, String)> {
    report["ejected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["pr"].as_i64().unwrap(),
                e["kind"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn landed(report: &Value) -> Vec<i64> {
    report["landed"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|l| {
            l["prs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_i64().unwrap())
        })
        .collect()
}

fn conflicting_pair(tag: &str) -> Fixture {
    let fx = setup(tag);
    // Both clean against the base, but they rewrite the same line: a pairwise conflict.
    fx.branch(
        "a",
        &[("shared.txt", "A\n2\n3\n4\n5\n"), ("tests/ta", "ok\n")],
    );
    fx.branch(
        "b",
        &[("shared.txt", "B\n2\n3\n4\n5\n"), ("tests/tb", "ok\n")],
    );
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx
}

/// The one-shot plan HOLDS #2 (conflicts-with-queued, stays queued). A cold-cache `run`
/// builds its first batch from base probes only, puts #1 and #2 together, and EJECTS #2.
#[test]
fn a_cold_cache_run_holds_what_the_plan_holds_instead_of_ejecting() {
    let fx = conflicting_pair("rb-cold");
    let run = json(&fx.mq_ok(&["run", "--shadow", "--json"]));
    assert_eq!(
        ejected(&run),
        vec![],
        "cold run ejected a change the plan would hold: {run}"
    );
}

/// Same queue, but `plan` ran first (warm pair cache): #2 is held, not ejected. The outcome of
/// `run` depends on whether the probe cache happened to be warm.
#[test]
fn a_warm_cache_run_holds_a_conflicting_change() {
    let fx = conflicting_pair("rb-warm");
    let plan = json(&fx.mq_ok(&["plan", "--json"]));
    assert_eq!(plan["held"][0]["kind"], "conflicts-with-queued", "{plan}");
    let run = json(&fx.mq_ok(&["run", "--shadow", "--json"]));
    assert_eq!(ejected(&run), vec![], "{run}");
}

/// #1 waits on #99 (not queued), so it is held. #2 is stacked on #1. The one-shot plan holds #2
/// (must land after #1). The cold staged run does not know the ancestry and lands #2, which
/// carries #1's commit onto the base despite #1's hold.
#[test]
fn a_cold_cache_run_never_lands_a_child_of_a_held_parent() {
    let fx = setup("rb-stack");
    fx.branch("p", &[("p.txt", "parent\n"), ("tests/tp", "ok\n")]);
    fx.branch_from("c", "p", &[("c.txt", "child\n"), ("tests/tc", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "p", "--after", "99"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let plan = json(&fx.mq_ok(&["plan", "--json"]));
    let held: Vec<i64> = plan["held"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["pr"].as_i64().unwrap())
        .collect();
    assert_eq!(held, vec![1, 2], "one-shot plan holds both: {plan}");
    // Fresh state for a cold cache.
    std::fs::remove_file(fx.queue().join(".git/merge-queue/probe-cache.json")).unwrap();
    let run = json(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(
        landed(&run),
        Vec::<i64>::new(),
        "child of a held parent landed: {run}"
    );
}

/// A repository attribute (`$GIT_DIR/info/attributes`) changes merge results but is not in the
/// probe-cache key: removing it leaves the stale "clean" answer in force.
#[test]
fn repository_attributes_are_part_of_the_probe_cache_key() {
    let fx = conflicting_pair("rb-attr");
    let info = fx.queue().join(".git/info");
    std::fs::create_dir_all(&info).unwrap();
    std::fs::write(info.join("attributes"), "shared.txt merge=union\n").unwrap();
    let plan = json(&fx.mq_ok(&["plan", "--json"]));
    assert!(
        plan["held"].as_array().unwrap().is_empty(),
        "union makes it clean: {plan}"
    );
    std::fs::remove_file(info.join("attributes")).unwrap();
    let plan = json(&fx.mq_ok(&["plan", "--json"]));
    assert_eq!(
        plan["held"].as_array().unwrap().len(),
        1,
        "attribute removed, merge now conflicts, but cached answer reused: {plan}"
    );
}
