//! Agent-teams health in each watch pass (#721 agent-teams-coordinator-handover): declaration
//! red flags for teams delivering through this registry, the receiving host's undeliverable
//! notices, and this host's outbox, which the watcher pumps so a message refused as
//! undeliverable is retried once its target is fixed.
//!
//! Every notice goes to the registry's designated coordinator as it stands at notify time (the
//! one the watcher started with may since have been retired by a handover), and its state is
//! recorded only once the notice is queued (#669 watch-notice-delivery): a failed send is tried
//! again next pass.
use super::*;

/// Seconds between the watcher's runs of `agent-teams pump` while this host's agent-teams
/// outbox holds deferred messages.
pub const TEAM_PUMP_INTERVAL_SECONDS: f64 = 600.0;

/// Most notices listed in one message; the rest are counted.
const TEAM_NOTICE_LINES: usize = 20;

/// Who receives agent-teams notices: the registry's designated coordinator now, else the
/// watcher's configured coordinator.
fn team_recipient(probe: &mut dyn Probe, options: &WatchAgentsOptions) -> String {
    probe
        .designated_coordinator()
        .unwrap_or_else(|| options.coordinator.clone())
}

/// The declaration red flags, and what telling them did. A changed flag set is told once it is
/// queued; a standing set is not repeated per pass. Warnings and errors are both told; the
/// pass's `ok` counts only errors.
pub(super) fn report_team_declarations(
    probe: &mut dyn Probe,
    notifier: &mut dyn Notifier,
    state: &mut WatchState,
    now: f64,
    options: &WatchAgentsOptions,
) -> (Vec<Value>, Value) {
    let flags = probe.team_declaration_flags();
    if flags.is_empty() {
        state.team_flags_reported.clear();
        return (flags, Value::Null);
    }
    let fingerprint = flags
        .iter()
        .map(|flag| {
            format!(
                "{}:{}",
                flag["team"].as_str().unwrap_or("?"),
                flag["kind"].as_str().unwrap_or("?")
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    if fingerprint == state.team_flags_reported {
        return (flags, json!({"reported": true}));
    }
    if options.check_only {
        return (flags, json!({"due": true}));
    }
    let lines: Vec<String> = flags
        .iter()
        .map(|flag| {
            format!(
                "- [{}] {} — fix: {}",
                flag["severity"].as_str().unwrap_or("?"),
                flag["detail"].as_str().unwrap_or("?"),
                flag["fix"].as_str().unwrap_or("?")
            )
        })
        .collect();
    let errors = flags
        .iter()
        .filter(|flag| flag["severity"] == "error")
        .count();
    let text = format!(
        "[agentctl watch-agents] agent-teams declaration health: {} red flag(s), {errors} of them errors, for teams delivering through this registry{}:\n{}",
        flags.len(),
        if errors > 0 {
            "; inter-team messages are failing until they are fixed"
        } else {
            ""
        },
        lines.join("\n")
    );
    let recipient = team_recipient(probe, options);
    let message_id = format!("agent-watch-team-declarations-{}", now as u64);
    let outcome = match notifier.notify(&recipient, &text, &message_id) {
        Ok(outcome) => {
            state.team_flags_reported = fingerprint;
            json!({"reported": true, "recipient": recipient, "outcome": outcome})
        }
        Err(error) => {
            json!({"reported": false, "recipient": recipient, "error": error.to_string()})
        }
    };
    (flags, outcome)
}

/// The receiving host's undeliverable notices: told in one message, then moved aside so each
/// is told once. A notice whose message could not be queued stays for the next pass.
pub(super) fn report_team_notices(
    probe: &mut dyn Probe,
    notifier: &mut dyn Notifier,
    now: f64,
    options: &WatchAgentsOptions,
) -> Vec<Value> {
    let notices = probe.team_undeliverable_notices();
    let mut rows: Vec<Value> = notices
        .iter()
        .map(|(path, notice)| {
            json!({
                "team": notice["team"], "id": notice["id"], "sender": notice["sender"],
                "reason": notice["reason"], "at": notice["at"], "path": path,
            })
        })
        .collect();
    if rows.is_empty() {
        return rows;
    }
    if options.check_only {
        for row in &mut rows {
            row["due"] = json!(true);
        }
        return rows;
    }
    let mut lines: Vec<String> = notices
        .iter()
        .take(TEAM_NOTICE_LINES)
        .map(|(_, notice)| {
            format!(
                "- team {}: {} from {}: {}",
                notice["team"].as_str().unwrap_or("?"),
                notice["id"].as_str().unwrap_or("?"),
                notice["sender"].as_str().unwrap_or("?"),
                notice["reason"].as_str().unwrap_or("?")
            )
        })
        .collect();
    if notices.len() > TEAM_NOTICE_LINES {
        lines.push(format!(
            "- and {} more under teams.d/<team>/notices/",
            notices.len() - TEAM_NOTICE_LINES
        ));
    }
    let text = format!(
        "[agentctl watch-agents] agent-teams refused {} inter-team message(s) to this host as undeliverable. Nothing was claimed here; each sender keeps its message and retries it with `agent-teams pump` (a sender's watch-agents pumps every {} min) once the delivery target is fixed. Fix the target first (`agent-teams check` names each fix):\n{}",
        notices.len(),
        (TEAM_PUMP_INTERVAL_SECONDS / 60.0) as u64,
        lines.join("\n")
    );
    let ids: Vec<&str> = notices
        .iter()
        .map(|(_, notice)| notice["id"].as_str().unwrap_or("?"))
        .collect();
    let message_id = format!(
        "agent-watch-team-undeliverable-{:016x}-{}",
        stable_hash(&ids.join(",")),
        now as u64
    );
    let recipient = team_recipient(probe, options);
    match notifier.notify(&recipient, &text, &message_id) {
        Ok(outcome) => {
            for ((path, _), row) in notices.iter().zip(rows.iter_mut()) {
                row["recipient"] = json!(recipient);
                row["outcome"] = json!(outcome);
                match notifier.acknowledge_team_notice(path) {
                    Ok(()) => row["reported"] = json!(true),
                    Err(error) => {
                        row["reported"] = json!(true);
                        row["acknowledge_error"] = json!(error);
                    }
                }
            }
        }
        Err(error) => {
            for row in &mut rows {
                row["reported"] = json!(false);
                row["error"] = json!(error.to_string());
            }
        }
    }
    rows
}

/// Run `agent-teams pump` at most every [`TEAM_PUMP_INTERVAL_SECONDS`] while this host's
/// outbox holds deferred messages, so a message refused as undeliverable is retried once its
/// target is fixed without anyone remembering to pump. `null` when nothing waits.
pub(super) fn drive_team_pump(
    probe: &mut dyn Probe,
    notifier: &mut dyn Notifier,
    state: &mut WatchState,
    now: f64,
    options: &WatchAgentsOptions,
) -> Value {
    let waiting = probe.team_outbox_waiting();
    if waiting == 0 {
        return Value::Null;
    }
    let mut row = json!({"waiting": waiting});
    if options.check_only {
        row["due"] = json!(true);
        return row;
    }
    if let Some(next_at) = state
        .team_pump_at
        .map(|at| at + TEAM_PUMP_INTERVAL_SECONDS)
        .filter(|next_at| now < *next_at)
    {
        row["next_at"] = json!(next_at);
        return row;
    }
    state.team_pump_at = Some(now);
    match notifier.run_team_pump() {
        Ok(output) => row["pumped"] = json!(output),
        Err(error) => row["error"] = json!(error),
    }
    row
}

#[cfg(test)]
mod tests {
    use super::super::tests::options;
    use super::*;

    /// A probe whose only findings are scripted agent-teams state.
    #[derive(Default)]
    struct TeamsProbe {
        designated: Option<String>,
        flags: Vec<Value>,
        notices: Vec<(PathBuf, Value)>,
        waiting: usize,
    }

    impl Probe for TeamsProbe {
        fn observe(&mut self) -> Result<Vec<Observation>> {
            Ok(Vec::new())
        }
        fn designated_coordinator(&mut self) -> Option<String> {
            self.designated.clone()
        }
        fn team_declaration_flags(&mut self) -> Vec<Value> {
            self.flags.clone()
        }
        fn team_undeliverable_notices(&mut self) -> Vec<(PathBuf, Value)> {
            self.notices.clone()
        }
        fn team_outbox_waiting(&mut self) -> usize {
            self.waiting
        }
    }

    #[derive(Default)]
    struct TeamsNotifier {
        sent: Vec<(String, String)>,
        fail: bool,
        acknowledged: Vec<PathBuf>,
        pumps: usize,
    }

    impl Notifier for TeamsNotifier {
        fn notify(&mut self, coordinator: &str, text: &str, _message_id: &str) -> Result<String> {
            if self.fail {
                return Err(fail("the coordinator's queue refused the notice"));
            }
            self.sent.push((coordinator.to_owned(), text.to_owned()));
            Ok("queued".to_owned())
        }
        fn acknowledge_team_notice(&mut self, path: &Path) -> std::result::Result<(), String> {
            self.acknowledged.push(path.to_owned());
            Ok(())
        }
        fn run_team_pump(&mut self) -> std::result::Result<String, String> {
            self.pumps += 1;
            Ok("pump: nothing to do".to_owned())
        }
    }

    fn flag(kind: &str, severity: &str) -> Value {
        json!({"team": "tools", "kind": kind, "severity": severity,
               "detail": format!("team tools: {kind}"), "fix": "agentctl coordinator set NEW"})
    }

    /// Review finding 4: a failed send leaves the flag set untold, so the next pass tells it;
    /// it goes to the coordinator designated at notify time, not the one the watcher started
    /// with.
    #[test]
    fn declaration_flags_are_recorded_only_after_delivery_to_the_designated_coordinator() {
        let options = options();
        let mut state = WatchState::default();
        let mut probe = TeamsProbe {
            designated: Some("coord-new".to_owned()),
            flags: vec![flag("target-archived", "error")],
            ..TeamsProbe::default()
        };
        let mut notifier = TeamsNotifier {
            fail: true,
            ..TeamsNotifier::default()
        };
        let (_, outcome) =
            report_team_declarations(&mut probe, &mut notifier, &mut state, 1_000.0, &options);
        assert_eq!(outcome["reported"], false, "{outcome}");
        assert!(state.team_flags_reported.is_empty());
        notifier.fail = false;
        let (_, outcome) =
            report_team_declarations(&mut probe, &mut notifier, &mut state, 1_060.0, &options);
        assert_eq!(outcome["reported"], true, "{outcome}");
        assert_eq!(notifier.sent.len(), 1);
        assert_eq!(notifier.sent[0].0, "coord-new");
        assert!(notifier.sent[0].1.contains("target-archived"));
        report_team_declarations(&mut probe, &mut notifier, &mut state, 1_120.0, &options);
        assert_eq!(notifier.sent.len(), 1, "a standing flag set is told once");
    }

    /// Review finding 5: a standing warning does not make the pass not-OK; an error does.
    #[test]
    fn only_error_flags_make_the_pass_not_ok() {
        let options = options();
        let mut state = WatchState::default();
        let mut notifier = TeamsNotifier::default();
        let mut probe = TeamsProbe {
            flags: vec![flag("pinned-agent", "warning")],
            ..TeamsProbe::default()
        };
        let report = watch_agents_once(&mut probe, &mut notifier, &mut state, 1_000.0, &options);
        assert_eq!(report["ok"], true, "{report}");
        assert_eq!(report["team_declarations"][0]["kind"], "pinned-agent");
        let mut probe = TeamsProbe {
            flags: vec![flag("target-missing", "error")],
            ..TeamsProbe::default()
        };
        let report = watch_agents_once(&mut probe, &mut notifier, &mut state, 1_060.0, &options);
        assert_eq!(report["ok"], false, "{report}");
    }

    /// Review finding 2: the receiving host's notices reach the coordinator once, and are set
    /// aside only after the notice is queued.
    #[test]
    fn undeliverable_notices_are_told_then_acknowledged() {
        let options = options();
        let path = PathBuf::from("/state/teams.d/tools/notices/tm-a.json");
        let mut probe = TeamsProbe {
            designated: Some("coord-new".to_owned()),
            notices: vec![(
                path.clone(),
                json!({"team": "tools", "id": "tm-a", "sender": "sim@host-b",
                       "reason": "undeliverable: delivery target \"old\" is archived"}),
            )],
            ..TeamsProbe::default()
        };
        let mut notifier = TeamsNotifier {
            fail: true,
            ..TeamsNotifier::default()
        };
        let rows = report_team_notices(&mut probe, &mut notifier, 1_000.0, &options);
        assert_eq!(rows[0]["reported"], false);
        assert!(notifier.acknowledged.is_empty(), "nothing set aside untold");
        notifier.fail = false;
        let rows = report_team_notices(&mut probe, &mut notifier, 1_060.0, &options);
        assert_eq!(rows[0]["reported"], true);
        assert_eq!(notifier.sent[0].0, "coord-new");
        assert!(notifier.sent[0].1.contains("tm-a from sim@host-b"));
        assert_eq!(notifier.acknowledged, vec![path]);
    }

    /// Review finding 2: the watcher pumps a waiting outbox, at most once per interval.
    #[test]
    fn a_waiting_outbox_is_pumped_once_per_interval() {
        let options = options();
        let mut state = WatchState::default();
        let mut notifier = TeamsNotifier::default();
        let mut probe = TeamsProbe::default();
        assert!(
            drive_team_pump(&mut probe, &mut notifier, &mut state, 1_000.0, &options).is_null()
        );
        probe.waiting = 2;
        let row = drive_team_pump(&mut probe, &mut notifier, &mut state, 1_000.0, &options);
        assert_eq!(row["pumped"], "pump: nothing to do", "{row}");
        let row = drive_team_pump(&mut probe, &mut notifier, &mut state, 1_300.0, &options);
        assert_eq!(row["next_at"], 1_600.0, "{row}");
        drive_team_pump(&mut probe, &mut notifier, &mut state, 1_600.0, &options);
        assert_eq!(notifier.pumps, 2);
    }
}
