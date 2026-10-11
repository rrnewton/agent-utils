//! Clearing a channel's unread state at its chat provider, as this server ingests it.
//! `#232 upstream-mark-read`.
//!
//! # Why
//!
//! vibe-talk reads only some of the owner's channels, and the ones it reads are mostly coding
//! agents posting updates. In the provider's own client those updates bury the messages from
//! people. With this turned on for a channel, every message this server receives there is also
//! marked read in the provider's client, so that client's unread list keeps the people and
//! vibe-talk keeps the agents. vibe-talk's own read marks and Done state are not touched.
//!
//! # How
//!
//! * **The setting is the server's, per channel**: off, a dry run, or on, in the durable store
//!   ([`crate::store::UpstreamClearSetting`]), because the server has to act with no page open.
//! * **Ingest never waits for it.** [`crate::live::LiveHub`] is where both message producers land
//!   (adapter pushes and polls). On every created message it calls [`Candidates::offer`], which
//!   is one map update under a mutex and a wake-up, with no I/O.
//! * **One push per burst.** A channel's newest message is pushed after [`Timing::quiet`] with
//!   nothing newer, or after [`Timing::max_wait`] during steady traffic, and never sooner than
//!   [`Timing::max_wait`] after that channel's previous push.
//! * **Never backwards.** A message at or before the newest one already covered is never pushed;
//!   that floor is in the store and survives a restart. The provider bridge also re-reads its
//!   real cursor and refuses to move it back.
//! * **Failures wait and retry**, doubling from [`Timing::retry_first`] to [`Timing::retry_cap`]
//!   with a little per-channel jitter. The newest message always wins, and a success resets it.
//! * **Every attempt is logged on one line** starting `upstream-clear:`, dry runs included, so
//!   an operator can see what the server would do before the owner relies on it.
//!
//! Pushes are serial across channels: one task, one provider call at a time.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;

use crate::chat::{ChatClient, ChatError, UpstreamClearOutcome};
use crate::model::{ChannelId, Message, MessageId};
use crate::store::{
    StateStore, StoreError, UpstreamClearAttempt, UpstreamClearMode, UpstreamClearResult,
    UpstreamClearSetting,
};

/// How long a channel must be quiet before its newest message is pushed.
pub const DEFAULT_QUIET: Duration = Duration::from_secs(5);
/// The longest a message waits during steady traffic, and the least time between two pushes for
/// one channel.
pub const DEFAULT_MAX_WAIT: Duration = Duration::from_secs(30);
/// The wait after a first failure.
pub const DEFAULT_RETRY_FIRST: Duration = Duration::from_secs(30);
/// The longest wait between retries.
pub const DEFAULT_RETRY_CAP: Duration = Duration::from_secs(15 * 60);

/// What a refusal for a thread-bound channel means, in words for Settings.
pub const THREAD_CHANNEL_REASON: &str = "this channel is one thread of a larger space, and the \
     provider's read marker covers the whole space, so it would also clear messages vibe-talk \
     never received";

/// The waits this module keeps. Tests shorten them; the server uses [`Timing::default`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// See [`DEFAULT_QUIET`].
    pub quiet: Duration,
    /// See [`DEFAULT_MAX_WAIT`].
    pub max_wait: Duration,
    /// See [`DEFAULT_RETRY_FIRST`].
    pub retry_first: Duration,
    /// See [`DEFAULT_RETRY_CAP`].
    pub retry_cap: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            quiet: DEFAULT_QUIET,
            max_wait: DEFAULT_MAX_WAIT,
            retry_first: DEFAULT_RETRY_FIRST,
            retry_cap: DEFAULT_RETRY_CAP,
        }
    }
}

/// How long to wait after `failures` failures in a row: doubling from `first`, never more than
/// `cap`, plus up to a tenth more that depends only on the channel, so channels that failed
/// together do not all retry in the same instant.
#[must_use]
pub fn retry_delay(timing: &Timing, channel: &ChannelId, failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    let base = timing
        .retry_first
        .saturating_mul(1_u32 << doublings)
        .min(timing.retry_cap);
    // FNV-1a over the id: stable, cheap, and spread enough for a handful of channels.
    let mut hash: u32 = 0x811c_9dc5;
    for byte in channel.as_str().bytes() {
        hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
    }
    base + base.mul_f64(f64::from(hash % 1000) / 10_000.0)
}

/// Where a message sits in its channel: its instant in milliseconds, then its numeric id for two
/// messages in the same millisecond. `None` when its instant cannot be read, and such a message
/// is never pushed: a cursor cannot be placed by a time nobody can read.
#[must_use]
pub fn position(message: &Message) -> Option<(i64, u64)> {
    position_of(&message.id, &message.timestamp)
}

fn position_of(id: &MessageId, timestamp: &str) -> Option<(i64, u64)> {
    let instant = timestamp.parse::<jiff::Timestamp>().ok()?;
    Some((instant.as_millisecond(), id.numeric().unwrap_or(0)))
}

/// A message waiting to be pushed.
#[derive(Clone, Debug)]
struct Pending {
    message: MessageId,
    timestamp: String,
    position: (i64, u64),
    first_seen: Instant,
    last_seen: Instant,
    immediate: bool,
}

/// One enabled channel's state between pushes.
#[derive(Debug)]
struct Runtime {
    mode: UpstreamClearMode,
    generation: u64,
    pending: Option<Pending>,
    floor: Option<(i64, u64)>,
    last_push: Option<Instant>,
    failures: u32,
    retry_at: Option<Instant>,
    retry_at_ms: Option<i64>,
    unavailable: Option<String>,
}

/// One push the loop has taken and not yet finished.
#[derive(Clone, Debug)]
pub struct Job {
    /// The channel.
    pub channel: ChannelId,
    /// The newest message, at the moment the push was taken.
    pub message: MessageId,
    /// That message's instant as the provider wrote it.
    pub timestamp: String,
    /// Off is never taken; this is a dry run or on.
    pub mode: UpstreamClearMode,
    position: (i64, u64),
    generation: u64,
    pending: Pending,
}

/// What the page is told about a channel's push state beyond what the store holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveStatus {
    /// Whether a message is waiting to be pushed.
    pub pending: bool,
    /// When the next retry is due, in milliseconds since the Unix epoch, after a failure.
    pub retry_at_ms: Option<i64>,
    /// Why this channel cannot be cleared at all, once the provider has said so.
    pub unavailable: Option<String>,
}

/// The newest message per enabled channel, waiting to be pushed, and the wake-up for the loop.
///
/// Owned by [`crate::live::LiveHub`], which feeds it. Holds nothing for a channel that is off, so
/// a deployment with the feature unused pays one map lookup per message.
#[derive(Debug, Default)]
pub struct Candidates {
    channels: Mutex<BTreeMap<ChannelId, Runtime>>,
    next_generation: Mutex<u64>,
    wake: Notify,
}

fn lock<T>(cell: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    cell.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

impl Candidates {
    /// An empty set: every channel off.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Note a newly created message. Never blocks on I/O; returns whether it is now the
    /// channel's newest waiting message.
    pub fn offer(&self, channel: &ChannelId, message: &Message) -> bool {
        self.note(channel, &message.id, &message.timestamp, false)
    }

    /// [`Candidates::offer`] from a message's id and instant alone.
    pub fn offer_parts(&self, channel: &ChannelId, id: &MessageId, timestamp: &str) -> bool {
        self.note(channel, id, timestamp, false)
    }

    /// Note `message` and push it as soon as the loop can, without waiting for quiet or for the
    /// spacing between pushes: the owner asked to clear what is unread now.
    pub fn offer_now(&self, channel: &ChannelId, message: &Message) -> bool {
        self.note(channel, &message.id, &message.timestamp, true)
    }

    fn note(&self, channel: &ChannelId, id: &MessageId, timestamp: &str, immediate: bool) -> bool {
        let Some(position) = position_of(id, timestamp) else {
            return false;
        };
        let now = Instant::now();
        let mut channels = lock(&self.channels);
        let Some(runtime) = channels.get_mut(channel) else {
            return false;
        };
        if runtime.unavailable.is_some() || runtime.floor.is_some_and(|floor| position <= floor) {
            return false;
        }
        let pending = match runtime.pending.take() {
            Some(mut pending) if position <= pending.position => {
                pending.last_seen = now;
                pending.immediate |= immediate;
                runtime.pending = Some(pending);
                drop(channels);
                if immediate {
                    self.wake.notify_one();
                }
                return false;
            }
            Some(pending) => Pending {
                message: id.clone(),
                timestamp: timestamp.to_owned(),
                position,
                first_seen: pending.first_seen,
                last_seen: now,
                immediate: immediate || pending.immediate,
            },
            None => Pending {
                message: id.clone(),
                timestamp: timestamp.to_owned(),
                position,
                first_seen: now,
                last_seen: now,
                immediate,
            },
        };
        runtime.pending = Some(pending);
        drop(channels);
        self.wake.notify_one();
        true
    }

    /// Put a channel in `mode`, with `floor` the position its provider cursor is already known to
    /// cover. Off forgets the channel. Any other change starts it afresh, keeping a message
    /// already waiting.
    pub fn set_mode(
        &self,
        channel: &ChannelId,
        mode: UpstreamClearMode,
        floor: Option<(i64, u64)>,
    ) {
        let generation = {
            let mut next = lock(&self.next_generation);
            *next += 1;
            *next
        };
        let mut channels = lock(&self.channels);
        if mode == UpstreamClearMode::Off {
            channels.remove(channel);
        } else {
            match channels.remove(channel) {
                // The same mode again, still possible: nothing to start afresh.
                Some(previous) if previous.mode == mode && previous.unavailable.is_none() => {
                    channels.insert(channel.clone(), previous);
                }
                previous => {
                    let pending = previous
                        .and_then(|previous| previous.pending)
                        .filter(|pending| floor.is_none_or(|floor| pending.position > floor));
                    channels.insert(
                        channel.clone(),
                        Runtime {
                            mode,
                            generation,
                            pending,
                            floor,
                            last_push: None,
                            failures: 0,
                            retry_at: None,
                            retry_at_ms: None,
                            unavailable: None,
                        },
                    );
                }
            }
        }
        drop(channels);
        self.wake.notify_one();
    }

    /// The mode the loop is acting on for `channel`: off when it holds nothing for it.
    #[must_use]
    pub fn mode(&self, channel: &ChannelId) -> UpstreamClearMode {
        lock(&self.channels)
            .get(channel)
            .map_or(UpstreamClearMode::Off, |runtime| runtime.mode)
    }

    /// What the page is told beyond the store, for one channel.
    #[must_use]
    pub fn status(&self, channel: &ChannelId) -> LiveStatus {
        lock(&self.channels)
            .get(channel)
            .map(|runtime| LiveStatus {
                pending: runtime.pending.is_some(),
                retry_at_ms: runtime.retry_at_ms,
                unavailable: runtime.unavailable.clone(),
            })
            .unwrap_or_default()
    }

    fn due_at(runtime: &Runtime, timing: &Timing) -> Option<Instant> {
        let pending = runtime.pending.as_ref()?;
        let mut due = if pending.immediate {
            pending.first_seen
        } else {
            let burst = (pending.last_seen + timing.quiet).min(pending.first_seen + timing.max_wait);
            runtime
                .last_push
                .map_or(burst, |pushed| burst.max(pushed + timing.max_wait))
        };
        if let Some(retry) = runtime.retry_at {
            due = due.max(retry);
        }
        Some(due)
    }

    /// When the next push is due, across every channel.
    fn next_due(&self, timing: &Timing) -> Option<Instant> {
        lock(&self.channels)
            .values()
            .filter_map(|runtime| Self::due_at(runtime, timing))
            .min()
    }

    /// Take the push that is due soonest, if one is due by `now`.
    fn take_due(&self, now: Instant, timing: &Timing) -> Option<Job> {
        let mut channels = lock(&self.channels);
        let (channel, _) = channels
            .iter()
            .filter_map(|(channel, runtime)| {
                Self::due_at(runtime, timing).map(|due| (channel.clone(), due))
            })
            .filter(|(_, due)| *due <= now)
            .min_by_key(|(_, due)| *due)?;
        let runtime = channels.get_mut(&channel)?;
        let pending = runtime.pending.take()?;
        Some(Job {
            channel,
            message: pending.message.clone(),
            timestamp: pending.timestamp.clone(),
            mode: runtime.mode,
            position: pending.position,
            generation: runtime.generation,
            pending,
        })
    }

    /// Fold a finished push back in.
    fn finish(&self, job: &Job, result: UpstreamClearResult, error: Option<&ChatError>, timing: &Timing) {
        let now = Instant::now();
        let mut channels = lock(&self.channels);
        let Some(runtime) = channels.get_mut(&job.channel) else {
            return;
        };
        if runtime.generation != job.generation {
            // The owner changed the mode while this was in flight; what it found is logged and
            // recorded, and the fresh state is left alone.
            return;
        }
        runtime.last_push = Some(now);
        match result {
            UpstreamClearResult::Advanced
            | UpstreamClearResult::AlreadyRead
            | UpstreamClearResult::DryRun => {
                runtime.floor = Some(runtime.floor.map_or(job.position, |floor| floor.max(job.position)));
                runtime.failures = 0;
                runtime.retry_at = None;
                runtime.retry_at_ms = None;
                if runtime
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.position <= job.position)
                {
                    runtime.pending = None;
                }
            }
            UpstreamClearResult::Skipped => {
                // The bridge no longer holds it. Wait for something newer rather than retrying.
                runtime.retry_at = None;
                runtime.retry_at_ms = None;
            }
            UpstreamClearResult::Unavailable => {
                runtime.pending = None;
                runtime.unavailable = Some(
                    error
                        .map(unavailable_reason)
                        .unwrap_or_else(|| THREAD_CHANNEL_REASON.to_owned()),
                );
            }
            UpstreamClearResult::Failed => {
                runtime.failures = runtime.failures.saturating_add(1);
                let mut wait = retry_delay(timing, &job.channel, runtime.failures);
                if let Some(asked) = error.and_then(ChatError::retry_after) {
                    wait = wait.max(asked);
                }
                runtime.retry_at = Some(now + wait);
                runtime.retry_at_ms = Some(
                    now_ms().saturating_add(i64::try_from(wait.as_millis()).unwrap_or(i64::MAX)),
                );
                // The newest message wins: put this one back only if nothing newer arrived.
                if runtime.pending.is_none() {
                    runtime.pending = Some(Pending {
                        immediate: false,
                        ..job.pending.clone()
                    });
                }
            }
        }
        drop(channels);
        self.wake.notify_one();
    }
}

/// The sentence Settings shows for a channel the provider refused outright.
fn unavailable_reason(error: &ChatError) -> String {
    match error.cause() {
        ChatError::Status { body, .. } if body.contains("thread_channel") => {
            THREAD_CHANNEL_REASON.to_owned()
        }
        _ => error.to_string(),
    }
}

/// What one provider answer comes to.
#[must_use]
pub fn classify(answer: &Result<UpstreamClearOutcome, ChatError>) -> UpstreamClearResult {
    match answer {
        Ok(outcome) if outcome.dry_run => UpstreamClearResult::DryRun,
        Ok(outcome) if outcome.advanced => UpstreamClearResult::Advanced,
        Ok(_) => UpstreamClearResult::AlreadyRead,
        Err(error) => match error.cause() {
            ChatError::Status { status: 409, .. } => UpstreamClearResult::Unavailable,
            ChatError::Status { status: 404, .. } => UpstreamClearResult::Skipped,
            _ => UpstreamClearResult::Failed,
        },
    }
}

/// Everything the loop needs, without the whole server state, so a test can build one.
#[derive(Clone)]
pub struct Clearer {
    /// The provider router; each push goes to the channel's own provider.
    pub chat: Arc<dyn ChatClient>,
    /// Where the setting and every attempt are kept.
    pub store: Arc<dyn StateStore>,
    /// The hub's waiting messages.
    pub candidates: Arc<Candidates>,
    /// The waits.
    pub timing: Timing,
    /// The channel's display name for the log line, when it has one.
    pub label: Arc<dyn Fn(&ChannelId) -> Option<String> + Send + Sync>,
    /// Whether `channel` is one the server answers for and whose provider can be cleared.
    pub eligible: Arc<dyn Fn(&ChannelId) -> bool + Send + Sync>,
}

impl std::fmt::Debug for Clearer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Clearer")
            .field("timing", &self.timing)
            .finish_non_exhaustive()
    }
}

/// The floor a stored setting gives the loop: the newest message already covered.
#[must_use]
pub fn stored_floor(setting: &UpstreamClearSetting) -> Option<(i64, u64)> {
    setting.pushed.as_ref().map(UpstreamClearAttempt::position)
}

impl Clearer {
    /// The loop for a running server.
    #[must_use]
    pub fn for_state(state: &crate::state::AppState) -> Self {
        let labels = state.clone();
        let eligible = state.clone();
        Self {
            chat: Arc::clone(&state.chat),
            store: Arc::clone(&state.store),
            candidates: Arc::clone(state.live.upstream_clear()),
            timing: Timing::default(),
            label: Arc::new(move |channel| {
                labels.channel(channel.as_str()).map(|info| {
                    info.alias.unwrap_or(info.label)
                })
            }),
            eligible: Arc::new(move |channel| {
                eligible.channel(channel.as_str()).is_some()
                    && eligible
                        .providers
                        .client_for(channel)
                        .is_some_and(|client| client.supports_upstream_clear())
            }),
        }
    }

    /// Load every enabled channel from the store and seed each with its newest message, so a
    /// message that arrived while the server was down is still cleared.
    ///
    /// # Errors
    ///
    /// The store's, when it cannot be read. [`StoreError::Unavailable`] means no store is
    /// configured, and nothing can be enabled.
    pub async fn seed(&self) -> Result<usize, StoreError> {
        let settings = self.store.upstream_clear_settings().await?;
        let mut enabled = 0;
        for setting in settings {
            if setting.mode == UpstreamClearMode::Off || !(self.eligible)(&setting.channel) {
                continue;
            }
            enabled += 1;
            self.candidates
                .set_mode(&setting.channel, setting.mode, stored_floor(&setting));
            tracing::info!(
                channel = %setting.channel,
                mode = setting.mode.as_str(),
                "upstream-clear: {} channel {} ({}) is enabled; its newest message is checked now",
                setting.mode.as_str(),
                setting.channel,
                self.label_of(&setting.channel),
            );
            match self.chat.fetch_recent(&setting.channel, 1).await {
                Ok(messages) => {
                    if let Some(newest) = messages.last() {
                        self.candidates.offer(&setting.channel, newest);
                    }
                }
                Err(error) => tracing::warn!(
                    channel = %setting.channel,
                    %error,
                    "upstream-clear: could not read the newest message to seed this channel; \
                     the next message that arrives will be pushed instead"
                ),
            }
        }
        Ok(enabled)
    }

    fn label_of(&self, channel: &ChannelId) -> String {
        (self.label)(channel).unwrap_or_else(|| "unlisted".to_owned())
    }

    /// Push the soonest due message, if one is due now. Returns whether one was pushed.
    pub async fn push_due(&self) -> bool {
        let Some(job) = self.candidates.take_due(Instant::now(), &self.timing) else {
            return false;
        };
        if !(self.eligible)(&job.channel) {
            tracing::info!(
                channel = %job.channel,
                "upstream-clear: channel {} is no longer listed or its provider cannot be \
                 cleared; nothing was pushed",
                job.channel
            );
            return true;
        }
        let dry_run = job.mode == UpstreamClearMode::DryRun;
        let answer = self
            .chat
            .clear_upstream_through(&job.channel, &job.message, dry_run)
            .await;
        let result = classify(&answer);
        self.candidates
            .finish(&job, result, answer.as_ref().err(), &self.timing);
        let attempt = UpstreamClearAttempt {
            at_ms: now_ms(),
            message: job.message.clone(),
            message_ms: job.position.0,
            result,
            target: answer.as_ref().ok().map(|outcome| outcome.target.clone()),
            previous: answer.as_ref().ok().and_then(|outcome| outcome.previous.clone()),
            error: answer.as_ref().err().map(ToString::to_string),
        };
        self.log(&job, &attempt);
        if let Err(error) = self.store.record_upstream_clear(&job.channel, &attempt).await {
            tracing::warn!(
                channel = %job.channel,
                %error,
                "upstream-clear: could not record this attempt; Settings will show an older one"
            );
        }
        true
    }

    fn log(&self, job: &Job, attempt: &UpstreamClearAttempt) {
        let head = format!(
            "upstream-clear: {} channel {} ({}) through {} at {}",
            job.mode.as_str(),
            job.channel,
            self.label_of(&job.channel),
            job.message,
            job.timestamp,
        );
        let previous = attempt.previous.as_deref().unwrap_or("none");
        let target = attempt.target.as_deref().unwrap_or("?");
        match attempt.result {
            UpstreamClearResult::Advanced => tracing::info!(
                channel = %job.channel,
                "{head}: advanced (the provider's read marker moved from {previous} to {target})"
            ),
            UpstreamClearResult::AlreadyRead => tracing::info!(
                channel = %job.channel,
                "{head}: already-read (the provider's read marker was already at {previous})"
            ),
            UpstreamClearResult::DryRun => tracing::info!(
                channel = %job.channel,
                "{head}: dry-run (would move the provider's read marker from {previous} to \
                 {target}; nothing was written)"
            ),
            UpstreamClearResult::Skipped => tracing::info!(
                channel = %job.channel,
                "{head}: skipped (the bridge no longer holds that message; waiting for a newer one)"
            ),
            UpstreamClearResult::Unavailable => tracing::warn!(
                channel = %job.channel,
                "{head}: failed: not available for this channel ({}); no more attempts until the \
                 setting is changed",
                attempt.error.as_deref().unwrap_or("refused"),
            ),
            UpstreamClearResult::Failed => tracing::warn!(
                channel = %job.channel,
                "{head}: failed: {}; retrying later",
                attempt.error.as_deref().unwrap_or("unknown failure"),
            ),
        }
    }

    /// Push due messages until the process ends. Spawned once by `main`.
    pub async fn run(self) {
        match self.seed().await {
            Ok(enabled) => tracing::info!(
                enabled,
                "upstream-clear is ready: a channel set to clear unread in its provider has every \
                 message vibe-talk receives marked read there too, one push per burst. Off for \
                 every channel unless set in Settings"
            ),
            Err(StoreError::Unavailable(setting)) => {
                tracing::info!(
                    "upstream-clear is unavailable: it keeps its setting in the store, and \
                     {setting} is not configured"
                );
                return;
            }
            Err(error) => tracing::warn!(
                %error,
                "upstream-clear could not read its settings; channels set in Settings from now on \
                 still work"
            ),
        }
        loop {
            while self.push_due().await {}
            let notified = self.candidates.wake.notified();
            match self.candidates.next_due(&self.timing) {
                Some(due) => {
                    tokio::select! {
                        () = tokio::time::sleep_until(due) => {}
                        () = notified => {}
                    }
                }
                None => notified.await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::fake::FakeDiscord;
    use crate::store::fake::FakeStore;

    const QUICK: Timing = Timing {
        quiet: Duration::from_secs(5),
        max_wait: Duration::from_secs(30),
        retry_first: Duration::from_secs(30),
        retry_cap: Duration::from_secs(15 * 60),
    };

    #[test]
    fn the_retry_wait_doubles_to_its_cap_with_at_most_a_tenth_more() {
        let channel = ChannelId("1111111111".to_owned());
        let mut previous = Duration::ZERO;
        for failures in 1..=12 {
            let wait = retry_delay(&QUICK, &channel, failures);
            let base = (QUICK.retry_first * 2_u32.pow((failures - 1).min(16))).min(QUICK.retry_cap);
            assert!(wait >= base && wait <= base.mul_f64(1.1), "{failures}: {wait:?}");
            assert!(wait >= previous, "the wait shrank at {failures}");
            previous = wait;
        }
        assert!(retry_delay(&QUICK, &channel, 40) <= QUICK.retry_cap.mul_f64(1.1));
    }

    #[test]
    fn answers_are_classified_by_what_the_provider_said() {
        let outcome = |advanced, dry_run| UpstreamClearOutcome {
            scope: "space".to_owned(),
            target: "t".to_owned(),
            previous: None,
            advanced,
            dry_run,
        };
        assert_eq!(classify(&Ok(outcome(true, false))), UpstreamClearResult::Advanced);
        assert_eq!(classify(&Ok(outcome(false, false))), UpstreamClearResult::AlreadyRead);
        assert_eq!(classify(&Ok(outcome(false, true))), UpstreamClearResult::DryRun);
        let status = |status: u16| {
            Err(ChatError::Status { status, body: String::new() }.with_provider("Google Chat"))
        };
        assert_eq!(classify(&status(409)), UpstreamClearResult::Unavailable);
        assert_eq!(classify(&status(404)), UpstreamClearResult::Skipped);
        assert_eq!(classify(&status(502)), UpstreamClearResult::Failed);
        assert_eq!(classify(&status(405)), UpstreamClearResult::Failed);
        assert_eq!(
            classify(&Err(ChatError::Transport("reset".to_owned()))),
            UpstreamClearResult::Failed
        );
    }

    #[test]
    fn a_message_whose_instant_cannot_be_read_is_never_a_candidate() {
        let candidates = Candidates::new();
        let channel = ChannelId("1111111111".to_owned());
        candidates.set_mode(&channel, UpstreamClearMode::On, None);
        let fake = FakeDiscord::new();
        let id = fake.seed(&channel, "agent", "hello");
        let mut message = fake.message(&id).expect("seeded");
        message.timestamp = "yesterday".to_owned();
        assert!(!candidates.offer(&channel, &message));
        assert!(!candidates.status(&channel).pending);
    }

    /// A clearer over the fake provider and fake store, every channel eligible.
    fn clearer(fake: &Arc<FakeDiscord>, store: &Arc<FakeStore>) -> Clearer {
        Clearer {
            chat: fake.clone(),
            store: store.clone(),
            candidates: Arc::new(Candidates::new()),
            timing: QUICK,
            label: Arc::new(|_| Some("agents".to_owned())),
            eligible: Arc::new(|_| true),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_channel_that_is_off_offers_nothing() {
        let fake = Arc::new(FakeDiscord::new());
        fake.enable_upstream_clear();
        let store = Arc::new(FakeStore::new());
        let clearer = clearer(&fake, &store);
        let channel = ChannelId("1111111111".to_owned());
        let id = fake.seed(&channel, "agent", "one");
        let message = fake.message(&id).expect("seeded");
        assert!(!clearer.candidates.offer(&channel, &message));
        tokio::time::advance(Duration::from_secs(120)).await;
        assert!(!clearer.push_due().await);
        assert!(fake.upstream_clears().is_empty());
    }
}
