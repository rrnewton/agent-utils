//! Deterministic tick evaluation over injected gate and file-age boundaries.

use std::collections::BTreeMap;

use indexmap::IndexMap;

use crate::cadence::is_due;
use crate::emit::{
    format_action, format_error, format_health, format_note, HEALTH_STATUS_MISSING,
    HEALTH_STATUS_OK, HEALTH_STATUS_STALE,
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
) -> Result<(bool, IndexMap<String, String>), String> {
    let Some(gate) = gate else {
        return Ok((true, IndexMap::new()));
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
    Ok((
        gate_fires(gate.when, result.returncode, &result.stdout),
        captured,
    ))
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
            let (fire, captured) = match eval_gate(reminder.gate.as_ref(), gate_runner) {
                Ok(result) => result,
                Err(error) => {
                    lines.push(format_error(&format!(
                        "reminder {}: {error}",
                        reminder.name
                    )));
                    continue;
                }
            };
            new_fired.insert(reminder.name.clone(), now);
            if !fire {
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
}
