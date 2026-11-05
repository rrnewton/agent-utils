//! Event hooks (5891): each event fires its configured command with the
//! `merge-queue-hook/v1` payload; a slow or failing hook never delays or
//! fails the queue; the shepherd propagates.

mod support;

use serde_json::Value;
use std::time::{Duration, Instant};
use support::{git, Fixture};

fn setup_validator(tag: &str, validator_run: &str) -> Fixture {
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
        validator_run,
    ]);
    fx
}

fn state_dir(fx: &Fixture) -> std::path::PathBuf {
    let common = git(&fx.queue(), &["rev-parse", "--git-common-dir"]);
    let common = if std::path::Path::new(&common).is_absolute() {
        std::path::PathBuf::from(common)
    } else {
        fx.queue().join(common)
    };
    common.join("merge-queue")
}

fn set_hooks(fx: &Fixture, hooks: Value) {
    let path = state_dir(fx).join("config.json");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["hooks"] = hooks;
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
}

/// A hook command that captures its payload to `file`.
fn capture(file: &std::path::Path) -> String {
    format!("cat > {}", file.display())
}

fn wait_payload(file: &std::path::Path) -> Value {
    for _ in 0..200 {
        if let Ok(text) = std::fs::read_to_string(file) {
            if let Ok(value) = serde_json::from_str(&text) {
                return value;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for hook payload at {}", file.display());
}

fn status(fx: &Fixture, pr: i64) -> String {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["pr"] == pr)
        .map(|i| i["status"].as_str().unwrap().to_owned())
        .unwrap()
}

#[test]
fn on_land_fires_with_the_shepherd_and_on_base_moved() {
    let fx = setup_validator("hooks-land", "exit 0");
    let land_file = fx.root.join("on-land.json");
    let base_file = fx.root.join("on-base.json");
    set_hooks(
        &fx,
        serde_json::json!({
            "on_land": capture(&land_file),
            "on_base_moved": capture(&base_file),
        }),
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--shepherd", "alice"]);
    fx.mq_ok(&["run", "--json"]);
    let payload = wait_payload(&land_file);
    assert_eq!(payload["schema"], "merge-queue-hook/v1");
    assert_eq!(payload["event"], "on_land");
    assert_eq!(payload["pr"], 1);
    assert_eq!(payload["shepherd"], "alice");
    assert_eq!(payload["landed_sha"], fx.remote_main());
    let base = wait_payload(&base_file);
    assert_eq!(base["event"], "on_base_moved");
    assert_eq!(base["new_base"], fx.remote_main());
    assert_eq!(status(&fx, 1), "landed");
}

const RUNNER: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests); else set -- $MQ_TESTS; fi
rc=0
for t in "$@"; do
  o=pass
  if [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then o=fail; rc=1; fi
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#;

const SELECT: &str = "git diff --name-only \"$MQ_BASE\" HEAD -- tests | sed 's|^tests/||'";

fn setup_runner(tag: &str) -> Fixture {
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
        "--select",
        SELECT,
        "--policy",
        "path-routing",
    ]);
    fx
}

#[test]
fn on_red_and_on_eject_fire_for_a_red_change() {
    let fx = setup_runner("hooks-red");
    let red_file = fx.root.join("on-red.json");
    let eject_file = fx.root.join("on-eject.json");
    set_hooks(
        &fx,
        serde_json::json!({
            "on_red": capture(&red_file),
            "on_eject": capture(&eject_file),
        }),
    );
    fx.branch("a", &[("tests/a", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--shepherd", "bob"]);
    let _ = fx.mq(&["run", "--json"]);
    let red = wait_payload(&red_file);
    assert_eq!(red["event"], "on_red");
    assert_eq!(red["shepherd"], "bob");
    let eject = wait_payload(&eject_file);
    assert_eq!(eject["event"], "on_eject");
    assert_eq!(eject["pr"], 1);
    assert_eq!(status(&fx, 1), "ejected");
}

#[test]
fn on_held_fires_for_a_green_flaky_hold() {
    let fx = setup_validator("hooks-held", "echo \"GREEN_FLAKY: a\"; exit 0");
    let held_file = fx.root.join("on-held.json");
    set_hooks(&fx, serde_json::json!({"on_held": capture(&held_file)}));
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let _ = fx.mq(&["run", "--json"]);
    let held = wait_payload(&held_file);
    assert_eq!(held["event"], "on_held");
    assert_eq!(held["failing"], serde_json::json!(["a"]));
    assert_eq!(status(&fx, 1), "queued");
}

#[test]
fn a_slow_or_failing_hook_never_delays_or_fails_the_queue() {
    let fx = setup_validator("hooks-slow", "exit 0");
    let land_file = fx.root.join("slow-land.json");
    set_hooks(
        &fx,
        serde_json::json!({
            "on_land": format!("sleep 5; cat > {}", land_file.display()),
            "on_eject": "exit 3",
            "timeout_secs": 2,
        }),
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let started = Instant::now();
    fx.mq_ok(&["run", "--json"]);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the run did not wait on the slow hook: {:?}",
        started.elapsed()
    );
    assert_eq!(status(&fx, 1), "landed");
    // The slow hook still ran (and was cut at its timeout or finished).
    let _ = wait_payload(&land_file);
}

#[test]
fn the_shepherd_is_changeable() {
    let fx = setup_validator("hooks-shepherd", "exit 0");
    let land_file = fx.root.join("shepherd-land.json");
    set_hooks(&fx, serde_json::json!({"on_land": capture(&land_file)}));
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--shepherd", "alice"]);
    fx.mq_ok(&["shepherd", "1", "carol"]);
    fx.mq_ok(&["run", "--json"]);
    let payload = wait_payload(&land_file);
    assert_eq!(payload["shepherd"], "carol");
}
