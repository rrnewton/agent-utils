//! Count-pin recounting: two changes bumping the same pin batch together
//! (base plus both deltas), an unrecountable change is skipped and stays
//! queued, and with the flag off the same changes conflict as before.

mod support;

use serde_json::Value;
use support::{git, Fixture};

const PIN: &str = "ci/manifest-plan/src/validation_dag_static.rs";

const RUNNER: &str = r#"rc=0
for t in $(ls tests); do
  if grep -q FAIL "tests/$t"; then o=fail; rc=1; else o=pass; fi
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#;

fn pin_text(alpha: u64) -> String {
    format!(
        "pub(super) const NEXTEST_EXPECTED_COUNTS: &[(&str, u64)] = &[\n    (\"test.alpha\", {alpha}),\n    (\"test.beta\", 5),\n];\n"
    )
}

fn setup(tag: &str, recount: bool) -> Fixture {
    let fx = Fixture::new(tag);
    fx.write(PIN, &pin_text(10));
    fx.write("tests/t0", "ok\n");
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
    ];
    if recount {
        args.push("--recount-pins");
    }
    fx.mq_ok(&args);
    fx
}

fn report(stdout: &str) -> Value {
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("bad report JSON ({e}):\n{stdout}"))
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

#[test]
fn two_changes_bumping_the_same_pin_recount_to_base_plus_both_deltas() {
    let fx = setup("recount-batch", true);
    fx.branch("b", &[(PIN, &pin_text(13)), ("tests/b", "ok\n")]);
    fx.branch("c", &[(PIN, &pin_text(12)), ("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    // The pin conflict alone must not hold either change.
    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    assert!(plan["held"].as_array().unwrap().is_empty(), "{plan}");
    assert_eq!(
        plan["batches"][0]["prs"],
        serde_json::json!([1, 2]),
        "{plan}"
    );

    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1, 2]], "{r}");
    // 10 (base) + 3 (#1) + 2 (#2); the other pin is untouched.
    let landed_pin = git(&fx.remote(), &["show", &format!("main:{PIN}")]);
    assert!(landed_pin.contains("(\"test.alpha\", 15)"), "{landed_pin}");
    assert!(landed_pin.contains("(\"test.beta\", 5)"), "{landed_pin}");
}

#[test]
fn an_unrecountable_change_is_skipped_and_stays_queued() {
    let fx = setup("recount-refuse", true);
    fx.branch("b", &[(PIN, &pin_text(11)), ("tests/b", "ok\n")]);
    // A second occurrence of the same pin: no (name, occurrence) match.
    let bad = "pub(super) const NEXTEST_EXPECTED_COUNTS: &[(&str, u64)] = &[\n    (\"test.alpha\", 11),\n    (\"test.alpha\", 1),\n    (\"test.beta\", 5),\n];\n";
    fx.branch("c", &[(PIN, bad), ("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1]], "{r}");
    let skips = r["recount_skips"].as_array().unwrap();
    assert_eq!(skips.len(), 1, "{r}");
    assert_eq!(skips[0]["pr"], 2, "{r}");
    assert!(
        skips[0]["reason"]
            .as_str()
            .unwrap()
            .contains("recount-pins refused"),
        "{r}"
    );
    // Still queued, and the skip is in the decision log beside the
    // validation records (`decisions/skips.jsonl`, one JSON object per line).
    let list: Value = serde_json::from_str(&fx.mq_ok(&["list", "--json"])).unwrap();
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .any(|item| item["pr"] == 2 && item["status"] == "queued"),
        "{list}"
    );
    let skips_log =
        std::fs::read_to_string(fx.queue().join(".git/merge-queue/decisions/skips.jsonl")).unwrap();
    assert!(skips_log.contains("\"decision\":\"skip\""), "{skips_log}");
    assert!(skips_log.contains("\"pr\":2"), "{skips_log}");
    assert!(skips_log.contains("recount-pins refused"), "{skips_log}");
}

#[test]
fn without_the_flag_the_same_changes_conflict() {
    let fx = setup("recount-off", false);
    fx.branch("b", &[(PIN, &pin_text(13)), ("tests/b", "ok\n")]);
    fx.branch("c", &[(PIN, &pin_text(12)), ("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    assert_eq!(plan["held"][0]["kind"], "conflicts-with-queued", "{plan}");
}

/// A generator that compiles the pin file (refusing conflict markers, as a compiler would) and
/// writes the pins it read into `gen/dag.txt`, a generated file that embeds them.
const PIN_GENERATOR: &str = r#"if grep -q '^<<<<<<< ' ci/manifest-plan/src/validation_dag_static.rs; then
  echo 'error: expected type, found `<<<<<<<`' >&2; exit 101
fi
mkdir -p gen && grep -o '("test[.][a-z]*", [0-9]*)' ci/manifest-plan/src/validation_dag_static.rs > gen/dag.txt"#;

/// Like `RUNNER`, plus a `fresh` test that fails when `gen/dag.txt` is not what the generator
/// makes of the pin file (the generated file's own freshness check).
const RUNNER_FRESH: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests) fresh; else set -- $MQ_TESTS; fi
rc=0
for t in "$@"; do
  o=pass
  if [ "$t" = fresh ]; then
    grep -o '("test[.][a-z]*", [0-9]*)' ci/manifest-plan/src/validation_dag_static.rs | cmp -s - gen/dag.txt || o=fail
  elif grep -q FAIL "tests/$t"; then o=fail; fi
  [ $o = fail ] && rc=1
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#;

fn dag_text(alpha: u64) -> String {
    format!("(\"test.alpha\", {alpha})\n(\"test.beta\", 5)\n")
}

#[test]
fn a_pin_conflict_in_a_file_the_generator_compiles_is_settled_recounted_then_regenerated() {
    // The pin file is listed as regenerable and is compiled into the generator. A change and
    // the base each bumped the same pin, so combining them conflicts there and in the generated
    // file. The conflict must be settled before the generator runs (markers would stop it from
    // compiling, and the change would be ejected as `regen-failed`), the pins recounted next,
    // and only then the generated file rewritten, so that it embeds the recounted pins.
    let fx = Fixture::new("recount-regen");
    fx.write(PIN, &pin_text(10));
    fx.write("gen/dag.txt", &dag_text(10));
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        RUNNER_FRESH,
        "--recount-pins",
        "--regen-path",
        "gen/",
        "--regen-path",
        PIN,
        "--regen-command",
        PIN_GENERATOR,
    ]);
    // The change adds three tests: 10 -> 13, regenerated as its author saw it.
    fx.branch(
        "b",
        &[
            (PIN, &pin_text(13)),
            ("gen/dag.txt", &dag_text(13)),
            ("tests/b", "ok\n"),
        ],
    );
    // Meanwhile the base added one: 10 -> 11, regenerated likewise.
    fx.write(PIN, &pin_text(11));
    fx.write("gen/dag.txt", &dag_text(11));
    fx.commit_all("advance main");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    let base = fx.remote_main();

    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1]], "{r}");
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    // 11 (base) + 3 (the change), in the pin file and in the file generated from it.
    let landed_pin = git(&fx.remote(), &["show", &format!("main:{PIN}")]);
    assert!(landed_pin.contains("(\"test.alpha\", 14)"), "{landed_pin}");
    assert!(!landed_pin.contains("<<<<<<<"), "{landed_pin}");
    assert_eq!(
        git(&fx.remote(), &["show", "main:gen/dag.txt"]),
        dag_text(14).trim_end(),
        "{r}"
    );
    let regenerated = r["regenerated"].as_array().unwrap();
    assert_eq!(regenerated.len(), 1, "{r}");
    assert_eq!(
        regenerated[0]["conflicted"],
        serde_json::json!([PIN, "gen/dag.txt"]),
        "{r}"
    );
    // No commit on the landed range holds a conflict marker.
    for commit in git(&fx.remote(), &["rev-list", &format!("{base}..main")]).lines() {
        let text = git(&fx.remote(), &["show", &format!("{commit}:{PIN}")]);
        assert!(!text.contains("<<<<<<<"), "{commit}: {text}");
    }
}
