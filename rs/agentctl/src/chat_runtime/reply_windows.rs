//! Reply windows (#2338 chat-bridge-reply-window-retire): when a request's replies close by
//! themselves, so that the request retires and a busy bridge never fills up with requests that
//! were answered long ago.
//!
//! A request retires only once its replies are closed (see [`BridgeState::close_replies`]).
//! Nothing used to close them but `agentctl chat close`, which no adapter ever called, so every
//! request stayed: a bridge serving a busy coordinator reached [`MAX_REQUESTS`] and refused its
//! owner's next messages, which were then never acknowledged. The rules now are:
//!
//! * An answered request (at least one reply SENT to the chat, every reply sent or permanently
//!   failed, the prompt delivered and the acknowledgement settled) closes
//!   `answered_quiet_seconds` (24 h) after its last reply, so a progress reply sent within that
//!   time still reaches the chat.
//! * An unanswered request (delivered and acknowledged, no reply sent: none captured, or every
//!   one failed) closes `unanswered_max_age_seconds` (48 h) after its admission: well past the
//!   reply reminders' window, and never at the end of a turn.
//! * A request whose prompt is pending, being submitted or uncertain, or which has a reply that is
//!   not sent yet, never closes by these rules.
//! * At the request population limit, answered requests close oldest last reply first, with no age
//!   floor, then unanswered requests admitted more than `pressure_unanswered_age_seconds` (24 h)
//!   ago, oldest first, and then any other unanswered request, oldest first, until the new batch
//!   fits: an owner message is never held while an unanswered request could make room (#2440,
//!   lead-014). An owner's request closed that way while it awaited a reply gets an alarm line,
//!   naming its key, in the agent's notice.
//!
//! An unanswered request is never closed silently: before any is closed, the batch is appended to
//! [`CLOSURE_OUTBOX_FILE`] (one batch per sweep or per forced close), and the service drains it:
//! it logs each request, queues one notice to the bridge's agent, and posts one owner-alarm notice
//! for the requests an owner sent.
//!
//! When nothing can be closed (every request's prompt or reply is still in flight), admission
//! refuses as before: the batch is not committed, so the
//! provider redelivers it from the saved cursor once space frees. [`REQUEST_CAPACITY_FILE`]
//! records the hold until the next admission succeeds; `chat health` reports it, and the owner
//! alarm warns about it once an hour and once more when it clears.

use std::io;
use std::time::Duration;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    agent, current_reply_id, read_document, remove_if_exists, write_document, AckPhase,
    BridgeState, ChatRuntimeError, Checkpoint, ReplyPhase, RequestPhase, RequestRecord, Result,
    MAX_REQUESTS, MAX_REQUEST_BYTES,
};
use crate::forward_compat::KeptKeys;

/// Default quiet time after an answered request's last reply before its replies close.
pub const DEFAULT_ANSWERED_QUIET: Duration = Duration::from_secs(24 * 60 * 60);
/// Default age after admission at which an unanswered request's replies close.
pub const DEFAULT_UNANSWERED_MAX_AGE: Duration = Duration::from_secs(48 * 60 * 60);
/// Default age after admission past which an unanswered request may be closed at the limit.
pub const DEFAULT_PRESSURE_UNANSWERED_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// Default fill, in percent of the request limit, at which the bridge's agent is warned that the
/// store is nearly full, before any refusal; and the fill below which it is told it is not.
pub const DEFAULT_HIGH_WATER_PERCENT: u64 = 90;
pub const DEFAULT_LOW_WATER_PERCENT: u64 = 80;
/// How often the service looks for requests whose window has ended.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// The shortest and longest window a configuration may set, in seconds.
const MIN_WINDOW_SECONDS: u64 = 60;
const MAX_WINDOW_SECONDS: u64 = 366 * 24 * 60 * 60;

/// The list of unanswered requests closed by a window, waiting for the service to tell about them.
pub(crate) const CLOSURE_OUTBOX_FILE: &str = "reply-window-closures.json";
const CLOSURE_OUTBOX_SCHEMA: &str = "agentctl-chat-reply-window-closures/v1";
/// Most batches the outbox keeps; past it the two oldest are merged into one (#2430 C1).
const MAX_CLOSURE_BATCHES: usize = 64;
const MAX_CLOSURE_OUTBOX_BYTES: usize = 8 * 1024 * 1024;
/// Characters of a closed request's first line the outbox keeps.
pub(crate) const FIRST_LINE_CHARS: usize = 120;

/// The hold admission records while the request population is full.
pub(crate) const REQUEST_CAPACITY_FILE: &str = "request-capacity.json";
const REQUEST_CAPACITY_SCHEMA: &str = "agentctl-chat-request-capacity/v1";
const MAX_REQUEST_CAPACITY_BYTES: usize = 64 * 1024;

/// The `reply_windows` object of a bridge configuration: each window in seconds, absent for its
/// default.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReplyWindowSettings {
    /// Quiet time after an answered request's last reply before its replies close; default
    /// [`DEFAULT_ANSWERED_QUIET`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_quiet_seconds: Option<u64>,
    /// Age after admission at which an unanswered request's replies close; default
    /// [`DEFAULT_UNANSWERED_MAX_AGE`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unanswered_max_age_seconds: Option<u64>,
    /// Age after admission past which an unanswered request may be closed to make room at the
    /// request population limit; default [`DEFAULT_PRESSURE_UNANSWERED_AGE`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressure_unanswered_age_seconds: Option<u64>,
    /// Fill, in percent of the request limit, at which the bridge's agent is warned; default
    /// [`DEFAULT_HIGH_WATER_PERCENT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high_water_percent: Option<u64>,
    /// Fill below which the agent is told the store is no longer nearly full; default
    /// [`DEFAULT_LOW_WATER_PERCENT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low_water_percent: Option<u64>,
    /// Keys a newer agentctl added.
    #[serde(flatten)]
    pub unknown: KeptKeys,
}

/// The windows a bridge applies, in milliseconds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ReplyWindowRules {
    pub(crate) answered_quiet_millis: u64,
    pub(crate) unanswered_max_age_millis: u64,
    pub(crate) pressure_unanswered_age_millis: u64,
    pub(crate) high_water_percent: u64,
    pub(crate) low_water_percent: u64,
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

impl ReplyWindowSettings {
    /// Refuse a window outside one minute to one year.
    pub(crate) fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("answered_quiet_seconds", self.answered_quiet_seconds),
            (
                "unanswered_max_age_seconds",
                self.unanswered_max_age_seconds,
            ),
            (
                "pressure_unanswered_age_seconds",
                self.pressure_unanswered_age_seconds,
            ),
        ] {
            if let Some(seconds) = value {
                if !(MIN_WINDOW_SECONDS..=MAX_WINDOW_SECONDS).contains(&seconds) {
                    return Err(ChatRuntimeError::invalid(format!(
                        "reply_windows.{name} must be {MIN_WINDOW_SECONDS}-{MAX_WINDOW_SECONDS} \
                         seconds"
                    )));
                }
            }
        }
        let rules = self.rules();
        if !(1..=100).contains(&rules.high_water_percent)
            || rules.low_water_percent >= rules.high_water_percent
        {
            return Err(ChatRuntimeError::invalid(
                "reply_windows.high_water_percent must be 1-100 and above low_water_percent",
            ));
        }
        Ok(())
    }

    /// The windows, with each absent one at its default.
    pub(crate) fn rules(&self) -> ReplyWindowRules {
        let pick = |value: Option<u64>, default: Duration| {
            value.map_or_else(|| millis(default), |seconds| seconds.saturating_mul(1_000))
        };
        ReplyWindowRules {
            answered_quiet_millis: pick(self.answered_quiet_seconds, DEFAULT_ANSWERED_QUIET),
            unanswered_max_age_millis: pick(
                self.unanswered_max_age_seconds,
                DEFAULT_UNANSWERED_MAX_AGE,
            ),
            pressure_unanswered_age_millis: pick(
                self.pressure_unanswered_age_seconds,
                DEFAULT_PRESSURE_UNANSWERED_AGE,
            ),
            high_water_percent: self
                .high_water_percent
                .unwrap_or(DEFAULT_HIGH_WATER_PERCENT),
            low_water_percent: self.low_water_percent.unwrap_or(DEFAULT_LOW_WATER_PERCENT),
        }
    }
}

/// Where a request's reply window stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Window {
    /// Its replies are already closed, or it is not settled enough to close: its prompt is not
    /// confirmed delivered, its acknowledgement is not settled, or a reply is not sent yet.
    Open,
    /// At least one reply was sent and every reply is terminal (sent or permanently failed);
    /// the last one became terminal at this time.
    Answered { last_reply_millis: u64 },
    /// Delivered and acknowledged, with no reply sent: none captured, or every one failed.
    Unanswered,
}

/// Why a batch of requests was closed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClosureCause {
    /// Their window ended.
    Age,
    /// The request population was full.
    Capacity,
}

/// One unanswered request a window closed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ClosedRequest {
    pub(crate) key: String,
    /// The reply ID its prompt gave.
    pub(crate) reply_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) message_number: Option<u64>,
    /// The first line of the message, cut to [`FIRST_LINE_CHARS`].
    pub(crate) first_line: String,
    pub(crate) sender_id: String,
    /// Whether an owner (a permanent allowed sender) sent it.
    pub(crate) from_owner: bool,
    pub(crate) admitted_at_millis: u64,
    #[serde(flatten)]
    pub(crate) unknown: KeptKeys,
}

/// The requests one sweep or one forced close closed, and which of its notices are done.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ClosureBatch {
    /// Stable for the batch: the agent notice's queue ID is derived from it.
    pub(crate) id: String,
    pub(crate) cause: ClosureCause,
    pub(crate) closed_at_millis: u64,
    /// Answered requests closed with it, counted only (they need no notice).
    #[serde(default)]
    pub(crate) answered_closed: u64,
    /// Unanswered requests closed with it.
    #[serde(default)]
    pub(crate) requests: Vec<ClosedRequest>,
    #[serde(default)]
    pub(crate) logged: bool,
    #[serde(default)]
    pub(crate) agent_notified: bool,
    #[serde(default)]
    pub(crate) owner_notified: bool,
    #[serde(flatten)]
    pub(crate) unknown: KeptKeys,
}

impl ClosureBatch {
    fn done(&self) -> bool {
        self.logged && self.agent_notified && self.owner_notified
    }
}

/// [`CLOSURE_OUTBOX_FILE`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct ClosureOutbox {
    schema: String,
    #[serde(default)]
    batches: Vec<ClosureBatch>,
    /// Batches dropped untold because the outbox was full.
    #[serde(default)]
    dropped_batches: u64,
    #[serde(flatten)]
    unknown: KeptKeys,
}

/// A step of telling about a closure batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClosureStep {
    Logged,
    AgentNotified,
    OwnerNotified,
}

/// [`REQUEST_CAPACITY_FILE`]: the request population is full, so admission refuses new messages
/// (they wait in the provider stream, uncommitted, until space frees).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequestCapacityHold {
    schema: String,
    /// When the first refusal of this hold happened.
    pub held_since_millis: u64,
    /// When the latest refusal happened.
    pub last_refused_at_millis: u64,
    /// Refusals since the hold began.
    pub refusals: u64,
    /// The retained request count at the latest refusal.
    pub request_count: u64,
    /// The retained request bytes at the latest refusal.
    pub request_bytes: u64,
    /// New requests the refused batch held.
    pub waiting_messages: u64,
    /// The request count limit.
    pub max_requests: u64,
    /// The request byte limit.
    pub max_request_bytes: u64,
    #[serde(flatten)]
    unknown: KeptKeys,
}

/// What one sweep closed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct WindowSweep {
    pub(crate) answered: u64,
    pub(crate) unanswered: u64,
    /// Of those, how many retired at once (the rest retire at the next retirement chance).
    pub(crate) retired: u64,
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let mut cut: String = line
        .chars()
        .filter(|character| !character.is_control())
        .take(FIRST_LINE_CHARS)
        .collect();
    if line.chars().count() > FIRST_LINE_CHARS {
        cut.push('…');
    }
    cut
}

impl BridgeState {
    /// The windows this bridge applies.
    pub(crate) fn reply_window_rules(&self) -> ReplyWindowRules {
        self.config
            .reply_windows
            .as_ref()
            .map(ReplyWindowSettings::rules)
            .unwrap_or_else(|| ReplyWindowSettings::default().rules())
    }

    fn window_of(&self, record: &RequestRecord) -> Result<Window> {
        if record.reply_closed
            || record.phase != RequestPhase::Delivered
            || !matches!(record.ack_phase, AckPhase::Disabled | AckPhase::Acked)
            || record.next_send_ordinal != record.next_reply_ordinal
            || record.admitted_cursor.is_none()
        {
            return Ok(Window::Open);
        }
        if record.reply_count == 0 {
            return Ok(Window::Unanswered);
        }
        let mut last_reply_millis = record.admitted_at_millis;
        let mut sent = false;
        for ordinal in 1..=record.reply_count {
            let reply = match self.read_reply(&record.key, ordinal) {
                Ok(reply) => reply,
                // A reply that cannot be read is not known to be terminal.
                Err(_) => return Ok(Window::Open),
            };
            match reply.phase {
                ReplyPhase::Sent => {
                    sent = true;
                    last_reply_millis = last_reply_millis
                        .max(reply.sent_at_millis.unwrap_or(reply.captured_at_millis));
                }
                ReplyPhase::Failed => {
                    last_reply_millis = last_reply_millis.max(
                        reply
                            .last_send_failure_at_millis
                            .unwrap_or(reply.captured_at_millis),
                    );
                }
                // A reply still to send: the request is mid-reply and stays open.
                ReplyPhase::Pending | ReplyPhase::Sending => return Ok(Window::Open),
            }
        }
        // Answered means a reply reached the chat: one whose every reply failed is unanswered,
        // so it waits the unanswered window and is listed when it closes.
        Ok(if sent {
            Window::Answered { last_reply_millis }
        } else {
            Window::Unanswered
        })
    }

    fn closed_request(&self, record: &RequestRecord) -> ClosedRequest {
        let alias = self
            .read_reply_aliases()
            .ok()
            .and_then(|aliases| aliases.alias_of(&record.key));
        ClosedRequest {
            key: record.key.clone(),
            reply_id: current_reply_id(record, alias),
            message_number: record.message_number,
            first_line: first_line(&record.message.text),
            sender_id: record.message.sender_id.clone(),
            from_owner: self
                .config
                .allowed_senders
                .contains(&record.message.sender_id),
            admitted_at_millis: record.admitted_at_millis,
            unknown: KeptKeys::default(),
        }
    }

    /// Close one request's replies and retire it when it is eligible: the body of
    /// [`Self::close_replies`], under the state lock. Whether it retired.
    pub(super) fn close_and_retire_locked(
        &self,
        checkpoint: &mut Checkpoint,
        key: &str,
    ) -> Result<bool> {
        let mut request = match self.read_request(key) {
            Ok(request) => request,
            Err(ChatRuntimeError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                if self.retired_key(key)?.is_some() {
                    return Ok(true);
                }
                return Err(ChatRuntimeError::Io(error));
            }
            Err(error) => return Err(error),
        };
        if !request.reply_closed {
            request.reply_closed = true;
            self.write_request_accounted(&request, checkpoint)?;
            // Retirement is deliberately the next fallible phase. Persist the exact mutated
            // record size first so an in-process retry never subtracts from stale accounting.
            self.persist_checkpoint(checkpoint)?;
        }
        self.retire_if_eligible_locked(checkpoint, key)
    }

    /// Close the replies of every request whose window has ended at `now_millis`, and retire
    /// each that can retire. Unanswered requests are listed in the closure outbox before any is
    /// closed. The service calls this every [`SWEEP_INTERVAL`].
    pub(crate) fn close_settled_replies(&self, now_millis: u64) -> Result<WindowSweep> {
        let rules = self.reply_window_rules();
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        state_lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
        let _state_lock = agent_queue::fsio::HeldLock::new(state_lock);
        let mut checkpoint = self.read_checkpoint()?;
        self.complete_pending_retirement_locked(&mut checkpoint)?;
        let mut answered = Vec::new();
        let mut unanswered = Vec::new();
        for (record, _) in self.request_records()? {
            match self.window_of(&record)? {
                Window::Open => {}
                Window::Answered { last_reply_millis } => {
                    if now_millis.saturating_sub(last_reply_millis) >= rules.answered_quiet_millis {
                        answered.push(record.key);
                    }
                }
                Window::Unanswered => {
                    if now_millis.saturating_sub(record.admitted_at_millis)
                        >= rules.unanswered_max_age_millis
                    {
                        unanswered.push(record);
                    }
                }
            }
        }
        let mut sweep = WindowSweep::default();
        if !unanswered.is_empty() {
            let listed = unanswered
                .iter()
                .map(|record| self.closed_request(record))
                .collect::<Vec<_>>();
            self.append_closure_batch_locked(ClosureCause::Age, now_millis, 0, listed)?;
        }
        for key in answered {
            sweep.answered += 1;
            sweep.retired += u64::from(self.close_and_retire_locked(&mut checkpoint, &key)?);
        }
        for record in unanswered {
            sweep.unanswered += 1;
            sweep.retired += u64::from(self.close_and_retire_locked(&mut checkpoint, &record.key)?);
        }
        self.persist_checkpoint(&mut checkpoint)?;
        Ok(sweep)
    }

    /// At the request population limit, close answered requests (oldest last reply first) and
    /// then unanswered ones admitted more than the pressure age ago (oldest first) until
    /// `additional_count` more requests of `additional_bytes` fit, retiring each. The unanswered
    /// ones are listed in the closure outbox first. Runs only after the plain retirement scan
    /// left the population over the limit.
    pub(super) fn close_for_capacity_locked(
        &self,
        checkpoint: &mut Checkpoint,
        additional_count: u64,
        additional_bytes: u64,
        now_millis: u64,
    ) -> Result<()> {
        let fits = |checkpoint: &Checkpoint| {
            checkpoint.request_count.saturating_add(additional_count) <= MAX_REQUESTS
                && checkpoint.request_bytes.saturating_add(additional_bytes) <= MAX_REQUEST_BYTES
        };
        if fits(checkpoint) {
            return Ok(());
        }
        let rules = self.reply_window_rules();
        let mut answered = Vec::new();
        let mut unanswered = Vec::new();
        for (record, bytes) in self.request_records()? {
            match self.window_of(&record)? {
                Window::Open => {}
                Window::Answered { last_reply_millis } => {
                    answered.push((last_reply_millis, record.key));
                }
                // Past the pressure age first; then, so that an owner message is never held, any
                // other unanswered request, oldest first (#2440 option 3, lead-014).
                Window::Unanswered => {
                    let young = now_millis.saturating_sub(record.admitted_at_millis)
                        < rules.pressure_unanswered_age_millis;
                    unanswered.push((young, record.admitted_at_millis, bytes, record));
                }
            }
        }
        answered.sort();
        unanswered.sort_by(|left, right| {
            (left.0, left.1, &left.3.key).cmp(&(right.0, right.1, &right.3.key))
        });
        let mut answered_closed = 0;
        for (_, key) in answered {
            if fits(checkpoint) {
                break;
            }
            answered_closed += 1;
            self.close_and_retire_locked(checkpoint, &key)?;
        }
        // The unanswered requests that would bring the population under the limit, judged by
        // their sizes, are listed before any of them is closed.
        let mut count = checkpoint.request_count.saturating_add(additional_count);
        let mut bytes = checkpoint.request_bytes.saturating_add(additional_bytes);
        let mut chosen = Vec::new();
        for (_, _, record_bytes, record) in unanswered {
            if count <= MAX_REQUESTS && bytes <= MAX_REQUEST_BYTES {
                break;
            }
            count = count.saturating_sub(1);
            bytes = bytes.saturating_sub(record_bytes);
            chosen.push(record);
        }
        if answered_closed > 0 || !chosen.is_empty() {
            let listed = chosen
                .iter()
                .map(|record| self.closed_request(record))
                .collect::<Vec<_>>();
            self.append_closure_batch_locked(
                ClosureCause::Capacity,
                now_millis,
                answered_closed,
                listed,
            )?;
        }
        for record in chosen {
            self.close_and_retire_locked(checkpoint, &record.key)?;
        }
        Ok(())
    }

    fn closure_outbox_path(&self) -> std::path::PathBuf {
        self.root.join(CLOSURE_OUTBOX_FILE)
    }

    fn read_closure_outbox(&self) -> Result<ClosureOutbox> {
        match read_document::<ClosureOutbox>(&self.closure_outbox_path(), MAX_CLOSURE_OUTBOX_BYTES)
        {
            Ok(outbox) if outbox.schema == CLOSURE_OUTBOX_SCHEMA => Ok(outbox),
            Ok(outbox) => Err(ChatRuntimeError::invalid(format!(
                "{CLOSURE_OUTBOX_FILE} has unsupported schema {}",
                outbox.schema
            ))),
            Err(ChatRuntimeError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                Ok(ClosureOutbox {
                    schema: CLOSURE_OUTBOX_SCHEMA.to_owned(),
                    batches: Vec::new(),
                    dropped_batches: 0,
                    unknown: KeptKeys::default(),
                })
            }
            Err(error) => Err(error),
        }
    }

    fn write_closure_outbox(&self, outbox: &ClosureOutbox) -> Result<()> {
        if outbox.batches.is_empty() && outbox.dropped_batches == 0 {
            return remove_if_exists(&self.closure_outbox_path());
        }
        write_document(&self.closure_outbox_path(), outbox)
    }

    pub(super) fn append_closure_batch_locked(
        &self,
        cause: ClosureCause,
        now_millis: u64,
        answered_closed: u64,
        requests: Vec<ClosedRequest>,
    ) -> Result<()> {
        let mut outbox = self.read_closure_outbox()?;
        // A request an untold batch already lists (a sweep cut short after listing it, then run
        // again after a restart) is not listed twice.
        let listed: std::collections::BTreeSet<String> = outbox
            .batches
            .iter()
            .flat_map(|batch| batch.requests.iter().map(|request| request.key.clone()))
            .collect();
        let requests: Vec<ClosedRequest> = requests
            .into_iter()
            .filter(|request| !listed.contains(&request.key))
            .collect();
        if requests.is_empty() && answered_closed == 0 {
            return Ok(());
        }
        let mut digest = Sha256::new();
        digest.update(format!("{cause:?}:{now_millis}:{answered_closed}"));
        for request in &requests {
            digest.update(b":");
            digest.update(request.key.as_bytes());
        }
        let id = format!("{:x}", digest.finalize())[..32].to_owned();
        let quiet = requests.is_empty();
        let owners = requests.iter().any(|request| request.from_owner);
        outbox.batches.push(ClosureBatch {
            id,
            cause,
            closed_at_millis: now_millis,
            answered_closed,
            requests,
            logged: false,
            // Only unanswered requests are told to the agent and owner.
            agent_notified: quiet,
            owner_notified: !owners,
            unknown: KeptKeys::default(),
        });
        // Past the bound the two oldest batches are merged, never dropped (#2430 C1): dropping
        // one left its closed requests untold. A step either had not done is redone for both,
        // so a request may be told twice but never not at all; the merged batch takes a new ID,
        // so its agent notice is not mistaken for one already queued.
        while outbox.batches.len() > MAX_CLOSURE_BATCHES {
            let older = outbox.batches.remove(0);
            let newer = &mut outbox.batches[0];
            let mut digest = Sha256::new();
            digest.update(format!("merge:{}:{}", older.id, newer.id));
            newer.id = format!("{:x}", digest.finalize())[..32].to_owned();
            newer.closed_at_millis = newer.closed_at_millis.min(older.closed_at_millis);
            newer.answered_closed = newer.answered_closed.saturating_add(older.answered_closed);
            let mut requests = older.requests;
            requests.append(&mut newer.requests);
            newer.requests = requests;
            newer.logged &= older.logged;
            newer.agent_notified &= older.agent_notified;
            newer.owner_notified &= older.owner_notified;
        }
        self.write_closure_outbox(&outbox)
    }

    /// The closure batches with a step still to tell, oldest first, and how many were dropped
    /// untold because the outbox was full.
    pub(crate) fn closure_batches(&self) -> Result<(Vec<ClosureBatch>, u64)> {
        let _snapshot = self.lock_state_snapshot()?;
        self.read_closure_outbox_unlocked()
    }

    /// [`Self::closure_batches`] for a caller that already holds a state lock.
    pub(super) fn read_closure_outbox_unlocked(&self) -> Result<(Vec<ClosureBatch>, u64)> {
        let outbox = self.read_closure_outbox()?;
        Ok((outbox.batches, outbox.dropped_batches))
    }

    /// Record that `step` is done for batch `id`; a batch with every step done is removed, and
    /// `clear_dropped` forgets the count of dropped batches once it was told.
    pub(crate) fn mark_closure_batch(
        &self,
        id: &str,
        step: ClosureStep,
        clear_dropped: bool,
    ) -> Result<()> {
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        state_lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
        let _state_lock = agent_queue::fsio::HeldLock::new(state_lock);
        let mut outbox = self.read_closure_outbox()?;
        if clear_dropped {
            outbox.dropped_batches = 0;
        }
        if let Some(batch) = outbox.batches.iter_mut().find(|batch| batch.id == id) {
            match step {
                ClosureStep::Logged => batch.logged = true,
                ClosureStep::AgentNotified => batch.agent_notified = true,
                ClosureStep::OwnerNotified => batch.owner_notified = true,
            }
        }
        outbox.batches.retain(|batch| !batch.done());
        self.write_closure_outbox(&outbox)
    }

    /// The retained request count and bytes, from the checkpoint.
    pub(crate) fn request_population(&self) -> Result<(u64, u64)> {
        let _snapshot = self.lock_state_snapshot()?;
        let checkpoint = self.read_checkpoint()?;
        Ok((checkpoint.request_count, checkpoint.request_bytes))
    }

    fn request_capacity_path(&self) -> std::path::PathBuf {
        self.root.join(REQUEST_CAPACITY_FILE)
    }

    /// The current request capacity hold, if admission is refusing for a full population.
    pub fn request_capacity_hold(&self) -> Option<RequestCapacityHold> {
        read_document::<RequestCapacityHold>(
            &self.request_capacity_path(),
            MAX_REQUEST_CAPACITY_BYTES,
        )
        .ok()
        .filter(|hold| hold.schema == REQUEST_CAPACITY_SCHEMA)
    }

    /// Record one refusal for a full population, under the state lock.
    pub(super) fn note_capacity_refusal_locked(
        &self,
        checkpoint: &Checkpoint,
        waiting_messages: u64,
        now_millis: u64,
    ) -> Result<()> {
        let previous = self.request_capacity_hold();
        let hold = RequestCapacityHold {
            schema: REQUEST_CAPACITY_SCHEMA.to_owned(),
            held_since_millis: previous
                .as_ref()
                .map_or(now_millis, |hold| hold.held_since_millis),
            last_refused_at_millis: now_millis,
            refusals: previous
                .as_ref()
                .map_or(0, |hold| hold.refusals)
                .saturating_add(1),
            request_count: checkpoint.request_count,
            request_bytes: checkpoint.request_bytes,
            waiting_messages,
            max_requests: MAX_REQUESTS,
            max_request_bytes: MAX_REQUEST_BYTES,
            unknown: KeptKeys::default(),
        };
        write_document(&self.request_capacity_path(), &hold)
    }

    /// Forget the capacity hold once a batch is admitted.
    pub(super) fn clear_capacity_hold_locked(&self) -> Result<()> {
        remove_if_exists(&self.request_capacity_path())
    }
}

/// The keys of the requests in `batches`.
#[cfg(test)]
pub(crate) fn listed_keys(batches: &[ClosureBatch]) -> std::collections::BTreeSet<String> {
    batches
        .iter()
        .flat_map(|batch| batch.requests.iter().map(|request| request.key.clone()))
        .collect()
}
