//! The receiver ledger (the durable seen-set) and `accept`: atomic claim
//! before hand-over, fences, and crash recovery (design sections 8.3, 8.4
//! and 9.2). One file per ID under `teams.d/<team>/ledger/`.

use crate::deliver::{Deliverer, Handover};
use crate::envelope::{check_envelope, envelope_sender, render, sha256_hex};
use crate::jsonfile::{
    create_private_dir_all, write_json_atomic, write_new, write_new_atomic, DataResult,
};
use crate::model::Declaration;
use crate::timeutil::{format_rfc3339_ms, now_unix_ms, parse_rfc3339_ms};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Where a team's ledger lives.
pub fn ledger_dir(state: &Path, team: &str) -> PathBuf {
    state.join("teams.d").join(team).join("ledger")
}

/// Path of one ledger entry.
pub fn entry_path(state: &Path, team: &str, id: &str) -> PathBuf {
    ledger_dir(state, team).join(format!("{id}.json"))
}

/// Whether a directory entry name is exactly `<message-id>.json` (not a
/// backup `<id>.json.<stamp>` left by an atomic rewrite, nor a temp file).
/// Directory scans must use this: backups parse as stale entries and would
/// otherwise be walked, listed, or superseded a second time (live-trial
/// finding, 2026-10-25: pump walked the same outbox entry twice).
pub fn is_entry_file_name(name: &str) -> bool {
    name.strip_suffix(".json")
        .is_some_and(crate::envelope::valid_message_id)
}

/// Read a ledger entry, if present. A pure read: never recovers (section 9.2).
pub fn read_entry(state: &Path, team: &str, id: &str) -> Option<Value> {
    let text = fs::read_to_string(entry_path(state, team, id)).ok()?;
    serde_json::from_str(&text).ok()
}

/// The state recorded in an entry, if any.
pub fn entry_state(entry: &Value) -> &str {
    entry.get("state").and_then(Value::as_str).unwrap_or("?")
}

/// Take the ledger's short lock (never the queue's delivery lock).
fn lock_ledger(dir: &Path) -> DataResult<fs::File> {
    use fs2::FileExt;
    fs::create_dir_all(dir)?;
    let file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(dir.join(".ledger.lock"))?;
    file.lock_exclusive()?;
    Ok(file)
}

fn unlock(file: fs::File) {
    let _ = fs2::FileExt::unlock(&file);
}

/// Context for one `accept` on the receiving host.
pub struct AcceptContext<'a> {
    /// State directory root.
    pub state: &'a Path,
    /// This host's key.
    pub this_host: &'a str,
    /// Declaration of the receiving team.
    pub declaration: &'a Declaration,
    /// Route the envelope arrived by (rendered and recorded).
    pub via: &'a str,
    /// How the sender was proved, as rendered.
    pub proof_note: &'a str,
    /// Hub that relayed the envelope, when any (trace checked, section 11.6).
    pub relayed_by: Option<&'a str>,
    /// Host the route authenticated (or asserted) as the origin; empty when
    /// unknown. A non-relayed envelope must claim this host as `from.host`.
    pub peer_host: &'a str,
}

/// The answer `accept` gives.
#[derive(Debug, Clone)]
pub struct AcceptAnswer {
    /// `accepted`, a known state, `refused`, `conflict`, or `route-refused`.
    pub outcome: String,
    /// Ledger state after the call, when an entry exists.
    pub state: Option<String>,
    /// Detail for the sender's log.
    pub detail: String,
    /// Receiver findings (for example `hook-only`).
    pub findings: Vec<String>,
}

impl AcceptAnswer {
    fn route_refused(detail: impl Into<String>) -> Self {
        AcceptAnswer {
            outcome: "route-refused".to_string(),
            state: None,
            detail: detail.into(),
            findings: Vec::new(),
        }
    }
    /// JSON form for the serve protocol.
    pub fn to_json(&self) -> Value {
        json!({
            "outcome": self.outcome,
            "state": self.state,
            "detail": self.detail,
            "findings": self.findings,
        })
    }
}

/// Record a refusal of the message as a durable ledger entry (a fence in
/// effect, section 9.4). Caller holds the ledger lock and has confirmed no
/// entry exists.
fn write_terminal(
    state: &Path,
    team: &str,
    id: &str,
    entry_state_name: &str,
    envelope: &Value,
    detail: &str,
) -> DataResult<()> {
    let entry = json!({
        "id": id,
        "state": entry_state_name,
        "body_sha256": envelope.get("body_sha256"),
        "sender": envelope_sender(envelope),
        "key": envelope.get("key"),
        "not_after": envelope.get("not_after"),
        "created_at": envelope.get("created_at"),
        "recorded_at": format_rfc3339_ms(now_unix_ms()),
        "detail": detail,
    });
    let _ = write_new_atomic(
        &entry_path(state, team, id),
        format!("{}\n", serde_json::to_string_pretty(&entry)?).as_bytes(),
    )?;
    Ok(())
}

/// Accept one envelope on this host. Claims the ID in the ledger before any
/// hand-over; a second arrival returns the recorded state.
pub fn accept(
    ctx: &AcceptContext<'_>,
    envelope: &Value,
    deliverer: &dyn Deliverer,
) -> DataResult<AcceptAnswer> {
    // Opportunistic retention pruning (its own ledger lock, taken and
    // released before accept's): entries past their horizon are exactly
    // the ones no live envelope can need, since any replay of them is
    // refused on not_after below.
    let _ = prune_team(ctx.state, &ctx.declaration.team, now_unix_ms());
    let team = &ctx.declaration.team;
    // Route refusals first: no ledger entry, nothing fenced (section 9.4).
    if let Err(reason) = check_envelope(envelope) {
        return Ok(AcceptAnswer::route_refused(format!(
            "envelope refused: {reason}"
        )));
    }
    let id = envelope["id"].as_str().unwrap_or_default().to_string();
    let to_team = envelope["to"]["team"].as_str().unwrap_or_default();
    let to_host = envelope["to"]["host"].as_str().unwrap_or_default();
    if to_host != ctx.this_host {
        return Ok(AcceptAnswer::route_refused(format!(
            "addressed to host {to_host}, not {}",
            ctx.this_host
        )));
    }
    if to_team != team {
        return Ok(AcceptAnswer::route_refused(format!(
            "addressed to team {to_team}, not {team}"
        )));
    }
    let sender_ns = crate::envelope::sender_namespace(&envelope_sender(envelope));
    if !id.starts_with(&format!("tm-{sender_ns}-")) {
        return Ok(AcceptAnswer::route_refused(
            "message ID does not belong to its sender",
        ));
    }
    if ctx.relayed_by.is_none() {
        let from_host = envelope["from"]["host"].as_str().unwrap_or_default();
        if !ctx.peer_host.is_empty() && from_host != ctx.peer_host {
            return Ok(AcceptAnswer::route_refused(format!(
                "envelope claims host {from_host}, route peer is {}",
                ctx.peer_host
            )));
        }
    }
    if let Some(hub) = ctx.relayed_by {
        if !ctx.declaration.accepts_relay_from.iter().any(|h| h == hub) {
            return Ok(AcceptAnswer::route_refused(format!(
                "relay from {hub} not accepted by team {team}"
            )));
        }
    }

    let dir = ledger_dir(ctx.state, team);
    let lock = lock_ledger(&dir)?;
    // Existing entry: same body returns its state; different body conflicts.
    if let Some(existing) = read_entry(ctx.state, team, &id) {
        let same_body = existing.get("body_sha256") == envelope.get("body_sha256");
        let recorded_state = entry_state(&existing).to_string();
        unlock(lock);
        if !same_body {
            return Ok(AcceptAnswer {
                outcome: "conflict".to_string(),
                state: Some(recorded_state),
                detail: "same ID with a different body".to_string(),
                findings: Vec::new(),
            });
        }
        // A crashed claim is recovered here (and in drain), never in receipt.
        // While the claimer is still alive this is a concurrent hand-over,
        // not a crash: report `claimed` and let the owner finish it.
        if recorded_state == "claimed" {
            let claimer_alive = existing
                .get("claimer_pid")
                .and_then(Value::as_u64)
                .is_some_and(|pid| {
                    pid != u64::from(std::process::id())
                        && crate::proof::identity_of(pid).is_some_and(|live| {
                            existing
                                .get("claimer_ticks")
                                .and_then(Value::as_u64)
                                .is_none_or(|ticks| ticks == live.starttime_ticks)
                        })
                });
            if claimer_alive {
                return Ok(AcceptAnswer {
                    outcome: "known".to_string(),
                    state: Some("claimed".to_string()),
                    detail: "hand-over in progress".to_string(),
                    findings: Vec::new(),
                });
            }
            let not_after_ms = existing
                .get("not_after")
                .and_then(Value::as_str)
                .and_then(parse_rfc3339_ms)
                .unwrap_or(0);
            if not_after_ms > 0 && not_after_ms <= now_unix_ms() {
                let lock2 = lock_ledger(&ledger_dir(ctx.state, team))?;
                if let Some(mut entry) = read_entry(ctx.state, team, &id) {
                    entry["state"] = json!("expired");
                    entry["detail"] = json!("expired before recovery");
                    write_json_atomic(&entry_path(ctx.state, team, &id), &entry)?;
                }
                unlock(lock2);
                return Ok(AcceptAnswer {
                    outcome: "refused".to_string(),
                    state: Some("expired".to_string()),
                    detail: "expired before recovery".to_string(),
                    findings: Vec::new(),
                });
            }
            let recovered = recover_claim(ctx, envelope, deliverer, &existing)?;
            return Ok(recovered);
        }
        // A retry of an accepted-but-maybe-undrained item nudges delivery:
        // the first hand-over's drain may have died with its serve process
        // or never run (live-trial step 3/5 finding).
        if recorded_state == "accepted" {
            deliverer.nudge(&id);
        }
        return Ok(AcceptAnswer {
            outcome: if recorded_state == "refused" || recorded_state == "fenced" {
                "refused".to_string()
            } else {
                "known".to_string()
            },
            state: Some(recorded_state),
            detail: "already recorded".to_string(),
            findings: Vec::new(),
        });
    }

    // Policy and freshness refusals of the message are durable (section 9.4).
    let sender = envelope_sender(envelope);
    let kind = envelope["kind"].as_str().unwrap_or("info");
    let body = envelope["body"].as_str().unwrap_or_default();
    let now_ms = now_unix_ms();
    let not_after_ms = envelope["not_after"]
        .as_str()
        .and_then(parse_rfc3339_ms)
        .unwrap_or(0);
    let refuse = |state_name: &str, detail: String, lock: fs::File| -> DataResult<AcceptAnswer> {
        write_terminal(ctx.state, team, &id, state_name, envelope, &detail)?;
        unlock(lock);
        Ok(AcceptAnswer {
            outcome: "refused".to_string(),
            state: Some(state_name.to_string()),
            detail,
            findings: Vec::new(),
        })
    };
    if !ctx.declaration.accepts(&sender, kind) {
        // Same-host default: a sender with no explicit rule is accepted
        // when it is another team's designated coordinator on THIS host,
        // proved by harness ancestry, at its own registered address.
        // Trust model: same user, same host - such a coordinator could
        // already type into this pane by hand; the ledger still claims
        // the ID and every other policy (size, ttl, rate) still applies.
        // An explicit rule for the sender always wins, including one
        // that lists other kinds only.
        let explicit = ctx
            .declaration
            .accepts_from
            .iter()
            .any(|rule| rule.sender == sender);
        let same_host_coordinator = !explicit
            && envelope["from"]["proof"].as_str() == Some("harness-ancestry")
            && envelope["from"]["host"].as_str() == Some(ctx.this_host)
            && crate::hostindex::live_coordinator_for_team(&sender).is_some();
        if !same_host_coordinator {
            return refuse(
                "refused",
                format!("sender {sender} may not send kind {kind} to team {team}"),
                lock,
            );
        }
    }
    if body.len() > ctx.declaration.max_bytes {
        return refuse(
            "refused",
            format!(
                "body of {} bytes over the limit {}",
                body.len(),
                ctx.declaration.max_bytes
            ),
            lock,
        );
    }
    if not_after_ms <= now_ms {
        return refuse("expired", "not_after has passed".to_string(), lock);
    }
    let created_ms = envelope["created_at"]
        .as_str()
        .and_then(parse_rfc3339_ms)
        .unwrap_or(now_ms);
    if not_after_ms - created_ms > ctx.declaration.max_ttl_seconds * 1000 {
        return refuse(
            "refused",
            "not_after beyond max_ttl_seconds".to_string(),
            lock,
        );
    }
    // Rate limit: arrivals from one sender in the trailing hour, counted
    // on durable recorded_at (refusals count too, so a flood cannot wash
    // its own record away; the window slides).
    if ctx.declaration.per_sender_per_hour > 0 {
        let window_start = now_ms - 3_600_000;
        let recent = list_entries(ctx.state, team)
            .iter()
            .filter(|entry| {
                entry.get("sender").and_then(Value::as_str) == Some(sender.as_str())
                    && entry
                        .get("recorded_at")
                        .and_then(Value::as_str)
                        .and_then(parse_rfc3339_ms)
                        .is_some_and(|at| at >= window_start)
            })
            .count();
        if recent as u64 >= ctx.declaration.per_sender_per_hour {
            return refuse(
                "refused",
                format!(
                    "rate limit: {recent} arrivals from {sender} in the last hour (limit {})",
                    ctx.declaration.per_sender_per_hour
                ),
                lock,
            );
        }
    }
    // Topic rule: an older message than the newest taken for delivery on its
    // topic starts superseded and is never queued (section 12.1).
    if let Some(topic) = envelope["topic"].as_str() {
        if let Some(newer) = newest_on_topic(ctx.state, team, &sender, topic) {
            let order = (created_ms, envelope["from"]["seq"].as_i64().unwrap_or(0));
            if order < newer {
                return refuse(
                    "superseded",
                    format!("a newer message on topic {topic} was already taken for delivery"),
                    lock,
                );
            }
        }
    }

    // Claim: exclusive create naming the hand-over target, before hand-over.
    let target = deliverer.target();
    let claim = json!({
        "id": id,
        "state": "claimed",
        "body_sha256": envelope.get("body_sha256"),
        "sender": sender,
        "key": envelope.get("key"),
        "topic": envelope.get("topic"),
        "seq": envelope["from"]["seq"],
        "created_at": envelope.get("created_at"),
        "not_after": envelope.get("not_after"),
        "via": ctx.via,
        "claimer_pid": std::process::id(),
        "claimer_ticks": crate::proof::identity_of(u64::from(std::process::id())).map(|i| i.starttime_ticks),
        "claimer_boot_id": crate::timeutil::boot_id(),
        "target": {
            "kind": target.kind,
            "queue_dir": target.queue_dir.map(|p| p.to_string_lossy().into_owned()),
            "inbox_dir": target.inbox_dir.map(|p| p.to_string_lossy().into_owned()),
        },
        "recorded_at": format_rfc3339_ms(now_ms),
        "envelope": envelope,
    });
    let claimed = write_new_atomic(
        &entry_path(ctx.state, team, &id),
        format!("{}\n", serde_json::to_string_pretty(&claim)?).as_bytes(),
    )?;
    unlock(lock);
    if !claimed {
        // Lost the race to another arrival; report what it recorded.
        if let Some(existing) = read_entry(ctx.state, team, &id) {
            return Ok(AcceptAnswer {
                outcome: "known".to_string(),
                state: Some(entry_state(&existing).to_string()),
                detail: "claimed concurrently".to_string(),
                findings: Vec::new(),
            });
        }
        return Ok(AcceptAnswer::route_refused("claim race without an entry"));
    }

    // Supersede older untyped messages on the topic and explicit keys.
    apply_supersede(ctx, envelope, deliverer)?;

    let rendered = render(envelope, ctx.via, ctx.proof_note);
    let outcome = deliverer.handover(&id, envelope, &rendered);
    finish_handover(ctx, &id, outcome)
}

/// Record the result of a hand-over in the ledger entry.
fn finish_handover(
    ctx: &AcceptContext<'_>,
    id: &str,
    outcome: Handover,
) -> DataResult<AcceptAnswer> {
    let team = &ctx.declaration.team;
    let path = entry_path(ctx.state, team, id);
    let lock = lock_ledger(&ledger_dir(ctx.state, team))?;
    let Some(mut entry) = read_entry(ctx.state, team, id) else {
        unlock(lock);
        return Ok(AcceptAnswer::route_refused("ledger entry vanished"));
    };
    // A retract that landed during the hand-over wins.
    if entry_state(&entry) == "retracted" {
        unlock(lock);
        return Ok(AcceptAnswer {
            outcome: "known".to_string(),
            state: Some("retracted".to_string()),
            detail: "retracted during hand-over".to_string(),
            findings: Vec::new(),
        });
    }
    let (new_state, answer_outcome, detail) = match &outcome {
        Handover::Accepted => (
            "accepted",
            "accepted",
            "handed to the deliverer".to_string(),
        ),
        Handover::Refused(reason) => ("refused", "refused", reason.clone()),
        Handover::PossiblySubmitted(reason) => ("possibly_submitted", "known", reason.clone()),
    };
    entry["state"] = json!(new_state);
    entry["detail"] = json!(detail);
    entry["recorded_at"] = json!(format_rfc3339_ms(now_unix_ms()));
    write_json_atomic(&path, &entry)?;
    unlock(lock);
    Ok(AcceptAnswer {
        outcome: answer_outcome.to_string(),
        state: Some(new_state.to_string()),
        detail,
        findings: Vec::new(),
    })
}

/// Recover a `claimed` entry whose claimer died (section 9.2): re-run the
/// hand-over only when the deliverer proves the ID absent; non-idempotent
/// deliverers settle as `possibly_submitted`.
fn recover_claim(
    ctx: &AcceptContext<'_>,
    envelope: &Value,
    deliverer: &dyn Deliverer,
    existing: &Value,
) -> DataResult<AcceptAnswer> {
    let id = envelope["id"].as_str().unwrap_or_default();
    match deliverer.contains(id) {
        Some(true) => {
            let mut entry = existing.clone();
            entry["state"] = json!("accepted");
            entry["detail"] = json!("recovered: found with the deliverer");
            write_json_atomic(&entry_path(ctx.state, &ctx.declaration.team, id), &entry)?;
            Ok(AcceptAnswer {
                outcome: "accepted".to_string(),
                state: Some("accepted".to_string()),
                detail: "recovered after claim".to_string(),
                findings: Vec::new(),
            })
        }
        Some(false) => {
            let rendered = render(envelope, ctx.via, ctx.proof_note);
            let outcome = deliverer.handover(id, envelope, &rendered);
            finish_handover(ctx, id, outcome)
        }
        None => {
            let mut entry = existing.clone();
            entry["state"] = json!("possibly_submitted");
            entry["detail"] = json!("recovery: deliverer cannot prove absence");
            write_json_atomic(&entry_path(ctx.state, &ctx.declaration.team, id), &entry)?;
            Ok(AcceptAnswer {
                outcome: "known".to_string(),
                state: Some("possibly_submitted".to_string()),
                detail: "claimed but hand-over unprovable; a human resolves it".to_string(),
                findings: Vec::new(),
            })
        }
    }
}

/// The newest `(created_ms, seq)` taken for delivery per sender and topic.
fn newest_on_topic(state: &Path, team: &str, sender: &str, topic: &str) -> Option<(i64, i64)> {
    let dir = ledger_dir(state, team);
    let entries = fs::read_dir(dir).ok()?;
    let mut newest: Option<(i64, i64)> = None;
    for entry in entries.flatten() {
        if !is_entry_file_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let Some(text) = fs::read_to_string(entry.path()).ok() else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue; // the lock file and any partial write are not entries
        };
        let taken = matches!(
            entry_state(&value),
            "claimed" | "accepted" | "typing" | "presented" | "delivered"
        );
        if !taken
            || value.get("sender").and_then(Value::as_str) != Some(sender)
            || value.get("topic").and_then(Value::as_str) != Some(topic)
        {
            continue;
        }
        let created = value
            .get("created_at")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_ms)
            .unwrap_or(0);
        let seq = value.get("seq").and_then(Value::as_i64).unwrap_or(0);
        newest = Some(newest.map_or((created, seq), |n| n.max((created, seq))));
    }
    newest
}

/// Write tombstones for older untyped messages this one supersedes, and mark
/// their ledger entries `superseded` (sections 12.1, 12.2).
fn apply_supersede(
    ctx: &AcceptContext<'_>,
    envelope: &Value,
    deliverer: &dyn Deliverer,
) -> DataResult<()> {
    let team = &ctx.declaration.team;
    let sender = envelope_sender(envelope);
    let topic = envelope["topic"].as_str();
    let supersedes: Vec<String> = envelope["supersedes"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let my_created = envelope["created_at"]
        .as_str()
        .and_then(parse_rfc3339_ms)
        .unwrap_or(0);
    let my_id = envelope["id"].as_str().unwrap_or_default();
    let dir = ledger_dir(ctx.state, team);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        if !is_entry_file_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(mut other) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let other_id = other["id"].as_str().unwrap_or_default().to_string();
        if other_id == my_id || entry_state(&other) != "accepted" {
            continue;
        }
        let same_topic = topic.is_some()
            && other.get("topic").and_then(Value::as_str) == topic
            && other.get("sender").and_then(Value::as_str) == Some(sender.as_str())
            && other
                .get("created_at")
                .and_then(Value::as_str)
                .and_then(parse_rfc3339_ms)
                .unwrap_or(0)
                < my_created;
        let named = other.get("sender").and_then(Value::as_str) == Some(sender.as_str())
            && other
                .get("key")
                .and_then(Value::as_str)
                .is_some_and(|key| supersedes.iter().any(|k| k == key));
        if same_topic || named {
            other["state"] = json!("superseded");
            other["detail"] = json!(format!("superseded by {my_id}"));
            write_json_atomic(&entry.path(), &other)?;
            write_tombstone(deliverer, &other_id, "superseded");
        }
    }
    Ok(())
}

/// A lock-free tombstone beside the deliverer's store (section 12.2).
///
/// A queue's tombstone is written by the queue library itself, so its lane is private (a
/// directory made under the usual 022 umask stops every reader of the queue) and an item still
/// waiting is set aside at once when no drain holds the queue. A hook inbox's tombstone is a
/// private file in a private directory.
pub fn write_tombstone(deliverer: &dyn Deliverer, id: &str, reason: &str) {
    let target = deliverer.target();
    match (&target.queue_dir, &target.inbox_dir) {
        (Some(queue), _) => {
            let _ = agent_queue::queue::cancel(queue, id, reason);
        }
        (None, Some(inbox)) => {
            let cancel_dir = inbox.join("cancel");
            if create_private_dir_all(&cancel_dir).is_ok() {
                let body =
                    json!({"id": id, "reason": reason, "at": format_rfc3339_ms(now_unix_ms())});
                let _ = write_new(
                    &cancel_dir.join(format!("{id}.json")),
                    body.to_string().as_bytes(),
                );
            }
        }
        _ => {}
    }
}

/// `fence`: bar an ID before it arrives, or return its recorded state.
pub fn fence(
    state: &Path,
    team: &str,
    id: &str,
    not_after: &str,
    body_sha256: Option<&str>,
) -> DataResult<Value> {
    if !crate::model::valid_team_name(team) || !crate::envelope::valid_message_id(id) {
        return Ok(json!({"outcome": "error", "detail": "bad team or message ID"}));
    }
    let dir = ledger_dir(state, team);
    let lock = lock_ledger(&dir)?;
    if let Some(existing) = read_entry(state, team, id) {
        let recorded = entry_state(&existing).to_string();
        unlock(lock);
        return Ok(json!({"outcome": "known", "state": recorded}));
    }
    let entry = json!({
        "id": id,
        "state": "fenced",
        "body_sha256": body_sha256,
        "not_after": not_after,
        "recorded_at": format_rfc3339_ms(now_unix_ms()),
        "detail": "fenced by the sender before trying another contact",
    });
    let written = write_new_atomic(
        &entry_path(state, team, id),
        format!("{}\n", serde_json::to_string_pretty(&entry)?).as_bytes(),
    )?;
    unlock(lock);
    if written {
        Ok(json!({"outcome": "fenced", "state": "fenced"}))
    } else if let Some(existing) = read_entry(state, team, id) {
        Ok(json!({"outcome": "known", "state": entry_state(&existing)}))
    } else {
        Ok(json!({"outcome": "fenced", "state": "fenced"}))
    }
}

/// Fold the agentctl queue's terminal state for an accepted entry back
/// into the ledger: `processed` becomes `delivered`, `cancelled` becomes
/// the set-aside reason's terminal state, and `failed` becomes
/// `possibly_submitted`. This is how a sender's receipt ever leaves
/// `accepted` (gap 2): `accept` ends at hand-over, but typing finishes
/// later and usually in a different process - our `drain-one`, the
/// coordinator's bridge, or any `agentctl send` that drains the queue on
/// its way. Returns the new state when it changed one. Only entries
/// still `accepted` with an `agentctl-queue` target are touched.
pub fn reconcile_queue_delivery(state: &Path, team: &str, id: &str) -> Option<String> {
    use agent_queue::queue::MessageState;
    let lock = lock_ledger(&ledger_dir(state, team)).ok()?;
    let mut entry = read_entry(state, team, id)?;
    if entry_state(&entry) != "accepted" {
        unlock(lock);
        return None;
    }
    let target = entry.get("target")?;
    if target.get("kind").and_then(Value::as_str) != Some("agentctl-queue") {
        unlock(lock);
        return None;
    }
    // A queue recorded before its registry moved to layout 2 is found where it is now (#593
    // registry-agents-subdir).
    let queue_dir = agent_queue::registry::relocated(&PathBuf::from(
        target.get("queue_dir").and_then(Value::as_str)?,
    ));
    let queue_state = agent_queue::queue::message_state(&queue_dir, id)
        .ok()
        .flatten();
    let (new_state, detail): (&str, String) = match queue_state {
        Some(MessageState::Processed) => (
            "delivered",
            "typed into the coordinator's pane (queue item processed)".to_string(),
        ),
        Some(MessageState::Cancelled) => {
            let item = fs::read_to_string(queue_dir.join("cancelled").join(format!("{id}.json")))
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok());
            let reason = item
                .as_ref()
                .and_then(|item| item.get("cancel_reason"))
                .and_then(Value::as_str)
                .unwrap_or("cancelled");
            if reason == "expired" {
                (
                    "expired",
                    "set aside untyped at drain: past not_after".to_string(),
                )
            } else {
                (
                    "retracted",
                    "set aside untyped at drain: tombstoned".to_string(),
                )
            }
        }
        Some(MessageState::Failed) => (
            "possibly_submitted",
            "queue quarantined the item as possibly submitted; settle with resolve".to_string(),
        ),
        _ => {
            unlock(lock);
            return None;
        }
    };
    entry["state"] = json!(new_state);
    entry["detail"] = json!(detail);
    entry["delivered_at"] = if new_state == "delivered" {
        json!(format_rfc3339_ms(now_unix_ms()))
    } else {
        Value::Null
    };
    let written = write_json_atomic(&entry_path(state, team, id), &entry);
    unlock(lock);
    written.ok()?;
    Some(new_state.to_string())
}

/// Sender/ledger states after which an entry can never move again and
/// may be pruned at its retention horizon. `possibly_submitted` and
/// `claimed`/`accepted` are never terminal here: the former needs a
/// human, the latter may still be typed.
const PRUNABLE_STATES: [&str; 7] = [
    "delivered",
    "expired",
    "superseded",
    "retracted",
    "fenced",
    "refused",
    "lost",
];

/// The retention horizon of one entry in Unix ms: 30 days past its own
/// stored `not_after` (design 9.2). Entries without a parseable
/// `not_after` have no horizon and are never pruned.
fn retention_horizon_ms(entry: &Value) -> Option<i64> {
    let not_after = entry.get("not_after").and_then(Value::as_str).or_else(|| {
        entry
            .get("envelope")
            .and_then(|envelope| envelope.get("not_after"))
            .and_then(Value::as_str)
    })?;
    parse_rfc3339_ms(not_after).map(|ms| ms + 30 * 86_400_000)
}

/// Delete one entry file and any atomic-write backups (`<id>.json.<stamp>`)
/// beside it.
fn remove_entry_files(dir: &Path, id: &str) {
    let base = format!("{id}.json");
    let _ = fs::remove_file(dir.join(&base));
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&format!("{base}.")) && name.len() > base.len() + 1 {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// Prune one team's ledger: terminal entries whose retention horizon has
/// passed. Returns how many entries were removed.
pub fn prune_team(state: &Path, team: &str, now_ms: i64) -> DataResult<usize> {
    let dir = ledger_dir(state, team);
    let lock = lock_ledger(&dir)?;
    let mut removed = 0;
    for entry in list_entries(state, team) {
        let Some(id) = entry.get("id").and_then(Value::as_str) else {
            continue;
        };
        if !PRUNABLE_STATES.contains(&entry_state(&entry)) {
            continue;
        }
        if retention_horizon_ms(&entry).is_some_and(|horizon| horizon <= now_ms) {
            remove_entry_files(&dir, id);
            removed += 1;
        }
    }
    unlock(lock);
    Ok(removed)
}

/// Prune every team's ledger under `state`. Returns `(team, removed)`
/// pairs for teams that lost entries.
pub fn prune_all(state: &Path, now_ms: i64) -> DataResult<Vec<(String, usize)>> {
    let mut out = Vec::new();
    let teams_dir = state.join("teams.d");
    if let Ok(entries) = fs::read_dir(&teams_dir) {
        for entry in entries.flatten() {
            let team = entry.file_name().to_string_lossy().into_owned();
            if crate::model::valid_team_name(&team) {
                let removed = prune_team(state, &team, now_ms)?;
                if removed > 0 {
                    out.push((team, removed));
                }
            }
        }
    }
    Ok(out)
}

/// Prune the sender-side outbox: terminal entries (per
/// [`crate::sender::is_terminal`]) past the same retention horizon.
/// Outbox entries carry their envelope's `not_after` (see
/// [`retention_horizon_ms`]); entries without one are kept.
pub fn prune_outbox(state: &Path, now_ms: i64) -> DataResult<usize> {
    let dir = state.join("outbox");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(0);
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        if !crate::envelope::valid_message_id(id) {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(document) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let state_name = document
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !crate::sender::is_terminal(state_name) {
            continue;
        }
        if retention_horizon_ms(&document).is_some_and(|horizon| horizon <= now_ms) {
            remove_entry_files(&dir, id);
            removed += 1;
        }
    }
    Ok(removed)
}

/// `receipt`: a pure read of the ledger (never recovers).
pub fn receipt(state: &Path, team: &str, id: &str) -> Value {
    match read_entry(state, team, id) {
        Some(entry) => json!({
            "known": true,
            "id": id,
            "state": entry_state(&entry),
            "detail": entry.get("detail"),
            "sender": entry.get("sender"),
            "recorded_at": entry.get("recorded_at"),
        }),
        None => json!({"known": false, "id": id, "state": "unknown"}),
    }
}

/// Retract a message that has not been typed yet (section 12.1).
pub fn retract(state: &Path, team: &str, id: &str, deliverer: &dyn Deliverer) -> DataResult<Value> {
    let lock = lock_ledger(&ledger_dir(state, team))?;
    let Some(mut entry) = read_entry(state, team, id) else {
        unlock(lock);
        return Ok(json!({"outcome": "unknown", "id": id}));
    };
    match entry_state(&entry) {
        "accepted" | "claimed" => {
            entry["state"] = json!("retracted");
            entry["detail"] = json!("retracted by the sender");
            write_json_atomic(&entry_path(state, team, id), &entry)?;
            unlock(lock);
            write_tombstone(deliverer, id, "retracted");
            Ok(json!({"outcome": "retracted", "state": "retracted"}))
        }
        other => {
            unlock(lock);
            Ok(json!({"outcome": "too-late", "state": other}))
        }
    }
}

/// List ledger entries in a state (for `status` and drain recovery).
pub fn list_entries(state: &Path, team: &str) -> Vec<Value> {
    let dir = ledger_dir(state, team);
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Value> = entries
        .flatten()
        .filter(|entry| is_entry_file_name(&entry.file_name().to_string_lossy()))
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .filter_map(|text| serde_json::from_str(&text).ok())
        .collect();
    out.sort_by_key(|entry| entry["id"].as_str().unwrap_or_default().to_string());
    out
}

/// Body hash of an envelope file body, for fence calls from the outbox.
pub fn body_hash(body: &str) -> String {
    sha256_hex(body.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deliver::{HandoverTarget, HookInboxDeliverer};
    use std::os::unix::fs::PermissionsExt;

    struct QueueTarget(PathBuf);

    impl Deliverer for QueueTarget {
        fn kind(&self) -> &str {
            "agentctl-queue"
        }
        fn target(&self) -> HandoverTarget {
            HandoverTarget {
                kind: self.kind().to_string(),
                queue_dir: Some(self.0.clone()),
                inbox_dir: None,
            }
        }
        fn handover(&self, _id: &str, _envelope: &Value, _rendered: &str) -> Handover {
            Handover::Refused("not used".to_string())
        }
        fn contains(&self, _id: &str) -> Option<bool> {
            None
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agent-teams-ledger-{name}-{}-{}",
            std::process::id(),
            now_unix_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// #557 agent-teams-cancel-perms: under the usual 022 umask a tombstone once made the
    /// queue's `cancel/` 0755 with a 0644 file, and every reader of the queue then refused it.
    #[test]
    fn tombstones_and_hook_inbox_items_are_private_under_umask_022() {
        // SAFETY: umask has no preconditions; 022 is the usual default, so other tests are
        // unaffected.
        unsafe { libc::umask(0o022) };
        let dir = scratch("tombstone");

        let queue = dir.join("registry/coordinator/queue");
        agent_queue::queue::prepare(&queue).unwrap();
        write_tombstone(&QueueTarget(queue.clone()), "tm-a-1", "retracted");
        let cancel = queue.join("cancel");
        assert_eq!(mode(&cancel), 0o700);
        assert_eq!(mode(&cancel.join("tm-a-1.json")), 0o600);
        // The queue's own readers accept the result.
        assert_eq!(
            agent_queue::queue::message_state(&queue, "tm-a-1").unwrap(),
            None
        );

        let inbox = dir.join("hook-inbox");
        let hook = HookInboxDeliverer {
            inbox_dir: inbox.clone(),
            ledger_path: dir.join("ledger/tm-b-1.json"),
        };
        assert!(matches!(
            hook.handover("tm-b-1", &json!({"id": "tm-b-1"}), "hello"),
            Handover::Accepted
        ));
        write_tombstone(&hook, "tm-b-1", "retracted");
        assert_eq!(mode(&inbox), 0o700);
        assert_eq!(mode(&inbox.join("tm-b-1.json")), 0o600);
        assert_eq!(mode(&inbox.join("cancel")), 0o700);
        assert_eq!(mode(&inbox.join("cancel/tm-b-1.json")), 0o600);

        let _ = fs::remove_dir_all(&dir);
    }
}
