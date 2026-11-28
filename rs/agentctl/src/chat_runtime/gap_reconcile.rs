//! Provider gaps never halt the bridge (#2440 chat-bridge-never-halt, D5).
//!
//! A provider that loses events reports a gap: one batch holding one gap event, whose cursor is
//! the gap's far side (where the provider's stream goes on). Admission takes that batch like a
//! checkpoint: the cursor moves to the far side through the normal commit path and the bridge
//! keeps running. The messages inside the gap come back from the provider's history (the Slack
//! plugin's periodic rescan reads the last day of the channel and admits what the stream did
//! not carry); the bridge's dedupe by message key absorbs any copy it already has.
//!
//! The gap is recorded twice, and nothing else changes shape:
//!
//! * `gap.json`, the audit record, written as before, but only ever with phase `resolved`: an
//!   older bridge reads that file, its phase is a closed enum, and it treats anything but
//!   `unresolved` as no gap (so a rollback never stops on it).
//! * [`GAP_RECONCILE_FILE`], new: the window the provider's history has to fill, and whether the
//!   bridge's agent and the owner were told. The service puts ONE line in the agent's pane and
//!   posts ONE owner-alarm notice per gap (see `chat_service::gap_reconcile`); `chat status`
//!   shows the record and `chat health` reports it, DEGRADED until it is finished, for
//!   [`GAP_RECONCILE_SHOWN`] after the gap. An older bridge never reads this file, so a rollback
//!   leaves an unfinished reconcile unfinished; that is accepted.
//!
//! A gap batch that is not a provider gap (a gap mixed with other events, a protocol violation)
//! keeps the old halt: an `unresolved` gap.json that only an operator approval clears. An
//! `unresolved` gap.json an older bridge wrote for a provider gap no longer stops a reconnect:
//! the next admitted batch resolves it and opens a reconcile for its window.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    agent, read_document, write_document, BridgeState, ChatRuntimeError, GapDiagnostic, Result,
};
use crate::forward_compat::KeptKeys;

/// The open or most recent gap reconcile, beside `gap.json`.
pub(crate) const GAP_RECONCILE_FILE: &str = "gap-reconcile.json";
const GAP_RECONCILE_SCHEMA: &str = "agentctl-chat-gap-reconcile/v1";
const MAX_GAP_RECONCILE_BYTES: usize = 64 * 1024;
/// How long after its gap `chat status` and `chat health` show a reconcile.
pub const GAP_RECONCILE_SHOWN: Duration = Duration::from_secs(24 * 60 * 60);
/// Provider activity after the gap before its reconcile counts as finished: at least three
/// passes of the Slack plugin's 90-second rescan, which admits the window's messages.
pub const GAP_RECONCILE_SETTLE: Duration = Duration::from_secs(5 * 60);
/// How far back the provider's history is read for missed messages (the Slack plugin's default
/// `rescan_window_seconds`); a message older than this at the gap cannot be backfilled.
pub const GAP_RESCAN_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// [`GAP_RECONCILE_FILE`]: one provider gap the bridge kept running through, and what its
/// provider's history has to fill.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GapReconcile {
    schema: String,
    /// What the provider said about the gap.
    pub reasons: Vec<String>,
    /// The cursor before the gap (the last one committed).
    pub prior_cursor: Option<String>,
    /// The gap batch's cursor, the far side, which the bridge committed.
    pub proposed_cursor: String,
    /// The gap batch's provider sequence and delivery.
    pub provider_sequence: u64,
    pub delivery_id: String,
    /// When the bridge saw the gap.
    pub observed_at_millis: u64,
    /// When the last batch before the gap was committed: the window's start, when known.
    pub window_from_millis: Option<u64>,
    /// The host batch that admitted the gap.
    pub host_batch_sequence: u64,
    /// Whether the line in the agent's pane was queued.
    #[serde(default)]
    pub agent_told: bool,
    /// Whether the owner-alarm notice was handed to the outbound path.
    #[serde(default)]
    pub owner_told: bool,
    #[serde(flatten)]
    unknown: KeptKeys,
}

impl GapReconcile {
    /// Built from the resolved gap record and the window's start.
    pub(super) fn new(
        gap: &GapDiagnostic,
        window_from_millis: Option<u64>,
        host_batch_sequence: u64,
    ) -> Self {
        Self {
            schema: GAP_RECONCILE_SCHEMA.to_owned(),
            reasons: gap.reasons.clone(),
            prior_cursor: gap.resume_cursor.clone(),
            proposed_cursor: gap.proposed_cursor.clone(),
            provider_sequence: gap.provider_sequence,
            delivery_id: gap.delivery_id.clone(),
            observed_at_millis: gap.observed_at_millis,
            window_from_millis,
            host_batch_sequence,
            agent_told: false,
            owner_told: false,
            unknown: KeptKeys::default(),
        }
    }

    /// The message ID of the line in the agent's pane: one per gap, so a restarted service never
    /// queues it twice.
    #[must_use]
    pub fn notice_id(&self) -> String {
        format!("chat-gap-reconciled-{}", self.observed_at_millis)
    }

    /// The earliest message the provider's history can still give back.
    #[must_use]
    pub fn backfill_floor_millis(&self) -> u64 {
        self.observed_at_millis
            .saturating_sub(u64::try_from(GAP_RESCAN_WINDOW.as_millis()).unwrap_or(u64::MAX))
    }

    /// Whether part of the window is older than the provider's history reaches.
    #[must_use]
    pub fn window_exceeds_history(&self) -> bool {
        self.window_from_millis
            .is_none_or(|from| from < self.backfill_floor_millis())
    }

    /// Finished once the agent was told and the provider stream has stayed alive
    /// [`GAP_RECONCILE_SETTLE`] past the gap, long enough for the rescan to fill the window.
    #[must_use]
    pub fn finished(&self, provider_alive_at_millis: Option<u64>) -> bool {
        let settled_at = self
            .observed_at_millis
            .saturating_add(u64::try_from(GAP_RECONCILE_SETTLE.as_millis()).unwrap_or(u64::MAX));
        self.agent_told && provider_alive_at_millis.is_some_and(|alive| alive >= settled_at)
    }

    /// Whether `chat status` and `chat health` still show it at `now_millis`.
    #[must_use]
    pub fn shown_at(&self, now_millis: u64) -> bool {
        now_millis.saturating_sub(self.observed_at_millis)
            < u64::try_from(GAP_RECONCILE_SHOWN.as_millis()).unwrap_or(u64::MAX)
    }

    /// The window in words, in ET: from the last delivery before the gap to the gap.
    #[must_use]
    pub fn window_text(&self) -> String {
        let at = |millis: u64| {
            let seconds = millis as f64 / 1_000.0;
            format!(
                "{} {}",
                crate::chat_trace::et_date(seconds),
                crate::chat_trace::et_clock(seconds)
            )
        };
        match self.window_from_millis {
            Some(from) => format!("{} to {}", at(from), at(self.observed_at_millis)),
            None => format!(
                "the unknown time of the last delivery to {}",
                at(self.observed_at_millis)
            ),
        }
    }

    fn reason_text(&self) -> String {
        self.reasons.join("; ")
    }

    /// The ONE line put in the bridge agent's pane.
    #[must_use]
    pub fn agent_notice(&self) -> String {
        let mut text = format!(
            "[chat-bridge ALARM] The chat provider reported a gap in its event stream ({}). The \
             bridge did not stop: it kept running from the gap's far side, and the messages sent \
             in the gap's window ({}) are being admitted from the channel's history (the rescan \
             reads the last {} h), so they reach you as normal requests; copies already delivered \
             are dropped.",
            self.reason_text(),
            self.window_text(),
            GAP_RESCAN_WINDOW.as_secs() / 3_600,
        );
        if self.window_exceeds_history() {
            text.push_str(&format!(
                " Messages from before {} are older than that history and cannot be backfilled: \
                 check the channel for them by hand.",
                crate::chat_trace::et_clock(self.backfill_floor_millis() as f64 / 1_000.0)
            ));
        }
        text.push_str(" `agentctl chat status` shows the reconcile.");
        text
    }

    /// The ONE owner-alarm notice's text (after the alarm's warning prefix).
    #[must_use]
    pub fn owner_notice(&self, agent_label: &str) -> String {
        let mut text = format!(
            "the chat provider lost events in a gap ({}). The bridge to {agent_label} kept \
             running; your messages from {} are being picked up from the channel's history and \
             delivered.",
            self.reason_text(),
            self.window_text(),
        );
        if self.window_exceeds_history() {
            text.push_str(&format!(
                " Anything you sent more than {} h before the gap cannot be recovered that way; \
                 send it again if it still matters.",
                GAP_RESCAN_WINDOW.as_secs() / 3_600
            ));
        }
        text
    }
}

impl BridgeState {
    fn gap_reconcile_path(&self) -> std::path::PathBuf {
        self.root.join(GAP_RECONCILE_FILE)
    }

    /// The most recent gap reconcile, if any.
    #[must_use]
    pub fn gap_reconcile(&self) -> Option<GapReconcile> {
        read_document::<GapReconcile>(&self.gap_reconcile_path(), MAX_GAP_RECONCILE_BYTES)
            .ok()
            .filter(|record| record.schema == GAP_RECONCILE_SCHEMA)
    }

    /// When the provider worker last took an item, from its liveness file.
    pub(crate) fn provider_alive_at_millis(&self, now_millis: u64) -> Option<u64> {
        self.provider_alive_age_millis(now_millis)
            .map(|age| now_millis.saturating_sub(age))
    }

    /// Open the reconcile for a gap admission just resolved, under the state lock. The same gap
    /// offered again (same far side, same reasons) keeps its record, so it is told about once; a
    /// new gap while one is unfinished widens the window back to the older one's start.
    pub(super) fn open_gap_reconcile_locked(
        &self,
        gap: &GapDiagnostic,
        window_from_millis: Option<u64>,
        host_batch_sequence: u64,
    ) -> Result<()> {
        let mut record = GapReconcile::new(gap, window_from_millis, host_batch_sequence);
        if let Some(previous) = self.gap_reconcile() {
            if previous.proposed_cursor == record.proposed_cursor
                && previous.reasons == record.reasons
            {
                return Ok(());
            }
            let now = super::unix_millis();
            if !previous.finished(self.provider_alive_at_millis(now)) && previous.shown_at(now) {
                record.window_from_millis =
                    match (previous.window_from_millis, record.window_from_millis) {
                        (Some(old), Some(new)) => Some(old.min(new)),
                        _ => None,
                    };
            }
        }
        if super::encoded_document_bytes(&record)? > MAX_GAP_RECONCILE_BYTES {
            return Err(ChatRuntimeError::invalid(
                "gap reconcile record exceeds its byte bound",
            ));
        }
        write_document(&self.gap_reconcile_path(), &record)
    }

    /// Record that the agent (`agent`) or the owner (`owner`) was told about the reconcile of
    /// the gap seen at `observed_at_millis`; a newer gap's record is left alone.
    pub(crate) fn mark_gap_reconcile_told(
        &self,
        observed_at_millis: u64,
        agent_told: bool,
        owner_told: bool,
    ) -> Result<()> {
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        fs2::FileExt::lock_exclusive(&state_lock).map_err(ChatRuntimeError::Io)?;
        let _state_lock = agent_queue::fsio::HeldLock::new(state_lock);
        let Some(mut record) = self.gap_reconcile() else {
            return Ok(());
        };
        if record.observed_at_millis != observed_at_millis {
            return Ok(());
        }
        record.agent_told |= agent_told;
        record.owner_told |= owner_told;
        write_document(&self.gap_reconcile_path(), &record)
    }

    /// The reconcile as `chat status` shows it: `null` once [`GAP_RECONCILE_SHOWN`] has passed.
    pub(crate) fn gap_reconcile_status(&self, now_millis: u64) -> Value {
        let Some(record) = self
            .gap_reconcile()
            .filter(|record| record.shown_at(now_millis))
        else {
            return Value::Null;
        };
        let finished = record.finished(self.provider_alive_at_millis(now_millis));
        json!({
            "reasons": record.reasons,
            "prior_cursor": record.prior_cursor,
            "proposed_cursor": record.proposed_cursor,
            "provider_sequence": record.provider_sequence,
            "delivery_id": record.delivery_id,
            "observed_at_millis": record.observed_at_millis,
            "window_from_millis": record.window_from_millis,
            "window": record.window_text(),
            "backfill_floor_millis": record.backfill_floor_millis(),
            "window_exceeds_history": record.window_exceeds_history(),
            "host_batch_sequence": record.host_batch_sequence,
            "agent_told": record.agent_told,
            "owner_told": record.owner_told,
            "finished": finished,
            "settle_seconds": GAP_RECONCILE_SETTLE.as_secs(),
        })
    }
}
