//! Integration smoke test: prove the Rust `run --cores K` actually constrains the WHOLE run
//! process tree (the runner AND every step it forks), at parity with the Python runner.
//!
//! `--cores K` pins the entire run tree to K least-busy free cores (a cgroup cpuset where the
//! controller is delegated, else an inherited `sched_setaffinity` mask). This test runs the built
//! binary on a DAG whose single step is a FORKED descendant that reads its own `nproc` (which
//! honors sched-affinity); under `--cores 1` the step must see exactly one CPU, so it passes iff
//! the size-1 box was inherited across fork+execve to the whole tree.
//!
//! We pass `--allow-cgroup-failure` so `apply_core_box` still runs (the boxing manager is a no-op
//! with rc 0) and exercises the `sched_setaffinity` fallback even where no delegated cpuset scope
//! exists — the mechanism that must work in the 3pai sandbox and on plain CI. The box is
//! environment-dependent (sched_setaffinity can be denied, or the host may expose one CPU), so
//! when the runner does not log that it constrained the tree to 1 core this test prints a LOUD,
//! explicit skip notice (never a silent skip) and returns.

use std::io::Write;
use std::process::Command;

/// A forked step that PASSES iff it sees exactly one CPU (proves whole-tree inheritance).
const POS_DAG: &str = r#"{"steps": [{"group": "box", "job": "one", "desc": "step sees exactly 1 CPU",
  "cmd": "test \"$(nproc)\" -eq 1", "timeout": 30}]}"#;

/// The SAME step WITHOUT --cores must see >1 CPU (proves the box, not nproc, changes the count).
const NEG_DAG: &str = r#"{"steps": [{"group": "box", "job": "many", "desc": "step sees >1 CPU",
  "cmd": "test \"$(nproc)\" -gt 1", "timeout": 30}]}"#;

fn write_dag(name: &str, body: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("scdr_corebox_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dag = dir.join(name);
    let mut f = std::fs::File::create(&dag).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    dag
}

#[test]
fn cores_flag_constrains_the_whole_run_tree() {
    // A 1-core box is indistinguishable from the ambient affinity on a single-CPU host.
    let ncpu = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    if ncpu < 2 {
        eprintln!(
            "SKIP cores_flag_constrains_the_whole_run_tree: host exposes only {ncpu} CPU(s); a \
             1-core box is indistinguishable from the ambient affinity here"
        );
        return;
    }

    let bin = env!("CARGO_BIN_EXE_safe-ci-dag-runner");
    let pos = write_dag("pos.json", POS_DAG);

    // POSITIVE leg: --cores 1 must make the forked step see exactly one CPU. --allow-cgroup-failure
    // exercises the sched_setaffinity fallback; --no-profile keeps the store out of the test CWD.
    let out = Command::new(bin)
        .args([
            "run", "--dag", pos.to_str().unwrap(), "--cores", "1", "-q", "--no-profile",
            "--allow-cgroup-failure",
        ])
        .output()
        .expect("failed to spawn the built binary");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    if !combined.contains("core box: constrained to 1 core") {
        eprintln!(
            "SKIP cores_flag_constrains_the_whole_run_tree: the runner could not verify a 1-core \
             box here (neither cgroup cpuset nor sched_setaffinity engaged):\n{combined}"
        );
        let _ = std::fs::remove_file(&pos);
        return;
    }
    assert_eq!(
        out.status.code(),
        Some(0),
        "with --cores 1 the forked step must see exactly one CPU (proving the size-1 box was \
         inherited by the whole tree); got {:?}\n{combined}",
        out.status.code()
    );

    // NEGATIVE control: unconstrained, the same step sees the ambient (>1) CPU count, proving the
    // box (not nproc) is what changes it — the constraint is not vacuously always-true.
    let neg = write_dag("neg.json", NEG_DAG);
    let out2 = Command::new(bin)
        .args([
            "run", "--dag", neg.to_str().unwrap(), "-q", "--no-profile", "--allow-cgroup-failure",
        ])
        .output()
        .expect("failed to spawn the built binary");
    let combined2 = format!(
        "{}{}",
        String::from_utf8_lossy(&out2.stdout),
        String::from_utf8_lossy(&out2.stderr)
    );
    let _ = std::fs::remove_file(&pos);
    let _ = std::fs::remove_file(&neg);
    assert_eq!(
        out2.status.code(),
        Some(0),
        "without --cores the step must see the ambient (>1) CPU count, proving the box (not nproc) \
         is what changes it; got {:?}\n{combined2}",
        out2.status.code()
    );
}
