//! Ready sources (`sync`), the API request budget, and closing landed pull requests, against a
//! local bare remote and a stub `gh`.

mod support;

use serde_json::Value;
use support::{git, Fixture};

const STUB_GH: &str = r#"#!/bin/sh
echo "$*" >> "$GH_LOG"
case "$1 $2" in
  "pr list") cat "$GH_LIST" ;;
  "pr close") if [ -e "$GH_FAIL_CLOSE" ]; then rm -f "$GH_FAIL_CLOSE"; echo "close failed" >&2; exit 1; fi ;;
esac
"#;

const RUNNER: &str = r#"rc=0; for t in $(ls tests); do o=pass; grep -q FAIL "tests/$t" && { o=fail; rc=1; }; printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"; done; exit $rc"#;

struct Gh {
    stub: String,
    log: String,
    list: String,
    fail_close: String,
}

fn stub(fx: &Fixture) -> Gh {
    let stub = fx.root.join("gh-stub");
    std::fs::write(&stub, STUB_GH).unwrap();
    std::process::Command::new("chmod")
        .args(["+x", stub.to_str().unwrap()])
        .status()
        .unwrap();
    Gh {
        stub: stub.display().to_string(),
        log: fx.root.join("gh.log").display().to_string(),
        list: fx.root.join("gh-list.json").display().to_string(),
        fail_close: fx.root.join("gh-fail-close").display().to_string(),
    }
}

/// Push a pull-request head as `refs/pull/N/head` with one commit writing `files`.
fn pull_request(fx: &Fixture, number: i64, files: &[(&str, &str)]) -> String {
    let name = format!("pr{number}");
    let head = fx.branch(&name, files);
    git(
        &fx.dev(),
        &[
            "push",
            "-q",
            "origin",
            &format!("{head}:refs/pull/{number}/head"),
        ],
    );
    head
}

fn listing(entries: &[(i64, &str, bool, &[&str])]) -> String {
    let prs: Vec<Value> = entries
        .iter()
        .map(|(number, head, draft, labels)| {
            serde_json::json!({
                "number": number, "headRefOid": head, "isDraft": draft, "title": format!("change {number}"),
                "author": {"login": "lane"},
                "labels": labels.iter().map(|l| serde_json::json!({"name": l})).collect::<Vec<_>>()
            })
        })
        .collect();
    serde_json::to_string(&prs).unwrap()
}

fn env(gh: &Gh) -> Vec<(&str, &str)> {
    vec![
        ("GH_LOG", gh.log.as_str()),
        ("GH_LIST", gh.list.as_str()),
        ("GH_FAIL_CLOSE", gh.fail_close.as_str()),
    ]
}

fn sync_json(fx: &Fixture, gh: &Gh, args: &[&str]) -> Value {
    let mut full = vec!["sync"];
    full.extend_from_slice(args);
    full.push("--json");
    let out = fx.mq_env(&full, &env(gh));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap()
}

fn items(fx: &Fixture) -> Value {
    serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap()
}

fn item(items: &Value, pr: i64) -> &Value {
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["pr"] == pr)
        .unwrap()
}

#[test]
fn label_sync_submits_withdraws_and_reads_priority_labels() {
    let fx = Fixture::new("sync-label");
    let gh = stub(&fx);
    fx.mq_ok(&["init", "--github-repo", "owner/name", "--gh", &gh.stub]);
    let h1 = pull_request(&fx, 1, &[("one", "1\n")]);
    let h2 = pull_request(&fx, 2, &[("two", "2\n")]);
    let h3 = pull_request(&fx, 3, &[("three", "3\n")]);
    fx.mq_ok(&["submit", "9", "--head", "pr1"]); // a direct submission no source may withdraw
    std::fs::write(
        &gh.list,
        listing(&[
            (1, &h1, false, &["ready", "p10"]),
            (2, &h2, false, &["ready"]),
            (3, &h3, true, &["ready"]),
        ]),
    )
    .unwrap();
    let r = sync_json(&fx, &gh, &["--label", "ready", "--limit", "100"]);
    assert_eq!(r["submitted"], serde_json::json!([1, 2]));
    assert_eq!(r["listed"], 2, "drafts are not ready");
    let all = items(&fx);
    assert_eq!(item(&all, 1)["priority"], 10);
    assert_eq!(item(&all, 1)["head_ref"], "refs/pull/1/head");
    assert_eq!(item(&all, 1)["source"], "label:ready");
    // Unchanged heads are not resubmitted; a dropped label withdraws only what the label added.
    std::fs::write(&gh.list, listing(&[(1, &h1, false, &["ready", "p10"])])).unwrap();
    let r = sync_json(&fx, &gh, &["--label", "ready", "--limit", "100"]);
    assert_eq!(r["unchanged"], serde_json::json!([1]));
    assert_eq!(r["withdrawn"], serde_json::json!([2]));
    let all = items(&fx);
    assert_eq!(item(&all, 2)["status"], "withdrawn");
    assert_eq!(item(&all, 9)["status"], "queued");
    // One batched list call per sync, charged one request per hundred listed.
    let log = std::fs::read_to_string(&gh.log).unwrap();
    assert_eq!(log.lines().count(), 2, "{log}");
    assert!(log.contains("pr list --repo owner/name --base main --state open --limit 100"));
    assert!(log.contains("--label ready"));
    assert_eq!(r["api_remaining"], 58);
}

#[test]
fn the_request_budget_refuses_calls_past_its_hourly_limit() {
    let fx = Fixture::new("sync-budget");
    let gh = stub(&fx);
    fx.mq_ok(&[
        "init",
        "--github-repo",
        "o/n",
        "--gh",
        &gh.stub,
        "--api-per-hour",
        "1",
    ]);
    std::fs::write(&gh.list, "[]").unwrap();
    sync_json(&fx, &gh, &["--all-open", "--limit", "100"]);
    let out = fx.mq_env(&["sync", "--all-open", "--limit", "100"], &env(&gh));
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("API request budget exhausted: 1 of 1"),
        "{stderr}"
    );
    // The refused call never reached gh.
    assert_eq!(std::fs::read_to_string(&gh.log).unwrap().lines().count(), 1);
}

#[test]
fn a_ready_file_needs_no_api_call() {
    let fx = Fixture::new("sync-file");
    fx.mq_ok(&["init"]);
    fx.branch("a", &[("a", "a\n")]);
    fx.branch("b", &[("b", "b\n")]);
    let file = fx.root.join("ready.txt");
    std::fs::write(
        &file,
        "# lanes append here\n1 head=a priority=20\n2 head=b after=1 solo\n",
    )
    .unwrap();
    let out = fx.mq_ok(&["sync", "--file", file.to_str().unwrap(), "--json"]);
    let r: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(r["submitted"], serde_json::json!([1, 2]));
    assert!(r.get("api_remaining").is_none());
    let all = items(&fx);
    assert_eq!(item(&all, 1)["priority"], 20);
    assert_eq!(item(&all, 2)["after"], serde_json::json!([1]));
    assert_eq!(item(&all, 2)["solo"], true);
}

#[test]
fn landed_pull_requests_are_closed_and_a_failed_close_is_retried_by_sync() {
    let fx = Fixture::new("sync-close");
    let gh = stub(&fx);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.mq_ok(&[
        "init",
        "--github-repo",
        "o/n",
        "--gh",
        &gh.stub,
        "--close-prs",
        "--validator",
        "command",
        "--validator-run",
        RUNNER,
        "--max-batch",
        "1",
    ]);
    let h1 = pull_request(&fx, 1, &[("tests/a", "ok\n")]);
    let h2 = pull_request(&fx, 2, &[("tests/b", "ok\n")]);
    std::fs::write(
        &gh.list,
        listing(&[(1, &h1, false, &[]), (2, &h2, false, &[])]),
    )
    .unwrap();
    sync_json(&fx, &gh, &["--all-open", "--limit", "100"]);
    // The first close fails (say, a transient error): it is postponed, not lost.
    std::fs::write(&gh.fail_close, "").unwrap();
    let out = fx.mq_env(&["run", "--json"], &env(&gh));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let all = items(&fx);
    assert_eq!(item(&all, 1)["status"], "landed");
    assert_eq!(item(&all, 1)["close_pending"], true);
    assert!(item(&all, 2).get("close_pending").is_none());
    let r = sync_json(&fx, &gh, &["--all-open", "--limit", "100"]);
    assert_eq!(r["closed"], serde_json::json!([1]));
    let log = std::fs::read_to_string(&gh.log).unwrap();
    assert_eq!(log.matches("pr close 1 --repo o/n").count(), 2, "{log}");
    assert_eq!(log.matches("pr close 2 --repo o/n").count(), 1, "{log}");
    assert!(log.contains("Landed by merge-queue in "));
}
