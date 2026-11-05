//! Network commands are bounded. A fetch that hangs fails the run once `net_timeout_secs` has
//! passed, instead of holding it (and the queue's writer lock) for ever. A push whose report
//! fails after the remote took it counts as landed: the base is the pushed tip.

mod support;

use std::time::{Duration, Instant};

use serde_json::Value;
use support::{git, Fixture};

fn setup(tag: &str, net_wrapper: &[&str], extra: &[&str]) -> Fixture {
    let fx = Fixture::new(tag);
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
        "exit 0",
    ];
    for word in net_wrapper {
        args.push("--net-wrapper");
        args.push(word);
    }
    args.extend_from_slice(extra);
    fx.mq_ok(&args);
    fx
}

fn status(fx: &Fixture, pr: i64) -> String {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["pr"] == pr)
        .map(|item| item["status"].as_str().unwrap().to_owned())
        .unwrap_or_else(|| "missing".to_owned())
}

#[test]
fn a_hung_fetch_fails_the_run_at_the_network_timeout() {
    let fx = setup("net-timeout-fetch", &[], &["--net-timeout-secs", "1"]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    // From now on every network command hangs: the wrapper never runs git.
    let config = config_path(&fx);
    let mut value: Value =
        serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    value["net_wrapper"] = serde_json::json!(["sh", "-c", "sleep 60", "wrap"]);
    std::fs::write(&config, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    let started = Instant::now();
    let out = fx.mq(&["run", "--json"]);
    let elapsed = started.elapsed();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("timed out after 1s (its process group was killed)"),
        "{stderr}"
    );
    // One timeout, not one per ref: a hung transport is not retried ref by ref.
    assert!(elapsed < Duration::from_secs(20), "{elapsed:?}: {stderr}");
    assert_eq!(status(&fx, 1), "queued");
}

#[test]
fn a_push_that_reports_failure_after_the_remote_took_it_has_landed() {
    // The wrapper runs git, then reports failure for a push whatever git said.
    let fx = setup(
        "net-push-report",
        &[
            "sh",
            "-c",
            r#""$@"; rc=$?; for a in "$@"; do [ "$a" = push ] && exit 1; done; exit $rc"#,
            "wrap",
        ],
        &[],
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let out = fx.mq(&["run", "--json"]);
    let report: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .unwrap_or_else(|e| panic!("bad report ({e}): {}", String::from_utf8_lossy(&out.stderr)));
    assert_eq!(
        report["landed"][0]["prs"],
        serde_json::json!([1]),
        "{report}"
    );
    assert_eq!(report["landed"][0]["tip"], fx.remote_main().as_str());
    assert_eq!(status(&fx, 1), "landed");
    let run_id = report["run"].as_str().unwrap();
    let events = std::fs::read_to_string(
        fx.queue()
            .join(".git/merge-queue/runs")
            .join(run_id)
            .join("events.jsonl"),
    )
    .unwrap();
    assert!(
        events.lines().any(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            event["kind"] == "push" && event["detail"].as_str().unwrap().contains("so it landed")
        }),
        "{events}"
    );
}

fn config_path(fx: &Fixture) -> std::path::PathBuf {
    let common = git(&fx.queue(), &["rev-parse", "--git-common-dir"]);
    let common = if std::path::Path::new(&common).is_absolute() {
        std::path::PathBuf::from(common)
    } else {
        fx.queue().join(common)
    };
    common.join("merge-queue").join("config.json")
}
