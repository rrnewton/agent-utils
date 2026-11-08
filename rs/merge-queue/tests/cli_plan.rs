//! End-to-end `init`, `submit`, `list` and `plan` against a local bare remote.

mod support;

use serde_json::Value;
use support::{git, Fixture};

#[test]
fn plan_orders_batches_and_holds_conflicts() {
    let fx = Fixture::new("plan");
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--base",
        "main",
        "--max-batch",
        "2",
    ]);

    let a = fx.branch("a", &[("shared.txt", "A\n2\n3\n4\n5\n")]);
    fx.branch("b", &[("shared.txt", "B\n2\n3\n4\n5\n")]);
    fx.branch("c", &[("c.txt", "c\n")]);
    fx.branch("d", &[("d.txt", "d\n")]);
    fx.branch_from("e", "a", &[("e.txt", "e\n")]);

    fx.mq_ok(&[
        "submit",
        "1",
        "--head",
        "a",
        "--sha",
        &a[..12],
        "--priority",
        "10",
    ]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&[
        "submit",
        "3",
        "--head",
        "c",
        "--tested-with",
        "unit::c_case,lint",
        "--policy",
        "path-routing",
        "--by",
        "agent-c",
    ]);
    fx.mq_ok(&["submit", "4", "--head", "d", "--solo"]);
    fx.mq_ok(&["submit", "5", "--head", "e"]);

    // A submission naming a head the branch is not at is refused.
    let refused = fx.mq(&["submit", "6", "--head", "c", "--sha", "deadbeef"]);
    assert!(!refused.status.success());

    let list = fx.mq_ok(&["list"]);
    assert!(list.contains("#3") && list.contains("focused"), "{list}");

    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    assert_eq!(plan["schema"], "merge-queue-plan/v1");
    assert_eq!(plan["base_sha"], fx.remote_main());
    let batches: Vec<Vec<i64>> = plan["batches"]
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
    // #1 has top priority; #5 is stacked on it; #4 lands alone; #2 conflicts with #1 (and #5).
    assert_eq!(batches, vec![vec![1, 3], vec![4], vec![5]]);
    let held = plan["held"].as_array().unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0]["pr"], 2);
    assert_eq!(held[0]["kind"], "conflicts-with-queued");
    assert_eq!(plan["dependencies"], serde_json::json!([[1, 5]]));

    let text = fx.mq_ok(&["plan"]);
    assert!(text.contains("batch 1: #1"), "{text}");
    assert!(text.contains("#2 conflicts-with-queued"), "{text}");
    assert!(text.contains("Conflict clusters:"), "{text}");

    // Moving a head after submission holds the change until it is resubmitted.
    fx.branch("c", &[("c.txt", "c2\n")]);
    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    let moved: Vec<&Value> = plan["held"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|held| held["kind"] == "head-moved")
        .collect();
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0]["pr"], 3);

    fx.mq_ok(&["withdraw", "2"]);
    let all = fx.mq_ok(&["list", "--all"]);
    assert!(all.contains("withdrawn"), "{all}");
}

/// One stacked-on-landed-rebased scenario, planned under the given combine
/// mode: `a` appends a line, `b` appends another on top of it, then main
/// lands `a`'s patch as a rebased commit (same patch, new parent) before
/// `b` is submitted.
fn stacked_on_landed_rebased_plan(tag: &str, combine: Option<&str>) -> Value {
    let fx = Fixture::new(tag);
    let mut args = vec!["init", "--remote", "origin", "--base", "main"];
    if let Some(combine) = combine {
        args.extend(["--combine", combine]);
    }
    fx.mq_ok(&args);
    fx.branch("a", &[("shared.txt", "1\n2\n3\n4\n5\nA\n")]);
    fx.branch_from("b", "a", &[("shared.txt", "1\n2\n3\n4\n5\nA\nB\n")]);
    fx.write("other.txt", "o\n");
    fx.commit_all("advance main");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.write("shared.txt", "1\n2\n3\n4\n5\nA\n");
    fx.commit_all("land a rebased");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap()
}

#[test]
fn stacked_on_landed_rebased_is_held_only_under_merge_combine() {
    // #1053: under rebase combine the replay drops the landed patch, so the
    // stacked change plans clean; under merge combine merge-tree holds it.
    let plan = stacked_on_landed_rebased_plan("probe-rebase", None);
    assert!(plan["held"].as_array().unwrap().is_empty(), "{plan}");
    assert_eq!(plan["batches"][0]["prs"], serde_json::json!([2]), "{plan}");

    let plan = stacked_on_landed_rebased_plan("probe-merge", Some("merge"));
    assert!(plan["batches"].as_array().unwrap().is_empty(), "{plan}");
    let held = plan["held"].as_array().unwrap();
    assert_eq!(held.len(), 1, "{plan}");
    assert_eq!(held[0]["pr"], 2, "{plan}");
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
fn state_lives_in_the_git_common_dir_and_guides_print() {
    let fx = Fixture::new("state");
    fx.mq_ok(&["init"]);
    assert!(fx.queue().join(".git/merge-queue/config.json").is_file());
    assert!(fx.queue().join(".git/merge-queue/items").is_dir());
    let again = fx.mq(&["init"]);
    assert!(!again.status.success());
    assert!(fx.mq_ok(&["quickstart"]).contains("merge-queue"));
    assert!(fx.mq_ok(&["userguide"]).contains("merge-queue"));
    assert!(fx.mq_ok(&["--help"]).contains("quickstart"));
}
