//! One record of the Slack client's conversation event stream, checked before anything of it is
//! used. `#452 slack-live-ingest`.
//!
//! The client's `slack.conversation tail --output=json --include-iris-seq-id` writes one JSON
//! object a line: `{"type": "message.created", "iris_seq_id": N, "payload": {"container_id":
//! "T…:C…", "slack_message": {"channel_id", "message_ts", "thread_ts", "slack_user_id", "bot_id",
//! "app_id", "subtype", "text", "team_id", …}, …}}` (the container is the workspace and the
//! conversation; a bare conversation id is read too, and fields this does not use are ignored), `reaction.created` and `reaction.deleted` records,
//! and `{"type": "HEARTBEAT"}`, `{"type": "GAP", "lost_from_iris_seq_id", "lost_to_iris_seq_id"}`,
//! `STALL` and `REVIVAL` control records. Posts by other bots, edits and deletions are not in it:
//! the live poller and the relist ([`crate::relist`]) find those.
//!
//! A chat bridge's own records arrive on the same route ([`CHAT_EVENT_SCHEMA`], `#708
//! vt-live-push`): a message it posted, which the stream never carries, and a reaction it added.
//!
//! The stream is a trust boundary. Every identifier a message record carries must have its Slack
//! shape, the conversation is named consistently, and the text is bounded; anything else refuses
//! the whole record. A refusal names the field and the rule it broke, never its value, so a
//! malformed record's content never reaches a log. A record that is well formed but carries
//! nothing to publish (a client notice with no sequence number, a creation with no Slack message,
//! a reaction that names no conversation, a kind this server does not read) is not refused: it is
//! acknowledged and ignored, since the poller reads whatever it was about.
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
        /// The stream's sequence number for the record, when it gave one.
        sequence: Option<u64>,
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
        /// Whether a chat bridge's own record said it, rather than the provider's stream: it says
        /// nothing of whether that stream is alive. `#708 vt-live-push`.
        bridge: bool,
    },
    /// A reaction a chat bridge added, from its own record (`agentctl.chat-event.v1`, `#708
    /// vt-live-push`): applied to the message at once, as present.
    ReactionAdded {
        /// The conversation of the message reacted to.
        conversation: String,
        /// The canonical `ts` of the message reacted to.
        ts: String,
        /// The emoji itself, or a custom emoji's name.
        emoji: String,
        /// Whether `emoji` is a custom emoji's name.
        custom: bool,
    },
    /// A record about a conversation that cannot be applied as it stands (a bridge's post that
    /// names no author): the poller reads what it was about.
    Unapplied {
        /// The conversation.
        conversation: String,
        /// Why, in words a log may carry.
        why: &'static str,
    },
    /// A reaction was added or removed. Its content is not read: the poller reads the set.
    Reaction {
        /// The stream's sequence number for the record, when it gave one.
        sequence: Option<u64>,
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
    /// A heartbeat, a stall, a revival, or another record with no sequence number: nothing to act
    /// on.
    Control,
    /// A record this server does not act on: a kind it does not read, a reaction whose
    /// conversation it cannot tell, or a creation that carries no message (the poller reads
    /// what such a record was about).
    Other {
        /// The stream's sequence number for the record, when it gave one.
        sequence: Option<u64>,
        /// Why it is not acted on, in words a log may carry.
        why: &'static str,
    },
}

impl TailRecord {
    /// Whether a chat bridge's own record said it, rather than the provider's event stream: such a
    /// record says nothing of whether that stream is alive. `#708 vt-live-push`.
    #[must_use]
    pub fn from_bridge(&self) -> bool {
        match self {
            Self::Message { bridge, .. } => *bridge,
            Self::ReactionAdded { .. } | Self::Unapplied { .. } => true,
            Self::Reaction { .. } | Self::Gap { .. } | Self::Control | Self::Other { .. } => false,
        }
    }
}

/// Why a record was refused: the field and the rule it broke, never its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the stream record is malformed: {field} {rule}")]
pub struct TailError {
    /// The field, as the record names it.
    pub field: &'static str,
    /// The rule it broke.
    pub rule: &'static str,
}

const fn refuse(field: &'static str, rule: &'static str) -> TailError {
    TailError { field, rule }
}

const NOT_A_STRING: &str = "is not a string";

fn field<'v>(
    object: &'v Map<String, Value>,
    key: &'static str,
) -> Result<Option<&'v str>, TailError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if text.is_empty() => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(refuse(key, NOT_A_STRING)),
    }
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
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// The conversation a `container_id` names: the conversation id alone, or, as the stream writes
/// it, the workspace and the conversation, `T…:C…`.
fn container_conversation(container: &str) -> Option<&str> {
    let conversation = match container.split_once(':') {
        None => container,
        Some((team, conversation)) => {
            if !is_prefixed_id(team, b"TE") {
                return None;
            }
            conversation
        }
    };
    ids::is_conversation_id(conversation).then_some(conversation)
}

/// The conversation a record is about: `channel_id` of its Slack object, or its container's.
/// Both said, they must agree; a container this cannot read beside a valid `channel_id` is left
/// alone, since the Slack object is what is published.
fn conversation_of(
    payload: &Map<String, Value>,
    inner: Option<&Map<String, Value>>,
) -> Result<String, TailError> {
    let named = match inner {
        Some(inner) => field(inner, "channel_id")?,
        None => None,
    };
    if named.is_some_and(|named| !ids::is_conversation_id(named)) {
        return Err(refuse("channel_id", "is not a Slack conversation id"));
    }
    let container = field(payload, "container_id")?;
    let contained = container.and_then(container_conversation);
    match (named, contained) {
        (Some(named), Some(contained)) if named != contained => Err(refuse(
            "container_id",
            "names another conversation than slack_message.channel_id",
        )),
        (Some(id), _) | (None, Some(id)) => Ok(id.to_owned()),
        (None, None) if container.is_some() => Err(refuse(
            "container_id",
            "is not a Slack conversation id, nor one prefixed by its workspace",
        )),
        (None, None) => Err(refuse("channel_id", "is missing, and so is container_id")),
    }
}

fn optional_sequence(
    record: &Map<String, Value>,
    key: &'static str,
) -> Result<Option<u64>, TailError> {
    match record.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or(refuse(key, "is not a non-negative integer")),
    }
}

/// Check one record.
///
/// # Errors
///
/// [`TailError`] naming the first field that does not have its shape, and how.
pub fn parse(value: &Value) -> Result<TailRecord, TailError> {
    let record = value
        .as_object()
        .ok_or(refuse("record", "is not a JSON object"))?;
    if record.get("schema").and_then(Value::as_str) == Some(CHAT_EVENT_SCHEMA) {
        return chat_event(record);
    }
    let kind = field(record, "type")?.ok_or(refuse("type", "is missing"))?;
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
    let sequence = optional_sequence(record, "iris_seq_id")?;
    let payload = record.get("payload").and_then(Value::as_object);
    let event_type = match payload {
        Some(payload) => field(payload, "event_type")?.unwrap_or(kind),
        None => kind,
    };
    let known = matches!(
        event_type,
        "message.created" | "reaction.created" | "reaction.deleted"
    );
    // The client writes notices of its own that no sequence names; like a heartbeat, they say
    // nothing about the conversation.
    if sequence.is_none() && !known {
        return Ok(TailRecord::Control);
    }
    let Some(payload) = payload else {
        return Ok(TailRecord::Other {
            sequence,
            why: "it carries no payload",
        });
    };
    match event_type {
        "message.created" => match payload.get("slack_message") {
            None | Some(Value::Null) => Ok(TailRecord::Other {
                sequence,
                why: "a creation that carries no Slack message",
            }),
            Some(Value::Object(slack)) => message(sequence, payload, slack),
            Some(_) => Err(refuse("slack_message", "is not an object")),
        },
        "reaction.created" | "reaction.deleted" => {
            let inner = payload
                .get("slack_reaction")
                .or_else(|| payload.get("slack_message"))
                .and_then(Value::as_object);
            Ok(match conversation_of(payload, inner) {
                Ok(conversation) => TailRecord::Reaction {
                    sequence,
                    conversation,
                },
                // The poller reads every reaction set anyway: a reaction record that cannot be
                // placed only loses its early wake-up, never a change.
                Err(_) => TailRecord::Other {
                    sequence,
                    why: "a reaction whose conversation it does not name",
                },
            })
        }
        _ => Ok(TailRecord::Other {
            sequence,
            why: "a kind this server does not act on",
        }),
    }
}

/// The schema of a chat bridge's own records (`agentctl chat run`'s event sinks). `#708
/// vt-live-push`.
pub const CHAT_EVENT_SCHEMA: &str = "agentctl.chat-event.v1";

/// The most characters of a sender's name or an emoji a bridge record may carry.
const MAX_NAME_CHARS: usize = 80;

/// The `ts` a bridge's resource names, `<conversation>/<kind>/<ts>`, checked.
fn resource_ts(
    record: &Map<String, Value>,
    key: &'static str,
    conversation: &str,
    kind: &str,
) -> Result<Option<String>, TailError> {
    let Some(resource) = field(record, key)? else {
        return Ok(None);
    };
    let ts = resource
        .strip_prefix(conversation)
        .and_then(|rest| rest.strip_prefix('/'))
        .and_then(|rest| rest.strip_prefix(kind))
        .and_then(|rest| rest.strip_prefix('/'))
        .ok_or(refuse(key, "does not name a resource of channel_id"))?;
    ids::canonical_ts(ts)
        .filter(|ts| ids::message_id_from_ts(ts).is_some())
        .map(Some)
        .ok_or(refuse(key, "does not end in a Slack ts"))
}

/// A short single-line name, trimmed; `None` for one that is empty or not text.
fn short_name(value: Option<&Value>) -> Option<String> {
    let name: String = value?
        .as_str()?
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_CHARS)
        .collect();
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// One record of a chat bridge's event sink: a message it posted, or a reaction it added.
/// `#708 vt-live-push`. The provider's own event stream, tailed as a person, never carries a bot's
/// posts; the bridge says each one the moment the provider took it.
///
/// A post becomes the `conversations.history` object a read would return, authored by the
/// bridge's account as its record names it, so a later read's copy is the same message by the
/// same author; one that names no account is left to the poller. Checked as strictly as a stream
/// record: every identifier its Slack shape, the resources of the one conversation, the text
/// bounded.
fn chat_event(record: &Map<String, Value>) -> Result<TailRecord, TailError> {
    let kind = field(record, "kind")?.ok_or(refuse("kind", "is missing"))?;
    let conversation = field(record, "channel_id")?.ok_or(refuse("channel_id", "is missing"))?;
    if !ids::is_conversation_id(conversation) {
        return Err(refuse("channel_id", "is not a Slack conversation id"));
    }
    let ts = resource_ts(record, "message_id", conversation, "messages")?
        .ok_or(refuse("message_id", "is missing"))?;
    let sender = record.get("sender").and_then(Value::as_object);
    let sender_id = match sender {
        Some(sender) => field(sender, "id")?,
        None => None,
    };
    match kind {
        "message.posted" => {
            let thread_ts = resource_ts(record, "thread_id", conversation, "threads")?
                .filter(|thread| *thread != ts);
            let text = match record.get("text") {
                None | Some(Value::Null) => "",
                Some(Value::String(text)) => text,
                Some(_) => return Err(refuse("text", NOT_A_STRING)),
            };
            if text.chars().count() > MAX_TEXT_CHARS {
                return Err(refuse("text", "is longer than Slack's 40,000 characters"));
            }
            let author = match sender_id {
                Some(id) if ids::is_user_id(id) => ("user", id),
                Some(id) if is_prefixed_id(id, b"B") => ("bot_id", id),
                Some(_) => return Err(refuse("sender.id", "is not a Slack user or bot id")),
                None => {
                    return Ok(TailRecord::Unapplied {
                        conversation: conversation.to_owned(),
                        why: "a bridge's post that names no author",
                    })
                }
            };
            // The bridge records the text it was asked to post, Markdown, which the provider
            // converts as it posts; what a read returns is Slack's own text, which normalization
            // reads back. Escaped as a post of it would be, its `&`, `<` and `>` come back as
            // written, and its links and mentions as they are; emphasis the provider rewrites is
            // put right by the next read of the message.
            let text = match field(record, "text_format")? {
                None | Some("markdown") => super::mrkdwn::escape_outgoing(text),
                Some("mrkdwn") => text.to_owned(),
                Some(_) => return Err(refuse("text_format", "is neither markdown nor mrkdwn")),
            };
            let mut raw = Map::new();
            raw.insert("type".to_owned(), Value::from("message"));
            raw.insert("ts".to_owned(), Value::from(ts.clone()));
            raw.insert("text".to_owned(), Value::from(text));
            raw.insert(author.0.to_owned(), Value::from(author.1));
            let bot = sender
                .and_then(|sender| sender.get("bot"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if bot {
                raw.insert("subtype".to_owned(), Value::from("bot_message"));
            }
            if let Some(name) = short_name(sender.and_then(|sender| sender.get("name"))) {
                let profile = if bot { "bot_profile" } else { "user_profile" };
                let key = if bot { "name" } else { "display_name" };
                raw.insert(profile.to_owned(), serde_json::json!({ key: name }));
            }
            if let Some(thread) = &thread_ts {
                raw.insert("thread_ts".to_owned(), Value::from(thread.clone()));
            }
            Ok(TailRecord::Message {
                sequence: None,
                conversation: conversation.to_owned(),
                ts,
                thread_ts,
                broadcast: false,
                raw: Value::Object(raw),
                bridge: true,
            })
        }
        "reaction.added" => {
            let named = field(record, "emoji")?.ok_or(refuse("emoji", "is missing"))?;
            let named = named.trim().trim_matches(':');
            if named.is_empty()
                || named.chars().count() > MAX_NAME_CHARS
                || named.chars().any(char::is_control)
            {
                return Err(refuse("emoji", "is not one short emoji or name"));
            }
            // A Slack name (`white_check_mark`) is the emoji it names, as a read reports it; the
            // emoji itself is taken as it is; any other name is a custom emoji's.
            let (emoji, custom) = if named.is_ascii() {
                match super::reactions::standard_emoji(named) {
                    Some(emoji) => (emoji, false),
                    None => (named.to_owned(), true),
                }
            } else {
                (named.to_owned(), false)
            };
            Ok(TailRecord::ReactionAdded {
                conversation: conversation.to_owned(),
                ts,
                emoji,
                custom,
            })
        }
        _ => Ok(TailRecord::Other {
            sequence: None,
            why: "a kind of bridge record this server does not act on",
        }),
    }
}

fn message(
    sequence: Option<u64>,
    payload: &Map<String, Value>,
    slack: &Map<String, Value>,
) -> Result<TailRecord, TailError> {
    let conversation = conversation_of(payload, Some(slack))?;
    let ts = field(slack, "message_ts")?.ok_or(refuse("message_ts", "is missing"))?;
    let ts = ids::canonical_ts(ts)
        .filter(|ts| ids::message_id_from_ts(ts).is_some())
        .ok_or(refuse("message_ts", "is not a Slack ts"))?;
    let thread_ts = match field(slack, "thread_ts")? {
        None => None,
        Some(raw) => Some(
            ids::canonical_ts(raw)
                .filter(|ts| ids::message_id_from_ts(ts).is_some())
                .ok_or(refuse("thread_ts", "is not a Slack ts"))?,
        ),
    };
    let user = field(slack, "slack_user_id")?;
    if user.is_some_and(|user| !ids::is_user_id(user)) {
        return Err(refuse("slack_user_id", "is not a Slack user id"));
    }
    let bot = field(slack, "bot_id")?;
    if bot.is_some_and(|bot| !is_prefixed_id(bot, b"B")) {
        return Err(refuse("bot_id", "is not a Slack bot id"));
    }
    let app = field(slack, "app_id")?;
    if app.is_some_and(|app| !is_prefixed_id(app, b"A")) {
        return Err(refuse("app_id", "is not a Slack app id"));
    }
    let subtype = field(slack, "subtype")?;
    if subtype.is_some_and(|subtype| !is_subtype(subtype)) {
        return Err(refuse("subtype", "is not a Slack message subtype"));
    }
    // A creation is never edited, but a record that says when its message was last edited
    // carries that time on, so the cache can rank this copy against a read's by edit time.
    let edited = match field(slack, "edited_ts")? {
        None => None,
        Some(raw) => Some(ids::canonical_ts(raw).ok_or(refuse("edited_ts", "is not a Slack ts"))?),
    };
    let team = field(slack, "team_id")?;
    if team.is_some_and(|team| !is_prefixed_id(team, b"TE")) {
        return Err(refuse("team_id", "is not a Slack workspace id"));
    }
    let text = match slack.get("text") {
        None | Some(Value::Null) => "",
        Some(Value::String(text)) => text,
        Some(_) => return Err(refuse("text", NOT_A_STRING)),
    };
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(refuse("text", "is longer than Slack's 40,000 characters"));
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
        bridge: false,
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

    /// A thread reply in the shape the stream was captured writing (identifiers and text
    /// replaced): the container names the workspace and the conversation, and the payload and
    /// the Slack object carry fields this parser does not use.
    fn captured(slack_extra: Value) -> Value {
        let mut slack = json!({
            "team_id": "T0TEAM00001", "channel_id": "C0000MAIN1",
            "message_ts": "1792637725.398100", "thread_ts": "1792637000.000100",
            "slack_user_id": "UOWNER0001", "slack_event_id": "Ev0001",
            "app_id": "A0APP000001", "text": "Maybe just re-roll if too similar?",
            "channel_type": "channel", "blocks_json": "[]"
        });
        for (key, value) in slack_extra.as_object().expect("an object") {
            slack[key] = value.clone();
        }
        json!({
            "seq": 7, "ts": 1.0, "type": "message.created", "source": "slack",
            "iris_seq_id": 7, "event_ts": 1_792_637_725_398_u64,
            "payload": {
                "event_id": "evt-0001", "source": "slack", "event_type": "message.created",
                "entity_type": "message", "entity_id": "T0TEAM00001:C0000MAIN1:1792637725.398100",
                "container_type": "channel", "container_id": "T0TEAM00001:C0000MAIN1",
                "actor_id": 1, "timestamp_ms": 1_792_637_725_398_u64,
                "actor_name": "Owner",
                "context": {"snippet": "a reply", "ancestry": {
                    "container": {"entity_type": "channel", "entity_id": "C0000MAIN1"}}},
                "viewer_relevance": {"is_actor_self": true},
                "slack_message": slack
            }
        })
    }

    #[test]
    fn the_captured_shape_with_a_workspace_prefixed_container_is_read() {
        let Ok(TailRecord::Message {
            sequence,
            conversation,
            thread_ts,
            raw,
            ..
        }) = parse(&captured(json!({})))
        else {
            panic!(
                "the captured record was refused: {:?}",
                parse(&captured(json!({})))
            );
        };
        assert_eq!(sequence, Some(7));
        assert_eq!(conversation, "C0000MAIN1");
        assert_eq!(thread_ts.as_deref(), Some("1792637000.000100"));
        assert_eq!(raw["user"], "UOWNER0001");
        assert_eq!(raw["app_id"], "A0APP000001");
        assert!(raw.get("blocks_json").is_none(), "only what a read carries");
    }

    #[test]
    fn bot_posts_broadcasts_file_shares_and_system_events_are_read() {
        let bot = captured(json!({"slack_user_id": null, "bot_id": "B0BOT00001",
            "subtype": "bot_message", "thread_ts": null}));
        let Ok(TailRecord::Message { raw, thread_ts, .. }) = parse(&bot) else {
            panic!("a bot post was refused");
        };
        assert_eq!(
            (raw["bot_id"].as_str(), thread_ts),
            (Some("B0BOT00001"), None)
        );

        let broadcast = captured(json!({"subtype": "thread_broadcast"}));
        let Ok(TailRecord::Message { broadcast, .. }) = parse(&broadcast) else {
            panic!("a broadcast was refused");
        };
        assert!(broadcast);

        for subtype in ["file_share", "me_message", "channel_join", "huddle_thread"] {
            assert!(
                matches!(
                    parse(&captured(json!({"subtype": subtype}))),
                    Ok(TailRecord::Message { .. })
                ),
                "{subtype}"
            );
        }
        // Slackbot, an enterprise workspace, an empty text, and no thread at all.
        let slackbot = captured(
            json!({"slack_user_id": "USLACKBOT", "team_id": "E0ENTERPRISE1",
            "text": "", "thread_ts": ""}),
        );
        assert!(matches!(
            parse(&slackbot),
            Ok(TailRecord::Message {
                thread_ts: None,
                ..
            })
        ));
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
            ..
        } = parse(&record).expect("valid")
        else {
            panic!("not a message");
        };
        assert_eq!(sequence, Some(42));
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
    fn every_malformed_identifier_or_oversized_text_refuses_the_record_by_field_and_rule() {
        let base = json!({
            "channel_id": "C0000MAIN1",
            "message_ts": "1700000000.000100",
            "slack_user_id": "UADA00001",
            "text": "fine",
        });
        let cases: Vec<(&str, Value, &str, &str)> = vec![
            (
                "channel_id",
                json!("general"),
                "channel_id",
                "is not a Slack conversation id",
            ),
            (
                "channel_id",
                json!("C0000OTHER"),
                "container_id",
                "names another conversation than slack_message.channel_id",
            ),
            (
                "message_ts",
                json!("yesterday"),
                "message_ts",
                "is not a Slack ts",
            ),
            (
                "message_ts",
                json!(1_700_000_000),
                "message_ts",
                NOT_A_STRING,
            ),
            (
                "thread_ts",
                json!("1.2.3"),
                "thread_ts",
                "is not a Slack ts",
            ),
            (
                "slack_user_id",
                json!("ada; rm -rf"),
                "slack_user_id",
                "is not a Slack user id",
            ),
            ("bot_id", json!("U0000"), "bot_id", "is not a Slack bot id"),
            ("app_id", json!("a0000"), "app_id", "is not a Slack app id"),
            (
                "subtype",
                json!("Bot Message"),
                "subtype",
                "is not a Slack message subtype",
            ),
            (
                "team_id",
                json!("x"),
                "team_id",
                "is not a Slack workspace id",
            ),
            ("text", json!(["not", "text"]), "text", NOT_A_STRING),
            (
                "text",
                json!("x".repeat(MAX_TEXT_CHARS + 1)),
                "text",
                "is longer than Slack's 40,000 characters",
            ),
            (
                "edited_ts",
                json!("later"),
                "edited_ts",
                "is not a Slack ts",
            ),
        ];
        for (key, value, field, rule) in cases {
            let mut slack = base.clone();
            slack[key] = value;
            let refused = parse(&created(slack)).expect_err(key);
            assert_eq!(refused, refuse(field, rule), "{key}");
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
        assert_eq!(
            parse(&missing).map(|_| ()).expect_err("no channel").field,
            "channel_id"
        );
        let mut bad_team = created(base.clone());
        bad_team["payload"]["container_id"] = json!("team:C0000MAIN1");
        bad_team["payload"]["slack_message"]
            .as_object_mut()
            .expect("message")
            .remove("channel_id");
        assert_eq!(
            parse(&bad_team)
                .map(|_| ())
                .expect_err("bad container")
                .field,
            "container_id"
        );
        let mut unsequenced = created(base);
        unsequenced["iris_seq_id"] = json!("42");
        assert_eq!(
            parse(&unsequenced)
                .map(|_| ())
                .expect_err("a string sequence")
                .field,
            "iris_seq_id"
        );
        assert_eq!(
            parse(&json!([1])).map(|_| ()).expect_err("an array").field,
            "record"
        );
    }

    #[test]
    fn a_container_this_cannot_read_beside_a_valid_channel_is_left_alone() {
        let mut record = captured(json!({}));
        record["payload"]["container_id"] = json!("123456789");
        assert!(matches!(parse(&record), Ok(TailRecord::Message { .. })));
    }

    #[test]
    fn control_gap_reaction_notice_and_unknown_records_are_told_apart() {
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
            parse(&json!({"type": "GAP", "lost_from_iris_seq_id": "x"})).map(|_| ()),
            Err(refuse(
                "lost_from_iris_seq_id",
                "is not a non-negative integer"
            ))
        );
        // A client notice with no sequence number is a control record, not a refusal.
        assert_eq!(
            parse(&json!({"type": "SUBSCRIBED"})),
            Ok(TailRecord::Control)
        );
        for container in ["C0000MAIN1", "T0TEAM00001:C0000MAIN1"] {
            assert_eq!(
                parse(&json!({
                    "type": "reaction.created",
                    "iris_seq_id": 7,
                    "payload": {"container_id": container, "slack_reaction": {"name": "eyes"}},
                })),
                Ok(TailRecord::Reaction {
                    sequence: Some(7),
                    conversation: "C0000MAIN1".to_owned()
                })
            );
        }
        assert!(matches!(
            parse(&json!({"type": "reaction.deleted", "iris_seq_id": 9, "payload": {}})),
            Ok(TailRecord::Other {
                sequence: Some(9),
                ..
            })
        ));
        assert!(matches!(
            parse(&json!({"type": "message.created", "iris_seq_id": 10, "payload": {}})),
            Ok(TailRecord::Other {
                sequence: Some(10),
                ..
            })
        ));
        assert!(matches!(
            parse(&json!({"type": "member.joined", "iris_seq_id": 8, "payload": {}})),
            Ok(TailRecord::Other {
                sequence: Some(8),
                ..
            })
        ));
        assert!(matches!(
            parse(&json!({"type": "conversation.updated", "iris_seq_id": 11})),
            Ok(TailRecord::Other {
                sequence: Some(11),
                ..
            })
        ));
    }

    /// A chat bridge's record of a post, as `agentctl chat run`'s event sink writes it.
    fn bridge_post(sender: Value, thread: Option<&str>) -> Value {
        json!({
            "schema": CHAT_EVENT_SCHEMA, "event_id": "message.posted:op-1",
            "kind": "message.posted", "at": "2026-10-30T16:25:17.953Z", "bridge": "coordinator",
            "channel_id": "C0000MAIN1", "message_id": "C0000MAIN1/messages/1792637725.398100",
            "thread_id": thread, "text": "That's expected while polling is paused",
            "text_format": "markdown",
            "sender": sender,
        })
    }

    #[test]
    fn a_bridge_post_becomes_the_message_a_read_would_return_by_the_same_author() {
        // `#708 vt-live-push`.
        let record = bridge_post(
            json!({"id": "UBRIDGE001", "name": "bridge", "bot": true}),
            Some("C0000MAIN1/threads/1792637000.000100"),
        );
        let Ok(TailRecord::Message {
            sequence,
            conversation,
            ts,
            thread_ts,
            broadcast,
            raw,
            bridge,
        }) = parse(&record)
        else {
            panic!("a message: {:?}", parse(&record));
        };
        assert_eq!(sequence, None);
        assert!(bridge);
        assert_eq!(conversation, "C0000MAIN1");
        assert_eq!(ts, "1792637725.398100");
        assert_eq!(thread_ts.as_deref(), Some("1792637000.000100"));
        assert!(!broadcast);
        assert_eq!(raw["user"], "UBRIDGE001");
        assert_eq!(raw["subtype"], "bot_message");
        assert_eq!(raw["bot_profile"]["name"], "bridge");
        assert_eq!(raw["thread_ts"], "1792637000.000100");
        assert_eq!(raw["text"], "That's expected while polling is paused");
        // Markdown's `&`, `<` and `>` reach normalization escaped, as Slack would hold them.
        let mut symbols = bridge_post(json!({"id": "UBRIDGE001"}), None);
        symbols["text"] = json!("a & b <c> <@UADA00001>");
        let Ok(TailRecord::Message { raw, .. }) = parse(&symbols) else {
            panic!("a message");
        };
        assert_eq!(raw["text"], "a &amp; b &lt;c&gt; <@UADA00001>");
        // A root names no thread, and a post that names no author is left to the poller.
        let root = bridge_post(
            json!({"id": "UBRIDGE001"}),
            Some("C0000MAIN1/threads/1792637725.398100"),
        );
        assert!(matches!(
            parse(&root),
            Ok(TailRecord::Message {
                thread_ts: None,
                ..
            })
        ));
        assert!(matches!(
            parse(&bridge_post(json!({"name": "bridge"}), None)),
            Ok(TailRecord::Unapplied { .. })
        ));
    }

    #[test]
    fn a_bridge_reaction_names_the_emoji_a_read_would_report() {
        let reaction = |emoji: &str| {
            json!({
                "schema": CHAT_EVENT_SCHEMA, "event_id": "reaction.added:op-2",
                "kind": "reaction.added", "channel_id": "C0000MAIN1",
                "message_id": "C0000MAIN1/messages/1792637725.398100", "emoji": emoji,
                "sender": {"id": "UBRIDGE001"},
            })
        };
        for (given, emoji, custom) in [
            ("✅", "✅", false),
            ("white_check_mark", "✅", false),
            (":eyes:", "👀", false),
            ("party-parrot", "party-parrot", true),
        ] {
            assert_eq!(
                parse(&reaction(given)),
                Ok(TailRecord::ReactionAdded {
                    conversation: "C0000MAIN1".to_owned(),
                    ts: "1792637725.398100".to_owned(),
                    emoji: emoji.to_owned(),
                    custom,
                }),
                "{given}"
            );
        }
        assert_eq!(
            parse(&reaction("::")),
            Err(TailError {
                field: "emoji",
                rule: "is not one short emoji or name"
            })
        );
    }

    #[test]
    fn a_bridge_record_naming_another_conversations_message_or_a_bad_author_is_refused() {
        let mut other = bridge_post(json!({"id": "UBRIDGE001"}), None);
        other["message_id"] = json!("C0000OTHR2/messages/1792637725.398100");
        assert_eq!(
            parse(&other),
            Err(TailError {
                field: "message_id",
                rule: "does not name a resource of channel_id"
            })
        );
        assert_eq!(
            parse(&bridge_post(json!({"id": "not an id"}), None)),
            Err(TailError {
                field: "sender.id",
                rule: "is not a Slack user or bot id"
            })
        );
        let mut unknown = bridge_post(json!({"id": "UBRIDGE001"}), None);
        unknown["kind"] = json!("message.admitted");
        assert!(matches!(parse(&unknown), Ok(TailRecord::Other { .. })));
    }
}
