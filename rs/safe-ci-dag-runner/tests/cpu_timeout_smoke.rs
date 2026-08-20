//! Integration smoke test for enforcement of a per-step CPU-time budget.
//!
//! `cpu_timeout` is a load-invariant ceiling on consumed user and system CPU. The test runs a
//! busy-looping step and requires the cgroup monitor to reap it as a `CPU-TIMEOUT` before its much
//! larger wall timeout. Hosts without delegated cgroup support report an explicit skip.

use std::io::Write;
use std::process::Command;

/// A step that burns a full core forever. Under boxing its cgroup `cpu.stat` usage crosses the
/// 1s `cpu_timeout` within ~one monitor tick, so it is reaped as a CPU-TIMEOUT. The generous wall
/// `timeout` exists only so a REGRESSION (enforcement silently broken) still terminates the test
/// via a distinct TIMEOUT reason instead of hanging.
const CPU_DAG: &str = r#"{"steps": [{"group": "cpu", "job": "burn", "desc": "burn CPU past budget",
  "cmd": "while :; do :; done",
  "cpu_timeout": 1, "timeout": 30}]}"#;

#[test]
fn boxing_cpu_timeout_reaps_a_step_past_its_budget() {
    let bin = env!("CARGO_BIN_EXE_safe-ci-dag-runner");

    let dir = std::env::temp_dir().join(format!("scdr_cpu_smoke_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dag = dir.join("cpu.json");
    let mut f = std::fs::File::create(&dag).unwrap();
    f.write_all(CPU_DAG.as_bytes()).unwrap();
    drop(f);

    // Default `run` = boxing REQUIRED. No --allow-cgroup-failure, so if boxing is unavailable the
    // binary exits 3 and we skip loudly rather than asserting on an environment that cannot box.
    // --no-profile keeps the default auto-logging profile store from writing into the test CWD.
    let output = Command::new(bin)
        .args(["run", "--dag", dag.to_str().unwrap(), "-q", "--no-profile"])
        .output()
        .expect("failed to spawn the built binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let code = output.status.code();
    let _ = std::fs::remove_dir_all(&dir);

    if code == Some(3) {
        eprintln!(
            "SKIP boxing_cpu_timeout_reaps_a_step_past_its_budget: cgroup boxing is unavailable in \
             this environment (need cgroup-v2 + a working systemd --user scope). Details:\n{stderr}"
        );
        return;
    }

    let combined = format!("{stdout}{stderr}");
    assert_eq!(
        code,
        Some(1),
        "boxed run should FAIL (exit 1) when the step exceeds its CPU-time budget; got {code:?}\n\
         {combined}"
    );
    assert!(
        combined.contains("CPU-TIMEOUT"),
        "expected a CPU-TIMEOUT report proving the per-step CPU-time budget fired (not the wall \
         TIMEOUT); the Python runner enforces this, so the Rust runner must too:\n{combined}"
    );
}
