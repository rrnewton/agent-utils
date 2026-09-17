//! The terminal step record must say WHY the step ended that way, not only that it did.
//!
//! `step_end` exists so the journal alone can answer what a run was doing, and it is the stream
//! that survives when the checkout and the console are both gone. It carried `ok` and `aborted`
//! and no cause, so a cancelled step could be COUNTED and never NAMED — and an unknown nobody can
//! name is an unknown nobody can drive down.
//!
//! The two cancellations are the case that makes this more than a missing field. They call for
//! completely different follow-up, and the outcome's `reason` used to say eager-exit for both, so
//! recording it verbatim would have put a confidently WRONG cause in the record: a reader
//! hunting for a failing peer that does not exist. Every case below is a real run.
//!
//! | case | asserts |
//! |---|---|
//! | `a_step_that_fails_on_its_own` | the record names its own exit |
//! | `a_peer_failure_abort` | the record names eager-exit |
//! | `an_outer_budget_abort` | the record names the run budget, and NOT eager-exit |
//! | `a_passing_step` (control) | no `reason` key at all, rather than an empty one |

use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "dagrun_reason_journal_{name}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Fixture { dir }
    }

    fn write_dag(&self, name: &str, steps: &str) -> PathBuf {
        let path = self.dir.join(format!("{name}.json"));
        std::fs::write(&path, format!(r#"{{"steps":[{steps}]}}"#)).unwrap();
        path
    }

    fn logs(&self) -> PathBuf {
        self.dir.join("logs")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Run the real binary with its journal directed into the fixture, and hand back every
/// `step_end` record as (step, whole line).
fn step_end_records(fx: &Fixture, dag: &Path, jobs: &str, extra: &[&str]) -> Vec<(String, String)> {
    let bin = env!("CARGO_BIN_EXE_dagrun");
    let mut args: Vec<&str> = vec![
        "run",
        "--dag",
        dag.to_str().unwrap(),
        "-j",
        jobs,
        "--unsafe-no-cgroups",
        "--no-profile",
        "--no-profile-feedback",
    ];
    args.extend_from_slice(extra);
    let output = Command::new(bin)
        .args(args)
        .env("DAGRUN_LOG_DIR", fx.logs())
        .output()
        .expect("failed to run dagrun");
    let journal = std::fs::read_to_string(fx.logs().join("journal.jsonl")).unwrap_or_else(|e| {
        panic!(
            "no journal at {}: {e}\n{}{}",
            fx.logs().join("journal.jsonl").display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    journal
        .lines()
        .filter(|line| line.contains(r#""event":"step_end""#))
        .map(|line| {
            let step = line
                .split(r#""step":""#)
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .unwrap_or_default()
                .to_string();
            (step, line.to_string())
        })
        .collect()
}

fn record_for<'a>(records: &'a [(String, String)], step: &str) -> &'a str {
    records
        .iter()
        .find(|(tag, _)| tag == step)
        .map(|(_, line)| line.as_str())
        .unwrap_or_else(|| panic!("no step_end for {step} in {records:?}"))
}

#[test]
fn a_step_that_fails_on_its_own_records_its_own_cause() {
    let fx = Fixture::new("fail");
    let dag = fx.write_dag(
        "fail",
        r#"{"group":"a","job":"boom","cmd":"exit 3","timeout":30,"cpu_timeout":60}"#,
    );
    let records = step_end_records(&fx, &dag, "1", &[]);
    let record = record_for(&records, "a.boom");
    assert!(
        record.contains(r#""ok":"false""#),
        "the fixture must really have failed: {record}"
    );
    assert!(
        record.contains(r#""reason":"exit 3""#),
        "a failed step must record the cause it already computed: {record}"
    );
}

#[test]
fn a_peer_failure_abort_records_eager_exit() {
    let fx = Fixture::new("peer");
    // `slow` is still running when `boom` fails, so eager-exit cancels it. Both are ready at
    // once, which is what makes the cancellation a peer cancellation rather than a skip.
    let dag = fx.write_dag(
        "peer",
        concat!(
            r#"{"group":"a","job":"slow","cmd":"sleep 20","timeout":30,"cpu_timeout":60},"#,
            r#"{"group":"a","job":"boom","cmd":"exit 1","timeout":30,"cpu_timeout":60}"#
        ),
    );
    let records = step_end_records(&fx, &dag, "2", &[]);
    let record = record_for(&records, "a.slow");
    assert!(
        record.contains(r#""aborted":"true""#),
        "the fixture must really have been cancelled: {record}"
    );
    assert!(
        record.contains("eager-exit after another step failed"),
        "a peer-failure cancellation must name the peer failure: {record}"
    );
    assert!(
        !record.contains("OUTER run budget"),
        "a peer-failure cancellation must not blame the run budget: {record}"
    );
}

#[test]
fn an_outer_budget_abort_records_the_budget_and_never_a_peer() {
    let fx = Fixture::new("outer");
    // No step here fails. The only thing that ends the run is its own outer budget, so any
    // mention of a failing peer in the record would be describing something that never happened.
    // A predecessor spends most of the budget so the run bound fires while the successor is
    // still inside its OWN step budget. The runner refuses to start a DAG whose step budgets
    // equal or exceed the run budget, precisely so a cut stays attributable to a node.
    let dag = fx.write_dag(
        "outer",
        concat!(
            r#"{"group":"a","job":"first","cmd":"sleep 2","timeout":3,"cpu_timeout":60},"#,
            r#"{"group":"a","job":"long","deps":["a.first"],"cmd":"sleep 30","timeout":3,"cpu_timeout":60}"#
        ),
    );
    let records = step_end_records(&fx, &dag, "1", &["--run-timeout", "4"]);
    let record = record_for(&records, "a.long");
    assert!(
        record.contains(r#""aborted":"true""#),
        "the fixture must really have been cancelled: {record}"
    );
    assert!(
        record.contains("cut short by the OUTER run budget"),
        "a run-budget cut must name the run budget: {record}"
    );
    // The control that makes the assertion above mean something. Before the reason distinguished
    // the two cancellations, this record would have carried the eager-exit wording and sent a
    // reader looking for a failing step that does not exist in this DAG.
    assert!(
        !record.contains("eager-exit"),
        "a run-budget cut must not claim a peer failed: {record}"
    );
}

#[test]
fn a_passing_step_carries_no_reason_key_at_all() {
    let fx = Fixture::new("pass");
    let dag = fx.write_dag(
        "pass",
        r#"{"group":"a","job":"fine","cmd":"true","timeout":30,"cpu_timeout":60}"#,
    );
    let records = step_end_records(&fx, &dag, "1", &[]);
    let record = record_for(&records, "a.fine");
    assert!(
        record.contains(r#""ok":"true""#),
        "the control must really have passed: {record}"
    );
    // Absent rather than empty, for the same reason an unset budget is absent: `"reason":""` in
    // the record would read as a cause that was looked for and not found.
    assert!(
        !record.contains(r#""reason""#),
        "a passing step must not carry an empty reason: {record}"
    );
}
