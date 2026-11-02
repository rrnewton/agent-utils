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
    let regenerated = r["regenerated"].as_array().unwrap();
    assert_eq!(regenerated.len(), 1, "{r}");
    assert_eq!(regenerated[0]["pr"], 2);
    assert_eq!(
        regenerated[0]["rewritten"],
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
fn regeneration_that_touches_other_files_ejects_the_change() {
    let bad = "cat $(ls src/* | sort) > gen/all.txt && echo oops > tests/t0";
    let fx = setup(
        "regen-bad",
        &["--regen-path", "gen/", "--regen-command", bad],
    );
    source_change(&fx, "b");
    source_change(&fx, "c");
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1]]);
    assert_eq!(r["ejected"][0]["pr"], 2);
    assert_eq!(r["ejected"][0]["kind"], "regeneration");
    assert!(
        r["ejected"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("not generated: tests/t0"),
        "{r}"
    );
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
