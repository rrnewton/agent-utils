//! Generated-file regeneration, the exact-commit rule, fast-forward-preserving rebase and the
//! queue's commit identity, end to end against a local bare remote.

mod support;

use serde_json::Value;
use support::{git, Fixture};

/// Each file under `tests/` is a test that fails when it contains FAIL; `gen` is a test that
/// fails when `gen/all.txt` is not exactly the sorted concatenation of `src/*`.
const RUNNER: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests) gen; else set -- $MQ_TESTS; fi
rc=0
for t in "$@"; do
  o=pass
  if [ "$t" = gen ]; then
    cat $(ls src/* | sort) | cmp -s - gen/all.txt || o=fail
  elif [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then o=fail; fi
  [ $o = fail ] && rc=1
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#;

const REGEN: &str = "mkdir -p gen && cat $(ls src/* | sort) > gen/all.txt";

fn setup(tag: &str, extra: &[&str]) -> Fixture {
    let fx = Fixture::new(tag);
    fx.write("src/a", "a\n");
    fx.write("gen/all.txt", "a\n");
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
    args.extend_from_slice(extra);
    fx.mq_ok(&args);
    fx
}

/// A change adding source `name` and updating the generated file as its author saw it.
fn source_change(fx: &Fixture, name: &str) -> String {
    git(&fx.dev(), &["checkout", "-q", "-B", name, "main"]);
    fx.write(&format!("src/{name}"), &format!("{name}\n"));
    fx.write("gen/all.txt", &format!("a\n{name}\n"));
    fx.write(&format!("tests/{name}"), "ok\n");
    fx.commit_all(&format!("{name}: add source"));
    git(
        &fx.dev(),
        &["push", "-q", "-f", "origin", &format!("HEAD:{name}")],
    );
    let head = git(&fx.dev(), &["rev-parse", "HEAD"]);
    git(&fx.dev(), &["checkout", "-q", "main"]);
    head
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
fn conflicts_only_in_generated_files_are_regenerated_not_held() {
    let regen = [
        "--regen-path",
        "gen/",
        "--regen-command",
        REGEN,
        "--identity-name",
        "queue-bot",
        "--identity-email",
        "bot@example.com",
    ];
    let fx = setup("regen", &regen);
    source_change(&fx, "b");
    source_change(&fx, "c");
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    // Both changes rewrote gen/all.txt differently; that conflict is not a reason to hold one.
    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    assert!(plan["held"].as_array().unwrap().is_empty(), "{plan}");
    assert_eq!(plan["batches"][0]["prs"], serde_json::json!([1, 2]));

    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1, 2]], "{r}");
    // Regeneration runs after each change that touched the generated file: #1's own version is
    // already current, #2's combination conflicted and is rewritten.
    let regenerated = r["regenerated"].as_array().unwrap();
    assert_eq!(regenerated.len(), 2, "{r}");
    assert_eq!(regenerated[0]["pr"], 1);
    assert_eq!(regenerated[0]["rewritten"], serde_json::json!([]));
    assert_eq!(regenerated[1]["pr"], 2);
    assert_eq!(
        regenerated[1]["rewritten"],
        serde_json::json!(["gen/all.txt"])
    );
    let generated = git(&fx.remote(), &["show", "main:gen/all.txt"]);
    assert_eq!(generated, "a\nb\nc");
    // The regeneration commit is the queue's, under its configured identity, and it is what
    // landed (the exact validated commit).
    let committer = git(
        &fx.remote(),
        &["log", "-1", "--format=%cn <%ce>|%an|%s", "main"],
    );
    assert!(
        committer.starts_with(
            "queue-bot <bot@example.com>|queue-bot|merge-queue: regenerate gen/all.txt after #2"
        ),
        "{committer}"
    );
}

#[test]
fn a_generator_built_from_its_own_output_never_sees_conflict_markers() {
    // The generated file is also source the generator is built from (a generated table compiled
    // into the generator), so conflict markers in it would fail the generator before it rewrote
    // anything. A conflicted generated file is taken from the base side, then rewritten.
    let regen = [
        "--regen-path",
        "gen/",
        "--regen-command",
        "if grep -q '<<<<<<<' gen/all.txt; then echo marked; exit 101; fi; mkdir -p gen && cat $(ls src/* | sort) > gen/all.txt",
        "--identity-name",
        "queue-bot",
        "--identity-email",
        "bot@example.com",
    ];
    let fx = setup("regen-own-source", &regen);
    source_change(&fx, "b");
    source_change(&fx, "c");
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1, 2]], "{r}");
    assert_eq!(git(&fx.remote(), &["show", "main:gen/all.txt"]), "a\nb\nc");
}

#[test]
fn without_regeneration_the_same_changes_conflict() {
    let fx = setup("regen-off", &[]);
    source_change(&fx, "b");
    source_change(&fx, "c");
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    assert_eq!(plan["held"][0]["kind"], "conflicts-with-queued");
}

#[test]
fn regeneration_that_touches_other_files_ejects_only_that_change() {
    // The generator misbehaves only on trees containing src/c.
    let bad =
        "cat $(ls src/* | sort) > gen/all.txt && if [ -e src/c ]; then echo oops > tests/t0; fi";
    let fx = setup(
        "regen-bad",
        &["--regen-path", "gen/", "--regen-command", bad],
    );
    source_change(&fx, "b");
    source_change(&fx, "c");
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1]], "{r}");
    assert_eq!(r["ejected"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["ejected"][0]["pr"], 2);
    assert_eq!(r["ejected"][0]["kind"], "regen-touched-other");
    assert!(
        r["ejected"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("not generated: tests/t0"),
        "{r}"
    );
}

#[test]
fn a_change_that_breaks_the_generator_is_ejected_as_regen_failed() {
    let fx = setup(
        "regen-gen-fails",
        &[
            "--regen-path",
            "gen/",
            "--regen-command",
            "if [ -e src/c ]; then echo 'unknown argument' >&2; exit 3; fi; cat $(ls src/* | sort) > gen/all.txt",
        ],
    );
    source_change(&fx, "b");
    source_change(&fx, "c");
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1]], "{r}");
    assert_eq!(r["ejected"][0]["pr"], 2);
    assert_eq!(r["ejected"][0]["kind"], "regen-failed");
    assert!(r["ejected"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("unknown argument"));
}

#[test]
fn a_generator_killed_by_an_outside_signal_ejects_nothing() {
    // A service stop or the OOM killer can kill the generator while it works on a change. That
    // says nothing about the change: the run stops with no verdict instead of reading the kill
    // as `regen-failed` and ejecting it.
    let fx = setup(
        "regen-killed",
        &[
            "--regen-path",
            "gen/",
            "--regen-command",
            "if [ -e src/c ]; then kill -TERM $$; fi; cat $(ls src/* | sort) > gen/all.txt",
        ],
    );
    source_change(&fx, "b");
    source_change(&fx, "c");
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let out = fx.mq(&["run", "--json"]);
    assert_eq!(out.status.code(), Some(3));
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    let stopped = r["stopped"].as_str().unwrap();
    assert!(
        stopped.contains("regenerate.command was killed by signal 15"),
        "{stopped}"
    );
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    for item in items.as_array().unwrap() {
        assert_ne!(item["status"], "ejected", "{items}");
    }
}

#[test]
fn a_generator_broken_on_the_base_stops_the_queue_without_ejecting() {
    // Whatever the change, the generator fails: blaming each change in turn would eject them
    // all. The queue checks the tree before the change, sees it fail too, and stops.
    let fx = setup(
        "regen-base-broken",
        &["--regen-path", "gen/", "--regen-command", "exit 3"],
    );
    source_change(&fx, "b");
    source_change(&fx, "c");
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let out = fx.mq(&["run", "--json"]);
    assert_eq!(out.status.code(), Some(3));
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    assert!(
        r["stopped"]
            .as_str()
            .unwrap()
            .contains("regeneration fails on"),
        "{r}"
    );
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--json"])).unwrap();
    assert_eq!(items.as_array().unwrap().len(), 2, "both still queued");
}

#[test]
fn regenerate_always_refreshes_stale_generated_files() {
    let fx = setup(
        "regen-always",
        &[
            "--regen-path",
            "gen/",
            "--regen-command",
            REGEN,
            "--regen-when",
            "always",
        ],
    );
    // This change adds a source but forgets the generated file: textually clean, but stale.
    fx.branch("d", &[("src/d", "d\n"), ("tests/d", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "d"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1]]);
    assert_eq!(git(&fx.remote(), &["show", "main:gen/all.txt"]), "a\nd");
}

#[test]
fn a_change_already_on_the_base_lands_with_its_own_commit() {
    let fx = setup("ff", &[]);
    let head = fx.branch("e", &[("tests/e", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "e"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1]]);
    assert_eq!(fx.remote_main(), head);
}

#[test]
fn exact_commit_rule_reruns_the_whole_selection_after_an_ejection() {
    let fx = setup("exact", &["--exact-sha", "--confirm-reruns"]);
    fx.branch("a1", &[("tests/a1", "ok\n")]);
    fx.branch("b1", &[("tests/b1", "FAIL\n")]);
    fx.branch("c1", &[("tests/c1", "ok\n")]);
    for (pr, name) in [(1, "a1"), (2, "b1"), (3, "c1")] {
        fx.mq_ok(&["submit", &pr.to_string(), "--head", name]);
    }
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(r["ejected"][0]["pr"], 2);
    assert_eq!(landed(&r), vec![vec![1, 3]]);
    let validations = r["validations"].as_array().unwrap();
    let remainder = validations
        .iter()
        .find(|v| v["purpose"] == "remainder")
        .unwrap();
    // No carry-forward: the remainder ran the batch's whole selection (`all` here).
    assert_eq!(remainder["selection"]["kind"], "all");
    let claim = &r["landed"][0]["claim"];
    assert_eq!(claim["scope"], "complete");
    assert_eq!(claim["policy"], "all");
    // The landed tree is exactly the one validated green.
    let tip = r["landed"][0]["tip"].as_str().unwrap();
    assert!(validations
        .iter()
        .any(|v| v["commit"] == tip && v["green"] == true && v["selection"]["kind"] == "all"));
    assert_eq!(fx.remote_main(), tip);
}

/// A regeneration command that counts its invocations by appending to a file
/// outside the checkout (a file inside it would be a stray change), then does
/// the usual regeneration. Returns the command and the count file.
const REGEN_IDENTITY: &[&str] = &[
    "--regen-path",
    "gen/",
    "--regen-command",
    REGEN,
    "--identity-name",
    "queue-bot",
    "--identity-email",
    "bot@example.com",
];

fn regen_args(command: &str) -> Vec<&str> {
    vec![
        "--regen-path",
        "gen/",
        "--regen-command",
        command,
        "--identity-name",
        "queue-bot",
        "--identity-email",
        "bot@example.com",
    ]
}

#[test]
fn an_exact_change_with_current_generated_files_lands_untouched() {
    let fx = setup("regen-exact-pass", REGEN_IDENTITY);
    let head = source_change(&fx, "b");
    fx.mq_ok(&["submit", "1", "--head", "b", "--exact"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1]], "{r}");
    // The landed commit is the submitted head itself: the generator ran
    // check-only and rewrote nothing.
    assert_eq!(
        git(&fx.remote(), &["rev-parse", "main"]).trim(),
        head.trim()
    );
}

#[test]
fn an_exact_change_failing_the_generator_check_ejects_regen_check_failed() {
    // The generator refuses any tree whose sources mention BAD; the base has
    // none, so the failure is this change's, and as a check on an exact
    // change it ejects as regen-check-failed, not regen-failed.
    let check = "if grep -qr BAD src; then echo 'bad source' >&2; exit 1; fi; \
                 mkdir -p gen && cat $(ls src/* | sort) > gen/all.txt";
    let fx = setup("regen-exact-check", &regen_args(check));
    git(&fx.dev(), &["checkout", "-q", "-B", "b", "main"]);
    fx.write("src/b", "BAD\n");
    fx.write("gen/all.txt", "a\nBAD\n");
    fx.write("tests/b", "ok\n");
    fx.commit_all("b: add source");
    git(&fx.dev(), &["push", "-q", "-f", "origin", "HEAD:b"]);
    git(&fx.dev(), &["checkout", "-q", "main"]);
    fx.mq_ok(&["submit", "1", "--head", "b", "--exact"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert!(landed(&r).is_empty(), "{r}");
    assert_eq!(r["ejected"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["ejected"][0]["pr"], 1);
    assert_eq!(r["ejected"][0]["kind"], "regen-check-failed");
    assert!(
        r["ejected"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("bad source"),
        "{r}"
    );
}

fn counting_regen(fx: &Fixture) -> (String, std::path::PathBuf) {
    let count = fx.root.join("regen-count.txt");
    (
        format!("printf 'x\\n' >> {} && {REGEN}", count.display()),
        count,
    )
}

fn regen_count(count: &std::path::Path) -> usize {
    std::fs::read_to_string(count).map_or(0, |text| text.lines().count())
}

#[test]
fn regenerate_always_runs_after_each_change_so_every_prefix_is_consistent() {
    let fx = Fixture::new("regen-hoist-count");
    fx.write("src/a", "a\n");
    fx.write("gen/all.txt", "a\n");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let (command, count) = counting_regen(&fx);
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
        "--regen-path",
        "gen/",
        "--regen-command",
        &command,
        "--regen-when",
        "always",
    ]);
    // Both changes add a source but forget the generated file: textually clean, but stale.
    // `always` regenerates after each change, so the one-change prefix a culprit search might
    // probe is as consistent as the tip.
    fx.branch("b", &[("src/b", "b\n"), ("tests/b", "ok\n")]);
    fx.branch("c", &[("src/c", "c\n"), ("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1, 2]], "{r}");
    assert_eq!(regen_count(&count), 2, "once per change: {r}");
    assert_eq!(r["regenerated"].as_array().unwrap().len(), 2, "{r}");
    assert_eq!(git(&fx.remote(), &["show", "main:gen/all.txt"]), "a\nb\nc");
}

#[test]
fn conflict_repair_still_regenerates_per_conflicted_change() {
    let fx = Fixture::new("regen-hoist-conflict");
    fx.write("src/a", "a\n");
    fx.write("gen/all.txt", "a\n");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let (command, count) = counting_regen(&fx);
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
        "--regen-path",
        "gen/",
        "--regen-command",
        &command,
    ]);
    source_change(&fx, "b");
    source_change(&fx, "c");
    source_change(&fx, "d");
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    fx.mq_ok(&["submit", "3", "--head", "d"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1, 2, 3]], "{r}");
    // Every change touched the generated file (`b` changed it; `c` and `d` conflicted in it),
    // so the generator runs after each of the three.
    assert_eq!(regen_count(&count), 3, "{r}");
    assert_eq!(r["regenerated"].as_array().unwrap().len(), 3, "{r}");
    assert_eq!(
        git(&fx.remote(), &["show", "main:gen/all.txt"]),
        "a\nb\nc\nd"
    );
}

#[test]
fn culprit_search_regenerates_the_prefix_it_probes() {
    let fx = Fixture::new("regen-hoist-culprit");
    fx.write("src/a", "a\n");
    fx.write("gen/all.txt", "a\n");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let (command, count) = counting_regen(&fx);
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
        "--regen-path",
        "gen/",
        "--regen-command",
        &command,
        "--regen-when",
        "always",
        "--confirm-reruns",
    ]);
    fx.branch("b", &[("src/b", "b\n"), ("tests/b", "ok\n")]);
    fx.branch("c", &[("src/c", "c\n"), ("tests/c", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(r["ejected"][0]["pr"], 2, "{r}");
    assert_eq!(landed(&r), vec![vec![1]], "{r}");
    assert_eq!(git(&fx.remote(), &["show", "main:gen/all.txt"]), "a\nb");
    // One run for the batch tip, one for the probed first prefix (regenerated on
    // demand, then cached), and one for the rebuilt remainder's tip.
    assert_eq!(regen_count(&count), 3, "{r}");
}

#[test]
fn hoisted_always_tree_matches_per_change_regeneration() {
    let batch = setup(
        "regen-hoist-tree-batch",
        &[
            "--regen-path",
            "gen/",
            "--regen-command",
            REGEN,
            "--regen-when",
            "always",
        ],
    );
    batch.branch("b", &[("src/b", "b\n"), ("tests/b", "ok\n")]);
    batch.branch("c", &[("src/c", "c\n"), ("tests/c", "ok\n")]);
    batch.mq_ok(&["submit", "1", "--head", "b"]);
    batch.mq_ok(&["submit", "2", "--head", "c"]);
    let r = report(&batch.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1, 2]], "{r}");
    let batch_tree = git(&batch.remote(), &["rev-parse", "main^{tree}"]);

    // Per-change behaviour: land the same changes one batch at a time, so the
    // generator runs on each change's own tip.
    let single = setup(
        "regen-hoist-tree-single",
        &[
            "--regen-path",
            "gen/",
            "--regen-command",
            REGEN,
            "--regen-when",
            "always",
        ],
    );
    single.branch("b", &[("src/b", "b\n"), ("tests/b", "ok\n")]);
    single.branch("c", &[("src/c", "c\n"), ("tests/c", "ok\n")]);
    single.mq_ok(&["submit", "1", "--head", "b"]);
    let r = report(&single.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1]], "{r}");
    single.mq_ok(&["submit", "2", "--head", "c"]);
    let r = report(&single.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![2]], "{r}");
    let single_tree = git(&single.remote(), &["rev-parse", "main^{tree}"]);

    assert_eq!(batch_tree, single_tree);
    assert_eq!(
        git(&batch.remote(), &["show", "main:gen/all.txt"]),
        "a\nb\nc"
    );
}

const REGEN_ARGS: [&str; 4] = ["--regen-path", "gen/", "--regen-command", REGEN];

/// Advance `main` on the remote with one commit writing `files` (as if another
/// change landed directly), returning the new base commit.
fn advance_main(fx: &Fixture, files: &[(&str, &str)]) -> String {
    git(&fx.dev(), &["checkout", "-q", "main"]);
    git(&fx.dev(), &["pull", "-q", "origin", "main"]);
    for (path, text) in files {
        fx.write(path, text);
    }
    fx.commit_all("advance main");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    git(&fx.dev(), &["rev-parse", "HEAD"])
}

#[test]
fn only_generated_file_changes_batch_together_and_tip_is_regenerated() {
    // (a) Two changes each touching ONLY the regenerated file conflict with
    // each other on that file alone: no pairwise conflict is recorded, both
    // land in one batch. (d) The landed tip's generated file is the
    // regenerated output (from the unchanged sources), not either side's
    // version: regeneration actually ran before validation.
    let fx = setup("regen-only-pair", &REGEN_ARGS);
    fx.branch("b", &[("gen/all.txt", "stale-b\n")]);
    fx.branch("c", &[("gen/all.txt", "stale-c\n")]);
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    assert!(plan["held"].as_array().unwrap().is_empty(), "{plan}");
    assert!(plan["conflicts"].as_array().unwrap().is_empty(), "{plan}");
    assert_eq!(
        plan["batches"][0]["prs"],
        serde_json::json!([1, 2]),
        "{plan}"
    );

    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1, 2]], "{r}");
    assert_eq!(git(&fx.remote(), &["show", "main:gen/all.txt"]), "a", "{r}");
}

#[test]
fn only_generated_file_changes_batch_together_in_merge_mode() {
    // Same as above through the merge combine mode: the second merge
    // conflicts only in the generated file, is repaired by regeneration,
    // and the validated tip carries the regenerated output.
    let fx = Fixture::new("regen-only-merge");
    fx.write("src/a", "a\n");
    fx.write("gen/all.txt", "a\n");
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
        "--combine",
        "merge",
        "--regen-path",
        "gen/",
        "--regen-command",
        REGEN,
    ]);
    fx.branch("b", &[("gen/all.txt", "stale-b\n")]);
    fx.branch("c", &[("gen/all.txt", "stale-c\n")]);
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    assert!(plan["held"].as_array().unwrap().is_empty(), "{plan}");
    assert_eq!(
        plan["batches"][0]["prs"],
        serde_json::json!([1, 2]),
        "{plan}"
    );

    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1, 2]], "{r}");
    assert_eq!(git(&fx.remote(), &["show", "main:gen/all.txt"]), "a", "{r}");
}

#[test]
fn base_conflict_only_in_generated_file_is_not_held() {
    // (b) The base advanced after the change branched, rewriting the
    // generated file (with its own new source). The change's conflict with
    // the base is confined to the generated file: it is not held, and the
    // landed tip's generated file covers both sources.
    let fx = setup("regen-base", &REGEN_ARGS);
    source_change(&fx, "b");
    advance_main(&fx, &[("src/z", "z\n"), ("gen/all.txt", "a\nz\n")]);
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    assert!(plan["held"].as_array().unwrap().is_empty(), "{plan}");
    assert_eq!(plan["batches"][0]["prs"], serde_json::json!([1]), "{plan}");

    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1]], "{r}");
    assert_eq!(
        git(&fx.remote(), &["show", "main:gen/all.txt"]),
        "a\nb\nz",
        "{r}"
    );
}

#[test]
fn mixed_generated_and_real_base_conflict_stays_held() {
    // (c) A conflict spanning a generated path AND a real path is still a
    // conflict: the change is held, not batched.
    let fx = setup("regen-mixed", &REGEN_ARGS);
    // Change branched from the seed: rewrites line 1 of the real shared file
    // and the generated file (as its author saw it).
    fx.branch(
        "b",
        &[
            ("shared.txt", "B\n2\n3\n4\n5\n"),
            ("gen/all.txt", "stale-b\n"),
        ],
    );
    // The base then rewrote the same real line differently (and regenerated).
    advance_main(
        &fx,
        &[("shared.txt", "Z\n2\n3\n4\n5\n"), ("gen/all.txt", "a\nz\n")],
    );
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    assert!(plan["batches"].as_array().unwrap().is_empty(), "{plan}");
    let held = plan["held"].as_array().unwrap();
    assert_eq!(held.len(), 1, "{plan}");
    assert_eq!(held[0]["kind"], "conflicts-with-base", "{plan}");
    assert!(
        held[0]["paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "shared.txt"),
        "{plan}"
    );
}

#[test]
fn only_generated_changes_batch_with_directory_pattern_without_slash() {
    // The regenerate pattern names the generated directory without a
    // trailing slash: it still covers the files under it, so two changes
    // touching only the generated file batch together and the landed tip
    // is the regenerated output.
    let fx = setup(
        "regen-dir-noslash",
        &["--regen-path", "gen", "--regen-command", REGEN],
    );
    fx.branch("b", &[("gen/all.txt", "stale-b\n")]);
    fx.branch("c", &[("gen/all.txt", "stale-c\n")]);
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    assert!(plan["held"].as_array().unwrap().is_empty(), "{plan}");
    assert_eq!(
        plan["batches"][0]["prs"],
        serde_json::json!([1, 2]),
        "{plan}"
    );

    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1, 2]], "{r}");
    assert_eq!(git(&fx.remote(), &["show", "main:gen/all.txt"]), "a", "{r}");
}

/// Regeneration whose output orders every source line at once (`sort -u`):
/// a textual merge of two authors' generated files is not the regenerated
/// output when their edits land in different regions.
const REGEN_SORTED: &str = "cat src/* | sort -u > gen/all.txt";

/// Like `RUNNER`, but the `gen` test compares against the sorted output.
const RUNNER_SORTED: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests) gen; else set -- $MQ_TESTS; fi
rc=0
for t in "$@"; do
  o=pass
  if [ "$t" = gen ]; then
    cat src/* | sort -u | cmp -s - gen/all.txt || o=fail
  elif [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then o=fail; fi
  [ $o = fail ] && rc=1
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#;

#[test]
fn clean_generated_merge_still_regenerates_the_tip() {
    // Neither change textually conflicts when combined (their generated
    // edits are separated by an unchanged line), but the textually merged
    // generated file is stale: it is not sorted. With `when = conflict`
    // (the default) a batch that touches generated files must still have
    // its final tip regenerated before validation, in rebase mode.
    let fx = Fixture::new("regen-clean-merge");
    fx.write("src/a", "x\n");
    fx.write("src/b", "y\n");
    fx.write("src/c", "z\n");
    fx.write("gen/all.txt", "x\ny\nz\n");
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
        RUNNER_SORTED,
        "--max-batch",
        "4",
        "--regen-path",
        "gen/",
        "--regen-command",
        REGEN_SORTED,
    ]);
    // #1 rewrites the first source and regenerates as its author saw it.
    fx.branch("b", &[("src/a", "w\n"), ("gen/all.txt", "w\ny\nz\n")]);
    // #2 rewrites the last source and regenerates likewise.
    fx.branch("c", &[("src/c", "v\n"), ("gen/all.txt", "x\ny\nv\n")]);
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    assert!(plan["held"].as_array().unwrap().is_empty(), "{plan}");

    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1, 2]], "{r}");
    // The regenerated tip is the sorted union, not the textual merge
    // ("w\ny\nv").
    assert_eq!(
        git(&fx.remote(), &["show", "main:gen/all.txt"]),
        "v\nw\ny",
        "{r}"
    );
}

#[test]
fn clean_generated_merge_still_regenerates_the_tip_in_merge_mode() {
    // Same stale textual merge as `clean_generated_merge_still_regenerates_the_tip`,
    // through the merge combine mode: the merge commit's tree is regenerated
    // before validation.
    let fx = Fixture::new("regen-clean-merge-mode");
    fx.write("src/a", "x\n");
    fx.write("src/b", "y\n");
    fx.write("src/c", "z\n");
    fx.write("gen/all.txt", "x\ny\nz\n");
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
        RUNNER_SORTED,
        "--max-batch",
        "4",
        "--combine",
        "merge",
        "--regen-path",
        "gen/",
        "--regen-command",
        REGEN_SORTED,
    ]);
    fx.branch("b", &[("src/a", "w\n"), ("gen/all.txt", "w\ny\nz\n")]);
    fx.branch("c", &[("src/c", "v\n"), ("gen/all.txt", "x\ny\nv\n")]);
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1, 2]], "{r}");
    assert_eq!(
        git(&fx.remote(), &["show", "main:gen/all.txt"]),
        "v\nw\ny",
        "{r}"
    );
}

/// A numbered table of revisions under `rev/`, each naming the digest of the definitions under
/// `wire/`: `rev/table.txt` is a hand-written header line, then one `N DIGEST` row per revision,
/// and `rev/current.txt` names the newest. Each file under `tests/` is a test as in RUNNER;
/// `rev` fails unless the newest row is the current revision with the definitions' digest and
/// the rows count up by one.
const ROWS_RUNNER: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests) rev; else set -- $MQ_TESTS; fi
rc=0
for t in "$@"; do
  o=pass
  if [ "$t" = rev ]; then
    d=$(cat $(ls wire/* | sort) | cksum | cut -d' ' -f1)
    [ "$(grep '^[0-9]' rev/table.txt | tail -n 1)" = "$(cat rev/current.txt) $d" ] || o=fail
    grep '^[0-9]' rev/table.txt | awk 'NR > 1 && $1 != p + 1 { bad = 1 } { p = $1 } END { exit bad }' || o=fail
  elif [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then o=fail; fi
  [ $o = fail ] && rc=1
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#;

/// Keeps the table above in step, as a project's own command would: the header is merged three
/// ways from where the change's own edits start (both sides changing it is a real conflict),
/// the rows are the parent's, and a change that added a row of its own gets one new row after
/// the parent's newest, measured on the combined tree.
const ROWS_REGEN: &str = r#"set -e
[ -n "$MQ_HEAD" ] || exit 0
t=$(mktemp -d)
trap 'rm -rf "${t:?}"' EXIT
for side in base:$MQ_CHANGE_BASE parent:$MQ_PARENT head:$MQ_HEAD; do
  git show "${side#*:}:rev/table.txt" > "$t/${side%%:*}"
  grep -v '^[0-9]' "$t/${side%%:*}" > "$t/${side%%:*}.header" || true
done
if ! git merge-file -p "$t/parent.header" "$t/base.header" "$t/head.header" > "$t/header"; then
  echo "merge-queue-conflict: rev/table.txt: both sides changed its header"
  exit 1
fi
grep '^[0-9]' "$t/parent" > "$t/rows"
mine=$(grep '^[0-9]' "$t/head" | grep -vxF -f "$t/base" | tail -n 1 | cut -d' ' -f1)
digest=$(cat $(ls wire/* | sort) | cksum | cut -d' ' -f1)
last=$(tail -n 1 "$t/rows")
if [ -n "$mine" ] && [ "${last#* }" != "$digest" ]; then
  n=$((${last%% *} + 1))
  echo "$n $digest" >> "$t/rows"
  [ "$mine" = "$n" ] || echo "merge-queue-note: renumbered #$MQ_PR's revision $mine to $n"
fi
cat "$t/header" "$t/rows" > rev/table.txt
tail -n 1 "$t/rows" | cut -d' ' -f1 > rev/current.txt"#;

/// The digest ROWS_RUNNER and ROWS_REGEN compute for definitions concatenated to `text`.
fn digest(text: &str) -> String {
    let output = std::process::Command::new("sh")
        .args(["-c", "printf %s \"$1\" | cksum | cut -d' ' -f1", "sh", text])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// A fixture at revision 1 (definition `wire/0`), regenerating `rev/` with `regen`.
fn rows_setup(tag: &str, regen: &str) -> Fixture {
    let fx = Fixture::new(tag);
    fx.write("wire/0", "0\n");
    fx.write("tests/t0", "ok\n");
    fx.write(
        "rev/table.txt",
        &format!("# revisions\n1 {}\n", digest("0\n")),
    );
    fx.write("rev/current.txt", "1\n");
    fx.commit_all("seed");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        ROWS_RUNNER,
        "--max-batch",
        "4",
        "--regen-path",
        "rev/",
        "--regen-command",
        regen,
    ]);
    fx
}

/// Push branch `name` from `from` adding definition `wire/<name>` and the row its author
/// measured: revision `row` with the digest of the definitions on the branch, optionally
/// replacing the table's header. Returns the head.
fn wire_change(fx: &Fixture, name: &str, from: &str, row: u32, header: Option<&str>) -> String {
    git(&fx.dev(), &["checkout", "-q", "-B", name, from]);
    fx.write(&format!("wire/{name}"), &format!("{name}\n"));
    fx.write(&format!("tests/{name}"), "ok\n");
    let definitions = git(&fx.dev(), &["ls-files", "wire"])
        .lines()
        .map(|path| std::fs::read_to_string(fx.dev().join(path)).unwrap())
        .collect::<String>();
    let table = std::fs::read_to_string(fx.dev().join("rev/table.txt")).unwrap();
    let table = match header {
        Some(header) => format!("{header}\n{}", table.split_once('\n').unwrap().1),
        None => table,
    };
    fx.write(
        "rev/table.txt",
        &format!("{table}{row} {}\n", digest(&definitions)),
    );
    fx.write("rev/current.txt", &format!("{row}\n"));
    fx.commit_all(&format!("{name}: add a definition and revision {row}"));
    git(
        &fx.dev(),
        &["push", "-q", "-f", "origin", &format!("HEAD:{name}")],
    );
    let head = git(&fx.dev(), &["rev-parse", "HEAD"]);
    git(&fx.dev(), &["checkout", "-q", "main"]);
    head
}

/// The landed table, as `(revision, digest)` rows after the header, and the current revision.
fn landed_rows(fx: &Fixture) -> (Vec<(u32, String)>, String) {
    let table = git(&fx.remote(), &["show", "main:rev/table.txt"]);
    let rows = table
        .lines()
        .skip(1)
        .map(|line| {
            let (n, d) = line.split_once(' ').unwrap();
            (n.parse().unwrap(), d.to_owned())
        })
        .collect();
    (rows, git(&fx.remote(), &["show", "main:rev/current.txt"]))
}

/// The notes each change's regeneration recorded in the run report, by change.
fn regenerate_notes(r: &Value) -> Vec<(i64, Vec<String>)> {
    r["regenerated"]
        .as_array()
        .unwrap()
        .iter()
        .map(|regeneration| {
            let notes = regeneration["notes"]
                .as_array()
                .map(|notes| {
                    notes
                        .iter()
                        .map(|n| n.as_str().unwrap().to_owned())
                        .collect()
                })
                .unwrap_or_default();
            (regeneration["pr"].as_i64().unwrap(), notes)
        })
        .collect()
}

/// The `(kind, detail)` entries of change `pr`'s history.
fn history(fx: &Fixture, pr: i64) -> Vec<(String, String)> {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    let item = items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["pr"] == pr)
        .unwrap_or_else(|| panic!("no item #{pr} in {items}"));
    item["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["kind"].as_str().unwrap().to_owned(),
                e["detail"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// The details of the run's events of kind `kind`.
fn run_events(fx: &Fixture, r: &Value, kind: &str) -> Vec<String> {
    let path = fx
        .queue()
        .join(".git/merge-queue/runs")
        .join(r["run"].as_str().unwrap())
        .join("events.jsonl");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["kind"] == kind)
        .map(|event| event["detail"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn rows_two_changes_append_to_a_numbered_table_are_renumbered_in_either_landing_order() {
    // Both changes were written against revision 1 and claim revision 2. Whichever lands first
    // keeps it; the other is renumbered to 3, with the digest measured on the combined tree.
    for (first, second) in [("b", "c"), ("c", "b")] {
        let fx = rows_setup(&format!("rows-order-{first}"), ROWS_REGEN);
        wire_change(&fx, first, "main", 2, None);
        wire_change(&fx, second, "main", 2, None);
        fx.mq_ok(&["submit", "1", "--head", first]);
        fx.mq_ok(&["submit", "2", "--head", second]);
        let r = report(&fx.mq_ok(&["run", "--json"]));
        assert_eq!(landed(&r), vec![vec![1, 2]], "{r}");
        let (rows, current) = landed_rows(&fx);
        assert_eq!(
            rows,
            vec![
                (1, digest("0\n")),
                (2, digest(&format!("0\n{first}\n"))),
                (3, digest("0\nb\nc\n")),
            ],
            "{first} first"
        );
        assert_eq!(current, "3");
        let note = "renumbered #2's revision 2 to 3".to_owned();
        assert_eq!(
            regenerate_notes(&r),
            vec![(1, vec![]), (2, vec![note.clone()])],
            "{r}"
        );
        let events = run_events(&fx, &r, "regenerate");
        assert!(
            events
                .iter()
                .any(|e| e.starts_with("#2: ") && e.ends_with(&format!("; {note}"))),
            "{events:?}"
        );
        assert!(
            history(&fx, 2).contains(&("regenerate-note".to_owned(), note)),
            "{:?}",
            history(&fx, 2)
        );
        assert!(!history(&fx, 1)
            .iter()
            .any(|(kind, _)| kind == "regenerate-note"));
    }
}

#[test]
fn rows_a_change_written_before_another_landed_is_renumbered_in_a_later_run() {
    let fx = rows_setup("rows-later-run", ROWS_REGEN);
    wire_change(&fx, "c", "main", 2, None);
    wire_change(&fx, "b", "main", 2, None);
    fx.mq_ok(&["submit", "1", "--head", "c"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1]], "{r}");
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![2]], "{r}");
    let (rows, current) = landed_rows(&fx);
    assert_eq!(
        rows,
        vec![
            (1, digest("0\n")),
            (2, digest("0\nc\n")),
            (3, digest("0\nb\nc\n")),
        ]
    );
    assert_eq!(current, "3");
    assert_eq!(
        regenerate_notes(&r),
        vec![(2, vec!["renumbered #2's revision 2 to 3".to_owned()])],
        "{r}"
    );
}

#[test]
fn rows_a_stacked_change_gets_one_row_after_the_change_beneath_it() {
    // #3 is stacked on #2 and claims revision 3 after #2's 2; #1 lands first with its own 2, so
    // #2 becomes 3 and #3 becomes 4.
    let fx = rows_setup("rows-stacked", ROWS_REGEN);
    wire_change(&fx, "c", "main", 2, None);
    wire_change(&fx, "b", "main", 2, None);
    wire_change(&fx, "d", "b", 3, None);
    fx.mq_ok(&["submit", "1", "--head", "c"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "d"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1, 2, 3]], "{r}");
    let (rows, current) = landed_rows(&fx);
    assert_eq!(
        rows,
        vec![
            (1, digest("0\n")),
            (2, digest("0\nc\n")),
            (3, digest("0\nb\nc\n")),
            (4, digest("0\nb\nc\nd\n")),
        ]
    );
    assert_eq!(current, "4");
    assert_eq!(
        regenerate_notes(&r),
        vec![
            (1, vec![]),
            (2, vec!["renumbered #2's revision 2 to 3".to_owned()]),
            (3, vec!["renumbered #3's revision 3 to 4".to_owned()]),
        ],
        "{r}"
    );
}

/// Run the stacked scenario with the regenerate command logging what it was told, and check
/// it against the commits: `extra` are the run's own arguments. Returns the run report.
fn rows_environment_follows_the_change(tag: &str, extra: &[&str]) -> Value {
    let probe =
        std::env::temp_dir().join(format!("merge-queue-it-{tag}-{}.log", std::process::id()));
    let command = format!(
        "printf '%s %s %s %s\\n' \"$MQ_PR\" \"$MQ_HEAD\" \"$MQ_PARENT\" \"$MQ_CHANGE_BASE\" >> '{}'\n{ROWS_REGEN}",
        probe.display()
    );
    let fx = rows_setup(tag, &command);
    let main = fx.remote_main();
    let c = wire_change(&fx, "c", "main", 2, None);
    let b = wire_change(&fx, "b", "main", 2, None);
    let d = wire_change(&fx, "d", "b", 3, None);
    fx.mq_ok(&["submit", "1", "--head", "c"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "d"]);
    let mut args = vec!["run", "--json"];
    args.extend_from_slice(extra);
    let r = report(&fx.mq_ok(&args));
    let log = std::fs::read_to_string(&probe).unwrap();
    let _ = std::fs::remove_file(&probe);
    assert_eq!(
        landed(&r).concat(),
        vec![1, 2, 3],
        "{r}\nregenerate saw:\n{log}"
    );
    let seen: Vec<Vec<&str>> = log.lines().map(|l| l.split(' ').collect()).collect();
    assert_eq!(seen.len(), 3, "{log}");
    // Unstacked changes start where they branched from the base.
    assert_eq!(seen[0], vec!["1", c.as_str(), main.as_str(), main.as_str()]);
    assert_eq!(&seen[1][..2], &["2", b.as_str()]);
    assert_eq!(seen[1][3], main);
    // #2 was combined onto #1's tip, and #3 onto #2's copy, not onto #2's own head.
    let tree_has = |commit: &str, path: &str| {
        git(&fx.remote(), &["ls-tree", "--name-only", "-r", commit])
            .lines()
            .any(|p| p == path)
    };
    assert!(tree_has(seen[1][2], "wire/c"), "{log}");
    assert_eq!(&seen[2][..2], &["3", d.as_str()]);
    assert!(tree_has(seen[2][2], "wire/b") && seen[2][2] != b, "{log}");
    // A change stacked on a change the parent holds a copy of starts at that change's head.
    assert_eq!(seen[2][3], b, "{log}");
    r
}

#[test]
fn rows_the_command_learns_where_each_change_starts() {
    let r = rows_environment_follows_the_change("rows-env", &[]);
    assert_eq!(landed(&r), vec![vec![1, 2, 3]], "{r}");
}

#[test]
fn rows_the_command_learns_where_a_change_stacked_on_the_batch_beneath_starts() {
    // One change per batch and two batches in flight: #3's batch is built on #2's candidate
    // before #2 lands.
    let r = rows_environment_follows_the_change(
        "rows-env-window",
        &["--max-batch", "1", "--depth", "2"],
    );
    assert_eq!(landed(&r), vec![vec![1], vec![2], vec![3]], "{r}");
    assert_eq!(r["windows"], serde_json::json!([2, 2, 2]), "{r}");
}

#[test]
fn rows_a_real_conflict_the_command_reports_ejects_the_change_as_a_conflict() {
    let fx = rows_setup("rows-conflict", ROWS_REGEN);
    wire_change(&fx, "b", "main", 2, Some("# revisions after b"));
    wire_change(&fx, "c", "main", 2, Some("# revisions after c"));
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1]], "{r}");
    assert_eq!(r["ejected"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["ejected"][0]["pr"], 2);
    assert_eq!(r["ejected"][0]["kind"], "conflict");
    let reason = r["ejected"][0]["reason"].as_str().unwrap();
    assert!(
        reason.contains(
            "conflicts on rev/table.txt: both sides changed its header; rebase and resubmit"
        ),
        "{reason}"
    );
    let (rows, current) = landed_rows(&fx);
    assert_eq!(rows, vec![(1, digest("0\n")), (2, digest("0\nb\n"))]);
    assert_eq!(current, "2");
}

/// The history kinds of change `pr`.
fn history_kinds(fx: &Fixture, pr: i64) -> Vec<String> {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["pr"] == pr)
        .map(|item| {
            item["history"]
                .as_array()
                .unwrap()
                .iter()
                .map(|event| event["kind"].as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn rows_a_conflict_with_a_batch_in_flight_sets_the_change_aside_until_it_meets_the_base() {
    // One change per batch, two in flight: #2's batch is built on #1's candidate, where its
    // header conflicts. #1 has not landed yet, so #2 is set aside, not ejected; the next run
    // builds it on the base, with #1 landed, where the conflict is real.
    let fx = rows_setup("rows-conflict-in-flight", ROWS_REGEN);
    wire_change(&fx, "b", "main", 2, Some("# revisions after b"));
    wire_change(&fx, "c", "main", 2, Some("# revisions after c"));
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let args = ["run", "--json", "--max-batch", "1", "--depth", "2"];
    let r = report(&fx.mq_ok(&args));
    assert_eq!(landed(&r), vec![vec![1]], "{r}");
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(r["requeued"], serde_json::json!([2]), "{r}");
    assert!(
        history_kinds(&fx, 2).contains(&"set-aside".to_owned()),
        "{:?}",
        history_kinds(&fx, 2)
    );
    let r = report(&fx.mq_ok(&args));
    assert!(landed(&r).is_empty(), "{r}");
    assert_eq!(r["ejected"][0]["pr"], 2, "{r}");
    assert_eq!(r["ejected"][0]["kind"], "conflict", "{r}");
    let reason = r["ejected"][0]["reason"].as_str().unwrap();
    assert!(
        reason.contains(
            "conflicts on rev/table.txt: both sides changed its header; rebase and resubmit"
        ),
        "{reason}"
    );
}

#[test]
fn rows_a_change_set_aside_behind_a_red_batch_lands_once_that_batch_is_ejected() {
    // #2 fails to combine only onto #1's candidate, and #1 is red: ejecting #2 for it would
    // have thrown away a change that combines with the base.
    let fx = rows_setup("rows-set-aside-red", ROWS_REGEN);
    wire_change(&fx, "b", "main", 2, Some("# revisions after b"));
    fx.branch_from("b", "b", &[("tests/zz", "FAIL\n")]);
    wire_change(&fx, "c", "main", 2, Some("# revisions after c"));
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let args = ["run", "--json", "--max-batch", "1", "--depth", "2"];
    let r = report(&fx.mq_ok(&args));
    assert!(landed(&r).is_empty(), "{r}");
    assert_eq!(r["ejected"][0]["pr"], 1, "{r}");
    assert_eq!(r["ejected"].as_array().unwrap().len(), 1, "{r}");
    let r = report(&fx.mq_ok(&args));
    assert_eq!(landed(&r), vec![vec![2]], "{r}");
    let (rows, current) = landed_rows(&fx);
    assert_eq!(rows, vec![(1, digest("0\n")), (2, digest("0\nc\n"))]);
    assert_eq!(current, "2");
}
