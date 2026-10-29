//! Reply reminders: what `chat run` records about the reminders it sends a Claude Code
//! coordinator for delivered requests it has not answered, and how it hands one to the
//! coordinator's queue. `chat_service::reply_reminders` decides when one is due.
//!
//! A reply the coordinator writes only in its thinking never reaches the screen, so the bridge
//! posts nothing and the owner sees silence (#292 reply-in-thinking-block). A reminder tells the
//! coordinator, once for each such request, that no reply arrived. Every rule that keeps it to
//! once lives in `reply-reminders.json`:
//!
//! * the time reminders started for this state: no request whose prompt was typed by then is
//!   ever reminded, so a backlog unanswered before reminders were deployed gets none;
//! * each request reminded, recorded before its reminder is queued, so neither a crash nor a
//!   restart reminds it twice;
//! * the reminder itself, until the queue has settled it, so a reminder that may have been typed
//!   is never typed again, and one still in the queue's inbox is taken back out.

use std::collections::BTreeSet;
use std::env;
use std::io;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    agent, current_reply_id, format_age, guard_typing, read_document, valid_key, write_document,
    BridgeState, ChatRuntimeError, CoordinatorDelivery, DrainOptions, QueueMessageState,
    ReplyAliasRecord, RequestPhase, Result, MAX_REQUESTS,
};

/// The record of reply reminders in the bridge state directory.
pub(crate) const REPLY_REMINDER_FILE: &str = "reply-reminders.json";
const REPLY_REMINDER_SCHEMA: &str = "agentctl-chat-reply-reminders/v1";
/// A request whose prompt was typed longer ago than this is never reminded: by then the owner
/// has most likely asked again, and a coordinator back from an outage does not get a reminder
/// for every request it missed.
pub(crate) const MAX_REPLY_REMINDER_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// The longest a reminder waits for the coordinator to be ready for it, or `--ready-timeout` if
/// that is shorter. One the queue has not typed by then is taken back out of its inbox.
pub(crate) const REPLY_REMINDER_READY_TIMEOUT: Duration = Duration::from_secs(10);
/// Most reply IDs one reminder names; it counts the rest, and reminds them too.
pub(crate) const MAX_REMINDER_LISTED_IDS: usize = 32;
/// Most requests the record holds as reminded. Each write keeps only requests the state still
/// retains, and a state retains at most `MAX_REQUESTS`.
const MAX_REMINDED_REQUESTS: usize = MAX_REQUESTS as usize;
const MAX_REMINDER_REPLY_ID_BYTES: usize = 64;
const MAX_REPLY_REMINDER_PROMPT_BYTES: usize = 16 * 1_024;
const MAX_REPLY_REMINDER_BYTES: usize = 1_024 * 1_024;
const REMINDER_MESSAGE_PREFIX: &str = "chat-reminder-";

/// `reply-reminders.json`: when reminders started, which requests were reminded, and the
/// reminder the queue has not settled yet, if any.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplyReminderRecord {
    schema: String,
    /// When reminders started for this state, in milliseconds since the Unix epoch.
    pub(crate) started_at_millis: u64,
    /// Every request reminded, once each, that the state still retained at the last write.
    pub(crate) reminded: Vec<RemindedRequest>,
    /// The reminder handed to the queue that the queue has not settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending: Option<PendingReminder>,
}

/// One request a reminder named.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemindedRequest {
    pub(crate) key: String,
    /// The reply ID the reminder named it by.
    pub(crate) reply_id: String,
    /// When it was recorded as reminded, in milliseconds since the Unix epoch.
    pub(crate) at_millis: u64,
}

/// A reminder recorded before it was queued, until the queue settles it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PendingReminder {
    /// Its identifier in the coordinator's queue.
    pub(crate) message_id: String,
    /// The requests it reminds.
    pub(crate) keys: Vec<String>,
    /// Its exact text.
    pub(crate) prompt: String,
}

fn valid_reply_id(reply_id: &str) -> bool {
    !reply_id.is_empty()
        && reply_id.len() <= MAX_REMINDER_REPLY_ID_BYTES
        && reply_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_message_id(message_id: &str) -> bool {
    message_id
        .strip_prefix(REMINDER_MESSAGE_PREFIX)
        .is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
}

impl ReplyReminderRecord {
    fn new(started_at_millis: u64) -> Self {
        Self {
            schema: REPLY_REMINDER_SCHEMA.to_owned(),
            started_at_millis,
            reminded: Vec::new(),
            pending: None,
        }
    }

    fn validate(&self) -> Result<()> {
        if self.schema != REPLY_REMINDER_SCHEMA {
            return Err(ChatRuntimeError::invalid(format!(
                "unsupported schema {:?}",
                self.schema
            )));
        }
        let mut keys = BTreeSet::new();
        let reminded_valid = self.reminded.len() <= MAX_REMINDED_REQUESTS
            && self.reminded.iter().all(|request| {
                valid_key(&request.key)
                    && valid_reply_id(&request.reply_id)
                    && keys.insert(request.key.as_str())
            });
        if !reminded_valid {
            return Err(ChatRuntimeError::invalid(
                "its reminded requests are malformed, repeated or too many",
            ));
        }
        if let Some(pending) = &self.pending {
            let mut named = BTreeSet::new();
            let valid = valid_message_id(&pending.message_id)
                && !pending.keys.is_empty()
                && pending
                    .keys
                    .iter()
                    .all(|key| keys.contains(key.as_str()) && named.insert(key.as_str()))
                && !pending.prompt.is_empty()
                && pending.prompt.len() <= MAX_REPLY_REMINDER_PROMPT_BYTES;
            if !valid {
                return Err(ChatRuntimeError::invalid(
                    "its pending reminder is malformed or names a request it does not record as \
reminded",
                ));
            }
        }
        Ok(())
    }

    /// Whether `key` has been reminded.
    pub(crate) fn reminded(&self, key: &str) -> bool {
        self.reminded.iter().any(|request| request.key == key)
    }
}

/// A request that may be due a reminder: open, its prompt recorded as typed, and with no stored
/// reply. Whether it is due also depends on the record and on the coordinator's idle time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReminderCandidate {
    pub(crate) key: String,
    /// The reply ID its prompt gave.
    pub(crate) reply_id: String,
    /// When its prompt was recorded as typed, in milliseconds since the Unix epoch.
    pub(crate) delivered_at_millis: u64,
}

/// The requests a reminder may name, and every request key the state retains.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ReminderCandidates {
    /// Newest delivery first.
    pub(crate) candidates: Vec<ReminderCandidate>,
    pub(crate) retained: BTreeSet<String>,
}

impl ReminderCandidates {
    /// The candidates `record` lets a reminder name at `now_millis`: delivered after reminders
    /// started and no more than `MAX_REPLY_REMINDER_AGE` ago, and never reminded.
    pub(crate) fn eligible(
        &self,
        record: &ReplyReminderRecord,
        now_millis: u64,
    ) -> Vec<ReminderCandidate> {
        let max_age = u64::try_from(MAX_REPLY_REMINDER_AGE.as_millis()).unwrap_or(u64::MAX);
        self.candidates
            .iter()
            .filter(|candidate| {
                candidate.delivered_at_millis > record.started_at_millis
                    && now_millis.saturating_sub(candidate.delivered_at_millis) <= max_age
                    && !record.reminded(&candidate.key)
            })
            .cloned()
            .collect()
    }
}

/// What became of a reminder handed to the coordinator's queue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ReminderOutcome {
    /// The queue typed it.
    Typed,
    /// The queue had typed it before this look.
    AlreadyTyped,
    /// It may have been typed, so it is never typed again; its requests stay reminded.
    Uncertain(String),
    /// It was taken back out of the queue untyped, or never reached it, so its requests can be
    /// reminded again.
    Withdrawn(String),
    /// It is still in the queue's inbox, which could not give it back; it stays recorded.
    Waiting(String),
}

impl ReminderOutcome {
    /// Whether the reminder may have reached the coordinator's pane, and so started a turn. Only
    /// one taken back out of the queue untyped, or never queued, did not. One still waiting in
    /// the queue counts, since any drain may type it before a check takes it out.
    pub(crate) fn may_have_typed(&self) -> bool {
        !matches!(self, Self::Withdrawn(_))
    }

    /// What the log says became of the reminder for each request it named.
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Typed => "reached the coordinator".to_owned(),
            Self::AlreadyTyped => "had already reached the coordinator".to_owned(),
            Self::Uncertain(why) => {
                format!("may have reached the coordinator and is not sent again: {why}")
            }
            Self::Withdrawn(why) => format!(
                "was taken back out of the coordinator queue before it was typed, or never \
reached it, and its request can be reminded again: {why}"
            ),
            Self::Waiting(why) => format!(
                "waits in the coordinator queue, which could not give it back; a later check \
takes it out: {why}"
            ),
        }
    }
}

impl BridgeState {
    fn reply_reminder_path(&self) -> std::path::PathBuf {
        self.root.join(REPLY_REMINDER_FILE)
    }

    /// `reply-reminders.json`, or `None` when it does not exist.
    fn read_reply_reminders(&self) -> Result<Option<ReplyReminderRecord>> {
        let path = self.reply_reminder_path();
        let unusable = |error: ChatRuntimeError| {
            ChatRuntimeError::invalid(format!(
                "reply reminder record {} is unusable: {error}; no reply reminder is sent until \
an operator repairs or deletes it, and deleting it reminds no request twice, since reminders \
then start again from that time",
                path.display()
            ))
        };
        let record: ReplyReminderRecord = match read_document(&path, MAX_REPLY_REMINDER_BYTES) {
            Ok(record) => record,
            Err(ChatRuntimeError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(unusable(error)),
        };
        record.validate().map_err(unusable)?;
        Ok(Some(record))
    }

    /// Read `reply-reminders.json` under the state lock and pass it to `change`, creating it
    /// first, with reminders starting at `now_millis`, when it does not exist. Writes the record
    /// back when `change` returns `true` or the record was created. Before it is written, every
    /// reminded request that `retained` does not hold, or that was reminded more than
    /// `MAX_REPLY_REMINDER_AGE` ago, is dropped: neither can be reminded again. A request
    /// reminded that long ago was delivered longer ago still, so the age limit alone already
    /// excludes it.
    pub(crate) fn update_reply_reminders<T>(
        &self,
        now_millis: u64,
        retained: Option<&BTreeSet<String>>,
        change: impl FnOnce(&mut ReplyReminderRecord) -> (bool, T),
    ) -> Result<(ReplyReminderRecord, T)> {
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        fs2::FileExt::lock_exclusive(&state_lock).map_err(ChatRuntimeError::Io)?;
        let (mut record, created) = match self.read_reply_reminders()? {
            Some(record) => (record, false),
            None => (ReplyReminderRecord::new(now_millis), true),
        };
        let (changed, value) = change(&mut record);
        if changed || created {
            let max_age = u64::try_from(MAX_REPLY_REMINDER_AGE.as_millis()).unwrap_or(u64::MAX);
            let pending: BTreeSet<String> = record
                .pending
                .iter()
                .flat_map(|pending| pending.keys.iter().cloned())
                .collect();
            record.reminded.retain(|request| {
                pending.contains(&request.key)
                    || (retained.is_none_or(|retained| retained.contains(&request.key))
                        && now_millis.saturating_sub(request.at_millis) <= max_age)
            });
            record.validate()?;
            write_document(&self.reply_reminder_path(), &record)?;
        }
        Ok((record, value))
    }

    /// The open requests whose prompts are recorded as typed and that hold no reply, a reply
    /// the post-rate breaker holds back included, newest delivery first, each with the reply ID
    /// its prompt gave; and the key of every request the state retains.
    pub(crate) fn reply_reminder_candidates(&self) -> Result<ReminderCandidates> {
        // A prompt typed before the bridge was repointed went to the previous agent, which the
        // current one cannot answer for (#677 chat-bridge-repoint).
        let retargeted_at = self.retargeted_at_millis().unwrap_or(0);
        let (records, alias_record) = {
            let _snapshot = self.lock_state_snapshot()?;
            (self.request_records()?, self.read_reply_aliases().ok())
        };
        let aliases = alias_record
            .as_ref()
            .map(ReplyAliasRecord::by_key)
            .unwrap_or_default();
        let mut found = ReminderCandidates::default();
        for (record, _) in records {
            found.retained.insert(record.key.clone());
            if record.reply_closed
                || record.reply_count > 0
                || record.phase != RequestPhase::Delivered
            {
                continue;
            }
            let Some(delivered_at_millis) = record
                .delivered_at_millis
                .filter(|delivered| *delivered >= retargeted_at)
            else {
                continue;
            };
            found.candidates.push(ReminderCandidate {
                reply_id: current_reply_id(&record, aliases.get(record.key.as_str()).copied()),
                key: record.key,
                delivered_at_millis,
            });
        }
        found.candidates.sort_by(|left, right| {
            right
                .delivered_at_millis
                .cmp(&left.delivered_at_millis)
                .then_with(|| left.key.cmp(&right.key))
        });
        Ok(found)
    }

    /// The text of a reminder of `requests`, newest delivery first, at `now_millis`. It names up
    /// to `MAX_REMINDER_LISTED_IDS` reply IDs and counts the rest. It tells the coordinator to
    /// check what the bridge holds before it sends anything, printing for each request it names
    /// the `agentctl chat sent --request` command that lists that request's replies, since a reply
    /// printed shortly before the reminder can be captured just after it was typed (#260
    /// reminder-capture-race). With `chat run --offer-reply-command` it also prints, for each
    /// request it names, the `agentctl chat reply` command that request's prompt offered. A
    /// command that cannot be printed, or a text longer than its bound, falls back to general
    /// terms. It spells the marker lines out in parts, so that no row of it forms a marker
    /// however the terminal wraps it.
    pub(crate) fn reminder_prompt(
        &self,
        requests: &[ReminderCandidate],
        now_millis: u64,
    ) -> String {
        let named = &requests[..requests.len().min(MAX_REMINDER_LISTED_IDS)];
        let more = requests.len() - named.len();
        let listed = named
            .iter()
            .map(|request| {
                format!(
                    "{} (sent to you {})",
                    request.reply_id,
                    format_age(now_millis, request.delivered_at_millis)
                )
            })
            .collect::<Vec<_>>();
        let subject = if requests.len() == 1 {
            format!("the chat request with reply ID {}", listed.join(""))
        } else {
            let mut ids = listed.join(", ");
            if more > 0 {
                ids.push_str(&format!(", and {more} more"));
            }
            format!("the chat requests with reply IDs {ids}")
        };
        let lead = format!(
            "Chat reply reminder: the chat bridge has received no reply from you to {subject}. A \
reply written only in your thinking is not delivered: the bridge reads only the ordinary text you \
print. If you meant to reply, print the reply as ordinary text between two marker lines: an \
opening line made of the literal prefix `<CHAT_REPLY_`, the reply ID and `>`, and a closing line \
made of `</CHAT_REPLY_`, the same ID and `>`, each alone on its line and outside code fences."
        );
        let check_lead = "Before you send anything, check whether the bridge already holds a \
reply: one you printed shortly before this reminder can still be captured just after it was typed.";
        let general_check = format!(
            "{check_lead} `agentctl chat sent --bridge-state DIR --request KEY` lists the replies the \
bridge holds for a request; a reply it lists reached the bridge, so do not send that reply again."
        );
        let close = "If a request needs no reply, you answered it another way, or you are still \
working on it, ignore this reminder; the bridge sends it only once for each request.";
        let listed_commands = |command: &dyn Fn(&ReminderCandidate) -> Option<String>| {
            named
                .iter()
                .map(|request| command(request).map(|line| format!("{}: {line}", request.reply_id)))
                .collect::<Option<Vec<_>>>()
                .map(|lines| lines.join("\n"))
        };
        let check = listed_commands(&|request| self.sent_command(&request.key)).map_or_else(
            || general_check.clone(),
            |lines| {
                format!(
                    "{check_lead} Run this command for each request; a reply it lists reached the \
bridge, so do not send that reply again:\n{lines}\n"
                )
            },
        );
        let generic_with = |check: &str| {
            let separator = if check.ends_with('\n') { "" } else { " " };
            format!(
                "{lead} {check}{separator}If the request's prompt printed an `agentctl chat reply` \
command, you can instead write the reply to a file and send it with that command. {close}"
            )
        };
        let bounded_generic = || {
            let generic = generic_with(&check);
            if generic.len() <= MAX_REPLY_REMINDER_PROMPT_BYTES {
                generic
            } else {
                generic_with(&general_check)
            }
        };
        if !self.offer_reply_command {
            return bounded_generic();
        }
        let Some(commands) = listed_commands(&|request| {
            self.reply_command(env::current_exe(), &request.key, &request.reply_id)
        }) else {
            return bounded_generic();
        };
        let separator = if check.ends_with('\n') { "" } else { " " };
        let offered = format!(
            "{lead} {check}{separator}Otherwise you can write the reply to a file, without the two \
lines, and send it with `agentctl chat reply`, putting the file's path in place of \
PATH_TO_YOUR_REPLY:\n{commands}\n{close}"
        );
        if offered.len() <= MAX_REPLY_REMINDER_PROMPT_BYTES {
            offered
        } else {
            bounded_generic()
        }
    }
}

/// A reminder of `requests`, composed at `now_millis`, ready to be recorded and queued.
pub(crate) fn compose_reminder(
    state: &BridgeState,
    requests: &[ReminderCandidate],
    now_millis: u64,
) -> PendingReminder {
    let mut keys = requests
        .iter()
        .map(|request| request.key.clone())
        .collect::<Vec<_>>();
    keys.sort();
    let mut digest = Sha256::new();
    for key in &keys {
        digest.update(key.as_bytes());
        digest.update(b"\n");
    }
    digest.update(now_millis.to_string().as_bytes());
    PendingReminder {
        message_id: format!("{REMINDER_MESSAGE_PREFIX}{:x}", digest.finalize()),
        keys: requests.iter().map(|request| request.key.clone()).collect(),
        prompt: state.reminder_prompt(requests, now_millis),
    }
}

/// The options a reminder is typed with: `options`, waiting no longer than
/// `REPLY_REMINDER_READY_TIMEOUT` for the coordinator, and never typing into a working pane,
/// whatever `chat run --deliver-while-working` says, since a reminder is due only while the
/// coordinator is idle.
pub(crate) fn reminder_options(options: DrainOptions) -> DrainOptions {
    DrainOptions {
        ready_timeout: options.ready_timeout.min(REPLY_REMINDER_READY_TIMEOUT),
        deliver_while_working: false,
        ..options
    }
}

/// Hand the recorded reminder `reminder` to the coordinator's queue for the first time and say
/// what became of it. The pane is read first, as before any queued prompt is typed (see
/// [`guard_typing`]); if that fails, nothing is queued. The queue then types it only if the
/// coordinator is ready within `reminder_options`' wait, and a reminder it does not type is
/// taken back out of its inbox, as [`CoordinatorDelivery::submit_once`] describes.
pub(crate) fn deliver_reminder(
    state: &BridgeState,
    delivery: &dyn CoordinatorDelivery,
    reminder: &PendingReminder,
    options: DrainOptions,
) -> ReminderOutcome {
    if let Err(why) = guard_typing(state, delivery, None) {
        return ReminderOutcome::Withdrawn(why);
    }
    let agent_name = &state.config.agent_name;
    match delivery.submit_once(
        agent_name,
        &reminder.prompt,
        &reminder.message_id,
        reminder_options(options),
        &|_| {},
    ) {
        Ok(None) => ReminderOutcome::Typed,
        Ok(Some(why)) => ReminderOutcome::Withdrawn(why),
        Err(why) => match delivery.message_state(agent_name, &reminder.message_id) {
            Ok(Some(QueueMessageState::Processed)) => ReminderOutcome::Typed,
            Ok(Some(QueueMessageState::Inflight | QueueMessageState::Failed)) => {
                ReminderOutcome::Uncertain(why)
            }
            Ok(Some(QueueMessageState::Pending)) => ReminderOutcome::Waiting(why),
            Ok(None) => ReminderOutcome::Withdrawn(why),
            // A tombstone or expiry set it aside: it was never typed and never will be.
            Ok(Some(QueueMessageState::Cancelled)) => ReminderOutcome::Withdrawn(why),
            Err(read) => ReminderOutcome::Waiting(format!(
                "{why}; the coordinator queue could not be read afterwards: {read}"
            )),
        },
    }
}

/// Settle a reminder recorded as handed to the queue earlier, by this process or one before a
/// restart, and say what became of it. One still in the queue's inbox is taken back out. One the
/// queue has typed, or may have begun to type, is settled and never typed again. One the queue
/// does not hold may have been typed into a queue replaced since, or have been taken back out
/// just before a crash, so it too is settled and never typed again.
pub(crate) fn settle_reminder(
    state: &BridgeState,
    delivery: &dyn CoordinatorDelivery,
    reminder: &PendingReminder,
) -> ReminderOutcome {
    let agent_name = &state.config.agent_name;
    match delivery.message_state(agent_name, &reminder.message_id) {
        Ok(Some(QueueMessageState::Processed)) => ReminderOutcome::AlreadyTyped,
        Ok(Some(QueueMessageState::Inflight | QueueMessageState::Failed)) => {
            ReminderOutcome::Uncertain("the coordinator queue began to type it".to_owned())
        }
        Ok(None) => ReminderOutcome::Uncertain(
            "the coordinator queue no longer holds it, so it may have been typed".to_owned(),
        ),
        Ok(Some(QueueMessageState::Cancelled)) => {
            ReminderOutcome::Withdrawn("the coordinator queue set it aside untyped".to_owned())
        }
        Ok(Some(QueueMessageState::Pending)) => {
            match delivery.withdraw(agent_name, &reminder.message_id) {
                Ok(true) => ReminderOutcome::Withdrawn(
                    "an earlier check left it in the coordinator queue".to_owned(),
                ),
                Ok(false) => match delivery.message_state(agent_name, &reminder.message_id) {
                    Ok(Some(QueueMessageState::Processed)) => ReminderOutcome::AlreadyTyped,
                    Ok(Some(QueueMessageState::Pending)) => ReminderOutcome::Waiting(
                        "the coordinator queue still holds it after taking it out".to_owned(),
                    ),
                    Ok(_) => ReminderOutcome::Uncertain(
                        "the coordinator queue began to type it while it was taken back".to_owned(),
                    ),
                    Err(why) => ReminderOutcome::Waiting(why),
                },
                Err(why) => ReminderOutcome::Waiting(why),
            }
        }
        Err(why) => ReminderOutcome::Waiting(why),
    }
}

/// Record what became of the reminder `reminder` at `now_millis`: a settled one leaves the
/// record, a withdrawn one also releases its requests, and one still waiting stays recorded.
pub(crate) fn record_reminder_outcome(
    state: &BridgeState,
    reminder: &PendingReminder,
    outcome: &ReminderOutcome,
    now_millis: u64,
    retained: Option<&BTreeSet<String>>,
) -> Result<ReplyReminderRecord> {
    state
        .update_reply_reminders(now_millis, retained, |record| {
            if matches!(outcome, ReminderOutcome::Waiting(_)) {
                return (false, ());
            }
            if record
                .pending
                .as_ref()
                .is_some_and(|pending| pending.message_id == reminder.message_id)
            {
                record.pending = None;
            }
            if matches!(outcome, ReminderOutcome::Withdrawn(_)) {
                record
                    .reminded
                    .retain(|request| !reminder.keys.contains(&request.key));
            }
            (true, ())
        })
        .map(|(record, ())| record)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    use chat_subscription::{
        ChannelId, CommittableEvent, DeliveryBatch, DeliveryId, EventSequence, InboundMessage,
        MessageId, ProviderCursor, SenderId, ThreadId,
    };

    use super::super::{
        unix_millis, BridgeConfiguration, CoordinatorDeliveryResult, ReactionSetting,
    };
    use super::*;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    /// A new bridge state for the agent `coordinator`, with outbound chat on, in a private
    /// directory of its own.
    fn state(name: &str) -> (PathBuf, BridgeState) {
        let root = std::env::temp_dir().join(format!(
            "agentctl-reply-reminders-{name}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("state directory");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("private state");
        let state = BridgeState::initialize(
            &root,
            BridgeConfiguration {
                subscription_plugin: "fixture".to_owned(),
                subscription_environment: Vec::new(),
                channel_ids: vec!["spaces/example".to_owned()],
                allowed_senders: vec!["users/owner".to_owned()],
                max_grant_seconds: crate::chat_runtime::DEFAULT_MAX_GRANT_SECONDS,
                agent_name: "coordinator".to_owned(),
                agent_label: "coordinator".to_owned(),
                outbound_enabled: true,
                ack_reaction: ReactionSetting::Disabled,
                receipt_reaction: ReactionSetting::Inherit,
                reject_reaction: ReactionSetting::Inherit,
                backend_configuration: None,
                outbound_command: None,
                reply_alias_range: None,
                slack_format: Some(crate::chat_runtime::SlackFormat::Markdown),
                slack_format_by_channel: std::collections::BTreeMap::new(),
            },
        )
        .expect("initialize state");
        (root, state)
    }

    /// Admit the owner's message number `index` as provider event `index`; its request's key.
    fn admit(state: &BridgeState, index: u64) -> String {
        let message = InboundMessage::new(
            ChannelId::new("spaces/example").expect("channel"),
            MessageId::new(format!("spaces/example/messages/{index}")).expect("message"),
            ThreadId::new(format!("spaces/example/threads/{index}")).expect("thread"),
            SenderId::new("users/owner").expect("sender"),
            format!("request {index}"),
            "2026-10-14T12:00:00Z",
            false,
        )
        .expect("message");
        let batch = DeliveryBatch::new(
            EventSequence::new(index).expect("sequence"),
            ProviderCursor::new(format!("cursor-{index}")).expect("cursor"),
            DeliveryId::new(format!("delivery-{index}")).expect("delivery"),
            vec![CommittableEvent::message_created(message)],
        )
        .expect("batch");
        state
            .admit_batch(&batch)
            .expect("admit")
            .new_request_keys
            .remove(0)
    }

    /// A coordinator queue for reminders, which settles each prompt as `outcome` says.
    #[derive(Default)]
    struct ReminderQueue {
        states: Mutex<BTreeMap<String, QueueMessageState>>,
        /// Every prompt typed, with its message ID.
        typed: Mutex<Vec<(String, String)>>,
        /// The options each `submit_once` was given.
        options: Mutex<Vec<DrainOptions>>,
        /// What `submit_once` does with the next prompt.
        outcome: Mutex<Queueing>,
        /// What the reminder record held when the queue was first asked to type.
        record_at_submit: Mutex<Option<String>>,
        root: Mutex<Option<PathBuf>>,
    }

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    enum Queueing {
        /// The coordinator is ready: the prompt is typed.
        #[default]
        Typed,
        /// The coordinator is not ready in time: the prompt is taken back out untyped.
        NotReady,
        /// The coordinator is not ready, and the prompt cannot be taken back out.
        Stuck,
        /// Typing began and failed: the prompt may have reached the coordinator.
        Failed,
        /// The queue refuses the prompt before holding it.
        Refused,
    }

    impl CoordinatorDelivery for ReminderQueue {
        fn message_state(
            &self,
            _agent_name: &str,
            message_id: &str,
        ) -> std::result::Result<Option<QueueMessageState>, String> {
            Ok(self.states.lock().expect("states").get(message_id).copied())
        }

        fn submit(
            &self,
            _agent_name: &str,
            _prompt: &str,
            _message_id: &str,
            _options: DrainOptions,
        ) -> std::result::Result<(), String> {
            Err("a reminder is never submitted except through submit_once".to_owned())
        }

        fn drain(
            &self,
            _agent_name: &str,
            _options: DrainOptions,
        ) -> std::result::Result<(), String> {
            Err("a reminder never drains except through submit_once".to_owned())
        }

        fn screen(&self, _agent_name: &str) -> std::result::Result<String, String> {
            Ok(String::new())
        }

        fn submit_once(
            &self,
            _agent_name: &str,
            prompt: &str,
            message_id: &str,
            options: DrainOptions,
            _printed: &dyn Fn(&str),
        ) -> std::result::Result<Option<String>, String> {
            if let Some(root) = self.root.lock().expect("root").as_ref() {
                *self.record_at_submit.lock().expect("record") =
                    fs::read_to_string(root.join(REPLY_REMINDER_FILE)).ok();
            }
            self.options.lock().expect("options").push(options);
            let mut states = self.states.lock().expect("states");
            match *self.outcome.lock().expect("outcome") {
                Queueing::Typed => {
                    states.insert(message_id.to_owned(), QueueMessageState::Processed);
                    self.typed
                        .lock()
                        .expect("typed")
                        .push((message_id.to_owned(), prompt.to_owned()));
                    Ok(None)
                }
                Queueing::NotReady => Ok(Some("the coordinator was not ready in time".to_owned())),
                Queueing::Stuck => {
                    states.insert(message_id.to_owned(), QueueMessageState::Pending);
                    Err("the queue could not take the prompt back out".to_owned())
                }
                Queueing::Failed => {
                    states.insert(message_id.to_owned(), QueueMessageState::Failed);
                    Err("typing failed after it began".to_owned())
                }
                Queueing::Refused => Err("chat delivery was cancelled".to_owned()),
            }
        }

        fn withdraw(
            &self,
            _agent_name: &str,
            message_id: &str,
        ) -> std::result::Result<bool, String> {
            if *self.outcome.lock().expect("outcome") == Queueing::Stuck {
                return Err("the queue could not take the prompt back out".to_owned());
            }
            let mut states = self.states.lock().expect("states");
            if states.get(message_id) == Some(&QueueMessageState::Pending) {
                states.remove(message_id);
                Ok(true)
            } else {
                Ok(false)
            }
        }
    }

    /// Admit a request and record its prompt as typed, as a delivery the queue confirms does.
    fn delivered(state: &BridgeState, index: u64) -> String {
        let key = admit(state, index);
        assert_eq!(
            super::super::deliver_request_with(
                state,
                &TypingQueue::default(),
                &key,
                DrainOptions::default()
            )
            .expect("deliver"),
            CoordinatorDeliveryResult::Delivered
        );
        key
    }

    /// A queue that types every request prompt at once.
    #[derive(Default)]
    struct TypingQueue {
        states: Mutex<BTreeMap<String, QueueMessageState>>,
    }

    impl CoordinatorDelivery for TypingQueue {
        fn message_state(
            &self,
            _agent_name: &str,
            message_id: &str,
        ) -> std::result::Result<Option<QueueMessageState>, String> {
            Ok(self.states.lock().expect("states").get(message_id).copied())
        }

        fn submit(
            &self,
            _agent_name: &str,
            _prompt: &str,
            message_id: &str,
            _options: DrainOptions,
        ) -> std::result::Result<(), String> {
            self.states
                .lock()
                .expect("states")
                .insert(message_id.to_owned(), QueueMessageState::Processed);
            Ok(())
        }

        fn drain(
            &self,
            _agent_name: &str,
            _options: DrainOptions,
        ) -> std::result::Result<(), String> {
            Ok(())
        }

        fn screen(&self, _agent_name: &str) -> std::result::Result<String, String> {
            Ok(String::new())
        }
    }

    /// The reminder record, created with reminders starting at `started_at`.
    fn start(state: &BridgeState, started_at: u64) -> ReplyReminderRecord {
        state
            .update_reply_reminders(started_at, None, |_| (false, ()))
            .expect("reminder record")
            .0
    }

    /// Record `reminder` of `requests` as a check does before queuing it.
    fn record(
        state: &BridgeState,
        requests: &[ReminderCandidate],
        reminder: &PendingReminder,
        now: u64,
    ) {
        state
            .update_reply_reminders(now, None, |record| {
                for request in requests {
                    record.reminded.push(RemindedRequest {
                        key: request.key.clone(),
                        reply_id: request.reply_id.clone(),
                        at_millis: now,
                    });
                }
                record.pending = Some(reminder.clone());
                (true, ())
            })
            .expect("record the reminder");
    }

    #[test]
    fn reminders_start_when_their_record_is_created_so_older_requests_are_never_reminded() {
        let (root, state) = state("start");
        let before = delivered(&state, 1);
        std::thread::sleep(Duration::from_millis(5));
        let started_at = unix_millis();
        let record = start(&state, started_at);
        assert_eq!(record.started_at_millis, started_at);
        assert!(root.join(REPLY_REMINDER_FILE).exists());
        std::thread::sleep(Duration::from_millis(5));
        let after = delivered(&state, 2);
        let candidates = state.reply_reminder_candidates().expect("candidates");
        let keys = |eligible: Vec<ReminderCandidate>| {
            eligible
                .into_iter()
                .map(|candidate| candidate.key)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            keys(candidates.candidates.clone()),
            [after.clone(), before.clone()],
            "both are candidates, newest delivery first"
        );
        assert_eq!(
            keys(candidates.eligible(&record, unix_millis())),
            std::slice::from_ref(&after),
            "the request typed before reminders started is never reminded"
        );
        // The start time survives later reads, so a restart does not move it.
        let later = start(&state, unix_millis() + 60_000);
        assert_eq!(later.started_at_millis, started_at);
        // Deleting the record starts reminders again from that time.
        fs::remove_file(root.join(REPLY_REMINDER_FILE)).expect("delete the record");
        let restarted = start(&state, unix_millis() + 1);
        assert!(keys(candidates.eligible(&restarted, unix_millis())).is_empty());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn a_request_delivered_more_than_a_day_ago_is_never_reminded() {
        let day = u64::try_from(MAX_REPLY_REMINDER_AGE.as_millis()).expect("a day in ms");
        let candidates = ReminderCandidates {
            candidates: vec![ReminderCandidate {
                key: "a".repeat(64),
                reply_id: "001".to_owned(),
                delivered_at_millis: 1_000,
            }],
            retained: BTreeSet::new(),
        };
        let record = ReplyReminderRecord::new(0);
        assert_eq!(candidates.eligible(&record, 1_000 + day).len(), 1);
        assert!(candidates.eligible(&record, 1_000 + day + 1).is_empty());
    }

    #[test]
    fn only_an_open_request_whose_typed_prompt_has_no_reply_is_a_candidate() {
        let (root, state) = state("candidates");
        let waiting = admit(&state, 1);
        let answered = delivered(&state, 2);
        let open = delivered(&state, 3);
        // The newest request belongs to the current replay cursor, so closing it does not retire
        // it, and it stays among the records.
        let closed = delivered(&state, 4);
        let found = state.reply_reminder_candidates().expect("candidates");
        let reply_id = found
            .candidates
            .iter()
            .find(|candidate| candidate.key == answered)
            .expect("the answered request before its reply")
            .reply_id
            .clone();
        state
            .submit_reply(&answered, &reply_id, "the answer")
            .expect("store a reply");
        state.close_replies(&closed).expect("close");
        let found = state.reply_reminder_candidates().expect("candidates");
        assert_eq!(
            found
                .candidates
                .iter()
                .map(|candidate| candidate.key.as_str())
                .collect::<Vec<_>>(),
            [open.as_str()]
        );
        // The request never prompted got no reply ID, so the third request's is 002.
        assert_eq!(found.candidates[0].reply_id, "002");
        assert_eq!(
            found.retained,
            [waiting, answered, open, closed].into_iter().collect()
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn a_reminder_is_recorded_before_it_is_queued_so_its_requests_are_never_reminded_twice() {
        let (root, state) = state("recorded");
        start(&state, 1);
        let key = delivered(&state, 1);
        let now = unix_millis();
        let due = state
            .reply_reminder_candidates()
            .expect("candidates")
            .candidates;
        let reminder = compose_reminder(&state, &due, now);
        record(&state, &due, &reminder, now);
        let queue = ReminderQueue::default();
        *queue.root.lock().expect("root") = Some(root.clone());
        let outcome = deliver_reminder(&state, &queue, &reminder, DrainOptions::default());
        assert_eq!(outcome, ReminderOutcome::Typed);
        let at_submit: ReplyReminderRecord = serde_json::from_str(
            queue
                .record_at_submit
                .lock()
                .expect("record")
                .as_deref()
                .expect("the record existed when the queue was asked to type"),
        )
        .expect("record");
        assert_eq!(at_submit.pending.as_ref(), Some(&reminder));
        assert!(at_submit.reminded(&key));
        let settled = record_reminder_outcome(&state, &reminder, &outcome, now, None)
            .expect("record the outcome");
        assert_eq!(settled.pending, None);
        assert!(settled.reminded(&key), "the request stays reminded");
        let candidates = state.reply_reminder_candidates().expect("candidates");
        assert!(
            candidates.eligible(&settled, unix_millis()).is_empty(),
            "a request is reminded once"
        );
        assert_eq!(queue.typed.lock().expect("typed").len(), 1);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn a_reminder_waits_at_most_ten_seconds_and_never_types_into_a_working_pane() {
        let options = reminder_options(DrainOptions {
            ready_timeout: Duration::from_secs(900),
            deliver_while_working: true,
            ..DrainOptions::default()
        });
        assert_eq!(options.ready_timeout, REPLY_REMINDER_READY_TIMEOUT);
        assert_eq!(REPLY_REMINDER_READY_TIMEOUT, Duration::from_secs(10));
        assert!(!options.deliver_while_working);
        let shorter = reminder_options(DrainOptions {
            ready_timeout: Duration::ZERO,
            ..DrainOptions::default()
        });
        assert_eq!(shorter.ready_timeout, Duration::ZERO);
    }

    #[test]
    fn a_reminder_the_coordinator_is_not_ready_for_is_taken_back_and_its_requests_released() {
        for queueing in [Queueing::NotReady, Queueing::Refused] {
            let (root, state) = state("not-ready");
            start(&state, 1);
            let key = delivered(&state, 1);
            let now = unix_millis();
            let due = state
                .reply_reminder_candidates()
                .expect("candidates")
                .candidates;
            let reminder = compose_reminder(&state, &due, now);
            record(&state, &due, &reminder, now);
            let queue = ReminderQueue::default();
            *queue.outcome.lock().expect("outcome") = queueing;
            let outcome = deliver_reminder(
                &state,
                &queue,
                &reminder,
                DrainOptions {
                    deliver_while_working: true,
                    ..DrainOptions::default()
                },
            );
            assert!(
                matches!(outcome, ReminderOutcome::Withdrawn(_)),
                "{queueing:?}: {outcome:?}"
            );
            let options = queue.options.lock().expect("options")[0];
            assert_eq!(options.ready_timeout, REPLY_REMINDER_READY_TIMEOUT);
            assert!(!options.deliver_while_working);
            let released = record_reminder_outcome(&state, &reminder, &outcome, now, None)
                .expect("record the outcome");
            assert_eq!(released.pending, None);
            assert!(!released.reminded(&key), "{queueing:?}");
            assert!(queue.typed.lock().expect("typed").is_empty());
            fs::remove_dir_all(root).expect("cleanup");
        }
    }

    #[test]
    fn a_reminder_that_may_have_been_typed_is_never_typed_again() {
        let (root, state) = state("uncertain");
        start(&state, 1);
        let key = delivered(&state, 1);
        let now = unix_millis();
        let due = state
            .reply_reminder_candidates()
            .expect("candidates")
            .candidates;
        let reminder = compose_reminder(&state, &due, now);
        record(&state, &due, &reminder, now);
        let queue = ReminderQueue::default();
        *queue.outcome.lock().expect("outcome") = Queueing::Failed;
        let outcome = deliver_reminder(&state, &queue, &reminder, DrainOptions::default());
        assert!(
            matches!(outcome, ReminderOutcome::Uncertain(_)),
            "{outcome:?}"
        );
        let settled = record_reminder_outcome(&state, &reminder, &outcome, now, None)
            .expect("record the outcome");
        assert_eq!(settled.pending, None);
        assert!(settled.reminded(&key));
        // Settling a recorded reminder the queue holds in flight, failed, or not at all never
        // types it again either.
        for held in [
            Some(QueueMessageState::Inflight),
            Some(QueueMessageState::Failed),
            None,
        ] {
            let queue = ReminderQueue::default();
            if let Some(held) = held {
                queue
                    .states
                    .lock()
                    .expect("states")
                    .insert(reminder.message_id.clone(), held);
            }
            assert!(
                matches!(
                    settle_reminder(&state, &queue, &reminder),
                    ReminderOutcome::Uncertain(_)
                ),
                "{held:?}"
            );
            assert!(queue.typed.lock().expect("typed").is_empty());
        }
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn a_recorded_reminder_still_queued_is_taken_out_and_one_that_cannot_be_stays_recorded() {
        let (root, state) = state("settle");
        start(&state, 1);
        let key = delivered(&state, 1);
        let now = unix_millis();
        let due = state
            .reply_reminder_candidates()
            .expect("candidates")
            .candidates;
        let reminder = compose_reminder(&state, &due, now);
        record(&state, &due, &reminder, now);

        let stuck = ReminderQueue::default();
        *stuck.outcome.lock().expect("outcome") = Queueing::Stuck;
        stuck
            .states
            .lock()
            .expect("states")
            .insert(reminder.message_id.clone(), QueueMessageState::Pending);
        let outcome = settle_reminder(&state, &stuck, &reminder);
        assert!(
            matches!(outcome, ReminderOutcome::Waiting(_)),
            "{outcome:?}"
        );
        let kept = record_reminder_outcome(&state, &reminder, &outcome, now, None)
            .expect("record the outcome");
        assert_eq!(kept.pending.as_ref(), Some(&reminder), "it stays recorded");

        let queued = ReminderQueue::default();
        queued
            .states
            .lock()
            .expect("states")
            .insert(reminder.message_id.clone(), QueueMessageState::Pending);
        let outcome = settle_reminder(&state, &queued, &reminder);
        assert!(
            matches!(outcome, ReminderOutcome::Withdrawn(_)),
            "{outcome:?}"
        );
        assert_eq!(
            queued
                .states
                .lock()
                .expect("states")
                .get(&reminder.message_id),
            None,
            "taken out of the queue"
        );
        let released = record_reminder_outcome(&state, &reminder, &outcome, now, None)
            .expect("record the outcome");
        assert_eq!(released.pending, None);
        assert!(!released.reminded(&key));

        let typed = ReminderQueue::default();
        typed
            .states
            .lock()
            .expect("states")
            .insert(reminder.message_id.clone(), QueueMessageState::Processed);
        assert_eq!(
            settle_reminder(&state, &typed, &reminder),
            ReminderOutcome::AlreadyTyped
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn the_reminder_names_each_reply_id_and_points_to_agentctl_chat_reply() {
        let (root, state) = state("text");
        let now = 10_000_000;
        let request = |index: u64, delivered_at_millis: u64| ReminderCandidate {
            key: format!("{index:064x}"),
            reply_id: format!("{index:03}"),
            delivered_at_millis,
        };
        let one = state.reminder_prompt(&[request(1, now - 180_000)], now);
        assert!(
            one.starts_with(
                "Chat reply reminder: the chat bridge has received no reply from you to the chat \
request with reply ID 001 (sent to you 3m ago)."
            ),
            "{one}"
        );
        assert!(
            one.contains("written only in your thinking is not delivered"),
            "{one}"
        );
        assert!(one.contains("`agentctl chat reply`"), "{one}");
        assert!(one.contains("only once for each request"), "{one}");
        // It says to check what the bridge holds before sending anything, with the exact
        // command for the request (#260 reminder-capture-race).
        assert!(
            one.contains(
                "Before you send anything, check whether the bridge already holds a reply"
            ),
            "{one}"
        );
        let check = one
            .lines()
            .find(|line| line.starts_with("001: "))
            .expect("the check command line");
        assert!(check.contains(" chat sent --bridge-state "), "{check}");
        assert!(check.ends_with(&format!("--request {:064x}", 1)), "{check}");
        // No row of it is a marker line, however the terminal wraps it, and it names no reply
        // ID after a marker prefix, which a capture would read as a block or an unknown ID.
        assert!(!one.contains("CHAT_REPLY_0"), "{one}");
        assert!(
            one.lines()
                .all(|line| !line.trim_start().starts_with("<CHAT")),
            "{one}"
        );

        let many = (1..=40)
            .map(|index| request(index, now - index * 60_000))
            .collect::<Vec<_>>();
        let listed = state.reminder_prompt(&many, now);
        assert!(
            listed.contains("reply IDs 001 (sent to you 1m ago), 002"),
            "{listed}"
        );
        assert!(
            listed.contains("032 (sent to you 32m ago), and 8 more."),
            "{listed}"
        );
        // Only the named requests are listed, each with its check command.
        let (head, commands) = listed
            .split_once("so do not send that reply again:\n")
            .expect("the check commands");
        assert!(!head.contains("033"), "{listed}");
        assert_eq!(
            commands
                .lines()
                .take_while(|line| line.contains(" chat sent "))
                .count(),
            32,
            "{listed}"
        );
        assert!(
            !commands.lines().any(|line| line.starts_with("033: ")),
            "{listed}"
        );

        // With `chat run --offer-reply-command`, each named request's command is printed.
        let offering = state.clone().with_reply_command_offered(true);
        let offered = offering.reminder_prompt(&[request(7, now - 60_000)], now);
        assert!(
            offered.contains("send it with `agentctl chat reply`"),
            "{offered}"
        );
        let command = offered
            .lines()
            .find(|line| line.starts_with("007: ") && line.contains(" chat reply "))
            .expect("the command line");
        assert!(command.contains(" chat reply --bridge-state "), "{command}");
        assert!(
            command.contains(&format!("--request {:064x}", 7)),
            "{command}"
        );
        assert!(
            command.contains("--reply-id 007 --file PATH_TO_YOUR_REPLY"),
            "{command}"
        );
        // The check comes before the offer to send.
        let check = offered
            .find(" chat sent --bridge-state ")
            .expect("the check command");
        assert!(
            check
                < offered
                    .find(" chat reply --bridge-state ")
                    .expect("the reply command")
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn an_unusable_record_sends_no_reminder_until_it_is_repaired_or_deleted() {
        let (root, state) = state("unusable");
        let path = root.join(REPLY_REMINDER_FILE);
        fs::write(&path, b"{").expect("spoil the record");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("private");
        let error = state
            .update_reply_reminders(1, None, |_| (true, ()))
            .expect_err("an unusable record is an error")
            .to_string();
        assert!(error.contains(&path.display().to_string()), "{error}");
        assert!(error.contains("no reply reminder is sent"), "{error}");
        assert_eq!(fs::read(&path).expect("left as it was"), b"{");
        fs::remove_file(&path).expect("delete");
        assert_eq!(start(&state, 5).started_at_millis, 5);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn the_record_keeps_only_reminded_requests_that_could_still_be_reminded() {
        let (root, state) = state("prune");
        let day = u64::try_from(MAX_REPLY_REMINDER_AGE.as_millis()).expect("a day in ms");
        let now = 3 * day;
        let keep = "a".repeat(64);
        let retired = "b".repeat(64);
        let old = "c".repeat(64);
        let (record, ()) = state
            .update_reply_reminders(now, None, |record| {
                for (key, at_millis) in
                    [(&keep, now - 1), (&retired, now - 1), (&old, now - day - 1)]
                {
                    record.reminded.push(RemindedRequest {
                        key: key.clone(),
                        reply_id: "001".to_owned(),
                        at_millis,
                    });
                }
                (true, ())
            })
            .expect("record");
        assert_eq!(
            record.reminded.len(),
            2,
            "one reminded over a day ago is dropped"
        );
        let retained = [keep.clone(), old.clone()].into_iter().collect();
        let (record, ()) = state
            .update_reply_reminders(now, Some(&retained), |_| (true, ()))
            .expect("record");
        assert_eq!(
            record
                .reminded
                .iter()
                .map(|request| request.key.as_str())
                .collect::<Vec<_>>(),
            [keep.as_str()],
            "a request no longer retained is dropped"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }
}
