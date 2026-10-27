//! Per-record hold loading parity across every machine.
//!
//! Composes the active-shard and hold readers the way the existing
//! record-hold loader does: for each machine, every active record gets
//! its hold record (or null), and a per-slot load failure is captured
//! as that record's error instead of refusing the whole report.
//! Snapshot mode only: a machine with a complete event log derives its
//! holds, and this command refuses that case toward `rebuild`; an
//! invalid event log yields the loader's message as each record's
//! error, as the existing loader reports it. Read-only.

use std::path::Path;

use serde_json::{json, Value};

use crate::active_state::{load_cfg, parse_state_ordered, RecordOrder};
use crate::global_state::{
    discover_machines, event_log_status, validate_event_chain, EventLogStatus,
};
use crate::hold_state::parse_hold;

type Result<T> = std::result::Result<T, String>;

/// Load every active record's hold across all machines, printing one
/// entry per record in machine then shard order.
pub(crate) fn parse_holds(root: Option<&Path>) -> Result<Value> {
    let (cfg, _selected) = load_cfg(root, None)?;
    let machines = discover_machines(&cfg.control)?;
    let mut orders: Vec<Vec<RecordOrder>> = Vec::new();
    for machine in &machines {
        let (_active, order) = parse_state_ordered(root, Some(machine))?;
        orders.push(order);
    }
    let mut holds: Vec<Value> = Vec::new();
    for (machine, order) in machines.iter().zip(orders.iter()) {
        let events = cfg.control.join(format!("EVENTS.{machine}"));
        let captured = match event_log_status(&events) {
            EventLogStatus::Derived => match validate_event_chain(&events, machine) {
                Ok(()) => {
                    return Err(format!(
                        "machine state for {machine} is derived from the append-only event log; use rebuild instead of snapshot holds"
                    ));
                }
                Err(message) => Some(message),
            },
            EventLogStatus::Invalid(message) => Some(message),
            EventLogStatus::Snapshot => None,
        };
        match captured {
            Some(message) => {
                for record in order {
                    holds.push(json!({
                        "machine": machine,
                        "slot": record.slot,
                        "hold": Value::Null,
                        "error": message,
                    }));
                }
            }
            None => {
                for record in order.iter() {
                    let (hold, error) = match parse_hold(root, Some(machine), &record.slot) {
                        Ok(value) => (value, Value::Null),
                        Err(message) => (Value::Null, Value::String(message)),
                    };
                    holds.push(json!({
                        "machine": machine,
                        "slot": record.slot,
                        "hold": hold,
                        "error": error,
                    }));
                }
            }
        }
    }
    Ok(json!({
        "machines": machines,
        "holds": holds,
    }))
}
