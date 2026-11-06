//! Artifact-ready early release: a dependent declaring `artifact_deps`
//! starts when the producer signals, while the producer keeps running;
//! completion dependents still wait for exit. Failure semantics are
//! fail-closed. Mirrors py/tests/test_artifact_ready.py.

use std::io::Write;
use std::process::Command;

struct Run {
    code: Option<i32>,
    out: String,
    dir: std::path::PathBuf,
}

fn run_dag(label: &str, steps_json: &str) -> Run {
    let bin = env!("CARGO_BIN_EXE_dagrun");
    let dir =
        std::env::temp_dir().join(format!("dagrun_artifact_{}_{}", label, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dag_json = steps_json
        .replace("__DIR__", &dir.to_string_lossy())
        .replace("__BIN__", bin);
    let dag = dir.join("dag.json");
    let mut f = std::fs::File::create(&dag).unwrap();
    f.write_all(dag_json.as_bytes()).unwrap();
    drop(f);
    let output = Command::new(bin)
        .args([
            "run",
            "--dag",
            dag.to_str().unwrap(),
            "--unsafe-no-cgroups",
            "-q",
            "--no-profile",
        ])
        .current_dir(&dir)
        .output()
        .expect("failed to spawn the built binary");
    let out = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Run {
        code: output.status.code(),
        out,
        dir,
    }
}

fn stamp(run: &Run, name: &str) -> Option<f64> {
    std::fs::read_to_string(run.dir.join(name))
        .ok()
        .and_then(|text| text.trim().parse().ok())
}

const PRODUCER_OK: &str = r#"{"group":"build","job":"P","cmd":"date +%s.%N > __DIR__/p_start; sleep 2; printf x > \"$DAGRUN_ARTIFACT_READY_DIR/build.P/x.tmp\"; mv \"$DAGRUN_ARTIFACT_READY_DIR/build.P/x.tmp\" \"$DAGRUN_ARTIFACT_READY_DIR/build.P/x\"; sleep 6; date +%s.%N > __DIR__/p_end","publishes":[{"name":"x"}]}"#;
const DEPENDENT: &str = r#"{"group":"e2e","job":"D","cmd":"date +%s.%N > __DIR__/d_start; sleep 1; date +%s.%N > __DIR__/d_end","artifact_deps":[{"step":"build.P","artifact":"x"}]}"#;
const CONTROL: &str =
    r#"{"group":"gate","job":"C","cmd":"date +%s.%N > __DIR__/c_start","deps":["build.P"]}"#;

#[test]
fn dependent_releases_at_signal_while_producer_keeps_running() {
    let dag = format!("{{\"steps\":[{PRODUCER_OK},{DEPENDENT},{CONTROL}]}}");
    let run = run_dag("release", &dag);
    assert_eq!(run.code, Some(0), "{}", run.out);
    let p = stamp(&run, "p_start").unwrap();
    let p_end = stamp(&run, "p_end").unwrap();
    let d = stamp(&run, "d_start").unwrap();
    let d_end = stamp(&run, "d_end").unwrap();
    let c = stamp(&run, "c_start").unwrap();
    assert!(
        (1.5..4.0).contains(&(d - p)),
        "D started {:+.2}s after P, expected ~2s: {}",
        d - p,
        run.out
    );
    assert!(d_end < p_end, "D must finish before P exits");
    assert!(c >= p_end - 0.05, "C must wait for P to exit");
    let _ = std::fs::remove_dir_all(&run.dir);
}

#[test]
fn helper_command_signals_for_the_calling_step() {
    let producer = r#"{"group":"build","job":"P","cmd":"sleep 1; __BIN__ artifact-ready x; sleep 2; date +%s.%N > __DIR__/p_end","publishes":[{"name":"x"}]}"#;
    let dag = format!("{{\"steps\":[{producer},{DEPENDENT}]}}");
    let run = run_dag("helper", &dag);
    assert_eq!(run.code, Some(0), "{}", run.out);
    assert!(stamp(&run, "d_start").unwrap() < stamp(&run, "p_end").unwrap());
    let _ = std::fs::remove_dir_all(&run.dir);
}

#[test]
fn producer_failure_after_signal_discards_released_dependent() {
    let producer = r#"{"group":"build","job":"P","cmd":"sleep 2; printf x > \"$DAGRUN_ARTIFACT_READY_DIR/build.P/x.tmp\"; mv \"$DAGRUN_ARTIFACT_READY_DIR/build.P/x.tmp\" \"$DAGRUN_ARTIFACT_READY_DIR/build.P/x\"; sleep 6; exit 1","publishes":[{"name":"x"}]}"#;
    let dag = format!("{{\"steps\":[{producer},{DEPENDENT},{CONTROL}]}}");
    let run = run_dag("failure", &dag);
    assert_eq!(run.code, Some(1), "{}", run.out);
    assert!(
        stamp(&run, "d_start").is_some(),
        "D should have been released"
    );
    assert!(stamp(&run, "c_start").is_none(), "C must never start");
    assert!(
        run.out.contains("2 failed") && run.out.contains("build.P"),
        "run must fail naming the producer: {}",
        run.out
    );
    let _ = std::fs::remove_dir_all(&run.dir);
}

#[test]
fn missing_signal_fails_closed_and_dependent_never_starts() {
    let producer = r#"{"group":"build","job":"P","cmd":"sleep 1","publishes":[{"name":"x"}]}"#;
    let dag = format!("{{\"steps\":[{producer},{DEPENDENT}]}}");
    let run = run_dag("missing", &dag);
    assert_eq!(run.code, Some(1), "{}", run.out);
    assert!(stamp(&run, "d_start").is_none(), "D must never start");
    assert!(
        run.out.contains("ARTIFACT NOT SIGNALLED") && run.out.contains("build.P"),
        "failure must name the unsignalled artifact's producer: {}",
        run.out
    );
    let _ = std::fs::remove_dir_all(&run.dir);
}

#[test]
fn invalid_artifact_graphs_are_refused_at_load() {
    for (label, steps, needle) in [
        (
            "unknown-artifact",
            r#"{"steps":[{"group":"build","job":"P","cmd":"true","publishes":[{"name":"x"}]},{"group":"e2e","job":"D","cmd":"true","artifact_deps":[{"step":"build.P","artifact":"y"}]}]}"#,
            "does not publish",
        ),
        (
            "unknown-step",
            r#"{"steps":[{"group":"e2e","job":"D","cmd":"true","artifact_deps":[{"step":"build.P","artifact":"x"}]}]}"#,
            "which no step declares",
        ),
        (
            "mixed-cycle",
            r#"{"steps":[{"group":"a","job":"one","cmd":"true","publishes":[{"name":"x"}],"deps":["b.two"]},{"group":"b","job":"two","cmd":"true","artifact_deps":[{"step":"a.one","artifact":"x"}]}]}"#,
            "dependency cycle",
        ),
        (
            "duplicate-publish",
            r#"{"steps":[{"group":"build","job":"P","cmd":"true","publishes":[{"name":"x"},{"name":"x"}]}]}"#,
            "more than once",
        ),
    ] {
        let run = run_dag(label, steps);
        assert_eq!(run.code, Some(2), "{label}: {}", run.out);
        assert!(
            run.out.contains(needle),
            "{label}: expected {needle:?} in: {}",
            run.out
        );
        let _ = std::fs::remove_dir_all(&run.dir);
    }
}
