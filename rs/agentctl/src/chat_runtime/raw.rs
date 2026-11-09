//! Raw messages (#1136 raw-message-bridge): an owner's message marked raw is typed into the
//! bridged agent exactly as written — no sender header, no reply instructions — so a slash
//! command sent as its own one-line raw message runs as terminal input. Everything about the
//! mark fails closed: an unparsable marker, a sender who is not a permanent owner, or a text
//! over the cap is an ordinary request, its prompt carrying a note that the mark was ignored.

use super::*;

/// The most characters one raw text may hold, marker line already taken off and controls
/// already stripped; the posting server refuses a longer raw text the same way.
pub(super) const RAW_MAX_CHARS: usize = 4_000;

/// The provider payload schema whose `raw` key this reader honours: the Slack plugin's
/// `PAYLOAD_SCHEMA`. A `raw` key under any other schema is some other provider's vocabulary.
const RAW_PAYLOAD_SCHEMA: &str = "agentctl.slack.message.v1";

/// What the posting server appends to a raw post's words as their last line.
const MARKER_SUFFIX: &str = "\n[raw]";

/// How one admitted message stands with the raw mark, decided once, at admission.
pub(super) enum Classification {
    /// No mark: an ordinary request.
    NotRaw,
    /// An owner's mark within the cap: type this bare text, verbatim.
    Raw(String),
    /// A mark that must not be honoured: an ordinary request whose prompt carries this note.
    Ignored(String),
}

/// Whether `message`'s provider payload marks it raw: the Slack payload's `raw` boolean,
/// exactly `true`. Anything else is not a mark.
fn marked(message: &SavedMessage) -> bool {
    message.provider_payload.as_ref().is_some_and(|payload| {
        payload.schema == RAW_PAYLOAD_SCHEMA
            && payload.data.get("raw").and_then(Value::as_bool) == Some(true)
    })
}

/// The text a raw message types, from the words as they reached the bridge: line endings
/// normalized to line feeds, exactly one trailing marker line taken back off, and the control
/// characters gone (C0 and C1 controls and DEL, tab included — it would open the composer's
/// completion) except the line feed itself. What remains is typed verbatim.
pub(super) fn clean(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let unmarked = normalized
        .strip_suffix(MARKER_SUFFIX)
        .unwrap_or(&normalized);
    unmarked
        .chars()
        .filter(|&character| {
            character == '\n' || !matches!(character, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
        })
        .collect()
}

/// Classify `message` for raw delivery. `permanent_owner` is the bridge's owner authority
/// (its configured `allowed_senders`, as admission already decided it): only an owner's mark
/// is typed raw, because raw text is direct terminal input.
pub(super) fn classify(message: &SavedMessage, permanent_owner: bool) -> Classification {
    if !marked(message) {
        return Classification::NotRaw;
    }
    if !permanent_owner {
        return Classification::Ignored(
            "Note: this message was marked raw, but only an owner's raw mark is typed into \
             the agent as written, so the mark was ignored and this is an ordinary request."
                .to_owned(),
        );
    }
    let cleaned = clean(&message.text);
    if cleaned.chars().count() > RAW_MAX_CHARS {
        return Classification::Ignored(format!(
            "Note: this message was marked raw, but a raw text is at most {RAW_MAX_CHARS} \
             characters and this one is longer, so the mark was ignored and this is an \
             ordinary request."
        ));
    }
    if cleaned.is_empty() {
        return Classification::Ignored(
            "Note: this message was marked raw, but nothing typeable remained once its \
             marker line and control characters were taken off, so the mark was ignored \
             and this is an ordinary request."
                .to_owned(),
        );
    }
    Classification::Raw(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str, payload: Option<(&str, Value)>) -> SavedMessage {
        SavedMessage {
            channel_id: "spaces/example".to_owned(),
            message_id: "spaces/example/messages/one".to_owned(),
            thread_id: "spaces/example/threads/one".to_owned(),
            sender_id: "users/owner".to_owned(),
            text: text.to_owned(),
            created_at: "2026-09-21T12:00:00Z".to_owned(),
            thread_reply: false,
            provider_payload: payload.map(|(schema, data)| ProviderPayloadDocument {
                schema: schema.to_owned(),
                data: data.as_object().expect("payload object").clone(),
            }),
        }
    }

    fn raw_message(text: &str) -> SavedMessage {
        message(
            text,
            Some((RAW_PAYLOAD_SCHEMA, serde_json::json!({"raw": true}))),
        )
    }

    #[test]
    fn cleaning_takes_one_marker_line_and_the_controls_and_keeps_the_rest_verbatim() {
        assert_eq!(clean("/help\n[raw]"), "/help");
        assert_eq!(clean("one\ntwo\n[raw]"), "one\ntwo");
        // Words that themselves end with the marker line keep theirs: exactly one comes off.
        assert_eq!(clean("look\n[raw]\n[raw]"), "look\n[raw]");
        assert_eq!(
            clean("[raw]"),
            "[raw]",
            "no line of words, nothing taken off"
        );
        assert_eq!(clean("/help\r\n[raw]"), "/help");
        assert_eq!(clean("a\r\nb\rc\n[raw]"), "a\nb\nc");
        // Tab, the C0/C1 controls and DEL go; line feeds and ordinary text stay.
        assert_eq!(clean("a\tb\u{1b}[31mc\u{7f}d\u{85}e\n[raw]"), "ab[31mcde");
        assert_eq!(clean("keep  spaces \n[raw]"), "keep  spaces ");
        assert_eq!(clean("no marker at all"), "no marker at all");
    }

    #[test]
    fn only_an_owners_exact_mark_within_the_cap_classifies_raw() {
        match classify(&raw_message("/help\n[raw]"), true) {
            Classification::Raw(text) => assert_eq!(text, "/help"),
            _ => panic!("an owner's marked slash command is raw"),
        }
        assert!(matches!(
            classify(&raw_message("/help\n[raw]"), false),
            Classification::Ignored(_)
        ));
        assert!(matches!(
            classify(&message("/help\n[raw]", None), true),
            Classification::NotRaw
        ));
        for payload in [
            serde_json::json!({"raw": false}),
            serde_json::json!({"raw": "true"}),
            serde_json::json!({"op_key": "k.1"}),
        ] {
            assert!(matches!(
                classify(
                    &message("/help\n[raw]", Some((RAW_PAYLOAD_SCHEMA, payload))),
                    true
                ),
                Classification::NotRaw
            ));
        }
        assert!(matches!(
            classify(
                &message(
                    "/help\n[raw]",
                    Some(("fixture.message.v1", serde_json::json!({"raw": true})))
                ),
                true
            ),
            Classification::NotRaw
        ));
        let over = format!("{}\n[raw]", "x".repeat(RAW_MAX_CHARS + 1));
        assert!(matches!(
            classify(&raw_message(&over), true),
            Classification::Ignored(_)
        ));
        let at = format!("{}\n[raw]", "x".repeat(RAW_MAX_CHARS));
        assert!(matches!(
            classify(&raw_message(&at), true),
            Classification::Raw(_)
        ));
        assert!(matches!(
            classify(&raw_message("\n[raw]"), true),
            Classification::Ignored(_)
        ));
    }
}
