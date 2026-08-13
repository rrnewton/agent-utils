//! Deterministic tick evaluation over injected gate and file-age boundaries.

use std::collections::BTreeMap;

use indexmap::IndexMap;

use crate::cadence::is_due;
use crate::emit::{
    format_action, format_error, format_health, format_no_result, format_note,
    HEALTH_STATUS_MISSING, HEALTH_STATUS_OK, HEALTH_STATUS_STALE,
};
use crate::model::{Emit, EmitKind, Gate, GateWhen, HealthCheck, TickConfig};
use crate::protocols::{FileAgeProbe, GateRunner};
use crate::state::{flag_truthy, state_lines, OpsState};
use crate::text::{split_lines, string_repr, trim};

/// Everything produced by one tick.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TickResult {
    /// Ordered line protocol output.
    pub lines: Vec<String>,
    /// Advanced fired-state map.
    pub fired: BTreeMap<String, i64>,
    /// Number of `ACTION:` instructions emitted.
    pub actions_emitted: usize,
}

/// Parse non-comment `key=value` lines, retaining insertion order.
pub fn parse_kv_lines(text: &str) -> IndexMap<String, String> {
    let mut out = IndexMap::new();
    for raw_line in split_lines(text) {
        let line = trim(raw_line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((raw_key, raw_value)) = line.split_once('=') {
            let key = trim(raw_key);
            if !key.is_empty() {
                out.insert(key.to_string(), trim(raw_value).to_string());
            }
        }
    }
    out
}

/// Evaluate one freshness check.
pub fn evaluate_health(hc: &HealthCheck, probe: &dyn FileAgeProbe, now: i64) -> String {
    let age = probe.newest_age_secs(&hc.glob, now);
    let status = match age {
        None => HEALTH_STATUS_MISSING,
        Some(age) if age <= hc.threshold_secs => HEALTH_STATUS_OK,
        Some(_) => HEALTH_STATUS_STALE,
    };
    format_health(&hc.name, status, age, hc.threshold_secs, &hc.detail)
}

/// Exit code reserved for "I could not determine my condition".
///
/// 75 is EX_TEMPFAIL ("temporary failure, user is invited to retry"), chosen on
/// collision evidence rather than taste: across the scripts behind the live
/// gate set the codes already in use are 0, 1, 2, 3, 124 and 127. 2 is the
/// tempting choice because one gate already prints NO_RESULT while exiting 2 --
/// and it is WRONG, because argparse exits 2 on a usage error, so reserving it
/// would render a crashed gate as a non-answer. Softening a real failure into
/// NO_RESULT is the opposite of the point.
pub const NO_RESULT_EXIT: i32 = 75;

/// What one gate execution concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateOutcome {
    /// The gate determined its condition and it does not warrant emission.
    Quiet,
    /// The gate determined its condition and it warrants emission.
    Fire,
    /// The gate ran but could not determine its condition.
    NoResult,
}

fn gate_fires(when: GateWhen, returncode: i32, stdout: &str) -> bool {
    match when {
        GateWhen::Success => returncode == 0,
        GateWhen::Failure => returncode != 0,
        GateWhen::Nonempty => !trim(stdout).is_empty(),
        GateWhen::Always => true,
    }
}

fn eval_gate(
    gate: Option<&Gate>,
    runner: &dyn GateRunner,
) -> Result<(GateOutcome, IndexMap<String, String>), String> {
    let Some(gate) = gate else {
        return Ok((GateOutcome::Fire, IndexMap::new()));
    };
    let result = runner.run(&gate.cmd);
    if !result.ok {
        return Err(format!(
            "gate command could not run ({}): {}",
            string_repr(&gate.cmd),
            result.error.as_deref().unwrap_or("None")
        ));
    }
    let captured = if gate.capture {
        parse_kv_lines(&result.stdout)
    } else {
        IndexMap::new()
    };
    // Checked BEFORE `when`, so a gate that cannot determine its condition is
    // never reinterpreted through a fire/quiet rule. Under `when: failure` a
    // 75 would otherwise read as an ordinary failure verdict; under
    // `when: success` it would read as a clean pass, which is the exact
    // silence-means-healthy collapse this exists to remove.
    if result.returncode == NO_RESULT_EXIT {
        return Ok((GateOutcome::NoResult, captured));
    }
    let outcome = if gate_fires(gate.when, result.returncode, &result.stdout) {
        GateOutcome::Fire
    } else {
        GateOutcome::Quiet
    };
    Ok((outcome, captured))
}

fn interpolate(mut text: String, values: &IndexMap<String, String>) -> String {
    for (key, value) in values {
        text = text.replace(&format!("{{{key}}}"), value);
    }
    text
}

/// Render a fired emission, including captured-field interpolation.
pub fn render_emit(emit: &Emit, captured: &IndexMap<String, String>) -> String {
    let mut merged = emit.fields.clone();
    for (key, value) in captured {
        merged.insert(key.clone(), value.clone());
    }
    for key in emit.fields.keys() {
        if !captured.contains_key(key) {
            let value = merged.get(key).cloned().unwrap_or_default();
            merged.insert(key.clone(), interpolate(value, &merged));
        }
    }
    let title = interpolate(emit.title.clone(), &merged);
    match emit.kind {
        EmitKind::Note => format_note(&title),
        EmitKind::Action => format_action(&emit.skill, &merged, &title),
    }
}

/// Run one tick using explicit time and injected side-effect boundaries.
#[allow(clippy::too_many_arguments)]
pub fn run_tick(
    config: &TickConfig,
    state: &OpsState,
    now: i64,
    fired: &BTreeMap<String, i64>,
    gate_runner: &dyn GateRunner,
    age_probe: &dyn FileAgeProbe,
    current_tick_min: Option<i64>,
) -> TickResult {
    let mut lines = Vec::new();
    let mut actions = 0;
    for health in &config.health_checks {
        lines.push(evaluate_health(health, age_probe, now));
    }
    for line in state_lines(state, current_tick_min) {
        if line.starts_with("ACTION: ") {
            actions += 1;
        }
        lines.push(line);
    }
    let mut new_fired = fired.clone();
    if state.enabled {
        for reminder in &config.reminders {
            if !is_due(&reminder.name, reminder.cadence_secs, now, fired) {
                continue;
            }
            if !reminder
                .requires_flags
                .iter()
                .all(|name| flag_truthy(&state.flags, name))
            {
                continue;
            }
            let (outcome, captured) = match eval_gate(reminder.gate.as_ref(), gate_runner) {
                Ok(result) => result,
                Err(error) => {
                    lines.push(format_error(&format!(
                        "reminder {}: {error}",
                        reminder.name
                    )));
                    continue;
                }
            };
            // A NO_RESULT does NOT consume cadence, matching the existing
            // cannot-run path above: the gate keeps announcing every tick until
            // it can determine something. Deliberately noisy -- silence is the
            // hazard here, so repetition is the correct trade.
            if outcome == GateOutcome::NoResult {
                lines.push(format_no_result(
                    &reminder.name,
                    captured.get("summary").map(String::as_str).unwrap_or(""),
                ));
                continue;
            }
            new_fired.insert(reminder.name.clone(), now);
            if outcome == GateOutcome::Quiet {
                continue;
            }
            let line = render_emit(&reminder.emit, &captured);
            if line.starts_with("ACTION: ") {
                actions += 1;
            }
            lines.push(line);
        }
    }
    lines.push(format_note(&format!(
        "emitted {actions} instruction(s) this tick"
    )));
    TickResult {
        lines,
        fired: new_fired,
        actions_emitted: actions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Emit, Gate, HealthCheck, Reminder};
    use crate::protocols::GateResult;
    use crate::state::FlagValue;
    use std::cell::RefCell;

    struct FakeGate {
        outcomes: BTreeMap<String, GateResult>,
        calls: RefCell<Vec<String>>,
    }

    impl GateRunner for FakeGate {
        fn run(&self, cmd: &str) -> GateResult {
            self.calls.borrow_mut().push(cmd.to_string());
            self.outcomes
                .get(cmd)
                .cloned()
                .unwrap_or_else(|| GateResult::completed(0, ""))
        }
    }

    struct FakeProbe(BTreeMap<String, Option<i64>>);

    impl FileAgeProbe for FakeProbe {
        fn newest_age_secs(&self, pattern: &str, _now: i64) -> Option<i64> {
            self.0.get(pattern).copied().flatten()
        }
    }

    fn fakes() -> (FakeGate, FakeProbe) {
        (
            FakeGate {
                outcomes: BTreeMap::new(),
                calls: RefCell::new(Vec::new()),
            },
            FakeProbe(BTreeMap::new()),
        )
    }

    #[test]
    fn health_and_trailing_summary_are_ordered() {
        let config = TickConfig {
            health_checks: vec![
                HealthCheck {
                    name: "fresh".into(),
                    glob: "/f".into(),
                    threshold_secs: 100,
                    detail: "f".into(),
                },
                HealthCheck {
                    name: "gone".into(),
                    glob: "/g".into(),
                    threshold_secs: 100,
                    detail: "g".into(),
                },
            ],
            ..TickConfig::default()
        };
        let (gate, _) = fakes();
        let probe = FakeProbe(BTreeMap::from([
            ("/f".into(), Some(10)),
            ("/g".into(), None),
        ]));
        let result = run_tick(
            &config,
            &OpsState::default(),
            0,
            &BTreeMap::new(),
            &gate,
            &probe,
            None,
        );
        assert!(result.lines[0].starts_with("HEALTH: fresh ok"));
        assert!(result.lines[1].starts_with("HEALTH: gone missing"));
        assert_eq!(
            result.lines.last().unwrap(),
            "NOTE: emitted 0 instruction(s) this tick"
        );
    }

    #[test]
    fn capture_interpolates_and_preserves_field_order() {
        let mut emit = Emit::action("triage", "{count} ready (> {threshold})");
        emit.fields.insert("threshold".into(), "5".into());
        let mut reminder = Reminder::new("backlog", emit);
        reminder.gate = Some(Gate {
            cmd: "count".into(),
            when: GateWhen::Always,
            capture: true,
        });
        let config = TickConfig {
            reminders: vec![reminder],
            ..TickConfig::default()
        };
        let gate = FakeGate {
            outcomes: BTreeMap::from([("count".into(), GateResult::completed(0, "count=7\n"))]),
            calls: RefCell::new(Vec::new()),
        };
        let probe = FakeProbe(BTreeMap::new());
        let result = run_tick(
            &config,
            &OpsState::default(),
            0,
            &BTreeMap::new(),
            &gate,
            &probe,
            None,
        );
        assert!(result
            .lines
            .contains(&"ACTION: triage threshold=5 count=7 title=\"7 ready (> 5)\"".to_string()));
        assert_eq!(result.fired.get("backlog"), Some(&0));
    }

    #[test]
    fn static_fields_can_interpolate_other_static_and_captured_fields() {
        let mut emit = Emit::action("triage", "{copy}/{live}");
        emit.fields.insert("base".into(), "7".into());
        emit.fields.insert("copy".into(), "{base}".into());
        emit.fields.insert("live".into(), "{count}".into());
        let captured = IndexMap::from([("count".into(), "3".into())]);
        assert_eq!(
            render_emit(&emit, &captured),
            "ACTION: triage base=7 copy=7 live=3 count=3 title=\"7/3\""
        );
    }

    #[test]
    fn captured_values_honor_all_line_boundaries() {
        assert_eq!(
            parse_kv_lines("a=1\rb=2\u{b}c=3\u{85}d=4\u{2028}e=5"),
            IndexMap::<String, String>::from([
                ("a".into(), "1".into()),
                ("b".into(), "2".into()),
                ("c".into(), "3".into()),
                ("d".into(), "4".into()),
                ("e".into(), "5".into()),
            ])
        );
    }

    #[test]
    fn flag_suppression_does_not_consume_cadence() {
        let mut reminder = Reminder::new("bench", Emit::action("run-benchmark", "refresh"));
        reminder.requires_flags.push("benchmark_enabled".into());
        let config = TickConfig {
            reminders: vec![reminder],
            ..TickConfig::default()
        };
        let state = OpsState {
            flags: BTreeMap::from([("benchmark_enabled".into(), FlagValue::Bool(false))]),
            ..OpsState::default()
        };
        let (gate, probe) = fakes();
        let result = run_tick(&config, &state, 10, &BTreeMap::new(), &gate, &probe, None);
        assert!(!result
            .lines
            .iter()
            .any(|line| line.contains("run-benchmark")));
        assert!(!result.fired.contains_key("bench"));
    }

    #[test]
    fn gate_execution_failure_is_retried() {
        let mut reminder = Reminder::new("x", Emit::action("x", "x"));
        reminder.gate = Some(Gate::new("boom"));
        let config = TickConfig {
            reminders: vec![reminder],
            ..TickConfig::default()
        };
        let gate = FakeGate {
            outcomes: BTreeMap::from([("boom".into(), GateResult::failed("not found"))]),
            calls: RefCell::new(Vec::new()),
        };
        let probe = FakeProbe(BTreeMap::new());
        let result = run_tick(
            &config,
            &OpsState::default(),
            9,
            &BTreeMap::new(),
            &gate,
            &probe,
            None,
        );
        assert!(result.lines.iter().any(|line| {
            line == "ERROR: reminder x: gate command could not run ('boom'): not found"
        }));
        assert!(!result.fired.contains_key("x"));
    }

    // --- COULD-NOT-DETERMINE, BOTH DIRECTIONS ---------------------------------
    //
    // The negative half is the point: a gate that cannot determine its
    // condition must be visibly distinct from one that checked and found
    // nothing. The positive halves prove the change is not inert -- a gate that
    // CAN determine still reports exactly what it reported before.

    fn gated(
        name: &str,
        cmd: &str,
        when: GateWhen,
        code: i32,
        stdout: &str,
    ) -> (TickConfig, FakeGate, FakeProbe) {
        let mut reminder = Reminder::new(name, Emit::action("warn", "{summary}"));
        let mut gate = Gate::new(cmd);
        gate.when = when;
        gate.capture = true;
        reminder.gate = Some(gate);
        let config = TickConfig {
            reminders: vec![reminder],
            ..TickConfig::default()
        };
        let runner = FakeGate {
            outcomes: BTreeMap::from([(cmd.into(), GateResult::completed(code, stdout))]),
            calls: RefCell::new(Vec::new()),
        };
        (config, runner, FakeProbe(BTreeMap::new()))
    }

    #[test]
    fn could_not_determine_renders_no_result_not_a_pass() {
        let (config, gate, probe) = gated(
            "watcher",
            "probe",
            GateWhen::Failure,
            NO_RESULT_EXIT,
            "summary=backend unreachable\n",
        );
        let result = run_tick(
            &config,
            &OpsState::default(),
            10,
            &BTreeMap::new(),
            &gate,
            &probe,
            None,
        );
        let line = result
            .lines
            .iter()
            .find(|l| l.starts_with("NO_RESULT: "))
            .expect("a NO_RESULT line");
        assert!(line.contains("watcher"), "names the gate: {line}");
        assert!(
            line.contains("this is not a pass"),
            "distinct from a pass: {line}"
        );
        assert!(
            line.contains("backend unreachable"),
            "carries detail when present: {line}"
        );
        assert!(
            !result.lines.iter().any(|l| l.starts_with("ACTION: ")),
            "must not also fire a verdict"
        );
    }

    #[test]
    fn no_result_is_emittable_when_the_gate_printed_nothing() {
        // THE CORRELATED-FAILURE CASE. A gate that cannot determine its
        // condition is the one least likely to produce a usable summary=, so
        // the line must not depend on captured fields or on emit.title.
        let (config, gate, probe) = gated("mute", "probe", GateWhen::Failure, NO_RESULT_EXIT, "");
        let result = run_tick(
            &config,
            &OpsState::default(),
            10,
            &BTreeMap::new(),
            &gate,
            &probe,
            None,
        );
        let line = result
            .lines
            .iter()
            .find(|l| l.starts_with("NO_RESULT: "))
            .expect("a NO_RESULT line");
        assert!(line.contains("mute"));
        assert!(
            !line.contains('{'),
            "must not leak an unresolved placeholder: {line}"
        );
    }

    #[test]
    fn no_result_does_not_consume_cadence() {
        let (config, gate, probe) = gated("retry", "probe", GateWhen::Failure, NO_RESULT_EXIT, "");
        let result = run_tick(
            &config,
            &OpsState::default(),
            10,
            &BTreeMap::new(),
            &gate,
            &probe,
            None,
        );
        assert!(
            !result.fired.contains_key("retry"),
            "must re-announce next tick"
        );
    }

    #[test]
    fn no_result_is_not_reinterpreted_by_when_success() {
        // Under `when: success` a nonzero code means "quiet". Without the
        // pre-check, 75 would silently read as a clean pass -- the exact
        // collapse this change removes.
        let (config, gate, probe) = gated("s", "probe", GateWhen::Success, NO_RESULT_EXIT, "");
        let result = run_tick(
            &config,
            &OpsState::default(),
            10,
            &BTreeMap::new(),
            &gate,
            &probe,
            None,
        );
        assert!(
            result.lines.iter().any(|l| l.starts_with("NO_RESULT: ")),
            "{:?}",
            result.lines
        );
    }

    #[test]
    fn positive_control_a_determinable_gate_is_unchanged() {
        // fires on real failure ...
        let (config, gate, probe) =
            gated("f", "probe", GateWhen::Failure, 1, "summary=real problem\n");
        let fired = run_tick(
            &config,
            &OpsState::default(),
            10,
            &BTreeMap::new(),
            &gate,
            &probe,
            None,
        );
        assert!(fired
            .lines
            .iter()
            .any(|l| l.starts_with("ACTION: ") && l.contains("real problem")));
        assert!(!fired.lines.iter().any(|l| l.starts_with("NO_RESULT: ")));
        assert!(
            fired.fired.contains_key("f"),
            "a real verdict still consumes cadence"
        );
        // ... and stays quiet on a clean check.
        let (config, gate, probe) = gated("q", "probe", GateWhen::Failure, 0, "");
        let quiet = run_tick(
            &config,
            &OpsState::default(),
            10,
            &BTreeMap::new(),
            &gate,
            &probe,
            None,
        );
        assert!(!quiet
            .lines
            .iter()
            .any(|l| l.starts_with("ACTION: ") || l.starts_with("NO_RESULT: ")));
        assert!(quiet.fired.contains_key("q"));
    }
}
