//! Replies a restarted bridge holds back as stale (#781 chat-no-stale-replies-on-restart).
//!
//! On 2026-11-02 a bridge refreshed to a new build posted to its owner a reply that had waited in
//! its outbound queue for four hours. A reply that old, or one whose thread has moved on (a later
//! request in the same thread already has a reply sent), is no longer what the coordinator
//! would say. So when `chat run` starts, [`BridgeState::hold_stale_replies`] holds every unsent
//! reply captured longer ago than its bound (`chat run --hold-replies-older-than`, default
//! [`DEFAULT_HOLD_REPLIES_OLDER_THAN`]) or overtaken that way, in `held-replies.json`, and the
//! service does not send it. The coordinator is told, `chat status` and `chat health` list them,
//! and `chat held` sends ([`BridgeState::release_held_reply`]) or drops
//! ([`BridgeState::drop_held_reply`]) each one; to edit one, drop it and send the new text with
//! `chat reply`. Replies captured after the start are never held, and neither is a reply of a
//! held request's that comes after a held one: it waits behind it, in order, as any reply does.

use super::*;

/// The default `chat run --hold-replies-older-than`.
pub const DEFAULT_HOLD_REPLIES_OLDER_THAN: Duration = Duration::from_secs(15 * 60);
/// The file that lists the held replies.
const HELD_REPLIES_FILE: &str = "held-replies.json";
/// The largest `held-replies.json` read.
const MAX_HELD_REPLIES_BYTES: usize = 1024 * 1024;
/// Most replies held at once; a start that would hold more holds the oldest this many.
const MAX_HELD_REPLIES: usize = 512;

/// Why a restarted bridge held a reply back.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HoldReason {
    /// It was captured longer ago than the bound.
    Stale,
    /// A later request in the same thread already has a reply sent.
    Overtaken,
}

impl HoldReason {
    /// The reason in words.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Stale => "captured longer ago than the bound",
            Self::Overtaken => "a later request in the same thread already has a reply sent",
        }
    }
}

/// One reply a restarted bridge holds back.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HeldReply {
    /// The request key.
    pub key: String,
    /// The reply's number within its request.
    pub ordinal: u32,
    /// When it was captured, in Unix milliseconds.
    pub captured_at_millis: u64,
    /// When the start held it, in Unix milliseconds.
    pub held_at_millis: u64,
    /// Why it was held.
    pub reason: HoldReason,
}

impl HeldReply {
    /// The handle `chat held --send` and `--drop` take: `KEY/ORDINAL`.
    #[must_use]
    pub fn handle(&self) -> String {
        format!("{}/{}", self.key, self.ordinal)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct HeldRepliesRecord {
    schema: String,
    held: Vec<HeldReply>,
}

const HELD_REPLIES_SCHEMA: &str = "agentctl-chat-held-replies/v1";

/// A held reply's handle, `KEY/ORDINAL`, parsed.
pub fn parse_held_handle(handle: &str) -> Result<(String, u32)> {
    let (key, ordinal) = handle.split_once('/').ok_or_else(|| {
        ChatRuntimeError::invalid("a held reply is named KEY/ORDINAL, as `chat held` lists it")
    })?;
    if !valid_key(key) {
        return Err(ChatRuntimeError::invalid(
            "a held reply's KEY is its request's 64-character lowercase hexadecimal key",
        ));
    }
    let ordinal = ordinal
        .parse::<u32>()
        .ok()
        .filter(|ordinal| *ordinal > 0)
        .ok_or_else(|| ChatRuntimeError::invalid("a held reply's ORDINAL is a positive number"))?;
    Ok((key.to_owned(), ordinal))
}

impl BridgeState {
    fn held_replies_path(&self) -> PathBuf {
        self.root.join(HELD_REPLIES_FILE)
    }

    /// `held-replies.json`'s entries; none when it does not exist.
    fn read_held_locked(&self) -> Result<Vec<HeldReply>> {
        match read_document::<HeldRepliesRecord>(&self.held_replies_path(), MAX_HELD_REPLIES_BYTES)
        {
            Ok(record) if record.schema == HELD_REPLIES_SCHEMA => Ok(record.held),
            Ok(record) => Err(ChatRuntimeError::invalid(format!(
                "{} has schema {:?}, not {HELD_REPLIES_SCHEMA}",
                self.held_replies_path().display(),
                record.schema
            ))),
            Err(ChatRuntimeError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                Ok(Vec::new())
            }
            Err(error) => Err(error),
        }
    }

    fn write_held_locked(&self, held: Vec<HeldReply>) -> Result<()> {
        if held.is_empty() {
            return match fs::remove_file(self.held_replies_path()) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => {
                    Err(ChatRuntimeError::Io(error))
                }
                _ => Ok(()),
            };
        }
        write_document(
            &self.held_replies_path(),
            &HeldRepliesRecord {
                schema: HELD_REPLIES_SCHEMA.to_owned(),
                held,
            },
        )
    }

    /// Whether reply `ordinal` of request `key` is held; the caller holds the state lock.
    pub(super) fn reply_held_locked(&self, key: &str, ordinal: u32) -> Result<bool> {
        if fs::symlink_metadata(self.held_replies_path()).is_err() {
            return Ok(false);
        }
        Ok(self
            .read_held_locked()?
            .iter()
            .any(|held| held.key == key && held.ordinal == ordinal))
    }

    /// Hold every unsent reply that is stale at `now_millis`: the next one each request would
    /// send, when it was captured `older_than` or longer ago, or when a later request in the same
    /// thread already has a reply sent. Replies already held stay held while they are unsent.
    /// Returns the replies held by this call. `chat run` calls it once, as it starts.
    pub fn hold_stale_replies(
        &self,
        now_millis: u64,
        older_than: Duration,
    ) -> Result<Vec<HeldReply>> {
        let bound = u64::try_from(older_than.as_millis()).unwrap_or(u64::MAX);
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        state_lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
        let _state_lock = agent_queue::fsio::HeldLock::new(state_lock);
        let records = self.request_records()?;
        // When each thread last had a reply sent, and by the request admitted when.
        let mut answered: BTreeMap<String, u64> = BTreeMap::new();
        for (record, _) in &records {
            if record.next_send_ordinal > 1
                && self
                    .read_reply(&record.key, record.next_send_ordinal - 1)?
                    .phase
                    == ReplyPhase::Sent
            {
                let admitted = answered
                    .entry(record.message.thread_id.clone())
                    .or_insert(0);
                *admitted = (*admitted).max(record.admitted_at_millis);
            }
        }
        let mut held: Vec<HeldReply> = Vec::new();
        let earlier = self.read_held_locked()?;
        let mut added = Vec::new();
        for (record, _) in &records {
            if record.reply_closed || record.next_send_ordinal >= record.next_reply_ordinal {
                continue;
            }
            let reply = self.read_reply(&record.key, record.next_send_ordinal)?;
            if matches!(reply.phase, ReplyPhase::Sent | ReplyPhase::Failed) {
                continue;
            }
            if let Some(kept) = earlier
                .iter()
                .find(|held| held.key == record.key && held.ordinal == reply.ordinal)
            {
                held.push(kept.clone());
                continue;
            }
            let overtaken = answered
                .get(&record.message.thread_id)
                .is_some_and(|admitted| *admitted > record.admitted_at_millis);
            let stale = bound > 0 && now_millis.saturating_sub(reply.captured_at_millis) >= bound;
            let reason = match (stale, overtaken) {
                (true, _) => HoldReason::Stale,
                (false, true) => HoldReason::Overtaken,
                (false, false) => continue,
            };
            let entry = HeldReply {
                key: record.key.clone(),
                ordinal: reply.ordinal,
                captured_at_millis: reply.captured_at_millis,
                held_at_millis: now_millis,
                reason,
            };
            held.push(entry.clone());
            added.push(entry);
        }
        held.sort_by_key(|entry| entry.captured_at_millis);
        held.truncate(MAX_HELD_REPLIES);
        added.retain(|entry| held.contains(entry));
        self.write_held_locked(held)?;
        Ok(added)
    }

    /// The replies held now, oldest capture first, without the ones sent or dropped since.
    pub fn held_replies(&self) -> Result<Vec<HeldReply>> {
        let _snapshot = self.lock_state_snapshot()?;
        self.held_replies_locked()
    }

    /// [`Self::held_replies`], under a state lock the caller holds.
    pub(super) fn held_replies_locked(&self) -> Result<Vec<HeldReply>> {
        let mut held = Vec::new();
        for entry in self.read_held_locked()? {
            match self.read_reply(&entry.key, entry.ordinal) {
                Ok(reply) if !matches!(reply.phase, ReplyPhase::Sent | ReplyPhase::Failed) => {
                    held.push(entry);
                }
                Ok(_) => {}
                Err(ChatRuntimeError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(held)
    }

    /// The text of held reply `ordinal` of request `key`, for `chat held` to show.
    pub fn held_reply_text(&self, key: &str, ordinal: u32) -> Result<String> {
        let _snapshot = self.lock_state_snapshot()?;
        Ok(self.read_reply(key, ordinal)?.body)
    }

    /// Take the hold off reply `ordinal` of request `key`, so the service sends it.
    pub fn release_held_reply(&self, key: &str, ordinal: u32) -> Result<HeldReply> {
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        state_lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
        let _state_lock = agent_queue::fsio::HeldLock::new(state_lock);
        let mut held = self.read_held_locked()?;
        let position = held
            .iter()
            .position(|entry| entry.key == key && entry.ordinal == ordinal)
            .ok_or_else(|| ChatRuntimeError::invalid("no held reply has this KEY/ORDINAL"))?;
        let entry = held.remove(position);
        self.write_held_locked(held)?;
        Ok(entry)
    }

    /// Drop held reply `ordinal` of request `key`: it is never sent, as a reply the provider
    /// refused for good is not, and the request's later replies are no longer held behind it.
    pub fn drop_held_reply(&self, key: &str, ordinal: u32) -> Result<HeldReply> {
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        state_lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
        let _state_lock = agent_queue::fsio::HeldLock::new(state_lock);
        let mut held = self.read_held_locked()?;
        let position = held
            .iter()
            .position(|entry| entry.key == key && entry.ordinal == ordinal)
            .ok_or_else(|| ChatRuntimeError::invalid("no held reply has this KEY/ORDINAL"))?;
        let mut reply = self.read_reply(key, ordinal)?;
        if !matches!(reply.phase, ReplyPhase::Sent | ReplyPhase::Failed) {
            reply.phase = ReplyPhase::Failed;
            reply.send_permanent_failure = Some(
                "dropped with `chat held --drop`: a restarted bridge held it as stale, and it was \
                 not to be sent"
                    .to_owned(),
            );
            write_document(&self.reply_path(key, ordinal), &reply)?;
        }
        let entry = held.remove(position);
        self.write_held_locked(held)?;
        Ok(entry)
    }
}

impl BridgeState {
    /// The explicit `chat held` command that sends (`send`) or drops (`drop`) the held reply
    /// `handle` on this state, run by this executable; `None` when a word of it cannot be printed
    /// safely on one line.
    pub(crate) fn held_command(&self, action: &str, handle: &str) -> Option<String> {
        let root = std::path::absolute(&self.root).ok()?;
        let command = format!(
            "{} chat held --bridge-state {} --{action} {}",
            history_program(env::current_exe()),
            shell_word(root.to_str()?)?,
            shell_word(handle)?,
        );
        (neutral_capture_syntax(&command) == command).then_some(command)
    }
}
