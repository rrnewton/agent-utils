//! The backward cursor a timeline page answered from the message cache carries. `#384
//! cache-serve`, `#510 cache-serve-every-read`.
//!
//! A served page cannot hand out the provider's own `next_before`: no provider read made it. It
//! used to go without one, and the view was then not served again until a reader had been given
//! a provider page, so that older history was never stranded behind a page with no way back. That
//! left every other newest read of a view going upstream: served, then upstream, then served,
//! exactly half of what the rule would serve.
//!
//! Instead a served page carries this cursor, which says only "older than the oldest message I
//! showed". A step back with it reads the provider's own newest page, offered to the cache as any
//! newest read is, and walks back on the provider's cursors from there until it reaches a message
//! older than that one; it answers with those older messages and the provider's own `next_before`,
//! so every step after it is the provider's. Nothing is kept on the server: the cursor works after
//! a restart, from any tab, and the view may be served again at once.
//!
//! [`decode`] binds the cursor to the channel, view and thread it was issued for, as
//! [`crate::timeline_forward::decode`] does a forward one. It is unsigned for the same reason:
//! forging one only reads the same allowlisted channel the caller can already read. A backend's
//! own cursor never begins with [`PREFIX`], and is handed to the backend untouched.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::chat::ChatError;
use crate::model::{ChannelId, Message};
use crate::threads::{TimelinePage, TimelineView};
use crate::timeline_forward::{Mismatch, MAX_ENVELOPE};

/// What every served backward cursor begins with: not a shape any backend's cursor takes.
pub const PREFIX: &str = "vt-served.";
/// The most provider pages one step back from a served page reads, its newest page included,
/// looking for a message older than the served page's oldest.
pub const WALK_PAGES: usize = 4;
/// The envelope format. A different one is refused rather than guessed at.
const VERSION: u8 = 1;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    v: u8,
    c: String,
    w: TimelineView,
    t: Option<String>,
    /// The served page's oldest message: its provider timestamp and id. None for an empty page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    a: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    i: Option<String>,
}

/// Where a step back from a served page continues from: below its oldest message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Below {
    /// The served page's oldest message, when it had one: when it was written, and its id.
    oldest: Option<(Timestamp, String)>,
}

impl Below {
    /// Whether `message` is older than every message the served page showed: written before its
    /// oldest, or at the same instant and not that message. One whose time does not read is
    /// kept, as the provider listed it below its newer neighbours.
    #[must_use]
    pub fn holds(&self, message: &Message) -> bool {
        let Some((at, id)) = &self.oldest else {
            return true;
        };
        message
            .timestamp
            .parse::<Timestamp>()
            .map_or(true, |written| {
                written < *at || (written == *at && message.id.as_str() != id)
            })
    }

    /// `page` as an older page of the served one: only its messages [`Below::holds`] keeps, and
    /// nothing a newest read says of the newest end (`next_after`, a delta).
    #[must_use]
    pub fn older(&self, mut page: TimelinePage) -> TimelinePage {
        page.messages.retain(|message| self.holds(message));
        page.next_after = None;
        page.delta = None;
        page
    }
}

/// The backward cursor for a page served from the cache whose oldest message is `oldest`, bound
/// to the channel, view and thread it is issued for.
///
/// # Errors
///
/// [`ChatError::Shape`] when the cursor would exceed [`MAX_ENVELOPE`].
pub fn encode(
    channel: &ChannelId,
    view: TimelineView,
    thread: Option<&str>,
    oldest: Option<&Message>,
) -> Result<String, ChatError> {
    let envelope = Envelope {
        v: VERSION,
        c: channel.0.clone(),
        w: view,
        t: thread.map(str::to_owned),
        a: oldest.map(|message| message.timestamp.clone()),
        i: oldest.map(|message| message.id.0.clone()),
    };
    let bytes = serde_json::to_vec(&envelope)
        .map_err(|_| ChatError::Shape("could not encode the backward cursor".to_owned()))?;
    let encoded = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    if encoded.len() > MAX_ENVELOPE {
        return Err(ChatError::Shape(format!(
            "the backward cursor would be {} characters; at most {MAX_ENVELOPE} are accepted",
            encoded.len()
        )));
    }
    Ok(encoded)
}

/// Read a `before` cursor: `Ok(None)` for a backend's own, to hand it untouched, and
/// `Ok(Some(_))` for one a served page issued for exactly this channel, view and thread.
///
/// # Errors
///
/// [`Mismatch`] for a cursor that begins with [`PREFIX`] and is not one this server issued for
/// exactly this read.
pub fn decode(
    raw: &str,
    channel: &ChannelId,
    view: TimelineView,
    thread: Option<&str>,
) -> Result<Option<Below>, Mismatch> {
    let Some(body) = raw.strip_prefix(PREFIX) else {
        return Ok(None);
    };
    if raw.len() > MAX_ENVELOPE {
        return Err(Mismatch);
    }
    let envelope: Envelope = URL_SAFE_NO_PAD
        .decode(body)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or(Mismatch)?;
    if envelope.v != VERSION
        || envelope.c != channel.as_str()
        || envelope.w != view
        || envelope.t.as_deref() != thread
    {
        return Err(Mismatch);
    }
    let oldest = match (envelope.a, envelope.i) {
        (Some(at), Some(id)) => Some((at.parse::<Timestamp>().map_err(|_| Mismatch)?, id)),
        (None, None) => None,
        _ => return Err(Mismatch),
    };
    Ok(Some(Below { oldest }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MessageId, UserId};

    fn channel() -> ChannelId {
        ChannelId("C1".to_owned())
    }

    fn message(id: &str, timestamp: &str) -> Message {
        Message {
            edited_timestamp: None,
            id: MessageId(id.to_owned()),
            channel_id: channel(),
            author: "Reader".to_owned(),
            author_id: UserId("7".to_owned()),
            author_is_bot: false,
            timestamp: timestamp.to_owned(),
            spoken_time: String::new(),
            reply_to: None,
            content: format!("message {id}"),
            spoken_content: String::new(),
            content_html: String::new(),
            thread: None,
            noise: false,
            reactions: None,
            heading: None,
            mentions: Vec::new(),
            upstream_url: None,
            upstream_app_url: None,
            op_key: None,
            observed_ms: None,
        }
    }

    #[test]
    fn a_cursor_round_trips_bound_to_its_channel_view_and_thread() {
        let oldest = message("m7", "2026-08-18T12:07:00Z");
        let raw = encode(&channel(), TimelineView::Main, None, Some(&oldest)).expect("encodes");
        assert!(raw.starts_with(PREFIX), "{raw}");
        let below = decode(&raw, &channel(), TimelineView::Main, None)
            .expect("decodes")
            .expect("a served cursor");
        assert!(below.holds(&message("m6", "2026-08-18T12:06:00Z")));
        assert!(
            !below.holds(&oldest),
            "the served page's own oldest is not older"
        );
        assert!(!below.holds(&message("m8", "2026-08-18T12:08:00Z")));
        assert!(
            below.holds(&message("m7b", "2026-08-18T12:07:00Z")),
            "another message of the same instant may lie below it"
        );

        let other = ChannelId("C2".to_owned());
        assert_eq!(
            decode(&raw, &other, TimelineView::Main, None),
            Err(Mismatch)
        );
        assert_eq!(
            decode(&raw, &channel(), TimelineView::Flat, None),
            Err(Mismatch)
        );
        assert_eq!(
            decode(&raw, &channel(), TimelineView::Main, Some("t1")),
            Err(Mismatch)
        );
    }

    #[test]
    fn a_backends_own_cursor_is_left_to_the_backend_and_a_forged_one_is_refused() {
        assert_eq!(
            decode("dXNlcjpVMDYxTkZUVDI=", &channel(), TimelineView::Main, None),
            Ok(None)
        );
        assert_eq!(
            decode(
                &format!("{PREFIX}not-json"),
                &channel(),
                TimelineView::Main,
                None
            ),
            Err(Mismatch)
        );
    }

    #[test]
    fn an_empty_served_page_walks_back_over_everything() {
        let raw = encode(&channel(), TimelineView::Main, None, None).expect("encodes");
        let below = decode(&raw, &channel(), TimelineView::Main, None)
            .expect("decodes")
            .expect("a served cursor");
        assert!(below.holds(&message("m1", "2026-08-18T12:01:00Z")));
    }

    #[test]
    fn an_older_page_keeps_only_what_lies_below_and_says_nothing_of_the_newest_end() {
        let oldest = message("m7", "2026-08-18T12:07:00Z");
        let raw = encode(&channel(), TimelineView::Main, None, Some(&oldest)).expect("encodes");
        let below = decode(&raw, &channel(), TimelineView::Main, None)
            .expect("decodes")
            .expect("a served cursor");
        let page = TimelinePage {
            messages: vec![
                message("m6", "2026-08-18T12:06:00Z"),
                oldest,
                message("m8", "2026-08-18T12:08:00Z"),
            ],
            next_after: Some("forward".to_owned()),
            next_before: Some("backend".to_owned()),
            has_more: true,
            ..TimelinePage::default()
        };
        let older = below.older(page);
        let ids: Vec<_> = older.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["m6"]);
        assert_eq!(older.next_after, None);
        assert_eq!(older.next_before.as_deref(), Some("backend"));
        assert!(older.has_more);
    }
}
