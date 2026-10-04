//! Pages, timeline views, posting, registration and discovery, on top of the bounded walks.
//!
//! # The four timeline views
//!
//! * **Main** — a whole conversation's own timeline, thread roots included, newest page first.
//!   For a channel narrowed to one thread it is that thread, root and replies, and the page names
//!   the thread in [`TimelinePage::thread`] with `has_threads` false.
//! * **Threads** — roots with replies, found by scanning the newest
//!   [`THREAD_SCAN_PAGES`] pages of history, ordered by their latest reply. A scan that stops on
//!   that bound says so in the page's notice.
//! * **Thread** — one thread's messages, after `conversations.replies` has confirmed the thread
//!   exists in this conversation.
//! * **Flat** — the Main page plus the replies of the threads rooted on it, at most
//!   [`FLAT_MAX_THREADS`] threads and the newest [`FLAT_MAX_REPLIES`] replies of each, merged
//!   oldest first. A notice states the bound whenever it applied.
//!
//! Cursors are opaque, bound to the channel, view and thread they were issued for, and carry a
//! Slack `ts` (or, for the thread list, an activity instant and a root) as their boundary.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use futures_util::{stream, StreamExt as _, TryStreamExt as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::client::{refused, rfc3339, shape, HttpSlackClient, Raw};
use super::history::{Feed, SCAN_PAGE_LIMIT};
use super::ids::{self, ChannelRef};
use crate::chat::{ChatError, RegisteredChannel};
use crate::directory::{DirectoryEntry, DirectoryPage, DirectoryRequest, MAX_NAME_CHARS};
use crate::model::{ChannelId, Message, MessageId};
use crate::threads::{ThreadSummary, TimelinePage, TimelineRequest, TimelineView};

/// The most entries one timeline page returns.
pub const MAX_TIMELINE_LIMIT: u16 = 100;
/// Pages of history the thread list scans for roots.
pub const THREAD_SCAN_PAGES: usize = 10;
/// Threads whose replies one Flat page includes.
pub const FLAT_MAX_THREADS: usize = 10;
/// Newest replies of each thread one Flat page includes.
pub const FLAT_MAX_REPLIES: usize = 50;
/// Characters of a root's first line used as its thread title.
pub const THREAD_TITLE_CHARS: usize = 80;
/// Pages of `users.conversations` one discovery request may read while filtering.
pub const DISCOVERY_PAGES: usize = 10;
/// The page size asked of `users.conversations`.
const DISCOVERY_PAGE_LIMIT: u16 = 100;
/// The longest one timeline request may take, all of its requests together.
const TIMELINE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);
/// The conversation kinds the channel browser lists.
const DISCOVERY_TYPES: &str = "public_channel,private_channel,mpim,im";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    channel: String,
    view: TimelineView,
    thread_id: Option<String>,
    boundary: String,
}

impl Cursor {
    fn encode(&self) -> Result<String, ChatError> {
        serde_json::to_vec(self)
            .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
            .map_err(|_| shape("could not encode the timeline continuation"))
    }

    fn decode(
        raw: &str,
        channel: &ChannelId,
        request: &TimelineRequest,
    ) -> Result<Self, ChatError> {
        let cursor: Self = URL_SAFE_NO_PAD
            .decode(raw)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or_else(|| refused("invalid timeline cursor; refresh this view"))?;
        if cursor.channel != channel.as_str()
            || cursor.view != request.view
            || cursor.thread_id != request.thread_id
        {
            return Err(refused(
                "the timeline cursor belongs to a different channel or view",
            ));
        }
        Ok(cursor)
    }

    /// The boundary as an instant, for every view but the thread list.
    fn instant(&self) -> Result<i64, ChatError> {
        ids::ts_micros(&self.boundary).ok_or_else(|| refused("invalid timeline cursor boundary"))
    }

    /// The boundary as `(activity, root)` instants, for the thread list.
    fn activity(&self) -> Result<(i64, i64), ChatError> {
        self.boundary
            .split_once(':')
            .and_then(|(activity, root)| Some((activity.parse().ok()?, root.parse().ok()?)))
            .ok_or_else(|| refused("invalid thread-list cursor"))
    }
}

fn validate_request(request: &TimelineRequest) -> Result<(), ChatError> {
    if request.limit == 0 || request.before.as_ref().is_some_and(|c| c.len() > 8192) {
        return Err(refused(
            "timeline page size must be positive and its cursor at most 8192 bytes",
        ));
    }
    let thread = request.thread_id.as_deref();
    if (request.view == TimelineView::Thread) != thread.is_some() {
        return Err(refused(
            "supply a thread id only when reading the thread view",
        ));
    }
    Ok(())
}

/// A thread id from a caller, as a canonical root `ts`.
fn thread_ts(thread_id: &str) -> Result<String, ChatError> {
    ids::canonical_ts(thread_id)
        .filter(|canonical| canonical == thread_id)
        .ok_or_else(|| refused("a Slack thread id is its root ts, such as 1700000000.000100"))
}

/// A cursor id from a caller, as an instant.
fn cursor_micros(id: &MessageId) -> Result<i64, ChatError> {
    ids::message_id_micros(id).ok_or_else(|| {
        refused(&format!(
            "{id} is not a message id this Slack backend issued"
        ))
    })
}

/// The first line of a root, trimmed to [`THREAD_TITLE_CHARS`].
fn title(root: Option<&Message>) -> String {
    let line = root
        .and_then(|root| root.content.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or("");
    if line.is_empty() {
        return "Thread".to_owned();
    }
    if line.chars().count() <= THREAD_TITLE_CHARS {
        return line.to_owned();
    }
    let cut: String = line.chars().take(THREAD_TITLE_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

/// A display name the directory contract accepts: no control characters, bounded length.
fn directory_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let clean = clean.trim();
    if clean.chars().count() <= MAX_NAME_CHARS {
        clean.to_owned()
    } else {
        clean.chars().take(MAX_NAME_CHARS).collect()
    }
}

/// `mpdm-ada--grace--alan-1` as `Group: ada, grace, alan`; anything else unchanged.
fn group_name(name: &str) -> String {
    name.strip_prefix("mpdm-")
        .and_then(|rest| rest.rsplit_once('-').map(|(members, _)| members))
        .map(|members| {
            format!(
                "Group: {}",
                members.split("--").collect::<Vec<_>>().join(", ")
            )
        })
        .unwrap_or_else(|| name.to_owned())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectoryCursor {
    /// Slack's cursor for the page to read; empty for the first page.
    c: String,
    /// Conversations of that page already consumed.
    o: usize,
}

/// One backward page of messages, before conversion.
struct RawPage {
    raws: Vec<Raw>,
    has_more: bool,
    next_before: Option<String>,
}

impl HttpSlackClient {
    /// [`crate::chat::ChatClient::fetch_page`], without the provider label.
    pub(super) async fn read_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError> {
        if before.is_some() && after.is_some() {
            return Err(refused(
                "before and after cannot both be given: the chat API does not define what that means",
            ));
        }
        let target = Self::channel_ref(channel)?;
        let want = usize::from(limit.max(1));
        let before = before.map(cursor_micros).transpose()?;
        let after = after.map(cursor_micros).transpose()?;
        let conversation = target.conversation.as_str();
        let raws = match (&target.thread_ts, after) {
            (Some(thread), Some(after)) => {
                self.replies_oldest_after(conversation, thread, Some(after), want)
                    .await?
            }
            (Some(thread), None) => {
                self.replies_newest_before(conversation, thread, before, want)
                    .await?
            }
            (None, Some(after)) => self.history_oldest_after(conversation, after, want).await?,
            (None, None) => self.history_newest(conversation, before, want).await?.real,
        };
        if raws.iter().any(Raw::is_thread_root) {
            self.mark_threaded(conversation);
        }
        Ok(self.messages(channel, &raws).await)
    }

    /// [`crate::chat::ChatClient::fetch_timeline`], without the provider label.
    pub(super) async fn timeline(
        &self,
        channel: &ChannelId,
        request: &TimelineRequest,
    ) -> Result<TimelinePage, ChatError> {
        tokio::time::timeout(TIMELINE_TIMEOUT, self.timeline_inner(channel, request))
            .await
            .map_err(|_| {
                refused("the timeline exceeded its time budget; no partial timeline was returned")
            })?
    }

    async fn timeline_inner(
        &self,
        channel: &ChannelId,
        request: &TimelineRequest,
    ) -> Result<TimelinePage, ChatError> {
        validate_request(request)?;
        let target = Self::channel_ref(channel)?;
        let cursor = request
            .before
            .as_deref()
            .map(|raw| Cursor::decode(raw, channel, request))
            .transpose()?;
        let limit = usize::from(request.limit.clamp(1, MAX_TIMELINE_LIMIT));
        let conversation = target.conversation.as_str();
        if let Some(scope) = &target.thread_ts {
            return self
                .scoped_timeline(channel, conversation, scope, request, cursor, limit)
                .await;
        }
        match request.view {
            TimelineView::Main => {
                let page = self
                    .main_raw(channel, conversation, request, cursor.as_ref(), limit)
                    .await?;
                Ok(TimelinePage {
                    messages: self.messages(channel, &page.raws).await,
                    has_threads: self.is_threaded(conversation),
                    has_more: page.has_more,
                    next_before: page.next_before,
                    ..TimelinePage::default()
                })
            }
            TimelineView::Flat => {
                self.flat(channel, conversation, request, cursor.as_ref(), limit)
                    .await
            }
            TimelineView::Threads => {
                self.thread_list(channel, conversation, request, cursor.as_ref(), limit)
                    .await
            }
            TimelineView::Thread => {
                let thread = thread_ts(request.thread_id.as_deref().unwrap_or_default())?;
                let root = self.thread_root(conversation, &thread).await?;
                self.mark_threaded(conversation);
                let page = self
                    .thread_raw(
                        channel,
                        conversation,
                        &thread,
                        request,
                        cursor.as_ref(),
                        limit,
                    )
                    .await?;
                Ok(TimelinePage {
                    messages: self.messages(channel, &page.raws).await,
                    thread: Some(self.summary(channel, &root).await),
                    has_threads: true,
                    has_more: page.has_more,
                    next_before: page.next_before,
                    ..TimelinePage::default()
                })
            }
        }
    }

    /// A channel narrowed to one thread: its Main is the whole thread, and it has no children.
    async fn scoped_timeline(
        &self,
        channel: &ChannelId,
        conversation: &str,
        scope: &str,
        request: &TimelineRequest,
        cursor: Option<Cursor>,
        limit: usize,
    ) -> Result<TimelinePage, ChatError> {
        if request
            .thread_id
            .as_deref()
            .is_some_and(|thread| thread != scope)
        {
            return Err(refused(
                "that thread does not belong to the configured channel",
            ));
        }
        let root = self.thread_root(conversation, scope).await?;
        let summary = self.summary(channel, &root).await;
        if request.view == TimelineView::Threads {
            return Ok(TimelinePage {
                thread: Some(summary),
                ..TimelinePage::default()
            });
        }
        let page = self
            .thread_raw(
                channel,
                conversation,
                scope,
                request,
                cursor.as_ref(),
                limit,
            )
            .await?;
        Ok(TimelinePage {
            messages: self.messages(channel, &page.raws).await,
            thread: Some(summary),
            has_threads: false,
            has_more: page.has_more,
            next_before: page.next_before,
            ..TimelinePage::default()
        })
    }

    /// One backward page of a conversation's own timeline.
    async fn main_raw(
        &self,
        channel: &ChannelId,
        conversation: &str,
        request: &TimelineRequest,
        cursor: Option<&Cursor>,
        limit: usize,
    ) -> Result<RawPage, ChatError> {
        let before = cursor.map(Cursor::instant).transpose()?;
        let run = self.history_newest(conversation, before, limit + 1).await?;
        let mut raws = run.real;
        let overflow = raws.len() > limit;
        if overflow {
            raws.drain(..raws.len() - limit);
        }
        if raws.iter().any(Raw::is_thread_root) {
            self.mark_threaded(conversation);
        }
        // On its page budget the walk stopped short, not at the end: continue from where it
        // stopped, which is at or before the oldest message returned.
        let boundary = if overflow {
            raws.first().map(|raw| raw.micros)
        } else {
            run.exhausted_at
        };
        let next_before = boundary
            .map(|micros| self.cursor(channel, request, ids::micros_ts(micros)))
            .transpose()?;
        Ok(RawPage {
            raws,
            has_more: next_before.is_some(),
            next_before,
        })
    }

    /// One backward page of one thread, root included on its oldest page.
    async fn thread_raw(
        &self,
        channel: &ChannelId,
        conversation: &str,
        thread: &str,
        request: &TimelineRequest,
        cursor: Option<&Cursor>,
        limit: usize,
    ) -> Result<RawPage, ChatError> {
        let before = cursor.map(Cursor::instant).transpose()?;
        let mut raws = self
            .replies_newest_before(conversation, thread, before, limit + 1)
            .await?;
        let has_more = raws.len() > limit;
        if has_more {
            raws.drain(..raws.len() - limit);
        }
        let next_before = if has_more {
            Some(self.cursor(channel, request, raws[0].ts.clone())?)
        } else {
            None
        };
        Ok(RawPage {
            raws,
            has_more,
            next_before,
        })
    }

    fn cursor(
        &self,
        channel: &ChannelId,
        request: &TimelineRequest,
        boundary: String,
    ) -> Result<String, ChatError> {
        Cursor {
            channel: channel.0.clone(),
            view: request.view,
            thread_id: request.thread_id.clone(),
            boundary,
        }
        .encode()
    }

    /// The Main page plus the replies of the threads rooted on it, bounded.
    async fn flat(
        &self,
        channel: &ChannelId,
        conversation: &str,
        request: &TimelineRequest,
        cursor: Option<&Cursor>,
        limit: usize,
    ) -> Result<TimelinePage, ChatError> {
        let page = self
            .main_raw(channel, conversation, request, cursor, limit)
            .await?;
        let mut roots: Vec<&Raw> = page
            .raws
            .iter()
            .filter(|raw| raw.is_thread_root())
            .collect();
        roots.sort_by_key(|root| std::cmp::Reverse(root.activity_micros()));
        let bounded = roots.len() > FLAT_MAX_THREADS
            || roots
                .iter()
                .take(FLAT_MAX_THREADS)
                .any(|root| root.reply_count() > FLAT_MAX_REPLIES as u64);
        let threads: Vec<String> = roots
            .iter()
            .take(FLAT_MAX_THREADS)
            .map(|root| root.ts.clone())
            .collect();
        let reads: Vec<_> = threads
            .iter()
            .map(|thread| self.replies_newest_before(conversation, thread, None, FLAT_MAX_REPLIES))
            .collect();
        let replies: Vec<Vec<Raw>> = stream::iter(reads).buffered(4).try_collect().await?;
        let mut raws = page.raws;
        raws.extend(replies.into_iter().flatten());
        raws.sort_by_key(|raw| raw.micros);
        raws.dedup_by_key(|raw| raw.micros);
        Ok(TimelinePage {
            messages: self.messages(channel, &raws).await,
            has_threads: self.is_threaded(conversation),
            has_more: page.has_more,
            next_before: page.next_before,
            notice: bounded.then(|| {
                format!(
                    "Flat view includes replies for at most {FLAT_MAX_THREADS} threads per page and \
                     the newest {FLAT_MAX_REPLIES} replies of each; open a thread to read all of it."
                )
            }),
            ..TimelinePage::default()
        })
    }

    /// Thread roots with replies, from a bounded scan of recent history, by latest activity.
    async fn thread_list(
        &self,
        channel: &ChannelId,
        conversation: &str,
        request: &TimelineRequest,
        cursor: Option<&Cursor>,
        limit: usize,
    ) -> Result<TimelinePage, ChatError> {
        let boundary = cursor.map(Cursor::activity).transpose()?;
        let mut roots: Vec<Raw> = Vec::new();
        let mut scanned = 0_usize;
        let mut complete = false;
        let mut page_cursor: Option<String> = None;
        for _ in 0..THREAD_SCAN_PAGES {
            let page = self
                .page(
                    Feed::History(conversation),
                    None,
                    None,
                    false,
                    SCAN_PAGE_LIMIT,
                    page_cursor.as_deref(),
                )
                .await?;
            scanned += page.raws.len();
            roots.extend(
                page.raws
                    .into_iter()
                    .filter(|raw| raw.is_real() && raw.is_thread_root()),
            );
            if !page.has_more {
                complete = true;
                break;
            }
            if page.next_cursor == page_cursor {
                return Err(shape("a paging cursor did not advance"));
            }
            page_cursor = page.next_cursor;
        }
        if !roots.is_empty() {
            self.mark_threaded(conversation);
        }
        let has_threads = !roots.is_empty();
        let key = |raw: &Raw| (raw.activity_micros(), raw.micros);
        roots.sort_by_key(key);
        roots.dedup_by_key(|raw| raw.micros);
        roots.retain(|raw| boundary.is_none_or(|boundary| key(raw) < boundary));
        let has_more = roots.len() > limit;
        if has_more {
            roots.drain(..roots.len() - limit);
        }
        let next_before = if has_more {
            let (activity, root) = key(&roots[0]);
            Some(self.cursor(channel, request, format!("{activity}:{root}"))?)
        } else {
            None
        };
        let mut threads = Vec::with_capacity(roots.len());
        for root in &roots {
            threads.push(self.summary(channel, root).await);
        }
        Ok(TimelinePage {
            threads,
            has_threads,
            has_more,
            next_before,
            notice: (!complete).then(|| {
                format!(
                    "Threads are found among the newest {scanned} messages of this conversation; \
                     older threads are not listed."
                )
            }),
            ..TimelinePage::default()
        })
    }

    /// The root of `thread` in `conversation`, which also proves the thread is there.
    ///
    /// Slack answers `thread_not_found` (HTTP 404 here) for a thread that is not in the
    /// conversation. A `ts` that names a reply is refused: a thread is identified by its root.
    pub(super) async fn thread_root(
        &self,
        conversation: &str,
        thread: &str,
    ) -> Result<Raw, ChatError> {
        let page = self
            .page(
                Feed::Replies(conversation, thread),
                None,
                None,
                false,
                1,
                None,
            )
            .await?;
        let root = page
            .raws
            .into_iter()
            .next()
            .ok_or_else(|| ChatError::Status {
                status: 404,
                body: "conversations.replies answered with no root message".to_owned(),
            })?;
        if root.ts != thread {
            return Err(refused(
                "that ts is not the root of a thread in the configured channel",
            ));
        }
        Ok(root)
    }

    async fn summary(&self, channel: &ChannelId, root: &Raw) -> ThreadSummary {
        let message = self
            .messages(channel, std::slice::from_ref(root))
            .await
            .into_iter()
            .next();
        ThreadSummary {
            id: root.ts.clone(),
            title: title(message.as_ref()),
            root: message,
            reply_count: Some(root.reply_count()),
            reply_count_exact: true,
            updated_at: rfc3339(root.activity_micros()),
            display_name: None,
            summary: None,
        }
    }

    /// [`crate::chat::ChatClient::post_message`], without the provider label.
    ///
    /// Slack has no reply pointer: its only reply is a thread reply. So `reply_to` on a whole
    /// conversation posts into that message's thread — starting one when it has none — and a
    /// channel narrowed to one thread always posts into that thread.
    pub(super) async fn post(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        let target = Self::channel_ref(channel)?;
        let conversation = target.conversation.as_str();
        let thread = match (&target.thread_ts, reply_to) {
            (Some(scope), _) => Some(scope.clone()),
            (None, Some(id)) => Some(self.reply_thread(conversation, id).await?),
            (None, None) => None,
        };
        self.post_raw(channel, conversation, content, thread.as_deref())
            .await
    }

    /// The thread a reply to `id` belongs in: its own, or the one it roots.
    async fn reply_thread(&self, conversation: &str, id: &MessageId) -> Result<String, ChatError> {
        let micros = cursor_micros(id)?;
        let page = self
            .page(
                Feed::History(conversation),
                Some(micros),
                Some(micros),
                true,
                1,
                None,
            )
            .await?;
        let target = page
            .raws
            .into_iter()
            .find(|raw| raw.micros == micros)
            .ok_or_else(|| {
                refused(
                    "that message is not in this conversation's own timeline; if it is a thread \
                     reply, reply in its thread instead",
                )
            })?;
        Ok(target.thread_ts().unwrap_or(target.ts))
    }

    async fn post_raw(
        &self,
        channel: &ChannelId,
        conversation: &str,
        content: &str,
        thread: Option<&str>,
    ) -> Result<Message, ChatError> {
        if content.trim().is_empty() {
            return Err(refused("message content is empty"));
        }
        let length = content.chars().count();
        if length > super::client::SLACK_MAX_TEXT_CHARS {
            return Err(ChatError::Refused(format!(
                "message content is {length} characters; Slack accepts at most {}",
                super::client::SLACK_MAX_TEXT_CHARS
            )));
        }
        let mut params = vec![
            ("channel", conversation.to_owned()),
            ("text", super::mrkdwn::escape_outgoing(content)),
        ];
        if let Some(thread) = thread {
            params.push(("thread_ts", thread.to_owned()));
        }
        let value = self.call("chat.postMessage", &params).await?;
        if value.get("channel").and_then(Value::as_str) != Some(conversation) {
            return Err(shape("chat.postMessage answered for a different channel"));
        }
        let mut message = value
            .get("message")
            .filter(|message| message.is_object())
            .cloned()
            .ok_or_else(|| shape("chat.postMessage answer has no \"message\" object"))?;
        if message.get("ts").is_none() {
            message["ts"] = value.get("ts").cloned().unwrap_or(Value::Null);
        }
        let raw = Raw::parse(message)?;
        if let Some(user) = raw.value.get("user").and_then(Value::as_str) {
            self.learn_self(user);
        }
        self.messages(channel, std::slice::from_ref(&raw))
            .await
            .into_iter()
            .next()
            .ok_or_else(|| shape("chat.postMessage answered with a message that has no author"))
    }

    /// [`crate::chat::ChatClient::post_in_thread`], without the provider label.
    ///
    /// The destination is the thread; `reply_to` adds nothing Slack can carry and is ignored.
    pub(super) async fn thread_post(
        &self,
        channel: &ChannelId,
        thread_id: &str,
        content: &str,
        _reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        let target = Self::channel_ref(channel)?;
        let thread = self.member_thread(&target, thread_id)?;
        self.thread_root(&target.conversation, &thread).await?;
        self.post_raw(channel, &target.conversation, content, Some(&thread))
            .await
    }

    /// [`crate::chat::ChatClient::fetch_thread_message`], without the provider label.
    pub(super) async fn thread_message(
        &self,
        channel: &ChannelId,
        thread_id: &str,
        message_id: &MessageId,
    ) -> Result<Message, ChatError> {
        let target = Self::channel_ref(channel)?;
        let thread = self.member_thread(&target, thread_id)?;
        let micros = cursor_micros(message_id)?;
        let page = self
            .page(
                Feed::Replies(&target.conversation, &thread),
                Some(micros),
                Some(micros),
                true,
                5,
                None,
            )
            .await?;
        let raw = page
            .raws
            .into_iter()
            .find(|raw| raw.micros == micros)
            .ok_or_else(|| ChatError::Status {
                status: 404,
                body: "conversations.replies has no such message in that thread".to_owned(),
            })?;
        if raw.ts != thread && raw.thread_ts().as_deref() != Some(thread.as_str()) {
            return Err(refused(
                "that message does not belong to the selected thread",
            ));
        }
        self.messages(channel, std::slice::from_ref(&raw))
            .await
            .into_iter()
            .next()
            .ok_or_else(|| shape("the message has no author"))
    }

    /// A caller's thread id, checked against a thread-scoped channel's own thread.
    fn member_thread(&self, target: &ChannelRef, thread_id: &str) -> Result<String, ChatError> {
        let thread = thread_ts(thread_id)?;
        if target
            .thread_ts
            .as_deref()
            .is_some_and(|scope| scope != thread)
        {
            return Err(refused(
                "that thread does not belong to the configured channel",
            ));
        }
        Ok(thread)
    }

    /// [`crate::chat::ChatClient::register_channel`], without the provider label.
    pub(super) async fn register(
        &self,
        source: &str,
        _label: &str,
    ) -> Result<RegisteredChannel, ChatError> {
        if !self.channel_registration {
            return Err(refused(
                "channel registration is disabled; ask the deployment operator to enable it for this backend",
            ));
        }
        let (conversation, thread) = ids::parse_source(source).ok_or_else(|| {
            refused(
                "not a Slack conversation: paste a conversation id such as C0123ABCD, a link copied \
                 from Slack (https://<workspace>.slack.com/archives/…), or an app.slack.com/client link",
            )
        })?;
        let info = self
            .call("conversations.info", &[("channel", conversation.clone())])
            .await?;
        if info
            .get("channel")
            .and_then(|channel| channel.get("id"))
            .and_then(Value::as_str)
            != Some(conversation.as_str())
        {
            return Err(shape(
                "conversations.info answered for a different conversation",
            ));
        }
        if let Some(thread) = &thread {
            self.thread_root(&conversation, thread).await?;
        }
        Ok(RegisteredChannel {
            id: ChannelId(
                ChannelRef {
                    conversation,
                    thread_ts: thread,
                }
                .channel_id(),
            ),
            // Nothing upstream was created, so vibe-talk never issues a compensating removal.
            created: false,
            writable: self.registered_writable,
        })
    }

    /// [`crate::chat::ChatClient::discover_channels`], without the provider label.
    ///
    /// Filtering by name can leave a Slack page with fewer matches than were asked for, so up to
    /// [`DISCOVERY_PAGES`] pages are read to fill one answer. The answer never exceeds the
    /// requested limit: when it fills part-way through a Slack page, the continuation records how
    /// far into that page it got and the next request resumes there. A page can still be short
    /// when the page budget runs out first; its continuation resumes correctly.
    pub(super) async fn discover(
        &self,
        request: &DirectoryRequest,
    ) -> Result<DirectoryPage, ChatError> {
        if !self.channel_discovery {
            return Err(refused(
                "channel discovery is disabled for this backend; ask the deployment operator to enable it",
            ));
        }
        let mut position = match request.cursor.as_deref() {
            Some(raw) => URL_SAFE_NO_PAD
                .decode(raw)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<DirectoryCursor>(&bytes).ok())
                .ok_or_else(|| refused("invalid channel-directory cursor; start the list again"))?,
            None => DirectoryCursor {
                c: String::new(),
                o: 0,
            },
        };
        let query = request
            .query
            .as_deref()
            .map(str::trim)
            .filter(|query| !query.is_empty())
            .map(str::to_lowercase);
        let limit = usize::from(request.limit.max(1));
        let mut entries = Vec::new();
        for _ in 0..DISCOVERY_PAGES {
            let mut params = vec![
                ("types", DISCOVERY_TYPES.to_owned()),
                ("limit", DISCOVERY_PAGE_LIMIT.to_string()),
                ("exclude_archived", "true".to_owned()),
            ];
            if !position.c.is_empty() {
                params.push(("cursor", position.c.clone()));
            }
            let value = self.call("users.conversations", &params).await?;
            let channels = value
                .get("channels")
                .and_then(Value::as_array)
                .ok_or_else(|| shape("users.conversations answer has no \"channels\" array"))?;
            let next = value
                .get("response_metadata")
                .and_then(|metadata| metadata.get("next_cursor"))
                .and_then(Value::as_str)
                .filter(|cursor| !cursor.is_empty())
                .map(str::to_owned);
            for (index, channel) in channels.iter().enumerate().skip(position.o) {
                let Some(entry) = self.directory_entry(channel).await else {
                    continue;
                };
                if query
                    .as_deref()
                    .is_some_and(|query| !entry.name.to_lowercase().contains(query))
                {
                    continue;
                }
                entries.push(entry);
                if entries.len() == limit {
                    let rest_of_page = index + 1 < channels.len();
                    let resume = if rest_of_page {
                        Some(DirectoryCursor {
                            c: position.c.clone(),
                            o: index + 1,
                        })
                    } else {
                        next.map(|c| DirectoryCursor { c, o: 0 })
                    };
                    return Ok(DirectoryPage {
                        entries,
                        next_cursor: resume.map(|cursor| encode_directory(&cursor)).transpose()?,
                        truncated: false,
                    });
                }
            }
            match next {
                Some(c) if c != position.c => position = DirectoryCursor { c, o: 0 },
                Some(_) => return Err(shape("a paging cursor did not advance")),
                None => {
                    return Ok(DirectoryPage {
                        entries,
                        next_cursor: None,
                        truncated: false,
                    })
                }
            }
        }
        Ok(DirectoryPage {
            entries,
            next_cursor: Some(encode_directory(&position)?),
            truncated: false,
        })
    }

    /// One `users.conversations` entry as a directory entry; `None` when it has no usable id.
    async fn directory_entry(&self, channel: &Value) -> Option<DirectoryEntry> {
        let id = channel
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| ids::is_conversation_id(id))?;
        let flag = |key: &str| channel.get(key).and_then(Value::as_bool).unwrap_or(false);
        let name = channel
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty());
        let display = if flag("is_im") {
            let person = match channel.get("user").and_then(Value::as_str) {
                Some(user) => self
                    .user(user)
                    .await
                    .map_or_else(|| user.to_owned(), |(name, _)| name),
                None => id.to_owned(),
            };
            format!("DM: {person}")
        } else if flag("is_mpim") {
            group_name(name.unwrap_or(id))
        } else {
            format!("#{}", name.unwrap_or(id))
        };
        Some(DirectoryEntry {
            source: id.to_owned(),
            name: directory_name(&display),
            registered_channel_id: None,
        })
    }
}

fn encode_directory(cursor: &DirectoryCursor) -> Result<String, ChatError> {
    serde_json::to_vec(cursor)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|_| shape("could not encode the channel-directory continuation"))
}
