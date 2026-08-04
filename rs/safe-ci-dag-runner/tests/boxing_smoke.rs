//! Integration smoke test: prove the Rust `run`'s cgroup boxing actually CAPS memory.
//!
//! Cgroup boxing is the tool's primary purpose (`ds-4wldrc`): an unboxed runner is useless. This
//! test runs the built binary on a DAG whose single step grows a bash string past a tiny per-step
//! `hard_mem_max_bytes` cap; under real two-level cgroup-v2 boxing the kernel OOM-kills the step
//! at its cap, so the run fails with an `OOM-KILLED` reason.
//!
//! Cgroup boxing is environment-dependent (a CI container may have no delegated cgroup or systemd
//! `--user` scope). When boxing genuinely cannot be established the default `run` exits 3; this
//! test then prints a LOUD, explicit skip notice (never a silent skip) and returns — the boxing
//! assertion runs wherever a working cgroup-v2 + systemd `--user` scope is available.

use std::io::Write;
use std::process::Command;

/// A step that grows a bash string until it exceeds its cgroup memory cap (bash itself is the
/// process holding the memory, so bash is the one OOM-killed -> the step's leader exits non-zero).
const OOM_DAG: &str = r#"{"steps": [{"group": "mem", "job": "hog", "desc": "allocate past cap",
  "cmd": "s=x; while true; do s=\"$s$s\"; done",
  "hint": {"hard_mem_max_bytes": 67108864}}]}"#;

#[test]
fn boxing_oom_kills_a_step_past_its_cap() {
    let bin = env!("CARGO_BIN_EXE_safe-ci-dag-runner");

    let dir = std::env::temp_dir().join(format!("scdr_smoke_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dag = dir.join("oom.json");
    let mut f = std::fs::File::create(&dag).unwrap();
    f.write_all(OOM_DAG.as_bytes()).unwrap();
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
            "SKIP boxing_oom_kills_a_step_past_its_cap: cgroup boxing is unavailable in this \
             environment (need cgroup-v2 + a working systemd --user scope). Details:\n{stderr}"
        );
        return;
    }

    let combined = format!("{stdout}{stderr}");
    assert_eq!(
        code,
        Some(1),
        "boxed run should FAIL (exit 1) when the step is OOM-killed; got {code:?}\n{combined}"
    );
    assert!(
        combined.contains("OOM-KILLED") || combined.contains("MEMORY CAP HIT"),
        "expected an OOM-KILLED / MEMORY CAP HIT report proving the inner memory cap fired:\n{combined}"
    );
}

#[test]
fn boxed_oom_does_not_truncate_a_neighbour_artifact() {
    let bin = env!("CARGO_BIN_EXE_safe-ci-dag-runner");
    let dir = std::env::temp_dir().join(format!("scdr_oom_artifact_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("neighbour.c");
    let object = dir.join("neighbour.o");
    let executable = dir.join("neighbour");
    let dag = dir.join("oom-neighbour.json");

    // The neighbour first creates the exact persistent failure shape observed in
    // the DynamoRIO tree: a zero-byte .o with a valid mtime. It then sleeps across
    // the offender's OOM, compiles over that placeholder, links, and executes. If
    // the OOM is misattributed to the neighbour, the zero-byte object remains.
    let neighbour_cmd = format!(
        "printf 'int main(void) {{ return 0; }}\\n' > {source}; \
         : > {object}; sleep 1; \
         cc -c {source} -o {object}; test -s {object}; \
         cc {object} -o {executable}; {executable}",
        source = source.display(),
        object = object.display(),
        executable = executable.display(),
    );
    let dag_json = format!(
        r#"{{"steps": [
          {{"group": "artifact", "job": "neighbour", "desc": "compile through sibling OOM",
            "cmd": {neighbour_cmd:?}, "hint": {{"hard_mem_max_bytes": 268435456}}}},
          {{"group": "mem", "job": "offender", "desc": "allocate past cap",
            "cmd": "s=x; while true; do s=\"$s$s\"; done",
            "hint": {{"hard_mem_max_bytes": 67108864}}}}
        ]}}"#
    );
    std::fs::write(&dag, dag_json).unwrap();

    let output = Command::new(bin)
        .args([
            "run",
            "--dag",
            dag.to_str().unwrap(),
            "--jobs",
            "2",
            "--keep-going",
            "--no-profile",
        ])
        .output()
        .expect("failed to spawn the built binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    let code = output.status.code();
    if code == Some(3) {
        eprintln!(
            "SKIP boxed_oom_does_not_truncate_a_neighbour_artifact: cgroup boxing is unavailable \
             in this environment (need cgroup-v2 + a working systemd --user scope). Details:\n{stderr}"
        );
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    assert_eq!(
        code,
        Some(1),
        "the offender should fail the overall run while the neighbour completes; got \
         {code:?}\n{combined}"
    );
    assert!(
        combined.contains("mem.offender")
            && (combined.contains("OOM-KILLED") || combined.contains("MEMORY CAP HIT")),
        "failure attribution must name the OOM offender:\n{combined}"
    );
    assert!(
        combined.contains("[artifact.neighbour]")
            && combined.contains("PASS   compile through sibling OOM"),
        "the neighbour must complete after the sibling OOM:\n{combined}"
    );
    let object_len = std::fs::metadata(&object)
        .map(|meta| meta.len())
        .unwrap_or(0);
    assert!(
        object_len > 0,
        "neighbour survived as a process but left a truncated zero-byte object"
    );
    assert!(
        executable.is_file(),
        "neighbour object was not linkable into an executable"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
