//! The single source of truth for the agent-queue on-disk JSON formats.
//!
//! The structs in this module define the queue item, the cancellation
//! tombstone and the quarantine sidecar. The JSON Schemas under
//! `schemas/` are generated from these types (see
//! `examples/generate_schemas.rs`) and checked in; no other definition
//! of these formats may be maintained. Readers stay permissive: an item
//! may carry sender fields not listed here, and the schema says so.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Delivery state recorded on an item as it moves through the queue.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// Written when an item returns to a pending lane.
    Pending,
    /// Past the at-most-once barrier.
    Inflight,
    /// Delivery confirmed.
    Processed,
    /// Set aside untyped: cancelled or expired.
    Cancelled,
}

/// Why an item was set aside in `cancelled/`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelReason {
    /// A tombstone named the item.
    Cancelled,
    /// Its `not_after` deadline passed.
    Expired,
}

/// One recorded delivery misroute.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct Misroute {
    /// Unix seconds of the misroute.
    pub at: f64,
    /// What went wrong.
    pub error: String,
}

/// A queue item: the JSON object in one phase file of a queue.
///
/// Only `text` is required. The writers in [`crate::queue`] add `id`,
/// `queued_at` and `delivery_attempts` at enqueue; drainers add the
/// delivery fields. Sender fields not listed here are allowed and
/// ignored by readers, so the schema allows additional properties.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct QueueItem {
    /// The prompt text; never empty.
    #[schemars(length(min = 1))]
    pub text: String,
    /// The message identifier; equals the file name without `.json`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Unix seconds, fractional, when the item was queued.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queued_at: Option<f64>,
    /// How many delivery attempts have been made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_attempts: Option<u64>,
    /// The item's delivery state, when a drainer has recorded one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_state: Option<DeliveryState>,
    /// The last delivery error, when one was recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_error: Option<String>,
    /// Unix seconds when delivery was found blocked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_blocked_at: Option<f64>,
    /// Whether the item may have been typed already.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub possibly_submitted: Option<bool>,
    /// Unix seconds when the item entered `inflight/`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inflight_at: Option<f64>,
    /// Whether a tracking drainer stamped the item at the barrier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paste_tracking: Option<bool>,
    /// The lane the item entered `inflight/` from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inflight_from: Option<String>,
    /// Unix seconds immediately before the first terminal input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paste_started_at: Option<f64>,
    /// Unix seconds when delivery was confirmed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmed_at: Option<f64>,
    /// Unix seconds when delivery failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_failed_at: Option<f64>,
    /// Alias of `delivery_attempts` written by some drainers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tui_delivery_attempts: Option<u64>,
    /// Last terminal-delivery error recorded by a drainer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tui_delivery_error: Option<String>,
    /// When terminal delivery last failed, as an ISO timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tui_delivery_failed_at: Option<String>,
    /// Recorded misroutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub misroutes: Option<Vec<Misroute>>,
    /// Whether the item probably reached the wrong terminal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probable_misroute: Option<bool>,
    /// The text actually typed, when it is not `text`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typed_as: Option<String>,
    /// The numbered placeholder the harness drew for the paste, without its blanks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paste_label: Option<String>,
    /// Unix seconds when the prompt was first left stranded at the bottom of a TUI's screen
    /// after its Enter presses; it stays pending, and later passes only press Enter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stranded_at: Option<f64>,
    /// Drain passes that found the prompt still stranded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stranded_passes: Option<u64>,
    /// Unix seconds when an unsent draft was recovered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft_recovered_at: Option<f64>,
    /// How a recovered draft was evidenced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft_recovered_evidence: Option<String>,
    /// Unix time in milliseconds after which no drainer types the
    /// item; only an `inbox.v2` item may carry one. `null` is none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_after: Option<u64>,
    /// Why the item was set aside, when it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_reason: Option<CancelReason>,
    /// Detail for the set-aside.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_detail: Option<String>,
    /// Unix seconds when the item was set aside.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancelled_at: Option<f64>,
    /// Original send time recorded by a sender, in its own units.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<Value>,
    /// Opening text recorded by a sender.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opening: Option<String>,
    /// Sender fields not defined here; readers ignore them.
    #[serde(flatten)]
    #[schemars(schema_with = "extra_fields_schema")]
    pub extra: Map<String, Value>,
}

impl QueueItem {
    /// The document [`crate::queue::plain_item`] writes at enqueue.
    #[must_use]
    pub fn plain(identifier: &str, text: &str) -> Self {
        Self {
            text: text.to_owned(),
            id: Some(identifier.to_owned()),
            queued_at: Some(crate::queue::unix_seconds()),
            delivery_attempts: Some(0),
            delivery_state: None,
            delivery_error: None,
            delivery_blocked_at: None,
            possibly_submitted: None,
            inflight_at: None,
            paste_tracking: None,
            inflight_from: None,
            paste_started_at: None,
            confirmed_at: None,
            delivery_failed_at: None,
            tui_delivery_attempts: None,
            tui_delivery_error: None,
            tui_delivery_failed_at: None,
            misroutes: None,
            probable_misroute: None,
            typed_as: None,
            paste_label: None,
            stranded_at: None,
            stranded_passes: None,
            draft_recovered_at: None,
            draft_recovered_evidence: None,
            not_after: None,
            cancel_reason: None,
            cancel_detail: None,
            cancelled_at: None,
            sent_at: None,
            opening: None,
            extra: Map::new(),
        }
    }
}

fn extra_fields_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(serde_json::json!({
        "type": "object",
        "additionalProperties": true
    }))
    .expect("extra-fields schema is valid")
}

/// A cancellation tombstone: `cancel/<id>.json`.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct Tombstone {
    /// The message identifier not to type.
    pub id: String,
    /// Why it was cancelled.
    pub reason: String,
    /// Unix seconds when it was cancelled.
    pub cancelled_at: f64,
}

/// Outcome recorded in a quarantine sidecar.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuarantineOutcome {
    /// The item may have been typed; never replay it.
    PossiblySubmitted,
    /// The item is not a valid queue item.
    InvalidMessage,
}

/// The quarantine sidecar `failed/<id>.json.error`.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct QuarantineSidecar {
    /// The failed item's file name.
    pub artifact: String,
    /// Why it was quarantined.
    pub outcome: QuarantineOutcome,
    /// What went wrong.
    pub error: String,
    /// Unix seconds when it was quarantined.
    pub failed_at: f64,
}

/// The generated schema of each format, keyed by file stem.
#[must_use]
pub fn generated_schemas() -> Vec<(&'static str, Value)> {
    vec![
        (
            "queue-item",
            serde_json::to_value(schemars::schema_for!(QueueItem))
                .expect("queue item schema serializes"),
        ),
        (
            "tombstone",
            serde_json::to_value(schemars::schema_for!(Tombstone))
                .expect("tombstone schema serializes"),
        ),
        (
            "quarantine-sidecar",
            serde_json::to_value(schemars::schema_for!(QuarantineSidecar))
                .expect("quarantine sidecar schema serializes"),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_item_allows_sender_fields_and_rejects_empty_text() {
        let item: QueueItem = serde_json::from_value(serde_json::json!({
            "id": "a", "text": "hello", "queued_at": 1.5,
            "delivery_attempts": 0, "sender_field": {"x": 1}
        }))
        .unwrap();
        assert_eq!(item.text, "hello");
        assert!(item.extra.contains_key("sender_field"));
        let schema = schemars::schema_for!(QueueItem);
        let text = serde_json::to_value(&schema).unwrap()["properties"]["text"].clone();
        assert_eq!(text["minLength"], 1);
    }

    #[test]
    fn every_format_generates_a_schema() {
        assert_eq!(generated_schemas().len(), 3);
    }
}
