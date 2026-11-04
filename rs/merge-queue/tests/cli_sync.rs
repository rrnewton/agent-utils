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

/// Listing with reviews: `(number, head, author, [(state, reviewed-commit)])`,
/// in the `gh` shape (`reviews[].commit.oid`).
type ReviewedEntry<'a> = (i64, &'a str, &'a str, &'a [(&'a str, &'a str)]);

fn listing_reviewed(entries: &[ReviewedEntry<'_>]) -> String {
    let prs: Vec<Value> = entries
        .iter()
        .map(|(number, head, author, reviews)| {
            serde_json::json!({
                "number": number, "headRefOid": head, "isDraft": false,
                "title": format!("change {number}"),
                "author": {"login": author},
                "labels": [{"name": "ready"}],
                "reviews": reviews.iter().map(|(state, commit)| {
                    serde_json::json!({"state": state, "commit": {"oid": commit},
                        "author": {"login": "reviewer-fictional"},
                        "authorAssociation": "MEMBER"})
                }).collect::<Vec<_>>()
            })
        })
        .collect();
    serde_json::to_string(&prs).unwrap()
}

/// Move `refs/pull/N/head` to a new commit (force, as a pushed head move is).
fn move_pull_request(fx: &Fixture, number: i64, files: &[(&str, &str)]) -> String {
    // Unique branch per move: callers pass a distinct first path per move.
    let name = format!("pr{number}-{}", files[0].0.replace('/', "-"));
    let head = fx.branch(&name, files);
    git(
        &fx.dev(),
        &[
            "push",
            "-q",
            "-f",
            "origin",
            &format!("{head}:refs/pull/{number}/head"),
        ],
    );
    head
}

/// A fake `agentctl` recording one line per send in `$FAKE_LOG`.
fn fake_agentctl(fx: &Fixture) -> (String, String) {
    use std::os::unix::fs::PermissionsExt;
    let script = fx.root.join("fake-agentctl.sh");
    let log = fx.root.join("fake-agentctl.log");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$FAKE_LOG\"\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script.display().to_string(), log.display().to_string())
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
    assert_eq!(r["drafts"], serde_json::json!([3]), "but they are named");
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
fn a_listing_cut_short_by_its_limit_withdraws_nothing_it_did_not_list() {
    // A listing as long as its limit may have stopped before a change that still carries the
    // label: that change stays queued. A complete listing still withdraws it.
    let fx = Fixture::new("sync-truncated");
    let gh = stub(&fx);
    fx.mq_ok(&["init", "--github-repo", "owner/name", "--gh", &gh.stub]);
    let h1 = pull_request(&fx, 1, &[("one", "1\n")]);
    let h2 = pull_request(&fx, 2, &[("two", "2\n")]);
    std::fs::write(
        &gh.list,
        listing(&[(1, &h1, false, &["ready"]), (2, &h2, false, &["ready"])]),
    )
    .unwrap();
    let r = sync_json(&fx, &gh, &["--label", "ready", "--limit", "100"]);
    assert_eq!(r["submitted"], serde_json::json!([1, 2]));
    assert!(r.get("truncated").is_none(), "{r}");
    std::fs::write(&gh.list, listing(&[(1, &h1, false, &["ready"])])).unwrap();
    let r = sync_json(&fx, &gh, &["--label", "ready", "--limit", "1"]);
    assert_eq!(r["truncated"], true, "{r}");
    assert_eq!(r["withdrawn"], serde_json::json!([]), "{r}");
    assert_eq!(item(&items(&fx), 2)["status"], "queued");
    let r = sync_json(&fx, &gh, &["--label", "ready", "--limit", "2"]);
    assert_eq!(r["withdrawn"], serde_json::json!([2]), "{r}");
    assert_eq!(item(&items(&fx), 2)["status"], "withdrawn");
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
        "# lanes append here\n1 head=a priority=20\n2 head=b after=1 solo prioritized\n",
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
    assert_eq!(item(&all, 2)["prioritized"], true);
    assert!(item(&all, 1).get("prioritized").is_none());
}

#[test]
fn the_configured_priority_label_puts_a_change_at_the_front_and_resubmits_on_change() {
    let fx = Fixture::new("sync-priority-label");
    let gh = stub(&fx);
    fx.mq_ok(&[
        "init",
        "--github-repo",
        "owner/name",
        "--gh",
        &gh.stub,
        "--priority-label",
        "ci-fix",
    ]);
    let config = std::fs::read_to_string(fx.queue().join(".git/merge-queue/config.json")).unwrap();
    assert!(config.contains("\"ci-fix\""), "{config}");
    let h1 = pull_request(&fx, 1, &[("one", "1\n")]);
    let h2 = pull_request(&fx, 2, &[("two", "2\n")]);
    // The default label means nothing once another is configured.
    std::fs::write(
        &gh.list,
        listing(&[
            (1, &h1, false, &["CI-Fix"]),
            (2, &h2, false, &["mq-priority"]),
        ]),
    )
    .unwrap();
    sync_json(&fx, &gh, &["--all-open", "--limit", "100"]);
    let all = items(&fx);
    assert_eq!(item(&all, 1)["prioritized"], true);
    assert!(item(&all, 2).get("prioritized").is_none());
    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    assert_eq!(plan["batches"][0]["prs"], serde_json::json!([1]));
    assert_eq!(plan["batches"][0]["note"], "priority");
    // Adding the label to an unchanged head resubmits it at the front.
    std::fs::write(
        &gh.list,
        listing(&[(1, &h1, false, &["CI-Fix"]), (2, &h2, false, &["ci-fix"])]),
    )
    .unwrap();
    let r = sync_json(&fx, &gh, &["--all-open", "--limit", "100"]);
    assert_eq!(r["submitted"], serde_json::json!([2]));
    assert_eq!(item(&items(&fx), 2)["prioritized"], true);
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

#[test]
fn approval_at_head_queues_only_approved_and_counts_one_batched_call() {
    let fx = Fixture::new("sync-approval-new");
    let gh = stub(&fx);
    let (agentctl, _sent) = fake_agentctl(&fx);
    fx.mq_ok(&[
        "init",
        "--github-repo",
        "owner/name",
        "--gh",
        &gh.stub,
        "--require-approval-at-head",
        "--agentctl",
        &agentctl,
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
    ]);
    let h1 = pull_request(&fx, 1, &[("one", "1\n")]);
    let h2 = pull_request(&fx, 2, &[("two", "2\n")]);
    let h3 = pull_request(&fx, 3, &[("three", "3\n")]);
    let h4 = pull_request(&fx, 4, &[("four", "4\n")]);
    let stale = "0".repeat(40);
    std::fs::write(
        &gh.list,
        listing_reviewed(&[
            (1, &h1, "lane-fictional", &[("APPROVED", &h1)]),
            (2, &h2, "lane-fictional", &[]),
            (3, &h3, "lane-fictional", &[("APPROVED", &stale)]),
            (4, &h4, "lane-fictional", &[("CHANGES_REQUESTED", &h4)]),
        ]),
    )
    .unwrap();
    let r = sync_json(&fx, &gh, &["--all-open", "--limit", "100"]);
    assert_eq!(r["submitted"], serde_json::json!([1]), "{r}");
    assert_eq!(r["unapproved"], serde_json::json!([2, 3, 4]), "{r}");
    assert_eq!(r["withdrawn"], serde_json::json!([]), "{r}");
    let all = items(&fx);
    assert_eq!(item(&all, 1)["status"], "queued");
    for pr in [2, 3, 4] {
        assert!(
            all.as_array().unwrap().iter().all(|i| i["pr"] != pr),
            "#{pr} must not be queued: {all}"
        );
    }
    // Reviews rode the same single batched list call: exactly one gh call,
    // no per-PR review calls, charged REVIEWS_COST_FACTOR (3) requests for
    // the one page (limit 100) because of the nested review connections.
    let log = std::fs::read_to_string(&gh.log).unwrap();
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(
        log.contains("reviews"),
        "reviews in the batched call: {log}"
    );
    assert_eq!(r["api_remaining"], 57, "{r}");
}

#[test]
fn head_move_without_fresh_approval_withdraws_and_notifies_with_approval_it_stays() {
    let fx = Fixture::new("sync-approval-move");
    let gh = stub(&fx);
    let (agentctl, sent) = fake_agentctl(&fx);
    fx.mq_ok(&[
        "init",
        "--github-repo",
        "owner/name",
        "--gh",
        &gh.stub,
        "--require-approval-at-head",
        "--agentctl",
        &agentctl,
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
    ]);
    let h1 = pull_request(&fx, 1, &[("one", "1\n")]);
    std::fs::write(
        &gh.list,
        listing_reviewed(&[(1, &h1, "lane-fictional", &[("APPROVED", &h1)])]),
    )
    .unwrap();
    let r = sync_json(&fx, &gh, &["--all-open", "--limit", "100"]);
    assert_eq!(r["submitted"], serde_json::json!([1]), "{r}");
    assert_eq!(item(&items(&fx), 1)["status"], "queued");

    // Head moves; the only approval is on the old head. Withdrawn, and the
    // submitter (the author) is notified through the withdrawn path.
    let h2 = move_pull_request(&fx, 1, &[("one-v2", "2\n")]);
    std::fs::write(
        &gh.list,
        listing_reviewed(&[(1, &h2, "lane-fictional", &[("APPROVED", &h1)])]),
    )
    .unwrap();
    // sync fires the withdrawn notification itself (deduped for the daemon).
    let r: Value = {
        let out = fx.mq_env(
            &["sync", "--all-open", "--limit", "100", "--json"],
            &[
                ("GH_LOG", gh.log.as_str()),
                ("GH_LIST", gh.list.as_str()),
                ("GH_FAIL_CLOSE", gh.fail_close.as_str()),
                ("FAKE_LOG", sent.as_str()),
            ],
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap()
    };
    assert_eq!(r["withdrawn"], serde_json::json!([1]), "{r}");
    assert_eq!(r["unapproved"], serde_json::json!([1]), "{r}");
    let all = items(&fx);
    assert_eq!(item(&all, 1)["status"], "withdrawn");
    let sent_text = std::fs::read_to_string(&sent).unwrap_or_default();
    assert_eq!(
        sent_text.lines().count(),
        1,
        "notify must fire: {sent_text}"
    );
    let report_path = sent_text
        .lines()
        .next()
        .unwrap()
        .strip_prefix("send --deliver-while-working --file ")
        .and_then(|rest| rest.strip_suffix(" -- lane-fictional"))
        .unwrap_or_else(|| panic!("unexpected argv: {sent_text}"))
        .trim();
    let report = std::fs::read_to_string(report_path).unwrap();
    assert!(report.contains("verdict: withdrawn"), "{report}");
    // The daemon cycle does not resend the same withdrawn transition.
    let out = fx.mq_env(&["serve", "--once"], &[("FAKE_LOG", sent.as_str())]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&sent).unwrap().lines().count(),
        1,
        "withdrawn notification must be sent exactly once"
    );

    // Resubmission with an approval at the new head re-queues normally.
    std::fs::write(
        &gh.list,
        listing_reviewed(&[(1, &h2, "lane-fictional", &[("APPROVED", &h2)])]),
    )
    .unwrap();
    let r = sync_json(&fx, &gh, &["--all-open", "--limit", "100"]);
    assert_eq!(r["submitted"], serde_json::json!([1]), "{r}");
    let all = items(&fx);
    assert_eq!(item(&all, 1)["status"], "queued");
    assert_eq!(item(&all, 1)["head"], h2);

    // Head moves again, this time with a fresh approval at the new head:
    // resubmitted at the new head and stays queued, not withdrawn.
    let h3 = move_pull_request(&fx, 1, &[("one-v3", "3\n")]);
    std::fs::write(
        &gh.list,
        listing_reviewed(&[(1, &h3, "lane-fictional", &[("APPROVED", &h3)])]),
    )
    .unwrap();
    let r = sync_json(&fx, &gh, &["--all-open", "--limit", "100"]);
    assert_eq!(r["submitted"], serde_json::json!([1]), "{r}");
    assert_eq!(r["withdrawn"], serde_json::json!([]), "{r}");
    let all = items(&fx);
    assert_eq!(item(&all, 1)["status"], "queued");
    assert_eq!(item(&all, 1)["head"], h3);
    // Every sync was one batched list call (plus no other gh calls here).
    let log = std::fs::read_to_string(&gh.log).unwrap();
    assert_eq!(log.lines().count(), 4, "{log}");
    assert!(log.lines().all(|l| l.starts_with("pr list ")), "{log}");
}

#[test]
fn approval_sync_keeps_an_existing_notify_subscription() {
    let fx = Fixture::new("sync-approval-notify");
    let gh = stub(&fx);
    let (agentctl, _sent) = fake_agentctl(&fx);
    fx.mq_ok(&[
        "init",
        "--github-repo",
        "owner/name",
        "--gh",
        &gh.stub,
        "--require-approval-at-head",
        "--agentctl",
        &agentctl,
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
    ]);
    let h1 = pull_request(&fx, 1, &[("one", "1\n")]);
    fx.mq_ok(&[
        "submit",
        "1",
        "--head",
        "refs/pull/1/head",
        "--sha",
        &h1,
        "--notify",
        "lane-subscriber",
    ]);
    // The head moves and is approved there: sync resubmits at the new head,
    // and the author must not replace the existing subscription.
    let h2 = move_pull_request(&fx, 1, &[("one-v2", "2\n")]);
    std::fs::write(
        &gh.list,
        listing_reviewed(&[(1, &h2, "lane-author", &[("APPROVED", &h2)])]),
    )
    .unwrap();
    let r = sync_json(&fx, &gh, &["--all-open", "--limit", "100"]);
    assert_eq!(r["submitted"], serde_json::json!([1]), "{r}");
    let all = items(&fx);
    assert_eq!(item(&all, 1)["head"], h2);
    assert_eq!(item(&all, 1)["notify"], "lane-subscriber", "{all}");
    // Without a prior subscription, the author is the default target.
    let h3 = pull_request(&fx, 3, &[("three", "3\n")]);
    std::fs::write(
        &gh.list,
        listing_reviewed(&[
            (1, &h2, "lane-author", &[("APPROVED", &h2)]),
            (3, &h3, "lane-author", &[("APPROVED", &h3)]),
        ]),
    )
    .unwrap();
    sync_json(&fx, &gh, &["--all-open", "--limit", "100"]);
    assert_eq!(item(&items(&fx), 3)["notify"], "lane-author");
}

/// Approval at head must fail closed when review data cannot be had: a failed or garbled list
/// call changes nothing (no new submission, no withdrawal), and missing or null review fields
/// never approve.
#[test]
fn a_failed_review_fetch_fails_closed() {
    let fx = Fixture::new("sync-approval-fetch-fails");
    let gh = stub(&fx);
    let (agentctl, _sent) = fake_agentctl(&fx);
    fx.mq_ok(&[
        "init",
        "--github-repo",
        "owner/name",
        "--gh",
        &gh.stub,
        "--require-approval-at-head",
        "--agentctl",
        &agentctl,
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
    ]);
    let h1 = pull_request(&fx, 1, &[("one", "1\n")]);
    let h2 = pull_request(&fx, 2, &[("two", "2\n")]);
    std::fs::write(
        &gh.list,
        listing_reviewed(&[(1, &h1, "lane-fictional", &[("APPROVED", &h1)])]),
    )
    .unwrap();
    let r = sync_json(&fx, &gh, &["--all-open", "--limit", "100"]);
    assert_eq!(r["submitted"], serde_json::json!([1]), "{r}");

    // The hosting service errors: the sync fails, #1 stays queued, nothing else is submitted.
    std::fs::write(
        &gh.stub,
        "#!/bin/sh\necho 'HTTP 502: bad gateway' >&2\nexit 1\n",
    )
    .unwrap();
    let out = fx.mq_env(&["sync", "--all-open", "--limit", "100"], &env(&gh));
    assert!(!out.status.success(), "a failed fetch must fail the sync");
    // Garbled output (truncated JSON) fails the same way.
    std::fs::write(
        &gh.stub,
        "#!/bin/sh\necho '[{\"number\": 2, \"headRefOid\"'\n",
    )
    .unwrap();
    let out = fx.mq_env(&["sync", "--all-open", "--limit", "100"], &env(&gh));
    assert!(!out.status.success(), "garbled output must fail the sync");
    let all = items(&fx);
    assert_eq!(item(&all, 1)["status"], "queued", "{all}");
    assert!(
        all.as_array().unwrap().iter().all(|i| i["pr"] != 2),
        "{all}"
    );

    // The API answers but carries no review data for #2 (fields omitted), or null reviews:
    // neither approves.
    std::fs::write(&gh.stub, STUB_GH).unwrap();
    let omitted = serde_json::json!([{
        "number": 2, "headRefOid": h2, "isDraft": false, "title": "change 2",
        "author": {"login": "lane-fictional"}, "labels": []
    }]);
    std::fs::write(&gh.list, omitted.to_string()).unwrap();
    let out = fx.mq_env(
        &["sync", "--all-open", "--limit", "100", "--json"],
        &env(&gh),
    );
    let text = String::from_utf8_lossy(&out.stdout);
    if out.status.success() {
        let r: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(r["submitted"], serde_json::json!([]), "{r}");
    }
    let nulls = serde_json::json!([{
        "number": 2, "headRefOid": h2, "isDraft": false, "title": "change 2",
        "author": {"login": "lane-fictional"}, "labels": [],
        "reviews": null, "latestReviews": null, "reviewDecision": null
    }]);
    std::fs::write(&gh.list, nulls.to_string()).unwrap();
    let out = fx.mq_env(
        &["sync", "--all-open", "--limit", "100", "--json"],
        &env(&gh),
    );
    if out.status.success() {
        let r: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert_eq!(r["submitted"], serde_json::json!([]), "{r}");
    }
    let all = items(&fx);
    assert!(
        all.as_array()
            .unwrap()
            .iter()
            .all(|i| i["pr"] != 2 || i["status"] != "queued"),
        "#2 was never approved: {all}"
    );
}

fn heartbeat(fx: &Fixture) -> Value {
    let common = std::path::PathBuf::from(git(&fx.queue(), &["rev-parse", "--git-common-dir"]));
    let dir = if common.is_absolute() {
        common
    } else {
        fx.queue().join(common)
    };
    let text = std::fs::read_to_string(dir.join("merge-queue").join("serve.heartbeat.json"))
        .expect("heartbeat");
    serde_json::from_str(&text).unwrap()
}

const SERVE_ONCE: [&str; 6] = [
    "serve",
    "--once",
    "--poll-ms",
    "50",
    "--base-poll-secs",
    "0",
];

#[test]
fn serve_syncs_its_label_and_validates_what_it_submits_in_the_same_cycle() {
    // A pull request labelled ready that nobody submits is ingested by the daemon itself, under
    // its own writer lock, and validated in the same loop iteration.
    let fx = Fixture::new("serve-sync-label");
    let gh = stub(&fx);
    fx.mq_ok(&[
        "init",
        "--github-repo",
        "owner/name",
        "--gh",
        &gh.stub,
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
    ]);
    let h1 = pull_request(&fx, 1, &[("one", "1\n")]);
    std::fs::write(&gh.list, listing(&[(1, &h1, false, &["mq-ready"])])).unwrap();
    let mut args = SERVE_ONCE.to_vec();
    args.extend(["--sync-label", "mq-ready"]);
    let out = fx.mq_env(&args, &env(&gh));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let all = items(&fx);
    assert_eq!(item(&all, 1)["source"], "label:mq-ready", "{all}");
    assert_eq!(item(&all, 1)["status"], "landed", "{all}");
    let hb = heartbeat(&fx);
    assert_eq!(hb["sync_label"], "mq-ready", "{hb}");
    assert!(
        hb["last_sync"].as_str().unwrap().contains("submitted [#1]"),
        "{hb}"
    );
    assert!(hb.get("sync_error").is_none(), "{hb}");
    let log = std::fs::read_to_string(&gh.log).unwrap();
    assert!(log.contains("--label mq-ready"), "{log}");
}

#[test]
fn a_label_sync_lists_one_page_so_approval_at_head_costs_three_requests() {
    // The daemon's label sync asks for one page (--sync-limit, default 100). With approval at
    // head that is charged 3 requests, so it fits a small hourly budget; asking for 300
    // results was charged 9 and refused here.
    let fx = Fixture::new("serve-sync-limit");
    let gh = stub(&fx);
    fx.mq_ok(&[
        "init",
        "--github-repo",
        "owner/name",
        "--gh",
        &gh.stub,
        "--api-per-hour",
        "3",
        "--require-approval-at-head",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
    ]);
    std::fs::write(&gh.list, listing(&[])).unwrap();
    let mut args = SERVE_ONCE.to_vec();
    args.extend(["--sync-label", "mq-ready"]);
    let out = fx.mq_env(&args, &env(&gh));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hb = heartbeat(&fx);
    assert!(hb.get("sync_error").is_none(), "{hb}");
    assert!(
        hb["last_sync"].as_str().unwrap().contains("0 listed"),
        "{hb}"
    );
    let log = std::fs::read_to_string(&gh.log).unwrap();
    assert!(log.contains("--limit 100"), "{log}");
}

#[test]
fn a_failed_label_sync_never_blocks_validation() {
    let fx = Fixture::new("serve-sync-fails");
    let gh = stub(&fx);
    fx.mq_ok(&[
        "init",
        "--github-repo",
        "owner/name",
        "--gh",
        &gh.stub,
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
    ]);
    pull_request(&fx, 1, &[("one", "1\n")]);
    fx.mq_ok(&["submit", "1", "--head", "pr1"]);
    std::fs::write(
        &gh.stub,
        "#!/bin/sh\necho 'HTTP 502: bad gateway' >&2\nexit 1\n",
    )
    .unwrap();
    let mut args = SERVE_ONCE.to_vec();
    args.extend(["--sync-label", "mq-ready"]);
    let out = fx.mq_env(&args, &env(&gh));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("sync --label mq-ready failed"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(item(&items(&fx), 1)["status"], "landed");
    let hb = heartbeat(&fx);
    assert!(
        hb["sync_error"].as_str().unwrap().contains("bad gateway"),
        "{hb}"
    );
}

#[test]
fn serve_sync_label_needs_a_hosting_repository() {
    let fx = Fixture::new("serve-sync-no-repo");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
    ]);
    let mut args = SERVE_ONCE.to_vec();
    args.extend(["--sync-label", "mq-ready"]);
    let out = fx.mq(&args);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--sync-label needs a hosting repository"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // An interval without a label is a usage error, not a silent no-op.
    let mut args = SERVE_ONCE.to_vec();
    args.extend(["--sync-every", "60"]);
    assert_eq!(fx.mq(&args).status.code(), Some(2));
}
