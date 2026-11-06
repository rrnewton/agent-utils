//! `generated_paths` top-level config, the `regenerated-conflict` event, and
//! `merge-queue compose --check`: conflicts confined to declared generated
//! paths are settled by regeneration; anything else still needs a human.

mod support;

use serde_json::Value;
use support::{git, Fixture};

/// Each file under `tests/` is a test that fails when it contains FAIL; `gen`
/// fails when `gen/all.txt` is not the sorted concatenation of `src/*`.
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

/// A change adding source `name` and the generated file as its author saw it.
fn source_change(fx: &Fixture, name: &str) {
    git(&fx.dev(), &["checkout", "-q", "-B", name, "main"]);
    fx.write(&format!("src/{name}"), &format!("{name}\n"));
    fx.write("gen/all.txt", &format!("a\n{name}\n"));
    fx.write(&format!("tests/{name}"), "ok\n");
    fx.commit_all(&format!("{name}: add source"));
    git(
        &fx.dev(),
        &["push", "-q", "-f", "origin", &format!("HEAD:{name}")],
    );
    git(&fx.dev(), &["checkout", "-q", "main"]);
}

fn report(stdout: &str) -> Value {
    serde_json::from_str(stdout).unwrap()
}

fn landed(r: &Value) -> Vec<Vec<i64>> {
    r["landed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|batch| {
            batch["prs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pr| pr.as_i64().unwrap())
                .collect()
        })
        .collect()
}

#[test]
fn generated_paths_key_regenerates_and_records_event() {
    // No --regen-path: the generated set comes from the top-level
    // `generated_paths` key in config.json alone.
    let fx = setup("regen-key", &["--regen-command", REGEN]);
    let config_path = fx.queue().join(".git/merge-queue/config.json");
    let text = std::fs::read_to_string(&config_path).unwrap();
    let mut config: Value = serde_json::from_str(&text).unwrap();
    config["generated_paths"] = serde_json::json!(["gen/"]);
    std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    source_change(&fx, "b");
    source_change(&fx, "c");
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    fx.mq_ok(&["submit", "2", "--head", "c"]);

    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed(&r), vec![vec![1, 2]], "{r}");
    // The pushed tree is the validated (regenerated) tree.
    let generated = git(&fx.remote(), &["show", "main:gen/all.txt"]);
    assert_eq!(generated, "a\nb\nc");
    let regenerated = r["regenerated"].as_array().unwrap();
    let tip = regenerated.last().unwrap()["commit"].as_str().unwrap();
    let pushed_tree = git(&fx.remote(), &["rev-parse", "main^{tree}"]);
    let validated_tree = git(&fx.remote(), &["rev-parse", &format!("{tip}^{{tree}}")]);
    assert_eq!(pushed_tree, validated_tree, "{r}");
    // A regenerated-conflict event names the generated paths.
    let events_path = fx
        .queue()
        .join(".git/merge-queue/runs")
        .join(r["run"].as_str().unwrap())
        .join("events.jsonl");
    let events = std::fs::read_to_string(&events_path).unwrap();
    assert!(
        events.lines().any(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            event["kind"] == "regenerated-conflict"
                && event["detail"].as_str().unwrap().contains("gen/all.txt")
        }),
        "{events}"
    );
}

#[test]
fn compose_check_partitions_generated_and_human_conflicts() {
    let fx = setup(
        "compose-check",
        &["--regen-path", "gen/", "--regen-command", REGEN],
    );
    source_change(&fx, "b");
    fx.mq_ok(&["submit", "1", "--head", "b"]);
    // The base moves with its own generated-file rewrite, so #1 now conflicts
    // only in the generated file.
    fx.write("gen/all.txt", "a\nz\n");
    fx.commit_all("base: regenerate");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);

    let out = fx.mq_ok(&["compose", "--check", "1"]);
    assert!(out.contains("would auto-regenerate"), "{out}");
    assert!(out.contains("gen/all.txt"), "{out}");
    let json: Value =
        serde_json::from_str(&fx.mq_ok(&["compose", "--check", "1", "--json"])).unwrap();
    assert_eq!(json["auto_regenerated"], serde_json::json!(["gen/all.txt"]));
    assert_eq!(json["needs_human"], serde_json::json!([]));

    // A change that also conflicts in a hand-written file needs a human, and
    // an exact change is never rewritten even for generated paths.
    git(&fx.dev(), &["checkout", "-q", "-B", "mix", "main~1"]);
    fx.write("gen/all.txt", "a\nmix\n");
    fx.write("shared.txt", "9\n2\n3\n4\n5\n");
    fx.commit_all("mix: change");
    git(&fx.dev(), &["push", "-q", "-f", "origin", "HEAD:mix"]);
    git(&fx.dev(), &["checkout", "-q", "main"]);
    // The base rewrites the same hand-written line differently, so `mix`
    // conflicts in shared.txt as well as in the generated file.
    fx.write("shared.txt", "8\n2\n3\n4\n5\n");
    fx.commit_all("base: shared");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.mq_ok(&["submit", "2", "--head", "mix"]);
    let mixed: Value =
        serde_json::from_str(&fx.mq_ok(&["compose", "--check", "2", "--json"])).unwrap();
    assert!(
        mixed["needs_human"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "shared.txt"),
        "{mixed}"
    );

    fx.mq_ok(&["submit", "3", "--head", "b", "--exact"]);
    let exact: Value =
        serde_json::from_str(&fx.mq_ok(&["compose", "--check", "3", "--json"])).unwrap();
    assert_eq!(exact["exact"], true);
    assert_eq!(exact["auto_regenerated"], serde_json::json!([]));
    assert!(
        exact["needs_human"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "gen/all.txt"),
        "{exact}"
    );
}

#[test]
fn mixed_conflict_is_refused_not_regenerated() {
    let fx = setup(
        "regen-mixed",
        &["--regen-path", "gen/", "--regen-command", REGEN],
    );
    git(&fx.dev(), &["checkout", "-q", "-B", "mix", "main"]);
    fx.write("gen/all.txt", "a\nmix\n");
    fx.write("shared.txt", "9\n2\n3\n4\n5\n");
    fx.commit_all("mix: change");
    git(&fx.dev(), &["push", "-q", "-f", "origin", "HEAD:mix"]);
    git(&fx.dev(), &["checkout", "-q", "main"]);
    // The base rewrites the same hand-written lines differently.
    fx.write("shared.txt", "8\n2\n3\n4\n5\n");
    fx.commit_all("base: shared");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.mq_ok(&["submit", "1", "--head", "mix"]);

    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert!(landed(&r).is_empty(), "{r}");
    // Held against the base on the hand-written path: never landed, and the
    // generated path alone does not rescue it.
    let held = r["plan"]["held"].as_array().unwrap();
    assert_eq!(held.len(), 1, "{r}");
    assert!(
        held[0]["reason"].as_str().unwrap().contains("shared.txt"),
        "{r}"
    );
}
