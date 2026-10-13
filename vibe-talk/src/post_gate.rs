//! A voice agent may PROPOSE a post; only the person it speaks for may send it.
//!
//! `#34 voice-chat-write-confirm`. `post_reply` has always been marked as needing approval, and its
//! description has always told the model to read the text back and get a spoken yes. Nothing
//! outside the model enforced either. Over MCP a write-scope caller's `post_reply` posted on the
//! spot, so whether the owner's bot spoke was decided by the model alone — which is why no voice
//! bridge was trusted with the write credential.
//!
//! This module is the enforcement, and it lives in the application so every provider inherits it:
//!
//! 1. **Proposing does not post.** [`PostGate::propose`] records the exact text, the channel and
//!    the reply target, and hands back a short-lived, single-use handle. The model is told plainly
//!    that nothing was sent.
//! 2. **Committing needs something the model cannot produce.** The commit is an application route
//!    (`POST /api/v1/post-proposals/commit`), never a tool, so no model can call it: the voice page
//!    calls it when the owner taps Send, or a bridge calls it after attributing a separate, final
//!    user turn to the speaker. A tool that reached it would undo the whole gate, which is why the
//!    manifest test forbids one.
//! 3. **A handle is bound to what was proposed.** The commit restates the channel, the text and the
//!    reply target, and any difference refuses it. So does expiry, reuse, cancellation, or a newer
//!    proposal, and EVERY attempt spends the handle — a refused commit cannot be retried into a
//!    success by guessing what changed.
//! 4. **Nothing said is logged.** [`crate::access::post_gate`] records the proposal's serial, the
//!    channel, the text's length and the outcome, never the words.
//!
//! # Unless the owner turns it off
//!
//! `#264 voice-send-approval`. All of the above is what `post_reply` does while "Ask me before the
//! voice agent sends" is on, which is the shipped default. The owner may turn it off in Settings,
//! and then `post_reply` posts at once and nothing here is involved; see [`crate::send_approval`].
//! The switch is his alone — a write-scope route Settings calls, never a tool — so the rule above
//! still holds in the form that matters: nothing a model can call decides whether it is asked.
//!
//! # Why one pending proposal, not a queue
//!
//! A voice conversation proposes one message, reads it back, and waits. A second proposal means
//! the draft changed — "no, say it like this" — and the old one is exactly what must not be sent
//! by a tap that arrives late. So a new proposal SUPERSEDES the pending one rather than queueing
//! beside it, and the screen only ever offers the draft the conversation is currently on.
//!
//! # Why the handle is a capability anyway
//!
//! Every route here already needs the write-scope token, so the handle is not what keeps a
//! stranger out. It is what binds a tap to ONE proposal: 128 bits from the system CSPRNG, carried
//! in request bodies rather than paths so the access log's path field never holds it.
//!
//! # Every send is remembered for a while
//!
//! `#270 voice-transcript-markers`. The voice page draws every message the voice agent posted as a
//! card in the transcript, and the commit's own answer is not enough to do that: the answer can be
//! lost, a send can finish after the owner hung up, and a direct send has no commit at all. So the
//! gate also keeps a short SEND MEMORY: each send is entered when it starts — at [`PostGate::take`]
//! for a commit, at [`PostGate::begin_direct`] for a direct send — and, once it settles, is
//! numbered in the order sends settled (`send_seq`) and published on the proposal long poll. A send
//! abandoned on its way is filed as unconfirmed, as a direct send's repeat memory already did. The
//! memory lives [`SEND_MEMORY_TTL`] after a send settles, holds at most [`SEND_MEMORY`], and is
//! never written to disk; forgetting the stored conversations empties it.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;

use crate::store::{SendHow, SendOrigin, SendOutcome, SentRecord, MAX_TURN_CHARS};

/// How long a proposal can be confirmed.
///
/// Long enough to hear the read-back and tap Send, or answer it aloud; short enough that a
/// forgotten draft cannot be sent by a tap minutes later, after the conversation has moved on.
pub const PROPOSAL_TTL: Duration = Duration::from_secs(120);

/// How many spent handles are remembered, so a second commit is refused as `proposal_used`
/// rather than as a handle that never existed. Bounded because nothing else bounds it.
const SPENT_MEMORY: usize = 64;

/// How long after it ends a post the voice agent sent at once keeps the same words to the same
/// place from being posted again. `#264 voice-send-approval`.
///
/// A hosted tool call can time out, or be retried by its transport, after the post has landed, and
/// an owner told "not sent" says "try again". A commit cannot be repeated — it spends a single-use
/// handle — but a direct send has no handle, so this is what stands in for one. A minute covers a
/// retry and an owner's "try again"; it is short enough that saying the same thing again on
/// purpose, later, is not refused.
pub const DIRECT_REPEAT_WINDOW: Duration = Duration::from_secs(60);

/// How many direct sends are remembered at once. Bounded because nothing else bounds it.
const DIRECT_MEMORY: usize = 16;

/// How long a settled send stays in the send memory, for the voice page to draw and record.
/// `#270 voice-transcript-markers`.
///
/// Long enough for a page whose call dropped to come back, read what was sent meanwhile, and file
/// it in its transcript; short enough that the memory holds a quarter of an hour of messages that
/// are already in their channels, and nothing older.
pub const SEND_MEMORY_TTL: Duration = Duration::from_secs(15 * 60);

/// How many sends the send memory holds at once, the oldest leaving first.
pub const SEND_MEMORY: usize = 32;

/// What a proposal is bound to. A commit must restate all three exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    /// The channel id, already resolved from any name or alias the model used.
    pub channel_id: String,
    /// The exact text that would be posted.
    pub text: String,
    /// The message this answers, if any.
    pub reply_to: Option<String>,
}

/// One proposal waiting for confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    /// A small public number for correlating log lines and screens. Not a capability.
    pub serial: u64,
    /// The single-use capability a commit must present.
    pub handle: String,
    /// What was proposed.
    pub binding: Binding,
    /// The start of `binding.text` that this server put there — the owner's tag for what the voice
    /// agent posts (`#303 voice-post-prefix`) — or empty when it added nothing. Not part of the
    /// binding: a commit restates the whole text, tag and all, and is never tagged again.
    pub prefix: String,
    /// When it was made.
    pub created: Instant,
    /// How long it lives.
    pub ttl: Duration,
}

impl Proposal {
    /// Whole milliseconds left before it expires, as of `now`.
    #[must_use]
    pub fn expires_in_ms(&self, now: Instant) -> u64 {
        let left = self
            .ttl
            .saturating_sub(now.saturating_duration_since(self.created));
        u64::try_from(left.as_millis()).unwrap_or(u64::MAX)
    }

    fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.created) >= self.ttl
    }
}

/// What became of a proposal once it stopped being pending, as the long poll reports it.
/// `#270 voice-transcript-markers`.
///
/// Finer than [`Refusal`], which only has to tell a second commit why it is refused: the voice page
/// says "the draft lapsed" only for a proposal that really did, and nothing for one a newer draft
/// replaced or one that was sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposalFate {
    /// A commit was accepted for it. Whether the post then went is the send memory's to say.
    Committed,
    /// It was withdrawn.
    Cancelled,
    /// It outlived [`PROPOSAL_TTL`].
    Expired,
    /// A newer proposal replaced it.
    Superseded,
    /// A commit restated something else, which spent it.
    Refused,
}

impl ProposalFate {
    /// What a later commit of the same handle is refused with.
    fn refusal(self) -> Refusal {
        match self {
            Self::Expired => Refusal::Expired,
            Self::Superseded => Refusal::Superseded,
            Self::Committed | Self::Cancelled | Self::Refused => Refusal::Used,
        }
    }
}

/// Why a commit or a cancel was refused. Each is a machine code a client branches on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No proposal was ever made with this handle, or it is too old to be remembered.
    Unknown,
    /// The proposal outlived [`PROPOSAL_TTL`].
    Expired,
    /// The handle was already committed, refused, or cancelled.
    Used,
    /// A newer proposal replaced it before it was confirmed.
    Superseded,
    /// The commit restated a different channel, text, or reply target.
    Mismatch,
}

impl Refusal {
    /// The code a response and a log line carry.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Unknown => "proposal_unknown",
            Self::Expired => "proposal_expired",
            Self::Used => "proposal_used",
            Self::Superseded => "proposal_superseded",
            Self::Mismatch => "proposal_mismatch",
        }
    }

    /// One sentence for the person who tapped.
    #[must_use]
    pub fn sentence(self) -> &'static str {
        match self {
            Self::Unknown => "this server has no such proposal, so nothing was sent",
            Self::Expired => "the proposal expired before it was confirmed, so nothing was sent",
            Self::Used => {
                "that proposal was already confirmed or cancelled; it cannot be sent again"
            }
            Self::Superseded => "a newer draft replaced this one, so nothing was sent",
            Self::Mismatch => {
                "what was confirmed differs from what was proposed, so nothing was sent"
            }
        }
    }
}

#[derive(Debug, Default)]
struct Inner {
    pending: Option<Proposal>,
    /// Recently spent handles, their serials and their fates, newest last.
    spent: VecDeque<Spent>,
    next_serial: u64,
    /// Direct sends on their way, or ended within [`DIRECT_REPEAT_WINDOW`] having posted something
    /// or not knowing whether they did, oldest first.
    direct: VecDeque<DirectSend>,
    /// The send memory, oldest first. `#270 voice-transcript-markers`.
    sends: VecDeque<RecentSend>,
    /// The last `send_seq` handed out. Never reset, not even by a purge, so a page's cursor never
    /// points at a number that will be handed out again in the same process. It starts at the
    /// server's clock in milliseconds ([`PostGate::with_ttl`]), so it does not start again after a
    /// restart either: a page that read the last process's numbers still reads every send of this
    /// one, because each is numbered above them.
    send_seq: u64,
}

#[derive(Debug)]
struct Spent {
    handle: String,
    serial: u64,
    fate: ProposalFate,
}

/// One send in the send memory.
#[derive(Debug)]
struct RecentSend {
    serial: u64,
    binding: Binding,
    how: SendHow,
    origin: SendOrigin,
    channel_name: String,
    begun_at_ms: i64,
    /// `None` while it is on its way.
    settled: Option<SettledSend>,
    /// The conversation a transcript claim filed it into, once one has.
    claimed: Option<String>,
}

/// Why a transcript claim of a send was refused. `#270 voice-transcript-markers`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimRefusal {
    /// The memory does not hold it: this server restarted, it settled more than
    /// [`SEND_MEMORY_TTL`] ago, it was forgotten, or it never existed.
    Unknown,
    /// It is already filed in a transcript, this conversation's or another's.
    Recorded,
}

#[derive(Debug)]
struct SettledSend {
    at: Instant,
    send: VoiceSendRecord,
}

/// One settled send, as the long poll publishes it and a transcript claim files it. `#270
/// voice-transcript-markers`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceSendRecord {
    /// What a transcript row holds: the text that went, capped at [`MAX_TURN_CHARS`].
    pub text: String,
    /// Everything else, as the row's detail records it.
    pub record: SentRecord,
}

/// What a finished send posted, for the send memory to file. Built by the caller, which knows the
/// channel's name, what the reply answered and where the parts went; see
/// [`crate::send_approval::send_report`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendReport {
    /// What is known to have been posted.
    pub outcome: SendOutcome,
    /// The text that went: all of it for a sent or unconfirmed post, the parts known to have been
    /// posted for a partial one. Capped when filed.
    pub text: String,
    /// The first part as the chat service accepted it, when one was.
    pub message_id: Option<String>,
    /// The thread the post went to, when not the conversation itself.
    pub thread_id: Option<String>,
    /// That thread's name, when known.
    pub thread_name: Option<String>,
    /// Who wrote the message it answered, when known.
    pub reply_to_author: Option<String>,
    /// The first words of the message it answered, when known.
    pub reply_to_excerpt: Option<String>,
    /// How many parts are known to have been posted.
    pub parts_posted: u32,
    /// How many parts the message was cut into; 0 when nobody knows.
    pub parts_total: u32,
    /// The part, counted from 1, that may have been posted although its answer was lost.
    pub uncertain_part: Option<u32>,
    /// How many characters were certainly not posted.
    pub unsent_chars: u32,
}

impl SendReport {
    /// A send nobody can say anything about: abandoned on its way, or answered with nothing this
    /// server could read. The words may be in the channel, so they are all filed.
    #[must_use]
    pub fn unconfirmed(text: &str) -> Self {
        Self {
            outcome: SendOutcome::Unconfirmed,
            text: text.to_owned(),
            message_id: None,
            thread_id: None,
            thread_name: None,
            reply_to_author: None,
            reply_to_excerpt: None,
            parts_posted: 0,
            parts_total: 0,
            uncertain_part: None,
            unsent_chars: 0,
        }
    }

    /// What a direct send's repeat memory makes of this report.
    fn direct_outcome(&self) -> DirectOutcome {
        match self.outcome {
            SendOutcome::Sent => DirectOutcome::Sent,
            SendOutcome::Partial if self.uncertain_part.is_some() => {
                DirectOutcome::PartlyUnconfirmed
            }
            SendOutcome::Partial => DirectOutcome::Partly,
            SendOutcome::Unconfirmed => DirectOutcome::Unconfirmed,
        }
    }
}

/// A send in the send memory, on its way. Settle it with what it posted, or forget it when it
/// certainly posted nothing. `#270 voice-transcript-markers`.
///
/// Dropped unsettled — the request abandoned while the post was on the wire — it is filed as
/// unconfirmed, because that is exactly what it is: the page still draws a card saying the channel
/// may have it.
#[derive(Debug)]
#[must_use = "a send that is never settled is filed as unconfirmed"]
pub struct SendTicket<'a> {
    gate: &'a PostGate,
    serial: u64,
    done: bool,
}

impl SendTicket<'_> {
    /// The send's number: the proposal's, or the direct send's.
    #[must_use]
    pub fn serial(&self) -> u64 {
        self.serial
    }

    /// File what was posted, and return the `send_seq` it was published under — `None` when the
    /// memory was emptied while it was on its way.
    pub fn settle(mut self, report: SendReport) -> Option<u64> {
        self.done = true;
        self.gate.settle_send(self.serial, report)
    }

    /// Certainly nothing was posted: drop the send from the memory, publishing nothing.
    pub fn forget(mut self) {
        self.done = true;
        self.gate.forget_send(self.serial);
    }
}

impl Drop for SendTicket<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.gate.abandon_send(self.serial);
        }
    }
}

/// What became of a post the voice agent sent at once, as far as a repeat of it needs to know.
/// `#264 voice-send-approval`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectOutcome {
    /// Still on its way to the chat service.
    InFlight,
    /// Posted, all of it.
    Sent,
    /// Some of its parts were posted, and the rest were not.
    Partly,
    /// Some of its parts were posted, and whether the rest were could not be confirmed: the answer
    /// for the part that failed was lost, so it may be in the channel too.
    PartlyUnconfirmed,
    /// Nobody knows: the chat service's answer was lost, or the request was abandoned while the
    /// post was on its way.
    Unconfirmed,
}

impl DirectOutcome {
    /// The word a log line carries.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InFlight => "in_flight",
            Self::Sent => "sent",
            Self::Partly => "partly",
            Self::PartlyUnconfirmed => "partly_unconfirmed",
            Self::Unconfirmed => "unconfirmed",
        }
    }
}

/// An earlier direct send of the same words to the same place, which a new one is not posted over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Repeat {
    /// The earlier send's number.
    pub serial: u64,
    /// What became of it.
    pub outcome: DirectOutcome,
    /// How long ago it began.
    pub age: Duration,
    /// How much longer it keeps the same words from being posted again: the whole window while it
    /// is still on its way.
    pub left: Duration,
}

#[derive(Debug)]
struct DirectSend {
    serial: u64,
    binding: Binding,
    outcome: DirectOutcome,
    started: Instant,
    /// When it ended; `None` while it is on its way.
    ended: Option<Instant>,
}

impl DirectSend {
    fn lapsed(&self, now: Instant) -> bool {
        self.ended
            .is_some_and(|ended| now.saturating_duration_since(ended) >= DIRECT_REPEAT_WINDOW)
    }
}

/// A direct send on its way, from [`PostGate::begin_direct`]. Finish it with what happened.
///
/// Dropped unfinished — the request abandoned while the post was on the wire, as a client that
/// hangs up mid-call does — it is remembered as [`DirectOutcome::Unconfirmed`], because that is
/// exactly what it is, and the send memory files it as unconfirmed too.
#[derive(Debug)]
#[must_use = "a direct send that is never finished is remembered as unconfirmed"]
pub struct DirectTicket<'a> {
    send: SendTicket<'a>,
    finished: bool,
}

impl DirectTicket<'_> {
    /// The number this send's log lines carry.
    #[must_use]
    pub fn serial(&self) -> u64 {
        self.send.serial
    }

    /// Record what happened: `Some` when something was posted or may have been, `None` when
    /// certainly nothing was — which forgets the send, so the same words may go again at once, and
    /// publishes nothing. Returns the `send_seq` a report was published under.
    pub fn finish(mut self, report: Option<SendReport>) -> Option<u64> {
        self.finished = true;
        let gate = self.send.gate;
        let serial = self.send.serial;
        gate.end_direct(serial, report.as_ref().map(SendReport::direct_outcome));
        // Moved out, so the ticket's own drop does not run for it.
        let send = std::mem::replace(
            &mut self.send,
            SendTicket {
                gate,
                serial,
                done: true,
            },
        );
        match report {
            Some(report) => send.settle(report),
            None => {
                send.forget();
                None
            }
        }
    }
}

impl Drop for DirectTicket<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.send
                .gate
                .end_direct(self.send.serial, Some(DirectOutcome::Unconfirmed));
        }
    }
}

impl Inner {
    fn spend(&mut self, handle: String, serial: u64, fate: ProposalFate) {
        if self.spent.len() >= SPENT_MEMORY {
            self.spent.pop_front();
        }
        self.spent.push_back(Spent {
            handle,
            serial,
            fate,
        });
    }

    /// Move an expired pending proposal into the spent list, and say whether one was.
    fn expire(&mut self, now: Instant) -> Option<Proposal> {
        if self.pending.as_ref().is_some_and(|p| p.expired(now)) {
            let lapsed = self.pending.take()?;
            self.spend(lapsed.handle.clone(), lapsed.serial, ProposalFate::Expired);
            return Some(lapsed);
        }
        None
    }

    fn refusal_for(&self, handle: &str) -> Refusal {
        self.spent
            .iter()
            .rev()
            .find(|spent| spent.handle == handle)
            .map_or(Refusal::Unknown, |spent| spent.fate.refusal())
    }

    /// Enter a send in the memory as on its way.
    fn enter_send(
        &mut self,
        serial: u64,
        binding: Binding,
        how: SendHow,
        origin: SendOrigin,
        channel_name: String,
    ) {
        self.prune_sends(Instant::now());
        if self.sends.len() >= SEND_MEMORY {
            self.sends.pop_front();
        }
        self.sends.push_back(RecentSend {
            serial,
            binding,
            how,
            origin,
            channel_name,
            begun_at_ms: crate::store::now_ms(),
            settled: None,
            claimed: None,
        });
    }

    /// Drop the sends that settled more than [`SEND_MEMORY_TTL`] ago.
    fn prune_sends(&mut self, now: Instant) {
        self.sends.retain(|send| {
            send.settled
                .as_ref()
                .is_none_or(|settled| now.saturating_duration_since(settled.at) < SEND_MEMORY_TTL)
        });
    }
}

/// The server's one pending post proposal, and the rules for spending it.
#[derive(Debug)]
pub struct PostGate {
    inner: Mutex<Inner>,
    ttl: Duration,
    /// Bumped on every change, so a long poll wakes without polling.
    changed: watch::Sender<u64>,
    /// Which run of the server this gate belongs to. `#270 voice-transcript-markers`.
    instance: String,
}

impl Default for PostGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Why [`PostGate::take`] posted nothing: the refusal, and the proposal when there was one, so the
/// log line can name its serial and channel. Boxed, because the refusal is the common case and
/// should not carry a whole proposal's width on the stack.
pub type TakeRefused = (Refusal, Option<Box<Proposal>>);

/// What happened to a proposal that was replaced or lapsed on the way past, for the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Displaced {
    /// The proposal that stopped being confirmable.
    pub proposal: Proposal,
    /// Why.
    pub why: Refusal,
}

impl PostGate {
    /// A gate whose proposals live for [`PROPOSAL_TTL`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_ttl(PROPOSAL_TTL)
    }

    /// A gate with a different lifetime. For tests, which should not wait two minutes.
    #[must_use]
    pub fn with_ttl(ttl: Duration) -> Self {
        Self::with_ttl_and_numbering(ttl, u64::try_from(crate::store::now_ms()).unwrap_or(0))
    }

    /// A gate whose send memory numbers from `send_seq_base`: the first send settled is one above
    /// it. For tests, which want small numbers; [`PostGate::with_ttl`] starts from the clock.
    #[must_use]
    pub fn with_ttl_and_numbering(ttl: Duration, send_seq_base: u64) -> Self {
        let (changed, _) = watch::channel(0);
        Self {
            inner: Mutex::new(Inner {
                send_seq: send_seq_base,
                ..Inner::default()
            }),
            ttl,
            changed,
            instance: fresh_instance(),
        }
    }

    /// Which run of the server this gate belongs to: a token minted when the gate was made, so a
    /// new one for every start of the process. `#270 voice-transcript-markers`.
    ///
    /// Proposal serials start again at one in every run, so a serial alone does not name a
    /// proposal to a page that stays open across a restart: it has drawn a card for "serial 3"
    /// already, and the next run's serial 3 is another message, maybe in another conversation.
    /// The pair does. Not a secret and not a capability — it only tells runs apart.
    #[must_use]
    pub fn instance(&self) -> &str {
        &self.instance
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A poisoned lock means a panic while holding it; the state inside is still a valid
        // Option and list, and refusing every post forever would be worse than carrying on.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn notify(&self) {
        self.changed.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// File a settled send under the next `send_seq`, wake the long polls, and return the number.
    /// `None` when it is no longer in the memory: the memory was emptied while it was on its way.
    fn settle_send(&self, serial: u64, report: SendReport) -> Option<u64> {
        let now = Instant::now();
        let mut inner = self.lock();
        inner.prune_sends(now);
        let at = inner
            .sends
            .iter()
            .position(|send| send.serial == serial && send.settled.is_none())?;
        inner.send_seq += 1;
        let send_seq = inner.send_seq;
        let entry = inner.sends.get_mut(at)?;
        let text_chars = report.text.chars().count();
        let text: String = report.text.chars().take(MAX_TURN_CHARS).collect();
        let record = SentRecord {
            serial,
            send_seq,
            channel_id: entry.binding.channel_id.clone(),
            channel_name: entry.channel_name.clone(),
            reply_to: entry.binding.reply_to.clone(),
            reply_to_author: report.reply_to_author,
            reply_to_excerpt: report.reply_to_excerpt,
            thread_id: report.thread_id,
            thread_name: report.thread_name,
            message_id: report.message_id,
            parts_posted: report.parts_posted,
            parts_total: report.parts_total,
            uncertain_part: report.uncertain_part,
            unsent_chars: report.unsent_chars,
            outcome: report.outcome,
            how: entry.how,
            origin: entry.origin,
            text_truncated: text_chars > MAX_TURN_CHARS,
            text_chars: u32::try_from(text_chars).unwrap_or(u32::MAX),
            begun_at_ms: entry.begun_at_ms,
            sent_at_ms: crate::store::now_ms(),
        };
        entry.settled = Some(SettledSend {
            at: now,
            send: VoiceSendRecord { text, record },
        });
        drop(inner);
        self.notify();
        Some(send_seq)
    }

    fn forget_send(&self, serial: u64) {
        self.lock()
            .sends
            .retain(|send| send.serial != serial || send.settled.is_some());
    }

    /// A send dropped unsettled: filed as unconfirmed, with every word it was asked to post.
    fn abandon_send(&self, serial: u64) {
        let text = self
            .lock()
            .sends
            .iter()
            .find(|send| send.serial == serial && send.settled.is_none())
            .map(|send| send.binding.text.clone());
        if let Some(text) = text {
            self.settle_send(serial, SendReport::unconfirmed(&text));
        }
    }

    /// The settled sends published after `after`, oldest first, and the newest `send_seq` handed
    /// out so far. `#270 voice-transcript-markers`.
    ///
    /// A cursor AHEAD of that number belongs to an earlier process — this server restarted since
    /// the page last read — and is answered as if it were zero, so the page catches up with the
    /// sends of this process rather than waiting for the counter to pass its old number.
    #[must_use]
    pub fn sends_after(&self, after: u64) -> (Vec<VoiceSendRecord>, u64) {
        let mut inner = self.lock();
        inner.prune_sends(Instant::now());
        let through = inner.send_seq;
        let after = if after > through { 0 } else { after };
        let mut sends: Vec<VoiceSendRecord> = inner
            .sends
            .iter()
            .filter_map(|send| send.settled.as_ref())
            .filter(|settled| settled.send.record.send_seq > after)
            .map(|settled| settled.send.clone())
            .collect();
        sends.sort_by_key(|send| send.record.send_seq);
        (sends, through)
    }

    /// One settled send, by its `send_seq`, while the memory holds it.
    #[must_use]
    pub fn settled_send(&self, send_seq: u64) -> Option<VoiceSendRecord> {
        let mut inner = self.lock();
        inner.prune_sends(Instant::now());
        inner
            .sends
            .iter()
            .filter_map(|send| send.settled.as_ref())
            .find(|settled| settled.send.record.send_seq == send_seq)
            .map(|settled| settled.send.clone())
    }

    /// Mark the settled send `send_seq` as filed in `conversation`'s transcript, and hand back
    /// what to file — once. `#270 voice-transcript-markers`.
    ///
    /// # Errors
    ///
    /// [`ClaimRefusal::Unknown`] when the memory does not hold it, [`ClaimRefusal::Recorded`] when
    /// a claim already took it.
    pub fn claim_send(
        &self,
        send_seq: u64,
        conversation: &str,
    ) -> Result<VoiceSendRecord, ClaimRefusal> {
        let mut inner = self.lock();
        inner.prune_sends(Instant::now());
        let send = inner
            .sends
            .iter_mut()
            .find(|send| {
                send.settled
                    .as_ref()
                    .is_some_and(|settled| settled.send.record.send_seq == send_seq)
            })
            .ok_or(ClaimRefusal::Unknown)?;
        if send.claimed.is_some() {
            return Err(ClaimRefusal::Recorded);
        }
        send.claimed = Some(conversation.to_owned());
        send.settled
            .as_ref()
            .map(|settled| settled.send.clone())
            .ok_or(ClaimRefusal::Unknown)
    }

    /// Undo [`PostGate::claim_send`]: the transcript could not take the row, so a later claim may.
    pub fn unclaim_send(&self, send_seq: u64) {
        let mut inner = self.lock();
        if let Some(send) = inner.sends.iter_mut().find(|send| {
            send.settled
                .as_ref()
                .is_some_and(|settled| settled.send.record.send_seq == send_seq)
        }) {
            send.claimed = None;
        }
    }

    /// Forget the sends filed into `conversation`: the owner forgot that conversation, and its
    /// sends are part of what he forgot.
    pub fn forget_sends_claimed_into(&self, conversation: &str) {
        self.lock()
            .sends
            .retain(|send| send.claimed.as_deref() != Some(conversation));
    }

    /// The newest `send_seq` handed out so far.
    #[must_use]
    pub fn sends_through(&self) -> u64 {
        self.lock().send_seq
    }

    /// What became of the proposals this gate still remembers, oldest first.
    #[must_use]
    pub fn fates(&self) -> Vec<(u64, ProposalFate)> {
        self.lock()
            .spent
            .iter()
            .map(|spent| (spent.serial, spent.fate))
            .collect()
    }

    /// Empty the send memory: the owner forgot his stored conversations, or purged the store. A
    /// send still on its way is forgotten too, and publishes nothing when it settles.
    pub fn forget_sends(&self) {
        self.lock().sends.clear();
        self.notify();
    }

    /// Record a proposal, replacing any pending one, and return it with whatever it displaced.
    pub fn propose(&self, binding: Binding) -> (Proposal, Option<Displaced>) {
        self.propose_tagged(binding, String::new())
    }

    /// [`Self::propose`], for a text this server tagged: `prefix` is the start of `binding.text` it
    /// put there. `#303 voice-post-prefix`.
    pub fn propose_tagged(
        &self,
        binding: Binding,
        prefix: String,
    ) -> (Proposal, Option<Displaced>) {
        debug_assert!(binding.text.starts_with(&prefix));
        let now = Instant::now();
        let mut inner = self.lock();
        let displaced = match inner.expire(now) {
            Some(lapsed) => Some(Displaced {
                proposal: lapsed,
                why: Refusal::Expired,
            }),
            None => inner.pending.take().map(|old| Displaced {
                proposal: old,
                why: Refusal::Superseded,
            }),
        };
        if let Some(Displaced { proposal, why }) = &displaced {
            if *why == Refusal::Superseded {
                inner.spend(
                    proposal.handle.clone(),
                    proposal.serial,
                    ProposalFate::Superseded,
                );
            }
        }
        inner.next_serial += 1;
        let proposal = Proposal {
            serial: inner.next_serial,
            handle: fresh_handle(),
            binding,
            prefix,
            created: now,
            ttl: self.ttl,
        };
        inner.pending = Some(proposal.clone());
        drop(inner);
        self.notify();
        (proposal, displaced)
    }

    /// Number a post the voice agent is about to send at once, and remember it as on its way.
    /// `#264 voice-send-approval`.
    ///
    /// Unless the same words to the same place — the whole [`Binding`] — are already on their way,
    /// or ended within [`DIRECT_REPEAT_WINDOW`] posted, partly posted or unconfirmed. Then nothing
    /// is numbered or remembered, and the earlier send is returned for the caller to answer with
    /// instead of posting. A send that certainly posted nothing is forgotten when it ends, so it
    /// never stands in the way of trying again.
    ///
    /// The number comes from the proposals' counter, so the access log's `proposal=` numbers stay
    /// one sequence; nothing pending is touched and no watcher is woken, because nothing a watcher
    /// shows changed. The send is entered in the send memory as on its way, with `origin` saying
    /// which door it came through and `channel_name` how the owner knows the channel; watchers are
    /// woken when it settles.
    ///
    /// # Errors
    ///
    /// The earlier send, as a [`Repeat`].
    pub fn begin_direct(
        &self,
        binding: Binding,
        origin: SendOrigin,
        channel_name: String,
    ) -> Result<DirectTicket<'_>, Repeat> {
        let now = Instant::now();
        let mut inner = self.lock();
        inner.direct.retain(|send| !send.lapsed(now));
        if let Some(earlier) = inner.direct.iter().find(|send| send.binding == binding) {
            let left = earlier.ended.map_or(DIRECT_REPEAT_WINDOW, |ended| {
                DIRECT_REPEAT_WINDOW.saturating_sub(now.saturating_duration_since(ended))
            });
            return Err(Repeat {
                serial: earlier.serial,
                outcome: earlier.outcome,
                age: now.saturating_duration_since(earlier.started),
                left,
            });
        }
        if inner.direct.len() >= DIRECT_MEMORY {
            inner.direct.pop_front();
        }
        inner.next_serial += 1;
        let serial = inner.next_serial;
        inner.enter_send(
            serial,
            binding.clone(),
            SendHow::Direct,
            origin,
            channel_name,
        );
        inner.direct.push_back(DirectSend {
            serial,
            binding,
            outcome: DirectOutcome::InFlight,
            started: now,
            ended: None,
        });
        drop(inner);
        Ok(DirectTicket {
            send: SendTicket {
                gate: self,
                serial,
                done: false,
            },
            finished: false,
        })
    }

    fn end_direct(&self, serial: u64, outcome: Option<DirectOutcome>) {
        let now = Instant::now();
        let mut inner = self.lock();
        let Some(at) = inner.direct.iter().position(|send| send.serial == serial) else {
            return;
        };
        match outcome {
            None => {
                inner.direct.remove(at);
            }
            Some(outcome) => {
                if let Some(send) = inner.direct.get_mut(at) {
                    send.outcome = outcome;
                    send.ended = Some(now);
                }
            }
        }
    }

    /// The pending proposal, if one is still confirmable.
    #[must_use]
    pub fn pending(&self) -> Option<Proposal> {
        let mut inner = self.lock();
        if inner.expire(Instant::now()).is_some() {
            drop(inner);
            self.notify();
            return None;
        }
        inner.pending.clone()
    }

    /// Spend `handle` for a commit. Succeeds only when it names the pending, unexpired proposal
    /// AND `restated` matches its binding exactly. Every outcome except [`Refusal::Unknown`] and an
    /// already-spent handle consumes the proposal.
    ///
    /// On success the send is entered in the send memory as on its way, approved `how`, to the
    /// channel the owner knows as `channel_name`, and the ticket that settles it is handed back.
    ///
    /// # Errors
    ///
    /// The [`Refusal`] saying why nothing may be posted; the proposal, when there was one, so the
    /// log line can name its serial and channel. Boxed, because the refusal is the common case
    /// and should not carry a whole proposal's width on the stack.
    pub fn take(
        &self,
        handle: &str,
        restated: &Binding,
        how: SendHow,
        channel_name: String,
    ) -> Result<(Proposal, SendTicket<'_>), TakeRefused> {
        let now = Instant::now();
        let mut inner = self.lock();
        let lapsed = inner.expire(now);
        let names_pending = inner.pending.as_ref().is_some_and(|p| p.handle == handle);
        if !names_pending {
            let why = inner.refusal_for(handle);
            drop(inner);
            // Only an expiry noticed on the way changed anything a watcher could see; a stranger's
            // handle must not so much as wake one.
            if lapsed.is_some() {
                self.notify();
            }
            return Err((why, lapsed.filter(|p| p.handle == handle).map(Box::new)));
        }
        let Some(proposal) = inner.pending.take() else {
            return Err((Refusal::Unknown, None));
        };
        if proposal.binding != *restated {
            inner.spend(
                proposal.handle.clone(),
                proposal.serial,
                ProposalFate::Refused,
            );
            drop(inner);
            self.notify();
            return Err((Refusal::Mismatch, Some(Box::new(proposal))));
        }
        inner.spend(
            proposal.handle.clone(),
            proposal.serial,
            ProposalFate::Committed,
        );
        inner.enter_send(
            proposal.serial,
            proposal.binding.clone(),
            how,
            SendOrigin::Commit,
            channel_name,
        );
        drop(inner);
        self.notify();
        let ticket = SendTicket {
            gate: self,
            serial: proposal.serial,
            done: false,
        };
        Ok((proposal, ticket))
    }

    /// Withdraw the pending proposal named by `handle`.
    ///
    /// # Errors
    ///
    /// The [`Refusal`] saying why there was nothing to cancel.
    pub fn cancel(&self, handle: &str) -> Result<Proposal, Refusal> {
        let now = Instant::now();
        let mut inner = self.lock();
        let lapsed = inner.expire(now);
        if inner.pending.as_ref().is_none_or(|p| p.handle != handle) {
            let why = inner.refusal_for(handle);
            drop(inner);
            if lapsed.is_some() {
                self.notify();
            }
            return Err(why);
        }
        let proposal = inner.pending.take().ok_or(Refusal::Unknown)?;
        inner.spend(
            proposal.handle.clone(),
            proposal.serial,
            ProposalFate::Cancelled,
        );
        drop(inner);
        self.notify();
        Ok(proposal)
    }

    /// Wait until the pending proposal's serial differs from `seen`, or `wait` passes, and return
    /// whatever is pending then. `seen` of `None` means "nothing on screen yet".
    ///
    /// With `sent_after`, the wait also ends when the send memory has published past that
    /// `send_seq`, or when the cursor is ahead of anything this process handed out
    /// ([`PostGate::sends_after`]). Without it, a settled send wakes nothing, so a page that does
    /// not read the send memory waits exactly as it always did.
    pub async fn wait_for_change(
        &self,
        seen: Option<u64>,
        sent_after: Option<u64>,
        wait: Duration,
    ) -> Option<Proposal> {
        let mut changes = self.changed.subscribe();
        let deadline = Instant::now() + wait;
        loop {
            let current = self.pending();
            if current.as_ref().map(|p| p.serial) != seen {
                return current;
            }
            if sent_after.is_some_and(|after| self.sends_through() != after) {
                return current;
            }
            // Wake for a change, for the pending proposal's own expiry, or for the deadline —
            // whichever comes first. Expiry changes the answer without anybody calling in.
            let expiry = current
                .as_ref()
                .map_or(deadline, |p| (p.created + p.ttl).min(deadline));
            let woke = tokio::time::timeout_at(expiry, changes.changed()).await;
            if Instant::now() >= deadline {
                return self.pending();
            }
            if matches!(woke, Ok(Err(_))) {
                // The sender lives as long as the gate, so this is unreachable; returning is the
                // safe answer if it ever is not.
                return self.pending();
            }
        }
    }
}

/// A short random token naming one run of the server. Twelve bytes from the system CSPRNG would
/// be more than enough to never repeat; eight are, too, for a token that only has to differ from
/// the last few runs a page saw.
fn fresh_instance() -> String {
    use base64::Engine as _;
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("the system CSPRNG must be available to name a run");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn fresh_handle() -> String {
    use base64::Engine as _;
    let mut bytes = [0u8; 16];
    // Same rule as a speech ticket: without OS randomness the only alternative is a guessable
    // handle, and that is not a condition to paper over.
    getrandom::fill(&mut bytes).expect("the system CSPRNG must be available to mint a handle");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gate whose sends number from one, so a test can name them.
    fn numbered_from_zero() -> PostGate {
        PostGate::with_ttl_and_numbering(PROPOSAL_TTL, 0)
    }

    fn binding(text: &str) -> Binding {
        Binding {
            channel_id: "111".to_owned(),
            text: text.to_owned(),
            reply_to: None,
        }
    }

    /// [`PostGate::take`] as the tests before the send memory knew it: the proposal, with the send
    /// forgotten so it publishes nothing and wakes nobody.
    fn take(
        gate: &PostGate,
        handle: &str,
        restated: &Binding,
    ) -> Result<Proposal, (Refusal, Option<Box<Proposal>>)> {
        gate.take(handle, restated, SendHow::Tap, "lead team".to_owned())
            .map(|(proposal, ticket)| {
                ticket.forget();
                proposal
            })
    }

    fn direct<'a>(gate: &'a PostGate, binding: Binding) -> Result<DirectTicket<'a>, Repeat> {
        gate.begin_direct(binding, SendOrigin::Mcp, "lead team".to_owned())
    }

    /// A report of what a send posted, as tests need one.
    fn report(outcome: SendOutcome, uncertain: bool) -> SendReport {
        SendReport {
            outcome,
            uncertain_part: uncertain.then_some(2),
            parts_posted: u32::from(outcome != SendOutcome::Unconfirmed),
            parts_total: 2,
            message_id: (outcome != SendOutcome::Unconfirmed).then(|| "9000".to_owned()),
            ..SendReport::unconfirmed("words")
        }
    }

    #[tokio::test]
    async fn a_matching_commit_spends_the_proposal_exactly_once() {
        let gate = numbered_from_zero();
        let (proposal, displaced) = gate.propose(binding("ship it"));
        assert!(displaced.is_none());
        assert_eq!(
            take(&gate, &proposal.handle, &binding("ship it")),
            Ok(proposal.clone())
        );
        assert_eq!(
            take(&gate, &proposal.handle, &binding("ship it")).map_err(|e| e.0),
            Err(Refusal::Used)
        );
        assert!(gate.pending().is_none());
    }

    #[tokio::test]
    async fn any_difference_in_the_restated_binding_refuses_and_spends_it() {
        for restated in [
            binding("ship it!"),
            Binding {
                channel_id: "222".to_owned(),
                ..binding("ship it")
            },
            Binding {
                reply_to: Some("9".to_owned()),
                ..binding("ship it")
            },
        ] {
            let gate = numbered_from_zero();
            let (proposal, _) = gate.propose(binding("ship it"));
            assert_eq!(
                take(&gate, &proposal.handle, &restated).map_err(|e| e.0),
                Err(Refusal::Mismatch),
                "{restated:?}"
            );
            // Spent: the correct restatement cannot now succeed.
            assert_eq!(
                take(&gate, &proposal.handle, &binding("ship it")).map_err(|e| e.0),
                Err(Refusal::Used)
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn an_expired_proposal_is_refused_as_expired() {
        let gate = PostGate::with_ttl_and_numbering(Duration::from_secs(5), 0);
        let (proposal, _) = gate.propose(binding("late"));
        tokio::time::advance(Duration::from_secs(5)).await;
        assert!(gate.pending().is_none());
        assert_eq!(
            take(&gate, &proposal.handle, &binding("late")).map_err(|e| e.0),
            Err(Refusal::Expired)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_commit_that_is_first_to_notice_the_expiry_refuses_it_and_wakes_watchers() {
        // Nothing reads `pending` first here, so the lapse is found inside `take` itself.
        let gate = PostGate::with_ttl_and_numbering(Duration::from_secs(5), 0);
        let (proposal, _) = gate.propose(binding("late"));
        let changes = gate.changed.subscribe();
        tokio::time::advance(Duration::from_secs(5)).await;
        let (why, lapsed) = take(&gate, &proposal.handle, &binding("late"))
            .expect_err("an expired proposal was committed");
        assert_eq!(why, Refusal::Expired);
        assert_eq!(lapsed.map(|p| p.serial), Some(proposal.serial));
        assert!(changes.has_changed().expect("the gate is alive"));
        assert!(gate.pending().is_none());
    }

    #[tokio::test]
    async fn a_newer_proposal_supersedes_the_pending_one() {
        let gate = numbered_from_zero();
        let (first, _) = gate.propose(binding("draft one"));
        let (second, displaced) = gate.propose(binding("draft two"));
        assert_eq!(
            displaced,
            Some(Displaced {
                proposal: first.clone(),
                why: Refusal::Superseded
            })
        );
        assert!(second.serial > first.serial);
        assert_ne!(first.handle, second.handle);
        assert_eq!(
            take(&gate, &first.handle, &binding("draft one")).map_err(|e| e.0),
            Err(Refusal::Superseded)
        );
        assert_eq!(gate.pending(), Some(second));
    }

    #[tokio::test(start_paused = true)]
    async fn the_same_direct_send_is_not_begun_twice_on_its_way_or_within_a_minute() {
        // `#264 voice-send-approval`: a retried tool call, or an owner's "try again" after a lost
        // answer, must not post the same words twice.
        let gate = numbered_from_zero();
        let (draft, _) = gate.propose(binding("draft"));
        let changes = gate.changed.subscribe();
        let first = direct(&gate, binding("rebase")).expect("nothing like it yet");
        let serial = first.serial();
        assert!(serial > draft.serial, "direct sends share the counter");
        assert!(
            !changes.has_changed().expect("the gate is alive"),
            "beginning a direct send woke a watcher: nothing it shows has changed yet"
        );
        let on_its_way =
            direct(&gate, binding("rebase")).expect_err("begun twice while on its way");
        assert_eq!(on_its_way.serial, serial);
        assert_eq!(on_its_way.outcome, DirectOutcome::InFlight);
        assert_eq!(on_its_way.left, DIRECT_REPEAT_WINDOW);

        // Other words, another channel or another reply target are other posts.
        let other = direct(&gate, binding("merge")).expect("other words");
        other.finish(Some(report(SendOutcome::Sent, false)));
        let elsewhere = Binding {
            channel_id: "222".to_owned(),
            ..binding("rebase")
        };
        direct(&gate, elsewhere)
            .expect("another channel")
            .finish(None);
        let replying = Binding {
            reply_to: Some("42".to_owned()),
            ..binding("rebase")
        };
        direct(&gate, replying)
            .expect("another reply target")
            .finish(None);

        first.finish(Some(report(SendOutcome::Sent, false)));
        tokio::time::advance(Duration::from_secs(20)).await;
        let sent = direct(&gate, binding("rebase")).expect_err("posted again 20 s after it went");
        assert_eq!(sent.outcome, DirectOutcome::Sent);
        assert_eq!(sent.age, Duration::from_secs(20));
        assert_eq!(sent.left, Duration::from_secs(40));

        tokio::time::advance(Duration::from_secs(40)).await;
        direct(&gate, binding("rebase"))
            .expect("a minute after it ended, the same words may go again")
            .finish(None);
        assert_eq!(
            gate.pending(),
            Some(draft),
            "a direct send touched the pending proposal"
        );
        // `#270 voice-transcript-markers`: a send that SETTLED wakes the watchers, so the page
        // can draw it; the ones that posted nothing published nothing.
        assert!(changes.has_changed().expect("the gate is alive"));
        let (sends, through) = gate.sends_after(0);
        assert_eq!(through, 2, "only the two sends that posted were numbered");
        assert_eq!(
            sends
                .iter()
                .map(|send| send.text.as_str())
                .collect::<Vec<_>>(),
            ["words", "words"]
        );
    }

    #[tokio::test]
    async fn a_direct_send_that_certainly_posted_nothing_is_forgotten_and_an_unknown_one_is_not() {
        let gate = numbered_from_zero();
        direct(&gate, binding("refused"))
            .expect("first")
            .finish(None);
        direct(&gate, binding("refused"))
            .expect("nothing was posted, so trying again is allowed")
            .finish(None);

        for (outcome, sent) in [
            (DirectOutcome::Partly, report(SendOutcome::Partial, false)),
            (
                DirectOutcome::PartlyUnconfirmed,
                report(SendOutcome::Partial, true),
            ),
            (
                DirectOutcome::Unconfirmed,
                report(SendOutcome::Unconfirmed, false),
            ),
        ] {
            let text = outcome.as_str();
            direct(&gate, binding(text))
                .expect("first")
                .finish(Some(sent));
            let repeat = direct(&gate, binding(text))
                .expect_err("posted again over a send that may have landed");
            assert_eq!(repeat.outcome, outcome);
        }
    }

    #[tokio::test]
    async fn a_direct_send_abandoned_on_its_way_is_remembered_as_unconfirmed() {
        // A client that hangs up mid-call drops the request, and the post may already be on the
        // wire: that is a send nobody can confirm, not one that never happened.
        let gate = numbered_from_zero();
        let ticket = direct(&gate, binding("rebase")).expect("first");
        drop(ticket);
        let repeat = direct(&gate, binding("rebase")).expect_err("an abandoned send was forgotten");
        assert_eq!(repeat.outcome, DirectOutcome::Unconfirmed);
        // And the send memory files it as unconfirmed, every word, for the page to draw.
        let (sends, _) = gate.sends_after(0);
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0].text, "rebase");
        assert_eq!(sends[0].record.outcome, SendOutcome::Unconfirmed);
        assert_eq!(sends[0].record.how, SendHow::Direct);
        assert_eq!(sends[0].record.origin, SendOrigin::Mcp);
    }

    // --- #270 voice-transcript-markers: the send memory -----------------------------------------

    #[tokio::test]
    async fn a_commit_enters_the_send_memory_as_it_is_taken_and_publishes_when_it_settles() {
        let gate = numbered_from_zero();
        let (proposal, _) = gate.propose(binding("ship it"));
        let (taken, ticket) = gate
            .take(
                &proposal.handle,
                &binding("ship it"),
                SendHow::Voice,
                "lead team".to_owned(),
            )
            .expect("taken");
        assert_eq!(taken.serial, proposal.serial);
        assert_eq!(
            gate.sends_after(0),
            (Vec::new(), 0),
            "a send on its way was published"
        );
        let send_seq = ticket
            .settle(report(SendOutcome::Sent, false))
            .expect("filed");
        assert_eq!(send_seq, 1);
        let (sends, through) = gate.sends_after(0);
        assert_eq!(through, 1);
        let record = &sends[0].record;
        assert_eq!(record.serial, proposal.serial);
        assert_eq!(record.send_seq, 1);
        assert_eq!(record.how, SendHow::Voice);
        assert_eq!(record.origin, SendOrigin::Commit);
        assert_eq!(record.channel_name, "lead team");
        assert_eq!(record.channel_id, "111");
        assert!(record.sent_at_ms >= record.begun_at_ms);
        assert_eq!(gate.settled_send(1), Some(sends[0].clone()));
        assert_eq!(gate.sends_after(1), (Vec::new(), 1));
        assert_eq!(gate.fates(), [(proposal.serial, ProposalFate::Committed)]);
    }

    #[tokio::test]
    async fn send_seq_follows_the_order_sends_settle_in_not_their_serials() {
        // Two direct sends, settling out of serial order: a cursor of serials would skip the first.
        let gate = numbered_from_zero();
        let early = direct(&gate, binding("early")).expect("first");
        let late = direct(&gate, binding("late")).expect("second");
        assert!(early.serial() < late.serial());
        let late_serial = late.serial();
        assert_eq!(late.finish(Some(report(SendOutcome::Sent, false))), Some(1));
        let (sends, through) = gate.sends_after(0);
        assert_eq!((sends.len(), through), (1, 1));
        assert_eq!(sends[0].record.serial, late_serial);
        assert_eq!(
            early.finish(Some(report(SendOutcome::Sent, false))),
            Some(2)
        );
        let (after_cursor, through) = gate.sends_after(1);
        assert_eq!(through, 2);
        assert_eq!(
            after_cursor.len(),
            1,
            "the slower send was skipped past the cursor"
        );
        assert!(after_cursor[0].record.serial < late_serial);
    }

    #[tokio::test]
    async fn a_send_that_certainly_posted_nothing_is_never_published() {
        let gate = numbered_from_zero();
        let (proposal, _) = gate.propose(binding("refused upstream"));
        let (_, ticket) = gate
            .take(
                &proposal.handle,
                &binding("refused upstream"),
                SendHow::Tap,
                "lead team".to_owned(),
            )
            .expect("taken");
        ticket.forget();
        assert_eq!(gate.sends_after(0), (Vec::new(), 0));
    }

    #[tokio::test]
    async fn a_long_text_is_filed_capped_and_says_how_long_it_was() {
        let gate = numbered_from_zero();
        let long = "word ".repeat(MAX_TURN_CHARS);
        let ticket = direct(&gate, binding("long")).expect("first");
        ticket.finish(Some(SendReport {
            text: long.clone(),
            ..report(SendOutcome::Sent, false)
        }));
        let (sends, _) = gate.sends_after(0);
        assert_eq!(sends[0].text.chars().count(), MAX_TURN_CHARS);
        assert!(sends[0].record.text_truncated);
        assert_eq!(
            sends[0].record.text_chars,
            u32::try_from(long.chars().count()).expect("fits")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_send_memory_holds_thirty_two_for_fifteen_minutes_and_forgetting_empties_it() {
        let gate = numbered_from_zero();
        for i in 0..40 {
            direct(&gate, binding(&format!("m{i}")))
                .expect("distinct words")
                .finish(Some(report(SendOutcome::Sent, false)));
        }
        let (sends, through) = gate.sends_after(0);
        assert_eq!(through, 40);
        assert_eq!(sends.len(), SEND_MEMORY);
        assert_eq!(sends[0].record.send_seq, 9, "the oldest left first");

        tokio::time::advance(SEND_MEMORY_TTL - Duration::from_secs(1)).await;
        assert_eq!(gate.sends_after(0).0.len(), SEND_MEMORY);
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(gate.sends_after(0), (Vec::new(), 40));

        direct(&gate, binding("after"))
            .expect("new words")
            .finish(Some(report(SendOutcome::Sent, false)));
        let on_its_way = direct(&gate, binding("on its way")).expect("new words");
        gate.forget_sends();
        assert_eq!(gate.sends_after(0), (Vec::new(), 41));
        assert_eq!(
            on_its_way.finish(Some(report(SendOutcome::Sent, false))),
            None,
            "a send the memory forgot while it was on its way was published anyway"
        );
        assert_eq!(gate.sends_through(), 41, "the counter never goes back");
    }

    #[tokio::test]
    async fn a_settled_send_is_claimed_once_and_forgetting_its_conversation_forgets_it() {
        let gate = numbered_from_zero();
        direct(&gate, binding("ship it"))
            .expect("first")
            .finish(Some(report(SendOutcome::Sent, false)));
        assert_eq!(gate.claim_send(9, "conv"), Err(ClaimRefusal::Unknown));
        let claimed = gate.claim_send(1, "conv").expect("claimed");
        assert_eq!(claimed.text, "words");
        assert_eq!(gate.claim_send(1, "other"), Err(ClaimRefusal::Recorded));
        gate.unclaim_send(1);
        assert!(
            gate.claim_send(1, "other").is_ok(),
            "an undone claim stayed taken"
        );
        gate.forget_sends_claimed_into("conv");
        assert_eq!(
            gate.sends_after(0).0.len(),
            1,
            "another conversation's send was forgotten"
        );
        gate.forget_sends_claimed_into("other");
        assert_eq!(gate.sends_after(0), (Vec::new(), 1));
    }

    #[tokio::test]
    async fn a_new_gate_numbers_its_sends_above_the_clock_so_a_restart_never_numbers_lower() {
        let before = u64::try_from(crate::store::now_ms()).expect("after 1970");
        let first = PostGate::new();
        assert!(first.sends_through() >= before);
        direct(&first, binding("one"))
            .expect("first")
            .finish(Some(report(SendOutcome::Sent, false)));
        let read_through = first.sends_through();
        // The next process: every send it numbers is past a cursor the last one handed out.
        tokio::time::sleep(Duration::from_millis(2)).await;
        let next = PostGate::new();
        direct(&next, binding("two"))
            .expect("first")
            .finish(Some(report(SendOutcome::Sent, false)));
        let (sends, _) = next.sends_after(read_through);
        assert_eq!(
            sends.len(),
            1,
            "the restarted server's send fell under the old cursor"
        );
        assert_eq!(sends[0].text, "words");
    }

    #[test]
    fn each_run_names_itself_differently_and_keeps_its_name() {
        // Serials start again at one in every run: what tells a page this run's serial 3 from the
        // last run's is the run's name, which must stay put for the life of one gate.
        let first = PostGate::new();
        let next = PostGate::new();
        assert!(!first.instance().is_empty());
        assert_ne!(first.instance(), next.instance(), "two runs share a name");
        assert_eq!(first.instance(), first.instance());
        let (proposal, _) = first.propose(binding("one"));
        let (again, _) = next.propose(binding("one"));
        assert_eq!(
            proposal.serial, again.serial,
            "the serials really do come round again, which is why the name is needed"
        );
    }

    #[tokio::test]
    async fn a_cursor_ahead_of_this_process_is_read_as_zero() {
        // The page read from a server that has since restarted.
        let gate = numbered_from_zero();
        direct(&gate, binding("after the restart"))
            .expect("first")
            .finish(Some(report(SendOutcome::Sent, false)));
        let (sends, through) = gate.sends_after(57);
        assert_eq!((sends.len(), through), (1, 1));
    }

    #[tokio::test]
    async fn fates_tell_committed_cancelled_expired_superseded_and_refused_apart() {
        let gate = PostGate::with_ttl_and_numbering(Duration::from_millis(1), 0);
        let (lapsing, _) = gate.propose(binding("lapses"));
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(gate.pending().is_none());
        let gate_fates = gate.fates();
        assert_eq!(gate_fates, [(lapsing.serial, ProposalFate::Expired)]);

        let gate = numbered_from_zero();
        let (replaced, _) = gate.propose(binding("one"));
        let (cancelled, _) = gate.propose(binding("two"));
        gate.cancel(&cancelled.handle).expect("cancel");
        let (refused, _) = gate.propose(binding("three"));
        take(&gate, &refused.handle, &binding("three!")).expect_err("mismatch");
        let (committed, _) = gate.propose(binding("four"));
        take(&gate, &committed.handle, &binding("four")).expect("commit");
        assert_eq!(
            gate.fates(),
            [
                (replaced.serial, ProposalFate::Superseded),
                (cancelled.serial, ProposalFate::Cancelled),
                (refused.serial, ProposalFate::Refused),
                (committed.serial, ProposalFate::Committed),
            ]
        );
        // And a second commit is still told why, as before.
        assert_eq!(
            take(&gate, &replaced.handle, &binding("one")).map_err(|e| e.0),
            Err(Refusal::Superseded)
        );
        assert_eq!(
            take(&gate, &refused.handle, &binding("three")).map_err(|e| e.0),
            Err(Refusal::Used)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_long_poll_wakes_for_a_settled_send_only_when_it_reads_the_send_memory() {
        let gate = std::sync::Arc::new(numbered_from_zero());
        // Not reading the send memory: a settle does not end the wait.
        let blind = {
            let gate = gate.clone();
            tokio::spawn(async move {
                let started = Instant::now();
                gate.wait_for_change(None, None, Duration::from_secs(9))
                    .await;
                started.elapsed()
            })
        };
        // Reading it from 0: the settle ends the wait at once.
        let reader = {
            let gate = gate.clone();
            tokio::spawn(async move {
                let started = Instant::now();
                gate.wait_for_change(None, Some(0), Duration::from_secs(9))
                    .await;
                started.elapsed()
            })
        };
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(2)).await;
        direct(&gate, binding("posted"))
            .expect("first")
            .finish(Some(report(SendOutcome::Sent, false)));
        assert_eq!(reader.await.expect("task"), Duration::from_secs(2));
        assert_eq!(blind.await.expect("task"), Duration::from_secs(9));
        // A cursor already at the newest waits; one past it answers at once.
        let started = Instant::now();
        gate.wait_for_change(None, Some(1), Duration::from_secs(3))
            .await;
        assert_eq!(started.elapsed(), Duration::from_secs(3));
        let started = Instant::now();
        gate.wait_for_change(None, Some(5), Duration::from_secs(3))
            .await;
        assert_eq!(started.elapsed(), Duration::ZERO);
    }

    #[tokio::test]
    async fn a_cancelled_proposal_cannot_be_committed() {
        let gate = numbered_from_zero();
        let (proposal, _) = gate.propose(binding("never mind"));
        assert_eq!(gate.cancel(&proposal.handle), Ok(proposal.clone()));
        assert_eq!(gate.cancel(&proposal.handle), Err(Refusal::Used));
        assert_eq!(
            take(&gate, &proposal.handle, &binding("never mind")).map_err(|e| e.0),
            Err(Refusal::Used)
        );
    }

    #[tokio::test]
    async fn a_handle_that_was_never_issued_is_unknown_and_disturbs_nothing() {
        let gate = numbered_from_zero();
        let (proposal, _) = gate.propose(binding("real"));
        assert_eq!(
            take(&gate, "made-up", &binding("real")).map_err(|e| e.0),
            Err(Refusal::Unknown)
        );
        assert_eq!(gate.cancel("made-up"), Err(Refusal::Unknown));
        assert_eq!(gate.pending(), Some(proposal));
    }

    #[tokio::test]
    async fn handles_are_distinct_and_unguessably_long() {
        let gate = numbered_from_zero();
        let mut seen = std::collections::HashSet::new();
        for i in 0..100 {
            let (proposal, _) = gate.propose(binding(&format!("m{i}")));
            // 16 bytes, unpadded base64url.
            assert_eq!(proposal.handle.len(), 22, "{}", proposal.handle);
            assert!(seen.insert(proposal.handle));
        }
    }

    #[tokio::test]
    async fn concurrent_commits_of_one_handle_post_at_most_once() {
        let gate = std::sync::Arc::new(numbered_from_zero());
        let (proposal, _) = gate.propose(binding("once"));
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let gate = gate.clone();
            let handle = proposal.handle.clone();
            tasks.push(tokio::spawn(async move {
                take(&gate, &handle, &binding("once")).is_ok()
            }));
        }
        let mut wins = 0;
        for task in tasks {
            if task.await.expect("task") {
                wins += 1;
            }
        }
        assert_eq!(wins, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_long_poll_wakes_for_a_new_proposal_and_for_an_expiry() {
        let gate = std::sync::Arc::new(PostGate::with_ttl_and_numbering(Duration::from_secs(5), 0));
        let waiter = {
            let gate = gate.clone();
            tokio::spawn(async move {
                gate.wait_for_change(None, None, Duration::from_secs(30))
                    .await
            })
        };
        tokio::task::yield_now().await;
        let (proposal, _) = gate.propose(binding("hello"));
        assert_eq!(waiter.await.expect("task"), Some(proposal.clone()));

        // Seen: waits, then wakes by itself at expiry with nothing pending.
        let started = Instant::now();
        let after = gate
            .wait_for_change(Some(proposal.serial), None, Duration::from_secs(30))
            .await;
        assert_eq!(after, None);
        assert_eq!(started.elapsed(), Duration::from_secs(5));

        // Nothing changes: answers at the deadline with the same (empty) state.
        let started = Instant::now();
        assert_eq!(
            gate.wait_for_change(None, None, Duration::from_secs(7))
                .await,
            None
        );
        assert_eq!(started.elapsed(), Duration::from_secs(7));
    }
}
