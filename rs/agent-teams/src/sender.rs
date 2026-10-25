//! The sender side: outbox, the route walk, and `pump`
//! (design sections 8.4, 9.3, 9.4 and 11.1).

use crate::jsonfile::{write_json_atomic, DataResult};
use crate::model::{Contact, Entry, Host};
use crate::route::{AttemptOutcome, Route};
use crate::timeutil::{format_rfc3339_ms, now_unix_ms, parse_rfc3339_ms};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// The outbox directory.
pub fn outbox_dir(state: &Path) -> PathBuf {
    state.join("outbox")
}

/// Path of one outbox entry.
pub fn outbox_path(state: &Path, id: &str) -> PathBuf {
    outbox_dir(state).join(format!("{id}.json"))
}

/// Read an outbox entry.
pub fn read_outbox(state: &Path, id: &str) -> Option<Value> {
    let text = fs::read_to_string(outbox_path(state, id)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Find the outbox entry for a (sender, addressed-to, key) triple, if any.
pub fn find_by_key(state: &Path, addressed_to: &str, key: &str) -> Option<Value> {
    let dir = outbox_dir(state);
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        if !crate::ledger::is_entry_file_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if value.get("addressed_to").and_then(Value::as_str) == Some(addressed_to)
            && value["envelope"].get("key").and_then(Value::as_str) == Some(key)
        {
            return Some(value);
        }
    }
    None
}

/// Write a new outbox entry (envelope, proof, attempts so far).
pub fn write_outbox(
    state: &Path,
    envelope: &Value,
    addressed_to: &str,
    proof: &Value,
) -> DataResult<Value> {
    let id = envelope["id"].as_str().unwrap_or_default();
    let entry = json!({
        "id": id,
        "addressed_to": addressed_to,
        "envelope": envelope,
        "proof": proof,
        "state": "deferred",
        "pinned_contact": Value::Null,
        "attempts": [],
        "created_at": format_rfc3339_ms(now_unix_ms()),
    });
    write_json_atomic(&outbox_path(state, id), &entry)?;
    Ok(entry)
}

fn record_attempt(
    entry: &mut Value,
    contact: &str,
    route: &str,
    verb: &str,
    class: &str,
    detail: &str,
) {
    let attempt = json!({
        "at": format_rfc3339_ms(now_unix_ms()),
        "contact": contact,
        "route": route,
        "verb": verb,
        "classification": class,
        "detail": detail,
    });
    if let Some(list) = entry["attempts"].as_array_mut() {
        list.push(attempt);
    }
}

/// One step of the walk, printed by `send`.
#[derive(Debug, Clone)]
pub struct WalkStep {
    /// Contact label.
    pub contact: String,
    /// Route name.
    pub route: String,
    /// What happened.
    pub detail: String,
}

/// Result of a walk over an outbox entry.
#[derive(Debug, Clone)]
pub struct WalkResult {
    /// Sender-side state after the walk.
    pub state: String,
    /// Steps for display.
    pub steps: Vec<WalkStep>,
    /// Exit code per design section 16.
    pub exit_code: i32,
}

/// The sender-side exit code for a message state (design section 16).
pub fn exit_code_for(state: &str) -> i32 {
    match state {
        "delivered" | "journaled" => 0,
        "accepted" | "deferred" | "awaiting-pull" | "in-custody" | "claimed" | "typing"
        | "presented" => 75,
        "ambiguous" | "ambiguous-expired" | "possibly_submitted" | "lost" => 76,
        "expired" | "superseded" | "retracted" | "refused" | "fenced" | "journaled-halted" => 65,
        _ => 75,
    }
}

/// Whether a ledger state is terminal for the sender.
pub fn is_terminal(state: &str) -> bool {
    matches!(
        state,
        "delivered"
            | "expired"
            | "superseded"
            | "retracted"
            | "refused"
            | "fenced"
            | "journaled"
            | "journaled-halted"
    )
}

/// Run the walk for one outbox entry against its entry's contacts.
/// `routes_for` maps a host contact to its candidate routes in order.
pub fn walk(
    state: &Path,
    entry_value: &mut Value,
    team_entry: &Entry,
    host: &Host,
    routes_for: &[Box<dyn Route>],
) -> DataResult<WalkResult> {
    let envelope = entry_value["envelope"].clone();
    let id = envelope["id"].as_str().unwrap_or_default().to_string();
    let envelope_bytes = envelope.to_string().len();
    let not_after_ms = envelope["not_after"]
        .as_str()
        .and_then(parse_rfc3339_ms)
        .unwrap_or(0);
    let mut steps = Vec::new();
    let mut final_state = entry_value["state"]
        .as_str()
        .unwrap_or("deferred")
        .to_string();
    let mut any_ambiguous = entry_value["attempts"].as_array().is_some_and(|list| {
        list.iter()
            .any(|a| a.get("classification").and_then(Value::as_str) == Some("ambiguous"))
    });
    let pinned: Option<String> = entry_value["pinned_contact"].as_str().map(str::to_string);

    'contacts: for contact in &team_entry.contacts {
        let label = match contact {
            Contact::Host => format!("host@{}", team_entry.host),
            Contact::Session { .. } => contact.label(),
        };
        if let Some(pinned_label) = &pinned {
            if &label != pinned_label {
                continue;
            }
        }
        if !matches!(contact, Contact::Host) {
            // Session contacts ride command routes in session mode; not yet
            // driven by this walk. Recorded as a definite skip of this walk.
            steps.push(WalkStep {
                contact: label.clone(),
                route: "-".to_string(),
                detail: "session contacts are not driven yet".to_string(),
            });
            continue;
        }
        let mut contact_released = false;
        let mut contact_settled: Option<String> = None;
        for route in routes_for {
            if let Some(reason) = route.skip_reason(envelope_bytes) {
                steps.push(WalkStep {
                    contact: label.clone(),
                    route: route.name().to_string(),
                    detail: format!("skipped ({reason})"),
                });
                record_attempt(
                    entry_value,
                    &label,
                    route.name(),
                    "accept",
                    "not-delivered",
                    &reason,
                );
                continue;
            }
            if envelope["private"].as_bool() == Some(true) && route.exposes_body() {
                steps.push(WalkStep {
                    contact: label.clone(),
                    route: route.name().to_string(),
                    detail: "skipped (--private: route exposes the body)".to_string(),
                });
                continue;
            }
            // The attempt record is written before the route runs (section 11.1).
            record_attempt(entry_value, &label, route.name(), "accept", "started", "");
            write_json_atomic(&outbox_path(state, &id), entry_value)?;
            let outcome = route.run_verb("accept", &envelope);
            match outcome {
                AttemptOutcome::Answered(answer) => {
                    let answered_outcome = answer
                        .get("outcome")
                        .and_then(Value::as_str)
                        .unwrap_or("accepted");
                    // Record what the receiver actually said: a refusal of
                    // the route carries no state, and must never be logged
                    // as "accepted" (live-trial finding, 2026-10-25).
                    let answered_state = answer
                        .get("state")
                        .and_then(Value::as_str)
                        .unwrap_or(answered_outcome)
                        .to_string();
                    record_attempt(
                        entry_value,
                        &label,
                        route.name(),
                        "accept",
                        "answered",
                        &answered_state,
                    );
                    steps.push(WalkStep {
                        contact: label.clone(),
                        route: route.name().to_string(),
                        detail: format!("{answered_outcome}: {answered_state}"),
                    });
                    if answered_outcome == "route-refused" {
                        // Definite for this attempt only; try the next route.
                        continue;
                    }
                    if answered_outcome == "conflict" {
                        entry_value["pinned_contact"] = json!(label);
                        contact_settled = Some("refused".to_string());
                        steps.push(WalkStep {
                            contact: label.clone(),
                            route: route.name().to_string(),
                            detail: "conflict: this ID already holds a different body".to_string(),
                        });
                        break;
                    }
                    if answered_outcome == "error" {
                        // The receiver could not process it; nothing was
                        // necessarily accepted. Ask later, never settle.
                        any_ambiguous = true;
                        entry_value["pinned_contact"] = json!(label);
                        contact_settled = Some("ambiguous".to_string());
                        break;
                    }
                    entry_value["pinned_contact"] = json!(label);
                    contact_settled = Some(map_receiver_state(&answered_state));
                    break;
                }
                AttemptOutcome::NotDelivered(reason) => {
                    record_attempt(
                        entry_value,
                        &label,
                        route.name(),
                        "accept",
                        "not-delivered",
                        &reason,
                    );
                    steps.push(WalkStep {
                        contact: label.clone(),
                        route: route.name().to_string(),
                        detail: format!("not-delivered ({reason})"),
                    });
                }
                AttemptOutcome::Ambiguous(reason) => {
                    any_ambiguous = true;
                    record_attempt(
                        entry_value,
                        &label,
                        route.name(),
                        "accept",
                        "ambiguous",
                        &reason,
                    );
                    steps.push(WalkStep {
                        contact: label.clone(),
                        route: route.name().to_string(),
                        detail: format!("ambiguous ({reason}); asking"),
                    });
                    // Ask before resending (section 9.3).
                    let asked = ask_receipt(
                        entry_value,
                        routes_for,
                        &label,
                        &id,
                        envelope["to"]["team"].as_str().unwrap_or_default(),
                    );
                    match asked {
                        Some(known) if known["known"].as_bool() == Some(true) => {
                            let known_state =
                                known["state"].as_str().unwrap_or("accepted").to_string();
                            entry_value["pinned_contact"] = json!(label);
                            contact_settled = Some(map_receiver_state(&known_state));
                            steps.push(WalkStep {
                                contact: label.clone(),
                                route: route.name().to_string(),
                                detail: format!("receipt: {known_state}"),
                            });
                        }
                        Some(_) => {
                            // Unknown at the receiver: resend once, same ID.
                            steps.push(WalkStep {
                                contact: label.clone(),
                                route: route.name().to_string(),
                                detail: "receipt: unknown; resending once with the same ID"
                                    .to_string(),
                            });
                            record_attempt(
                                entry_value,
                                &label,
                                route.name(),
                                "accept",
                                "started",
                                "resend after unknown",
                            );
                            match route.run_verb("accept", &envelope) {
                                AttemptOutcome::Answered(answer) => {
                                    let st = answer["state"].as_str().unwrap_or("accepted");
                                    entry_value["pinned_contact"] = json!(label);
                                    contact_settled = Some(map_receiver_state(st));
                                }
                                _ => {
                                    contact_settled = Some("ambiguous".to_string());
                                }
                            }
                        }
                        None => {
                            contact_settled = Some("ambiguous".to_string());
                        }
                    }
                    break;
                }
            }
        }
        if let Some(settled) = contact_settled {
            final_state = settled;
            break 'contacts;
        }
        // All routes definite and undelivered: the contact is released only
        // when no attempt to it was ever ambiguous (else a fence is needed).
        if !any_ambiguous {
            contact_released = true;
        }
        if contact_released {
            continue 'contacts;
        }
        final_state = "ambiguous".to_string();
        break 'contacts;
    }

    if (final_state == "deferred" || final_state == "ambiguous")
        && not_after_ms > 0
        && now_unix_ms() > not_after_ms
    {
        final_state = if any_ambiguous {
            "ambiguous-expired".to_string()
        } else {
            "expired".to_string()
        };
    }
    let _ = host;
    entry_value["state"] = json!(final_state);
    write_json_atomic(&outbox_path(state, &id), entry_value)?;
    Ok(WalkResult {
        state: final_state.clone(),
        steps,
        exit_code: exit_code_for(&final_state),
    })
}

/// Map a receiver ledger state onto the sender-side state.
pub fn map_receiver_state(state: &str) -> String {
    match state {
        "fenced" => "refused".to_string(),
        other => other.to_string(),
    }
}

/// Ask the receiver for a receipt over the first route that answers.
fn ask_receipt(
    entry_value: &mut Value,
    routes: &[Box<dyn Route>],
    label: &str,
    id: &str,
    team: &str,
) -> Option<Value> {
    let payload = json!({"id": id, "team": team});
    for route in routes {
        // A skipped route is not this contact's authority: its ledger (for
        // example this host's own, via a skipped local route) answering
        // "unknown" must never license a resend (live-trial finding,
        // 2026-10-25). The walk's accept loop already honours skips.
        if route.skip_reason(0).is_some() {
            continue;
        }
        match route.run_verb("receipt", &payload) {
            AttemptOutcome::Answered(answer) => {
                record_attempt(
                    entry_value,
                    label,
                    route.name(),
                    "receipt",
                    "answered",
                    answer["state"].as_str().unwrap_or("?"),
                );
                return Some(answer);
            }
            AttemptOutcome::NotDelivered(reason) => {
                record_attempt(
                    entry_value,
                    label,
                    route.name(),
                    "receipt",
                    "not-delivered",
                    &reason,
                );
            }
            AttemptOutcome::Ambiguous(reason) => {
                record_attempt(
                    entry_value,
                    label,
                    route.name(),
                    "receipt",
                    "ambiguous",
                    &reason,
                );
            }
        }
    }
    None
}

/// The next per-host sender sequence number (topic order only, section 8.1).
pub fn next_seq(state: &Path) -> i64 {
    let path = state.join("seq.json");
    let current: i64 = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get("seq")?.as_i64())
        .unwrap_or(0);
    let next = current + 1;
    let _ = write_json_atomic(&path, &json!({"seq": next}));
    next
}

/// Append one line to `team-log.jsonl` (section 13; never the body).
pub fn log_event(state: &Path, event: &Value) {
    use std::io::Write;
    let _ = fs::create_dir_all(state);
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.join("team-log.jsonl"))
    {
        let _ = writeln!(file, "{}", event);
    }
}
