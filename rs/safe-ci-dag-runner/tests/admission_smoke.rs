//! Integration smoke test: prove the Rust `run`'s resource-exclusivity admission (the
//! `solo_validate` capability) REFUSES a validate invocation while a live competing holder holds
//! the box, and ADMITS it when the box is free — at parity with the Python runner.
//!
//! Admission is independent of cgroup boxing (it is a runner-level gate), so this test uses
//! `--allow-cgroup-failure` and runs in ANY environment. It brackets the guard from both sides:
//! NEGATIVE (a live foreign holder -> refusal exit 4) and POSITIVE (an empty holders dir -> admit
//! exit 0), so a guard that refused everything or admitted everything would fail.

use std::io::Write;
use std::process::{Child, Command};

const NOOP_DAG: &str =
    r#"{"steps": [{"group": "g", "job": "j", "desc": "noop", "cmd": "true", "timeout": 30}]}"#;

/// Spawn a long-lived child whose pid can back a holder file; killed at the end of the test. This is
/// our OWN child (we hold its `Child`), so signalling it is process-kill-safe.
fn spawn_live_child() -> Child {
    Command::new("sleep")
        .arg("120")
        .spawn()
        .expect("failed to spawn sleep child")
}

fn run(bin: &str, dag: &str, role: &str, holders: &str) -> Option<i32> {
    Command::new(bin)
        .args([
            "run",
            "--dag",
            dag,
            "-q",
            "--allow-cgroup-failure",
            "--no-profile",
            "--no-profile-feedback",
            "--exclusivity-role",
            role,
            "--box-holders-dir",
            holders,
        ])
        .output()
        .expect("failed to spawn the built binary")
        .status
        .code()
}

#[test]
fn admission_refuses_validate_while_a_live_holder_holds_the_box() {
    let bin = env!("CARGO_BIN_EXE_safe-ci-dag-runner");
    let dir = std::env::temp_dir().join(format!("scdr_adm_smoke_{}", std::process::id()));
    let holders = dir.join("holders");
    std::fs::create_dir_all(&holders).unwrap();
    let dag = dir.join("d.json");
    std::fs::File::create(&dag)
        .unwrap()
        .write_all(NOOP_DAG.as_bytes())
        .unwrap();
    let dag_s = dag.to_str().unwrap();
    let holders_s = holders.to_str().unwrap();

    // POSITIVE: box is free -> validate is admitted and the noop DAG passes (exit 0).
    assert_eq!(
        run(bin, dag_s, "validate", holders_s),
        Some(0),
        "validate must be ADMITTED (exit 0) when the holders dir is empty"
    );

    // NEGATIVE: plant a LIVE benchmark holder -> validate is REFUSED (exit 4).
    let mut child = spawn_live_child();
    let pid = child.id();
    std::fs::write(
        holders.join(format!("benchmark.{pid}.holder")),
        format!("role=benchmark\npid={pid}\n"),
    )
    .unwrap();
    let refused = run(bin, dag_s, "validate", holders_s);

    // NEGATIVE (other direction): a benchmark is refused while a live validate holds the box.
    std::fs::remove_file(holders.join(format!("benchmark.{pid}.holder"))).unwrap();
    std::fs::write(
        holders.join(format!("validate.{pid}.holder")),
        format!("role=validate\npid={pid}\n"),
    )
    .unwrap();
    let benchmark_refused = run(bin, dag_s, "benchmark", holders_s);

    // Kill ONLY our own child, then confirm the now-stale holder is ignored (admit).
    let _ = child.kill();
    let _ = child.wait();
    let stale_ignored = run(bin, dag_s, "validate", holders_s);

    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        refused,
        Some(4),
        "validate must be REFUSED (exit 4) while a live benchmark holds the box"
    );
    assert_eq!(
        benchmark_refused,
        Some(4),
        "benchmark must be REFUSED (exit 4) while a live validate holds the box"
    );
    assert_eq!(
        stale_ignored,
        Some(0),
        "a holder whose pid is dead is stale and must be IGNORED (admit, exit 0)"
    );
}
