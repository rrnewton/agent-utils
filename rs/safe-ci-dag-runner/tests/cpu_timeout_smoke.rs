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

/// A step whose descendants burn CPU on FOUR cores at once, so consumed CPU-seconds and elapsed
/// wall seconds are separated by roughly 4x rather than being the same number. That separation is
/// the whole point: it is what lets the journal assertion below tell the two quantities apart.
const PARALLEL_BURN_DAG: &str = r#"{"steps": [{"group": "cpu", "job": "burn4",
  "desc": "burn four cores past the budget",
  "cmd": "for i in 1 2 3 4; do (while :; do :; done) & done; wait",
  "cpu_timeout": 2, "timeout": 60}]}"#;

/// The termination record must say WHICH quantity crossed WHICH limit.
///
/// The CPU guard compares cgroup `cpu.stat` CPU-seconds against a CPU-second budget, correctly.
/// The RECORD of that decision used to carry an unlabelled `elapsed_s` -- which was WALL -- next
/// to a `limit_s` that was CPU-seconds. Unlabelled and side by side, the natural reading is that
/// the two are comparable; they are not, and a CPU breach could be quoted as having consumed more
/// seconds than its own run's whole CPU rollup contained.
#[test]
fn a_cpu_breach_journals_the_cpu_seconds_it_compared_not_the_wall_clock() {
    if std::thread::available_parallelism().map_or(0, |n| n.get()) < 4 {
        eprintln!(
            "SKIP a_cpu_breach_journals_the_cpu_seconds_it_compared_not_the_wall_clock: fewer \
             than four usable cores, so CPU-seconds and wall seconds cannot be told apart here."
        );
        return;
    }
    let bin = env!("CARGO_BIN_EXE_safe-ci-dag-runner");

    let dir = std::env::temp_dir().join(format!("scdr_cpu_units_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dag = dir.join("burn4.json");
    let mut f = std::fs::File::create(&dag).unwrap();
    f.write_all(PARALLEL_BURN_DAG.as_bytes()).unwrap();
    drop(f);
    let evidence = dir.join("evidence");

    let output = Command::new(bin)
        .args(["run", "--dag", dag.to_str().unwrap(), "-q", "--no-profile"])
        .env("SAFE_CI_DAG_RUNNER_LOG_DIR", &evidence)
        .output()
        .expect("failed to spawn the built binary");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.code() == Some(3) {
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!(
            "SKIP a_cpu_breach_journals_the_cpu_seconds_it_compared_not_the_wall_clock: cgroup \
             boxing is unavailable in this environment. Details:\n{stderr}"
        );
        return;
    }

    let journal = std::fs::read_to_string(evidence.join("journal.jsonl"))
        .expect("the run should have written a journal");
    let _ = std::fs::remove_dir_all(&dir);
    let record = journal
        .lines()
        .find(|line| line.contains(r#""event":"cpu_timeout""#))
        .unwrap_or_else(|| panic!("no cpu_timeout record in the journal:\n{journal}"))
        .to_string();

    assert!(
        record.contains(r#""unit":"cpu_seconds""#),
        "a CPU breach must name its unit:\n{record}"
    );
    let measured = json_number(&record, "measured_s");
    let wall = json_number(&record, "wall_elapsed_s");
    let limit = json_number(&record, "limit_s");
    assert_eq!(limit, 2.0, "the budget under test:\n{record}");
    assert!(
        measured >= limit,
        "the compared quantity must be the reading that crossed the budget:\n{record}"
    );
    // Four cores burning at once: consumed CPU outruns elapsed wall. Passing wall here -- the
    // defect -- would make these two equal, so the strict inequality is what catches it.
    assert!(
        measured > wall,
        "measured_s ({measured}) must be the CPU reading, not the wall clock ({wall}); four \
         concurrent burners consume CPU faster than wall time elapses:\n{record}"
    );

    // The TERMINAL record must be able to stand alone: a hard kill destroys the end-of-run
    // profile flush, and then this is the only thing left that can say what the step consumed
    // against the budget it was given.
    let end = journal
        .lines()
        .find(|line| line.contains(r#""event":"step_end""#))
        .unwrap_or_else(|| panic!("no step_end record in the journal:\n{journal}"))
        .to_string();
    assert_eq!(json_number(&end, "cpu_limit_s"), 2.0, "{end}");
    assert_eq!(json_number(&end, "wall_limit_s"), 60.0, "{end}");
    assert!(
        json_number(&end, "cpu_usage_usec") >= 2_000_000.0,
        "the journalled cgroup CPU total must cover the budget the step just blew:\n{end}"
    );
    assert!(json_number(&end, "wall_elapsed_s") > 0.0, "{end}");
}

/// Pull one `"key":"<number>"` out of a flat JSONL record without taking a JSON dependency on a
/// test target. Panics with the whole record when the key is absent, which is the failure a
/// missing field should produce.
fn json_number(record: &str, key: &str) -> f64 {
    let needle = format!("\"{key}\":\"");
    let start = record
        .find(&needle)
        .unwrap_or_else(|| panic!("no {key} in record:\n{record}"))
        + needle.len();
    let rest = &record[start..];
    let end = rest.find('"').expect("unterminated JSON string");
    rest[..end]
        .parse()
        .unwrap_or_else(|_| panic!("{key} is not a number in record:\n{record}"))
}
