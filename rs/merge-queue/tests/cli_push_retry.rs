//! A transient remote-side push rejection is retried with bounded backoff; a
//! non-fast-forward still rebuilds on the moved base. When a run stops anyway,
//! its green validations are kept and the next run lands them without
//! validating again, as long as the base has not moved under them.

mod support;

use std::path::PathBuf;

use serde_json::Value;
use support::{git, Fixture};

fn config_path(fx: &Fixture) -> PathBuf {
    let common = git(&fx.queue(), &["rev-parse", "--git-common-dir"]);
    let common = if std::path::Path::new(&common).is_absolute() {
        PathBuf::from(common)
    } else {
        fx.queue().join(common)
    };
    common.join("merge-queue").join("config.json")
}

fn edit_config(fx: &Fixture, edit: impl FnOnce(&mut Value)) {
    let path = config_path(fx);
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    edit(&mut value);
    std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

fn set_wrapper(fx: &Fixture, script: String) {
    edit_config(fx, |value| {
        value["net_wrapper"] = serde_json::json!(["sh", "-c", script, "wrap"]);
    });
}

/// A queue whose validator appends a line to `validated` on every run.
/// Returns the fixture, the validation count file and the push count file.
fn setup(tag: &str) -> (Fixture, PathBuf, PathBuf) {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let validated = fx.root.join("validated");
    let pushes = fx.root.join("pushes");
    let validator = format!("echo run >> '{}'; exit 0", validated.display());
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &validator,
    ]);
    // Push retries must not slow the test down (a configured 0 would mean
    // the 40-second default).
    edit_config(&fx, |value| {
        value["land"]["push_retry_backoff_secs"] = serde_json::json!(1);
    });
    (fx, validated, pushes)
}

fn count(path: &PathBuf) -> usize {
    std::fs::read_to_string(path).map_or(0, |text| text.lines().count())
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

fn latest_events(fx: &Fixture) -> String {
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

#[test]
fn a_transient_push_rejection_lands_on_the_retry() {
    let (fx, validated, pushes) = setup("push-retry-transient");
    set_wrapper(
        &fx,
        format!(
            r#"for a in "$@"; do if [ "$a" = push ]; then echo push >> '{p}'; n=$(wc -l < '{p}'); if [ $n -le 1 ]; then echo 'remote: fatal error in commit_refs while verifying commit graph' >&2; echo ' ! [remote rejected] main (commit_refs)' >&2; exit 1; fi; fi; done; exec "$@""#,
            p = pushes.display()
        ),
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let out = fx.mq(&["run"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(status(&fx, 1), "landed");
    assert_eq!(
        count(&validated),
        1,
        "validated once, landed on the push retry"
    );
    assert_eq!(count(&pushes), 2, "one transient rejection, one retry");
    let events = latest_events(&fx);
    assert!(
        events.lines().any(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            event["kind"] == "push-retry"
        }),
        "{events}"
    );
}

#[test]
fn a_persistent_transient_rejection_keeps_the_greens_for_the_next_run() {
    let (fx, validated, pushes) = setup("push-retry-persistent");
    set_wrapper(
        &fx,
        format!(
            r#"for a in "$@"; do if [ "$a" = push ]; then echo push >> '{p}'; echo 'remote: fatal error in commit_refs while verifying commit graph' >&2; echo ' ! [remote rejected] main (commit_refs)' >&2; exit 1; fi; done; exec "$@""#,
            p = pushes.display()
        ),
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let base_before = fx.remote_main();
    fx.mq(&["run"]);
    assert_eq!(
        status(&fx, 1),
        "queued",
        "the run stops, the change stays queued"
    );
    assert_eq!(fx.remote_main(), base_before, "nothing landed");
    assert_eq!(count(&validated), 1);
    assert_eq!(
        count(&pushes),
        3,
        "bounded retries: three tries, then it gives up"
    );
    // The remote fault clears: the next run lands the kept green validation
    // without validating again.
    edit_config(&fx, |value| {
        value["net_wrapper"] = serde_json::json!([]);
    });
    let out = fx.mq(&["run"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(status(&fx, 1), "landed");
    assert_eq!(
        count(&validated),
        1,
        "the kept validation was reused, not rerun"
    );
    let files = git(&fx.remote(), &["ls-tree", "-r", "--name-only", "main"]);
    assert!(files.lines().any(|line| line == "tests/a"), "{files}");
}

#[test]
fn an_ssh_banner_push_failure_is_transient_and_lands_on_the_retry() {
    let (fx, validated, pushes) = setup("push-retry-ssh");
    set_wrapper(
        &fx,
        format!(
            r#"for a in "$@"; do if [ "$a" = push ]; then echo push >> '{p}'; n=$(wc -l < '{p}'); if [ $n -le 1 ]; then echo 'kex_exchange_identification: banner line contains invalid characters' >&2; echo 'banner exchange: Connection to UNKNOWN port 65535: invalid format' >&2; echo 'fatal: Could not read from remote repository.' >&2; exit 128; fi; fi; done; exec "$@""#,
            p = pushes.display()
        ),
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let out = fx.mq(&["run"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(status(&fx, 1), "landed");
    assert_eq!(
        count(&validated),
        1,
        "validated once, landed on the push retry"
    );
    assert_eq!(count(&pushes), 2, "one SSH banner failure, one retry");
    let events = latest_events(&fx);
    assert!(
        events.lines().any(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            event["kind"] == "push-retry"
        }),
        "{events}"
    );
}

#[test]
fn a_head_stop_logs_at_once_and_detaches_the_in_flight_validation() {
    // Head validates fast and its push is refused for good (unknown, not
    // transient); the second batch validates slowly beside it. The run must
    // log `stopped` at the decision and end without waiting out the slow
    // validation.
    let fx = Fixture::new("push-stop-detach");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let validated = fx.root.join("validated");
    let validator = format!(
        "echo run >> '{}'; if [ -f tests/slow ]; then sleep 30; fi; exit 0",
        validated.display()
    );
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &validator,
        "--max-batch",
        "1",
        "--depth",
        "2",
    ]);
    set_wrapper(
        &fx,
        r#"for a in "$@"; do if [ "$a" = push ]; then echo 'permission denied' >&2; exit 1; fi; done; exec "$@""#.to_owned(),
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/slow", "slow\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let started = std::time::Instant::now();
    let out = fx.mq(&["run"]);
    let elapsed = started.elapsed();
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(
        elapsed < std::time::Duration::from_secs(25),
        "run parked behind the slow validation for {elapsed:?}"
    );
    let events = latest_events(&fx);
    let stopped_at = events
        .lines()
        .position(|line| serde_json::from_str::<Value>(line).is_ok_and(|e| e["kind"] == "stopped"))
        .expect("a stopped event at the stop decision");
    assert!(stopped_at < events.lines().count(), "{events}");
    assert_eq!(status(&fx, 1), "queued");
    assert_eq!(status(&fx, 2), "queued");
    let _ = count(&validated);
}

#[test]
fn a_non_fast_forward_push_still_rebuilds_on_the_moved_base() {
    let (fx, validated, pushes) = setup("push-retry-non-ff");
    set_wrapper(
        &fx,
        format!(
            r#"for a in "$@"; do if [ "$a" = push ]; then echo push >> '{p}'; n=$(wc -l < '{p}'); if [ $n -le 1 ]; then d=$3; t=$(git -C "$d" rev-parse 'origin/main^{{tree}}'); c=$(echo competing | git -C "$d" commit-tree $t -p origin/main); git -C "$d" push -q origin $c:refs/heads/main; fi; fi; done; exec "$@""#,
            p = pushes.display()
        ),
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let out = fx.mq(&["run"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(status(&fx, 1), "landed");
    assert_eq!(
        count(&validated),
        2,
        "rebuilt and revalidated on the moved base"
    );
    let files = git(&fx.remote(), &["ls-tree", "-r", "--name-only", "main"]);
    assert!(files.lines().any(|line| line == "tests/a"), "{files}");
}

/// Rewrite the kept validations as rooted prefix entries: every entry's
/// parent is the original base and its changes are the cumulative prefix,
/// as a stopped run with rooted prefix validations leaves them. Returns
/// the validation count so far.
fn seed_rooted_prefixes(fx: &Fixture) -> PathBuf {
    let kept_path = fx.queue().join(".git/merge-queue/kept-validations.json");
    let kept: Value = serde_json::from_str(&std::fs::read_to_string(&kept_path).unwrap()).unwrap();
    let entry = &kept[0];
    let tip = entry["tip"].as_str().unwrap().to_owned();
    let parent = entry["parent"].as_str().unwrap().to_owned();
    let changes = entry["changes"].as_array().unwrap().clone();
    // Prefix commits: the kept tip's ancestors, one change at a time.
    let mut prefixes = vec![tip.clone()];
    let mut cursor = tip.clone();
    for _ in 1..changes.len() {
        cursor = git(&fx.queue(), &["rev-parse", &format!("{cursor}^")]);
        prefixes.push(cursor.clone());
    }
    prefixes.reverse();
    let seeded: Vec<Value> = changes
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let mut claim = entry["claim"].clone();
            claim["commit"] = serde_json::json!(prefixes[index]);
            serde_json::json!({
                "tip": prefixes[index],
                "parent": parent,
                "changes": changes[..=index],
                "claim": claim,
                "run": entry["run"],
                "validation": index + 1,
            })
        })
        .collect();
    std::fs::write(&kept_path, serde_json::to_string_pretty(&seeded).unwrap()).unwrap();
    kept_path
}

fn kept_setup(tag: &str) -> (Fixture, PathBuf, PathBuf) {
    let (fx, validated, pushes) = setup(tag);
    set_wrapper(
        &fx,
        format!(
            r#"for a in "$@"; do if [ "$a" = push ]; then echo push >> '{p}'; echo 'remote: fatal error in commit_refs while verifying commit graph' >&2; echo ' ! [remote rejected] main (commit_refs)' >&2; exit 1; fi; done; exec "$@""#,
            p = pushes.display()
        ),
    );
    (fx, validated, pushes)
}

fn clear_fault(fx: &Fixture) {
    edit_config(fx, |value| {
        value["net_wrapper"] = serde_json::json!([]);
    });
}

#[test]
fn kept_greens_land_in_sequence_when_the_landed_part_is_the_validated_prefix() {
    let (fx, validated, _pushes) = kept_setup("kept-prefix-sequence");
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    fx.mq(&["run"]);
    assert_eq!(count(&validated), 1, "one batch validation, then the stop");
    seed_rooted_prefixes(&fx);
    clear_fault(&fx);
    let out = fx.mq(&["run"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(status(&fx, 1), "landed");
    assert_eq!(status(&fx, 2), "landed");
    assert_eq!(status(&fx, 3), "landed");
    assert_eq!(
        count(&validated),
        1,
        "each kept green landed on its validated prefix, none re-validated"
    );
}

#[test]
fn a_kept_green_whose_change_moved_head_is_validated_again() {
    let (fx, validated, _pushes) = kept_setup("kept-prefix-moved-head");
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    fx.mq(&["run"]);
    seed_rooted_prefixes(&fx);
    // Force-push change 2: a new head the kept validation never saw.
    fx.branch("b2", &[("tests/b", "changed\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b2"]);
    clear_fault(&fx);
    let out = fx.mq(&["run"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(status(&fx, 2), "landed");
    assert!(
        count(&validated) > 1,
        "the moved head was validated again, not landed on the kept green"
    );
}

#[test]
fn kept_greens_resume_in_order_and_stop_at_the_first_mismatch() {
    let (fx, validated, _pushes) = kept_setup("kept-prefix-mismatch");
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.branch("d", &[("tests/d", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    fx.mq_ok(&["submit", "4", "--head", "d"]);
    fx.mq(&["run"]);
    let kept_path = seed_rooted_prefixes(&fx);
    // Corrupt the third entry: its tip is the second prefix's commit, so
    // rebuilding its remainder can never match the validated tree.
    let mut seeded: Value =
        serde_json::from_str(&std::fs::read_to_string(&kept_path).unwrap()).unwrap();
    seeded[2]["tip"] = seeded[1]["tip"].clone();
    std::fs::write(&kept_path, serde_json::to_string_pretty(&seeded).unwrap()).unwrap();
    clear_fault(&fx);
    let out = fx.mq(&["run"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    for pr in 1..=4 {
        assert_eq!(status(&fx, pr), "landed", "change {pr} lands in the end");
    }
    assert!(
        count(&validated) > 1,
        "the mismatched green and everything after it were validated again"
    );
    let events = latest_events(&fx);
    assert!(
        events.contains("no longer builds to the kept validated tree"),
        "{events}"
    );
}
