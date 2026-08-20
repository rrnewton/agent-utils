//! Integration smoke test: `--max-mem` reaches the OUTER SCOPE, not only the sizing model.
//!
//! `--max-mem 20G` used to feed one thing — the modelled active-step ceiling. The outer systemd
//! scope was still created with 90% of `MemAvailable`, so on a large host a run that announced a
//! 20 GiB budget could grow to most of the machine before anything stopped it, and "two validates
//! with 20 GiB each" was a property of the arithmetic rather than of the host.
//!
//! The unit tests pin the ceiling RULE (smallest of availability, environment, request). This
//! pins the WIRING through the real binary, which is the part a unit test of either end cannot
//! see: it is exactly the mutation "the run command stops passing the parsed flag to scope
//! bring-up", which leaves both ends correct and the feature absent.
//!
//! Whether the scope can actually be created here is irrelevant and deliberately not asserted:
//! the ceiling is chosen and named BEFORE the re-exec is attempted, so the check works on a host
//! with no systemd --user session at all.

use std::io::Write;
use std::process::Command;

const OK_DAG: &str = r#"{"steps": [{"group": "g", "job": "ok", "cmd": "true"}]}"#;

/// 1 MiB: far below 90% of MemAvailable on any host that can run this test, so the request is
/// unambiguously the binding ceiling and the expected line is deterministic.
const TINY_BUDGET_BYTES: i64 = 1024 * 1024;

#[test]
fn max_mem_is_the_outer_scope_ceiling_the_run_announces() {
    let bin = env!("CARGO_BIN_EXE_safe-ci-dag-runner");
    let dir = std::env::temp_dir().join(format!("scdr_maxmem_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dag = dir.join("ok.json");
    let mut f = std::fs::File::create(&dag).unwrap();
    f.write_all(OK_DAG.as_bytes()).unwrap();
    drop(f);

    let with_budget = Command::new(bin)
        .args([
            "run",
            "--dag",
            dag.to_str().unwrap(),
            "-q",
            "--no-profile",
            "--max-mem",
            "1M",
        ])
        .env_remove("SAFE_CI_IN_SCOPE")
        .env_remove("SAFE_CI_OUTER_MEMORY_MAX_BYTES")
        .output()
        .expect("failed to spawn the built binary");
    let with_stderr = String::from_utf8_lossy(&with_budget.stderr).to_string();

    let without_budget = Command::new(bin)
        .args(["run", "--dag", dag.to_str().unwrap(), "-q", "--no-profile"])
        .env_remove("SAFE_CI_IN_SCOPE")
        .env_remove("SAFE_CI_OUTER_MEMORY_MAX_BYTES")
        .output()
        .expect("failed to spawn the built binary");
    let without_stderr = String::from_utf8_lossy(&without_budget.stderr).to_string();

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        with_stderr.contains(&format!(
            "--max-mem is the outer scope ceiling: MemoryMax={TINY_BUDGET_BYTES} bytes."
        )),
        "the requested budget must become the scope's MemoryMax and be named:\n{with_stderr}"
    );
    // Bracketed the other way: without the flag the run says nothing about a requested ceiling,
    // so the line is evidence of the flag rather than boilerplate printed on every run.
    assert!(
        !without_stderr.contains("--max-mem is the outer scope ceiling"),
        "a run with no --max-mem must not claim a requested ceiling:\n{without_stderr}"
    );
}
