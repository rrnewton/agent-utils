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

/// The pin file with `alpha`, then `beta` at 5, and `gamma` either absent or right after
/// `beta` (`gamma_after_beta`) or right after `alpha`.
fn pins_with(alpha: u64, gamma: Option<(u64, bool)>) -> String {
    let gamma_line = |value: u64| format!("    (\"test.gamma\", {value}),\n");
    let mut text = "pub(super) const NEXTEST_EXPECTED_COUNTS: &[(&str, u64)] = &[\n".to_owned();
    text.push_str(&format!("    (\"test.alpha\", {alpha}),\n"));
    if let Some((value, false)) = gamma {
        text.push_str(&gamma_line(value));
    }
    text.push_str("    (\"test.beta\", 5),\n");
    if let Some((value, true)) = gamma {
        text.push_str(&gamma_line(value));
    }
    text.push_str("];\n");
    text
}

#[test]
fn a_pin_occurrence_the_head_introduces_lands_with_the_head_value() {
    let fx = setup("recount-new-pin", true);
    // #1 bumps alpha; #2 bumps alpha too and introduces gamma, away from the alpha conflict.
    fx.branch("b", &[(PIN, &pin_text(12)), ("tests/b", "ok\n")]);
    fx.branch(
        "c",
        &[(PIN, &pins_with(11, Some((4, true)))), ("tests/c", "ok\n")],
    );
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1, 2]], "{r}");
    // No refusal: the report leaves out an empty `recount_skips`.
    assert!(r.get("recount_skips").is_none(), "{r}");
    // 10 (base) + 2 (#1) + 1 (#2); gamma is #2's own value.
    let landed_pin = git(&fx.remote(), &["show", &format!("main:{PIN}")]);
    assert!(landed_pin.contains("(\"test.alpha\", 13)"), "{landed_pin}");
    assert!(landed_pin.contains("(\"test.gamma\", 4)"), "{landed_pin}");
    assert!(landed_pin.contains("(\"test.beta\", 5)"), "{landed_pin}");
}

#[test]
fn a_pin_the_base_added_since_does_not_refuse_a_change_that_leaves_it() {
    let fx = setup("recount-base-new-pin", true);
    // The change branches from the seed, before the base gains gamma.
    fx.branch("c", &[(PIN, &pin_text(11)), ("tests/c", "ok\n")]);
    git(&fx.dev(), &["checkout", "-q", "main"]);
    fx.write(PIN, &pins_with(10, Some((4, true))));
    fx.commit_all("the base gains gamma");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.mq_ok(&["submit", "1", "--head", "c"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    // No refusal: the report leaves out an empty `recount_skips`.
    assert!(r.get("recount_skips").is_none(), "{r}");
    assert_eq!(landed(&r), vec![vec![1]], "{r}");
    let landed_pin = git(&fx.remote(), &["show", &format!("main:{PIN}")]);
    assert!(landed_pin.contains("(\"test.alpha\", 11)"), "{landed_pin}");
    assert!(landed_pin.contains("(\"test.gamma\", 4)"), "{landed_pin}");
}

#[test]
fn a_new_pin_lost_settling_a_conflict_ejects_the_change_naming_it() {
    let fx = setup("recount-new-pin-lost", true);
    // #2 adds gamma right next to the alpha line #1 also bumps: the settled conflict hunk takes
    // #1's side and drops gamma, so #2 cannot be recounted into the batch.
    fx.branch("b", &[(PIN, &pin_text(12)), ("tests/b", "ok\n")]);
    fx.branch(
        "c",
        &[(PIN, &pins_with(11, Some((4, false)))), ("tests/c", "ok\n")],
    );
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1]], "{r}");
    let ejected = r["ejected"].as_array().unwrap();
    assert_eq!(ejected[0]["pr"], 2, "{r}");
    let reason = ejected[0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("the pin \"test.gamma\" (occurrence 0) it introduces was lost"),
        "{reason}"
    );
    assert!(reason.contains("rebase and resubmit"), "{reason}");
    // The landed pin file holds no half-recounted gamma.
    let landed_pin = git(&fx.remote(), &["show", &format!("main:{PIN}")]);
    assert!(!landed_pin.contains("test.gamma"), "{landed_pin}");
}

#[test]
fn a_decrease_is_refused_naming_the_merge_base_and_head_values() {
    let fx = setup("recount-decrease", true);
    fx.branch("c", &[(PIN, &pin_text(8)), ("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "c"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert!(landed(&r).is_empty(), "{r}");
    let skips = r["recount_skips"].as_array().unwrap();
    let reason = skips[0]["reason"].as_str().unwrap();
    assert!(
        reason.contains(
            "pin \"test.alpha\" (occurrence 0) is 10 at the merge base and 8 at the head (-2)"
        ),
        "{reason}"
    );
    assert!(
        reason.contains("it resolves to 8, below the base"),
        "{reason}"
    );
    assert!(
        reason.contains("an undeclared decrease is refused"),
        "{reason}"
    );
    assert!(
        reason.contains("`Count-Pin-Decrease: test.alpha 2 <reason>`"),
        "{reason}"
    );
}

/// Push a branch from `main` with one commit writing `files` under `message`.
fn branch_with_message(fx: &Fixture, name: &str, files: &[(&str, &str)], message: &str) {
    git(&fx.dev(), &["checkout", "-q", "-B", name, "main"]);
    for (path, text) in files {
        fx.write(path, text);
    }
    fx.commit_all(message);
    git(
        &fx.dev(),
        &["push", "-q", "-f", "origin", &format!("HEAD:{name}")],
    );
    git(&fx.dev(), &["checkout", "-q", "main"]);
}

#[test]
fn a_declared_decrease_lands_and_is_recorded() {
    let fx = setup("recount-declared", true);
    // #1 moves two alpha tests away and says so; #2 adds one alpha test in the same batch.
    branch_with_message(
        &fx,
        "b",
        &[(PIN, &pin_text(8)), ("tests/b", "ok\n")],
        "Move two alpha tests to the privileged node\n\nCount-Pin-Decrease: test.alpha 2 two tests run on the privileged node instead\n",
    );
    fx.branch("c", &[(PIN, &pin_text(11)), ("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1, 2]], "{r}");
    // 10 (base) - 2 (#1, declared) + 1 (#2).
    let landed_pin = git(&fx.remote(), &["show", &format!("main:{PIN}")]);
    assert!(landed_pin.contains("(\"test.alpha\", 9)"), "{landed_pin}");
    // Recorded in the run report and in the change's history.
    assert_eq!(
        r["pin_decreases"],
        serde_json::json!([{
            "pr": 1,
            "pin": "test.alpha",
            "occurrence": 0,
            "amount": 2,
            "reason": "two tests run on the privileged node instead"
        }]),
        "{r}"
    );
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    let item = items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["pr"] == 1)
        .unwrap();
    let recorded = item["history"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["kind"] == "count-pin-decrease")
        .unwrap_or_else(|| panic!("{item}"));
    assert!(
        recorded["detail"].as_str().unwrap().contains(
            "declared decrease of pin \"test.alpha\" (occurrence 0) by 2 accepted (10 at the merge base, 8 at the head)"
        ),
        "{recorded}"
    );
}

#[test]
fn a_decrease_larger_than_declared_is_refused() {
    let fx = setup("recount-over-declared", true);
    branch_with_message(
        &fx,
        "b",
        &[(PIN, &pin_text(7)), ("tests/b", "ok\n")],
        "Move alpha tests\n\nCount-Pin-Decrease: test.alpha 2 two tests move\n",
    );
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert!(landed(&r).is_empty(), "{r}");
    let reason = r["recount_skips"][0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("is 10 at the merge base and 7 at the head (-3)"),
        "{reason}"
    );
    assert!(
        reason.contains("but the head commit declares a decrease of 2"),
        "{reason}"
    );
    assert!(r.get("pin_decreases").is_none(), "{r}");
}

const REMOVAL_SEED: &str = "pub(super) const NEXTEST_EXPECTED_COUNTS: &[(&str, u64)] = &[\n    (\"gone.a\", 10),\n    (\"gone.b\", 10),\n    (\"test.alpha\", 10),\n    (\"test.beta\", 5),\n];\n";
const REMOVAL_KEPT: &str = "pub(super) const NEXTEST_EXPECTED_COUNTS: &[(&str, u64)] = &[\n    (\"test.alpha\", 10),\n    (\"test.beta\", 5),\n];\n";

fn removal_fixture(tag: &str) -> Fixture {
    let fx = Fixture::new(tag);
    fx.write(PIN, REMOVAL_SEED);
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
        RUNNER,
        "--max-batch",
        "4",
        "--recount-pins",
    ]);
    fx
}

#[test]
fn a_declared_mass_removal_lands_and_is_recorded() {
    let fx = removal_fixture("recount-removal-declared");
    branch_with_message(
        &fx,
        "b",
        &[(PIN, REMOVAL_KEPT), ("tests/b", "ok\n")],
        "Remove gone pins for census gating\n\nCount-Pin-Removed: gone.* 2 census gating\n",
    );
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1]], "{r}");
    let landed_pin = git(&fx.remote(), &["show", &format!("main:{PIN}")]);
    assert!(!landed_pin.contains("gone."), "{landed_pin}");
    assert_eq!(
        r["pin_removals"],
        serde_json::json!([{
            "pr": 1,
            "pattern": "gone.*",
            "removed": 2,
            "reason": "census gating"
        }]),
        "{r}"
    );
}

#[test]
fn an_undeclared_mass_removal_names_every_pin_in_one_refusal() {
    let fx = removal_fixture("recount-removal-undeclared");
    branch_with_message(
        &fx,
        "b",
        &[(PIN, REMOVAL_KEPT), ("tests/b", "ok\n")],
        "Remove gone pins\n",
    );
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert!(landed(&r).is_empty(), "{r}");
    let reason = r["recount_skips"][0]["reason"].as_str().unwrap();
    for pin in ["gone.a", "gone.b"] {
        assert!(
            reason.contains(&format!("Count-Pin-Removed: {pin} <reason>")),
            "{reason}"
        );
    }
}

/// Push `name` from main deleting the pin file in one commit under `message`.
fn branch_deleting_pin_file(fx: &Fixture, name: &str, test_path: &str, message: &str) {
    git(&fx.dev(), &["checkout", "-q", "-B", name, "main"]);
    git(&fx.dev(), &["rm", "-q", PIN]);
    fx.write(test_path, "ok\n");
    fx.commit_all(message);
    git(
        &fx.dev(),
        &["push", "-q", "-f", "origin", &format!("HEAD:{name}")],
    );
    git(&fx.dev(), &["checkout", "-q", "main"]);
}

#[test]
fn an_exact_change_is_never_recounted() {
    // The same undeclared removal as above lands untouched when submitted
    // exact: recount would refuse it, and an exact change is never recounted
    // or rewritten.
    let fx = removal_fixture("recount-exact-skip");
    branch_with_message(
        &fx,
        "b",
        &[(PIN, REMOVAL_KEPT), ("tests/b", "ok\n")],
        "Remove gone pins\n",
    );
    fx.mq_ok(&["submit", "1", "--head", "b", "--exact"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1]], "{r}");
    let landed_pin = git(&fx.remote(), &["show", &format!("main:{PIN}")]);
    assert!(!landed_pin.contains("gone."), "{landed_pin}");
    assert!(r.get("recount_skips").is_none(), "{r}");
}

#[test]
fn a_declared_retirement_deleting_the_pin_file_lands_and_is_recorded() {
    let fx = removal_fixture("recount-retirement-declared");
    branch_deleting_pin_file(
        &fx,
        "b",
        "tests/b",
        "Retire count pins for census gating\n\nCount-Pins-Retired: census gating\n",
    );
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1]], "{r}");
    let listed = git(&fx.remote(), &["ls-tree", "--name-only", "main", "--", PIN]);
    assert!(listed.trim().is_empty(), "{listed}");
    assert_eq!(
        r["pin_retirements"],
        serde_json::json!([{"pr": 1, "reason": "census gating"}]),
        "{r}"
    );
}

#[test]
fn an_undeclared_retirement_is_refused_naming_the_trailer() {
    let fx = removal_fixture("recount-retirement-undeclared");
    branch_deleting_pin_file(&fx, "b", "tests/b", "Retire count pins\n");
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert!(landed(&r).is_empty(), "{r}");
    let reason = r["recount_skips"][0]["reason"].as_str().unwrap();
    assert!(reason.contains("Count-Pins-Retired: <reason>"), "{reason}");
}

/// Push `name` from `from` with one commit writing `files` under `message`.
fn stacked_branch(fx: &Fixture, name: &str, from: &str, files: &[(&str, &str)], message: &str) {
    git(&fx.dev(), &["checkout", "-q", "-B", name, from]);
    for (path, text) in files {
        fx.write(path, text);
    }
    fx.commit_all(message);
    git(
        &fx.dev(),
        &["push", "-q", "-f", "origin", &format!("HEAD:{name}")],
    );
    git(&fx.dev(), &["checkout", "-q", "main"]);
}

#[test]
fn a_change_stacked_on_another_in_the_batch_counts_only_its_own_pin_edits() {
    // #1 moves two alpha tests away and declares it; #2 is stacked on #1 and adds one alpha
    // test. Measured from its merge base #2 would carry #1's -2 as an undeclared -1 of its own;
    // measured from #1's head it adds +1, and the batch counts #1's decrease once.
    let fx = setup("recount-stacked", true);
    branch_with_message(
        &fx,
        "b",
        &[(PIN, &pin_text(8)), ("tests/b", "ok\n")],
        "Move two alpha tests\n\nCount-Pin-Decrease: test.alpha 2 two tests move\n",
    );
    stacked_branch(
        &fx,
        "c",
        "b",
        &[(PIN, &pin_text(9)), ("tests/c", "ok\n")],
        "Add an alpha test",
    );
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1, 2]], "{r}");
    // 10 (base) - 2 (#1, declared) + 1 (#2).
    let landed_pin = git(&fx.remote(), &["show", &format!("main:{PIN}")]);
    assert!(landed_pin.contains("(\"test.alpha\", 9)"), "{landed_pin}");
    assert_eq!(r["pin_decreases"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["pin_decreases"][0]["pr"], 1, "{r}");
}

#[test]
fn a_change_stacked_on_one_that_landed_counts_only_its_own_pin_edits() {
    // #1 lands first (as a rebased copy, so #2's merge base with the new base still predates
    // it). #2 is measured from #1's head, which the base already counts.
    let fx = setup("recount-stacked-landed", true);
    branch_with_message(
        &fx,
        "b",
        &[(PIN, &pin_text(8)), ("tests/b", "ok\n")],
        "Move two alpha tests\n\nCount-Pin-Decrease: test.alpha 2 two tests move\n",
    );
    // The base moves on, so #1 lands as a rebased copy rather than as its own commit.
    fx.write("other", "other\n");
    fx.commit_all("an unrelated change");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1]], "{r}");
    let landed_b = git(&fx.remote(), &["rev-parse", "main"]);
    let original_b = git(&fx.remote(), &["rev-parse", "b"]);
    assert_ne!(landed_b, original_b, "#1 must land as a copy for this test");
    stacked_branch(
        &fx,
        "c",
        "b",
        &[(PIN, &pin_text(9)), ("tests/c", "ok\n")],
        "Add an alpha test",
    );
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![2]], "{r}");
    // 8 (base, with #1) + 1 (#2).
    let landed_pin = git(&fx.remote(), &["show", &format!("main:{PIN}")]);
    assert!(landed_pin.contains("(\"test.alpha\", 9)"), "{landed_pin}");
    assert!(r.get("pin_decreases").is_none(), "{r}");
}

/// #1 moves two alpha tests away and declares it; #2 is stacked on #1, changes no pin, and
/// carries a copy of #1's trailer (as a lane might, after seeing #2 refused for #1's decrease).
/// The base moves on first, so #1 lands as a rebased copy.
fn stacked_with_copied_trailer(tag: &str) -> Fixture {
    let fx = setup(tag, true);
    branch_with_message(
        &fx,
        "b",
        &[(PIN, &pin_text(8)), ("tests/b", "ok\n")],
        "Move two alpha tests\n\nCount-Pin-Decrease: test.alpha 2 two tests move\n",
    );
    stacked_branch(
        &fx,
        "c",
        "b",
        &[("tests/c", "ok\n")],
        "Add a beta test\n\nCount-Pin-Decrease: test.alpha 2 two tests move\n",
    );
    fx.write("other", "other\n");
    fx.commit_all("an unrelated change");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    fx
}

/// The pin landed, the decreases the run recorded, and how many times #1's commit landed.
fn landed_outcome(fx: &Fixture, r: &Value) -> (String, Vec<i64>, usize) {
    let landed_pin = git(&fx.remote(), &["show", &format!("main:{PIN}")]);
    let decreases = r["pin_decreases"]
        .as_array()
        .map(|all| all.iter().map(|d| d["pr"].as_i64().unwrap()).collect())
        .unwrap_or_default();
    let subjects = git(&fx.remote(), &["log", "--format=%s", "main"]);
    (
        landed_pin,
        decreases,
        subjects.matches("Move two alpha tests").count(),
    )
}

#[test]
fn a_trailer_copied_onto_a_change_stacked_in_the_same_batch_counts_nothing() {
    let fx = stacked_with_copied_trailer("recount-stacked-trailer");
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1, 2]], "{r}");
    let (pin, decreases, copies) = landed_outcome(&fx, &r);
    // 10 (base) - 2 (#1, declared); #2 changes no pin, so its trailer matches nothing.
    assert!(pin.contains("(\"test.alpha\", 8)"), "{pin}");
    assert_eq!(decreases, vec![1], "{r}");
    assert_eq!(copies, 1);
}

#[test]
fn a_change_stacked_on_one_in_the_batch_beneath_counts_only_its_own_pin_edits() {
    // One change per batch, two batches in flight: #2's batch is built on #1's candidate, which
    // carries a rebased copy of #1 before it lands. Measured from its merge base, #2 would carry
    // #1's -2 again and its copied trailer would accept it, counting the -2 twice; measured from
    // #1's head, as once #1 has landed, it changes nothing.
    let fx = stacked_with_copied_trailer("recount-stacked-window");
    let r = report(&fx.mq_ok(&["run", "--json", "--max-batch", "1", "--depth", "2"]));
    assert_eq!(landed(&r), vec![vec![1], vec![2]], "{r}");
    assert_eq!(r["windows"], serde_json::json!([2, 2]), "{r}");
    let (pin, decreases, copies) = landed_outcome(&fx, &r);
    assert!(pin.contains("(\"test.alpha\", 8)"), "{pin}");
    assert_eq!(decreases, vec![1], "{r}");
    // #1's own commit is not replayed on top of its copy.
    assert_eq!(copies, 1);
}
