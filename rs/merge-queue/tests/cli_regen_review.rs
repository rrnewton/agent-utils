//! Regression tests from an adversarial review of regeneration and refusal handling: each
//! scenario once produced a wrong outcome (an innocent change ejected, a broken generator
//! ejecting everything, a red run deferred forever, a broken prefix chain, a wedged search).

mod support;

use serde_json::Value;
use support::{git, Fixture};

const RUNNER: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests); else set -- $MQ_TESTS; fi
rc=0
for t in "$@"; do
  o=pass
  if [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then o=fail; cat "tests/$t"; fi
  [ $o = fail ] && rc=1
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#;

/// Generator that is broken whenever a file named BREAK exists at the root.
const REGEN: &str =
    "if [ -e BREAK ]; then echo broken >&2; exit 3; fi; mkdir -p gen && cat $(ls src/* | sort) > gen/all.txt";

fn setup(tag: &str, seed_break: bool, extra: &[&str]) -> Fixture {
    let fx = Fixture::new(tag);
    fx.write("src/a", "a\n");
    fx.write("gen/all.txt", "a\n");
    fx.write("tests/t0", "ok\n");
    if seed_break {
        fx.write("BREAK", "x\n");
    }
    fx.commit_all("seed");
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
        "4",
        "--refusal-max-retries",
        "0",
        "--refusal-backoff-secs",
        "0",
    ];
    args.extend_from_slice(extra);
    fx.mq_ok(&args);
    fx
}

fn run(fx: &Fixture) -> Value {
    let out = fx.mq(&["run", "--json"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "bad report JSON ({e}):\n{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn landed(r: &Value) -> Vec<Vec<i64>> {
    r["landed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            l["prs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_i64().unwrap())
                .collect()
        })
        .collect()
}

fn ejected(r: &Value) -> Vec<(i64, String)> {
    r["ejected"]
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

const ALWAYS: [&str; 6] = [
    "--regen-path",
    "gen/",
    "--regen-command",
    REGEN,
    "--regen-when",
    "always",
];

/// #1 breaks the generator; #2 is innocent. Regeneration after each change blames #1 alone.
#[test]
fn a_generator_breaking_change_is_blamed_not_the_change_after_it() {
    let fx = setup("rev-a-hoist-blame", false, &ALWAYS);
    fx.branch("x", &[("BREAK", "x\n"), ("tests/x", "ok\n")]);
    fx.branch("b", &[("src/b", "b\n"), ("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "x"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let r = run(&fx);
    let ej = ejected(&r);
    assert!(
        !ej.iter().any(|(pr, _)| *pr == 2),
        "innocent #2 was ejected: {ej:?}\n{r}"
    );
    assert_eq!(landed(&r), vec![vec![2]], "{r}");
    assert_eq!(ejected(&r), vec![(1, "regen-failed".to_owned())], "{r}");
}

/// The base itself breaks the generator: nothing is the changes' fault, so nothing is ejected
/// and the run stops with the reason.
#[test]
fn a_generator_broken_on_the_base_ejects_nothing() {
    let fx = setup("rev-a-base-broken", true, &ALWAYS);
    fx.branch("b", &[("src/b", "b\n"), ("tests/b", "ok\n")]);
    fx.branch("c", &[("src/c", "c\n"), ("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let r = run(&fx);
    assert!(
        ejected(&r).is_empty(),
        "changes ejected for a generator broken on the base: {:?}\n{r}",
        ejected(&r)
    );
    assert!(
        r["stopped"]
            .as_str()
            .unwrap()
            .contains("regeneration fails on"),
        "{r}"
    );
}

/// A genuinely red run whose output merely mentions the refusal marker is a failure, not a
/// refusal: the culprit is ejected rather than deferred forever.
#[test]
fn a_red_run_mentioning_the_refusal_marker_is_still_red() {
    let fx = setup("rev-a-marker", false, &[]);
    fx.branch(
        "x",
        &[("tests/x", "FAIL: expected COULD_NOT_RUN handling\n")],
    );
    fx.mq_ok(&["submit", "1", "--head", "x"]);
    let r = run(&fx);
    assert!(
        r["deferred"].as_array().is_none_or(|d| d.is_empty()),
        "red run deferred as a refusal: {r}"
    );
    assert_eq!(ejected(&r), vec![(1, "failed-tests".to_owned())], "{r}");
}

/// With regeneration after each change, the regenerated prefixes form a fast-forward chain,
/// so a per-change driver that pushes $MQ_PREFIX lands the whole batch.
#[test]
fn regenerated_prefixes_form_a_fast_forward_chain_for_the_command_driver() {
    let mut extra: Vec<&str> = ALWAYS.to_vec();
    extra.extend([
        "--land",
        "command",
        "--land-command",
        "git push -q \"$MQ_REMOTE\" \"$MQ_PREFIX:refs/heads/$MQ_BASE\"",
        "--allow-batches",
    ]);
    let fx = setup("rev-a-cmd-chain", false, &extra);
    // Both add a source and forget the generated file (always mode repairs it).
    fx.branch("b", &[("src/b", "b\n"), ("tests/b", "ok\n")]);
    fx.branch("c", &[("src/c", "c\n"), ("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let r = run(&fx);
    assert_eq!(landed(&r), vec![vec![1, 2]], "{r}");
}

/// Stacked changes: #1 alone breaks the generator, #2 (built on #1) fixes it. #1's own prefix
/// cannot be regenerated, so #1 is ejected when the candidate is built and #2, which contains
/// it, is set aside as its dependent: the queue moves on instead of wedging on a prefix.
#[test]
fn a_prefix_that_cannot_be_regenerated_ejects_its_change_without_wedging() {
    let regen = "if [ -e BREAK ] && [ ! -e FIX ]; then exit 3; fi; mkdir -p gen && cat $(ls src/* | sort) > gen/all.txt";
    let fx = setup(
        "rev-a-prefix-wedge",
        false,
        &[
            "--regen-path",
            "gen/",
            "--regen-command",
            regen,
            "--regen-when",
            "always",
        ],
    );
    fx.branch("x", &[("BREAK", "x\n"), ("tests/x", "ok\n")]);
    fx.branch_from("y", "x", &[("FIX", "y\n"), ("tests/y", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "x"]);
    fx.mq_ok(&["submit", "2", "--head", "y"]);
    let first = run(&fx);
    assert!(first.get("stopped").is_none(), "{first}");
    assert_eq!(
        ejected(&first),
        vec![(1, "regen-failed".to_owned())],
        "{first}"
    );
    assert_eq!(first["requeued"], serde_json::json!([2]), "{first}");
    let second = run(&fx);
    assert!(second.get("stopped").is_none(), "{second}");
    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    assert_eq!(plan["held"][0]["kind"], "waiting-on-dependency", "{plan}");
}
