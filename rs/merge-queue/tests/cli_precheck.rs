//! The precheck: a quick command every submitted head must pass, run on the head alone before
//! the change is queued. A head that fails it is ejected at submission with the command's
//! output and its submitter is notified, so it never sinks a batch; a head that passes is queued;
//! a check that could not run queues the change anyway; verdicts are kept per head; and
//! `--skip-precheck` overrides a refusal.

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde_json::Value;
use support::{git, Fixture};

/// Every test file passes unless it says FAIL.
const RUNNER: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests); else set -- $MQ_TESTS; fi
rc=0
for t in "$@"; do
  o=pass
  if [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then o=fail; fi
  [ $o = fail ] && rc=1
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#;

/// A queue whose precheck is `precheck` (in a script file), with a fake agentctl recording its
/// sends. Returns the fixture, the precheck's call log and the fake's send log.
fn setup(tag: &str, precheck: &str, extra: &[&str]) -> (Fixture, PathBuf, PathBuf) {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let calls = fx.root.join("precheck-calls");
    let script = fx.root.join("precheck.sh");
    std::fs::write(
        &script,
        format!(
            "echo \"$MQ_PR $MQ_HEAD $MQ_HEAD_REF $MQ_BASE\" >> '{}'\n{precheck}\n",
            calls.display()
        ),
    )
    .unwrap();
    let fake = fx.root.join("fake-agentctl.sh");
    let sends = fx.root.join("fake-agentctl.log");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
            sends.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let precheck_arg = format!("sh '{}'", script.display());
    let fake_arg = fake.to_string_lossy().into_owned();
    let mut args = vec![
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        RUNNER,
        "--precheck",
        &precheck_arg,
        "--agentctl",
        &fake_arg,
    ];
    args.extend_from_slice(extra);
    fx.mq_ok(&args);
    (fx, calls, sends)
}

/// Fails when a test file says BADFMT, printing a formatter-style diff that names the file by
/// its path in the checkout.
const FORMAT_CHECK: &str = r#"bad=$(grep -l BADFMT tests/* 2>/dev/null)
[ -z "$bad" ] && exit 0
for f in $bad; do echo "Diff in $PWD/$f:1:"; echo "-BADFMT"; echo "+ok"; done
exit 1"#;

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
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
        .find(|i| i["pr"] == pr && i["status"] != "pending-intake")
        .cloned()
        .unwrap_or(Value::Null)
}

fn history(item: &Value) -> Vec<(String, String)> {
    item["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["kind"].as_str().unwrap().to_owned(),
                e["detail"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn state_dir(fx: &Fixture) -> PathBuf {
    let common = git(&fx.queue(), &["rev-parse", "--git-common-dir"]);
    let common = if Path::new(&common).is_absolute() {
        PathBuf::from(common)
    } else {
        fx.queue().join(common)
    };
    common.join("merge-queue")
}

#[test]
fn a_head_that_fails_the_precheck_is_ejected_at_submission_and_never_validated() {
    let (fx, calls, sends) = setup("precheck-refuse", FORMAT_CHECK, &[]);
    let a = fx.branch("a", &[("tests/a", "ok\n")]);
    let b = fx.branch("b", &[("tests/b", "BADFMT\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let queued = item(&fx, 1);
    assert_eq!(queued["status"], "queued");
    assert!(
        history(&queued)
            .iter()
            .any(|(kind, detail)| kind == "submitted" && detail.contains("; precheck passed (")),
        "{queued}"
    );

    let out = fx.mq(&["submit", "2", "--head", "b", "--notify", "watcher"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!(
            "#2: the precheck failed on {} (exit 1 after",
            &b[..10]
        )),
        "{stderr}"
    );
    // The output is quoted with the checkout's path taken out of the paths it names.
    assert!(
        stderr.contains("\n    Diff in tests/b:1:\n    -BADFMT\n    +ok"),
        "{stderr}"
    );
    assert!(!stderr.contains("scratch/precheck/"), "{stderr}");
    assert!(stderr.contains("--skip-precheck"), "{stderr}");

    let refused = item(&fx, 2);
    assert_eq!(refused["status"], "ejected", "{refused}");
    let kinds: Vec<String> = history(&refused)
        .into_iter()
        .map(|(kind, _)| kind)
        .collect();
    assert_eq!(kinds[..2], ["submitted", "ejected"], "{refused}");
    let ejected = &history(&refused)[1].1;
    assert!(
        ejected.starts_with("refused at submission: #2: the precheck failed on"),
        "{ejected}"
    );

    // The submitter heard of it at once, with the diff.
    let sent = lines(&sends);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        sent[0].starts_with("send --deliver-while-working --file ")
            && sent[0].ends_with(" -- watcher"),
        "{sent:?}"
    );
    let report_path = sent[0]
        .split_whitespace()
        .skip_while(|word| *word != "--file")
        .nth(1)
        .unwrap();
    let report = std::fs::read_to_string(report_path).unwrap();
    assert!(report.contains("verdict: ejected"), "{report}");
    assert!(report.contains("Diff in tests/b:1:"), "{report}");
    assert!(report.contains("precheck/verdicts/"), "{report}");
    // Delivered once: the daemon's sweep does not send it again.
    assert!(
        history(&item(&fx, 2))
            .iter()
            .any(|(kind, _)| kind == "notified"),
        "{}",
        item(&fx, 2)
    );

    // Each head was checked once, on its own commit, with the base the queue saw.
    let base = fx.remote_main();
    assert_eq!(
        lines(&calls),
        [format!("1 {a} a {base}"), format!("2 {b} b {base}")]
    );

    // The run validates and lands only the change that passed.
    let report: Value = serde_json::from_str(&fx.mq_ok(&["run", "--json"])).unwrap();
    assert_eq!(
        report["landed"][0]["prs"],
        serde_json::json!([1]),
        "{report}"
    );
    let files = git(&fx.remote(), &["ls-tree", "-r", "--name-only", "main"]);
    assert!(files.lines().any(|line| line == "tests/a"), "{files}");
    assert!(!files.lines().any(|line| line == "tests/b"), "{files}");
}

#[test]
fn a_head_keeps_its_verdict_and_skip_precheck_overrides_a_refusal() {
    let (fx, calls, _) = setup("precheck-keep", FORMAT_CHECK, &[]);
    let b = fx.branch("b", &[("tests/b", "BADFMT\n")]);
    assert!(!fx.mq(&["submit", "2", "--head", "b"]).status.success());
    // The same head again: refused again from its kept verdict, not checked again.
    let again = fx.mq(&["submit", "2", "--head", "b"]);
    assert!(!again.status.success());
    assert!(
        String::from_utf8_lossy(&again.stderr).contains("the precheck failed on"),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    assert_eq!(lines(&calls).len(), 1, "{:?}", lines(&calls));
    let kept = state_dir(&fx).join("precheck").join("verdicts");
    let verdicts: Vec<_> = std::fs::read_dir(&kept).unwrap().flatten().collect();
    assert_eq!(verdicts.len(), 1);
    assert!(verdicts[0]
        .file_name()
        .to_string_lossy()
        .starts_with(&format!("{b}-")));

    // An owner who knows better queues it anyway; its history says so.
    fx.mq_ok(&["submit", "2", "--head", "b", "--skip-precheck"]);
    let skipped = item(&fx, 2);
    assert_eq!(skipped["status"], "queued");
    assert!(
        history(&skipped)
            .iter()
            .any(|(kind, detail)| kind == "submitted"
                && detail.contains("; precheck skipped (--skip-precheck)")),
        "{skipped}"
    );
    assert_eq!(lines(&calls).len(), 1);

    // A fixed head is a new head: it is checked, passes, and is queued.
    let fixed = fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    assert_eq!(item(&fx, 2)["status"], "queued");
    assert_eq!(lines(&calls).len(), 2);
    assert!(lines(&calls)[1].starts_with(&format!("2 {fixed} ")));
}

#[test]
fn a_precheck_that_cannot_run_queues_the_change_and_says_why() {
    // Exit 127: the shell's answer for a command that is not there.
    let (fx, calls, _) = setup("precheck-missing", "exit 127", &[]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let queued = item(&fx, 1);
    assert_eq!(queued["status"], "queued");
    assert!(
        history(&queued)
            .iter()
            .any(|(kind, detail)| kind == "submitted"
                && detail.contains("precheck could not run")
                && detail.contains("it exited 127")),
        "{queued}"
    );
    // Not kept: the next submission of the head tries again.
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    assert_eq!(lines(&calls).len(), 2);
}

#[test]
fn a_precheck_past_its_deadline_is_killed_and_the_change_queued() {
    let (fx, _, _) = setup(
        "precheck-timeout",
        "sleep 60",
        &["--precheck-timeout-secs", "1"],
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    let started = Instant::now();
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    let queued = item(&fx, 1);
    assert_eq!(queued["status"], "queued");
    assert!(
        history(&queued)
            .iter()
            .any(|(kind, detail)| kind == "submitted"
                && detail.contains("it was still running after 1s")),
        "{queued}"
    );
}

#[test]
fn a_recorded_submission_is_prechecked_when_it_is_applied() {
    let (fx, _, _) = setup("precheck-recorded", FORMAT_CHECK, &[]);
    fx.branch("b", &[("tests/b", "BADFMT\n")]);
    // Another writer holds the queue: the submission is recorded, not checked yet.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state_dir(&fx).join("lock"))
        .unwrap();
    lock.try_lock_exclusive().unwrap();
    let out = fx.mq_ok(&["submit", "2", "--head", "b"]);
    assert!(out.contains("recorded #2"), "{out}");
    drop(lock);
    // The next holder of the lock applies it: refused, ejected, never validated.
    fx.mq_ok(&["run", "--json"]);
    assert_eq!(item(&fx, 2)["status"], "ejected");
    // `status` exits non-zero without a daemon; its report is what matters here.
    let status = String::from_utf8_lossy(&fx.mq(&["status"]).stdout).into_owned();
    let line = status
        .lines()
        .find(|line| line.contains("submit #2 recorded at"))
        .unwrap_or_else(|| panic!("{status}"));
    assert!(line.contains(": #2: the precheck failed on"), "{line}");
    assert!(line.contains(" more line(s) in intake/refused/"), "{line}");
    assert!(!status.contains("Diff in"), "{status}");
}

/// Set `precheck.command` in the queue's config.json (or clear it with an empty command).
fn set_precheck(fx: &Fixture, command: &str) {
    let path = state_dir(fx).join("config.json");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    if command.is_empty() {
        config.as_object_mut().unwrap().remove("precheck");
    } else {
        config["precheck"] = serde_json::json!({ "command": command });
    }
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
}

#[test]
fn a_change_queued_before_the_precheck_is_checked_before_it_joins_a_batch() {
    let (fx, calls, _) = setup("precheck-gate", FORMAT_CHECK, &[]);
    let precheck = format!("sh '{}'", fx.root.join("precheck.sh").display());
    // Queued while no precheck was configured.
    set_precheck(&fx, "");
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "BADFMT\n")]);
    fx.branch("c", &[("tests/c", "BADFMT\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    assert!(lines(&calls).is_empty());
    set_precheck(&fx, &precheck);
    // Skipped by its owner after the precheck came in: the skip is kept for its head.
    fx.mq_ok(&["submit", "3", "--head", "c", "--skip-precheck"]);
    let report: Value = serde_json::from_str(&fx.mq_ok(&["run", "--json"])).unwrap();
    // #2 is ejected before any batch is built on it; #1 and the skipped #3 land.
    assert_eq!(item(&fx, 2)["status"], "ejected");
    let ejected = history(&item(&fx, 2))
        .into_iter()
        .find(|(kind, _)| kind == "ejected")
        .unwrap()
        .1;
    assert!(
        ejected.starts_with("precheck: #2: the precheck failed on"),
        "{ejected}"
    );
    assert_eq!(item(&fx, 1)["status"], "landed", "{report}");
    assert_eq!(item(&fx, 3)["status"], "landed", "{report}");
    let files = git(&fx.remote(), &["ls-tree", "-r", "--name-only", "main"]);
    assert!(!files.lines().any(|line| line == "tests/b"), "{files}");
    // Only the heads with no kept verdict were checked: #1 and #2, once each.
    let checked: Vec<String> = lines(&calls)
        .iter()
        .map(|line| line.split(' ').next().unwrap().to_owned())
        .collect();
    assert_eq!(checked, ["1", "2"], "{:?}", lines(&calls));
}

/// A slow check for the background worker: fails a head whose test file says BADDOC, and logs
/// each call with the directory it ran in.
fn doc_check(calls: &Path) -> String {
    format!(
        r#"echo "$MQ_PR $PWD" >> '{}'
bad=$(grep -l BADDOC tests/* 2>/dev/null)
[ -z "$bad" ] && exit 0
for f in $bad; do echo "error: broken documentation link in $f"; done
exit 1"#,
        calls.display()
    )
}

/// Merge `precheck` into the queue's `precheck` configuration.
fn configure_precheck(fx: &Fixture, precheck: Value) {
    let path = state_dir(fx).join("config.json");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for (key, value) in precheck.as_object().unwrap() {
        config["precheck"][key] = value.clone();
    }
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
}

fn events(fx: &Fixture, report: &Value) -> Vec<(String, String)> {
    let path = state_dir(fx)
        .join("runs")
        .join(report["run"].as_str().unwrap())
        .join("events.jsonl");
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            (
                event["kind"].as_str().unwrap().to_owned(),
                event["detail"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

#[test]
fn a_slow_check_runs_beside_the_run_and_ejects_a_change_before_its_batch() {
    // One batch at a time, each validation 2 s: the worker checks #2 while #1 validates.
    let slow_runner = format!("sleep 2\n{RUNNER}");
    let (fx, _, _) = setup(
        "precheck-background",
        "exit 0",
        &["--depth", "1", "--max-batch", "1"],
    );
    let doc_calls = fx.root.join("doc-calls");
    let doc = fx.root.join("doc-check.sh");
    std::fs::write(&doc, doc_check(&doc_calls)).unwrap();
    configure_precheck(
        &fx,
        serde_json::json!({ "background": format!("sh '{}'", doc.display()) }),
    );
    let path = state_dir(&fx).join("config.json");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["validator"]["run"] = Value::String(slow_runner);
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "BADDOC\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let report: Value = serde_json::from_str(&fx.mq_ok(&["run", "--json"])).unwrap();
    assert_eq!(item(&fx, 1)["status"], "landed", "{report}");
    let refused = item(&fx, 2);
    assert_eq!(refused["status"], "ejected", "{report}");
    let ejected = history(&refused)
        .into_iter()
        .find(|(kind, _)| kind == "ejected")
        .unwrap()
        .1;
    assert!(
        ejected.starts_with("precheck: #2: the precheck failed on"),
        "{ejected}"
    );
    assert!(
        ejected.contains("error: broken documentation link in tests/b"),
        "{ejected}"
    );
    // #2 was never validated: one validation in the run, #1's.
    assert_eq!(
        report["validations"].as_array().unwrap().len(),
        1,
        "{report}"
    );
    // Both heads were checked once each, in the worker's own checkout.
    let calls = lines(&doc_calls);
    let mut prs: Vec<&str> = calls.iter().map(|line| &line[..1]).collect();
    prs.sort_unstable();
    assert_eq!(prs, ["1", "2"], "{calls:?}");
    assert!(
        calls
            .iter()
            .all(|line| line.ends_with("/scratch/precheck-background")),
        "{calls:?}"
    );
}

#[test]
fn a_red_batch_its_precheck_explains_ejects_the_culprit_and_keeps_the_rest_together() {
    // The worker's check of #1 waits until the first validation has started, so #1 joins the
    // batch unchecked; the validation waits for that check to fail before it reports. It fails
    // the `lint.doc` step when a test file says BADDOC.
    let (fx, _, _) = setup("precheck-culprit", "exit 0", &["--max-batch", "3"]);
    let validating = fx.root.join("validating");
    let checked = fx.root.join("doc-failed");
    let wait = |marker: &Path| {
        format!(
            "i=0; while [ ! -e '{}' ] && [ $i -lt 400 ]; do sleep 0.05; i=$((i+1)); done",
            marker.display()
        )
    };
    let runner = format!(
        "touch '{}'\n{}\nsleep 0.5\nif [ \"$MQ_SELECTION\" = all ]; then \
         if grep -q BADDOC tests/* 2>/dev/null; then o=fail; else o=pass; fi; \
         printf '{{\"id\":\"lint.doc\",\"outcome\":\"%s\"}}\\n' \"$o\" >> \"$MQ_RESULTS\"; \
         [ $o = fail ] && exit 1; fi\n{RUNNER}",
        validating.display(),
        wait(&checked)
    );
    let doc_calls = fx.root.join("doc-calls");
    let doc = fx.root.join("doc-check.sh");
    std::fs::write(
        &doc,
        doc_check(&doc_calls)
            .replacen("\nbad=", &format!("\n{}\nbad=", wait(&validating)), 1)
            .replacen(
                "\nexit 1",
                &format!("\ntouch '{}'\nexit 1", checked.display()),
                1,
            ),
    )
    .unwrap();
    configure_precheck(
        &fx,
        serde_json::json!({
            "background": format!("sh '{}'", doc.display()),
            "covers": ["lint.doc"],
        }),
    );
    let path = state_dir(&fx).join("config.json");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["validator"]["run"] = Value::String(runner);
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    fx.branch("a", &[("tests/a", "BADDOC\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    for (pr, head) in [("1", "a"), ("2", "b"), ("3", "c")] {
        fx.mq_ok(&["submit", pr, "--head", head]);
    }
    let report: Value = serde_json::from_str(&fx.mq_ok(&["run", "--json"])).unwrap();
    assert_eq!(item(&fx, 1)["status"], "ejected", "{report}");
    let ejected = history(&item(&fx, 1))
        .into_iter()
        .find(|(kind, _)| kind == "ejected")
        .unwrap()
        .1;
    assert!(
        ejected.starts_with("precheck: #1: the precheck failed on"),
        "{ejected}"
    );
    assert_eq!(item(&fx, 2)["status"], "landed", "{report}");
    assert_eq!(item(&fx, 3)["status"], "landed", "{report}");
    let seen = events(&fx, &report);
    assert!(
        seen.iter().any(|(kind, detail)| kind == "precheck-culprit"
            && detail.contains("#1, #2, #3 red (lint.doc)")
            && detail.ends_with("ejected, and #2, #3 go back as one batch")),
        "{seen:?}"
    );
    assert!(!seen.iter().any(|(kind, _)| kind == "split"), "{seen:?}");
    // The rest went back together: two validations in all, the red batch and theirs.
    assert_eq!(
        report["validations"].as_array().unwrap().len(),
        2,
        "{report}"
    );
}
