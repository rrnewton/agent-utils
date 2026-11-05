//! `notify_mode` (`both` / `hooks-only` / `notify-only`): hooks-only
//! suppresses the built-in `--notify` delivery only for a verdict whose
//! hook is configured (never silently dropping a change), a per-item
//! `--notify-mode` overrides the queue config, the default is unchanged,
//! and notify-only is the mirror for hooks.

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;
use support::{git, Fixture};

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

fn setup(tag: &str, init_extra: &[&str]) -> (Fixture, PathBuf) {
    let fx = Fixture::new(tag);
    let (script, log) = fake_agentctl(&fx);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let script_arg = script.to_string_lossy().into_owned();
    let mut args = vec![
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
        "--agentctl",
        &script_arg,
    ];
    args.extend_from_slice(init_extra);
    fx.mq_ok(&args);
    (fx, log)
}

fn state_dir(fx: &Fixture) -> PathBuf {
    let common = git(&fx.queue(), &["rev-parse", "--git-common-dir"]);
    let common = if std::path::Path::new(&common).is_absolute() {
        PathBuf::from(common)
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

fn serve_once(fx: &Fixture, log: &std::path::Path) {
    let out = fx.mq_env(
        &["serve", "--once"],
        &[("FAKE_LOG", &log.to_string_lossy()), ("FAKE_EXIT", "0")],
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

fn history_details(it: &Value, kind: &str) -> Vec<String> {
    it["history"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == kind)
        .map(|e| e["detail"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[test]
fn hooks_only_with_a_hook_suppresses_notify_and_records_it() {
    let (fx, log) = setup("mode-hooks-only-hook", &["--notify-mode", "hooks-only"]);
    let land_file = fx.root.join("on-land.json");
    set_hooks(&fx, serde_json::json!({ "on_land": capture(&land_file) }));
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--notify", "lane-fictional"]);
    serve_once(&fx, &log);
    // The hook is the delivery; no built-in send happens.
    let payload = wait_payload(&land_file);
    assert_eq!(payload["event"], "on_land");
    assert_eq!(payload["notify_mode"], "hooks-only");
    assert!(sent_lines(&log).is_empty(), "{:?}", sent_lines(&log));
    let it = item(&fx, 1);
    assert_eq!(it["status"], "landed");
    assert_eq!(it["notify_mode"], "hooks-only", "{it}");
    let suppressed = history_details(&it, "notify-suppressed");
    assert_eq!(suppressed.len(), 1, "{it}");
    assert!(suppressed[0].contains("hooks-only"), "{suppressed:?}");
    assert!(suppressed[0].contains("on_land"), "{suppressed:?}");
    assert!(
        history_details(&it, "submitted")[0].contains("notify_mode=hooks-only"),
        "{it}"
    );
    // A later cycle does not send it either, and does not re-record it.
    serve_once(&fx, &log);
    assert!(sent_lines(&log).is_empty());
    assert_eq!(history_details(&item(&fx, 1), "notify-suppressed").len(), 1);
}

#[test]
fn hooks_only_without_a_hook_for_the_event_still_notifies() {
    let (fx, log) = setup("mode-hooks-only-no-hook", &["--notify-mode", "hooks-only"]);
    // Only on_eject is configured; a landing has no hook to take its place.
    let eject_file = fx.root.join("on-eject.json");
    set_hooks(&fx, serde_json::json!({ "on_eject": capture(&eject_file) }));
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--notify", "lane-fictional"]);
    serve_once(&fx, &log);
    assert_eq!(item(&fx, 1)["status"], "landed");
    assert_eq!(sent_lines(&log).len(), 1, "{:?}", sent_lines(&log));
    assert_eq!(item(&fx, 1)["notify_delivery"]["delivered"], true);
    assert!(history_details(&item(&fx, 1), "notify-suppressed").is_empty());

    // A verdict with no hook counterpart at all (withdrawn) also notifies.
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b", "--notify", "lane-fictional"]);
    fx.mq_ok(&["withdraw", "2"]);
    serve_once(&fx, &log);
    assert_eq!(sent_lines(&log).len(), 2, "{:?}", sent_lines(&log));
}

#[test]
fn a_per_item_override_beats_the_queue_config() {
    let (fx, log) = setup("mode-per-item", &["--notify-mode", "hooks-only"]);
    let land_file = fx.root.join("on-land.json");
    set_hooks(&fx, serde_json::json!({ "on_land": capture(&land_file) }));
    // Inherits hooks-only: suppressed.
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--notify", "lane-fictional"]);
    serve_once(&fx, &log);
    let _ = wait_payload(&land_file);
    assert!(sent_lines(&log).is_empty(), "{:?}", sent_lines(&log));
    assert_eq!(item(&fx, 1)["notify_mode"], "hooks-only");

    // The same queue, one change overriding to both: notify fires again.
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&[
        "submit",
        "2",
        "--head",
        "b",
        "--notify",
        "lane-fictional",
        "--notify-mode",
        "both",
    ]);
    serve_once(&fx, &log);
    assert_eq!(sent_lines(&log).len(), 1, "{:?}", sent_lines(&log));
    let it = item(&fx, 2);
    assert_eq!(it["notify_mode"], "both", "{it}");
    assert_eq!(it["notify_delivery"]["delivered"], true);
    assert!(
        history_details(&it, "submitted")[0].contains("notify_mode=both (per-item)"),
        "{it}"
    );
    // Resubmitting without the flag keeps the recorded override.
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    assert_eq!(item(&fx, 2)["notify_mode"], "both");
}

#[test]
fn the_default_is_both_hook_and_notify() {
    let (fx, log) = setup("mode-default-both", &[]);
    // The default config carries no notify_mode field at all.
    let config: Value =
        serde_json::from_str(&std::fs::read_to_string(state_dir(&fx).join("config.json")).unwrap())
            .unwrap();
    assert!(config.get("notify_mode").is_none(), "{config}");
    let land_file = fx.root.join("on-land.json");
    set_hooks(&fx, serde_json::json!({ "on_land": capture(&land_file) }));
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--notify", "lane-fictional"]);
    serve_once(&fx, &log);
    let payload = wait_payload(&land_file);
    assert_eq!(payload["event"], "on_land");
    assert_eq!(sent_lines(&log).len(), 1, "{:?}", sent_lines(&log));
    let it = item(&fx, 1);
    assert_eq!(it["notify_mode"], "both", "{it}");
    assert_eq!(it["notify_delivery"]["delivered"], true);
    assert!(history_details(&it, "notify-suppressed").is_empty(), "{it}");
}

#[test]
fn notify_mode_is_documented_on_the_help_and_guide_surfaces() {
    let (fx, _log) = setup("mode-help", &[]);
    for args in [vec!["submit", "--help"], vec!["init", "--help"]] {
        let out = fx.mq(&args);
        assert!(out.status.success(), "{args:?} failed");
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(text.contains("--notify-mode"), "{args:?}: {text}");
        assert!(text.contains("hooks-only"), "{args:?}: {text}");
    }
    assert!(merge_queue::USER_GUIDE.contains("notify_mode"));
    assert!(merge_queue::USER_GUIDE.contains("hooks-only"));
}

#[test]
fn notify_only_suppresses_the_hook_but_still_notifies() {
    let (fx, log) = setup("mode-notify-only", &["--notify-mode", "notify-only"]);
    let land_file = fx.root.join("on-land.json");
    set_hooks(&fx, serde_json::json!({ "on_land": capture(&land_file) }));
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--notify", "lane-fictional"]);
    serve_once(&fx, &log);
    assert_eq!(sent_lines(&log).len(), 1, "{:?}", sent_lines(&log));
    // Give a (wrongly) fired hook time to appear, then check it did not.
    std::thread::sleep(Duration::from_millis(500));
    assert!(!land_file.exists(), "notify-only must not fire on_land");

    // With no notify target, the hook is the only channel and still fires.
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    serve_once(&fx, &log);
    let payload = wait_payload(&land_file);
    assert_eq!(payload["pr"], 2);
    assert_eq!(sent_lines(&log).len(), 1, "{:?}", sent_lines(&log));
}
