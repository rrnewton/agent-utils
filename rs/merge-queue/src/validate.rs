//! Running a selection of tests against one exact commit.
//!
//! The interface is a selection in, per-test results out. Two runner kinds are built in:
//!
//! * `command`: a shell template. It reads the selection from `$MQ_TESTS` (space separated) or
//!   `$MQ_TESTS_FILE` (one per line; `$MQ_SELECTION` is `all` or `tests`) and writes one JSON object
//!   per line to `$MQ_RESULTS`: `{"id": "step::case", "outcome": "pass|fail|skip|error",
//!   "duration_ms": 12}`. A command that writes no results still works, but its exit status is
//!   then the only signal: every selected test inherits it and the run is marked unattributed.
//! * `dagrun`: a dagrun DAG. Test IDs are step names; the selection becomes `--selected`, which
//!   also runs each step's dependencies, and per-step verdicts come from the run journal.
//!
//! Both share the selection policy: a shell command printing test IDs for the checkout at
//! `$MQ_HEAD` relative to `$MQ_BASE`, or `all` when none is configured.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::state::ValidatorConfig;
use crate::tests::{Outcome, Selection, TestId, TestResult, TestResults};

/// Test ID used when a runner failed without attributing the failure to any test.
pub const UNATTRIBUTED: &str = "(unattributed)";

/// One finished validation.
#[derive(Clone, Debug)]
pub struct Ran {
    /// Per-test results, with every selected-but-unreported test counted as an error.
    pub results: TestResults,
    /// Runner exit status (`None` when killed by the timeout).
    pub exit: Option<i32>,
    /// Wall time.
    pub elapsed: Duration,
    /// Combined runner output.
    pub log: PathBuf,
    /// The queue cancelled the run (its candidate was discarded); its results mean nothing.
    pub cancelled: bool,
}

impl Ran {
    /// Whether the run is green: every result non-blocking and the runner exited zero.
    pub fn green(&self) -> bool {
        !self.cancelled && self.exit == Some(0) && self.results.green()
    }

    /// Failed tests. A red run with no attributed failure reports [`UNATTRIBUTED`].
    pub fn failed(&self) -> BTreeSet<TestId> {
        let failed = self.results.failed();
        if failed.is_empty() && !self.green() {
            return [TestId::step(UNATTRIBUTED)].into();
        }
        failed
    }
}

/// The selection a policy chose, and the policy's name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Chosen {
    /// Selected tests.
    pub selection: Selection,
    /// Policy name.
    pub policy: String,
}

/// A configured validator.
pub trait Validator: Sync {
    /// Ask the selection policy which tests the change from `base` to the checkout's HEAD needs.
    fn select(&self, checkout: &Path, base: &str, head: &str) -> Result<Chosen, String>;
    /// Run `selection` in `checkout`, writing logs under `out`. Setting `cancel` stops the run
    /// early (its process group is killed) and marks the result cancelled.
    fn run(
        &self,
        checkout: &Path,
        selection: &Selection,
        out: &Path,
        cancel: &AtomicBool,
    ) -> Result<Ran, String>;
}

/// Build the validator a configuration names.
pub fn from_config(config: &ValidatorConfig) -> Result<Box<dyn Validator>, String> {
    match config.kind.as_str() {
        "command" => {
            if config.run.trim().is_empty() {
                return Err("validator.kind is `command` but validator.run is empty".to_owned());
            }
            Ok(Box::new(CommandValidator(config.clone())))
        }
        "dagrun" => {
            if config.dag.trim().is_empty() {
                return Err("validator.kind is `dagrun` but validator.dag is empty".to_owned());
            }
            Ok(Box::new(DagrunValidator(config.clone())))
        }
        "" => Err("no validator configured: set validator.kind in config.json".to_owned()),
        other => Err(format!(
            "unknown validator.kind {other:?}; use `command` or `dagrun`"
        )),
    }
}

fn parse_ids(text: &str, prefix: &str) -> Result<Selection, String> {
    let mut tests = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(id) = line.strip_prefix(prefix) else {
            continue;
        };
        let id = id.trim();
        if id.is_empty() || id.starts_with('#') {
            continue;
        }
        if id == "all" {
            return Ok(Selection::All);
        }
        tests.insert(TestId::parse(id)?);
    }
    Ok(Selection::Tests(tests))
}

fn shared_select(
    config: &ValidatorConfig,
    checkout: &Path,
    base: &str,
    head: &str,
) -> Result<Chosen, String> {
    if config.select.trim().is_empty() {
        return Ok(Chosen {
            selection: Selection::All,
            policy: "all".to_owned(),
        });
    }
    let output = Command::new("sh")
        .arg("-c")
        .arg(&config.select)
        .current_dir(checkout)
        .env("MQ_BASE", base)
        .env("MQ_HEAD", head)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run validator.select: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "validator.select failed ({}): {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let selection = parse_ids(&String::from_utf8_lossy(&output.stdout), &config.id_prefix)?;
    let policy = if config.policy.is_empty() {
        "select-command".to_owned()
    } else {
        config.policy.clone()
    };
    Ok(Chosen { selection, policy })
}

/// Run a shell command under the admission prefix with a timeout; output goes to `log`.
fn run_logged(
    config: &ValidatorConfig,
    argv: &[String],
    checkout: &Path,
    env: &[(&str, String)],
    log: &Path,
    cancel: &AtomicBool,
) -> Result<(Option<i32>, Duration, bool), String> {
    let file = std::fs::File::create(log)
        .map_err(|error| format!("cannot create {}: {error}", log.display()))?;
    let err = file
        .try_clone()
        .map_err(|error| format!("cannot clone log handle: {error}"))?;
    let full: Vec<String> = config
        .admission
        .iter()
        .cloned()
        .chain(argv.iter().cloned())
        .collect();
    let mut command = Command::new(&full[0]);
    command
        .args(&full[1..])
        .current_dir(checkout)
        .stdin(Stdio::null())
        .stdout(file)
        .stderr(err);
    for (key, value) in env {
        command.env(key, value);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let started = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start {}: {error}", full[0]))?;
    let limit = (config.timeout_secs > 0).then(|| Duration::from_secs(config.timeout_secs));
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("cannot wait for validation: {error}"))?
        {
            return Ok((status.code(), started.elapsed(), false));
        }
        let cancelled = cancel.load(Ordering::Relaxed);
        let timed_out = limit.is_some_and(|limit| started.elapsed() > limit);
        if cancelled || timed_out {
            let _ = Command::new("kill")
                .args(["-TERM", &format!("-{}", child.id())])
                .status();
            let grace = Instant::now();
            while grace.elapsed() < Duration::from_secs(2) {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = Command::new("kill")
                .args(["-KILL", &format!("-{}", child.id())])
                .status();
            let _ = child.kill();
            let _ = child.wait();
            if let Ok(mut log) = std::fs::OpenOptions::new().append(true).open(log) {
                let why = if cancelled {
                    "cancelled by the queue".to_owned()
                } else {
                    format!("killed after {}s", config.timeout_secs)
                };
                let _ = writeln!(log, "merge-queue: validation {why}");
            }
            return Ok((None, started.elapsed(), cancelled));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A shell-template runner with a JSON-lines results contract.
pub struct CommandValidator(pub ValidatorConfig);

#[derive(Deserialize)]
struct ResultLine {
    id: String,
    outcome: String,
    #[serde(default)]
    duration_ms: Option<u64>,
}

impl Validator for CommandValidator {
    fn select(&self, checkout: &Path, base: &str, head: &str) -> Result<Chosen, String> {
        shared_select(&self.0, checkout, base, head)
    }

    fn run(
        &self,
        checkout: &Path,
        selection: &Selection,
        out: &Path,
        cancel: &AtomicBool,
    ) -> Result<Ran, String> {
        std::fs::create_dir_all(out)
            .map_err(|error| format!("cannot create {}: {error}", out.display()))?;
        let results_path = out.join("results.jsonl");
        let tests_path = out.join("tests.txt");
        let _ = std::fs::remove_file(&results_path);
        let listed: Vec<String> = selection
            .tests()
            .map(|tests| tests.iter().map(ToString::to_string).collect())
            .unwrap_or_default();
        std::fs::write(
            &tests_path,
            listed
                .iter()
                .map(|id| format!("{id}\n"))
                .collect::<String>(),
        )
        .map_err(|error| format!("cannot write {}: {error}", tests_path.display()))?;
        let env = [
            (
                "MQ_SELECTION",
                if selection.is_all() { "all" } else { "tests" }.to_owned(),
            ),
            ("MQ_TESTS", listed.join(" ")),
            ("MQ_TESTS_FILE", tests_path.display().to_string()),
            ("MQ_RESULTS", results_path.display().to_string()),
        ];
        let log = out.join("run.log");
        let argv = vec!["sh".to_owned(), "-c".to_owned(), self.0.run.clone()];
        let (exit, elapsed, cancelled) = run_logged(&self.0, &argv, checkout, &env, &log, cancel)?;
        let mut results = TestResults::default();
        if let Ok(text) = std::fs::read_to_string(&results_path) {
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                let parsed: ResultLine = serde_json::from_str(line).map_err(|error| {
                    format!("bad line in {}: {error}: {line}", results_path.display())
                })?;
                let outcome = Outcome::parse(&parsed.outcome).ok_or_else(|| {
                    format!(
                        "unknown outcome {:?} in {}",
                        parsed.outcome,
                        results_path.display()
                    )
                })?;
                results.record(TestResult {
                    id: TestId::parse(&parsed.id)?,
                    outcome,
                    duration_ms: parsed.duration_ms,
                });
            }
        }
        if results.results.is_empty() {
            results.unattributed = true;
            let outcome = if exit == Some(0) {
                Outcome::Pass
            } else {
                Outcome::Error
            };
            let ids: Vec<TestId> = match selection.tests() {
                Some(tests) => tests.iter().cloned().collect(),
                None => vec![TestId::step(UNATTRIBUTED)],
            };
            for id in ids {
                results.record(TestResult {
                    id,
                    outcome,
                    duration_ms: None,
                });
            }
        }
        results.require(selection);
        Ok(Ran {
            results,
            exit,
            elapsed,
            log,
            cancelled,
        })
    }
}

/// A dagrun DAG runner: steps are tests.
pub struct DagrunValidator(pub ValidatorConfig);

#[derive(Deserialize)]
struct JournalLine {
    #[serde(default)]
    event: String,
    #[serde(default)]
    step: String,
    #[serde(default)]
    ok: Option<bool>,
    #[serde(default)]
    wall_elapsed_s: Option<String>,
}

/// Create `dir` readable only by its owner; dagrun writes its journal only into a private,
/// owned directory.
fn private_dir(dir: &Path) -> Result<(), String> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("cannot make {} private: {error}", dir.display()))?;
    }
    Ok(())
}

/// Per-step results from dagrun's printed verdict lines: `[step] ✓ PASS ...`,
/// `[step] ✗ FAIL ...`, `[step] SKIPPED ...`.
pub fn parse_step_lines(text: &str) -> TestResults {
    let mut results = TestResults::default();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix('[') else {
            continue;
        };
        let Some((step, verdict)) = rest.split_once("] ") else {
            continue;
        };
        if step.is_empty() || step.contains(' ') {
            continue;
        }
        let outcome = if verdict.starts_with("✓ PASS") {
            Outcome::Pass
        } else if verdict.starts_with("✗ FAIL") {
            Outcome::Fail
        } else if verdict.starts_with("SKIPPED") {
            Outcome::Skip
        } else {
            continue;
        };
        results.record(TestResult {
            id: TestId::step(step),
            outcome,
            duration_ms: None,
        });
    }
    results
}

/// Per-step results from a dagrun `journal.jsonl`.
pub fn parse_journal(text: &str) -> TestResults {
    let mut results = TestResults::default();
    for line in text.lines() {
        let Ok(record) = serde_json::from_str::<JournalLine>(line) else {
            continue;
        };
        if record.step.is_empty() {
            continue;
        }
        let outcome = match (record.event.as_str(), record.ok) {
            ("step_end", Some(true)) => Outcome::Pass,
            ("step_end", _) => Outcome::Fail,
            ("step_skip", _) => Outcome::Skip,
            _ => continue,
        };
        let duration_ms = record
            .wall_elapsed_s
            .and_then(|secs| secs.parse::<f64>().ok())
            .map(|secs| (secs * 1000.0).round() as u64);
        results.record(TestResult {
            id: TestId::step(record.step),
            outcome,
            duration_ms,
        });
    }
    results
}

impl Validator for DagrunValidator {
    fn select(&self, checkout: &Path, base: &str, head: &str) -> Result<Chosen, String> {
        shared_select(&self.0, checkout, base, head)
    }

    fn run(
        &self,
        checkout: &Path,
        selection: &Selection,
        out: &Path,
        cancel: &AtomicBool,
    ) -> Result<Ran, String> {
        let journal_dir = out.join("dagrun-log");
        let _ = std::fs::remove_dir_all(&journal_dir);
        private_dir(&journal_dir)?;
        let dagrun = if self.0.dagrun.is_empty() {
            "dagrun".to_owned()
        } else {
            self.0.dagrun.clone()
        };
        let mut argv = vec![
            dagrun,
            "run".to_owned(),
            "--dag".to_owned(),
            self.0.dag.clone(),
            "--keep-going".to_owned(),
        ];
        if let Some(tests) = selection.tests() {
            // dagrun selects whole steps; a case-level ID narrows nothing here.
            let steps: BTreeSet<&str> = tests.iter().map(|test| test.step.as_str()).collect();
            if steps.is_empty() {
                return Ok(Ran {
                    results: TestResults::default(),
                    exit: Some(0),
                    elapsed: Duration::ZERO,
                    log: out.join("run.log"),
                    cancelled: false,
                });
            }
            argv.push("--selected".to_owned());
            argv.push(steps.into_iter().collect::<Vec<_>>().join(","));
        }
        let env = [("DAGRUN_LOG_DIR", journal_dir.display().to_string())];
        let log = out.join("run.log");
        let (exit, elapsed, cancelled) = run_logged(&self.0, &argv, checkout, &env, &log, cancel)?;
        let journal =
            std::fs::read_to_string(journal_dir.join("journal.jsonl")).unwrap_or_default();
        let mut results = parse_journal(&journal);
        if results.results.is_empty() {
            // No journal (dagrun disables it when it distrusts the directory): read the verdict
            // lines it prints instead.
            results = parse_step_lines(&std::fs::read_to_string(&log).unwrap_or_default());
        }
        let whole_steps = selection
            .tests()
            .map(|tests| Selection::of(tests.iter().map(TestId::whole_step)));
        results.require(whole_steps.as_ref().unwrap_or(selection));
        Ok(Ran {
            results,
            exit,
            elapsed,
            log,
            cancelled,
        })
    }
}

#[cfg(test)]
mod unit {
    use super::*;
    use crate::git::testrepo::TempDir;

    fn config(run: &str) -> ValidatorConfig {
        ValidatorConfig {
            kind: "command".into(),
            run: run.into(),
            ..ValidatorConfig::default()
        }
    }

    #[test]
    fn command_results_contract_and_missing_tests() {
        let tmp = TempDir::new("validate");
        let v = from_config(&config(
            r#"for t in $MQ_TESTS; do case $t in bad*) o=fail;; *) o=pass;; esac; echo "{\"id\":\"$t\",\"outcome\":\"$o\"}" >> "$MQ_RESULTS"; done; grep -q bad "$MQ_TESTS_FILE" && exit 1; exit 0"#,
        ))
        .unwrap();
        let sel = Selection::of(["a", "bad1"].map(TestId::step));
        let ran = v
            .run(&tmp.0, &sel, &tmp.0.join("out1"), &AtomicBool::new(false))
            .unwrap();
        assert!(!ran.green());
        assert_eq!(ran.failed(), [TestId::step("bad1")].into());
        let ran = v
            .run(
                &tmp.0,
                &Selection::of([TestId::step("a")]),
                &tmp.0.join("out2"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(ran.green());
    }

    #[test]
    fn exit_status_only_runner_is_unattributed() {
        let tmp = TempDir::new("validate-exit");
        let v = from_config(&config("exit 3")).unwrap();
        let ran = v
            .run(
                &tmp.0,
                &Selection::All,
                &tmp.0.join("o"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(ran.results.unattributed);
        assert_eq!(ran.failed(), [TestId::step(UNATTRIBUTED)].into());
        let ok = from_config(&config("true")).unwrap();
        assert!(ok
            .run(
                &tmp.0,
                &Selection::All,
                &tmp.0.join("p"),
                &AtomicBool::new(false)
            )
            .unwrap()
            .green());
    }

    #[test]
    fn timeout_kills_and_reports_errors() {
        let tmp = TempDir::new("validate-timeout");
        let mut cfg = config("sleep 30");
        cfg.timeout_secs = 1;
        let v = from_config(&cfg).unwrap();
        let ran = v
            .run(
                &tmp.0,
                &Selection::of([TestId::step("x")]),
                &tmp.0.join("o"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(ran.exit, None);
        assert!(!ran.green());
        assert!(ran.elapsed < Duration::from_secs(10));
        assert!(!ran.cancelled);
    }

    #[test]
    fn cancel_stops_a_running_validation() {
        let tmp = TempDir::new("validate-cancel");
        let v = from_config(&config("sleep 30")).unwrap();
        let cancel = AtomicBool::new(false);
        let ran = std::thread::scope(|scope| {
            let handle = scope.spawn(|| v.run(&tmp.0, &Selection::All, &tmp.0.join("o"), &cancel));
            std::thread::sleep(Duration::from_millis(300));
            cancel.store(true, Ordering::Relaxed);
            handle.join().unwrap().unwrap()
        });
        assert!(ran.cancelled);
        assert!(!ran.green());
        assert!(ran.elapsed < Duration::from_secs(10));
    }

    #[test]
    fn select_command_with_prefix() {
        let tmp = TempDir::new("validate-select");
        let mut cfg = config("true");
        cfg.select = "echo RUN group; echo STEP a.b; echo STEP c.d::e; echo base=$MQ_BASE".into();
        cfg.id_prefix = "STEP ".into();
        cfg.policy = "path-routing".into();
        let chosen = from_config(&cfg).unwrap().select(&tmp.0, "B", "H").unwrap();
        assert_eq!(chosen.policy, "path-routing");
        assert_eq!(
            chosen.selection,
            Selection::of([TestId::step("a.b"), TestId::parse("c.d::e").unwrap()])
        );
        cfg.select.clear();
        assert_eq!(
            from_config(&cfg)
                .unwrap()
                .select(&tmp.0, "B", "H")
                .unwrap()
                .selection,
            Selection::All
        );
    }

    #[test]
    fn verdict_line_fallback() {
        let text = "[scheduler] WARNING: evidence directory is not private\n\
[a.b] ▶ START  something\n\
[a.b] ✓ PASS   something (0s)  [ok]\n\
[c.d] ✗ FAIL   other (3s, exit 1)\n\
[e.f] SKIPPED reason=unsupported\n\
dagrun: PASS - 1 passed";
        let results = parse_step_lines(text);
        assert_eq!(results.results.len(), 3);
        assert_eq!(results.results[&TestId::step("a.b")], Outcome::Pass);
        assert_eq!(results.failed(), [TestId::step("c.d")].into());
        assert_eq!(results.results[&TestId::step("e.f")], Outcome::Skip);
    }

    #[test]
    fn evidence_directory_is_private() {
        let tmp = TempDir::new("validate-private");
        let dir = tmp.0.join("a/dagrun-log");
        private_dir(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[test]
    fn journal_parsing() {
        let text = r#"{"event":"step_start","step":"a"}
{"event":"step_end","step":"a","ok":true,"wall_elapsed_s":"1.5"}
{"event":"step_end","step":"b","ok":false}
{"event":"step_skip","step":"c","reason":"x"}
not json"#;
        let results = parse_journal(text);
        assert_eq!(results.results[&TestId::step("a")], Outcome::Pass);
        assert_eq!(results.durations_ms[&TestId::step("a")], 1500);
        assert_eq!(results.failed(), [TestId::step("b")].into());
        assert_eq!(results.results[&TestId::step("c")], Outcome::Skip);
    }
}
