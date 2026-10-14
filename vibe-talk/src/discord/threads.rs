//! Native thread discovery and the optional normalized bridge protocol.
//!
//! Native flat pages only follow a complete inventory. Archived collections have different sort
//! keys, so each is exhausted before sorting by message activity. The inventory is pinned by the
//! opaque cursor: an old page cannot silently use a different set of threads.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use futures_util::{stream, StreamExt as _, TryStreamExt as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    fresh_post_nonce, page_request, parse_message, post_request_with_nonce, HttpDiscordClient,
    PreparedRequest,
};
use crate::chat::ChatError;
use crate::model::{ChannelId, Message, MessageId};
use crate::threads::{
    MessageThread, ThreadApi, ThreadSummary, TimelineDelta, TimelinePage, TimelineRequest,
    TimelineView,
};

const MAX_THREADS: usize = 256;
const MAX_ARCHIVE_PAGES: usize = 100;
const SNAPSHOT_TTL: Duration = Duration::from_secs(300);
const MAX_SNAPSHOTS: usize = 16;
const TIMELINE_TIMEOUT: Duration = Duration::from_secs(90);
const THREAD_CONTAINER_NOTICE: &str =
    "This channel contains threads. Open a thread to post a reply.";
/// The longest cursor, either direction, a caller may hand back.
const MAX_CURSOR: usize = 8192;
/// The longest forward cursor the bridge contract lets a bridge issue. `#203 incremental-refresh`.
const MAX_BRIDGE_FORWARD_CURSOR: usize = 2048;
/// The most ids one bridge delta may list as deleted, or as threads removed. The contract caps
/// deletions at 500 and answers 410 past that; the margin keeps a bridge on the edge readable.
const MAX_DELTA_REMOVALS: usize = 1000;

#[derive(Debug, Default)]
pub(super) struct InventoryCache {
    next: AtomicU64,
    snapshots: Mutex<VecDeque<Arc<Inventory>>>,
}

#[derive(Debug)]
struct Inventory {
    serial: u64,
    created: Instant,
    channel: String,
    parent_kind: u64,
    threads: Vec<NativeThread>,
}

/// One page of a native message history, and what Discord itself answered for it.
struct NativePage {
    /// The page as [`HttpDiscordClient::native_messages`] answers it.
    messages: Vec<Message>,
    /// The newest snowflake among the rows Discord answered, a starter whose root is gone
    /// included.
    newest: Option<u64>,
}

/// Whether Discord answered that it has no such message: HTTP 404 with its code 10008, Unknown
/// Message. Any other 404 (an unknown channel, a bridge's own) proves nothing about the message.
fn unknown_message(error: &ChatError) -> bool {
    match error.cause() {
        ChatError::Status { status: 404, body } => {
            serde_json::from_str::<Value>(body)
                .ok()
                .and_then(|value| value.get("code").and_then(Value::as_u64))
                == Some(10_008)
        }
        _ => false,
    }
}

#[derive(Clone, Debug)]
struct NativeThread {
    id: String,
    parent: String,
    title: String,
    last_message: u64,
    /// The newest message the thread object vouches for: its `last_message_id`, or its own id when
    /// that is `null` (no message was ever sent there). `None` when the object leaves the field
    /// out (or gives something else), which says nothing about what was added, so a read after an
    /// instant reads it as a thread that moved. Fix round 3 of remote-ops-audit W14
    /// `#287 bulk-reads`. [`Self::last_message`] falls back to the thread's own id either way, for
    /// what it has always been used for.
    last_said: Option<u64>,
    replies: Option<u64>,
}

impl NativeThread {
    fn parse(value: &Value) -> Result<Self, ChatError> {
        if !matches!(value.get("type").and_then(Value::as_u64), Some(10..=12)) {
            return Err(ChatError::Refused(
                "the selected channel is not a thread".to_owned(),
            ));
        }
        let id = string_field(value, "id")?.to_owned();
        let numeric = native_id(&id)?;
        let parent = string_field(value, "parent_id")?.to_owned();
        native_id(&parent)?;
        let last_message = value
            .get("last_message_id")
            .and_then(Value::as_str)
            .map(native_id)
            .transpose()?
            .unwrap_or(numeric);
        let last_said = match value.get("last_message_id") {
            Some(Value::String(_)) => Some(last_message),
            Some(Value::Null) => Some(numeric),
            _ => None,
        };
        Ok(Self {
            id,
            parent,
            title: value
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or("Thread")
                .to_owned(),
            last_message,
            last_said,
            replies: value.get("message_count").and_then(Value::as_u64),
        })
    }

    fn membership(&self, root: bool) -> MessageThread {
        MessageThread {
            id: self.id.clone(),
            root_message_id: Some(MessageId(self.id.clone())),
            is_root: root,
            reply_count: self.replies,
            // Threads created before July 2022 have counters capped at 50. The wire count does
            // not promise exactness for every thread, so do not promote it to an exact count.
            reply_count_exact: false,
        }
    }

    fn summary(&self) -> Result<ThreadSummary, ChatError> {
        let ms = MessageId(self.last_message.to_string())
            .created_at_ms()
            .ok_or_else(|| shape("thread activity has an invalid message id"))?;
        let updated_at = jiff::Timestamp::from_millisecond(ms)
            .map_err(|_| shape("thread activity timestamp is out of range"))?
            .to_string();
        Ok(ThreadSummary {
            id: self.id.clone(),
            root: None,
            title: self.title.clone(),
            reply_count: self.replies,
            reply_count_exact: false,
            updated_at,
            display_name: None,
            summary: None,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeCursor {
    channel: String,
    view: TimelineView,
    thread_id: Option<String>,
    snapshot: Option<u64>,
    boundary: String,
}

impl NativeCursor {
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
}

/// A native forward cursor: the newest snowflake a newest read or delta of this view delivered.
/// `#203 incremental-refresh`. Snowflakes are unique and ordered, so `after` needs no overlap and
/// has no ties. A separate shape from [`NativeCursor`], so neither can be handed back as the other.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeForward {
    channel: String,
    view: TimelineView,
    thread_id: Option<String>,
    after: String,
    /// Whether the newest read found child threads: a Main delta reads no inventory, so it
    /// carries this forward rather than guessing.
    threads: bool,
}

impl NativeForward {
    fn encode(&self) -> Result<String, ChatError> {
        serde_json::to_vec(self)
            .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
            .map_err(|_| shape("could not encode the forward cursor"))
    }

    /// The cursor, if this backend issued it for exactly this read.
    ///
    /// Anything else is [`ChatError::CursorExpired`], never a refusal. vibe-talk's envelope has
    /// already checked the channel, view and thread, so a cursor that still does not fit here was
    /// issued by some other backend for this channel — a bridge, before the deployment moved the
    /// channel to the native API — and the caller's answer is a newest read, which issues one that
    /// fits. A 400 would leave the page retrying the same cursor and reporting each refresh as
    /// failed. `#203 incremental-refresh`.
    fn decode(
        raw: &str,
        channel: &ChannelId,
        request: &TimelineRequest,
    ) -> Result<Self, ChatError> {
        let gone = |detail: &str| ChatError::CursorExpired(detail.to_owned());
        let cursor: Self = URL_SAFE_NO_PAD
            .decode(raw)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or_else(|| gone("this backend did not issue that forward cursor"))?;
        if cursor.channel != channel.as_str()
            || cursor.view != request.view
            || cursor.thread_id != request.thread_id
        {
            return Err(gone(
                "the forward cursor belongs to a different channel or view",
            ));
        }
        native_id(&cursor.after).map_err(|_| gone("the forward cursor names no message"))?;
        Ok(cursor)
    }
}

/// The newest message of a page as a forward cursor, or `floor` when the page is empty: a read
/// forward from there returns everything posted after it.
fn forward_from(
    channel: &ChannelId,
    request: &TimelineRequest,
    page: &TimelinePage,
    floor: &str,
) -> Result<String, ChatError> {
    let newest = page
        .messages
        .iter()
        .filter_map(|message| message.id.numeric())
        .max()
        .map_or_else(|| floor.to_owned(), |id| id.to_string());
    NativeForward {
        channel: channel.0.clone(),
        view: request.view,
        thread_id: request.thread_id.clone(),
        after: newest,
        threads: page.has_threads,
    }
    .encode()
}

fn shape(detail: &str) -> ChatError {
    ChatError::Shape(detail.to_owned())
}
fn refused(detail: &str) -> ChatError {
    ChatError::Refused(detail.to_owned())
}

fn string_field<'a>(value: &'a Value, field: &str) -> Result<&'a str, ChatError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| shape(&format!("thread response is missing a string {field:?}")))
}

fn native_id(raw: &str) -> Result<u64, ChatError> {
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(refused("this backend requires a numeric native identifier"));
    }
    raw.parse()
        .map_err(|_| refused("native identifier is outside its supported range"))
}

fn validate_request(request: &TimelineRequest) -> Result<(), ChatError> {
    request.one_direction()?;
    if request.limit == 0
        || [&request.before, &request.after]
            .into_iter()
            .flatten()
            .any(|cursor| cursor.len() > MAX_CURSOR)
    {
        return Err(refused(
            "timeline page size must be positive and its cursor at most 8192 bytes",
        ));
    }
    let thread = request.thread_id.as_deref();
    if (request.view == TimelineView::Thread) != thread.is_some()
        || thread
            .is_some_and(|id| id.is_empty() || id.len() > 2048 || id.chars().any(char::is_control))
    {
        return Err(refused(
            "supply a thread id only when reading the thread view",
        ));
    }
    Ok(())
}

impl HttpDiscordClient {
    fn thread_request(
        &self,
        method: &'static str,
        path: &[&str],
        query: &[(&str, String)],
    ) -> Result<PreparedRequest, ChatError> {
        let mut url =
            reqwest::Url::parse(&self.api_base).map_err(|_| shape("invalid chat API base"))?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| shape("chat API base cannot contain path segments"))?;
            segments.pop_if_empty();
            for segment in path {
                if segment.is_empty()
                    || matches!(*segment, "." | "..")
                    || segment.len() > 2048
                    || segment.chars().any(char::is_control)
                {
                    return Err(refused("invalid channel, thread, or message identifier"));
                }
                segments.push(segment);
            }
        }
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(key, value)| (*key, value.as_str())));
        }
        Ok(PreparedRequest {
            method,
            url: url.into(),
            body: None,
        })
    }

    async fn read_thread_json(
        &self,
        path: &[&str],
        query: &[(&str, String)],
    ) -> Result<Value, ChatError> {
        self.send(self.thread_request("GET", path, query)?).await
    }

    pub(super) async fn timeline(
        &self,
        channel: &ChannelId,
        request: &TimelineRequest,
    ) -> Result<TimelinePage, ChatError> {
        validate_request(request)?;
        match self.thread_api {
            ThreadApi::Off => Err(refused("thread timelines are disabled on this backend")),
            ThreadApi::Bridge => {
                let mut query = vec![
                    ("view", request.view.as_str().to_owned()),
                    ("limit", request.limit.to_string()),
                ];
                if let Some(id) = &request.thread_id {
                    query.push(("thread_id", id.clone()));
                }
                if let Some(before) = &request.before {
                    query.push(("before", before.clone()));
                }
                if let Some(after) = &request.after {
                    query.push(("after", after.clone()));
                }
                let value = self
                    .send_with_timeout(
                        self.thread_request(
                            "GET",
                            &["channels", channel.as_str(), "timeline"],
                            &query,
                        )?,
                        TIMELINE_TIMEOUT,
                    )
                    .await
                    .map_err(|error| forward_refusal(error, request))?;
                parse_bridge_page(value, channel, request)
            }
            ThreadApi::Native => {
                tokio::time::timeout(TIMELINE_TIMEOUT, self.native_timeline(channel, request))
                    .await
                    .map_err(|_| {
                        refused(
                    "thread discovery exceeded its time budget; no partial timeline was returned",
                )
                    })?
            }
        }
    }

    /// What `channel`'s read marker covers, by the bridge's own word. `#292 reply-marks-read`.
    ///
    /// A registration narrowed to one conversation names that conversation as `thread` in every
    /// main page (see [`parse_bridge_page`]), and the provider's marker it moves is the whole
    /// space's, so marking in it would clear messages vibe-talk never received. A main page that
    /// names none is a whole registration. Only the bridge's timeline says: native threads and
    /// threads turned off serve a narrowed registration as an ordinary channel, so there the answer
    /// is [`crate::chat::UpstreamReadScope::Unknown`] and nothing is read.
    pub(super) async fn upstream_read_scope_of(
        &self,
        channel: &ChannelId,
    ) -> Result<crate::chat::UpstreamReadScope, ChatError> {
        if self.thread_api != ThreadApi::Bridge {
            return Ok(crate::chat::UpstreamReadScope::Unknown);
        }
        let request = TimelineRequest {
            view: TimelineView::Main,
            limit: 1,
            ..TimelineRequest::default()
        };
        let page = self.timeline(channel, &request).await?;
        Ok(if page.thread.is_some() {
            crate::chat::UpstreamReadScope::Wider
        } else {
            crate::chat::UpstreamReadScope::Channel
        })
    }

    /// A channel's most recent messages, read from the newest page of the bridge's Flat view.
    /// `#208 persistent-cache`, milestone `bridge-recent-flat`.
    ///
    /// A bridge that keeps a copy of each channel answers Flat from that copy in milliseconds,
    /// where `/messages` runs a provider command for seconds; when the copy could not be brought
    /// up to date it says so with `as_of`, which is handed on rather than dropped. Not `exhausted`:
    /// a bridge has no proof of a channel's start to give.
    pub(super) async fn flat_recent(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<crate::chat::RecentRead, ChatError> {
        let request = TimelineRequest {
            view: TimelineView::Flat,
            limit,
            ..TimelineRequest::default()
        };
        Ok(self.timeline(channel, &request).await?.into_recent(limit))
    }

    async fn archive_threads(
        &self,
        channel: &ChannelId,
        private: bool,
        joined: bool,
        found: &mut BTreeMap<String, NativeThread>,
    ) -> Result<(), ChatError> {
        let mut before: Option<String> = None;
        let mut seen = HashSet::new();
        for _ in 0..MAX_ARCHIVE_PAGES {
            let mut query = vec![("limit", "100".to_owned())];
            if let Some(before) = &before {
                query.push(("before", before.clone()));
            }
            let path = if joined {
                vec![
                    "channels",
                    channel.as_str(),
                    "users",
                    "@me",
                    "threads",
                    "archived",
                    "private",
                ]
            } else {
                vec![
                    "channels",
                    channel.as_str(),
                    "threads",
                    "archived",
                    if private { "private" } else { "public" },
                ]
            };
            let response = self.read_thread_json(&path, &query).await?;
            let threads = response
                .get("threads")
                .and_then(Value::as_array)
                .ok_or_else(|| shape("archived thread response has no thread list"))?;
            for value in threads {
                let thread = NativeThread::parse(value)?;
                if thread.parent != channel.as_str() {
                    return Err(shape(
                        "archived thread response included a different parent channel",
                    ));
                }
                found.entry(thread.id.clone()).or_insert(thread);
                if found.len() > MAX_THREADS {
                    return Err(refused("channel exceeds the 256-thread discovery budget; no partial timeline was returned"));
                }
            }
            let has_more = response
                .get("has_more")
                .and_then(Value::as_bool)
                .ok_or_else(|| shape("archived thread response has no has_more flag"))?;
            if !has_more {
                return Ok(());
            }
            let last = threads
                .last()
                .ok_or_else(|| shape("archived thread cursor did not advance"))?;
            let next = if joined {
                string_field(last, "id")?
            } else {
                last.get("thread_metadata")
                    .and_then(|metadata| metadata.get("archive_timestamp"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| shape("archived thread has no archive timestamp"))?
            }
            .to_owned();
            if !seen.insert(next.clone()) {
                return Err(shape("archived thread cursor did not advance"));
            }
            before = Some(next);
        }
        Err(refused("thread archive exceeds the 100-page discovery budget; no partial timeline was returned"))
    }

    async fn inventory(
        &self,
        channel: &ChannelId,
        cursor: Option<&NativeCursor>,
    ) -> Result<Arc<Inventory>, ChatError> {
        if let Some(serial) = cursor.and_then(|cursor| cursor.snapshot) {
            let mut snapshots = self
                .thread_inventory
                .snapshots
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            snapshots.retain(|item| item.created.elapsed() < SNAPSHOT_TTL);
            return snapshots
                .iter()
                .find(|item| item.serial == serial && item.channel == channel.as_str())
                .cloned()
                .ok_or_else(|| refused("this timeline cursor has expired; refresh the channel"));
        }
        if cursor.is_some() {
            return Err(refused("timeline cursor has no discovery snapshot"));
        }
        native_id(channel.as_str())?;
        let parent = self
            .read_thread_json(&["channels", channel.as_str()], &[])
            .await?;
        if string_field(&parent, "id")? != channel.as_str() {
            return Err(shape("channel lookup returned a different id"));
        }
        let parent_kind = parent
            .get("type")
            .and_then(Value::as_u64)
            .ok_or_else(|| shape("channel response has no type"))?;
        let mut found = BTreeMap::new();
        if matches!(parent_kind, 0 | 5 | 15 | 16) {
            let guild = string_field(&parent, "guild_id")?;
            native_id(guild)?;
            let active = self
                .read_thread_json(&["guilds", guild, "threads", "active"], &[])
                .await?;
            for value in active
                .get("threads")
                .and_then(Value::as_array)
                .ok_or_else(|| shape("active thread response has no thread list"))?
            {
                if value.get("parent_id").and_then(Value::as_str) == Some(channel.as_str()) {
                    let thread = NativeThread::parse(value)?;
                    found.insert(thread.id.clone(), thread);
                }
            }
            if found.len() > MAX_THREADS {
                return Err(refused("channel exceeds the 256-thread discovery budget; no partial timeline was returned"));
            }
            self.archive_threads(channel, false, false, &mut found)
                .await?;
            if parent_kind == 0 {
                if let Err(error) = self.archive_threads(channel, true, false, &mut found).await {
                    if matches!(error.cause(), ChatError::Status { status: 403, .. }) {
                        self.archive_threads(channel, true, true, &mut found)
                            .await?;
                    } else {
                        return Err(error);
                    }
                }
            }
        }
        let mut threads: Vec<_> = found.into_values().collect();
        threads.sort_by_key(|thread| (thread.last_message, thread.id.parse::<u64>().unwrap_or(0)));
        let inventory = Arc::new(Inventory {
            serial: self.thread_inventory.next.fetch_add(1, Ordering::Relaxed) + 1,
            created: Instant::now(),
            channel: channel.0.clone(),
            parent_kind,
            threads,
        });
        let mut snapshots = self
            .thread_inventory
            .snapshots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshots.retain(|item| item.created.elapsed() < SNAPSHOT_TTL);
        while snapshots.len() >= MAX_SNAPSHOTS {
            snapshots.pop_front();
        }
        snapshots.push_back(Arc::clone(&inventory));
        Ok(inventory)
    }

    async fn verified_thread(
        &self,
        channel: &ChannelId,
        id: &str,
    ) -> Result<NativeThread, ChatError> {
        native_id(channel.as_str())?;
        native_id(id)?;
        let value = self.read_thread_json(&["channels", id], &[]).await?;
        let thread = NativeThread::parse(&value)?;
        if thread.id != id || thread.parent != channel.as_str() {
            return Err(refused(
                "that thread does not belong to the configured channel",
            ));
        }
        Ok(thread)
    }

    async fn thread_root(
        &self,
        channel: &ChannelId,
        thread: &NativeThread,
        parent_kind: u64,
    ) -> Result<Option<Message>, ChatError> {
        let physical = if matches!(parent_kind, 15 | 16) {
            thread.id.as_str()
        } else {
            channel.as_str()
        };
        match self
            .read_thread_json(&["channels", physical, "messages", &thread.id], &[])
            .await
        {
            Ok(value) => {
                let mut message = parse_message(&value)?;
                if message.channel_id.as_str() != physical || message.id.as_str() != thread.id {
                    return Err(shape("thread root lookup returned a different message"));
                }
                message.channel_id = channel.clone();
                message.thread = Some(thread.membership(true));
                Ok(Some(message))
            }
            Err(error) if matches!(error.cause(), ChatError::Status { status: 404, .. }) => {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    async fn native_messages(
        &self,
        channel: &ChannelId,
        thread: Option<&NativeThread>,
        before: Option<&str>,
        after: Option<&str>,
        limit: u16,
    ) -> Result<Vec<Message>, ChatError> {
        Ok(self
            .native_page(channel, thread, before, after, limit)
            .await?
            .messages)
    }

    /// [`Self::native_messages`], with the newest snowflake Discord itself answered, read before a
    /// starter whose root is gone was left out.
    async fn native_page(
        &self,
        channel: &ChannelId,
        thread: Option<&NativeThread>,
        before: Option<&str>,
        after: Option<&str>,
        limit: u16,
    ) -> Result<NativePage, ChatError> {
        let physical = thread.map_or(channel.as_str(), |thread| thread.id.as_str());
        let cursor = before.map(|value| MessageId(value.to_owned()));
        let forward = after.map(|value| MessageId(value.to_owned()));
        let value = self
            .send(page_request(
                &self.api_base,
                &ChannelId(physical.to_owned()),
                limit,
                cursor.as_ref(),
                forward.as_ref(),
            )?)
            .await?;
        let raw = value
            .as_array()
            .ok_or_else(|| shape("message history is not an array"))?;
        let newest = raw
            .iter()
            .filter_map(|value| value.get("id")?.as_str()?.parse::<u64>().ok())
            .max();
        let mut messages = Vec::with_capacity(raw.len());
        for value in raw {
            if value.get("channel_id").and_then(Value::as_str) != Some(physical) {
                return Err(shape("message history returned a different channel"));
            }
            let mut message = if value.get("type").and_then(Value::as_u64) == Some(21) {
                // A starter is a reference, never an empty message to display. When its inline
                // reference was omitted, read the exact root from the verified parent. A deleted
                // root is omitted rather than replaced by the empty system stub.
                let context = match thread {
                    Some(thread) => thread.clone(),
                    None => NativeThread::parse(
                        &self.read_thread_json(&["channels", physical], &[]).await?,
                    )?,
                };
                let root = match value
                    .get("referenced_message")
                    .filter(|root| root.is_object())
                {
                    Some(root) => parse_message(root)?,
                    None => match self
                        .thread_root(&ChannelId(context.parent.clone()), &context, 0)
                        .await?
                    {
                        Some(root) => root,
                        None => continue,
                    },
                };
                if root.id.as_str() != context.id || root.channel_id.as_str() != context.parent {
                    return Err(shape("thread starter referred to a different root"));
                }
                root
            } else {
                parse_message(value)?
            };
            native_id(message.id.as_str())?;
            message.channel_id = channel.clone();
            if let Some(thread) = thread {
                message.thread = Some(thread.membership(message.id.as_str() == thread.id));
            } else if value.get("type").and_then(Value::as_u64) == Some(21) {
                // A thread configured as its own top-level channel has no nested child thread.
                message.thread = None;
            }
            messages.push(message);
        }
        Ok(NativePage { messages, newest })
    }

    pub(super) async fn native_probe(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<Vec<Message>, ChatError> {
        native_id(channel.as_str())?;
        let metadata = self
            .read_thread_json(&["channels", channel.as_str()], &[])
            .await?;
        if string_field(&metadata, "id")? != channel.as_str() {
            return Err(shape("channel probe returned a different channel"));
        }
        if matches!(metadata.get("type").and_then(Value::as_u64), Some(15 | 16)) {
            let request = TimelineRequest {
                limit,
                ..TimelineRequest::default()
            };
            Ok(self.timeline(channel, &request).await?.messages)
        } else {
            crate::chat::ChatClient::fetch_recent(self, channel, limit).await
        }
    }

    pub(super) fn validate_main_post(&self, channel: &ChannelId) -> Result<(), ChatError> {
        if self.thread_api != ThreadApi::Native {
            return Ok(());
        }
        // Startup probing and timeline reads remember the native container kind. A forum or media
        // container has no message endpoint; posting requires an explicitly selected child thread.
        let snapshots = self
            .thread_inventory
            .snapshots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if snapshots
            .iter()
            .rev()
            .find(|item| item.channel == channel.as_str())
            .is_some_and(|item| matches!(item.parent_kind, 15 | 16))
        {
            return Err(refused(THREAD_CONTAINER_NOTICE));
        }
        Ok(())
    }

    async fn native_timeline(
        &self,
        channel: &ChannelId,
        request: &TimelineRequest,
    ) -> Result<TimelinePage, ChatError> {
        let limit = usize::from(request.limit.clamp(1, 99));
        if let Some(raw) = &request.after {
            let forward = NativeForward::decode(raw, channel, request)?;
            return self.native_forward(channel, request, &forward, limit).await;
        }
        let mut page = self.native_backward(channel, request, limit).await?;
        // `#203 incremental-refresh`. Only where one request answers a forward read: a thread, and
        // the Main of a text or announcement channel. The thread list, All, and a forum's Main
        // need the whole inventory again, so vibe-talk's generic reader catches those up.
        if request.before.is_none() {
            // A forum or media container's Main says so in its notice: it has no message
            // endpoint to read forward.
            let floor = match request.view {
                TimelineView::Thread => request.thread_id.clone(),
                TimelineView::Main if page.notice.is_none() => Some(channel.0.clone()),
                _ => None,
            };
            if let Some(floor) = floor {
                page.next_after = Some(forward_from(channel, request, &page, &floor)?);
            }
        }
        Ok(page)
    }

    /// What was posted in a thread, or in a text channel's Main, after the cursor's snowflake,
    /// oldest first, as a delta. `#203 incremental-refresh`. Filtered by creation, so it carries no
    /// edit or deletion, and a reply in a thread does not bring its root's new count into Main:
    /// `complete: false`, and the page keeps its full poll for this channel.
    async fn native_forward(
        &self,
        channel: &ChannelId,
        request: &TimelineRequest,
        forward: &NativeForward,
        limit: usize,
    ) -> Result<TimelinePage, ChatError> {
        let ask = u16::try_from(limit + 1).unwrap_or(crate::discord::http::DISCORD_MAX_LIMIT);
        let (mut messages, thread, has_threads) = match request.view {
            TimelineView::Thread => {
                let id = request.thread_id.as_deref().expect("validated thread id");
                let thread = self.verified_thread(channel, id).await?;
                let messages = self
                    .native_messages(channel, Some(&thread), None, Some(&forward.after), ask)
                    .await?;
                let parent = self
                    .read_thread_json(&["channels", channel.as_str()], &[])
                    .await?;
                let parent_kind = parent
                    .get("type")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| shape("channel response has no type"))?;
                let mut summary = thread.summary()?;
                summary.root = self.thread_root(channel, &thread, parent_kind).await?;
                (messages, Some(summary), true)
            }
            TimelineView::Main => {
                native_id(channel.as_str())?;
                let mut messages = self
                    .native_messages(channel, None, None, Some(&forward.after), ask)
                    .await?;
                // No inventory is read for a delta. One read in the last few minutes still names
                // which of these messages head threads; without one they are drawn as posted.
                let inventory = self
                    .thread_inventory
                    .snapshots
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .rev()
                    .find(|item| {
                        item.channel == channel.as_str() && item.created.elapsed() < SNAPSHOT_TTL
                    })
                    .cloned();
                if let Some(inventory) = &inventory {
                    for message in &mut messages {
                        if let Some(thread) = inventory
                            .threads
                            .iter()
                            .find(|thread| thread.id == message.id.as_str())
                        {
                            message.thread = Some(thread.membership(true));
                        }
                    }
                }
                let found = inventory.is_some_and(|inventory| !inventory.threads.is_empty());
                (messages, None, forward.threads || found)
            }
            // Never issued for this view, so the cursor came from another backend: see
            // `NativeForward::decode`.
            _ => {
                return Err(ChatError::CursorExpired(
                    "this view is not read forward natively".to_owned(),
                ))
            }
        };
        let after = forward.after.parse::<u64>().unwrap_or(0);
        messages.retain(|message| message.id.numeric().unwrap_or(0) > after);
        messages.sort_by_key(|message| message.id.numeric().unwrap_or(0));
        messages.dedup_by(|later, earlier| later.id == earlier.id);
        let more = messages.len() > limit;
        messages.truncate(limit);
        let mut page = TimelinePage {
            messages,
            thread,
            has_threads,
            delta: Some(TimelineDelta {
                more,
                complete: false,
                deleted: Vec::new(),
                removed_threads: Vec::new(),
            }),
            ..TimelinePage::default()
        };
        page.next_after = Some(forward_from(channel, request, &page, &forward.after)?);
        Ok(page)
    }

    /// A newest or older page, through the backward cursor.
    async fn native_backward(
        &self,
        channel: &ChannelId,
        request: &TimelineRequest,
        limit: usize,
    ) -> Result<TimelinePage, ChatError> {
        let cursor = request
            .before
            .as_deref()
            .map(|before| NativeCursor::decode(before, channel, request))
            .transpose()?;
        if request.view == TimelineView::Thread {
            let id = request.thread_id.as_deref().expect("validated thread id");
            let thread = self.verified_thread(channel, id).await?;
            let before = cursor.as_ref().map(|cursor| cursor.boundary.as_str());
            if let Some(before) = before {
                native_id(before)?;
            }
            let mut messages = self
                .native_messages(channel, Some(&thread), before, None, (limit + 1) as u16)
                .await?;
            let parent = self
                .read_thread_json(&["channels", channel.as_str()], &[])
                .await?;
            let parent_kind = parent
                .get("type")
                .and_then(Value::as_u64)
                .ok_or_else(|| shape("channel response has no type"))?;
            let mut summary = thread.summary()?;
            summary.root = self.thread_root(channel, &thread, parent_kind).await?;
            if let Some(root) = &summary.root {
                if before.is_none_or(|before| {
                    root.id.numeric().unwrap_or(0) < before.parse::<u64>().unwrap_or(0)
                }) {
                    messages.push(root.clone());
                }
            }
            let mut page = message_page(messages, channel, request, None, limit)?;
            page.has_threads = true;
            page.thread = Some(summary);
            return Ok(page);
        }
        let inventory = self.inventory(channel, cursor.as_ref()).await?;
        let has_threads = !inventory.threads.is_empty();
        if request.view == TimelineView::Threads {
            let end = cursor
                .as_ref()
                .map(|cursor| cursor.boundary.parse::<usize>())
                .transpose()
                .map_err(|_| refused("invalid thread-list cursor"))?
                .unwrap_or(inventory.threads.len());
            if end > inventory.threads.len() {
                return Err(refused("invalid thread-list cursor"));
            }
            let start = end.saturating_sub(limit);
            let parent_kind = inventory.parent_kind;
            let threads = stream::iter(inventory.threads[start..end].iter().cloned().map(
                |thread| async move {
                    let mut summary = thread.summary()?;
                    summary.root = self.thread_root(channel, &thread, parent_kind).await?;
                    Ok::<_, ChatError>(summary)
                },
            ))
            .buffered(4)
            .try_collect()
            .await?;
            let next_before = if start > 0 {
                Some(
                    NativeCursor {
                        channel: channel.0.clone(),
                        view: request.view,
                        thread_id: None,
                        snapshot: Some(inventory.serial),
                        boundary: start.to_string(),
                    }
                    .encode()?,
                )
            } else {
                None
            };
            return Ok(TimelinePage {
                threads,
                has_threads,
                has_more: next_before.is_some(),
                next_before,
                notice: matches!(inventory.parent_kind, 15 | 16)
                    .then(|| THREAD_CONTAINER_NOTICE.to_owned()),
                ..TimelinePage::default()
            });
        }
        let before = cursor.as_ref().map(|cursor| cursor.boundary.as_str());
        if let Some(before) = before {
            native_id(before)?;
        }
        if request.view == TimelineView::Main && matches!(inventory.parent_kind, 15 | 16) {
            // Thread-only containers still have a useful Main view: their original posts. Sort
            // roots by creation, while the separate Threads view sorts by latest reply activity.
            let boundary = before.map(native_id).transpose()?;
            let mut candidates: Vec<_> = inventory
                .threads
                .iter()
                .filter(|thread| {
                    boundary.is_none_or(|boundary| thread.id.parse::<u64>().unwrap_or(0) < boundary)
                })
                .cloned()
                .collect();
            candidates
                .sort_by_key(|thread| std::cmp::Reverse(thread.id.parse::<u64>().unwrap_or(0)));
            let parent_kind = inventory.parent_kind;
            let messages = stream::iter(candidates.into_iter().map(|thread| async move {
                self.verified_thread(channel, &thread.id).await?;
                self.thread_root(channel, &thread, parent_kind).await
            }))
            .buffered(4)
            .try_filter_map(|root| std::future::ready(Ok(root)))
            .take(limit + 1)
            .try_collect()
            .await?;
            let mut page = message_page(messages, channel, request, Some(inventory.serial), limit)?;
            page.has_threads = has_threads;
            page.notice = Some(THREAD_CONTAINER_NOTICE.to_owned());
            return Ok(page);
        }
        let mut sources = Vec::new();
        if !matches!(inventory.parent_kind, 15 | 16) {
            sources.push(None);
        }
        if request.view == TimelineView::Flat {
            sources.extend(inventory.threads.iter().cloned().map(Some));
        }
        let groups: Vec<Vec<Message>> =
            stream::iter(sources.into_iter().map(|thread| async move {
                // Cached discovery is not permission to read an arbitrary child after its parent has
                // changed. Verify membership on every selected stream before fetching its messages.
                if let Some(thread) = &thread {
                    self.verified_thread(channel, &thread.id).await?;
                }
                self.native_messages(channel, thread.as_ref(), before, None, (limit + 1) as u16)
                    .await
            }))
            .buffer_unordered(4)
            .try_collect()
            .await?;
        let mut messages: Vec<Message> = groups.into_iter().flatten().collect();
        for message in &mut messages {
            if let Some(thread) = inventory
                .threads
                .iter()
                .find(|thread| thread.id == message.id.as_str())
            {
                message.thread = Some(thread.membership(true));
            }
        }
        let mut page = message_page(messages, channel, request, Some(inventory.serial), limit)?;
        page.has_threads = has_threads;
        if matches!(inventory.parent_kind, 15 | 16) {
            page.notice = Some(THREAD_CONTAINER_NOTICE.to_owned());
        }
        Ok(page)
    }

    pub(super) async fn thread_message(
        &self,
        channel: &ChannelId,
        id: &str,
        message_id: &MessageId,
    ) -> Result<Message, ChatError> {
        match self.thread_api {
            ThreadApi::Off => Err(refused("thread message lookup is disabled on this backend")),
            ThreadApi::Bridge => {
                let value = self
                    .send_with_timeout(
                        self.thread_request(
                            "GET",
                            &[
                                "channels",
                                channel.as_str(),
                                "threads",
                                id,
                                "messages",
                                message_id.as_str(),
                            ],
                            &[],
                        )?,
                        TIMELINE_TIMEOUT,
                    )
                    .await?;
                let message = parse_message(&value)?;
                validate_bridge_message(&message, channel, Some(id))?;
                if message.id != *message_id {
                    return Err(shape("thread lookup returned a different message"));
                }
                Ok(message)
            }
            ThreadApi::Native => {
                let thread = self.verified_thread(channel, id).await?;
                self.verified_thread_message(channel, &thread, message_id)
                    .await
            }
        }
    }

    /// One message of a thread already verified to belong to `channel`, from Discord itself.
    async fn verified_thread_message(
        &self,
        channel: &ChannelId,
        thread: &NativeThread,
        message_id: &MessageId,
    ) -> Result<Message, ChatError> {
        let id = thread.id.as_str();
        native_id(message_id.as_str())?;
        let value = match self
            .read_thread_json(&["channels", id, "messages", message_id.as_str()], &[])
            .await
        {
            Ok(value) => value,
            Err(error)
                if message_id.as_str() == id
                    && matches!(error.cause(), ChatError::Status { status: 404, .. }) =>
            {
                // A public thread's root belongs to its parent channel. It remains a
                // legitimate thread context message even when no starter mirror exists.
                self.read_thread_json(&["channels", channel.as_str(), "messages", id], &[])
                    .await?
            }
            Err(error) => return Err(error),
        };
        let physical = string_field(&value, "channel_id")?;
        if physical != id && !(message_id.as_str() == id && physical == channel.as_str()) {
            return Err(shape("thread lookup returned a different channel"));
        }
        let value = if value.get("type").and_then(Value::as_u64) == Some(21) {
            value
                .get("referenced_message")
                .filter(|value| value.is_object())
                .unwrap_or(&value)
        } else {
            &value
        };
        let mut message = parse_message(value)?;
        if message.id != *message_id {
            return Err(shape("thread lookup returned a different message"));
        }
        message.channel_id = channel.clone();
        message.thread = Some(thread.membership(message.id.as_str() == id));
        Ok(message)
    }

    /// [`crate::chat::ChatClient::fetch_messages`] against Discord itself. remote-ops-audit W11
    /// `#287 bulk-reads`.
    ///
    /// Snowflakes order as messages were sent, so ids near each other are one page: `after` the
    /// snowflake before the oldest asked, up to 100. That page holds every message from the oldest
    /// asked up to the newest row it returned, so an id in that stretch is on it or proved absent.
    /// A page short of 100 proves nothing past its newest row: Discord answers an empty history
    /// to a bot without Read Message History, where the same id asked alone is refused. Any other
    /// id is one `GET .../messages/{id}`, and absent only when Discord answers Unknown Message
    /// (code 10008), so an id past the page costs a request of its own only when it was not on it.
    /// A thread is checked against the channel once, for every id; its root, which may live in
    /// the parent channel, is never proved absent by the page and is looked up on its own when the
    /// page did not have it. One id is one request (two in a thread), as a lookup of one always was.
    pub(super) async fn native_lookup(
        &self,
        channel: &ChannelId,
        thread: Option<&str>,
        wanted: &[MessageId],
    ) -> Result<Vec<crate::chat::Lookup>, ChatError> {
        use crate::chat::Lookup;

        native_id(channel.as_str())?;
        let verified = match thread {
            Some(id) => Some(self.verified_thread(channel, id).await?),
            None => None,
        };
        let root = verified.as_ref().map(|thread| thread.id.as_str());
        let mut numbers: Vec<u64> = wanted
            .iter()
            .filter_map(|id| native_id(id.as_str()).ok())
            .collect();
        numbers.sort_unstable();
        numbers.dedup();
        let mut answers: BTreeMap<u64, Lookup> = BTreeMap::new();
        if let [first, _, ..] = numbers.as_slice() {
            let after = first.saturating_sub(1).to_string();
            let limit = super::DISCORD_MAX_LIMIT;
            // `reached` is the newest row Discord answered, and never more, however short the
            // page: Discord answers an empty history to a bot without Read Message History, so a
            // short page proves nothing about what lies past its newest row.
            let (messages, reached) = match &verified {
                Some(thread) => {
                    let page = self
                        .native_page(channel, Some(thread), None, Some(&after), limit)
                        .await?;
                    (page.messages, page.newest.unwrap_or(0))
                }
                None => {
                    let value = self
                        .send(page_request(
                            &self.api_base,
                            channel,
                            limit,
                            None,
                            Some(&MessageId(after)),
                        )?)
                        .await?;
                    let messages = super::parse_message_list(&value)?;
                    if messages
                        .iter()
                        .any(|message| message.channel_id != *channel)
                    {
                        return Err(shape("message history returned a different channel"));
                    }
                    let reached = messages
                        .iter()
                        .filter_map(|message| message.id.as_str().parse::<u64>().ok())
                        .max()
                        .unwrap_or(0);
                    (messages, reached)
                }
            };
            for &number in numbers.iter().filter(|number| **number <= reached) {
                let id = number.to_string();
                match messages.iter().find(|message| message.id.as_str() == id) {
                    Some(message) => {
                        answers.insert(number, Lookup::Found(Box::new(message.clone())));
                    }
                    None if root == Some(id.as_str()) => {}
                    None => {
                        answers.insert(number, Lookup::Absent);
                    }
                }
            }
        }
        for number in numbers {
            if answers.contains_key(&number) {
                continue;
            }
            let id = MessageId(number.to_string());
            let looked = match &verified {
                Some(thread) => self.verified_thread_message(channel, thread, &id).await,
                None => self.channel_message(channel, &id).await,
            };
            let answer = match looked {
                Ok(message) => Lookup::Found(Box::new(message)),
                Err(error) if unknown_message(&error) => Lookup::Absent,
                Err(error) => return Err(error),
            };
            answers.insert(number, answer);
        }
        Ok(wanted
            .iter()
            .map(|id| {
                native_id(id.as_str())
                    .ok()
                    .and_then(|number| answers.get(&number).cloned())
                    .unwrap_or(Lookup::Unseen)
            })
            .collect())
    }

    /// [`crate::chat::ChatClient::fetch_threads`] against Discord itself (`thread_api = "native"`).
    /// remote-ops-audit W14 `#287 bulk-reads`.
    ///
    /// Discord has no read of several threads, so each thread is still read on its own,
    /// [`crate::chat::THREADS_IN_FLIGHT`] at a time, and checked against the channel
    /// (`GET /channels/{thread}`), as every thread read is. What a read of several saves is what
    /// the thread view repeats per thread, and what a `newer_than` ask makes unnecessary:
    ///
    /// * The parent channel's kind, which says where a thread's root lives, is read once for all
    ///   of them, and only when some root is to be read.
    /// * An ask without `newer_than_ms` is answered exactly as the thread view's newest page is:
    ///   the newest `limit` messages, the root among them where it falls, the summary with its
    ///   root, and the forward cursor. Three requests a thread rather than the view's four.
    /// * An ask with it reads nothing more when the thread's `last_message_id` is not newer than
    ///   the instant (a message added since would have moved it) or is `null`, and otherwise (a
    ///   thread object that leaves the field out included) the messages after the instant
    ///   (`after=` its snowflake), at most `limit`, the oldest first, with `delta.more` when there
    ///   were more. Its root is read only when the root itself is newer than the instant: an older
    ///   root is not news, and the summary then leaves it out. So a quiet thread costs one request
    ///   and a moved one two.
    ///
    /// A thread that cannot be read, or does not belong to the channel, is an error in its own
    /// place.
    pub(super) async fn native_threads(
        &self,
        channel: &ChannelId,
        asks: &[crate::chat::ThreadAsk],
    ) -> crate::chat::ThreadAnswers {
        let parent_kind = tokio::sync::OnceCell::new();
        let reads: Vec<_> = asks
            .iter()
            .map(|ask| self.native_thread_answer(channel, ask, &parent_kind))
            .collect();
        let answers: Vec<_> = stream::iter(reads)
            .buffered(crate::chat::THREADS_IN_FLIGHT)
            .collect()
            .await;
        asks.iter()
            .zip(answers)
            .map(|(ask, answer)| {
                (
                    ask.id.clone(),
                    answer.map_err(|error| error.with_provider(&self.provider_name)),
                )
            })
            .collect()
    }

    /// One thread of [`Self::native_threads`]; `parent_kind` is the parent channel's kind, read at
    /// most once for every thread of the call.
    async fn native_thread_answer(
        &self,
        channel: &ChannelId,
        ask: &crate::chat::ThreadAsk,
        parent_kind: &tokio::sync::OnceCell<u64>,
    ) -> Result<TimelinePage, ChatError> {
        let request = ask.request();
        validate_request(&request)?;
        let limit = usize::from(request.limit.clamp(1, 99));
        let thread = self.verified_thread(channel, &ask.id).await?;
        let kind = || async {
            parent_kind
                .get_or_try_init(|| async {
                    let parent = self
                        .read_thread_json(&["channels", channel.as_str()], &[])
                        .await?;
                    parent
                        .get("type")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| shape("channel response has no type"))
                })
                .await
                .copied()
        };
        let mut summary = thread.summary()?;
        let Some(newer_than_ms) = ask.newer_than_ms else {
            // The thread view's newest page, as `native_backward` and `native_timeline` make it.
            let mut messages = self
                .native_messages(channel, Some(&thread), None, None, (limit + 1) as u16)
                .await?;
            summary.root = self.thread_root(channel, &thread, kind().await?).await?;
            if let Some(root) = &summary.root {
                messages.push(root.clone());
            }
            let mut page = message_page(messages, channel, &request, None, limit)?;
            page.has_threads = true;
            page.thread = Some(summary);
            page.next_after = Some(forward_from(channel, &request, &page, &thread.id)?);
            return Ok(page);
        };
        // The newest snowflake of `newer_than_ms`: everything after it was created later.
        let last_at =
            MessageId::at_time_ms(newer_than_ms).numeric().unwrap_or(0) | ((1_u64 << 22) - 1);
        // A thread object that does not say its last message is read as one that moved: fix round
        // 3 of remote-ops-audit W14 `#287 bulk-reads`.
        let mut messages = if thread.last_said.is_none_or(|last| last > last_at) {
            let ask = u16::try_from(limit + 1).unwrap_or(super::DISCORD_MAX_LIMIT);
            self.native_messages(
                channel,
                Some(&thread),
                None,
                Some(&last_at.to_string()),
                ask,
            )
            .await?
        } else {
            Vec::new()
        };
        if native_id(&thread.id)? > last_at {
            summary.root = self.thread_root(channel, &thread, kind().await?).await?;
            if let Some(root) = &summary.root {
                messages.push(root.clone());
            }
        }
        messages.retain(|message| message.id.numeric().unwrap_or(0) > last_at);
        messages.sort_by_key(|message| message.id.numeric().unwrap_or(0));
        messages.dedup_by(|later, earlier| later.id == earlier.id);
        let more = messages.len() > limit;
        messages.truncate(limit);
        Ok(TimelinePage {
            messages,
            thread: Some(summary),
            has_threads: true,
            delta: Some(TimelineDelta {
                more,
                complete: false,
                deleted: Vec::new(),
                removed_threads: Vec::new(),
            }),
            ..TimelinePage::default()
        })
    }

    /// [`crate::chat::ChatClient::fetch_messages`] against a bridge configured with `id_reads`.
    /// remote-ops-audit W11 `#287 bulk-reads`.
    ///
    /// Every distinct id in one request ([`super::ids_request`]), or one per
    /// [`super::BRIDGE_LOOKUP_IDS`] of them, with the time a thread read is allowed: a bridge may
    /// have to ask its service for some of them. What it answers is read strictly
    /// ([`super::parse_id_lookup`]); an id it does not mention is [`crate::chat::Lookup::Unseen`].
    ///
    /// `None` when the bridge showed it does not serve lookups by id: `404`, `405` or `501`
    /// ([`super::unserved`]), or a plain message list where the answer object belongs. The caller
    /// then makes the lookups it was configured to replace.
    pub(super) async fn bridge_lookup(
        &self,
        channel: &ChannelId,
        thread: Option<&str>,
        wanted: &[MessageId],
    ) -> Result<Option<Vec<crate::chat::Lookup>>, ChatError> {
        let mut seen = HashSet::new();
        let distinct: Vec<MessageId> = wanted
            .iter()
            .filter(|id| seen.insert(id.as_str()))
            .cloned()
            .collect();
        let mut answers = std::collections::HashMap::new();
        for chunk in distinct.chunks(super::BRIDGE_LOOKUP_IDS) {
            let request = super::ids_request(&self.api_base, channel, thread, chunk)?;
            let value = match self.send_with_timeout(request, TIMELINE_TIMEOUT).await {
                Ok(value) if !value.is_object() => return Ok(None),
                Ok(value) => value,
                Err(error) if super::unserved(&error) => return Ok(None),
                Err(error) => return Err(error),
            };
            answers.extend(super::parse_id_lookup(&value, channel, thread, chunk)?);
        }
        Ok(Some(
            wanted
                .iter()
                .map(|id| {
                    answers
                        .get(id)
                        .cloned()
                        .unwrap_or(crate::chat::Lookup::Unseen)
                })
                .collect(),
        ))
    }

    /// One of the channel's own messages, from Discord itself, as its message history has it.
    async fn channel_message(
        &self,
        channel: &ChannelId,
        message_id: &MessageId,
    ) -> Result<Message, ChatError> {
        let value = self
            .read_thread_json(
                &[
                    "channels",
                    channel.as_str(),
                    "messages",
                    message_id.as_str(),
                ],
                &[],
            )
            .await?;
        let message = parse_message(&value)?;
        if message.channel_id != *channel || message.id != *message_id {
            return Err(shape("message lookup returned a different message"));
        }
        Ok(message)
    }

    pub(super) async fn thread_post(
        &self,
        channel: &ChannelId,
        id: &str,
        content: &str,
        reply_to: Option<&MessageId>,
        key: Option<&str>,
    ) -> Result<Message, ChatError> {
        match self.thread_api {
            ThreadApi::Off => Err(refused("thread posting is disabled on this backend")),
            ThreadApi::Bridge => {
                // `#195 send-resilience`: the caller's idempotency key, when it sent one, is this
                // post's provider request id on every attempt; see `supports_idempotent_posts`.
                let nonce = key.map_or_else(fresh_post_nonce, str::to_owned);
                let mut request = post_request_with_nonce(
                    &self.api_base,
                    channel,
                    content,
                    reply_to,
                    Some(&nonce),
                    &self.post_limit,
                )?;
                request.url = self
                    .thread_request(
                        "POST",
                        &["channels", channel.as_str(), "threads", id, "messages"],
                        &[],
                    )?
                    .url;
                let value = self.send_with_timeout(request, TIMELINE_TIMEOUT).await?;
                let mut message = parse_message(&value)?;
                confirm_bridge_post(&mut message, channel, id)?;
                Ok(message)
            }
            ThreadApi::Native => {
                let thread = self.verified_thread(channel, id).await?;
                // The root is context for this destination, not a cross-channel reply reference.
                let reply_to = reply_to.filter(|message| message.as_str() != id);
                if let Some(message) = reply_to {
                    self.thread_message(channel, id, message).await?;
                }
                let value = self
                    .send(post_request_with_nonce(
                        &self.api_base,
                        &ChannelId(id.to_owned()),
                        content,
                        reply_to,
                        None,
                        &self.post_limit,
                    )?)
                    .await?;
                let mut message = parse_message(&value)?;
                if message.channel_id.as_str() != id {
                    return Err(shape("thread post returned a different channel"));
                }
                message.channel_id = channel.clone();
                message.thread = Some(thread.membership(false));
                Ok(message)
            }
        }
    }
}

fn message_page(
    mut messages: Vec<Message>,
    channel: &ChannelId,
    request: &TimelineRequest,
    snapshot: Option<u64>,
    limit: usize,
) -> Result<TimelinePage, ChatError> {
    messages.sort_by_key(|message| message.id.numeric().unwrap_or(0));
    messages.dedup_by(|later, earlier| later.id == earlier.id);
    let has_more = messages.len() > limit;
    if has_more {
        messages.drain(..messages.len() - limit);
    }
    let next_before = if has_more {
        Some(
            NativeCursor {
                channel: channel.0.clone(),
                view: request.view,
                thread_id: request.thread_id.clone(),
                snapshot,
                boundary: messages[0].id.0.clone(),
            }
            .encode()?,
        )
    } else {
        None
    };
    Ok(TimelinePage {
        messages,
        has_more,
        next_before,
        ..TimelinePage::default()
    })
}

fn validate_bridge_message(
    message: &Message,
    channel: &ChannelId,
    thread: Option<&str>,
) -> Result<(), ChatError> {
    if message.channel_id != *channel {
        return Err(shape("timeline response included a different channel"));
    }
    if let Some(thread) = thread {
        if message
            .thread
            .as_ref()
            .map(|membership| membership.id.as_str())
            != Some(thread)
        {
            return Err(shape("timeline response included a different thread"));
        }
    }
    Ok(())
}

/// A bridge's answer to a post into one of its threads, read as the confirmation it is.
/// `send-confirm-thread`.
///
/// The answer must be in the channel posted to and, where it names a thread, in the thread posted
/// to: one naming another channel or thread came from a bridge that put the post somewhere else,
/// and is refused as before. But a message's `thread` is optional on the wire, and a bridge may
/// confirm a post it delivered without it. A Google Chat bridge did, on a reply the owner sent
/// into a thread: its live event stream had stored the new message before the provider's send
/// command returned, and it answered with that stored copy, which carried no membership. The
/// reply was in the thread and its recipient answered it; vibe-talk said "Delivery unconfirmed",
/// and a Retry, asking for the same post again, got the same answer.
///
/// The thread route posts into that thread or fails, so a message it answers with and does not
/// place is in that thread, and a reply there: a thread's root is the message the thread began
/// with, which was there before anything could be posted into it. The root's id and the reply
/// count are left unknown rather than guessed. A retry is unchanged by this: it is the same
/// request under the same nonce, and a bridge that recognises it answers with the same message.
fn confirm_bridge_post(
    message: &mut Message,
    channel: &ChannelId,
    thread: &str,
) -> Result<(), ChatError> {
    if message.channel_id != *channel {
        return Err(shape("thread post was answered from a different channel"));
    }
    match &message.thread {
        Some(membership) if membership.id != thread => {
            Err(shape("thread post was answered from a different thread"))
        }
        Some(_) => Ok(()),
        None => {
            message.thread = Some(MessageThread {
                id: thread.to_owned(),
                root_message_id: None,
                is_root: false,
                reply_count: None,
                reply_count_exact: false,
            });
            Ok(())
        }
    }
}

fn parse_bridge_summary(mut value: Value, channel: &ChannelId) -> Result<ThreadSummary, ChatError> {
    let root = value
        .get_mut("root")
        .map(Value::take)
        .filter(|value| !value.is_null())
        .map(|value| parse_message(&value))
        .transpose()?;
    let mut summary: ThreadSummary = serde_json::from_value(value)
        .map_err(|error| shape(&format!("invalid thread summary: {error}")))?;
    if let Some(root) = &root {
        validate_bridge_message(root, channel, Some(&summary.id))?;
    }
    summary.root = root;
    Ok(summary)
}

/// A bridge's refusal of a forward read, as the cursor problem it is. `#203 incremental-refresh`.
///
/// 410 is the contract's own answer for a cursor the bridge can no longer serve. 400, 405 and 501
/// can only reach a forward read after the bridge was rolled back to a version without `after`,
/// because vibe-talk sends `after` only to a bridge that issued a cursor: that cursor is just as
/// unusable, and the newest read the caller makes next learns the capability is gone. Any other
/// failure, and every failure of a read without `after`, is unchanged.
fn forward_refusal(error: ChatError, request: &TimelineRequest) -> ChatError {
    if request.after.is_none() {
        return error;
    }
    match error.cause() {
        ChatError::Status { status, body } if matches!(status, 400 | 405 | 410 | 501) => {
            ChatError::CursorExpired(format!(
                "the bridge answered HTTP {status} to a forward read: {body}"
            ))
        }
        _ => error,
    }
}

/// What the forward-read fields of a bridge page may say. `#203 incremental-refresh`.
///
/// A delta answers only `after`, always continues with `next_after`, and says nothing about older
/// history. An answer to `after` WITHOUT a delta is a bridge that ignored the parameter: it is a
/// newest page and passes as one, because the caller folds by what it received, not what it asked.
fn validate_bridge_forward(
    page: &TimelinePage,
    request: &TimelineRequest,
) -> Result<(), ChatError> {
    if let Some(cursor) = &page.next_after {
        if cursor.is_empty()
            || cursor.len() > MAX_BRIDGE_FORWARD_CURSOR
            || !cursor.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(shape(
                "next_after must be a URL-safe string of at most 2048 characters",
            ));
        }
    }
    if let Some(at) = &page.as_of {
        at.parse::<jiff::Timestamp>()
            .map_err(|_| shape("as_of is not an RFC 3339 instant"))?;
    }
    let Some(delta) = &page.delta else {
        return Ok(());
    };
    if request.after.is_none() {
        return Err(shape(
            "a timeline delta answered a read that asked for none",
        ));
    }
    if page.next_after.is_none() {
        return Err(shape("a timeline delta has no next_after to continue from"));
    }
    if page.has_more || page.next_before.is_some() {
        return Err(shape("a timeline delta claimed older history"));
    }
    if delta.deleted.len() > MAX_DELTA_REMOVALS || delta.removed_threads.len() > MAX_DELTA_REMOVALS
    {
        return Err(shape("a timeline delta listed more than 1000 removals"));
    }
    if delta
        .deleted
        .iter()
        .map(|id| id.as_str())
        .chain(delta.removed_threads.iter().map(String::as_str))
        .any(|id| id.is_empty() || id.len() > 2048)
    {
        return Err(shape("a timeline delta named an empty or oversized id"));
    }
    Ok(())
}

pub(super) fn parse_bridge_page(
    mut value: Value,
    channel: &ChannelId,
    request: &TimelineRequest,
) -> Result<TimelinePage, ChatError> {
    let messages = value
        .get_mut("messages")
        .map(Value::take)
        .and_then(|value| value.as_array().cloned())
        .ok_or_else(|| shape("timeline response has no message list"))?;
    let messages: Vec<Message> = messages
        .iter()
        .map(parse_message)
        .collect::<Result<_, _>>()?;
    for message in &messages {
        validate_bridge_message(message, channel, request.thread_id.as_deref())?;
    }
    let summaries = value
        .get_mut("threads")
        .map(Value::take)
        .and_then(|value| value.as_array().cloned())
        .ok_or_else(|| shape("timeline response has no thread list"))?;
    let threads = summaries
        .into_iter()
        .map(|value| parse_bridge_summary(value, channel))
        .collect::<Result<_, _>>()?;
    let thread = value
        .get_mut("thread")
        .map(Value::take)
        .filter(|value| !value.is_null())
        .map(|value| parse_bridge_summary(value, channel))
        .transpose()?;
    // Deserialize required pagination metadata only after removing the wire-format messages.
    value["messages"] = serde_json::json!([]);
    value["threads"] = serde_json::json!([]);
    value["thread"] = Value::Null;
    let mut page: TimelinePage = serde_json::from_value(value)
        .map_err(|error| shape(&format!("invalid timeline metadata: {error}")))?;
    if page.has_more != page.next_before.is_some() {
        return Err(shape("timeline continuation disagrees with has_more"));
    }
    validate_bridge_forward(&page, request)?;
    if request.view == TimelineView::Thread
        && thread.as_ref().map(|thread| thread.id.as_str()) != request.thread_id.as_deref()
    {
        return Err(shape("timeline selected a different thread"));
    }
    page.messages = messages;
    page.threads = threads;
    // Outside the thread view, `thread` names the one conversation a narrowed registration is
    // scoped to. That channel has no child threads: the only summary it can list is itself, and
    // offering it under Threads presents the channel as though it were a different conversation.
    if request.view != TimelineView::Thread {
        if let Some(scope) = &thread {
            page.threads.retain(|summary| summary.id != scope.id);
            page.has_threads = false;
        }
    }
    page.thread = thread;
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::ChatClient as _;
    use axum::http::{Method, StatusCode, Uri};
    use axum::response::IntoResponse as _;
    use axum::{Json, Router};
    use serde_json::json;
    use std::collections::HashMap;

    type Replies = HashMap<String, (StatusCode, Value)>;

    struct Mock {
        client: HttpDiscordClient,
        seen: Arc<Mutex<Vec<String>>>,
        bodies: Arc<Mutex<Vec<Value>>>,
        server: tokio::task::JoinHandle<()>,
    }

    impl Drop for Mock {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    async fn mock(routes: Replies, mode: ThreadApi) -> Mock {
        mock_with(routes, |discord| discord.thread_api = mode).await
    }

    async fn mock_with(
        routes: Replies,
        configure: impl FnOnce(&mut crate::config::DiscordConfig),
    ) -> Mock {
        let routes = Arc::new(routes);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&seen);
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let captured_bodies = Arc::clone(&bodies);
        let app =
            Router::new().fallback(move |method: Method, uri: Uri, body: axum::body::Bytes| {
                let routes = Arc::clone(&routes);
                let captured = Arc::clone(&captured);
                let captured_bodies = Arc::clone(&captured_bodies);
                async move {
                    if !body.is_empty() {
                        captured_bodies
                            .lock()
                            .expect("bodies")
                            .push(serde_json::from_slice(&body).expect("JSON body"));
                    }
                    let key = format!("{method} {uri}");
                    captured.lock().expect("seen").push(key.clone());
                    match routes.get(&key) {
                        Some((status, value)) => (*status, Json(value.clone())).into_response(),
                        None => (
                            StatusCode::NOT_FOUND,
                            Json(json!({"missing_mock_route":key})),
                        )
                            .into_response(),
                    }
                }
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("server");
        });
        let mut config = crate::testing::config();
        let discord = config.discord_mut().expect("discord provider");
        discord.api_base = format!("http://{address}");
        discord.provider_name = "Example Chat".to_owned();
        configure(discord);
        let client = HttpDiscordClient::new(discord).expect("client");
        Mock {
            client,
            seen,
            bodies,
            server,
        }
    }

    fn message(channel: &str, id: &str, content: &str) -> Value {
        json!({"id":id,"channel_id":channel,"author":{"id":"42","username":"person"},
            "timestamp":"2026-09-19T10:00:00Z","content":content})
    }

    fn thread(id: &str, last: &str, parent: &str) -> Value {
        json!({"id":id,"type":11,"parent_id":parent,"last_message_id":last,"name":format!("Thread {id}"),
            "message_count":2,"thread_metadata":{"archive_timestamp":"2026-09-18T10:00:00Z"}})
    }

    fn ok(routes: &mut Replies, path: &str, value: Value) {
        routes.insert(format!("GET {path}"), (StatusCode::OK, value));
    }

    fn inventory_routes() -> Replies {
        let mut routes = Replies::new();
        ok(
            &mut routes,
            "/channels/100",
            json!({"id":"100","type":0,"guild_id":"9"}),
        );
        ok(
            &mut routes,
            "/guilds/9/threads/active",
            json!({"threads":[thread("201","901","100"),thread("999","9999","9990")]}),
        );
        ok(
            &mut routes,
            "/channels/100/threads/archived/public?limit=100",
            json!({"threads":[thread("301","801","100")],"has_more":false}),
        );
        routes.insert(
            "GET /channels/100/threads/archived/private?limit=100".into(),
            (StatusCode::FORBIDDEN, json!({})),
        );
        ok(
            &mut routes,
            "/channels/100/users/@me/threads/archived/private?limit=100",
            json!({"threads":[],"has_more":false}),
        );
        ok(&mut routes, "/channels/201", thread("201", "901", "100"));
        ok(&mut routes, "/channels/301", thread("301", "801", "100"));
        ok(
            &mut routes,
            "/channels/100/messages/201",
            message("100", "201", "first root"),
        );
        ok(
            &mut routes,
            "/channels/100/messages/301",
            message("100", "301", "second root"),
        );
        routes
    }

    #[tokio::test]
    async fn native_flat_pages_merge_every_discovered_stream_without_repeating_roots() {
        let mut routes = inventory_routes();
        let first_root = message("100", "201", "first root");
        let second_root = message("100", "301", "second root");
        let mut first_starter = message("201", "201", "");
        first_starter["type"] = json!(21);
        first_starter["referenced_message"] = first_root.clone();
        let mut second_starter = message("301", "301", "");
        second_starter["type"] = json!(21);
        second_starter["referenced_message"] = second_root.clone();
        ok(
            &mut routes,
            "/channels/100/messages?limit=3",
            json!([message("100", "850", "main"), second_root, first_root]),
        );
        ok(
            &mut routes,
            "/channels/201/messages?limit=3",
            json!([
                message("201", "901", "new reply"),
                message("201", "701", "old reply"),
                first_starter
            ]),
        );
        ok(
            &mut routes,
            "/channels/301/messages?limit=3",
            json!([message("301", "801", "archived reply"), second_starter]),
        );
        for before in ["850", "701"] {
            ok(
                &mut routes,
                &format!("/channels/100/messages?limit=3&before={before}"),
                json!([
                    message("100", "301", "second root"),
                    message("100", "201", "first root")
                ]),
            );
            ok(
                &mut routes,
                &format!("/channels/201/messages?limit=3&before={before}"),
                if before == "850" {
                    json!([message("201", "701", "old reply"), first_starter])
                } else {
                    json!([first_starter])
                },
            );
            ok(
                &mut routes,
                &format!("/channels/301/messages?limit=3&before={before}"),
                if before == "850" {
                    json!([message("301", "801", "archived reply"), second_starter])
                } else {
                    json!([second_starter])
                },
            );
        }
        let mock = mock(routes, ThreadApi::Native).await;
        let channel = ChannelId("100".into());
        let mut request = TimelineRequest {
            view: TimelineView::Flat,
            limit: 2,
            ..TimelineRequest::default()
        };
        let mut got = Vec::new();
        for expected in [["850", "901"], ["701", "801"], ["201", "301"]] {
            let page = mock
                .client
                .fetch_timeline(&channel, &request)
                .await
                .expect("page");
            assert_eq!(
                page.messages
                    .iter()
                    .map(|m| m.id.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
            assert!(page
                .messages
                .iter()
                .all(|message| message.channel_id == channel));
            assert!(page.has_threads);
            got.extend(page.messages);
            request.before = page.next_before;
        }
        assert!(request.before.is_none());
        assert_eq!(got.len(), 6);
        assert_eq!(
            got.iter()
                .filter(|message| message.thread.as_ref().is_some_and(|thread| thread.is_root))
                .count(),
            2
        );
        assert_eq!(
            mock.seen
                .lock()
                .expect("seen")
                .iter()
                .filter(|path| path.contains("/guilds/"))
                .count(),
            1,
            "older pages must use the complete pinned inventory"
        );
    }

    #[tokio::test]
    async fn native_thread_order_uses_last_message_activity_and_pages_with_bound_cursors() {
        let mock = mock(inventory_routes(), ThreadApi::Native).await;
        let channel = ChannelId("100".into());
        let mut request = TimelineRequest {
            view: TimelineView::Threads,
            limit: 1,
            ..TimelineRequest::default()
        };
        let page = mock
            .client
            .fetch_timeline(&channel, &request)
            .await
            .expect("newest thread");
        assert_eq!(
            page.threads[0].id, "201",
            "the newer-created archived thread had older reply activity"
        );
        assert_eq!(
            page.threads[0].root.as_ref().expect("root").content,
            "first root"
        );
        assert!(!page.threads[0].reply_count_exact);
        request.before = page.next_before;
        let other_channel = mock
            .client
            .fetch_timeline(&ChannelId("999".into()), &request)
            .await
            .expect_err("bound cursor");
        assert!(matches!(other_channel.cause(), ChatError::Refused(_)));
        let page = mock
            .client
            .fetch_timeline(&channel, &request)
            .await
            .expect("older thread");
        assert_eq!(page.threads[0].id, "301");
        assert!(!page.has_more);
    }

    #[tokio::test]
    async fn native_thread_reads_and_posts_reject_threads_belonging_to_other_channels() {
        let mut routes = Replies::new();
        ok(&mut routes, "/channels/900", thread("900", "901", "999"));
        let mock = mock(routes, ThreadApi::Native).await;
        let channel = ChannelId("100".into());
        let request = TimelineRequest {
            view: TimelineView::Thread,
            thread_id: Some("900".into()),
            ..TimelineRequest::default()
        };
        assert!(matches!(
            mock.client
                .fetch_timeline(&channel, &request)
                .await
                .expect_err("read refused")
                .cause(),
            ChatError::Refused(_)
        ));
        assert!(matches!(
            mock.client
                .post_in_thread(&channel, "900", "wrong destination", None)
                .await
                .expect_err("post refused")
                .cause(),
            ChatError::Refused(_)
        ));
        assert!(mock
            .client
            .fetch_thread_message(&channel, "900", &MessageId("901".into()))
            .await
            .is_err());
        assert!(mock
            .seen
            .lock()
            .expect("seen")
            .iter()
            .all(|path| path == "GET /channels/900"));
    }

    #[tokio::test]
    async fn native_archive_discovery_refuses_a_non_advancing_cursor() {
        let mut routes = inventory_routes();
        let page = json!({"threads":[thread("301","801","100")],"has_more":true});
        ok(
            &mut routes,
            "/channels/100/threads/archived/public?limit=100",
            page.clone(),
        );
        ok(
            &mut routes,
            "/channels/100/threads/archived/public?limit=100&before=2026-09-18T10%3A00%3A00Z",
            page,
        );
        let mock = mock(routes, ThreadApi::Native).await;
        let request = TimelineRequest {
            view: TimelineView::Flat,
            ..TimelineRequest::default()
        };
        let error = mock
            .client
            .fetch_timeline(&ChannelId("100".into()), &request)
            .await
            .expect_err("must not return partial history");
        assert!(
            error.to_string().contains("cursor did not advance"),
            "{error}"
        );
        assert!(mock
            .seen
            .lock()
            .expect("seen")
            .iter()
            .all(|path| !path.contains("/messages")));
    }

    #[tokio::test]
    async fn native_forum_has_no_main_message_stream() {
        let mut routes = Replies::new();
        ok(
            &mut routes,
            "/channels/100",
            json!({"id":"100","type":15,"guild_id":"9"}),
        );
        ok(
            &mut routes,
            "/guilds/9/threads/active",
            json!({"threads":[]}),
        );
        ok(
            &mut routes,
            "/channels/100/threads/archived/public?limit=100",
            json!({"threads":[],"has_more":false}),
        );
        let mock = mock(routes, ThreadApi::Native).await;
        let page = mock
            .client
            .fetch_timeline(&ChannelId("100".into()), &TimelineRequest::default())
            .await
            .expect("forum main");
        assert!(page.messages.is_empty());
        assert!(!page.has_more);
        assert!(mock
            .seen
            .lock()
            .expect("seen")
            .iter()
            .all(|path| !path.contains("/messages")));
    }

    #[tokio::test]
    async fn bridge_contract_keeps_opaque_ids_and_rejects_cross_channel_messages() {
        let mut routes = Replies::new();
        let mut wire = message("parent", "message-id", "in a thread");
        wire["thread"] = json!({"id":"opaque/thread","root_message_id":"root-id","is_root":false,"reply_count":1,"reply_count_exact":true});
        let summary = json!({"id":"opaque/thread","root":null,"title":"Topic","reply_count":1,"reply_count_exact":true,"updated_at":"2026-09-19T10:00:00Z"});
        ok(
            &mut routes,
            "/channels/parent/timeline?view=thread&limit=50&thread_id=opaque%2Fthread",
            json!({"messages":[wire.clone()],"threads":[],"thread":summary,"has_threads":true,"has_more":false,"next_before":null,"notice":null}),
        );
        ok(
            &mut routes,
            "/channels/parent/threads/opaque%2Fthread/messages/message-id",
            wire.clone(),
        );
        wire["channel_id"] = json!("other-parent");
        ok(
            &mut routes,
            "/channels/parent/timeline?view=flat&limit=50",
            json!({"messages":[wire],"threads":[],"thread":null,"has_threads":true,"has_more":false,"next_before":null,"notice":null}),
        );
        let mock = mock(routes, ThreadApi::Bridge).await;
        let channel = ChannelId("parent".into());
        let request = TimelineRequest {
            view: TimelineView::Thread,
            thread_id: Some("opaque/thread".into()),
            ..TimelineRequest::default()
        };
        let page = mock
            .client
            .fetch_timeline(&channel, &request)
            .await
            .expect("bridge thread");
        assert_eq!(
            page.messages[0].thread.as_ref().expect("membership").id,
            "opaque/thread"
        );
        assert_eq!(
            mock.client
                .fetch_thread_message(&channel, "opaque/thread", &MessageId("message-id".into()))
                .await
                .expect("lookup")
                .id
                .as_str(),
            "message-id"
        );
        let error = mock
            .client
            .fetch_timeline(
                &channel,
                &TimelineRequest {
                    view: TimelineView::Flat,
                    ..TimelineRequest::default()
                },
            )
            .await
            .expect_err("wrong channel");
        assert!(error.to_string().contains("different channel"));
    }

    #[tokio::test]
    async fn a_registration_scoped_to_one_thread_offers_no_child_threads() {
        // The reported failure: a source registered as one conversation reported `has_threads`, so
        // its Threads control listed that same conversation as its only, apparently unrelated, entry.
        let mut routes = Replies::new();
        let mut root = message("scoped", "root-id", "the linked conversation");
        root["thread"] = json!({"id":"opaque/scope","root_message_id":"root-id","is_root":true,"reply_count":2,"reply_count_exact":true});
        let scope = json!({"id":"opaque/scope","root":root.clone(),"title":"Linked","reply_count":2,"reply_count_exact":true,"updated_at":"2026-09-19T10:00:00Z"});
        ok(
            &mut routes,
            "/channels/scoped/timeline?view=main&limit=50",
            json!({"messages":[root],"threads":[],"thread":scope.clone(),"has_threads":true,"has_more":false,"next_before":null,"notice":null}),
        );
        ok(
            &mut routes,
            "/channels/scoped/timeline?view=threads&limit=50",
            json!({"messages":[],"threads":[scope.clone()],"thread":scope,"has_threads":true,"has_more":false,"next_before":null,"notice":null}),
        );
        let mock = mock(routes, ThreadApi::Bridge).await;
        let channel = ChannelId("scoped".into());
        for view in [TimelineView::Main, TimelineView::Threads] {
            let page = mock
                .client
                .fetch_timeline(
                    &channel,
                    &TimelineRequest {
                        view,
                        ..TimelineRequest::default()
                    },
                )
                .await
                .expect("scoped timeline");
            assert!(
                !page.has_threads,
                "{view:?} offered child-thread navigation"
            );
            assert!(
                page.threads.is_empty(),
                "{view:?} listed the channel as its own child"
            );
            assert_eq!(page.thread.expect("scope").id, "opaque/scope");
        }
    }

    #[tokio::test]
    async fn the_read_markers_scope_is_the_bridge_timelines_word_and_unknown_without_one() {
        // `#292 reply-marks-read`. A narrowed registration's main page names its conversation as
        // `thread`, and its marker is the whole space's; a whole registration's names none.
        use crate::chat::UpstreamReadScope;
        let mut routes = Replies::new();
        let mut root = message("scoped", "root-id", "the linked conversation");
        root["thread"] = json!({"id":"opaque/scope","root_message_id":"root-id","is_root":true,"reply_count":2,"reply_count_exact":true});
        let scope = json!({"id":"opaque/scope","root":root.clone(),"title":"Linked","reply_count":2,"reply_count_exact":true,"updated_at":"2026-09-19T10:00:00Z"});
        ok(
            &mut routes,
            "/channels/scoped/timeline?view=main&limit=1",
            json!({"messages":[root],"threads":[],"thread":scope,"has_threads":false,"has_more":false,"next_before":null,"notice":null}),
        );
        ok(
            &mut routes,
            "/channels/whole/timeline?view=main&limit=1",
            json!({"messages":[message("whole", "m-1", "a space of its own")],"threads":[],"thread":null,"has_threads":false,"has_more":false,"next_before":null,"notice":null}),
        );
        let bridge = mock_with(routes.clone(), |discord| {
            discord.thread_api = ThreadApi::Bridge;
            discord.upstream_read_marks = true;
        })
        .await;
        let id = |text: &str| ChannelId(text.into());
        assert_eq!(
            bridge
                .client
                .upstream_read_scope(&id("scoped"))
                .await
                .expect("scoped"),
            UpstreamReadScope::Wider
        );
        assert_eq!(
            bridge
                .client
                .upstream_read_scope(&id("whole"))
                .await
                .expect("whole"),
            UpstreamReadScope::Channel
        );
        // A failed read is a failure, not a guess either way.
        assert!(bridge
            .client
            .upstream_read_scope(&id("gone"))
            .await
            .is_err());

        // Native threads and threads turned off serve a narrowed registration as an ordinary
        // channel, so they cannot say, and nothing is read; nor is it without the read-mark opt-in.
        for (mode, marks) in [
            (ThreadApi::Native, true),
            (ThreadApi::Off, true),
            (ThreadApi::Bridge, false),
        ] {
            let other = mock_with(routes.clone(), |discord| {
                discord.thread_api = mode;
                discord.upstream_read_marks = marks;
            })
            .await;
            assert_eq!(
                other
                    .client
                    .upstream_read_scope(&id("scoped"))
                    .await
                    .expect("no read"),
                UpstreamReadScope::Unknown,
                "{mode:?}, marks {marks}"
            );
            let seen = other.seen.lock().expect("seen").clone();
            assert!(seen.is_empty(), "{mode:?}, marks {marks}: {seen:?}");
        }
    }

    /// The same three messages as `/messages` answers them, newest first, and as the newest Flat
    /// page does, oldest first with one reply's thread membership and an `as_of`.
    fn recent_routes() -> Replies {
        let mut routes = Replies::new();
        let wire = |id: &str| message("parent", id, &format!("text {id}"));
        ok(
            &mut routes,
            "/channels/parent/messages?limit=3",
            json!([wire("1003"), wire("1002"), wire("1001")]),
        );
        let mut reply = wire("1003");
        reply["thread"] = json!({"id":"t-1","root_message_id":"1002","is_root":false,
            "reply_count":1,"reply_count_exact":true});
        let mut page = bridge_page(json!([wire("1001"), wire("1002"), reply]));
        page["has_more"] = json!(true);
        page["next_before"] = json!("opaque-back");
        page["as_of"] = json!("2026-10-12T09:00:00Z");
        ok(
            &mut routes,
            "/channels/parent/timeline?view=flat&limit=3",
            page,
        );
        routes
    }

    fn ids_of(messages: &[Message]) -> Vec<&str> {
        messages.iter().map(|m| m.id.as_str()).collect()
    }

    /// `#208 persistent-cache`, milestone `bridge-recent-flat`. Off by default: a bridge's recent
    /// read is `/messages`, and claims no `as_of`, exactly as before.
    #[tokio::test]
    async fn a_bridge_reads_its_recent_messages_from_messages_unless_told_otherwise() {
        let mock = mock(recent_routes(), ThreadApi::Bridge).await;
        let channel = ChannelId("parent".into());
        let read = mock
            .client
            .fetch_recent_dated(&channel, 3)
            .await
            .expect("recent read");
        assert_eq!(ids_of(&read.messages), ["1001", "1002", "1003"]);
        assert_eq!((read.as_of, read.exhausted), (None, false));
        assert_eq!(
            ids_of(&mock.client.fetch_recent(&channel, 3).await.expect("read")),
            ["1001", "1002", "1003"]
        );
        let seen = mock.seen.lock().expect("seen").clone();
        assert_eq!(seen, ["GET /channels/parent/messages?limit=3"; 2]);
    }

    /// With `recent_view = "flat"`, the newest Flat page answers, in the window's shape: the same
    /// messages in the same order, the page's `as_of` kept, its cursor and thread list dropped.
    #[tokio::test]
    async fn a_flat_recent_read_has_the_shape_of_a_messages_read_and_keeps_as_of() {
        let mock = mock_with(recent_routes(), |discord| {
            discord.thread_api = ThreadApi::Bridge;
            discord.recent.view = crate::config::RecentView::Flat;
        })
        .await;
        let channel = ChannelId("parent".into());
        let read = mock
            .client
            .fetch_recent_dated(&channel, 3)
            .await
            .expect("recent read");
        assert_eq!(ids_of(&read.messages), ["1001", "1002", "1003"]);
        assert_eq!(read.as_of.as_deref(), Some("2026-10-12T09:00:00Z"));
        assert!(!read.exhausted, "a bridge proves no channel start");
        assert_eq!(
            read.messages[2].thread.as_ref().map(|t| t.id.as_str()),
            Some("t-1"),
            "thread membership is part of the message"
        );
        // The plain read and the startup probe go the same way.
        let plain = mock.client.fetch_recent(&channel, 3).await.expect("read");
        assert_eq!(plain, read.messages);
        let seen = mock.seen.lock().expect("seen").clone();
        assert_eq!(seen, ["GET /channels/parent/timeline?view=flat&limit=3"; 2]);
    }

    #[test]
    fn a_flat_page_becomes_a_window_oldest_first_once_each_and_at_most_the_limit() {
        let parsed = |id: &str| parse_message(&message("parent", id, "m")).expect("a message");
        let page = TimelinePage {
            messages: vec![parsed("12"), parsed("10"), parsed("11"), parsed("12")],
            as_of: Some("2026-10-12T09:00:00Z".into()),
            has_more: true,
            next_before: Some("back".into()),
            ..TimelinePage::default()
        };
        let read = page.into_recent(2);
        assert_eq!(ids_of(&read.messages), ["11", "12"]);
        assert_eq!(read.as_of.as_deref(), Some("2026-10-12T09:00:00Z"));

        // Newest first, with a repeat: sorted before the repeat is dropped and the oldest cut, or
        // the newest messages would be the ones lost.
        let page = TimelinePage {
            messages: ["13", "12", "12", "11", "10"]
                .into_iter()
                .map(parsed)
                .collect(),
            ..TimelinePage::default()
        };
        let read = page.into_recent(3);
        assert_eq!(ids_of(&read.messages), ["11", "12", "13"]);
        assert_eq!(read.as_of, None);
    }

    fn bridge_page(messages: Value) -> Value {
        json!({"messages":messages,"threads":[],"thread":null,"has_threads":false,
            "has_more":false,"next_before":null,"notice":null})
    }

    fn after(cursor: &str) -> TimelineRequest {
        TimelineRequest {
            after: Some(cursor.to_owned()),
            ..TimelineRequest::default()
        }
    }

    /// `#203 incremental-refresh`. A bridge advertises forward reads only by issuing `next_after`;
    /// the client hands `after` back untouched and keeps what the delta says about itself.
    #[tokio::test]
    async fn bridge_forward_reads_pass_the_cursor_through_and_keep_the_delta() {
        let mut routes = Replies::new();
        let mut newest = bridge_page(json!([message("parent", "m-1", "old")]));
        newest["next_after"] = json!("rev-1");
        ok(
            &mut routes,
            "/channels/parent/timeline?view=main&limit=50",
            newest,
        );
        let mut delta = bridge_page(json!([message("parent", "m-2", "new")]));
        delta["next_after"] = json!("rev-2");
        delta["delta"] =
            json!({"more":true,"complete":true,"deleted":["m-0"],"removed_threads":[]});
        delta["as_of"] = json!("2026-10-04T07:00:00Z");
        ok(
            &mut routes,
            "/channels/parent/timeline?view=main&limit=50&after=rev-1",
            delta,
        );
        // A bridge that ignored the parameter answers with a newest page, and it passes as one.
        ok(
            &mut routes,
            "/channels/parent/timeline?view=main&limit=50&after=rev-2",
            bridge_page(json!([message("parent", "m-2", "new")])),
        );
        let mock = mock(routes, ThreadApi::Bridge).await;
        let channel = ChannelId("parent".into());
        let page = mock
            .client
            .fetch_timeline(&channel, &TimelineRequest::default())
            .await
            .expect("newest page");
        assert_eq!(page.next_after.as_deref(), Some("rev-1"));
        assert_eq!(page.delta, None);
        let page = mock
            .client
            .fetch_timeline(&channel, &after("rev-1"))
            .await
            .expect("delta");
        assert_eq!(page.messages[0].id.as_str(), "m-2");
        assert_eq!(page.next_after.as_deref(), Some("rev-2"));
        assert_eq!(page.as_of.as_deref(), Some("2026-10-04T07:00:00Z"));
        let delta = page.delta.expect("a delta");
        assert!(delta.more && delta.complete);
        assert_eq!(delta.deleted, [MessageId("m-0".into())]);
        let page = mock
            .client
            .fetch_timeline(&channel, &after("rev-2"))
            .await
            .expect("an ignored after");
        assert_eq!((page.delta, page.next_after), (None, None));
        let both = TimelineRequest {
            before: Some("b".into()),
            ..after("rev-1")
        };
        let error = mock
            .client
            .fetch_timeline(&channel, &both)
            .await
            .expect_err("both cursors");
        assert!(matches!(error.cause(), ChatError::Refused(_)), "{error}");
        assert_eq!(
            mock.seen.lock().expect("seen").len(),
            3,
            "a refused read was sent"
        );
    }

    #[tokio::test]
    async fn a_bridge_delta_that_breaks_the_contract_is_refused() {
        let delta = |fields: Value| {
            let mut page = bridge_page(json!([]));
            page["next_after"] = json!("rev-9");
            page["delta"] = json!({"more":false,"complete":true,"deleted":[],"removed_threads":[]});
            for (key, value) in fields.as_object().expect("fields") {
                page[key] = value.clone();
            }
            page
        };
        let cases = [
            ("delta on a newest read", None, delta(json!({}))),
            (
                "no next_after",
                Some("c"),
                delta(json!({"next_after":null})),
            ),
            (
                "older history",
                Some("c"),
                delta(json!({"has_more":true,"next_before":"older"})),
            ),
            (
                "too many deletions",
                Some("c"),
                delta(json!({"delta":{"more":false,"complete":true,
                    "deleted":vec!["x"; MAX_DELTA_REMOVALS + 1],"removed_threads":[]}})),
            ),
            (
                "an empty id",
                Some("c"),
                delta(
                    json!({"delta":{"more":false,"complete":true,"deleted":[""],"removed_threads":[]}}),
                ),
            ),
            (
                "an oversized next_after",
                None,
                delta(json!({"delta":null,"next_after":"n".repeat(MAX_BRIDGE_FORWARD_CURSOR + 1)})),
            ),
            (
                "a next_after with a space",
                None,
                delta(json!({"delta":null,"next_after":"two words"})),
            ),
            (
                "an unreadable as_of",
                Some("c"),
                delta(json!({"as_of":"seven"})),
            ),
        ];
        for (why, cursor, answer) in cases {
            let mut routes = Replies::new();
            let path = match cursor {
                Some(cursor) => {
                    format!("/channels/parent/timeline?view=main&limit=50&after={cursor}")
                }
                None => "/channels/parent/timeline?view=main&limit=50".to_owned(),
            };
            ok(&mut routes, &path, answer);
            let mock = mock(routes, ThreadApi::Bridge).await;
            let request = cursor.map_or_else(TimelineRequest::default, after);
            let error = mock
                .client
                .fetch_timeline(&ChannelId("parent".into()), &request)
                .await
                .expect_err(why);
            assert!(
                matches!(error.cause(), ChatError::Shape(_)),
                "{why}: {error}"
            );
        }
    }

    #[tokio::test]
    async fn a_bridge_that_cannot_continue_a_forward_read_expires_the_cursor() {
        for (status, expired) in [
            (StatusCode::GONE, true),
            (StatusCode::BAD_REQUEST, true),
            (StatusCode::METHOD_NOT_ALLOWED, true),
            (StatusCode::NOT_IMPLEMENTED, true),
            (StatusCode::NOT_FOUND, false),
            (StatusCode::BAD_GATEWAY, false),
        ] {
            let mut routes = Replies::new();
            routes.insert(
                "GET /channels/parent/timeline?view=main&limit=50&after=old".into(),
                (
                    status,
                    json!({"error":"cursor_expired","message":"issued by an earlier process"}),
                ),
            );
            routes.insert(
                "GET /channels/parent/timeline?view=main&limit=50".into(),
                (status, json!({"message":"no"})),
            );
            let mock = mock(routes, ThreadApi::Bridge).await;
            let channel = ChannelId("parent".into());
            let error = mock
                .client
                .fetch_timeline(&channel, &after("old"))
                .await
                .expect_err("refused forward read");
            assert_eq!(
                matches!(error.cause(), ChatError::CursorExpired(_)),
                expired,
                "HTTP {status} to a forward read: {error}"
            );
            if expired {
                assert!(
                    error.to_string().contains("can no longer continue"),
                    "{error}"
                );
                assert!(!error.is_transient());
            }
            let error = mock
                .client
                .fetch_timeline(&channel, &TimelineRequest::default())
                .await
                .expect_err("refused newest read");
            assert!(
                matches!(error.cause(), ChatError::Status { .. }),
                "HTTP {status} to a newest read is not a cursor problem: {error}"
            );
        }
    }

    #[tokio::test]
    async fn thread_root_lookup_falls_back_to_the_verified_parent_message() {
        let mut routes = inventory_routes();
        routes.insert(
            "GET /channels/201/messages/201".into(),
            (StatusCode::NOT_FOUND, json!({})),
        );
        let mock = mock(routes, ThreadApi::Native).await;
        let message = mock
            .client
            .fetch_thread_message(&ChannelId("100".into()), "201", &MessageId("201".into()))
            .await
            .expect("root lookup");
        assert_eq!(message.content, "first root");
        assert!(message.thread.expect("thread context").is_root);
    }

    #[tokio::test]
    async fn bridge_thread_post_carries_nonce_and_explicit_reply_in_the_scoped_route() {
        let mut routes = Replies::new();
        let mut wire = message("parent", "new-message", "answer");
        wire["thread"] = json!({"id":"opaque/thread","root_message_id":"root","is_root":false,"reply_count":null,"reply_count_exact":false});
        routes.insert(
            "POST /channels/parent/threads/opaque%2Fthread/messages".into(),
            (StatusCode::OK, wire),
        );
        let mock = mock(routes, ThreadApi::Bridge).await;
        let posted = mock
            .client
            .post_in_thread(
                &ChannelId("parent".into()),
                "opaque/thread",
                "answer",
                Some(&MessageId("earlier-message".into())),
            )
            .await
            .expect("post");
        assert_eq!(posted.id.as_str(), "new-message");
        {
            let bodies = mock.bodies.lock().expect("bodies");
            assert_eq!(bodies.len(), 1);
            assert_eq!(
                bodies[0]["message_reference"]["message_id"],
                "earlier-message"
            );
            assert!(bodies[0]["nonce"]
                .as_str()
                .is_some_and(|nonce| nonce.len() > 20));
        }
        assert!(mock
            .client
            .post_in_thread(&ChannelId("parent".into()), "..", "answer", None)
            .await
            .is_err());
        assert_eq!(
            mock.seen.lock().expect("seen").len(),
            1,
            "invalid path segments must not be sent"
        );
    }

    /// `send-confirm-thread`. The owner's reply into a thread was delivered, and the bridge
    /// answered with a copy of the message that carried no `thread`: its live event stream had
    /// stored the message before the provider's send command returned. vibe-talk refused that
    /// answer as "a different thread" and showed a delivered reply as unconfirmed. A message the
    /// thread route answers with and does not place is in that thread; one placed elsewhere, or
    /// in another channel, is still refused.
    #[tokio::test]
    async fn a_bridge_thread_post_answered_without_membership_is_confirmed_in_that_thread() {
        let mut routes = Replies::new();
        routes.insert(
            "POST /channels/parent/threads/opaque%2Fthread/messages".into(),
            (StatusCode::OK, message("parent", "new-message", "answer")),
        );
        let mut elsewhere = message("parent", "misplaced", "answer");
        elsewhere["thread"] = json!({"id":"another/thread","root_message_id":"root",
            "is_root":false,"reply_count":null,"reply_count_exact":false});
        routes.insert(
            "POST /channels/parent/threads/wrong-thread/messages".into(),
            (StatusCode::OK, elsewhere),
        );
        routes.insert(
            "POST /channels/parent/threads/wrong-channel/messages".into(),
            (
                StatusCode::OK,
                message("other-parent", "misplaced", "answer"),
            ),
        );
        let mock = mock(routes, ThreadApi::Bridge).await;
        let channel = ChannelId("parent".into());
        let posted = mock
            .client
            .post_in_thread(
                &channel,
                "opaque/thread",
                "answer",
                Some(&MessageId("quoted".into())),
            )
            .await
            .expect("a delivered reply is confirmed");
        assert_eq!(posted.id.as_str(), "new-message");
        assert_eq!(
            posted.thread,
            Some(MessageThread {
                id: "opaque/thread".into(),
                root_message_id: None,
                is_root: false,
                reply_count: None,
                reply_count_exact: false,
            })
        );
        for (thread, said) in [
            ("wrong-thread", "different thread"),
            ("wrong-channel", "different channel"),
        ] {
            let error = mock
                .client
                .post_in_thread(&channel, thread, "answer", None)
                .await
                .expect_err("an answer from somewhere else is not a confirmation");
            assert!(error.to_string().contains(said), "{thread}: {error}");
            assert!(error.may_have_delivered(), "{thread}: {error}");
        }
    }

    #[tokio::test]
    async fn exceeding_native_discovery_budget_is_an_error_not_a_partial_flat_page() {
        let mut routes = inventory_routes();
        let threads: Vec<_> = (0..=MAX_THREADS)
            .map(|index| thread(&(index + 1000).to_string(), "99999", "100"))
            .collect();
        ok(
            &mut routes,
            "/guilds/9/threads/active",
            json!({"threads":threads}),
        );
        let mock = mock(routes, ThreadApi::Native).await;
        let error = mock
            .client
            .fetch_timeline(
                &ChannelId("100".into()),
                &TimelineRequest {
                    view: TimelineView::Flat,
                    ..TimelineRequest::default()
                },
            )
            .await
            .expect_err("discovery must not be truncated");
        assert!(error.to_string().contains("discovery budget"), "{error}");
        assert!(mock
            .seen
            .lock()
            .expect("seen")
            .iter()
            .all(|path| !path.contains("/messages")));
    }

    #[tokio::test]
    async fn selected_native_history_includes_the_original_root_on_its_oldest_page() {
        let mut routes = inventory_routes();
        let root = message("100", "201", "first root");
        let mut starter = message("201", "201", "");
        starter["type"] = json!(21);
        starter["referenced_message"] = root;
        ok(
            &mut routes,
            "/channels/201/messages?limit=3",
            json!([
                message("201", "901", "new reply"),
                message("201", "701", "old reply"),
                starter
            ]),
        );
        ok(
            &mut routes,
            "/channels/201/messages?limit=3&before=701",
            json!([starter]),
        );
        let mock = mock(routes, ThreadApi::Native).await;
        let mut request = TimelineRequest {
            view: TimelineView::Thread,
            thread_id: Some("201".into()),
            limit: 2,
            ..TimelineRequest::default()
        };
        let page = mock
            .client
            .fetch_timeline(&ChannelId("100".into()), &request)
            .await
            .expect("newest");
        assert_eq!(
            page.messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            ["701", "901"]
        );
        assert_eq!(
            page.thread
                .expect("selected")
                .root
                .expect("root context")
                .content,
            "first root"
        );
        request.before = page.next_before;
        let page = mock
            .client
            .fetch_timeline(&ChannelId("100".into()), &request)
            .await
            .expect("oldest");
        assert_eq!(page.messages.len(), 1);
        assert_eq!(page.messages[0].content, "first root");
        assert!(
            page.messages[0]
                .thread
                .as_ref()
                .expect("membership")
                .is_root
        );
        assert!(!page.has_more);
    }

    /// `#203 incremental-refresh`. A thread, and a text channel's Main, are read forward with one
    /// `after=` request each from the newest snowflake delivered; the thread list and All need the
    /// whole inventory again and issue no native cursor, so the generic reader catches them up.
    #[tokio::test]
    async fn native_threads_and_text_main_read_forward_from_the_newest_snowflake() {
        let mut routes = inventory_routes();
        let root = message("100", "201", "first root");
        let mut starter = message("201", "201", "");
        starter["type"] = json!(21);
        starter["referenced_message"] = root;
        ok(
            &mut routes,
            "/channels/201/messages?limit=3",
            json!([
                message("201", "901", "new reply"),
                message("201", "701", "old reply"),
                starter
            ]),
        );
        // Discord answers `after` newest first as well; the client puts it oldest first.
        ok(
            &mut routes,
            "/channels/201/messages?limit=3&after=901",
            json!([
                message("201", "950", "newest reply"),
                message("201", "920", "newer reply")
            ]),
        );
        ok(
            &mut routes,
            "/channels/201/messages?limit=3&after=950",
            json!([]),
        );
        ok(
            &mut routes,
            "/channels/100/messages?limit=3",
            json!([
                message("100", "850", "main"),
                message("100", "301", "second root"),
                message("100", "201", "first root")
            ]),
        );
        ok(
            &mut routes,
            "/channels/100/messages?limit=3&after=850",
            json!([
                message("100", "880", "later still"),
                message("100", "870", "later"),
                message("100", "860", "soon after")
            ]),
        );
        let mock = mock(routes, ThreadApi::Native).await;
        let channel = ChannelId("100".into());
        let thread = TimelineRequest {
            view: TimelineView::Thread,
            thread_id: Some("201".into()),
            limit: 2,
            ..TimelineRequest::default()
        };
        let newest = mock
            .client
            .fetch_timeline(&channel, &thread)
            .await
            .expect("newest thread page");
        let cursor = newest
            .next_after
            .expect("a thread is read forward natively");
        let ids = |page: &TimelinePage| {
            page.messages
                .iter()
                .map(|message| message.id.0.clone())
                .collect::<Vec<_>>()
        };
        let delta = mock
            .client
            .fetch_timeline(
                &channel,
                &TimelineRequest {
                    after: Some(cursor.clone()),
                    ..thread.clone()
                },
            )
            .await
            .expect("thread delta");
        assert_eq!(ids(&delta), ["920", "950"]);
        assert_eq!(
            delta.delta,
            Some(TimelineDelta {
                more: false,
                complete: false,
                ..TimelineDelta::default()
            })
        );
        assert!(delta
            .messages
            .iter()
            .all(|message| message.thread.as_ref().is_some_and(|t| t.id == "201")));
        assert_eq!(
            delta
                .thread
                .expect("current summary")
                .root
                .expect("root")
                .content,
            "first root"
        );
        assert!(!delta.has_more && delta.next_before.is_none());
        let quiet = mock
            .client
            .fetch_timeline(
                &channel,
                &TimelineRequest {
                    after: delta.next_after.clone(),
                    ..thread.clone()
                },
            )
            .await
            .expect("an empty delta");
        assert!(quiet.messages.is_empty());
        assert_eq!(
            quiet.next_after, delta.next_after,
            "an empty delta moved its cursor"
        );

        let main = TimelineRequest {
            limit: 2,
            ..TimelineRequest::default()
        };
        let newest = mock
            .client
            .fetch_timeline(&channel, &main)
            .await
            .expect("newest main page");
        let delta = mock
            .client
            .fetch_timeline(
                &channel,
                &TimelineRequest {
                    after: newest.next_after.clone(),
                    ..main.clone()
                },
            )
            .await
            .expect("main delta");
        assert_eq!(
            ids(&delta),
            ["860", "870"],
            "the oldest `limit` after the cursor"
        );
        assert!(delta.delta.as_ref().expect("a delta").more);
        assert!(
            delta.has_threads,
            "the newest read's thread navigation was dropped"
        );
        // A cursor issued for Main is no cursor for the thread.
        let crossed = mock
            .client
            .fetch_timeline(
                &channel,
                &TimelineRequest {
                    after: newest.next_after.clone(),
                    ..thread.clone()
                },
            )
            .await
            .expect_err("a cursor from another view");
        assert!(
            matches!(crossed.cause(), ChatError::CursorExpired(_)),
            "{crossed}"
        );
        // A cursor this backend never issued — a bridge's, from before the channel moved to the
        // native API, or one for a view it does not read forward — is gone, not a bad request:
        // the page then reads the newest page instead of failing every refresh with it.
        let thread_list = NativeForward {
            channel: channel.0.clone(),
            view: TimelineView::Threads,
            thread_id: None,
            after: "850".into(),
            threads: true,
        }
        .encode()
        .expect("a cursor for the thread list");
        for (view, cursor) in [
            (thread.clone(), "rev-1".to_owned()),
            (main.clone(), "rev-1".to_owned()),
            (
                TimelineRequest {
                    view: TimelineView::Threads,
                    ..main.clone()
                },
                thread_list,
            ),
        ] {
            let error = mock
                .client
                .fetch_timeline(
                    &channel,
                    &TimelineRequest {
                        after: Some(cursor),
                        ..view
                    },
                )
                .await
                .expect_err("a cursor another backend issued");
            assert!(
                matches!(error.cause(), ChatError::CursorExpired(_)),
                "{error}"
            );
        }
        let threads = mock
            .client
            .fetch_timeline(
                &channel,
                &TimelineRequest {
                    view: TimelineView::Threads,
                    limit: 2,
                    ..TimelineRequest::default()
                },
            )
            .await
            .expect("thread list");
        assert_eq!(
            threads.next_after, None,
            "the thread list issued a native forward cursor"
        );
    }

    #[tokio::test]
    async fn forum_main_and_startup_probe_read_original_posts_without_a_main_message_endpoint() {
        for kind in [15, 16] {
            let mut routes = inventory_routes();
            ok(
                &mut routes,
                "/channels/100",
                json!({"id":"100","type":kind,"guild_id":"9"}),
            );
            ok(
                &mut routes,
                "/channels/201/messages/201",
                message("201", "201", "older original post"),
            );
            ok(
                &mut routes,
                "/channels/301/messages/301",
                message("301", "301", "newer original post"),
            );
            let mock = mock(routes, ThreadApi::Native).await;
            let mut request = TimelineRequest {
                limit: 1,
                ..TimelineRequest::default()
            };
            let page = mock
                .client
                .fetch_timeline(&ChannelId("100".into()), &request)
                .await
                .expect("main originals");
            assert_eq!(
                page.messages[0].id.as_str(),
                "301",
                "Main uses root creation, not newest reply activity"
            );
            assert!(
                page.messages[0]
                    .thread
                    .as_ref()
                    .expect("root membership")
                    .is_root
            );
            assert!(page.has_more);
            assert!(page
                .notice
                .as_deref()
                .is_some_and(|notice| notice.contains("Open a thread")));
            request.before = page.next_before;
            let page = mock
                .client
                .fetch_timeline(&ChannelId("100".into()), &request)
                .await
                .expect("older original");
            assert_eq!(page.messages[0].id.as_str(), "201");
            assert!(!page.has_more);
            let report = crate::probe::probe_channels(
                &mock.client,
                &[crate::model::ChannelInfo {
                    id: ChannelId("100".into()),
                    label: "thread-only container".into(),
                    writable: true,
                    alias: None,
                    added: false,
                    provider: None,
                }],
            )
            .await;
            assert!(!report.is_failure(), "{}", report.render());
            assert!(report.warnings().is_empty());
            let error = mock
                .client
                .post_message(&ChannelId("100".into()), "a reply", None)
                .await
                .expect_err("a thread must be selected before posting");
            assert!(
                matches!(error.cause(), ChatError::Refused(detail) if detail.contains("Open a thread"))
            );
            assert!(mock
                .seen
                .lock()
                .expect("seen")
                .iter()
                .all(|path| !path.starts_with("GET /channels/100/messages")
                    && !path.starts_with("POST ")));
        }
    }

    #[tokio::test]
    async fn flat_history_resolves_missing_starter_content_and_omits_deleted_roots() {
        for deleted in [false, true] {
            let mut routes = inventory_routes();
            let mut starter = message("201", "250", "");
            starter["type"] = json!(21);
            starter["message_reference"] = json!({"message_id":"201","channel_id":"100"});
            if deleted {
                routes.remove("GET /channels/100/messages/201");
            }
            ok(
                &mut routes,
                "/channels/100/messages?limit=51",
                if deleted {
                    json!([])
                } else {
                    json!([message("100", "201", "first root")])
                },
            );
            ok(
                &mut routes,
                "/channels/201/messages?limit=51",
                json!([message("201", "901", "actual reply"), starter]),
            );
            ok(&mut routes, "/channels/301/messages?limit=51", json!([]));
            let mock = mock(routes, ThreadApi::Native).await;
            let page = mock
                .client
                .fetch_timeline(
                    &ChannelId("100".into()),
                    &TimelineRequest {
                        view: TimelineView::Flat,
                        ..TimelineRequest::default()
                    },
                )
                .await
                .expect("flat");
            assert!(page
                .messages
                .iter()
                .all(|message| message.id.as_str() != "250" && !message.content.is_empty()));
            assert_eq!(
                page.messages
                    .iter()
                    .filter(|message| message.id.as_str() == "201")
                    .count(),
                usize::from(!deleted)
            );
            assert!(!page.has_more);
        }
    }

    // remote-ops-audit W11 `#287 bulk-reads`: lookups by id against Discord itself.

    fn lookup_ids(list: &[&str]) -> Vec<MessageId> {
        list.iter().map(|id| MessageId((*id).to_owned())).collect()
    }

    fn outcome(answer: &crate::chat::Lookup) -> String {
        match answer {
            crate::chat::Lookup::Found(message) => format!("found {}", message.id),
            crate::chat::Lookup::Absent => "absent".to_owned(),
            crate::chat::Lookup::Unseen => "unseen".to_owned(),
        }
    }

    fn unknown_message_reply() -> (StatusCode, Value) {
        (
            StatusCode::NOT_FOUND,
            json!({"message": "Unknown Message", "code": 10008}),
        )
    }

    #[tokio::test]
    async fn native_lookups_read_ids_near_each_other_as_one_page_after_the_oldest() {
        let mut routes = Replies::new();
        ok(&mut routes, "/channels/201", thread("201", "905", "100"));
        ok(
            &mut routes,
            "/channels/201/messages?limit=100&after=901",
            json!([
                message("201", "905", "e"),
                message("201", "904", "d"),
                message("201", "903", "c"),
                message("201", "902", "b")
            ]),
        );
        let mock = mock(routes, ThreadApi::Native).await;
        let channel = ChannelId("100".into());
        assert_eq!(
            mock.client.read_caps(&channel).by_ids,
            crate::chat::Support::Native
        );
        let answers = mock
            .client
            .fetch_messages(
                &channel,
                Some("201"),
                &lookup_ids(&["904", "902", "903", "904"]),
            )
            .await
            .expect("lookup");
        assert_eq!(
            answers.iter().map(outcome).collect::<Vec<_>>(),
            ["found 904", "found 902", "found 903", "found 904"],
            "the page holds everything from the oldest asked up to its newest row"
        );
        let crate::chat::Lookup::Found(found) = &answers[0] else {
            unreachable!()
        };
        assert_eq!(
            found.channel_id.as_str(),
            "100",
            "named by its parent channel"
        );
        assert_eq!(found.thread.as_ref().expect("membership").id, "201");
        assert_eq!(
            *mock.seen.lock().expect("seen"),
            [
                "GET /channels/201",
                "GET /channels/201/messages?limit=100&after=901"
            ],
            "the thread checked once and one page, where one at a time was eight requests"
        );
    }

    /// Fix round 1 of remote-ops-audit W11 `#287 bulk-reads`: a page short of 100 is not the end of
    /// the thread. An id past its newest row is asked on its own, and is found when Discord has
    /// it, as it was when looked up alone.
    #[tokio::test]
    async fn native_lookups_ask_for_an_id_past_a_short_page_on_its_own() {
        let mut routes = Replies::new();
        ok(&mut routes, "/channels/201", thread("201", "909", "100"));
        ok(
            &mut routes,
            "/channels/201/messages?limit=100&after=901",
            json!([message("201", "902", "b")]),
        );
        ok(
            &mut routes,
            "/channels/201/messages/909",
            message("201", "909", "exists"),
        );
        routes.insert(
            "GET /channels/201/messages/911".into(),
            unknown_message_reply(),
        );
        let paged = mock(routes, ThreadApi::Native).await;
        let channel = ChannelId("100".into());
        let answers = paged
            .client
            .fetch_messages(&channel, Some("201"), &lookup_ids(&["902", "909", "911"]))
            .await
            .expect("lookup");
        assert_eq!(
            answers.iter().map(outcome).collect::<Vec<_>>(),
            ["found 902", "found 909", "absent"],
            "only Unknown Message proves an id past the page's newest row absent"
        );
        assert_eq!(
            *paged.seen.lock().expect("seen"),
            [
                "GET /channels/201",
                "GET /channels/201/messages?limit=100&after=901",
                "GET /channels/201/messages/909",
                "GET /channels/201/messages/911"
            ]
        );

        // The channel's own messages read the same way.
        let mut routes = Replies::new();
        ok(
            &mut routes,
            "/channels/100/messages?limit=100&after=849",
            json!([message("100", "850", "one")]),
        );
        ok(
            &mut routes,
            "/channels/100/messages/860",
            message("100", "860", "exists"),
        );
        let main = mock(routes, ThreadApi::Native).await;
        let answers = main
            .client
            .fetch_messages(&channel, None, &lookup_ids(&["850", "860"]))
            .await
            .expect("lookup");
        assert_eq!(
            answers.iter().map(outcome).collect::<Vec<_>>(),
            ["found 850", "found 860"]
        );
    }

    /// Fix round 1 of remote-ops-audit W11 `#287 bulk-reads`: Discord answers an empty history to a
    /// bot without Read Message History. That proves no id absent: two ids fail with Discord's
    /// refusal exactly as one id alone does, rather than come back absent.
    #[tokio::test]
    async fn an_empty_history_proves_no_id_absent() {
        for thread_id in [Some("201"), None] {
            let physical = thread_id.unwrap_or("100");
            let mut routes = Replies::new();
            ok(&mut routes, "/channels/201", thread("201", "903", "100"));
            ok(
                &mut routes,
                &format!("/channels/{physical}/messages?limit=100&after=901"),
                json!([]),
            );
            for id in ["902", "903"] {
                routes.insert(
                    format!("GET /channels/{physical}/messages/{id}"),
                    (
                        StatusCode::FORBIDDEN,
                        json!({"message": "Missing Access", "code": 50001}),
                    ),
                );
            }
            let discord = mock(routes, ThreadApi::Native).await;
            let channel = ChannelId("100".into());
            for asked in [vec!["902"], vec!["902", "903"]] {
                let error = discord
                    .client
                    .fetch_messages(&channel, thread_id, &lookup_ids(&asked))
                    .await
                    .expect_err("an empty history proves nothing");
                assert!(
                    matches!(error.cause(), ChatError::Status { status: 403, .. }),
                    "{asked:?} in {thread_id:?}: {error}"
                );
            }
        }
    }

    #[tokio::test]
    async fn native_lookups_ask_for_an_id_past_a_full_page_on_its_own() {
        let mut routes = Replies::new();
        ok(&mut routes, "/channels/201", thread("201", "6000", "100"));
        let page: Vec<Value> = (1000..1100)
            .rev()
            .map(|n| message("201", &n.to_string(), "near"))
            .collect();
        ok(
            &mut routes,
            "/channels/201/messages?limit=100&after=999",
            Value::Array(page),
        );
        routes.insert(
            "GET /channels/201/messages/5000".into(),
            unknown_message_reply(),
        );
        ok(
            &mut routes,
            "/channels/201/messages/6000",
            message("201", "6000", "far"),
        );
        let paged = mock(routes, ThreadApi::Native).await;
        let channel = ChannelId("100".into());
        let answers = paged
            .client
            .fetch_messages(
                &channel,
                Some("201"),
                &lookup_ids(&["1000", "1050", "5000", "6000"]),
            )
            .await
            .expect("lookup");
        assert_eq!(
            answers.iter().map(outcome).collect::<Vec<_>>(),
            ["found 1000", "found 1050", "absent", "found 6000"],
            "only Unknown Message proves an id past the page absent"
        );
        assert_eq!(paged.seen.lock().expect("seen").len(), 4);

        // Any other 404 proves nothing, and fails the lookup as a lookup of one would.
        let mut routes = Replies::new();
        ok(&mut routes, "/channels/201", thread("201", "6000", "100"));
        routes.insert(
            "GET /channels/201/messages/5000".into(),
            (
                StatusCode::NOT_FOUND,
                json!({"message": "Unknown Channel", "code": 10003}),
            ),
        );
        let refusing = mock(routes, ThreadApi::Native).await;
        let error = refusing
            .client
            .fetch_messages(&channel, Some("201"), &lookup_ids(&["5000"]))
            .await
            .expect_err("not proved absent");
        assert!(matches!(
            error.cause(),
            ChatError::Status { status: 404, .. }
        ));
    }

    #[tokio::test]
    async fn native_lookups_of_the_channels_own_messages_cost_what_one_lookup_did() {
        let mut routes = Replies::new();
        ok(
            &mut routes,
            "/channels/100/messages/850",
            message("100", "850", "one"),
        );
        routes.insert(
            "GET /channels/100/messages/860".into(),
            unknown_message_reply(),
        );
        ok(
            &mut routes,
            "/channels/100/messages?limit=100&after=849",
            json!([message("100", "852", "three"), message("100", "850", "one")]),
        );
        let mock = mock(routes, ThreadApi::Native).await;
        let channel = ChannelId("100".into());
        for (asked, expected) in [
            (vec!["850"], vec!["found 850"]),
            (vec!["860"], vec!["absent"]),
            (
                vec!["852", "850", "851"],
                vec!["found 852", "found 850", "absent"],
            ),
        ] {
            let answers = mock
                .client
                .fetch_messages(&channel, None, &lookup_ids(&asked))
                .await
                .expect("lookup");
            assert_eq!(answers.iter().map(outcome).collect::<Vec<_>>(), expected);
        }
        assert_eq!(
            *mock.seen.lock().expect("seen"),
            [
                "GET /channels/100/messages/850",
                "GET /channels/100/messages/860",
                "GET /channels/100/messages?limit=100&after=849"
            ]
        );
    }

    #[tokio::test]
    async fn a_threads_root_is_never_proved_absent_by_a_page_of_its_replies() {
        let mut routes = inventory_routes();
        ok(
            &mut routes,
            "/channels/201/messages?limit=100&after=200",
            json!([message("201", "902", "reply")]),
        );
        let mock = mock(routes, ThreadApi::Native).await;
        let answers = mock
            .client
            .fetch_messages(
                &ChannelId("100".into()),
                Some("201"),
                &lookup_ids(&["201", "902"]),
            )
            .await
            .expect("lookup");
        assert_eq!(
            answers.iter().map(outcome).collect::<Vec<_>>(),
            ["found 201", "found 902"]
        );
        let crate::chat::Lookup::Found(root) = &answers[0] else {
            unreachable!()
        };
        assert!(root.thread.as_ref().expect("membership").is_root);
        assert_eq!(root.content, "first root", "read from the parent channel");
    }

    #[tokio::test]
    async fn a_bridge_looks_messages_up_one_at_a_time_as_before() {
        let mut routes = Replies::new();
        let mut wire = message("parent", "message-id", "in a thread");
        wire["thread"] = json!({"id":"opaque/thread","root_message_id":"root-id","is_root":false,"reply_count":1,"reply_count_exact":true});
        ok(
            &mut routes,
            "/channels/parent/threads/opaque%2Fthread/messages/message-id",
            wire,
        );
        let mock = mock(routes, ThreadApi::Bridge).await;
        let channel = ChannelId("parent".into());
        assert_eq!(
            mock.client.read_caps(&channel).by_ids,
            crate::chat::Support::Emulated
        );
        let answers = mock
            .client
            .fetch_messages(
                &channel,
                Some("opaque/thread"),
                &lookup_ids(&["message-id", "message-id"]),
            )
            .await
            .expect("lookup");
        assert_eq!(
            answers.iter().map(outcome).collect::<Vec<_>>(),
            ["found message-id", "found message-id"]
        );
        assert_eq!(mock.seen.lock().expect("seen").len(), 2, "one each");
    }

    fn in_thread(id: &str, thread: &str) -> Value {
        let mut wire = message("parent", id, &format!("text of {id}"));
        wire["thread"] = json!({"id":thread,"root_message_id":"root","is_root":false,"reply_count":null,"reply_count_exact":false});
        wire
    }

    async fn id_reading_bridge(routes: Replies) -> Mock {
        mock_with(routes, |discord| {
            discord.thread_api = ThreadApi::Bridge;
            discord.id_reads = true;
        })
        .await
    }

    #[tokio::test]
    async fn a_bridge_with_id_reads_asks_for_every_id_in_one_request() {
        let mut routes = Replies::new();
        ok(
            &mut routes,
            "/channels/parent/messages?ids=m%2F1&ids=m%2F2&ids=m%2C3&thread=opaque%2Fthread",
            json!({
                "messages": [in_thread("m/1", "opaque/thread"), in_thread("m,3", "opaque/thread")],
                "missing": ["m/2"],
            }),
        );
        let mock = id_reading_bridge(routes).await;
        let channel = ChannelId("parent".into());
        assert_eq!(
            mock.client.read_caps(&channel).by_ids,
            crate::chat::Support::Native
        );
        let answers = mock
            .client
            .fetch_messages(
                &channel,
                Some("opaque/thread"),
                &lookup_ids(&["m/1", "m/2", "m,3", "m/1"]),
            )
            .await
            .expect("lookup");
        assert_eq!(
            answers.iter().map(outcome).collect::<Vec<_>>(),
            ["found m/1", "absent", "found m,3", "found m/1"]
        );
        assert_eq!(mock.seen.lock().expect("seen").len(), 1, "one request");
    }

    #[tokio::test]
    async fn a_bridge_lookup_of_many_ids_is_a_request_per_fifty() {
        let ids: Vec<String> = (0..120).map(|n| format!("m{n}")).collect();
        let mut routes = Replies::new();
        for chunk in ids.chunks(super::super::BRIDGE_LOOKUP_IDS) {
            let query: Vec<String> = chunk.iter().map(|id| format!("ids={id}")).collect();
            ok(
                &mut routes,
                &format!("/channels/parent/messages?{}", query.join("&")),
                json!({"messages": [], "missing": chunk}),
            );
        }
        let mock = id_reading_bridge(routes).await;
        let asked: Vec<&str> = ids.iter().map(String::as_str).collect();
        let answers = mock
            .client
            .fetch_messages(&ChannelId("parent".into()), None, &lookup_ids(&asked))
            .await
            .expect("lookup");
        assert!(answers
            .iter()
            .all(|answer| *answer == crate::chat::Lookup::Absent));
        assert_eq!(mock.seen.lock().expect("seen").len(), 3, "50, 50 and 20");
    }

    /// Fix round 1 of remote-ops-audit W11 `#287 bulk-reads`: a bridge configured with `id_reads`
    /// whose adapter does not serve them answers `404`, `405` or `501`, or the plain message list.
    /// The lookup is then made as it is with the setting off, one thread lookup per id, rather
    /// than fail; and the bridge is not asked for one again while that is remembered.
    #[tokio::test]
    async fn a_bridge_that_does_not_serve_lookups_by_id_is_looked_up_as_before() {
        let channel = ChannelId("parent".into());
        let ids_route = "GET /channels/parent/messages?ids=m1&ids=m2&thread=t1";
        let mut routes = Replies::new();
        ok(
            &mut routes,
            "/channels/parent/threads/t1/messages/m1",
            in_thread("m1", "t1"),
        );
        ok(
            &mut routes,
            "/channels/parent/threads/t1/messages/m2",
            in_thread("m2", "t1"),
        );
        let off = mock_with(routes.clone(), |discord| {
            discord.thread_api = ThreadApi::Bridge;
        })
        .await;
        let today = off
            .client
            .fetch_messages(&channel, Some("t1"), &lookup_ids(&["m1", "m2"]))
            .await
            .expect("lookup");
        assert_eq!(
            today.iter().map(outcome).collect::<Vec<_>>(),
            ["found m1", "found m2"]
        );

        for refusal in [
            (StatusCode::NOT_FOUND, json!({"detail": "Not Found"})),
            (StatusCode::METHOD_NOT_ALLOWED, json!({})),
            (StatusCode::NOT_IMPLEMENTED, json!({})),
            (StatusCode::OK, json!([in_thread("m9", "t1")])),
        ] {
            let mut routes = routes.clone();
            routes.insert(ids_route.into(), refusal.clone());
            let on = id_reading_bridge(routes).await;
            assert_eq!(
                on.client.read_caps(&channel).by_ids,
                crate::chat::Support::Native
            );
            let answers = on
                .client
                .fetch_messages(&channel, Some("t1"), &lookup_ids(&["m1", "m2"]))
                .await
                .expect("looked up as before");
            assert_eq!(answers, today, "{refusal:?}");
            let mut asked = taken(&on);
            asked.sort();
            assert_eq!(
                asked,
                [
                    ids_route,
                    "GET /channels/parent/threads/t1/messages/m1",
                    "GET /channels/parent/threads/t1/messages/m2"
                ],
                "{refusal:?}"
            );
            assert_eq!(
                on.client.read_caps(&channel).by_ids,
                crate::chat::Support::Emulated,
                "{refusal:?}: remembered, so callers look ids up as they did without the setting"
            );
            on.client
                .fetch_messages(&channel, Some("t1"), &lookup_ids(&["m1"]))
                .await
                .expect("lookup");
            assert_eq!(
                taken(&on),
                ["GET /channels/parent/threads/t1/messages/m1"],
                "{refusal:?}: not asked again"
            );
            // Once the memory lapses, the bridge is asked again.
            on.client.unserved.forget();
            assert_eq!(
                on.client.read_caps(&channel).by_ids,
                crate::chat::Support::Native
            );
        }

        // A thread the bridge does not know fails as the lookup it replaced does, and proves
        // nothing about the bridge: nothing is remembered.
        let mut routes = Replies::new();
        routes.insert(
            ids_route.into(),
            (StatusCode::NOT_FOUND, json!({"detail": "Not Found"})),
        );
        let on = id_reading_bridge(routes).await;
        let error = on
            .client
            .fetch_messages(&channel, Some("t1"), &lookup_ids(&["m1", "m2"]))
            .await
            .expect_err("the thread lookup fails");
        assert!(
            matches!(error.cause(), ChatError::Status { status: 404, .. }),
            "{error}"
        );
        assert_eq!(
            on.client.read_caps(&channel).by_ids,
            crate::chat::Support::Native
        );
    }

    #[test]
    fn a_bridges_lookup_answer_is_read_strictly() {
        let channel = ChannelId("parent".into());
        let asked = lookup_ids(&["a", "b"]);
        let read = |value: Value, thread: Option<&str>| {
            super::super::parse_id_lookup(&value, &channel, thread, &asked)
        };
        let refused = |value: Value, thread: Option<&str>, expected: &str| {
            let error = read(value, thread).expect_err(expected);
            assert!(
                matches!(&error, ChatError::Shape(detail) if detail.contains(expected)),
                "{error}"
            );
        };
        refused(json!([in_thread("a", "t")]), None, "plain message list");
        refused(json!({"messages": []}), None, "missing must be an array");
        refused(json!({"missing": []}), None, "messages must be an array");
        refused(
            json!({"messages": [in_thread("c", "t")], "missing": []}),
            None,
            "not asked for",
        );
        refused(
            json!({"messages": [in_thread("a", "u")], "missing": []}),
            Some("t"),
            "different thread",
        );
        let mut elsewhere = in_thread("a", "t");
        elsewhere["channel_id"] = json!("other");
        refused(
            json!({"messages": [elsewhere], "missing": []}),
            None,
            "different channel",
        );
        refused(
            json!({"messages": [in_thread("a", "t"), in_thread("a", "t")], "missing": []}),
            None,
            "answered twice",
        );
        refused(
            json!({"messages": [], "missing": ["z"]}),
            None,
            "not asked for",
        );
        refused(
            json!({"messages": [in_thread("a", "t")], "missing": ["a"]}),
            None,
            "both answered and missing",
        );
        refused(
            json!({"messages": [], "missing": [7]}),
            None,
            "array of ids",
        );
        refused(
            json!({"messages": [], "missing": [], "as_of": 5}),
            None,
            "as_of must be a string",
        );

        let current = read(
            json!({"messages": [in_thread("a", "t")], "missing": ["b"]}),
            Some("t"),
        )
        .expect("read");
        assert_eq!(
            current
                .iter()
                .map(|(_, answer)| outcome(answer))
                .collect::<Vec<_>>(),
            ["found a", "absent"]
        );
        // A copy older than now proves neither the text nor the absence.
        let stale = read(
            json!({"messages": [in_thread("a", "t")], "missing": ["b"], "as_of": "2026-10-13T12:00:00Z"}),
            Some("t"),
        )
        .expect("read");
        assert_eq!(
            stale
                .iter()
                .map(|(_, answer)| outcome(answer))
                .collect::<Vec<_>>(),
            ["unseen", "unseen"]
        );
    }

    fn taken(mock: &Mock) -> Vec<String> {
        std::mem::take(&mut *mock.seen.lock().expect("seen"))
    }

    /// remote-ops-audit W14: several threads of a native channel are each read as the thread
    /// view's newest page is, answer for answer, but the parent is read once for all of them, so
    /// two threads cost seven requests where two thread view reads cost eight.
    #[tokio::test]
    async fn native_thread_reads_share_the_parent_and_answer_as_the_thread_view_does() {
        let mut routes = inventory_routes();
        ok(
            &mut routes,
            "/channels/201/messages?limit=3",
            json!([
                message("201", "901", "new reply"),
                message("201", "701", "old reply"),
            ]),
        );
        ok(
            &mut routes,
            "/channels/301/messages?limit=3",
            json!([message("301", "801", "only reply")]),
        );
        ok(&mut routes, "/channels/999", thread("999", "9999", "9990"));
        let mock = mock(routes, ThreadApi::Native).await;
        let channel = ChannelId("100".into());
        assert_eq!(
            mock.client.read_caps(&channel).many_threads,
            crate::chat::Support::Native
        );
        let asks = [
            crate::chat::ThreadAsk::newest("201", 2),
            crate::chat::ThreadAsk::newest("301", 2),
            crate::chat::ThreadAsk::newest("999", 2),
        ];
        let answers = mock.client.fetch_threads(&channel, &asks).await;
        let mut requests = taken(&mock);
        requests.sort();
        assert_eq!(
            requests,
            [
                "GET /channels/100",
                "GET /channels/100/messages/201",
                "GET /channels/100/messages/301",
                "GET /channels/201",
                "GET /channels/201/messages?limit=3",
                "GET /channels/301",
                "GET /channels/301/messages?limit=3",
                "GET /channels/999",
            ],
            "the parent once; each thread checked, its page and its root; nothing more of a \
             thread that is not the channel's"
        );
        assert!(
            matches!(answers[2].1, Err(ref error) if matches!(error.cause(), ChatError::Refused(_))),
            "a thread of another channel is refused in its own place: {:?}",
            answers[2].1
        );
        for (at, id) in [(0, "201"), (1, "301")] {
            let request = TimelineRequest {
                view: TimelineView::Thread,
                thread_id: Some(id.into()),
                limit: 2,
                ..TimelineRequest::default()
            };
            let viewed = mock
                .client
                .fetch_timeline(&channel, &request)
                .await
                .expect("the thread view");
            assert_eq!(
                answers[at].1.as_ref().expect("a page"),
                &viewed,
                "{id}: exactly the thread view's newest page"
            );
            assert_eq!(answers[at].0, id);
        }
        assert_eq!(
            taken(&mock).len(),
            8,
            "the two thread view reads it replaces cost four requests each"
        );
    }

    /// remote-ops-audit W14: asked for what is newer than an instant, a thread whose
    /// `last_message_id` has not moved past it costs its membership check alone; one that moved is
    /// read `after=` the instant's snowflake; a root is read only when it is itself newer. Every
    /// answer is a delta of additions, never a whole thread.
    #[tokio::test]
    async fn native_thread_reads_after_an_instant_read_only_the_threads_that_moved() {
        const BASE_MS: i64 = 1_787_000_000_000;
        let at = |ms: i64| MessageId::at_time_ms(ms).numeric().expect("a snowflake");
        let id = |ms: i64| at(ms).to_string();
        let newer_than_ms = BASE_MS + 10_000;
        let last_at = at(newer_than_ms) | ((1 << 22) - 1);
        let (quiet, moved, fresh) = (id(BASE_MS), id(BASE_MS + 1_000), id(BASE_MS + 12_000));
        let mut routes = Replies::new();
        ok(
            &mut routes,
            &format!("/channels/{quiet}"),
            thread(&quiet, &id(BASE_MS + 5_000), "100"),
        );
        ok(
            &mut routes,
            &format!("/channels/{moved}"),
            thread(&moved, &id(BASE_MS + 20_000), "100"),
        );
        ok(
            &mut routes,
            &format!("/channels/{fresh}"),
            thread(&fresh, &id(BASE_MS + 13_000), "100"),
        );
        ok(
            &mut routes,
            &format!("/channels/{moved}/messages?limit=51&after={last_at}"),
            json!([
                message(&moved, &id(BASE_MS + 20_000), "newest"),
                message(&moved, &id(BASE_MS + 15_000), "newer"),
            ]),
        );
        let root = message("100", &fresh, "a root newer than the instant");
        let mut starter = message(&fresh, &fresh, "");
        starter["type"] = json!(21);
        starter["referenced_message"] = root.clone();
        ok(
            &mut routes,
            &format!("/channels/{fresh}/messages?limit=51&after={last_at}"),
            json!([
                message(&fresh, &id(BASE_MS + 13_000), "first reply"),
                starter
            ]),
        );
        ok(
            &mut routes,
            "/channels/100",
            json!({"id":"100","type":0,"guild_id":"9"}),
        );
        ok(
            &mut routes,
            &format!("/channels/100/messages/{fresh}"),
            root,
        );
        let mock = mock(routes, ThreadApi::Native).await;
        let channel = ChannelId("100".into());
        let ask = |thread: &str| crate::chat::ThreadAsk {
            id: thread.to_owned(),
            newer_than_ms: Some(newer_than_ms),
            limit: 50,
        };
        let answers = mock
            .client
            .fetch_threads(&channel, &[ask(&quiet), ask(&moved), ask(&fresh)])
            .await;
        let mut requests = taken(&mock);
        requests.sort();
        let mut expected = vec![
            format!("GET /channels/{quiet}"),
            format!("GET /channels/{moved}"),
            format!("GET /channels/{moved}/messages?limit=51&after={last_at}"),
            format!("GET /channels/{fresh}"),
            format!("GET /channels/{fresh}/messages?limit=51&after={last_at}"),
            "GET /channels/100".to_owned(),
            format!("GET /channels/100/messages/{fresh}"),
        ];
        expected.sort();
        assert_eq!(requests, expected, "the quiet thread costs one request");
        let contents = |at: usize| -> Vec<String> {
            answers[at]
                .1
                .as_ref()
                .expect("a page")
                .messages
                .iter()
                .map(|m| m.content.clone())
                .collect()
        };
        assert!(contents(0).is_empty());
        assert_eq!(contents(1), ["newer", "newest"]);
        assert_eq!(
            contents(2),
            ["a root newer than the instant", "first reply"]
        );
        for (at, root) in [(0, false), (1, false), (2, true)] {
            let page = answers[at].1.as_ref().expect("a page");
            assert_eq!(
                page.delta,
                Some(TimelineDelta {
                    more: false,
                    complete: false,
                    deleted: Vec::new(),
                    removed_threads: Vec::new(),
                }),
                "{at}: additions only"
            );
            assert!(!page.has_more && page.next_before.is_none() && page.next_after.is_none());
            let summary = page.thread.as_ref().expect("a summary");
            assert_eq!(
                summary.root.is_some(),
                root,
                "{at}: a root only when it is news"
            );
        }
    }

    /// Fix round 3 of remote-ops-audit W14 `#287 bulk-reads`: a thread object that leaves
    /// `last_message_id` out says nothing about what was added to the thread, so asked for what is
    /// newer than an instant it is read `after=` the instant, as a thread that moved. Only
    /// `last_message_id: null` says no message was ever sent there.
    #[tokio::test]
    async fn native_thread_reads_after_an_instant_read_a_thread_that_does_not_say_its_last_message()
    {
        const BASE_MS: i64 = 1_787_000_000_000;
        let at = |ms: i64| MessageId::at_time_ms(ms).numeric().expect("a snowflake");
        let id = |ms: i64| at(ms).to_string();
        let newer_than_ms = BASE_MS + 10_000;
        let last_at = at(newer_than_ms) | ((1 << 22) - 1);
        let (unsaid, empty) = (id(BASE_MS), id(BASE_MS + 1_000));
        let mut unsaid_thread = thread(&unsaid, &unsaid, "100");
        unsaid_thread
            .as_object_mut()
            .expect("an object")
            .remove("last_message_id");
        let mut empty_thread = thread(&empty, &empty, "100");
        empty_thread["last_message_id"] = Value::Null;
        let mut routes = Replies::new();
        ok(&mut routes, &format!("/channels/{unsaid}"), unsaid_thread);
        ok(&mut routes, &format!("/channels/{empty}"), empty_thread);
        ok(
            &mut routes,
            &format!("/channels/{unsaid}/messages?limit=51&after={last_at}"),
            json!([message(&unsaid, &id(BASE_MS + 20_000), "added since")]),
        );
        let mock = mock(routes, ThreadApi::Native).await;
        let channel = ChannelId("100".into());
        let ask = |thread: &str| crate::chat::ThreadAsk {
            id: thread.to_owned(),
            newer_than_ms: Some(newer_than_ms),
            limit: 50,
        };
        let answers = mock
            .client
            .fetch_threads(&channel, &[ask(&unsaid), ask(&empty)])
            .await;
        let mut requests = taken(&mock);
        requests.sort();
        let mut expected = vec![
            format!("GET /channels/{unsaid}"),
            format!("GET /channels/{unsaid}/messages?limit=51&after={last_at}"),
            format!("GET /channels/{empty}"),
        ];
        expected.sort();
        assert_eq!(
            requests, expected,
            "only the thread that says it has no messages is not read"
        );
        let contents = |at: usize| -> Vec<String> {
            answers[at]
                .1
                .as_ref()
                .expect("a page")
                .messages
                .iter()
                .map(|m| m.content.clone())
                .collect()
        };
        assert_eq!(contents(0), ["added since"]);
        assert!(contents(1).is_empty());
    }
}
