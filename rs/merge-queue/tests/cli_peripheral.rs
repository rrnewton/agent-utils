//! The peripheral lane: a consumer hook marks changes peripheral; they batch wide, never
//! mix with binary-touching changes, and validate with `$MQ_PERIPHERAL=1` so the select
//! command can answer the focused selection. The hook's answer is cached per (base, head).

mod support;

use serde_json::Value;
use support::{git, Fixture};

fn plan(fx: &Fixture) -> Value {
    serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap()
}

#[test]
fn peripheral_changes_batch_wide_with_a_focused_selection() {
    let fx = Fixture::new("peripheral");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    // The validator records the flag it saw and passes everything it is asked for; the
    // select command answers only `t0` for a peripheral batch, `all` otherwise.
    let validator = format!(
        r#"echo "peripheral=${{MQ_PERIPHERAL:-}} tests=$MQ_TESTS" >> '{root}/seen'; for t in $MQ_TESTS; do printf '{{"id":"%s","outcome":"pass"}}\n' "$t" >> "$MQ_RESULTS"; done; exit 0"#,
        root = fx.root.display()
    );
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &validator,
        "--select",
        "if [ \"$MQ_PERIPHERAL\" = 1 ]; then echo t0; else echo all; fi",
        "--peripheral-command",
        "if [ -f peripheral.txt ]; then echo peripheral; else echo binary-touching; fi",
        "--max-batch",
        "1",
    ]);
    fx.branch("a", &[("peripheral.txt", "x\n"), ("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("peripheral.txt", "x\n"), ("docs/b.md", "x\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.branch("c", &[("src/c.rs", "fn c() {}\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);

    let planned = plan(&fx);
    let batches: Vec<Vec<i64>> = planned["batches"]
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
        .collect();
    assert_eq!(batches, vec![vec![1, 2], vec![3]], "{planned}");
    let items: Vec<&Value> = planned["items"].as_array().unwrap().iter().collect();
    assert_eq!(items[0]["peripheral"], true, "{planned}");
    assert_eq!(items[0]["peripheral_source"], "hook", "{planned}");
    assert!(items[2].get("peripheral").is_none(), "{planned}");
    // A second plan at the same base and heads is answered by the cache.
    let planned = plan(&fx);
    let items: Vec<&Value> = planned["items"].as_array().unwrap().iter().collect();
    assert_eq!(items[0]["peripheral_source"], "cache", "{planned}");

    let out = fx.mq(&["run"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let seen = std::fs::read_to_string(fx.root.join("seen")).unwrap();
    let peripheral_runs: Vec<&str> = seen
        .lines()
        .filter(|line| line.contains("peripheral=1"))
        .collect();
    assert_eq!(peripheral_runs.len(), 1, "{seen}");
    assert!(peripheral_runs[0].contains("tests=t0"), "{seen}");
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    for item in items.as_array().unwrap() {
        assert_eq!(item["status"], "landed", "{item}");
    }
}
