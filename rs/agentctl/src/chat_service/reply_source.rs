//! Where the bridge's reply text comes from, and replies it never captured (#783
//! chat-reply-capture-miss).
//!
//! On 2026-11-02 a coordinator printed a well-formed reply block in a message of its own, and the
//! bridge captured nothing: its record named no conversation, so the transcript reader had no
//! file to read, the screen capture missed the block, and the request stayed delivered with no
//! reply and no word to anyone. The transcript reader now discovers such a coordinator's
//! transcript (see `crate::chat_transcript`). At each scan, [`ReplySourceWatch::step`] then:
//!
//! 1. binds into the coordinator's record a conversation that the reader found through a live
//!    process for a record that names none, never waiting for the record's locks (a sender can
//!    hold them for minutes): a busy record is bound at a later scan;
//! 2. finds capture misses ([`find_misses`]): requests delivered and not answered whose reply
//!    block the coordinator's transcript has held for [`CAPTURE_MISS_GRACE`] or longer. Each is
//!    logged and posted to the owner as a `[chat-bridge WARNING]` once, and the coordinator is
//!    told once, as a reply reminder is typed (only while it is ready, never behind another
//!    prompt), with the `chat reply` command that sends the reply; a notice it was not given is
//!    tried again at most every [`NOTICE_RETRY`] while the miss lasts;
//! 3. records where the text comes from, and the misses, in `reply-source.json` for
//!    `chat health`.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::agent::DrainOptions;
use crate::chat_runtime::{
    self, BridgeState, PendingReminder, ReminderCandidate, ReminderOutcome, RootMessageTransport,
    TranscriptBlock,
};
use crate::chat_transcript::ReplySource;
use crate::subagents::{ManagedAgents, ManagedApi};

use super::{
    post_owner_notices, service_log, CancellableDelivery, StopRuntime, StopState, TypingNotes,
    TypingWatch,
};

/// How long a reply block may wait in the coordinator's transcript, uncaptured, before it is a
/// capture miss. A capture takes a block seconds after it is written.
pub(crate) const CAPTURE_MISS_GRACE: Duration = Duration::from_secs(2 * 60);
/// The shortest time between two attempts to tell the coordinator of one miss.
const NOTICE_RETRY: Duration = Duration::from_secs(60);
/// The prefix of a capture-miss notice's ID in the coordinator's queue.
const NOTICE_MESSAGE_PREFIX: &str = "chat-capture-miss-";

/// A request whose reply block the coordinator's transcript holds while the bridge holds no
/// reply to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Miss {
    pub(crate) key: String,
    pub(crate) reply_id: String,
    /// When the transcript recorded the block, in Unix milliseconds.
    pub(crate) block_at_millis: u64,
}

/// The capture misses among `candidates`, the requests delivered and not answered, at
/// `now_millis`: each whose reply ID heads a block in `blocks` that the transcript recorded after
/// the request was delivered and at least `grace` ago. A block with no time is never judged.
pub(crate) fn find_misses(
    candidates: &[ReminderCandidate],
    blocks: &[TranscriptBlock],
    now_millis: u64,
    grace: Duration,
) -> Vec<Miss> {
    let grace = u64::try_from(grace.as_millis()).unwrap_or(u64::MAX);
    candidates
        .iter()
        .filter_map(|candidate| {
            let block_at_millis = blocks
                .iter()
                .filter(|block| block.identifier == candidate.reply_id)
                .filter_map(|block| block.at_millis)
                .filter(|at| *at >= candidate.delivered_at_millis)
                .min()?;
            (now_millis.saturating_sub(block_at_millis) >= grace).then(|| Miss {
                key: candidate.key.clone(),
                reply_id: candidate.reply_id.clone(),
                block_at_millis,
            })
        })
        .collect()
}

/// What one miss has been told.
#[derive(Debug)]
struct Told {
    miss: Miss,
    /// The coordinator has been given its notice, or may have been.
    coordinator: bool,
    /// When the coordinator was last tried, in Unix milliseconds.
    tried_at_millis: Option<u64>,
    attempts: u32,
}

/// The capture-miss watch and conversation binding of one `chat run`: see the module
/// documentation.
#[derive(Debug)]
pub(crate) struct ReplySourceWatch {
    /// How long an uncaptured block waits before it is a miss: [`CAPTURE_MISS_GRACE`].
    grace: Duration,
    /// The misses known, by request key.
    told: BTreeMap<String, Told>,
    /// The last failure to bind, logged once until it changes.
    bind_problem: Option<String>,
    /// The record last written, written again only when it changes.
    written: Option<Value>,
}

impl Default for ReplySourceWatch {
    fn default() -> Self {
        Self {
            grace: CAPTURE_MISS_GRACE,
            told: BTreeMap::new(),
            bind_problem: None,
            written: None,
        }
    }
}

impl ReplySourceWatch {
    /// This watch, judging a block a miss once it is `grace` old.
    #[cfg(test)]
    pub(super) fn with_grace(grace: Duration) -> Self {
        Self {
            grace,
            ..Self::default()
        }
    }

    /// One step, as the module documentation describes. Every problem is logged and the next
    /// step tries again; nothing here ends the loop.
    pub(super) fn step<A: ManagedApi + ?Sized>(
        &mut self,
        state: &BridgeState,
        manager: &ManagedAgents<'_, A>,
        stop: &StopState,
        delivery: DrainOptions,
        transport: Option<&mut dyn RootMessageTransport>,
    ) {
        let Some(source) = state.reply_source() else {
            // No transcript reader: a headless coordinator, or screen text by request.
            return;
        };
        let agent = state.config().agent_name.clone();
        if let Some(session) = &source.unbound_session {
            self.bind(manager, &agent, session);
        }
        let now = chat_runtime::unix_millis();
        let candidates = match state.reply_reminder_candidates() {
            Ok(candidates) => candidates.candidates,
            Err(error) => {
                service_log(format_args!(
                    "agentctl: capture-miss check skipped: the request records are unreadable: \
                     {error}"
                ));
                return;
            }
        };
        let misses = find_misses(&candidates, &state.transcript_blocks(), now, self.grace);
        // A miss answered since, by `chat reply` or a later capture, is no longer one.
        self.told
            .retain(|key, _| misses.iter().any(|miss| miss.key == *key));
        let mut owner = Vec::new();
        for miss in &misses {
            if self.told.contains_key(&miss.key) {
                continue;
            }
            let line = format!(
                "the coordinator's transcript has held a reply block for reply ID {} (request \
                 {}) since {}, but the bridge captured no reply to it",
                miss.reply_id,
                miss.key.get(..12).unwrap_or(&miss.key),
                chat_runtime::utc_timestamp(miss.block_at_millis)
            );
            service_log(format_args!("agentctl: CAPTURE MISS: {line}"));
            owner.push(format!(
                "[chat-bridge WARNING] {agent} wrote a reply to your message (reply ID {}) {} min \
                 ago, but the bridge did not capture it, so it was not posted. {agent} has been \
                 asked to send it again.",
                miss.reply_id,
                now.saturating_sub(miss.block_at_millis) / 60_000
            ));
            self.told.insert(
                miss.key.clone(),
                Told {
                    miss: miss.clone(),
                    coordinator: false,
                    tried_at_millis: None,
                    attempts: 0,
                },
            );
        }
        post_owner_notices(owner, state, transport, "capture-miss warning");
        self.tell_coordinator(state, manager, stop, delivery, now);
        self.record(state, &source, now);
    }

    /// Bind `session` into `agent`'s record, logging a failure once until it changes.
    fn bind<A: ManagedApi + ?Sized>(
        &mut self,
        manager: &ManagedAgents<'_, A>,
        agent: &str,
        session: &str,
    ) {
        match manager.bind_discovered_session(agent, session) {
            Ok(true) => {
                self.bind_problem = None;
                service_log(format_args!(
                    "agentctl: bound conversation {session}, which the process in {agent}'s pane \
                     names, into its record, which named none"
                ));
            }
            Ok(false) => self.bind_problem = None,
            Err(error) => {
                let problem = error.to_string();
                if self.bind_problem.as_deref() != Some(problem.as_str()) {
                    service_log(format_args!(
                        "agentctl: conversation {session} of {agent} is not bound yet; a later \
                         scan tries again: {problem}"
                    ));
                    self.bind_problem = Some(problem);
                }
            }
        }
    }

    /// Give the coordinator each notice it has not been given, as a reply reminder is typed.
    fn tell_coordinator<A: ManagedApi + ?Sized>(
        &mut self,
        state: &BridgeState,
        manager: &ManagedAgents<'_, A>,
        stop: &StopState,
        delivery: DrainOptions,
        now: u64,
    ) {
        let retry = u64::try_from(NOTICE_RETRY.as_millis()).unwrap_or(u64::MAX);
        for told in self.told.values_mut() {
            if told.coordinator
                || told
                    .tried_at_millis
                    .is_some_and(|at| now.saturating_sub(at) < retry)
                || stop.is_stopped()
            {
                continue;
            }
            told.tried_at_millis = Some(now);
            told.attempts += 1;
            let notice = PendingReminder {
                message_id: format!(
                    "{NOTICE_MESSAGE_PREFIX}{}-{}",
                    told.miss.key.get(..16).unwrap_or(&told.miss.key),
                    told.attempts
                ),
                keys: vec![told.miss.key.clone()],
                prompt: coordinator_notice(state, &told.miss, now),
            };
            let notes = TypingNotes::default();
            let outcome = chat_runtime::deliver_reminder(
                state,
                &TypingWatch {
                    delivery: &CancellableDelivery {
                        manager,
                        runtime: StopRuntime::new(stop),
                    },
                    notes: &notes,
                    state,
                    receipts: stop.ack_queue.as_ref(),
                },
                &notice,
                delivery,
            );
            told.coordinator = !matches!(outcome, ReminderOutcome::Withdrawn(_));
            service_log(format_args!(
                "agentctl: capture-miss notice for reply ID {} {}",
                told.miss.reply_id,
                if told.coordinator {
                    outcome.describe()
                } else {
                    format!("{}; tried again in {}s", outcome.describe(), retry / 1000)
                }
            ));
        }
    }

    /// Write `reply-source.json` when what it says changed.
    fn record(&mut self, state: &BridgeState, source: &ReplySource, now: u64) {
        let misses: Vec<Value> = self
            .told
            .values()
            .map(|told| {
                json!({
                    "key": told.miss.key,
                    "reply_id": told.miss.reply_id,
                    "block_at_millis": told.miss.block_at_millis,
                    "coordinator_told": told.coordinator,
                })
            })
            .collect();
        let document = json!({
            "schema": "agentctl-chat-reply-source/v1",
            "agent": state.config().agent_name,
            "source": source,
            "capture_misses": misses,
        });
        if self.written.as_ref() == Some(&document) {
            return;
        }
        let mut written = document.clone();
        written["written_at_millis"] = json!(now);
        state.note_reply_source(&written);
        self.written = Some(document);
    }
}

/// The notice that tells the coordinator its reply to `miss` was never captured.
fn coordinator_notice(state: &BridgeState, miss: &Miss, now: u64) -> String {
    let send = state
        .capture_miss_reply_command(&miss.key, &miss.reply_id)
        .map_or_else(
            || "with `agentctl chat reply`, which `agentctl chat help` describes".to_owned(),
            |command| format!("with `{command}`, after writing the reply's text to that file"),
        );
    format!(
        "[chat-bridge WARNING] The chat bridge did not capture your reply to the chat request with \
reply ID {}: your transcript holds the reply you printed {} min ago, but the bridge stored \
nothing for it and posted nothing. Send that reply again {send}. If you already sent it another \
way, ignore this notice; it is sent only once.",
        miss.reply_id,
        now.saturating_sub(miss.block_at_millis) / 60_000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(key: &str, reply_id: &str, delivered: u64) -> ReminderCandidate {
        ReminderCandidate {
            key: key.to_owned(),
            reply_id: reply_id.to_owned(),
            delivered_at_millis: delivered,
        }
    }

    fn block(identifier: &str, at: Option<u64>) -> TranscriptBlock {
        TranscriptBlock {
            identifier: identifier.to_owned(),
            body: "text".to_owned(),
            at_millis: at,
        }
    }

    #[test]
    fn a_block_held_past_the_grace_for_an_unanswered_request_is_a_miss() {
        let minute = 60_000;
        let candidates = [
            candidate("a", "31", 10 * minute),
            candidate("b", "32", 10 * minute),
            candidate("c", "33", 10 * minute),
            candidate("d", "34", 10 * minute),
        ];
        let blocks = [
            // Written 3 minutes ago: a miss, dated by its first block.
            block("31", Some(12 * minute)),
            block("31", Some(13 * minute)),
            // Written 1 minute ago: the capture may still take it.
            block("32", Some(14 * minute)),
            // Written before the request was delivered: an earlier request's ID.
            block("33", Some(5 * minute)),
            // No time: never judged.
            block("34", None),
            // A block for a request already answered is not a candidate at all.
            block("99", Some(minute)),
        ];
        assert_eq!(
            find_misses(&candidates, &blocks, 15 * minute, CAPTURE_MISS_GRACE),
            [Miss {
                key: "a".into(),
                reply_id: "31".into(),
                block_at_millis: 12 * minute,
            }]
        );
        assert!(find_misses(&candidates, &[], 15 * minute, CAPTURE_MISS_GRACE).is_empty());
    }
}
