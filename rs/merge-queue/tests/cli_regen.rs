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
fn generated_file_that_is_also_generator_source_regenerates_from_base_side() {
    // The regenerated file is also an input the generator is built from:
    // conflict markers in it make the generator fail before it can rewrite
    // anything. Regeneration must start the generated paths from the base
    // side (the tree before the change combined) and let the generator
    // rewrite them. The script below fails on markers exactly like a build
    // would.
    const COMMAND: &str = "if grep -q '<<<<<<<' gen/self.txt; then echo marked; exit 101; fi; cat $(ls src/* | sort) > gen/self.txt; cat $(ls src/* | sort) > gen/all.txt";
    let fx = Fixture::new("regen-self-source");
    fx.write("src/a", "a\n");
    fx.write("gen/self.txt", "a\n");
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
        "--regen-path",
        "gen/",
        "--regen-command",
        COMMAND,
        "--identity-name",
        "queue-bot",
        "--identity-email",
        "bot@example.com",
    ]);
    for (pr, name) in [(1, "b"), (2, "c")] {
        git(&fx.dev(), &["checkout", "-q", "-B", name, "main"]);
        fx.write(&format!("src/{name}"), &format!("{name}\n"));
        fx.write("gen/self.txt", &format!("a\n{name}\n"));
        fx.write(&format!("tests/{name}"), "ok\n");
        fx.commit_all(&format!("{name}: add source"));
        git(
            &fx.dev(),
            &["push", "-q", "-f", "origin", &format!("HEAD:{name}")],
        );
        git(&fx.dev(), &["checkout", "-q", "main"]);
        fx.mq_ok(&["submit", &pr.to_string(), "--head", name]);
    }
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1, 2]], "{r}");
    let landed_tree = git(&fx.dev(), &["show", "origin/main:gen/self.txt"]);
    assert_eq!(landed_tree.trim(), "a\nb\nc");
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
    let fx = setup("exact", &["--exact-sha"]);
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
