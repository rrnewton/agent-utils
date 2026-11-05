//! JSON purity: an unknown config key must not corrupt a donor-facing JSON reply.
//! The warning travels in the reply's `warnings` field and on stderr (once), never
//! as a line before the JSON on stdout.

mod support;

use serde_json::Value;
use support::Fixture;

#[test]
fn donor_info_with_an_unknown_config_key_still_prints_json_first() {
    let fx = Fixture::new("json-purity");
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
        "--offload",
        "decide",
    ]);
    // A key this build does not know (a newer queue's setting, say).
    let path = fx.queue().join(".git/merge-queue/config.json");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["future_setting"] = serde_json::json!({"x": 1});
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    let out = fx.mq(&["donor", "info"]);
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let first = stdout.lines().next().unwrap_or_default();
    assert!(
        first.starts_with('{'),
        "stdout must start with JSON: {stdout}"
    );
    let info: Value = serde_json::from_str(first).unwrap();
    let warnings = info["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{info}");
    assert!(
        warnings[0].as_str().unwrap().contains("future_setting"),
        "{info}"
    );
    // And the donor's tolerant parser takes the same reply even with noise lines
    // merged in front of it (an exec route that folds stderr into stdout).
    let noisy = format!(
        "merge-queue: warning: merged noise\nnote: --ws-url unset\n{}",
        stdout
    );
    let reparsed: Value = serde_json::from_str(noisy.lines().last().unwrap()).unwrap();
    assert_eq!(reparsed["warnings"], info["warnings"]);
}
