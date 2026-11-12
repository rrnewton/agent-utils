//! After a restore: the deterministic "I'm back" summary and catch-up prompts (#746).
//!
//! The summary is the same fixed text the coordinator's own notice carries, posted to
//! every other registry that names the coordinator (its chat bridges), and each agent
//! the restore revived gets one standard catch-up prompt. Everything is queued with
//! `queue_identified`, never drained: a restore must not sit inside a send while a lane
//! is mid-turn. Each kind carries a supersede key (#1372, on #1156), so a later
//! boot's prompt replaces an earlier boot's still-undelivered one untyped instead
//! of stacking in the inbox; the per-boot message ID still makes a same-boot
//! rerun a no-op.
//! is mid-turn, and a message that matters arrives with the next drain. Agents are
//! addressed in the report's own order, which the restore built managers-first.

use std::path::Path;

use serde_json::{json, Value};

use crate::restore::{resume_brief_template, RestoreConfig};
use crate::subagents::{ManagedAgents, ManagedApi};

/// One queued message's outcome, as the report records it.
fn queued(
    name: &str,
    message_id: &str,
    result: crate::subagents::Result<crate::agent::QueueResult>,
) -> Value {
    match result {
        Ok(result) => json!({
            "name": name,
            "action": "queued",
            "message_id": message_id,
            "outcome": json!(result)["outcome"],
        }),
        Err(error) => json!({
            "name": name,
            "action": "refused",
            "message_id": message_id,
            "reason": error.to_string(),
        }),
    }
}

/// The boot-scoped stable ID of one after-restore message.
fn message_id(kind: &str, boot_id: &str, name: &str) -> String {
    format!("restore-{kind}-{}-{name}", &boot_id[..8.min(boot_id.len())])
}

/// Post the restore summary to each bridge registry and queue each revived agent's
/// catch-up prompt. Returns the per-message outcomes for the report. Never blocks on
/// a pane and never fails the restore: every refusal is recorded, not raised.
pub fn after_restore<A: ManagedApi + ?Sized>(
    client: &A,
    registry: &Path,
    config: &RestoreConfig,
    report: &Value,
    summary: Option<&str>,
    boot_id: &str,
) -> Value {
    let mut bridges = Vec::new();
    if let Some(summary) = summary {
        for other in &config.also_registered {
            let id = message_id("imback", boot_id, &other.name);
            let result = (|| -> crate::subagents::Result<crate::agent::QueueResult> {
                let manager = ManagedAgents::new(client, &other.registry)?;
                manager.queue_identified(
                    &other.name,
                    summary,
                    None,
                    Some("restore-imback"),
                    Some(&id),
                )
            })();
            bridges.push(queued(&other.name, &id, result));
        }
    }
    let mut catch_up = Vec::new();
    match ManagedAgents::new(client, registry) {
        Err(error) => catch_up.push(json!({
            "name": registry.display().to_string(),
            "action": "refused",
            "reason": error.to_string(),
        })),
        Ok(manager) => {
            let rows = report["agents"].as_array().cloned().unwrap_or_default();
            for row in rows.iter().filter(|row| {
                matches!(
                    row["action"].as_str(),
                    Some("revived" | "revived-fresh" | "promoted")
                )
            }) {
                let Some(name) = row["name"].as_str() else {
                    continue;
                };
                let id = message_id("catchup", boot_id, name);
                let text = catch_up_text(name, row, boot_id);
                catch_up.push(queued(
                    name,
                    &id,
                    manager.queue_identified(name, &text, None, Some("restore-catchup"), Some(&id)),
                ));
            }
        }
    }
    json!({"bridges": bridges, "catch_up": catch_up})
}

/// Queue the configured resume brief once per revived or promoted lane
/// (#1131), under a per-boot message ID a same-boot rerun recognises as
/// already queued. Returns the per-lane outcomes for the report, or
/// `Value::Null` when no template is in force. Skipped, excluded, and
/// refused lanes receive nothing: they never came back to brief. The
/// coordinator gets the recovery notice, not a brief.
pub fn resume_briefs<A: ManagedApi + ?Sized>(
    client: &A,
    registry: &Path,
    config: &RestoreConfig,
    report: &Value,
    boot_id: &str,
) -> Value {
    let Some(template) = resume_brief_template(config) else {
        return Value::Null;
    };
    let manager = match ManagedAgents::new(client, registry) {
        Ok(manager) => manager,
        Err(error) => {
            return json!([{
                "name": registry.display().to_string(),
                "action": "refused",
                "reason": error.to_string(),
            }]);
        }
    };
    let rows = report["agents"].as_array().cloned().unwrap_or_default();
    let mut briefs = Vec::new();
    for row in rows.iter().filter(|row| {
        matches!(
            row["action"].as_str(),
            Some("revived" | "revived-fresh" | "promoted")
        )
    }) {
        let Some(name) = row["name"].as_str() else {
            continue;
        };
        let id = message_id("brief", boot_id, name);
        let text = render_resume_brief(template, config, report, row);
        let row_outcome =
            match manager.queue_identified(name, &text, None, Some("restore-brief"), Some(&id)) {
                Ok(result) => json!({
                    "name": name,
                    "action": "queued",
                    "message_id": id,
                    "outcome": json!(result)["outcome"],
                }),
                Err(error) if error.to_string().contains("already exists") => json!({
                    "name": name,
                    "action": "already-queued",
                    "message_id": id,
                    "outcome": "already-queued",
                }),
                Err(error) => json!({
                    "name": name,
                    "action": "refused",
                    "message_id": id,
                    "reason": error.to_string(),
                }),
            };
        briefs.push(row_outcome);
    }
    json!(briefs)
}

/// Render one lane's brief from the facts the report already holds.
fn render_resume_brief(
    template: &str,
    config: &RestoreConfig,
    report: &Value,
    row: &Value,
) -> String {
    let seconds = |value: &Value| match value {
        Value::Number(number) => number.to_string(),
        _ => "unknown".to_owned(),
    };
    let (slot, slot_outcome) = slot_outcome(row);
    let values: [(&str, String); 8] = [
        (
            "boot_id",
            report["boot_id"].as_str().unwrap_or("?").to_owned(),
        ),
        ("boot_time", seconds(&report["boot_time"])),
        ("finished_at", seconds(&report["finished_at"])),
        ("agent", row["name"].as_str().unwrap_or("?").to_owned()),
        (
            "coordinator",
            config
                .coordinator
                .clone()
                .unwrap_or_else(|| "none".to_owned()),
        ),
        ("slot", slot),
        ("slot_outcome", slot_outcome),
        (
            "report",
            report["report"].as_str().unwrap_or("?").to_owned(),
        ),
    ];
    let mut rendered = template.to_owned();
    for (name, value) in &values {
        rendered = rendered.replace(&format!("{{{{{name}}}}}"), value);
        rendered = rendered.replace(&format!("{{{{ {name} }}}}"), value);
    }
    rendered
}

/// The `slot` and `slot_outcome` facts of one lane's row: a refused
/// transfer names the refusal and its remedy, never "rebind is done".
fn slot_outcome(row: &Value) -> (String, String) {
    let owner = &row["slot_owner"];
    let slot = owner["slot"].as_str().unwrap_or("none").to_owned();
    let outcome = match owner["action"].as_str() {
        Some("transferred") => {
            "slot rebind done (the slot owner is the revived harness)".to_owned()
        }
        Some("refused") => format!(
            "slot rebind REFUSED: {}; run {}",
            owner["reason"].as_str().unwrap_or("no reason recorded"),
            owner["remedy"].as_str().unwrap_or("agentctl rebind-slot")
        ),
        Some("skipped") => format!(
            "slot rebind not done: {}; run {}",
            owner["reason"].as_str().unwrap_or("not transferred"),
            owner["remedy"].as_str().unwrap_or("agentctl rebind-slot")
        ),
        _ if slot != "none" => "slot rebind outcome not recorded".to_owned(),
        _ => "no slot".to_owned(),
    };
    (slot, outcome)
}

/// The one standard catch-up prompt. Fixed wording, so a lane can recognise it.
fn catch_up_text(name: &str, row: &Value, boot_id: &str) -> String {
    let fresh = row["action"].as_str() == Some("revived-fresh");
    format!(
        "[agentctl restore] {name}: you were restored after the host restarted (boot {boot_id}). \
         Read your queued messages first, then check the state of your work and continue where \
         you left off.{}",
        if fresh {
            " Your conversation started fresh with no memory of the previous one: your launch brief holds your task."
        } else {
            ""
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catch_up_text_is_fixed_and_names_the_fresh_start() {
        let revived = json!({"name": "lane", "action": "revived"});
        assert!(catch_up_text("lane", &revived, "boot-1").contains("continue where"));
        assert!(!catch_up_text("lane", &revived, "boot-1").contains("started fresh"));
        let fresh = json!({"name": "lane", "action": "revived-fresh"});
        assert!(catch_up_text("lane", &fresh, "boot-1").contains("started fresh"));
        assert_eq!(
            message_id("catchup", "boot-123456789", "lane"),
            "restore-catchup-boot-123-lane"
        );
    }
}
