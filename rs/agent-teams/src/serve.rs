//! The receiving host's protocol endpoint: `serve` verbs and `probe`
//! (design sections 7, 8.3 and 10.2). Verification happens here, on the
//! receiving host, at delivery time.

use crate::deliver::deliverer_for;
use crate::jsonfile::DataResult;
use crate::ledger::{accept, fence, receipt, AcceptContext};
use crate::proof::{coordinator_identity, identity_alive, load_declarations};
use serde_json::{json, Value};
use std::path::Path;

/// Handle one serve verb. `peer` is the authenticated (or asserted) origin host.
pub fn handle_verb(
    state: &Path,
    this_host: &str,
    peer: &str,
    verb: &str,
    payload: &Value,
) -> Value {
    match verb {
        "ping" => json!({
            "outcome": "pong",
            "host": this_host,
            "boot_id": crate::timeutil::boot_id(),
            "protocol": 1,
        }),
        "probe" => probe(state, this_host),
        "receipt" => {
            let team = payload["team"].as_str().unwrap_or_default();
            let id = payload["id"].as_str().unwrap_or_default();
            if !crate::model::valid_team_name(team) || !crate::envelope::valid_message_id(id) {
                return json!({"known": false, "id": id, "state": "unknown"});
            }
            // A receipt answer should reflect typing that finished after
            // accept, whichever drainer did it; the read itself stays pure.
            let _ = crate::ledger::reconcile_queue_delivery(state, team, id);
            receipt(state, team, id)
        }
        "fence" => {
            let team = payload["team"].as_str().unwrap_or_default();
            let id = payload["id"].as_str().unwrap_or_default();
            let not_after = payload["not_after"].as_str().unwrap_or_default();
            match fence(state, team, id, not_after, payload["body_sha256"].as_str()) {
                Ok(value) => value,
                Err(e) => json!({"outcome": "error", "detail": e.to_string()}),
            }
        }
        "accept" => match handle_accept(state, this_host, peer, payload) {
            Ok(answer) => answer,
            Err(e) => json!({"outcome": "error", "detail": e.to_string()}),
        },
        other => json!({"outcome": "error", "detail": format!("unknown verb {other:?}")}),
    }
}

fn handle_accept(state: &Path, this_host: &str, peer: &str, envelope: &Value) -> DataResult<Value> {
    let to_team = envelope["to"]["team"].as_str().unwrap_or_default();
    let declarations = load_declarations(state)?;
    let Some((_, _, declaration)) = declarations.iter().find(|(team, _, _)| team == to_team) else {
        return Ok(json!({
            "outcome": "route-refused",
            "state": Value::Null,
            "detail": format!("no declaration for team {to_team} on this host"),
        }));
    };
    let id = envelope["id"].as_str().unwrap_or_default();
    let ledger_path = crate::ledger::entry_path(state, to_team, id);
    let team_state_dir = state.join("teams.d").join(to_team);
    let deliverer = deliverer_for(declaration, &team_state_dir, &ledger_path)?;
    let proof = envelope["from"]["proof"].as_str().unwrap_or("asserted");
    let proof_note = format!("sender proved on {peer} ({proof})");
    let relayed_by = envelope
        .get("relay_trace")
        .and_then(Value::as_array)
        .and_then(|trace| trace.first())
        .and_then(|hop| hop.get("relay"))
        .and_then(Value::as_str);
    let ctx = AcceptContext {
        state,
        this_host,
        declaration,
        via: &format!("serve:{peer}"),
        proof_note: &proof_note,
        relayed_by,
        peer_host: peer,
    };
    let answer = accept(&ctx, envelope, deliverer.as_ref())?;
    Ok(answer.to_json())
}

/// `probe --json`: what this host's own tools say (design section 7).
pub fn probe(state: &Path, this_host: &str) -> Value {
    let mut teams = Vec::new();
    let mut findings = Vec::new();
    match load_declarations(state) {
        Ok(declarations) => {
            for (team, path, declaration) in &declarations {
                let registry = declaration
                    .deliverer
                    .get("registry")
                    .and_then(Value::as_str)
                    .map(crate::jsonfile::expand_home);
                let coordinator =
                    coordinator_identity(&declaration.designation, registry.as_deref());
                let live = coordinator.as_ref().is_some_and(identity_alive);
                if !live {
                    findings.push(json!({
                        "kind": "coordinator-not-live",
                        "team": team,
                        "detail": "the designated coordinator process is not live with its recorded identity",
                    }));
                }
                teams.push(json!({
                    "team": team,
                    "declaration": path.to_string_lossy(),
                    "deliverer": declaration.deliverer_kind,
                    "coordinator_live": live,
                    "coordinator": coordinator.map(|c| json!({
                        "boot_id": c.boot_id, "pid": c.pid, "starttime_ticks": c.starttime_ticks,
                    })),
                }));
            }
        }
        Err(e) => findings.push(json!({"kind": "declaration-error", "detail": e.to_string()})),
    }
    json!({
        "outcome": "probe",
        "host": this_host,
        "boot_id": crate::timeutil::boot_id(),
        "protocol": 1,
        "teams": teams,
        "findings": findings,
        "echo": "agent-teams-probe",
    })
}
