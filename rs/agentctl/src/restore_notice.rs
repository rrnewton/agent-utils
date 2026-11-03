//! After a restore: the deterministic "I'm back" summary and catch-up prompts (#746).
//!
//! The summary is the same fixed text the coordinator's own notice carries, posted to
//! every other registry that names the coordinator (its chat bridges), and each agent
//! the restore revived gets one standard catch-up prompt. Everything is queued with
//! `queue_identified`, never drained: a restore must not sit inside a send while a lane
//! is mid-turn, and a message that matters arrives with the next drain. Agents are
//! addressed in the report's own order, which the restore built managers-first.

use std::path::Path;

use serde_json::{json, Value};

use crate::restore::RestoreConfig;
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
                manager.queue_identified(&other.name, summary, None, Some(&id))
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
                    manager.queue_identified(name, &text, None, Some(&id)),
                ));
            }
        }
    }
    json!({"bridges": bridges, "catch_up": catch_up})
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
