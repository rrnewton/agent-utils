//! Push notifications: `submit --notify` stores an agentctl name on the item,
//! the `serve` daemon sends exactly one message per state change through a
//! fake agentctl that records its argv, delivery is deduplicated across
//! daemon restarts, failures retry at most three times, and the queue lands
//! even when notification fails.

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use serde_json::Value;
use support::{git, Fixture};

/// A tiny test runner: each file under `tests/` is one test, failing when it
/// contains FAIL.
const RUNNER: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests); else set -- $MQ_TESTS; fi
rc=0
for t in "$@"; do
  o=pass
  if [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then o=fail; fi
  [ $o = fail ] && rc=1
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#;

/// Path-routing policy: the tests whose files changed.
const SELECT: &str = "git diff --name-only \"$MQ_BASE\" HEAD -- tests | sed 's|^tests/||'";

/// A fake `agentctl`: records `"$*"` (one line per call) in `$FAKE_LOG` and
/// exits with `$FAKE_EXIT` (default 0).
fn fake_agentctl(fx: &Fixture) -> (PathBuf, PathBuf) {
    let script = fx.root.join("fake-agentctl.sh");
    let log = fx.root.join("fake-agentctl.log");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$FAKE_LOG\"\nexit \"${FAKE_EXIT:-0}\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, log)
}

fn setup(tag: &str, validator_run: &str, extra: &[&str]) -> (Fixture, PathBuf) {
    let fx = Fixture::new(tag);
    let (script, log) = fake_agentctl(&fx);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let mut args = vec![
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        validator_run,
        "--agentctl",
    ];
    let script_arg = script.to_string_lossy().into_owned();
    args.push(&script_arg);
    args.extend_from_slice(extra);
    fx.mq_ok(&args);
    (fx, log)
}

fn serve_once(fx: &Fixture, log: &std::path::Path, exit: &str) {
    let out = fx.mq_env(
        &["serve", "--once"],
        &[("FAKE_LOG", &log.to_string_lossy()), ("FAKE_EXIT", exit)],
    );
    assert!(
        out.status.success(),
        "serve --once failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn sent_lines(log: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn item(fx: &Fixture, pr: i64) -> Value {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["pr"] == pr)
        .unwrap_or_else(|| panic!("no item #{pr}"))
        .clone()
}

/// The `--file` argument of the single recorded send, and its contents.
fn report_of(log: &std::path::Path) -> String {
    let lines = sent_lines(log);
    assert_eq!(lines.len(), 1, "expected one send, got {lines:?}");
    let line = &lines[0];
    // Options first, then `--`, then the name (a name may start with `-`).
    let path = line
        .strip_prefix("send --deliver-while-working --file ")
        .and_then(|rest| rest.strip_suffix(" -- lane-fictional"))
        .unwrap_or_else(|| panic!("unexpected argv: {line}"))
        .trim();
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

#[test]
fn submit_stores_notify_and_sync_ready_file_sets_it() {
    let (fx, _log) = setup("notify-store", "exit 0", &[]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--notify", "lane-fictional"]);
    assert_eq!(item(&fx, 1)["notify"], "lane-fictional");
    // Resubmitting without --notify keeps the recorded name.
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    assert_eq!(item(&fx, 1)["notify"], "lane-fictional");

    // The sync equivalent: a ready-file `notify=` field.
    fx.branch("b", &[("tests/b", "ok\n")]);
    let ready = fx.root.join("ready.txt");
    std::fs::write(&ready, "2 head=b notify=lane-fictional\n").unwrap();
    fx.mq_ok(&["sync", "--file", &ready.to_string_lossy()]);
    assert_eq!(item(&fx, 2)["notify"], "lane-fictional");
}

#[test]
fn landed_sends_exactly_once_across_daemon_restarts() {
    let (fx, log) = setup(
        "notify-landed",
        RUNNER,
        &["--select", SELECT, "--policy", "path-routing"],
    );
    let head = fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--notify", "lane-fictional"]);
    serve_once(&fx, &log, "0");
    assert_eq!(item(&fx, 1)["status"], "landed");
    let report = report_of(&log);
    assert!(report.contains("verdict: landed"), "{report}");
    assert!(
        report.contains(&format!("candidate_sha: {head}")),
        "{report}"
    );
    assert!(
        report.contains(&format!("landed_main_sha: {}", fx.remote_main())),
        "{report}"
    );
    assert!(report.contains("run_report: "), "{report}");
    // Daemon restarts (fresh `serve --once` processes) never resend.
    serve_once(&fx, &log, "0");
    serve_once(&fx, &log, "0");
    assert_eq!(sent_lines(&log).len(), 1);
    assert_eq!(item(&fx, 1)["notify_delivery"]["delivered"], true);
}

#[test]
fn ejected_sends_exactly_once_with_failing_tests() {
    let (fx, log) = setup(
        "notify-ejected",
        RUNNER,
        &["--select", SELECT, "--policy", "path-routing"],
    );
    fx.branch("a", &[("tests/a", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--notify", "lane-fictional"]);
    serve_once(&fx, &log, "0");
    assert_eq!(item(&fx, 1)["status"], "ejected");
    let report = report_of(&log);
    assert!(report.contains("verdict: ejected"), "{report}");
    assert!(report.contains("ejection_report: "), "{report}");
    // Failed test IDs are inline, and the linked run report is the deciding
    // run's (recorded on the item), which is under the queue's runs/.
    assert!(report.contains("failed: a"), "{report}");
    assert!(report.contains("run_report: "), "{report}");
    assert!(
        item(&fx, 1)["deciding_report"]
            .as_str()
            .unwrap()
            .contains("report.md"),
        "{}",
        item(&fx, 1)
    );
    serve_once(&fx, &log, "0");
    assert_eq!(sent_lines(&log).len(), 1);
}

#[test]
fn withdrawn_sends_exactly_once() {
    let (fx, log) = setup("notify-withdrawn", "exit 0", &[]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--notify", "lane-fictional"]);
    fx.mq_ok(&["withdraw", "1"]);
    serve_once(&fx, &log, "0");
    assert_eq!(item(&fx, 1)["status"], "withdrawn");
    let report = report_of(&log);
    assert!(report.contains("verdict: withdrawn"), "{report}");
    serve_once(&fx, &log, "0");
    assert_eq!(sent_lines(&log).len(), 1);
}

#[test]
fn refusal_deferred_sends_exactly_once_per_refusal() {
    let (fx, log) = setup(
        "notify-refused",
        "echo COULD_NOT_RUN; exit 75",
        &["--refusal-max-retries", "0", "--refusal-backoff-secs", "60"],
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--notify", "lane-fictional"]);
    serve_once(&fx, &log, "0");
    assert_eq!(item(&fx, 1)["status"], "queued");
    assert_eq!(item(&fx, 1)["validation_refusals"], 1);
    let report = report_of(&log);
    assert!(report.contains("verdict: refusal-deferred"), "{report}");
    // Still deferred on the next cycle: no revalidation, no resend.
    serve_once(&fx, &log, "0");
    assert_eq!(sent_lines(&log).len(), 1);
}

#[test]
fn failing_agentctl_retries_at_most_three_times_and_the_queue_still_lands() {
    let (fx, log) = setup("notify-failing", "exit 0", &[]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--notify", "lane-fictional"]);
    for _ in 0..5 {
        serve_once(&fx, &log, "1");
    }
    // The queue landed on the first cycle even though every send failed.
    assert_eq!(item(&fx, 1)["status"], "landed");
    // Bounded across daemon restarts: three attempts, then it gives up.
    assert_eq!(sent_lines(&log).len(), 3, "{}", sent_lines(&log).join("\n"));
    let delivery = &item(&fx, 1)["notify_delivery"];
    assert_eq!(delivery["attempts"], 3, "{delivery}");
    assert!(delivery["delivered"].is_null() || delivery["delivered"] == false);
    assert!(
        delivery["last_error"]
            .as_str()
            .unwrap()
            .contains("exited 1"),
        "{delivery}"
    );
    let landed_item = item(&fx, 1);
    let kinds: Vec<&str> = landed_item["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"notify-failed"), "{kinds:?}");
}

#[test]
fn missing_agentctl_binary_is_a_failed_send_not_a_queue_failure() {
    let fx = Fixture::new("notify-missing");
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
        "exit 0",
        "--agentctl",
        "/definitely/missing/agentctl",
    ]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--notify", "lane-fictional"]);
    let out = fx.mq(&["serve", "--once"]);
    assert!(
        out.status.success(),
        "a missing agentctl must not fail the daemon:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(item(&fx, 1)["status"], "landed");
    let delivery = &item(&fx, 1)["notify_delivery"];
    assert_eq!(delivery["attempts"], 1, "{delivery}");
    assert!(
        delivery["last_error"]
            .as_str()
            .unwrap()
            .contains("cannot run"),
        "{delivery}"
    );
}

#[test]
fn notify_help_is_documented_on_every_changed_surface() {
    let (fx, _log) = setup("notify-help", "exit 0", &[]);
    for args in [
        vec!["submit", "--help"],
        vec!["submit", "-h"],
        vec!["serve", "--help"],
        vec!["serve", "-h"],
        vec!["init", "--help"],
        vec!["init", "-h"],
    ] {
        let out = fx.mq(&args);
        assert!(out.status.success(), "{args:?} failed");
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        match args[0] {
            "submit" => assert!(text.contains("--notify"), "{text}"),
            "serve" => {
                assert!(text.contains("--agentctl"), "{text}");
                assert!(text.contains("MERGE_QUEUE_AGENTCTL"), "{text}");
            }
            _ => assert!(text.contains("--agentctl"), "{text}"),
        }
    }
}
