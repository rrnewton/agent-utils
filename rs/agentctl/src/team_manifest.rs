//! The recovery facts contract: what should be running, what is running,
//! and which records are too incomplete to revive safely (`#1559`).
//!
//! `agentctl team manifest` builds `agentctl-team-manifest/v1` from one
//! shared verify probe pass: an allowlist projection of status rows, plus
//! `record_gaps` judged from raw records internally (owner 257: report
//! records missing profile, bypass flag, declared effort, or recorded
//! session — never rewrite them), a queued census, unit booleans, and a
//! bounded projection of the `agentctl-team-verify/v1` report. Raw status
//! rows and raw verify free text are never passed through: they can carry
//! record tokens, launch arguments, pane text, and bridge text that no
//! recovery brief may contain. The embedded projection's `fix` fields and
//! its two count/state evidence lines remain bounded free text (120/160
//! chars), generated from counts, names, and queue states only.
//!
//! Landing 1 is facts only: nothing here rewrites a record, a driver
//! state file, or a pane. It is not literally write-free — status probing
//! may tighten an existing queue subdirectory to private permissions,
//! and current-profile reading may run git, exactly as status already
//! does. Status rows and unit outcomes come from the verify snapshot;
//! the raw-record facts behind `record_gaps` are a second read taken
//! immediately after it, so on a live registry they can describe the
//! instant just after the agents[] rows they sit beside. Persisting the
//! manifest rides with `team act`.

use std::path::Path;

use serde_json::{json, Value};

use crate::agent::AgentError;
use crate::restore::{CommandOutcome, Host, RestoreConfig};
use crate::subagents::{ManagedAgents, ManagedApi, ManifestProfileState, ManifestRecordFacts};
use crate::team_verify::{unit_row, verify_ok, verify_team_with_snapshot, VerifyOptions};

/// Schema of a manifest document.
pub const MANIFEST_SCHEMA: &str = "agentctl-team-manifest/v1";

type Result<T> = std::result::Result<T, AgentError>;

/// Status-row keys a manifest may copy. Everything else — above all
/// `token`, `arguments`, and launch specs — stays out by construction.
const AGENT_KEYS: [&str; 14] = [
    "harness",
    "profile",
    "cwd",
    "adapter",
    "lifecycle",
    "agent_status",
    "pane_id",
    "workspace_id",
    "declared_model",
    "effective_model",
    "declared_effort",
    "effective_effort",
    "slot",
    "slot_project",
];

/// Build the manifest for one registry from the same probe snapshot its
/// embedded verify judged. It rewrites no record, driver state, or pane;
/// status probing itself may tighten queue-subdirectory permissions and
/// current-profile reading may run git. An `Err` means the registry
/// itself could not be read, never a failed check — failed evidence lands
/// in `errors`/`record_gaps`/verify under stable class names.
pub fn manifest_team<A: ManagedApi + ?Sized>(
    client: &A,
    registry: &Path,
    host: &dyn Host,
    config: &RestoreConfig,
    options: &VerifyOptions,
) -> Result<Value> {
    let (verify, snapshot) = verify_team_with_snapshot(client, registry, host, config, options)?;
    let manager = ManagedAgents::new(client, registry)?;
    let mut errors: Vec<String> = Vec::new();
    for row in &snapshot.desired.undecidable {
        let name = row["subject"].as_str().unwrap_or("unknown agent");
        errors.push(format!("{name}: desired state unreadable"));
    }
    for (name, status) in &snapshot.statuses {
        if status.is_err() {
            errors.push(format!("{name}: status unavailable"));
        }
    }
    if snapshot.pins.is_err() {
        errors.push("turnover pin unreadable".to_owned());
    }
    let facts: Vec<(String, Option<ManifestRecordFacts>)> = snapshot
        .desired
        .names
        .iter()
        .map(|name| {
            let facts = manager.manifest_record_facts(name).ok();
            (name.clone(), facts)
        })
        .collect();
    for (name, facts) in &facts {
        let status_ok = snapshot
            .statuses
            .iter()
            .any(|(known, found)| known == name && found.is_ok());
        if status_ok && facts.is_none() {
            errors.push(format!("{name}: record facts unavailable"));
        }
    }
    let queued: Vec<(String, Value)> = snapshot
        .desired
        .names
        .iter()
        .map(|name| {
            let status = snapshot
                .statuses
                .iter()
                .find(|(known, _)| known == name)
                .and_then(|(_, found)| found.as_ref().ok());
            (name.clone(), queued_census(&manager, name, status))
        })
        .collect();
    for (name, census) in &queued {
        if census["error"].as_str().is_some() {
            errors.push(format!("{name}: queued census unavailable"));
        }
    }
    let pins = snapshot.pins.as_ref().cloned().unwrap_or_default();
    let units: Vec<Value> = snapshot
        .unit_outcomes
        .iter()
        .map(|(unit, outcome)| unit_fact(unit, outcome, pins.reason_for(unit).is_some()))
        .collect();
    Ok(build_manifest(
        registry,
        config,
        &snapshot.desired.names,
        &snapshot.desired.excluded,
        &snapshot.desired.undecidable,
        &snapshot.statuses,
        &facts,
        &queued,
        units,
        bounded_verify(&verify),
        errors,
    ))
}

/// Pending + inflight counts for one agent, read-only. `pending` counts
/// queued prompts never yet typed; `inflight` counts queue entries claimed
/// for delivery. Either side is `null` when it could not be read.
fn queued_census<A: ManagedApi + ?Sized>(
    manager: &ManagedAgents<'_, A>,
    name: &str,
    status: Option<&Value>,
) -> Value {
    let (pending, error) = match manager.pending_prompts(name) {
        Ok(prompts) => (json!(prompts.len()), Value::Null),
        Err(_) => (Value::Null, json!("queued census unavailable")),
    };
    let inflight = status
        .and_then(|value| value["queue"].as_str())
        .and_then(|queue| count_json_entries(&Path::new(queue).join("inflight")))
        .map(|count| json!(count))
        .unwrap_or(Value::Null);
    let archived = status
        .and_then(|value| value["queued_archived_undelivered"].as_u64())
        .unwrap_or(0);
    json!({
        "pending": pending,
        "inflight": inflight,
        "archived_undelivered": archived,
        "error": error,
    })
}

fn count_json_entries(dir: &Path) -> Option<u64> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut count: u64 = 0;
    for entry in entries.flatten() {
        if entry.path().extension().is_some_and(|ext| ext == "json") {
            count += 1;
        }
    }
    Some(count)
}

/// `units[]` booleans from the snapshot's one `is-active` outcome, judged
/// by the same `unit_row` verify uses. Evidence is a closed class, never
/// the pin reason or systemctl output.
fn unit_fact(unit: &str, outcome: &CommandOutcome, held: bool) -> Value {
    let row = unit_row(unit, outcome, held.then_some("pin"));
    let status = row["status"].as_str().unwrap_or_default();
    let evidence = match status {
        "pass" if held => "active; held by turnover pin",
        "pass" => "active",
        "held" => "held by turnover pin",
        _ => "not active",
    };
    json!({
        "name": unit,
        "active": status == "pass",
        "held": status == "held" || (status == "pass" && held),
        "status": status,
        "evidence": evidence,
    })
}

/// Assemble the manifest document from gathered facts (pure).
#[allow(clippy::too_many_arguments)]
fn build_manifest(
    registry: &Path,
    config: &RestoreConfig,
    desired: &[String],
    excluded: &[String],
    undecidable: &[Value],
    statuses: &[(String, std::result::Result<Value, String>)],
    facts: &[(String, Option<ManifestRecordFacts>)],
    queued: &[(String, Value)],
    units: Vec<Value>,
    verify: Value,
    errors: Vec<String>,
) -> Value {
    let mut agents: Vec<Value> = Vec::new();
    let mut gap_rows: Vec<Value> = Vec::new();
    for row in undecidable {
        let name = row["subject"].as_str().unwrap_or("unknown agent");
        gap_rows.push(unreadable_gap(name, "record lifecycle could not be read"));
    }
    for name in desired {
        let status = statuses
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, found)| found);
        let fact = facts
            .iter()
            .find(|(known, _)| known == name)
            .and_then(|(_, found)| found.as_ref());
        match status {
            Some(Ok(status)) => {
                let census = queued
                    .iter()
                    .find(|(known, _)| known == name)
                    .map(|(_, census)| census.clone())
                    .unwrap_or(Value::Null);
                agents.push(project_agent(name, status, census, fact));
                match fact {
                    Some(fact) => {
                        if let Some(gaps) = record_gaps(name, status, fact) {
                            gap_rows.push(gaps);
                        }
                    }
                    None => gap_rows.push(unreadable_gap(name, "record facts could not be read")),
                }
            }
            Some(Err(_)) => {
                agents.push(json!({
                    "name": name,
                    "liveness": "unknown",
                    "error": "status unavailable",
                }));
                gap_rows.push(unreadable_gap(name, "record status could not be read"));
            }
            None => {
                agents.push(json!({
                    "name": name,
                    "liveness": "unknown",
                    "error": "status unavailable",
                }));
                gap_rows.push(unreadable_gap(name, "record has no status"));
            }
        }
    }
    json!({
        "schema": MANIFEST_SCHEMA,
        "registry": registry,
        "desired_state_source": if config.agents.is_some() { "pinned-list" } else { "registry-derived" },
        "coordinator": config.coordinator.clone(),
        "epoch": verify["epoch"].clone(),
        "boot_id": verify["boot_id"].clone(),
        "checked_at": verify["checked_at"].clone(),
        "ok": verify_ok(&verify),
        "desired": desired,
        "excluded": excluded,
        "agents": agents,
        "record_gaps": gap_rows,
        "units": units,
        "verify": verify,
        "lost": ["in-session subagents and backgrounded work of any dead session are unrecoverable; relaunch from coordinator notes after handoff"],
        "errors": errors,
    })
}

/// One `agents[]` row: the allowlist projection of a status row, with
/// declared effort/session/profile facts patched from the raw record
/// where a shape's status row omits them (agentcloud above all).
fn project_agent(
    name: &str,
    status: &Value,
    queued: Value,
    fact: Option<&ManifestRecordFacts>,
) -> Value {
    let mut row = json!({
        "name": name,
        "liveness": liveness(status),
        "native_session_present": fact
            .map(|fact| fact.native_session_present)
            .unwrap_or_else(|| session_present(status)),
        "queued": queued,
    });
    for key in AGENT_KEYS {
        row[key] = status.get(key).cloned().unwrap_or(Value::Null);
    }
    if let Some(fact) = fact {
        row["profile"] = json!(fact.profile);
        row["declared_effort"] = json!(fact.declared_effort);
        if row["effective_effort"].is_null() {
            row["effective_effort"] = json!(fact.declared_effort);
        }
        row["profile_state"] = json!(profile_state_name(&fact.profile_state));
        if let ManifestProfileState::Drifted(fields) = &fact.profile_state {
            row["profile_drift_fields"] = json!(fields);
        }
    }
    if status["probe_error"]
        .as_str()
        .is_some_and(|error| !error.is_empty())
    {
        row["probe_error"] = json!("probe unavailable");
    }
    row
}

fn profile_state_name(state: &ManifestProfileState) -> &'static str {
    match state {
        ManifestProfileState::Absent => "absent",
        ManifestProfileState::Current => "current",
        ManifestProfileState::Drifted(_) => "drifted",
        ManifestProfileState::Missing => "missing",
        ManifestProfileState::Incompatible => "incompatible",
        ManifestProfileState::Unreadable => "unreadable",
        ManifestProfileState::NotApplicable => "not-applicable",
    }
}

fn liveness(status: &Value) -> &'static str {
    if status
        .get("probe_error")
        .is_some_and(|value| !value.is_null())
    {
        return "unknown";
    }
    match status["agent_status"].as_str() {
        Some("running") | Some("working") | Some("idle") | Some("blocked") => "alive",
        Some("dead") | Some("stopped") | Some("missing") => "dead",
        _ if status["lifecycle"].as_str() == Some("running") => "alive",
        _ => "unknown",
    }
}

/// Whether the record carries a recorded native session revive could use.
fn session_present(status: &Value) -> bool {
    if status["native_session_missing"] == json!(true) {
        return false;
    }
    let non_empty = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty())
    };
    match status.get("native_session") {
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Object(map)) => ["value", "session", "id", "session_id"]
            .iter()
            .any(|key| non_empty(map.get(*key))),
        _ => {
            non_empty(status.pointer("/launch_spec/native_session/value"))
                || non_empty(status.pointer("/launch/native_session/value"))
                || non_empty(status.get("session"))
                || non_empty(status.get("session_id"))
                || non_empty(status.get("session_value"))
                || non_empty(status.get("resume"))
                || non_empty(status.get("native_session_id"))
        }
    }
}

/// A `record_gaps` row for a record no reader could judge at all.
fn unreadable_gap(name: &str, evidence: &str) -> Value {
    json!({
        "name": name,
        "missing": ["unreadable"],
        "evidence": evidence,
    })
}

/// `record_gaps` row for one agent, or `None` when the record is complete
/// (owner 257). Every judgement comes from the raw record through
/// `ManifestRecordFacts`; the redacted status row is never asked for argv,
/// declared effort, or a native session it may not even carry.
fn record_gaps(name: &str, status: &Value, fact: &ManifestRecordFacts) -> Option<Value> {
    let mut missing: Vec<&str> = Vec::new();
    let mut details: Vec<String> = Vec::new();
    match &fact.profile_state {
        ManifestProfileState::Absent => {
            missing.push("profile");
            details.push("no launch profile recorded".to_owned());
        }
        ManifestProfileState::Missing => {
            missing.push("profile");
            details.push("named launch profile is no longer defined".to_owned());
        }
        ManifestProfileState::Incompatible => {
            missing.push("profile");
            details.push(
                "named launch profile now names another harness, mode, or cloud launch".to_owned(),
            );
        }
        ManifestProfileState::Unreadable => {
            missing.push("unreadable");
            details.push("launch profile could not be read".to_owned());
        }
        ManifestProfileState::Drifted(fields) => {
            missing.push("profile-drift");
            details.push(format!("launch profile differs in {}", fields.join(", ")));
        }
        ManifestProfileState::Current | ManifestProfileState::NotApplicable => {}
    }
    if fact.adopted_without_launch_spec {
        missing.push("launch-spec");
        details.push("adopted record has no launch spec; anchor it before revive".to_owned());
    }
    if let Some(flag) = fact.missing_bypass_flag {
        missing.push("bypass");
        details.push(format!(
            "no permission-bypass flag on the argv revive would replay (needs {flag})"
        ));
    }
    if fact
        .declared_effort
        .as_deref()
        .is_none_or(|text| text.is_empty())
    {
        missing.push("effort");
        details.push("no declared reasoning effort".to_owned());
    }
    if !fact.native_session_present {
        missing.push("session");
        details.push("no recorded native session (candidates are report-only)".to_owned());
    }
    let _ = status;
    if missing.is_empty() {
        return None;
    }
    let mut row = json!({
        "name": name,
        "missing": missing,
        "evidence": details.join("; "),
    });
    if let ManifestProfileState::Drifted(fields) = &fact.profile_state {
        row["profile_drift_fields"] = json!(fields);
    }
    Some(row)
}

/// The embedded verify copy the manifest may carry: closed fields only.
/// Standalone `team verify` keeps its detailed evidence contract; this
/// projection drops alarm bodies and replaces check free text (pane
/// questions, bridge health lines, pin reasons, probe errors) with stable
/// classes or short generated lines, so no pane/chat/secret text can ride
/// into a recovery brief.
fn bounded_verify(verify: &Value) -> Value {
    let checks: Vec<Value> = verify["checks"]
        .as_array()
        .map(|rows| rows.iter().take(512).map(bounded_check).collect())
        .unwrap_or_default();
    json!({
        "schema": verify["schema"].clone(),
        "registry": verify["registry"].clone(),
        "epoch": {"boot_id": verify["boot_id"].clone()},
        "boot_id": verify["boot_id"].clone(),
        "checked_at": verify["checked_at"].clone(),
        "ok": verify["ok"].clone(),
        "desired": verify["desired"].clone(),
        "excluded": verify["excluded"].clone(),
        "alarm": {"present": !verify["alarm"].is_null()},
        "checks": checks,
    })
}

fn bounded_check(row: &Value) -> Value {
    let raw_name = row["name"].as_str().unwrap_or_default();
    let name = match raw_name {
        "desired-live"
        | "units-bridges"
        | "queued-not-archived"
        | "watcher-dead-agents"
        | "pane-at-prompt"
        | "coordinator-ack" => raw_name,
        _ => "unknown-check",
    };
    let raw_status = row["status"].as_str().unwrap_or_default();
    let status = match raw_status {
        "pass" | "fail" | "skipped" | "held" => raw_status,
        _ => "unknown-status",
    };
    let subject = row["subject"].as_str().map(|text| bounded_line(text, 80));
    let evidence = bounded_evidence(
        name,
        status,
        subject.as_deref(),
        row["evidence"].as_str().unwrap_or_default(),
    );
    let fix = if name == "unknown-check" {
        Value::Null
    } else {
        row["fix"]
            .as_str()
            .map(|text| json!(bounded_line(text, 120)))
            .unwrap_or(Value::Null)
    };
    json!({
        "name": name,
        "status": status,
        "subject": subject,
        "evidence": evidence,
        "fix": fix,
    })
}

fn bounded_evidence(name: &str, status: &str, subject: Option<&str>, evidence: &str) -> String {
    let subject = subject.unwrap_or("team");
    match name {
        "desired-live" => {
            if status == "pass" {
                format!("{subject} live")
            } else if evidence.contains("profile drift") {
                format!("{subject} runs with profile drift")
            } else if evidence.contains("no status") {
                format!("{subject} has no status")
            } else if evidence.contains("unreadable") {
                format!("{subject} status unavailable")
            } else {
                format!("{subject} is not live")
            }
        }
        "units-bridges" => {
            if status == "held" {
                "held by turnover pin".to_owned()
            } else if subject.starts_with("bridge ") {
                match status {
                    "pass" => format!("{subject} chat health ok"),
                    "skipped" => "no bridge evidence".to_owned(),
                    _ => format!("{subject} chat health not ok"),
                }
            } else {
                match status {
                    "pass" => format!("{subject} active"),
                    "skipped" => "no units or bridges configured".to_owned(),
                    _ => format!("{subject} not active"),
                }
            }
        }
        "pane-at-prompt" => {
            if status == "pass" && evidence.contains("mid-turn") {
                "pane mid-turn with a follow-up editor".to_owned()
            } else if status == "pass" {
                "pane at an input prompt".to_owned()
            } else if status == "skipped" {
                "no pane prompt to judge".to_owned()
            } else if evidence.contains("blocked behind a question") {
                "blocked behind a question".to_owned()
            } else if evidence.contains("menu or picker") {
                "blocked behind a menu or picker".to_owned()
            } else if evidence.contains("dialog") {
                "blocked behind a dialog".to_owned()
            } else if evidence.contains("unreadable") {
                "pane unreadable".to_owned()
            } else {
                "pane not at a prompt".to_owned()
            }
        }
        "watcher-dead-agents" => {
            if status == "pass" {
                "watcher fresh this boot and ok".to_owned()
            } else if evidence.contains("no readable watch health") {
                "no readable watch health".to_owned()
            } else if evidence.contains("belongs to boot") {
                "watch health belongs to another boot".to_owned()
            } else if evidence.contains("old") {
                "watch health stale".to_owned()
            } else {
                "watcher reports not ok".to_owned()
            }
        }
        // These two checks' evidence is generated from counts, names, and
        // queue states only; a short bound keeps it that way.
        "queued-not-archived" | "coordinator-ack" => bounded_line(evidence, 160),
        _ => "details omitted in manifest".to_owned(),
    }
}

/// One line, bounded in characters, for generated (non-free-text) lines.
fn bounded_line(text: &str, max_chars: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() > max_chars {
        format!("{}...", joined.chars().take(max_chars).collect::<String>())
    } else if joined.is_empty() {
        "no evidence recorded".to_owned()
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status_row(extra: Value) -> Value {
        let mut row = json!({
            "name": "lane",
            "harness": "claude",
            "profile": "worker",
            "cwd": "/tmp/proj",
            "lifecycle": "running",
            "agent_status": "running",
            "pane_id": "w1:p1",
            "declared_effort": "xhigh",
            "effective_effort": "xhigh",
            "declared_model": "m",
            "effective_model": "m",
            "native_session": {"value": "sess-1", "source": "observed"},
            "token": "SECRET-TOKEN",
            "arguments": ["--model", "<set>"],
            "launch_spec": {"argv": ["--model", "<set>"]},
            "queue": "/tmp/queue",
        });
        if let (Value::Object(base), Value::Object(patch)) = (&mut row, extra) {
            base.extend(patch);
        }
        row
    }

    fn complete_facts() -> ManifestRecordFacts {
        ManifestRecordFacts {
            profile: Some("worker".to_owned()),
            profile_state: ManifestProfileState::Current,
            declared_effort: Some("xhigh".to_owned()),
            native_session_present: true,
            missing_bypass_flag: None,
            adopted_without_launch_spec: false,
        }
    }

    #[test]
    fn projection_never_carries_secrets_or_argv() {
        let row = project_agent(
            "lane",
            &status_row(json!({})),
            json!({"pending": 2}),
            Some(&complete_facts()),
        );
        assert_eq!(row["name"], "lane");
        assert_eq!(row["liveness"], "alive");
        assert_eq!(row["native_session_present"], true);
        assert_eq!(row["queued"]["pending"], 2);
        let text = row.to_string();
        assert!(!text.contains("SECRET-TOKEN"), "token leaked: {text}");
        assert!(row.get("token").is_none());
        assert!(row.get("arguments").is_none());
        assert!(row.get("launch_spec").is_none());
        assert!(row.get("queue").is_none());
    }

    #[test]
    fn complete_record_has_no_gaps() {
        assert!(record_gaps("lane", &status_row(json!({})), &complete_facts()).is_none());
    }

    #[test]
    fn gaps_name_every_missing_piece() {
        let facts = ManifestRecordFacts {
            profile: None,
            profile_state: ManifestProfileState::Absent,
            declared_effort: None,
            native_session_present: false,
            missing_bypass_flag: Some(
                "--dangerously-skip-permissions (or --permission-mode bypassPermissions)",
            ),
            adopted_without_launch_spec: false,
        };
        let gaps = record_gaps("lane", &status_row(json!({})), &facts).expect("gaps");
        assert_eq!(
            gaps["missing"],
            json!(["profile", "bypass", "effort", "session"])
        );
        assert!(gaps["evidence"]
            .as_str()
            .unwrap()
            .contains("--dangerously-skip-permissions"));
    }

    #[test]
    fn gaps_come_from_internal_facts_not_redacted_status() {
        // A redacted pair-form bypass in the status row must not be judged
        // here at all: the internal facts already judged the raw record.
        let row = status_row(json!({
            "harness": "muse",
            "profile": null,
            "declared_effort": null,
            "native_session": null,
            "arguments": ["--approval-mode", "<set>"],
        }));
        let mut facts = complete_facts();
        facts.profile = None;
        facts.profile_state = ManifestProfileState::Absent;
        let gaps = record_gaps("lane", &row, &facts).expect("profile gap only");
        assert_eq!(gaps["missing"], json!(["profile"]));
    }

    #[test]
    fn profile_states_are_named() {
        for (state, missing, evidence) in [
            (
                ManifestProfileState::Missing,
                json!(["profile"]),
                "no longer defined",
            ),
            (
                ManifestProfileState::Incompatible,
                json!(["profile"]),
                "another harness",
            ),
            (
                ManifestProfileState::Unreadable,
                json!(["unreadable"]),
                "could not be read",
            ),
            (
                ManifestProfileState::Drifted(vec!["argv"]),
                json!(["profile-drift"]),
                "differs in argv",
            ),
        ] {
            let mut facts = complete_facts();
            facts.profile_state = state;
            let gaps = record_gaps("lane", &status_row(json!({})), &facts).expect("gaps");
            assert_eq!(gaps["missing"], missing);
            assert!(
                gaps["evidence"].as_str().unwrap().contains(evidence),
                "{gaps}"
            );
        }
        let mut facts = complete_facts();
        facts.profile_state = ManifestProfileState::Drifted(vec!["argv"]);
        let gaps = record_gaps("lane", &status_row(json!({})), &facts).unwrap();
        assert_eq!(gaps["profile_drift_fields"], json!(["argv"]));
    }

    #[test]
    fn adopted_without_spec_reports_spec_not_a_guessed_bypass() {
        let mut facts = complete_facts();
        facts.profile = None;
        facts.profile_state = ManifestProfileState::Absent;
        facts.adopted_without_launch_spec = true;
        let gaps = record_gaps("lane", &status_row(json!({})), &facts).unwrap();
        assert_eq!(gaps["missing"], json!(["profile", "launch-spec"]));
    }

    #[test]
    fn cloud_declared_effort_comes_from_record_facts() {
        let row = status_row(json!({
            "adapter": "agentcloud",
            "declared_effort": null,
            "effective_effort": null,
        }));
        let mut facts = complete_facts();
        facts.profile_state = ManifestProfileState::NotApplicable;
        let projected = project_agent("lane", &row, json!({}), Some(&facts));
        assert_eq!(projected["declared_effort"], "xhigh");
        assert_eq!(projected["effective_effort"], "xhigh");
        assert!(record_gaps("lane", &row, &facts).is_none());
    }

    #[test]
    fn session_presence_handles_record_shapes() {
        assert!(session_present(&status_row(json!({}))));
        assert!(session_present(&status_row(
            json!({"native_session": "sess-2"})
        )));
        assert!(!session_present(&status_row(
            json!({"native_session": null})
        )));
        assert!(session_present(&status_row(json!({
            "native_session": null,
            "launch_spec": {"native_session": {"value": "sess-3", "source": "caller"}},
        }))));
        assert!(session_present(&status_row(json!({
            "native_session": null,
            "session": "sess-4",
        }))));
        assert!(!session_present(&status_row(
            json!({"native_session_missing": true})
        )));
    }

    #[test]
    fn build_manifest_reports_errors_and_failed_statuses() {
        let registry = Path::new("/tmp/registry");
        let config = RestoreConfig::load(registry).unwrap();
        let statuses = vec![
            ("lane".to_owned(), Ok(status_row(json!({})))),
            ("gone".to_owned(), Err("boom SECRET".to_owned())),
        ];
        let facts = vec![
            ("lane".to_owned(), Some(complete_facts())),
            ("gone".to_owned(), None),
        ];
        let undecidable = vec![json!({
            "name": "desired-live",
            "status": "fail",
            "subject": "broken",
            "evidence": "cannot derive desired state: SECRET",
            "fix": "inspect",
        })];
        let queued = vec![
            ("lane".to_owned(), json!({"pending": 0})),
            ("gone".to_owned(), Value::Null),
        ];
        let verify = json!({
            "schema": "agentctl-team-verify/v1",
            "ok": false,
            "epoch": {"boot_id": "boot-a"},
            "boot_id": "boot-a",
            "checked_at": 1.0,
            "checks": [],
        });
        let manifest = build_manifest(
            registry,
            &config,
            &["lane".to_owned(), "gone".to_owned()],
            &["parked".to_owned()],
            &undecidable,
            &statuses,
            &facts,
            &queued,
            Vec::new(),
            verify,
            vec!["gone: status unavailable".to_owned()],
        );
        assert_eq!(manifest["schema"], MANIFEST_SCHEMA);
        assert_eq!(manifest["desired_state_source"], "registry-derived");
        assert_eq!(manifest["ok"], false);
        assert_eq!(manifest["agents"][1]["error"], "status unavailable");
        assert_eq!(manifest["excluded"], json!(["parked"]));
        assert_eq!(manifest["errors"].as_array().unwrap().len(), 1);
        assert_eq!(manifest["record_gaps"][0]["name"], "broken");
        assert_eq!(manifest["record_gaps"][0]["missing"], json!(["unreadable"]));
        assert_eq!(manifest["record_gaps"][1]["name"], "gone");
        assert_eq!(manifest["record_gaps"][1]["missing"], json!(["unreadable"]));
        assert!(!manifest.to_string().contains("SECRET"), "{manifest}");
    }

    #[test]
    fn bounded_verify_drops_free_text_and_alarm_bodies() {
        let verify = json!({
            "schema": "agentctl-team-verify/v1",
            "registry": "/tmp/registry",
            "epoch": {"boot_id": "boot-a", "user_manager": "pid:1"},
            "boot_id": "boot-a",
            "checked_at": 1.0,
            "ok": false,
            "desired": ["lane"],
            "excluded": [],
            "alarm": {"message": "ALARM-SECRET"},
            "checks": [
                {"name": "pane-at-prompt", "status": "fail", "subject": "lane",
                 "evidence": "lane: blocked behind a question: PANE-SECRET",
                 "fix": "look at lane"},
                {"name": "units-bridges", "status": "fail", "subject": "bridge chat",
                 "evidence": "bridge chat chat health down: BRIDGE-SECRET",
                 "fix": "read agentctl chat health --bridge-state /tmp/bridge"},
                {"name": "units-bridges", "status": "held", "subject": "unit.service",
                 "evidence": "held (pin: PIN-SECRET)", "fix": null},
                {"name": "desired-live", "status": "fail", "subject": "lane",
                 "evidence": "status of lane unreadable: PROBE-SECRET",
                 "fix": "read status"},
                {"name": "future-check", "status": "weird", "subject": "lane",
                 "evidence": "FUTURE-SECRET", "fix": "FUTURE-FIX-SECRET with a very long tail that should still be bounded"},
            ],
        });
        let bounded = bounded_verify(&verify);
        let text = bounded.to_string();
        for marker in [
            "ALARM-SECRET",
            "PANE-SECRET",
            "BRIDGE-SECRET",
            "PIN-SECRET",
            "PROBE-SECRET",
            "FUTURE-SECRET",
        ] {
            assert!(!text.contains(marker), "{marker} leaked: {text}");
        }
        assert_eq!(bounded["alarm"], json!({"present": true}));
        assert_eq!(bounded["epoch"], json!({"boot_id": "boot-a"}));
        assert_eq!(
            bounded["checks"][0]["evidence"],
            "blocked behind a question"
        );
        assert_eq!(
            bounded["checks"][1]["evidence"],
            "bridge chat chat health not ok"
        );
        assert_eq!(bounded["checks"][4]["name"], "unknown-check");
        assert_eq!(bounded["checks"][4]["status"], "unknown-status");
        assert_eq!(
            bounded["checks"][4]["evidence"],
            "details omitted in manifest"
        );
    }
}
