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
    MessageThread, ThreadApi, ThreadSummary, TimelinePage, TimelineRequest, TimelineView,
};

const MAX_THREADS: usize = 256;
const MAX_ARCHIVE_PAGES: usize = 100;
const SNAPSHOT_TTL: Duration = Duration::from_secs(300);
const MAX_SNAPSHOTS: usize = 16;
const TIMELINE_TIMEOUT: Duration = Duration::from_secs(90);
const THREAD_CONTAINER_NOTICE: &str =
    "This channel contains threads. Open a thread to post a reply.";

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

#[derive(Clone, Debug)]
struct NativeThread {
    id: String,
    parent: String,
    title: String,
    last_message: u64,
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
    if request.limit == 0
        || request
            .before
            .as_ref()
            .is_some_and(|cursor| cursor.len() > 8192)
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
                let value = self
                    .send_with_timeout(
                        self.thread_request(
                            "GET",
                            &["channels", channel.as_str(), "timeline"],
                            &query,
                        )?,
                        TIMELINE_TIMEOUT,
                    )
                    .await?;
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
        limit: u16,
    ) -> Result<Vec<Message>, ChatError> {
        let physical = thread.map_or(channel.as_str(), |thread| thread.id.as_str());
        let cursor = before.map(|value| MessageId(value.to_owned()));
        let value = self
            .send(page_request(
                &self.api_base,
                &ChannelId(physical.to_owned()),
                limit,
                cursor.as_ref(),
                None,
            )?)
            .await?;
        let raw = value
            .as_array()
            .ok_or_else(|| shape("message history is not an array"))?;
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
        Ok(messages)
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
        let cursor = request
            .before
            .as_deref()
            .map(|before| NativeCursor::decode(before, channel, request))
            .transpose()?;
        let limit = usize::from(request.limit.clamp(1, 99));
        if request.view == TimelineView::Thread {
            let id = request.thread_id.as_deref().expect("validated thread id");
            let thread = self.verified_thread(channel, id).await?;
            let before = cursor.as_ref().map(|cursor| cursor.boundary.as_str());
            if let Some(before) = before {
                native_id(before)?;
            }
            let mut messages = self
                .native_messages(channel, Some(&thread), before, (limit + 1) as u16)
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
                self.native_messages(channel, thread.as_ref(), before, (limit + 1) as u16)
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
        }
    }

    pub(super) async fn thread_post(
        &self,
        channel: &ChannelId,
        id: &str,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        match self.thread_api {
            ThreadApi::Off => Err(refused("thread posting is disabled on this backend")),
            ThreadApi::Bridge => {
                let nonce = fresh_post_nonce();
                let mut request = post_request_with_nonce(
                    &self.api_base,
                    channel,
                    content,
                    reply_to,
                    Some(&nonce),
                )?;
                request.url = self
                    .thread_request(
                        "POST",
                        &["channels", channel.as_str(), "threads", id, "messages"],
                        &[],
                    )?
                    .url;
                let value = self.send_with_timeout(request, TIMELINE_TIMEOUT).await?;
                let message = parse_message(&value)?;
                validate_bridge_message(&message, channel, Some(id))?;
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

fn parse_bridge_page(
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
        config.discord.api_base = format!("http://{address}");
        config.discord.thread_api = mode;
        config.discord.provider_name = "Example Chat".to_owned();
        let client = HttpDiscordClient::new(&config.discord).expect("client");
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
}
