//! Integration smoke test: prove the SMALL DEFAULT CAP (the "forcing function") is applied and
//! enforced for a step that DECLARES NOTHING.
//!
//! Boxing-on-by-default plus a deliberately tight default (1 core / 1 GiB / 10 s CPU) is the
//! mechanism that makes an undeclared node hit the cap immediately and DECLARE its real needs, so
//! per-node resource metadata is generated EMPIRICALLY. This test plants BOTH directions plus a
//! control so a one-sided "kills everything" cap cannot pass:
//!
//!   A (breach)    : NO hint, allocate ~1.4 GiB  -> OOM-killed at the 1 GiB default (exit 1)
//!   B (compliant) : NO hint, allocate ~0.3 GiB  -> passes (the default does NOT kill everything)
//!   C (control)   : SAME ~1.4 GiB but DECLARES hard_mem_max 4 GiB -> passes, proving it was the
//!                   1 GiB *default* (not an ambient/outer limit) that killed A.
//!
//! Enforcement is GUARANTEED reached: default `run` requires boxing (no `--allow-cgroup-failure`),
//! so an environment that cannot box exits 3 and this test skips LOUDLY rather than asserting on a
//! non-boxing host. A non-3 exit means boxing was ACTIVE and the child-cgroup memory.max fired.

use std::io::Write;
use std::process::Command;

// Undeclared steps (no `hint`) — the ONLY inner memory.max they get is the small default.
const BREACH_DAG: &str = r#"{"steps": [{"group": "mem", "job": "breach-default",
  "desc": "undeclared step allocates 1.4GiB",
  "cmd": "python3 -c 'b=bytearray(1400*1024*1024); print(len(b))'"}]}"#;
const COMPLIANT_DAG: &str = r#"{"steps": [{"group": "mem", "job": "compliant-default",
  "desc": "undeclared step allocates 300MiB",
  "cmd": "python3 -c 'b=bytearray(300*1024*1024); print(len(b))'"}]}"#;
// Same allocation as BREACH but declares a 4 GiB cap: escapes the 1 GiB default.
const DECLARED_DAG: &str = r#"{"steps": [{"group": "mem", "job": "breach-but-declared",
  "desc": "1.4GiB alloc but declares hard_mem_max 4GiB",
  "cmd": "python3 -c 'b=bytearray(1400*1024*1024); print(len(b))'",
  "hint": {"hard_mem_max_bytes": 4294967296}}]}"#;

/// Run `dag_json` through the built binary under default (boxing-required) `run`; returns
/// `(exit_code, combined_stdout_stderr)`.
fn run_dag(label: &str, dag_json: &str) -> (Option<i32>, String) {
    let bin = env!("CARGO_BIN_EXE_safe-ci-dag-runner");
    let dir = std::env::temp_dir().join(format!("scdr_defcap_{}_{}", label, std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dag = dir.join("dag.json");
    let mut f = std::fs::File::create(&dag).unwrap();
    f.write_all(dag_json.as_bytes()).unwrap();
    drop(f);
    let output = Command::new(bin)
        .args(["run", "--dag", dag.to_str().unwrap(), "-q", "--no-profile"])
        .output()
        .expect("failed to spawn the built binary");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
    (output.status.code(), combined)
}

#[test]
fn small_default_cap_boxes_an_undeclared_step() {
    // Probe with the breach DAG; a code-3 means boxing is unavailable here -> skip loudly.
    let (breach_code, breach_out) = run_dag("breach", BREACH_DAG);
    if breach_code == Some(3) {
        eprintln!(
            "SKIP small_default_cap_boxes_an_undeclared_step: cgroup boxing unavailable (need \
             cgroup-v2 + a working systemd --user scope). Details:\n{breach_out}"
        );
        return;
    }

    // A: an UNDECLARED step over the 1 GiB default is OOM-killed. No hint is present, so the only
    // possible inner memory cap is the small default -> this proves the default reached enforcement.
    assert_eq!(
        breach_code,
        Some(1),
        "undeclared 1.4GiB step should be OOM-killed at the 1 GiB default (exit 1); got \
         {breach_code:?}\n{breach_out}"
    );
    assert!(
        breach_out.contains("OOM-KILLED") || breach_out.contains("MEMORY CAP HIT"),
        "expected the inner memory cap to fire on the undeclared step:\n{breach_out}"
    );

    // B: an UNDECLARED step UNDER the default passes -> the cap does not kill everything.
    let (compliant_code, compliant_out) = run_dag("compliant", COMPLIANT_DAG);
    assert_eq!(
        compliant_code,
        Some(0),
        "undeclared 300MiB step is under the 1 GiB default and must PASS; got \
         {compliant_code:?}\n{compliant_out}"
    );

    // C: the SAME 1.4GiB allocation passes when the step declares a 4 GiB cap -> it was the 1 GiB
    // *default* (not an ambient/outer limit) that killed A.
    let (declared_code, declared_out) = run_dag("declared", DECLARED_DAG);
    assert_eq!(
        declared_code,
        Some(0),
        "1.4GiB step declaring hard_mem_max 4GiB must escape the default and PASS; got \
         {declared_code:?}\n{declared_out}"
    );
}
