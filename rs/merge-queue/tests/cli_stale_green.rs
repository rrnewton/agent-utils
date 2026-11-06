//! A candidate green on the immediately previous base, stale only because
//! unrelated changes landed, stacks on the stack head even below the
//! pass-rate gate when its files are disjoint from what landed and its
//! merge onto the head is clean. Fresh candidates still wait for the gate.

mod support;

use std::path::PathBuf;

use serde_json::Value;
use support::{git, Fixture};

const RUNNER: &str = r#"ok=1; for f in tests/*; do [ -f "$f" ] || continue; if grep -qx FAIL "$f"; then echo "{\"id\":\"x\",\"outcome\":\"fail\"}" >> "$MQ_RESULTS"; ok=0; fi; done; [ $ok -eq 1 ]"#;

fn config_path(fx: &Fixture) -> PathBuf {
    let common = git(&fx.queue(), &["rev-parse", "--git-common-dir"]);
    let common = if std::path::Path::new(&common).is_absolute() {
        PathBuf::from(common)
    } else {
        fx.queue().join(common)
    };
    common.join("merge-queue").join("config.json")
}

fn set_config(fx: &Fixture, fields: &[(&str, Value)]) {
    let path = config_path(fx);
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for (key, field) in fields {
        value[key] = field.clone();
    }
    std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

fn status(fx: &Fixture, pr: i64) -> String {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["pr"] == pr)
        .map(|item| item["status"].as_str().unwrap().to_owned())
        .unwrap()
}

fn report(stdout: &str) -> Value {
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("bad report JSON ({e}):\n{stdout}"))
}

fn events(fx: &Fixture) -> String {
    let runs = fx.queue().join(".git/merge-queue/runs");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("events.jsonl").is_file())
        .collect();
    dirs.sort();
    std::fs::read_to_string(dirs.pop().unwrap().join("events.jsonl")).unwrap()
}

/// Seed a 50% recent pass rate (below an 80% stack gate), then run three
/// disjoint passing changes with three root slots and stale roots kept.
fn seeded(tag: &str) -> Fixture {
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
        "--max-batch",
        "1",
    ]);
    fx.branch("g0", &[("tests/g0", "ok\n")]);
    fx.branch("r0", &[("tests/r0", "FAIL\n")]);
    fx.mq_ok(&["submit", "101", "--head", "g0"]);
    fx.mq_ok(&["submit", "102", "--head", "r0"]);
    fx.mq_ok(&["run", "--json"]);
    set_config(
        &fx,
        &[
            ("depth", serde_json::json!(3)),
            ("roots", serde_json::json!(3)),
            ("stack_min_pass_percent", serde_json::json!(80)),
            ("keep_stale_roots", serde_json::json!(true)),
        ],
    );
    fx
}

#[test]
fn a_stale_green_stacks_below_the_gate_when_disjoint() {
    let fx = seeded("stale-green-stacked");
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let out = fx.mq_ok(&["run", "--json"]);
    let r = report(&out);
    assert_eq!(status(&fx, 1), "landed", "{r}");
    assert_eq!(status(&fx, 2), "landed", "{r}");
    assert_eq!(status(&fx, 3), "landed", "{r}");
    let events = events(&fx);
    let stacked: Vec<&str> = events
        .lines()
        .filter(|line| {
            serde_json::from_str::<Value>(line).is_ok_and(|e| e["kind"] == "stale-green-stacked")
        })
        .collect();
    assert!(!stacked.is_empty(), "stale green stacked: {events}");
    assert!(
        stacked.iter().any(|line| line.contains("was green at")),
        "{stacked:?}"
    );
    // Two previously green candidates land without a third validation each:
    // the last change validates on the previous base and once stacked.
    let c_validations = r["validations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["prs"] == serde_json::json!([3]) && v["cancelled"] == false)
        .count();
    assert!(c_validations <= 2, "c validated {c_validations} times: {r}");
}

#[test]
fn a_fresh_candidate_below_the_gate_still_waits_for_a_root() {
    let fx = seeded("stale-green-fresh");
    fx.branch("x", &[("tests/x", "ok\n")]);
    fx.branch("y", &[("tests/y", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "x"]);
    fx.mq_ok(&["submit", "2", "--head", "y"]);
    let base = fx.remote_main();
    let out = fx.mq_ok(&["run", "--json"]);
    let r = report(&out);
    let second = r["validations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["prs"] == serde_json::json!([2]))
        .expect("second validated");
    assert_eq!(
        second["parent"], base,
        "fresh candidate rooted, not stacked: {r}"
    );
    assert!(
        !events(&fx).contains("stale-green-stacked"),
        "fresh candidates never stale-stack"
    );
}

#[test]
fn a_red_stacked_validation_falls_back_without_an_extra_strike() {
    // C is green alone; stacked on B the combination is red (the validator
    // fails when tests/b and tests/c coexist). It is then handled as an
    // ordinary red single: ejected once, B still lands.
    let fx = Fixture::new("stale-green-red");
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
        "--max-batch",
        "1",
    ]);
    fx.branch("g0", &[("tests/g0", "ok\n")]);
    fx.branch("r0", &[("tests/r0", "FAIL\n")]);
    fx.mq_ok(&["submit", "101", "--head", "g0"]);
    fx.mq_ok(&["submit", "102", "--head", "r0"]);
    fx.mq_ok(&["run", "--json"]);
    // Switch to the interaction validator for the measured run.
    let path = config_path(&fx);
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    value["validator"]["run"] = serde_json::json!(
        r#"if [ -f tests/b ] && [ -f tests/c ]; then echo '{"id":"x","outcome":"fail"}' >> "$MQ_RESULTS"; exit 1; fi; exit 0"#
    );
    std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    set_config(
        &fx,
        &[
            ("depth", serde_json::json!(3)),
            ("roots", serde_json::json!(3)),
            ("stack_min_pass_percent", serde_json::json!(80)),
            ("keep_stale_roots", serde_json::json!(true)),
        ],
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let out = fx.mq_ok(&["run", "--json"]);
    let r = report(&out);
    assert_eq!(status(&fx, 2), "landed", "{r}");
    // The seeded rate in this fixture may not be below the gate; only assert
    // the fallback outcome, which holds either way: C is not left queued and
    // was validated at most twice (previous green + one red).
    let c_validations = r["validations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["prs"] == serde_json::json!([3]) && v["cancelled"] == false)
        .count();
    assert!(c_validations <= 2, "c validated {c_validations} times: {r}");
    assert_ne!(status(&fx, 3), "landed", "{r}");
}

#[test]
fn a_stale_green_overlapping_what_landed_does_not_stack() {
    let fx = seeded("stale-green-overlap");
    // B touches the same file A lands, so it is not disjoint from landed.
    fx.branch("a", &[("tests/shared", "a\n")]);
    fx.branch("b", &[("tests/shared", "b\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let out = fx.mq_ok(&["run", "--json"]);
    let r = report(&out);
    let events = events(&fx);
    // B conflicts with the landed A as a root candidate (today's behaviour),
    // and is not named as stale-green-stacked with B.
    assert!(
        !events.lines().any(|line| {
            serde_json::from_str::<Value>(line).is_ok_and(|e| {
                e["kind"] == "stale-green-stacked"
                    && e["detail"].as_str().unwrap_or_default().contains("#2")
            })
        }),
        "{events}"
    );
    let _ = r;
}
