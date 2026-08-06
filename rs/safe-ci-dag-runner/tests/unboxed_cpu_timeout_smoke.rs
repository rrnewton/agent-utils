//! Integration smoke test for enforcement of a per-step CPU-time budget **without cgroups**.
//!
//! `cpu_timeout_smoke.rs` anchors the BOXED path only, and it SKIPS on any host that cannot box.
//! Unboxed, `cpu_stats` returns `None` and the scheduler's guard used to be skipped entirely, so
//! every `cpu_timeout` was inert on exactly the lane that matters most: any caller passing
//! `--allow-cgroup-failure`, which hermit's `ci/run-node.sh` does unconditionally under
//! `GITHUB_ACTIONS`/`CI`. Measured before the procfs fallback existed, the spinner below burned
//! 60 CPU-seconds against a 3-second budget and exited GREEN.
//!
//! Unlike its boxed sibling this test can NEVER skip: it deliberately runs with boxing off, so it
//! is a real guard on every machine, including boxing-less CI. It mirrors
//! `py/tests/test_cli.py::test_unboxed_run_enforces_cpu_timeout_and_stays_cpu_bound`.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

/// 60 CPU-seconds of pure spin against a 3 CPU-second budget. The generous wall `timeout` exists
/// only so a REGRESSION (enforcement silently broken) still terminates via a distinct TIMEOUT
/// reason instead of hanging — if the wall ever fires here the test has lost its discriminating
/// power and must be treated as failed, not passed.
const BREACH_DAG: &str = r#"{"steps": [{"group": "cpu", "job": "burn", "desc": "burn CPU past budget",
  "cmd": "python3 -c \"import time\nt=time.time()\nwhile time.time()-t<60: pass\"",
  "cpu_timeout": 3, "timeout": 300}]}"#;

/// THE DISCRIMINATOR, not decoration: 20 s of wall against the SAME 3 s CPU budget. It runs 6.7x
/// its budget in WALL terms while burning ~no CPU, so a wall timeout mislabelled as a CPU timeout
/// kills it. Only a genuine CPU bound lets it through.
const SLEEPER_DAG: &str = r#"{"steps": [{"group": "cpu", "job": "idle", "desc": "sleep well past the budget",
  "cmd": "sleep 20", "cpu_timeout": 3, "timeout": 300}]}"#;

fn write_dag(dir: &PathBuf, name: &str, body: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!("{name}.json"));
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    path
}

/// Run one DAG through the built binary with boxing forced OFF the same way a CI lane does.
fn run_unboxed(dir: &PathBuf, name: &str, body: &str) -> (Option<i32>, String) {
    let bin = env!("CARGO_BIN_EXE_safe-ci-dag-runner");
    let dag = write_dag(dir, name, body);
    let output = Command::new(bin)
        .args([
            "run",
            "--dag",
            dag.to_str().unwrap(),
            "--allow-cgroup-failure",
            "-q",
            "--no-profile",
        ])
        // Force the unboxed path exactly as hermit's ci/run-node.sh does.
        .env("GITHUB_ACTIONS", "1")
        .env("CI", "1")
        .output()
        .expect("failed to spawn the built binary");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.code(), combined)
}

#[test]
fn unboxed_cpu_timeout_reaps_a_breach_and_spares_an_idle_step() {
    let dir = std::env::temp_dir().join(format!("scdr_unboxed_cpu_{}", std::process::id()));

    // --- negative side: the breach must be killed ---
    let (code, out) = run_unboxed(&dir, "burn", BREACH_DAG);
    assert!(
        out.contains("running UNBOXED"),
        "this test is only meaningful with boxing OFF; a boxed run exercises the cgroup path and \
         proves nothing about the fallback:\n{out}"
    );
    assert_eq!(
        code,
        Some(1),
        "an UNBOXED step burning 20x its CPU budget must FAIL; exit 0 here is the original defect \
         (declared budget, no enforcement):\n{out}"
    );
    assert!(
        out.contains("CPU-TIMEOUT >3s cpu"),
        "expected the CPU budget to fire, not the (300s) wall timeout:\n{out}"
    );
    assert!(
        out.contains("PROCFS SUBTREE"),
        "an unboxed breach must NAME the degraded accounting that produced it, so a reader can \
         weigh its known blind spots:\n{out}"
    );

    // --- positive side: an idle step far past its budget in WALL terms must survive ---
    let (idle_code, idle_out) = run_unboxed(&dir, "idle", SLEEPER_DAG);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        idle_code,
        Some(0),
        "a step that sleeps 20s on a 3s CPU budget burns ~no CPU and must SURVIVE. Killing it \
         would mean the guard is measuring WALL time while calling itself a CPU timeout — the \
         exact confusion this bound exists to avoid:\n{idle_out}"
    );
}
