//! A thread's layout before its bodies. `#2385 vibe-talk-thread-open-layout-first`.
//!
//! # Why this exists
//!
//! The owner, 2026-11-27, of a thread opened on a weak connection that showed one reply for a long
//! while and then the rest at once: "blank boxes would have appeared from the very first packets
//! that are received instead of waiting in this ambiguous state for messages to then all the
//! sudden appear later." The page draws a box for each reply it lacks from the root's count alone,
//! all of one height. Before the bodies, this server knows each reply's id, time, author and size,
//! and sent only the count.
//!
//! # What it answers
//!
//! `GET /api/v1/channels/{id}/threads/{thread_id}/layout`: the thread's reply count as the copy
//! records it, and one entry per reply the copy holds, in thread order, with the stored body's
//! length and lines and what it is (shares a file, folds). The page prices each box from its own
//! entry. It is answered from this server's own copy and nothing else: no read of the chat service
//! begins for it, inline or behind (the thread read the page sends beside it schedules any fill),
//! so it answers while the provider is held (`tests/state_sync.rs`) and at any provider's rate
//! limits. A copy that holds none of the thread's replies, or may not answer for them, answers no
//! entries, and the page keeps the boxes it priced from the count.
//!
//! # A first slice
//!
//! The first slice of the sparse incremental loading design, scoped to one thread's view; the
//! sizes are estimates, said so on every entry (`estimated`).

use crate::cache::{CachedMessage, ThreadRecord};
use crate::contract::{ThreadLayoutEntry, ThreadLayoutFlags, ThreadLayoutResponse};
use crate::model::{ChannelId, Message};

/// Past this many UTF-16 code units the page folds a body: the page's own `COLLAPSE_OVER_CHARS`
/// (`web/src/view/channel/folding.ts`, compared against JavaScript's `String.length`), mirrored
/// here so an entry says what the page will draw. Change the two together.
pub const LONG_OVER_UTF16: u64 = 280;

/// How many replies an answer carries when the request does not say: the newest page the page's
/// thread read asks for (`DISCORD_PAGE_LIMIT`, `web/src/core/channels/paging.ts`), which is what
/// its boxes stand for.
pub const DEFAULT_LIMIT: usize = 50;

/// The most replies one answer carries, as for a timeline page.
pub const MAX_LIMIT: usize = crate::ops::MAX_PAGE as usize;

/// Whether `content` lists a shared file or an embed: a line of its own starting with the file
/// mark, as every provider's file lines are drawn (`crate::slack::files`).
#[must_use]
pub fn lists_a_file(content: &str) -> bool {
    content.lines().any(|line| {
        line.trim_start()
            .starts_with(crate::slack::files::FILE_MARK)
    })
}

/// One reply's entry, from its stored message and creation time.
#[must_use]
pub fn entry(message: &Message, created_ms: i64) -> ThreadLayoutEntry {
    let length_utf16 = u64::try_from(message.content.encode_utf16().count()).unwrap_or(u64::MAX);
    let breaks = message.content.matches('\n').count();
    ThreadLayoutEntry {
        id: message.id.clone(),
        created_ms,
        author_id: message.author_id.clone(),
        length_utf16,
        lines: u64::try_from(breaks).unwrap_or(u64::MAX).saturating_add(1),
        flags: ThreadLayoutFlags {
            attachments: lists_a_file(&message.content),
            long: length_utf16 > LONG_OVER_UTF16,
        },
        estimated: true,
    }
}

/// Whether `row` is the thread's root rather than one of its replies.
fn is_root(row: &CachedMessage, record: Option<&ThreadRecord>, thread_id: &str) -> bool {
    row.message
        .thread
        .as_ref()
        .is_some_and(|thread| thread.is_root)
        || record
            .and_then(|found| found.root_id.as_ref())
            .is_some_and(|root| *root == row.message.id)
        || row.message.id.0 == thread_id
}

/// The answer for `thread_id` of `channel_id` from what the copy holds: its record of the thread,
/// when it has one, and the thread scope's newest rows, oldest first (more than `limit` when there
/// are more). The root is left out; the newest `limit` replies are kept, in thread order.
#[must_use]
pub fn answer(
    channel_id: ChannelId,
    thread_id: &str,
    record: Option<&ThreadRecord>,
    rows: Vec<CachedMessage>,
    limit: usize,
) -> ThreadLayoutResponse {
    let mut replies: Vec<CachedMessage> = rows
        .into_iter()
        .filter(|row| !is_root(row, record, thread_id))
        .collect();
    replies.sort_by(|a, b| (a.created_ms, &a.message.id.0).cmp(&(b.created_ms, &b.message.id.0)));
    replies.dedup_by(|a, b| a.message.id == b.message.id);
    let more_before = replies.len() > limit;
    let skip = replies.len().saturating_sub(limit);
    ThreadLayoutResponse {
        channel_id,
        thread_id: thread_id.to_owned(),
        total: record.and_then(|found| found.reply_count),
        total_exact: record
            .is_some_and(|found| found.reply_count.is_some() && found.reply_count_exact),
        entries: replies
            .iter()
            .skip(skip)
            .map(|row| entry(&row.message, row.created_ms))
            .collect(),
        more_before,
    }
}

/// The answer when the copy may not answer for the thread at all: no entries, no count.
#[must_use]
pub fn empty(channel_id: ChannelId, thread_id: &str) -> ThreadLayoutResponse {
    ThreadLayoutResponse {
        channel_id,
        thread_id: thread_id.to_owned(),
        total: None,
        total_exact: false,
        entries: Vec::new(),
        more_before: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MessageId, UserId};
    use crate::threads::MessageThread;

    fn row(id: &str, created_ms: i64, content: &str, root: bool) -> CachedMessage {
        CachedMessage {
            message: Message {
                thread: Some(
                    serde_json::from_value::<MessageThread>(serde_json::json!({
                        "id": "100.000",
                        "root_message_id": "100.000",
                        "is_root": root,
                        "reply_count": null,
                        "reply_count_exact": false,
                    }))
                    .expect("a thread record"),
                ),
                id: MessageId(id.into()),
                channel_id: ChannelId("C".into()),
                author: "Ada".into(),
                author_id: UserId(format!("U{id}")),
                author_is_bot: false,
                timestamp: String::new(),
                spoken_time: String::new(),
                edited_timestamp: None,
                reply_to: None,
                content: content.into(),
                spoken_content: String::new(),
                content_html: String::new(),
                noise: false,
                labels: Vec::new(),
                plain_labels: Vec::new(),
                manual_labels: Vec::new(),
                sent_refs: Vec::new(),
                sent_vestigial: false,
                reactions: None,
                heading: None,
                mentions: Vec::new(),
                upstream_url: None,
                upstream_app_url: None,
                op_key: None,
                observed_ms: None,
            },
            created_ms,
            observed_ms: created_ms,
            content_partial: false,
            vouched_ms: None,
        }
    }

    #[test]
    fn the_fold_threshold_mirrors_the_page() {
        // The page's constant, read from its source, so the two cannot drift apart unseen.
        let page = include_str!("../web/src/view/channel/folding.ts");
        assert!(
            page.contains(&format!(
                "export const COLLAPSE_OVER_CHARS = {LONG_OVER_UTF16};"
            )),
            "web/src/view/channel/folding.ts's COLLAPSE_OVER_CHARS is no longer {LONG_OVER_UTF16}"
        );
    }

    #[test]
    fn an_entry_measures_the_body_as_the_page_does() {
        let long = "x".repeat(281);
        let at = entry(&row("1", 5, &long, false).message, 5);
        assert_eq!((at.length_utf16, at.lines), (281, 1));
        assert!(at.flags.long && !at.flags.attachments && at.estimated);
        let edge = entry(&row("2", 6, &"y".repeat(280), false).message, 6);
        assert!(!edge.flags.long, "280 is not over the threshold");
        // One astral character is two UTF-16 code units, as `String.length` counts it.
        let emoji = entry(&row("3", 7, "ok \u{1F680}\nnext", false).message, 7);
        assert_eq!((emoji.length_utf16, emoji.lines), (10, 2));
        let file = entry(
            &row(
                "4",
                8,
                "see this\n\n📎 [a.png](<https://x.example/a>) · PNG",
                false,
            )
            .message,
            8,
        );
        assert!(file.flags.attachments && !file.flags.long);
        assert_eq!(file.author_id, UserId("U4".into()));
    }

    #[test]
    fn the_answer_is_the_newest_replies_in_thread_order_without_the_root() {
        let record = ThreadRecord {
            thread_id: "100.000".into(),
            root_id: Some(MessageId("100.000".into())),
            reply_count: Some(9),
            reply_count_exact: true,
            last_activity_ms: 40,
        };
        let rows = vec![
            row("100.000", 1, "the root", true),
            row("101", 10, "a", false),
            row("103", 30, "ccc", false),
            row("102", 20, "bb", false),
            row("104", 40, "dddd", false),
        ];
        let all = answer(
            ChannelId("C".into()),
            "100.000",
            Some(&record),
            rows.clone(),
            50,
        );
        let ids: Vec<&str> = all.entries.iter().map(|e| e.id.0.as_str()).collect();
        assert_eq!(ids, ["101", "102", "103", "104"]);
        assert_eq!(
            (all.total, all.total_exact, all.more_before),
            (Some(9), true, false)
        );
        let newest = answer(ChannelId("C".into()), "100.000", Some(&record), rows, 2);
        let ids: Vec<&str> = newest.entries.iter().map(|e| e.id.0.as_str()).collect();
        assert_eq!(ids, ["103", "104"]);
        assert!(newest.more_before);
        let unrecorded = answer(ChannelId("C".into()), "100.000", None, Vec::new(), 50);
        assert_eq!((unrecorded.total, unrecorded.total_exact), (None, false));
        assert!(unrecorded.entries.is_empty());
    }
}
