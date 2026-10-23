//! One record of the Slack client's conversation event stream, checked before anything of it is
//! used. `#452 slack-live-ingest`.
//!
//! The client's `slack.conversation tail --output=json --include-iris-seq-id` writes one JSON
//! object a line: `{"type": "message.created", "iris_seq_id": N, "payload": {"container_id":
//! "C…", "slack_message": {"channel_id", "message_ts", "thread_ts", "slack_user_id", "bot_id",
//! "app_id", "subtype", "text", "team_id"}}}`, `reaction.created` and `reaction.deleted` records,
//! and `{"type": "HEARTBEAT"}`, `{"type": "GAP", "lost_from_iris_seq_id", "lost_to_iris_seq_id"}`,
//! `STALL` and `REVIVAL` control records. Posts by other bots, edits and deletions are not in it:
//! the live poller and the relist ([`crate::relist`]) find those.
//!
//! The stream is a trust boundary. Every identifier must have its Slack shape, the conversation
//! is named consistently, and the text is bounded; anything else refuses the whole record. A
//! refusal names the field, never its value, so a malformed record's content never reaches a log.
//! The text that passes is still somebody else's words, untrusted exactly as a read's are.
//!
//! A message record becomes the `conversations.history` object a read of the same message would
//! have returned, as far as the stream carries one, and goes through the same normalization
//! ([`crate::chat::ChatClient::pushed_message`]). What the stream does not carry (files,
//! reactions, a display name) a read brings later. An `edited_ts`, should a record carry one, is
//! carried on as the message's edit time. Nothing in a record says whether its text is the
//! message's whole body: the stream leaves out the files a message shares, which a read writes
//! as lines of the body, so a pushed copy stays partial in the cache until a read proves it whole
//! (merge rule 5).

use serde_json::{Map, Value};

use super::ids;

/// The most characters a message's text may hold: Slack's own ceiling on a message.
pub const MAX_TEXT_CHARS: usize = super::SLACK_MAX_TEXT_CHARS;

/// The most bytes of one record the ingest route reads. A message at [`MAX_TEXT_CHARS`] of
/// four-byte characters, and the record around it.
pub const MAX_RECORD_BYTES: usize = 4 * MAX_TEXT_CHARS + 8 * 1024;

/// One checked record.
#[derive(Clone, Debug, PartialEq)]
pub enum TailRecord {
    /// A message was posted: the `conversations.history` object for it.
    Message {
        /// The stream's sequence number for the record.
        sequence: u64,
        /// The conversation it was posted in.
        conversation: String,
        /// Its canonical `ts`.
        ts: String,
        /// The canonical root `ts` of the thread it was posted in, when that is not itself.
        thread_ts: Option<String>,
        /// Whether it is a thread reply that was also sent to the conversation.
        broadcast: bool,
        /// The object to normalize.
        raw: Value,
    },
    /// A reaction was added or removed. Its content is not read: the poller reads the set.
    Reaction {
        /// The stream's sequence number for the record.
        sequence: u64,
        /// The conversation of the message reacted to.
        conversation: String,
    },
    /// The stream lost the records between two sequence numbers, either of which it may not say.
    Gap {
        /// The first sequence lost, when said.
        from: Option<u64>,
        /// The last sequence lost, when said.
        to: Option<u64>,
    },
    /// A heartbeat, a stall or a revival: nothing to act on.
    Control,
    /// A sequenced record of a kind this server does not act on.
    Other {
        /// The stream's sequence number for the record.
        sequence: u64,
    },
}

/// Why a record was refused: the field, never its value.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the stream record is malformed: {0}")]
pub struct TailError(pub &'static str);

fn field<'v>(object: &'v Map<String, Value>, key: &str) -> Result<Option<&'v str>, TailError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if text.is_empty() => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(TailError(key_name(key))),
    }
}

/// The field names a refusal may say, as static strings.
fn key_name(key: &str) -> &'static str {
    const KNOWN: [&str; 15] = [
        "type",
        "iris_seq_id",
        "payload",
        "event_type",
        "container_id",
        "slack_message",
        "channel_id",
        "message_ts",
        "thread_ts",
        "slack_user_id",
        "bot_id",
        "app_id",
        "subtype",
        "text",
        "edited_ts",
    ];
    KNOWN
        .iter()
        .find(|known| **known == key)
        .copied()
        .unwrap_or("a field")
}

/// `B`, `A` or `T`/`E` and then upper-case letters and digits: a bot, app or team id.
fn is_prefixed_id(id: &str, prefixes: &[u8]) -> bool {
    id.len() >= 2
        && id.len() <= 32
        && prefixes.contains(&id.as_bytes()[0])
        && id
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn is_subtype(subtype: &str) -> bool {
    !subtype.is_empty()
        && subtype.len() <= 64
        && subtype
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
}

fn conversation_of(
    payload: &Map<String, Value>,
    inner: Option<&Map<String, Value>>,
) -> Result<String, TailError> {
    let container = field(payload, "container_id")?;
    let named = match inner {
        Some(inner) => field(inner, "channel_id")?,
        None => None,
    };
    let conversation = match (named, container) {
        (Some(named), Some(container)) if named != container => {
            return Err(TailError("channel_id"));
        }
        (Some(id), _) | (None, Some(id)) => id,
        (None, None) => return Err(TailError("channel_id")),
    };
    if !ids::is_conversation_id(conversation) {
        return Err(TailError("channel_id"));
    }
    Ok(conversation.to_owned())
}

fn sequence_of(record: &Map<String, Value>) -> Result<u64, TailError> {
    record
        .get("iris_seq_id")
        .and_then(Value::as_u64)
        .ok_or(TailError("iris_seq_id"))
}

fn optional_sequence(
    record: &Map<String, Value>,
    key: &'static str,
) -> Result<Option<u64>, TailError> {
    match record.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or(TailError(key)),
    }
}

/// Check one record.
///
/// # Errors
///
/// [`TailError`] naming the first field that does not have its shape.
pub fn parse(value: &Value) -> Result<TailRecord, TailError> {
    let record = value.as_object().ok_or(TailError("type"))?;
    let kind = field(record, "type")?.ok_or(TailError("type"))?;
    match kind {
        "HEARTBEAT" | "STALL" | "REVIVAL" => return Ok(TailRecord::Control),
        "GAP" => {
            return Ok(TailRecord::Gap {
                from: optional_sequence(record, "lost_from_iris_seq_id")?,
                to: optional_sequence(record, "lost_to_iris_seq_id")?,
            })
        }
        _ => {}
    }
    let sequence = sequence_of(record)?;
    let payload = record
        .get("payload")
        .and_then(Value::as_object)
        .ok_or(TailError("payload"))?;
    let event_type = field(payload, "event_type")?.unwrap_or(kind);
    match event_type {
        "message.created" => message(sequence, payload),
        "reaction.created" | "reaction.deleted" => {
            let inner = payload
                .get("slack_reaction")
                .or_else(|| payload.get("slack_message"))
                .and_then(Value::as_object);
            Ok(TailRecord::Reaction {
                sequence,
                conversation: conversation_of(payload, inner)?,
            })
        }
        _ => Ok(TailRecord::Other { sequence }),
    }
}

fn message(sequence: u64, payload: &Map<String, Value>) -> Result<TailRecord, TailError> {
    let slack = payload
        .get("slack_message")
        .and_then(Value::as_object)
        .ok_or(TailError("slack_message"))?;
    let conversation = conversation_of(payload, Some(slack))?;
    let ts = field(slack, "message_ts")?
        .and_then(ids::canonical_ts)
        .filter(|ts| ids::message_id_from_ts(ts).is_some())
        .ok_or(TailError("message_ts"))?;
    let thread_ts = match field(slack, "thread_ts")? {
        None => None,
        Some(raw) => Some(
            ids::canonical_ts(raw)
                .filter(|ts| ids::message_id_from_ts(ts).is_some())
                .ok_or(TailError("thread_ts"))?,
        ),
    };
    let user = field(slack, "slack_user_id")?;
    if user.is_some_and(|user| !ids::is_user_id(user)) {
        return Err(TailError("slack_user_id"));
    }
    let bot = field(slack, "bot_id")?;
    if bot.is_some_and(|bot| !is_prefixed_id(bot, b"B")) {
        return Err(TailError("bot_id"));
    }
    let app = field(slack, "app_id")?;
    if app.is_some_and(|app| !is_prefixed_id(app, b"A")) {
        return Err(TailError("app_id"));
    }
    let subtype = field(slack, "subtype")?;
    if subtype.is_some_and(|subtype| !is_subtype(subtype)) {
        return Err(TailError("subtype"));
    }
    // A creation is never edited, but a record that says when its message was last edited
    // carries that time on, so the cache can rank this copy against a read's by edit time.
    let edited = match field(slack, "edited_ts")? {
        None => None,
        Some(raw) => Some(ids::canonical_ts(raw).ok_or(TailError("edited_ts"))?),
    };
    let team = field(slack, "team_id")?;
    if team.is_some_and(|team| !is_prefixed_id(team, b"TE")) {
        return Err(TailError("team_id"));
    }
    let text = match slack.get("text") {
        None | Some(Value::Null) => "",
        Some(Value::String(text)) => text,
        Some(_) => return Err(TailError("text")),
    };
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(TailError("text"));
    }
    let mut raw = Map::new();
    raw.insert("type".to_owned(), Value::from("message"));
    raw.insert("ts".to_owned(), Value::from(ts.clone()));
    raw.insert("text".to_owned(), Value::from(text));
    let mut put = |key: &str, value: Option<&str>| {
        if let Some(value) = value {
            raw.insert(key.to_owned(), Value::from(value));
        }
    };
    put("user", user);
    put("bot_id", bot);
    put("app_id", app);
    put("subtype", subtype);
    put("team", team);
    if let Some(edited) = edited {
        raw.insert("edited".to_owned(), serde_json::json!({ "ts": edited }));
    }
    // A root names no thread; the stream may still say its own `ts` as one.
    let thread_ts = thread_ts.filter(|thread| *thread != ts);
    if let Some(thread) = &thread_ts {
        raw.insert("thread_ts".to_owned(), Value::from(thread.clone()));
    }
    Ok(TailRecord::Message {
        sequence,
        conversation,
        ts,
        thread_ts,
        broadcast: subtype == Some("thread_broadcast"),
        raw: Value::Object(raw),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn created(slack: Value) -> Value {
        json!({
            "type": "message.created",
            "iris_seq_id": 42,
            "payload": {"container_id": "C0000MAIN1", "slack_message": slack},
        })
    }

    #[test]
    fn a_message_record_becomes_the_history_object_a_read_would_return() {
        let record = created(json!({
            "channel_id": "C0000MAIN1",
            "message_ts": "1700000000.0001",
            "thread_ts": "1699999999.000001",
            "slack_user_id": "UADA00001",
            "text": "hi &amp; bye",
            "team_id": "T0000TEAM",
        }));
        let TailRecord::Message {
            sequence,
            conversation,
            ts,
            thread_ts,
            broadcast,
            raw,
        } = parse(&record).expect("valid")
        else {
            panic!("not a message");
        };
        assert_eq!(sequence, 42);
        assert_eq!(conversation, "C0000MAIN1");
        assert_eq!(ts, "1700000000.000100");
        assert_eq!(thread_ts.as_deref(), Some("1699999999.000001"));
        assert!(!broadcast);
        assert_eq!(
            raw,
            json!({
                "type": "message",
                "ts": "1700000000.000100",
                "thread_ts": "1699999999.000001",
                "user": "UADA00001",
                "text": "hi &amp; bye",
                "team": "T0000TEAM",
            })
        );
    }

    #[test]
    fn an_edit_time_a_record_carries_reaches_the_history_object() {
        let record = created(json!({
            "message_ts": "1700000000.000100",
            "slack_user_id": "UADA00001",
            "text": "fixed",
            "edited_ts": "1700000100.5",
        }));
        let Ok(TailRecord::Message { raw, .. }) = parse(&record) else {
            panic!("not a message");
        };
        assert_eq!(raw["edited"], json!({"ts": "1700000100.500000"}));
    }

    #[test]
    fn a_root_naming_its_own_ts_as_its_thread_names_none() {
        let record = created(json!({
            "message_ts": "1700000000.000100",
            "thread_ts": "1700000000.000100",
            "bot_id": "B0BOT0001",
            "subtype": "bot_message",
            "text": "",
        }));
        let Ok(TailRecord::Message { thread_ts, raw, .. }) = parse(&record) else {
            panic!("not a message");
        };
        assert_eq!(thread_ts, None);
        assert!(raw.get("thread_ts").is_none());
        assert_eq!(raw["bot_id"], "B0BOT0001");
    }

    #[test]
    fn every_malformed_identifier_or_oversized_text_refuses_the_record_by_field_name() {
        let base = json!({
            "channel_id": "C0000MAIN1",
            "message_ts": "1700000000.000100",
            "slack_user_id": "UADA00001",
            "text": "fine",
        });
        let cases: Vec<(&str, Value, &str)> = vec![
            ("channel_id", json!("general"), "channel_id"),
            ("channel_id", json!("C0000OTHER"), "channel_id"),
            ("message_ts", json!("yesterday"), "message_ts"),
            ("message_ts", json!(1_700_000_000), "message_ts"),
            ("thread_ts", json!("1.2.3"), "thread_ts"),
            ("slack_user_id", json!("ada; rm -rf"), "slack_user_id"),
            ("bot_id", json!("U0000"), "bot_id"),
            ("app_id", json!("a0000"), "app_id"),
            ("subtype", json!("Bot Message"), "subtype"),
            ("team_id", json!("x"), "team_id"),
            ("edited_ts", json!("later"), "edited_ts"),
            ("text", json!(["not", "text"]), "text"),
            ("text", json!("x".repeat(MAX_TEXT_CHARS + 1)), "text"),
        ];
        for (key, value, named) in cases {
            let mut slack = base.clone();
            slack[key] = value;
            let refused = parse(&created(slack)).expect_err(key);
            assert_eq!(refused, TailError(named), "{key}");
            assert!(!refused.to_string().contains("rm -rf"));
        }
        let mut missing = created(base.clone());
        missing["payload"]
            .as_object_mut()
            .expect("payload")
            .remove("container_id");
        missing["payload"]["slack_message"]
            .as_object_mut()
            .expect("message")
            .remove("channel_id");
        assert_eq!(parse(&missing), Err(TailError("channel_id")));
        let mut unsequenced = created(base);
        unsequenced["iris_seq_id"] = json!("42");
        assert_eq!(parse(&unsequenced), Err(TailError("iris_seq_id")));
        assert_eq!(parse(&json!([1])), Err(TailError("type")));
    }

    #[test]
    fn control_gap_reaction_and_unknown_records_are_told_apart() {
        assert_eq!(
            parse(&json!({"type": "HEARTBEAT", "ts": 1.0})),
            Ok(TailRecord::Control)
        );
        assert_eq!(
            parse(&json!({"type": "GAP", "lost_from_iris_seq_id": 1, "lost_to_iris_seq_id": 5})),
            Ok(TailRecord::Gap {
                from: Some(1),
                to: Some(5)
            })
        );
        assert_eq!(
            parse(&json!({"type": "GAP"})),
            Ok(TailRecord::Gap {
                from: None,
                to: None
            })
        );
        assert_eq!(
            parse(&json!({"type": "GAP", "lost_from_iris_seq_id": "x"})),
            Err(TailError("lost_from_iris_seq_id"))
        );
        assert_eq!(
            parse(&json!({
                "type": "reaction.created",
                "iris_seq_id": 7,
                "payload": {"container_id": "C0000MAIN1", "slack_reaction": {"name": "eyes"}},
            })),
            Ok(TailRecord::Reaction {
                sequence: 7,
                conversation: "C0000MAIN1".to_owned()
            })
        );
        assert_eq!(
            parse(&json!({"type": "member.joined", "iris_seq_id": 8, "payload": {}})),
            Ok(TailRecord::Other { sequence: 8 })
        );
    }
}
