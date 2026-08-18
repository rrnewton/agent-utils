//! A stdin DAG must survive the runner's mandatory systemd scope re-exec.

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn boxed_stdin_dag_keeps_step_and_cpu_limits_independent() {
    let exe = env!("CARGO_BIN_EXE_safe-ci-dag-runner");
    let mut child = Command::new(exe)
        .args([
            "run",
            "--dag",
            "-",
            "--stress",
            "3",
            "--max-steps",
            "3",
            "--max-cpus",
            "2",
            "--no-profile",
            "--no-profile-feedback",
            "-q",
        ])
        .env_remove("CI")
        .env_remove("GITHUB_ACTIONS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn safe-ci-dag-runner");
    child
        .stdin
        .take()
        .expect("stdin pipe")
        .write_all(
            br#"{"steps":[{"group":"stress","job":"singleton","cmd":"sleep 1","hint":{"hard_mem_max_bytes":67108864}}]}"#,
        )
        .expect("write stdin DAG");
    let output = child.wait_with_output().expect("wait for runner");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    if output.status.code() == Some(3) {
        eprintln!("skipping: cgroup boxing unavailable on this host\n{combined}");
        return;
    }
    assert!(output.status.success(), "{combined}");
    assert!(combined.contains("containment OBSERVED"), "{combined}");
    assert!(
        combined.contains("per-run CPU cap: CPUQuota=200%"),
        "outer CPUQuota was not read back exactly:\n{combined}"
    );
    let cpu_max = combined
        .lines()
        .find_map(|line| {
            line.split_once("cpu.max=")
                .and_then(|(_, tail)| tail.split_once(" (bound)"))
                .map(|(value, _)| value)
        })
        .expect("outer cgroup audit did not report a bound cpu.max");
    let parts: Vec<i64> = cpu_max
        .split_whitespace()
        .map(|part| part.parse::<i64>().expect("numeric cpu.max field"))
        .collect();
    assert_eq!(parts.len(), 2, "unexpected cpu.max: {cpu_max}");
    assert_eq!(
        parts[0],
        2 * parts[1],
        "outer cpu.max must encode exactly two CPUs: {cpu_max}"
    );
    assert!(!combined.contains("invalid JSON"), "{combined}");
    assert!(
        stdout.contains("stress.singleton: 3/3 passed"),
        "{combined}"
    );
    assert!(
        stdout.contains(
            "maximum concurrent steps: 3 (--max-steps 3; --max-cpus 2 CPU target/per-step ceiling)"
        ),
        "{combined}"
    );
}
