//! Seconds-long integration of the DAG runner with controlled, cargo-test, and nextest suites.
//!
//! The controlled suite is the strong path: explicit boundaries name every live test and retain
//! elapsed time. Third-party runners use only observable process facts. Nextest's one-process-per-
//! test `--exact TEST` argv binds a process to a test; ordinary cargo test's shared binary does not.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "scdr_test_runner_integration_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn run(&self, name: &str, command: &str, timeout: u64) -> (Output, String, String) {
        let dag = self.dir.join(format!("{name}.json"));
        let evidence = self.dir.join(format!("{name}-evidence"));
        std::fs::write(
            &dag,
            serde_json::to_vec(&serde_json::json!({
                "steps": [{
                    "group": "tests",
                    "job": name,
                    "desc": name,
                    "cmd": command,
                    "timeout": timeout,
                    "cpu_timeout": 600
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_safe-ci-dag-runner"))
            .args([
                "run",
                "--dag",
                dag.to_str().unwrap(),
                "-j",
                "1",
                "--no-profile",
                "--no-profile-feedback",
                "--unsafe-no-cgroups",
            ])
            .env("SAFE_CI_DAG_RUNNER_LOG_DIR", &evidence)
            .output()
            .unwrap();
        let console = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let journal = std::fs::read_to_string(evidence.join("journal.jsonl")).unwrap();
        (output, console, journal)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn probe_manifest() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-runner-probe/Cargo.toml")
}

#[test]
fn controlled_and_third_party_runners_leave_attributable_kill_evidence() {
    let fixture = Fixture::new();

    // OUR runner convention: two tests are concurrently live. Wall starts first, so it is the
    // likely culprit, but the complete 2/2 live set and both elapsed times remain visible. The
    // TERM trap proves the gentle phase ran and flushed output before hard escalation.
    let controlled = r#"trap 'printf '"'"'graceful-flush-marker\n'"'"' >&2; exit 0' TERM;
printf '##TEST-START suite::wall_hang\n'; sleep 60 & wall=$!;
sleep 0.2; printf '##TEST-START suite::cpu_burn\n';
bash -c 'while :; do :; done' & cpu=$!; wait"#;
    let (output, console, journal) = fixture.run("controlled", controlled, 2);
    assert_ne!(output.status.code(), Some(0), "{console}");
    assert!(
        console.contains("likely culprit test suite::wall_hang"),
        "longest-running declared test was not named:\n{console}"
    );
    assert!(
        console.contains("2 test(s) in flight")
            && console.contains("suite::wall_hang")
            && console.contains("suite::cpu_burn"),
        "complete live-test snapshot missing:\n{console}"
    );
    assert!(
        console.contains("signature=cpu-burning") && console.contains("signature=wall-stalled"),
        "CPU-burning and wall-stalled process signatures must remain distinct:\n{console}"
    );
    assert!(
        console.contains("graceful-flush-marker"),
        "SIGTERM did not get a chance to flush evidence before escalation:\n{console}"
    );
    assert!(
        journal.contains("\"in_flight_count\":\"2\"")
            && journal.contains("\"event\":\"process_snapshot\""),
        "durable pre-kill snapshot missing:\n{journal}"
    );

    let manifest = probe_manifest();
    let built = Command::new("cargo")
        .args([
            "test",
            "--manifest-path",
            manifest.to_str().unwrap(),
            "--no-run",
        ])
        .status()
        .unwrap();
    assert!(
        built.success(),
        "could not prebuild third-party runner probe"
    );

    // Ordinary cargo test runs several tests inside one shared binary. Its process argv does not
    // contain a live test id, so the process snapshot MUST NOT manufacture one.
    let cargo_command = format!(
        "DAG_RUNNER_PROBE_MODE=wall cargo test --manifest-path '{}' -- --test-threads=4 --nocapture",
        manifest.display()
    );
    let (output, console, journal) = fixture.run("cargo_test", &cargo_command, 2);
    assert_ne!(output.status.code(), Some(0), "{console}");
    for row in journal
        .lines()
        .filter(|line| line.contains("\"event\":\"process_snapshot\""))
    {
        assert!(
            row.contains("\"test\":\"\""),
            "cargo test shared process was falsely bound to a test:\n{row}"
        );
    }

    // Nextest may not be installed in a minimal package consumer. When present, its exact child
    // argv is a direct binding and the timed-out test must be named without parsing a guess from
    // prose. This repository's full gate installs nextest and therefore exercises this branch.
    let nextest = Command::new("cargo")
        .args(["nextest", "--version"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !nextest {
        eprintln!("SKIP nextest exact-process bracket: cargo-nextest is not installed");
        return;
    }
    let nextest_command = format!(
        "DAG_RUNNER_PROBE_MODE=wall cargo nextest run --manifest-path '{}' --status-level all --final-status-level all",
        manifest.display()
    );
    let (output, console, journal) = fixture.run("nextest", &nextest_command, 2);
    assert_ne!(output.status.code(), Some(0), "{console}");
    assert!(
        console.contains("culprit test tests::planted_wall_hang")
            && console.contains("test=tests::planted_wall_hang"),
        "nextest's exact child process was not attributed:\n{console}"
    );
    assert!(
        journal.contains("\"test\":\"tests::planted_wall_hang\"")
            && journal.contains("\"test_basis\":\"libtest --exact process argv\""),
        "nextest binding was not durable:\n{journal}"
    );
}
