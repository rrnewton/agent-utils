//! Mid-turn reply notices for a headless coordinator (#2386 headless-reply-in-thinking): what
//! `chat run` records about the one notice it may queue, while the turn that took a request is
//! still running, when that request holds no reply after several tool calls.
//! `chat_service::headless_reminders` decides when one is due.
//!
//! A headless coordinator can write its reply only in its thinking. The thinking never reaches
//! the event log the bridge reads, so nothing is captured and the coordinator believes it
//! answered. The turn-end reminder (#2331) catches that only after the turn, which can run for
//! many minutes. The notice comes during the turn instead, once for each request. Every rule
//! that keeps it to once lives in `reply-notices.json`, a file of its own so a release that does
//! not know it leaves it alone:
//!
//! * when the record was made, with the first notice (which requests may be noticed at all is
//!   the reminders' rule: delivered after reminders started, within a day, never reminded);
//! * each request noticed, recorded before its notice is queued, so neither a crash nor a restart
//!   notices it twice;
//! * the notice itself while it is being queued. One the queue refused is taken back, with its
//!   requests, so a later check can notice them again; one queued, or that may have been queued,
//!   counts as delivered.

use std::collections::BTreeSet;
use std::env;
use std::io;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::reply_reminders::MAX_REPLY_REMINDER_AGE;
use super::{
    agent, format_age, read_document, valid_key, write_document, BridgeState, ChatRuntimeError,
    ReminderCandidate, Result, MAX_REQUESTS,
};

/// The record of mid-turn reply notices in the bridge state directory.
pub(crate) const REPLY_NOTICE_FILE: &str = "reply-notices.json";
pub(crate) const REPLY_NOTICE_SCHEMA: &str = "agentctl-chat-reply-notices/v1";
/// Most reply IDs one notice names; it counts the rest, and notices them too.
const MAX_NOTICE_LISTED_IDS: usize = 32;
/// Most requests the record holds as noticed. Each write keeps only requests the state still
/// retains, and a state retains at most `MAX_REQUESTS`.
const MAX_NOTICED_REQUESTS: usize = MAX_REQUESTS as usize;
const MAX_NOTICE_REPLY_ID_BYTES: usize = 64;
const MAX_REPLY_NOTICE_PROMPT_BYTES: usize = 16 * 1_024;
const MAX_REPLY_NOTICE_BYTES: usize = 1_024 * 1_024;
const NOTICE_MESSAGE_PREFIX: &str = "chat-notice-";

/// `reply-notices.json`: when notices started, which requests were noticed, and the notice being
/// queued, if any.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ReplyNoticeRecord {
    schema: String,
    /// When the record was made, with the first notice, in milliseconds since the Unix epoch.
    pub(crate) started_at_millis: u64,
    /// Every request noticed, once each, that the state still retained at the last write.
    pub(crate) noticed: Vec<NoticedRequest>,
    /// The notice recorded before it was queued, until the queue settles it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending: Option<PendingNotice>,
}

/// One request a notice named.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct NoticedRequest {
    pub(crate) key: String,
    /// The reply ID the notice named it by.
    pub(crate) reply_id: String,
    /// When it was recorded as noticed, in milliseconds since the Unix epoch.
    pub(crate) at_millis: u64,
}

/// A notice recorded before it was queued, until the queue settles it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct PendingNotice {
    /// Its identifier in the coordinator's queue.
    pub(crate) message_id: String,
    /// The requests it names.
    pub(crate) keys: Vec<String>,
    /// Its exact text.
    pub(crate) prompt: String,
}

fn valid_reply_id(reply_id: &str) -> bool {
    !reply_id.is_empty()
        && reply_id.len() <= MAX_NOTICE_REPLY_ID_BYTES
        && reply_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_message_id(message_id: &str) -> bool {
    message_id
        .strip_prefix(NOTICE_MESSAGE_PREFIX)
        .is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
}

impl ReplyNoticeRecord {
    fn new(started_at_millis: u64) -> Self {
        Self {
            schema: REPLY_NOTICE_SCHEMA.to_owned(),
            started_at_millis,
            noticed: Vec::new(),
            pending: None,
        }
    }

    fn validate(&self) -> Result<()> {
        if self.schema != REPLY_NOTICE_SCHEMA {
            return Err(ChatRuntimeError::invalid(format!(
                "unsupported schema {:?}",
                self.schema
            )));
        }
        let mut keys = BTreeSet::new();
        let noticed_valid = self.noticed.len() <= MAX_NOTICED_REQUESTS
            && self.noticed.iter().all(|request| {
                valid_key(&request.key)
                    && valid_reply_id(&request.reply_id)
                    && keys.insert(request.key.as_str())
            });
        if !noticed_valid {
            return Err(ChatRuntimeError::invalid(
                "its noticed requests are malformed, repeated or too many",
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
                && pending.prompt.len() <= MAX_REPLY_NOTICE_PROMPT_BYTES;
            if !valid {
                return Err(ChatRuntimeError::invalid(
                    "its pending notice is malformed or names a request it does not record as \
noticed",
                ));
            }
        }
        Ok(())
    }

    /// Whether `key` has been noticed.
    pub(crate) fn noticed(&self, key: &str) -> bool {
        self.noticed.iter().any(|request| request.key == key)
    }

    /// Record `notice` of `requests` at `now_millis`: each request as noticed, and the notice as
    /// pending until the queue settles it.
    pub(crate) fn record(
        &mut self,
        notice: &PendingNotice,
        requests: &[ReminderCandidate],
        now_millis: u64,
    ) {
        for request in requests {
            if !self.noticed(&request.key) {
                self.noticed.push(NoticedRequest {
                    key: request.key.clone(),
                    reply_id: request.reply_id.clone(),
                    at_millis: now_millis,
                });
            }
        }
        self.pending = Some(notice.clone());
    }

    /// Settle the pending notice `message_id`: one the queue took, or may have taken, stays
    /// recorded with its requests; one it refused is taken back with them, so a later check can
    /// notice them again. Returns whether the record changed.
    pub(crate) fn settle(&mut self, message_id: &str, queued: bool) -> bool {
        let Some(pending) = self
            .pending
            .take_if(|pending| pending.message_id == message_id)
        else {
            return false;
        };
        if !queued {
            let named: BTreeSet<&str> = pending.keys.iter().map(String::as_str).collect();
            self.noticed
                .retain(|request| !named.contains(request.key.as_str()));
        }
        true
    }
}

impl BridgeState {
    fn reply_notice_path(&self) -> std::path::PathBuf {
        self.root.join(REPLY_NOTICE_FILE)
    }

    /// `reply-notices.json`, or `None` when it does not exist.
    fn read_reply_notices(&self) -> Result<Option<ReplyNoticeRecord>> {
        let path = self.reply_notice_path();
        let unusable = |error: ChatRuntimeError| {
            ChatRuntimeError::invalid(format!(
                "reply notice record {} is unusable: {error}; no mid-turn reply notice is sent \
until an operator repairs or deletes it, and deleting it notices no request twice, since notices \
then start again from that time",
                path.display()
            ))
        };
        let record: ReplyNoticeRecord = match read_document(&path, MAX_REPLY_NOTICE_BYTES) {
            Ok(record) => record,
            Err(ChatRuntimeError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(unusable(error)),
        };
        record.validate().map_err(unusable)?;
        Ok(Some(record))
    }

    /// The notice record, read under the state lock, or a new one starting at `now_millis` when
    /// none exists yet. A new record is not written: the file appears only with the first notice.
    pub(crate) fn reply_notices(&self, now_millis: u64) -> Result<ReplyNoticeRecord> {
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        fs2::FileExt::lock_shared(&state_lock).map_err(ChatRuntimeError::Io)?;
        let _state_lock = agent_queue::fsio::HeldLock::new(state_lock);
        Ok(self
            .read_reply_notices()?
            .unwrap_or_else(|| ReplyNoticeRecord::new(now_millis)))
    }

    /// Read `reply-notices.json` under the state lock and pass it to `change`, starting a record
    /// at `now_millis` when it does not exist. Writes the record back when `change` returns
    /// `true`. Before it is written, every noticed request that `retained` does not hold, or that
    /// was noticed more than a day ago, is dropped, unless the pending notice names it: neither
    /// can be noticed again.
    pub(crate) fn update_reply_notices<T>(
        &self,
        now_millis: u64,
        retained: Option<&BTreeSet<String>>,
        change: impl FnOnce(&mut ReplyNoticeRecord) -> (bool, T),
    ) -> Result<(ReplyNoticeRecord, T)> {
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        fs2::FileExt::lock_exclusive(&state_lock).map_err(ChatRuntimeError::Io)?;
        let _state_lock = agent_queue::fsio::HeldLock::new(state_lock);
        let mut record = self
            .read_reply_notices()?
            .unwrap_or_else(|| ReplyNoticeRecord::new(now_millis));
        let (changed, value) = change(&mut record);
        if changed {
            let max_age = u64::try_from(MAX_REPLY_REMINDER_AGE.as_millis()).unwrap_or(u64::MAX);
            let pending: BTreeSet<String> = record
                .pending
                .iter()
                .flat_map(|pending| pending.keys.iter().cloned())
                .collect();
            record.noticed.retain(|request| {
                pending.contains(&request.key)
                    || (retained.is_none_or(|retained| retained.contains(&request.key))
                        && now_millis.saturating_sub(request.at_millis) <= max_age)
            });
            record.validate()?;
            write_document(&self.reply_notice_path(), &record)?;
        }
        Ok((record, value))
    }

    /// The text of a mid-turn notice of `requests`, newest delivery first, `tool_calls` tool
    /// calls into the turn, at `now_millis`. It names up to `MAX_NOTICE_LISTED_IDS` reply IDs and
    /// counts the rest. It spells the marker lines out in parts, as the reminder does, so that no
    /// row of it forms a marker however the terminal wraps it. With `chat run
    /// --offer-reply-command` it prints, for each request it names, the `agentctl chat reply`
    /// command that request's prompt offered.
    pub(crate) fn notice_prompt(
        &self,
        requests: &[ReminderCandidate],
        tool_calls: usize,
        now_millis: u64,
    ) -> String {
        let named = &requests[..requests.len().min(MAX_NOTICE_LISTED_IDS)];
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
            "Chat reply check: the chat bridge has received no reply from you yet to {subject}, \
{tool_calls} tool calls into this turn. A reply written only in your thinking is not delivered: \
the bridge reads only the ordinary text you print. If you have answered only in thinking, print \
the reply as ordinary text between two marker lines: an opening line made of the literal prefix \
`<CHAT_REPLY_`, the reply ID and `>`, and a closing line made of `</CHAT_REPLY_`, the same ID and \
`>`, each alone on its line and outside code fences"
        );
        let close = "If you are still working on it, carry on; this check comes once per request.";
        let generic = format!(
            "{lead}, or send it with the `agentctl chat reply` command its prompt printed. {close}"
        );
        if !self.offer_reply_command {
            return generic;
        }
        let commands = named
            .iter()
            .map(|request| {
                self.reply_command(env::current_exe(), &request.key, &request.reply_id)
                    .map(|command| (request.reply_id.as_str(), command))
            })
            .collect::<Option<Vec<_>>>();
        let Some(commands) = commands else {
            return generic;
        };
        let placeholder = if self.prompt_commands.discoverable {
            "FILE"
        } else {
            "PATH_TO_YOUR_REPLY"
        };
        let offered = if let [(_, command)] = commands.as_slice() {
            format!(
                "{lead}, or write the reply to a file, without the two lines, and send it with \
this, putting the file's path in place of {placeholder}:\n{command}\n{close}"
            )
        } else {
            let lines = commands
                .iter()
                .map(|(reply_id, command)| format!("{reply_id}: {command}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{lead}, or write each reply to a file, without the two lines, and send it with \
its command, putting the file's path in place of {placeholder}:\n{lines}\n{close}"
            )
        };
        if offered.len() <= MAX_REPLY_NOTICE_PROMPT_BYTES {
            offered
        } else {
            generic
        }
    }
}

/// A mid-turn notice of `requests`, `tool_calls` tool calls into the turn, composed at
/// `now_millis`, ready to be recorded and queued.
pub(crate) fn compose_notice(
    state: &BridgeState,
    requests: &[ReminderCandidate],
    tool_calls: usize,
    now_millis: u64,
) -> PendingNotice {
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
    PendingNotice {
        message_id: format!("{NOTICE_MESSAGE_PREFIX}{:x}", digest.finalize()),
        keys: requests.iter().map(|request| request.key.clone()).collect(),
        prompt: state.notice_prompt(requests, tool_calls, now_millis),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(digit: char) -> String {
        digit.to_string().repeat(64)
    }

    fn candidate(digit: char) -> ReminderCandidate {
        ReminderCandidate {
            key: key(digit),
            reply_id: format!("00{digit}"),
            delivered_at_millis: 10,
        }
    }

    fn notice(digits: &[char]) -> PendingNotice {
        PendingNotice {
            message_id: format!("{NOTICE_MESSAGE_PREFIX}{}", "f".repeat(64)),
            keys: digits.iter().map(|digit| key(*digit)).collect(),
            prompt: "Chat reply check: test".to_owned(),
        }
    }

    #[test]
    fn a_refused_notice_is_taken_back_and_a_queued_one_stays_recorded() {
        let mut record = ReplyNoticeRecord::new(1);
        let first = notice(&['1', '2']);
        record.record(&first, &[candidate('1'), candidate('2')], 100);
        assert!(record.noticed(&key('1')) && record.noticed(&key('2')));
        record
            .validate()
            .expect("a pending notice names its noticed requests");
        // The queue refused it: it and its requests are taken back, so a later check can try.
        assert!(record.settle(&first.message_id, false));
        assert!(record.pending.is_none());
        assert!(!record.noticed(&key('1')) && !record.noticed(&key('2')));
        // Queued (or possibly queued): it stays recorded, and settling twice changes nothing.
        record.record(&first, &[candidate('1'), candidate('2')], 200);
        assert!(record.settle(&first.message_id, true));
        assert!(!record.settle(&first.message_id, true));
        assert!(record.noticed(&key('1')) && record.pending.is_none());
        // Recording a request again keeps its first entry.
        record.record(&notice(&['1']), &[candidate('1')], 300);
        assert_eq!(record.noticed.len(), 2);
        assert_eq!(record.noticed[0].at_millis, 200);
        record.validate().expect("valid");
    }

    #[test]
    fn a_malformed_record_is_refused_and_the_file_keeps_its_schema_and_fields() {
        let mut record = ReplyNoticeRecord::new(1);
        record.record(&notice(&['1']), &[candidate('1')], 100);
        let text = serde_json::to_value(&record).expect("serialize");
        assert_eq!(text["schema"], REPLY_NOTICE_SCHEMA);
        assert_eq!(text["started_at_millis"], 1);
        assert_eq!(text["noticed"][0]["key"], key('1'));
        assert_eq!(text["noticed"][0]["reply_id"], "001");
        assert_eq!(text["noticed"][0]["at_millis"], 100);
        assert_eq!(text["pending"]["keys"][0], key('1'));
        // No pending notice, no `pending` key.
        record.settle(&notice(&['1']).message_id, true);
        let settled = serde_json::to_value(&record).expect("serialize");
        assert!(settled.get("pending").is_none(), "{settled}");

        let mut bad = record.clone();
        bad.schema = "agentctl-chat-reply-notices/v9".to_owned();
        assert!(bad.validate().is_err());
        let mut bad = record.clone();
        bad.noticed.push(bad.noticed[0].clone());
        assert!(bad.validate().is_err(), "a repeated request");
        let mut bad = record.clone();
        bad.pending = Some(notice(&['2']));
        assert!(
            bad.validate().is_err(),
            "a pending notice naming an unnoticed request"
        );
        let mut bad = record.clone();
        bad.noticed[0].reply_id = "no spaces".to_owned();
        assert!(bad.validate().is_err());
        let mut bad = record;
        bad.record(
            &PendingNotice {
                message_id: "chat-reminder-x".to_owned(),
                ..notice(&['1'])
            },
            &[candidate('1')],
            100,
        );
        assert!(
            bad.validate().is_err(),
            "a notice id must carry the notice prefix"
        );
    }
}
