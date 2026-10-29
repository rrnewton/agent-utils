//! Per-record hold loading parity across every machine.
//!
//! Composes the active-shard and hold readers the way the existing
//! record-hold loader does: for each machine, every active record gets
//! its hold record (or null), and a per-slot load failure is captured
//! as that record's error instead of refusing the whole report.
//! A machine whose state is derived from its event log replays holds
//! from the chain, like the Python loader; an invalid event log yields
//! the loader's message as each record's error. Read-only.

use std::path::Path;

use serde_json::{json, Value};

use crate::active_state::{load_cfg, parse_state_ordered, RecordOrder};
use crate::global_state::{discover_machines, event_log_status, load_event_chain, EventLogStatus};
use crate::hold_state::parse_hold;
use crate::hold_write::hold_from_events;

type Result<T> = std::result::Result<T, String>;

/// One record's hold outcome: the hold document (or null) and the
/// captured load error (or none), in machine then shard order.
pub(crate) struct HoldEntry {
    pub(crate) machine: String,
    pub(crate) slot: String,
    pub(crate) hold: Value,
    pub(crate) error: Option<String>,
}

/// Load every active record's hold across all machines.
pub(crate) fn load_record_holds(root: Option<&Path>) -> Result<(Vec<String>, Vec<HoldEntry>)> {
    let (cfg, _selected) = load_cfg(root, None)?;
    let machines = discover_machines(&cfg.control)?;
    let mut orders: Vec<Vec<RecordOrder>> = Vec::new();
    for machine in &machines {
        // Derived machines order their records from the chain fold,
        // matching the states the Python loader is given. A corrupt
        // chain falls back to the snapshot order and is captured as
        // each record's error below, as the Python loader reports it.
        let events_dir = cfg.control.join(format!("EVENTS.{machine}"));
        if matches!(event_log_status(&events_dir), EventLogStatus::Derived) {
            match crate::hold_write::active_document(root, &cfg, machine) {
                Ok(document) => {
                    let order = document
                        .get("slots")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .map(|slot| RecordOrder {
                            slot: slot
                                .get("slot")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            agent: slot
                                .get("agent")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            slot_type: slot
                                .get("slot_type")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            imported: true,
                        })
                        .collect();
                    orders.push(order);
                    continue;
                }
                Err(_message) => {
                    let (_active, order) = parse_state_ordered(root, Some(machine))?;
                    orders.push(order);
                    continue;
                }
            }
        }
        let (_active, order) = parse_state_ordered(root, Some(machine))?;
        orders.push(order);
    }
    let mut holds: Vec<HoldEntry> = Vec::new();
    for (machine, order) in machines.iter().zip(orders.iter()) {
        let events = cfg.control.join(format!("EVENTS.{machine}"));
        // Event-log-derived machines replay their holds from the chain,
        // like the Python loader; only an invalid chain is captured as
        // each record's error.
        let derived_events = match event_log_status(&events) {
            EventLogStatus::Derived => match load_event_chain(&events, machine) {
                Ok(events) => Some(events),
                Err(message) => {
                    for record in order {
                        holds.push(HoldEntry {
                            machine: machine.clone(),
                            slot: record.slot.clone(),
                            hold: Value::Null,
                            error: Some(message.clone()),
                        });
                    }
                    continue;
                }
            },
            EventLogStatus::Invalid(message) => {
                for record in order {
                    holds.push(HoldEntry {
                        machine: machine.clone(),
                        slot: record.slot.clone(),
                        hold: Value::Null,
                        error: Some(message.clone()),
                    });
                }
                continue;
            }
            EventLogStatus::Snapshot => None,
        };
        match derived_events {
            Some(events) => {
                for record in order {
                    holds.push(HoldEntry {
                        machine: machine.clone(),
                        slot: record.slot.clone(),
                        hold: hold_from_events(&events, machine, &record.slot),
                        error: None,
                    });
                }
            }
            None => {
                for record in order.iter() {
                    let (hold, error) = match parse_hold(root, Some(machine), &record.slot) {
                        Ok(value) => (value, None),
                        Err(message) => (Value::Null, Some(message)),
                    };
                    holds.push(HoldEntry {
                        machine: machine.clone(),
                        slot: record.slot.clone(),
                        hold,
                        error,
                    });
                }
            }
        }
    }
    Ok((machines, holds))
}

/// Load every active record's hold across all machines, printing one
/// entry per record in machine then shard order.
pub(crate) fn parse_holds(root: Option<&Path>) -> Result<Value> {
    let (machines, entries) = load_record_holds(root)?;
    let holds: Vec<Value> = entries
        .into_iter()
        .map(|entry| {
            json!({
                "machine": entry.machine,
                "slot": entry.slot,
                "hold": entry.hold,
                "error": entry.error,
            })
        })
        .collect();
    Ok(json!({
        "machines": machines,
        "holds": holds,
    }))
}
