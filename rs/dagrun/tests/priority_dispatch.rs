//! Dispatch order by step `priority` (#860): with fewer slots than ready
//! steps, higher priority starts first and longest-estimated-first (LPT)
//! breaks ties; with every priority at the default 0, today's LPT order is
//! kept exactly. Mirrors py/tests/test_scheduler.py's priority tests.

use std::io::Write;
use std::process::Command;

fn run_order(label: &str, steps_json: &str, jobs: &str) -> (Option<i32>, Vec<String>, String) {
    let bin = env!("CARGO_BIN_EXE_dagrun");
    let dir =
        std::env::temp_dir().join(format!("dagrun_priority_{}_{}", label, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("order.txt");
    let dag_json = steps_json.replace("__LOG__", &log.to_string_lossy());
    let dag = dir.join("dag.json");
    let mut f = std::fs::File::create(&dag).unwrap();
    f.write_all(dag_json.as_bytes()).unwrap();
    drop(f);
    let output = Command::new(bin)
        .args([
            "run",
            "--dag",
            dag.to_str().unwrap(),
            "--jobs",
            jobs,
            "--unsafe-no-cgroups",
            "-q",
            "--no-profile",
        ])
        .current_dir(&dir)
        .output()
        .expect("failed to spawn the built binary");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let order = std::fs::read_to_string(&log)
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    (output.status.code(), order, combined)
}

#[test]
fn priority_dispatches_before_lpt_when_slots_are_short() {
    let steps = r#"{"steps": [
      {"group": "g", "job": "long-low", "cmd": "echo long-low >> __LOG__",
       "hint": {"est_duration_s": 100}},
      {"group": "g", "job": "short-high", "cmd": "echo short-high >> __LOG__",
       "priority": 5, "hint": {"est_duration_s": 1}},
      {"group": "g", "job": "long-high", "cmd": "echo long-high >> __LOG__",
       "priority": 5, "hint": {"est_duration_s": 50}},
      {"group": "g", "job": "short-low", "cmd": "echo short-low >> __LOG__",
       "hint": {"est_duration_s": 10}}]}"#;
    let (code, order, out) = run_order("short", steps, "1");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(order, ["long-high", "short-high", "long-low", "short-low"]);
}

#[test]
fn all_default_priorities_keep_lpt_order_exactly() {
    let steps = r#"{"steps": [
      {"group": "g", "job": "mid", "cmd": "echo mid >> __LOG__",
       "hint": {"est_duration_s": 10}},
      {"group": "g", "job": "longest", "cmd": "echo longest >> __LOG__",
       "hint": {"est_duration_s": 100}},
      {"group": "g", "job": "unestimated", "cmd": "echo unestimated >> __LOG__"}]}"#;
    let (code, order, out) = run_order("default", steps, "1");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(order, ["longest", "mid", "unestimated"]);
}

#[test]
fn enough_slots_start_everything_at_once() {
    let steps = r#"{"steps": [
      {"group": "g", "job": "a", "cmd": "echo start-a >> __LOG__; sleep 0.5; echo end-a >> __LOG__"},
      {"group": "g", "job": "b", "cmd": "echo start-b >> __LOG__; sleep 0.5; echo end-b >> __LOG__", "priority": 9},
      {"group": "g", "job": "c", "cmd": "echo start-c >> __LOG__; sleep 0.5; echo end-c >> __LOG__", "priority": 3}]}"#;
    let (code, order, out) = run_order("wide", steps, "8");
    assert_eq!(code, Some(0), "{out}");
    assert!(
        order.iter().take(3).all(|line| line.starts_with("start-")),
        "all steps must start before any ends: {order:?}"
    );
}
