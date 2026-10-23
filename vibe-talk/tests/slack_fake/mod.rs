//! A loopback fake of the Slack Web API subset `vibe_talk::slack` uses.
//!
//! Deterministic data, a configurable page cap (real servers may answer with fewer messages than
//! asked for), per-method call counters, injectable `ok:false` errors and a one-shot 429. It
//! implements exactly the nine methods the client is allowed to call and answers
//! `unknown_method` for anything else, so a test fails if the client strays outside the subset.
//!
//! `conversations.mark` behaves as Slack's does: it moves the read cursor wherever it is told,
//! backwards too, so a test can see whether the CLIENT kept it from going back.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use serde_json::{json, Value};
use vibe_talk::config::Secret;
use vibe_talk::slack::{HttpSlackClient, SlackConfig};

/// The token every fake accepts.
pub const TOKEN: &str = "xoxb-fake-token-0123456789-abcdefghij";
/// The account the fake's `auth.test` reports.
pub const SELF_USER: &str = "UBOTSELF1";
/// The workspace the fake's `auth.test` reports, unless [`FakeState::team`] says another.
pub const TEAM: &str = "T0000TEAM";
/// The first instant the fixtures use: 2023-11-14T22:13:20Z.
pub const BASE_SECS: i64 = 1_700_000_000;

/// The methods the fake implements, which are the only ones the client may use.
pub const METHODS: [&str; 9] = [
    "auth.test",
    "conversations.info",
    "conversations.history",
    "conversations.replies",
    "chat.postMessage",
    "users.info",
    "users.conversations",
    "conversations.mark",
    "conversations.members",
];

/// Everything the fake knows and records.
#[derive(Default)]
pub struct FakeState {
    /// Most messages or conversations one page returns, whatever `limit` asked for.
    pub page_cap: usize,
    /// A conversation's own timeline: roots, ordinary messages, broadcasts and system events.
    pub history: HashMap<String, Vec<Value>>,
    /// Replies of one thread, keyed by `(conversation, root ts)`, root excluded.
    pub replies: HashMap<(String, String), Vec<Value>>,
    /// `users.info` answers by id.
    pub users: HashMap<String, Value>,
    /// What `users.conversations` lists, in order.
    pub memberships: Vec<Value>,
    /// `conversations.members` answers by conversation id, in order. A conversation with none here
    /// answers `channel_not_found`. `#317 slack-group-dm-names`.
    pub members: HashMap<String, Vec<String>>,
    /// Calls per method.
    pub calls: HashMap<String, usize>,
    /// Every request: method and decoded form parameters.
    pub requests: Vec<(String, HashMap<String, String>)>,
    /// Method → Slack `error` answered with `ok:false`, every time.
    pub fail: HashMap<String, String>,
    /// Thread root `ts` → HTTP status answered to every `conversations.replies` request for that
    /// thread, as a bridge answers when its command fails on one thread's content.
    pub fail_threads: HashMap<String, u16>,
    /// Method → an HTTP status and a plain body that is no Slack answer at all, every time: what a
    /// shim route that is gone, an intermediary refusing the destination or a wrong address gives.
    pub http_fail: HashMap<String, (u16, String)>,
    /// One 429 for this method, with this `Retry-After` in seconds.
    pub rate_limit_once: Option<(String, u64)>,
    /// Answer every request with HTTP 500 and a body that echoes the token.
    pub echo_token_500: bool,
    /// Include the thread root on EVERY `conversations.replies` page, as some servers do.
    pub root_always: bool,
    /// Ignore `oldest` on `conversations.replies`, as a lenient server may: every read of a thread
    /// starts at its beginning.
    pub replies_ignore_oldest: bool,
    /// Answer `conversations.replies` with neither `has_more` nor a cursor, as a server that pages
    /// nothing may, however much it cut the page short.
    pub replies_omit_has_more: bool,
    /// The next posted message's microsecond offset past the newest fixture.
    pub post_counter: i64,
    /// The account's read cursor per conversation, as `conversations.info` reports `last_read`.
    /// A conversation with none here answers without `last_read`, as an older bridge does.
    pub last_read: HashMap<String, String>,
    /// `conversations.info` reports this conversation with `is_member: false`.
    pub not_member: Vec<String>,
    /// The `name` `conversations.info` reports for a conversation; `general` for one not here.
    /// `#318 slack-mention-names`.
    pub conversation_names: HashMap<String, String>,
    /// Every `conversations.mark`, in order: conversation and `ts`.
    pub marks: Vec<(String, String)>,
    /// `chat.postMessage` calls, numbered from 1, answered with HTTP 502, as a gateway that timed
    /// out waiting for Slack would. Nothing is posted for them.
    pub fail_posts: Vec<usize>,
    /// Answer the next `conversations.replies` request for one thread late: see [`Hold`].
    pub hold_replies: Option<Hold>,
    /// The handle `auth.test` reports for the token's account; `vibe-bot` when unset.
    /// `#325 provider-diagnostics-ids`.
    pub auth_user: Option<String>,
    /// The `team_id` `auth.test` reports instead of [`TEAM`]; an empty one reports none at all.
    pub team: Option<String>,
    /// How long every answer is held before it is sent: a bridge's own read latency, for a test
    /// that measures what an extra provider read costs. `#452 slack-live-ingest`.
    pub delay: Option<std::time::Duration>,
    /// How long every `chat.postMessage` takes before it is answered — and before it is posted —
    /// as a slow bridge in front of Slack does. `#448 send-dedupe`.
    pub post_delay: Option<std::time::Duration>,
    /// Drop `chat.postMessage`'s `metadata`, as a transport in front of Slack that does not pass
    /// it through would: the message is posted without it. `#465 replication-c6-op-key-marker`.
    pub drops_metadata: bool,
    /// `chat.postMessage` calls, numbered from 1, that ARE posted and then answered with HTTP 502,
    /// as a gateway that gave up after Slack took the post would. `#465
    /// replication-c6-op-key-marker`.
    pub lose_post_answers: Vec<usize>,
}

/// Take `metadata` off every message of a read answer that did not ask for it with
/// `include_all_metadata`, as Slack does.
fn strip_metadata(answer: &mut Value, params: &HashMap<String, String>) {
    if params.get("include_all_metadata").map(String::as_str) == Some("true") {
        return;
    }
    if let Some(messages) = answer.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages {
            if let Some(fields) = message.as_object_mut() {
                fields.remove("metadata");
            }
        }
    }
}

/// A `conversations.replies` request answered late, so a test can act while a read is in flight.
/// The answer is computed from the state when the request ARRIVES, as a server would, and sent
/// only once the test releases it. Only the first matching request is held.
#[derive(Clone)]
pub struct Hold {
    /// The thread, by root `ts`, whose next replies request is held.
    pub thread: String,
    /// Notified once that request has arrived and its answer has been computed.
    pub arrived: Arc<tokio::sync::Notify>,
    /// Notify this to send the answer.
    pub release: Arc<tokio::sync::Notify>,
}

impl Hold {
    /// Hold the next replies request for `thread`.
    pub fn new(thread: &str) -> Self {
        Self {
            thread: thread.to_owned(),
            arrived: Arc::new(tokio::sync::Notify::new()),
            release: Arc::new(tokio::sync::Notify::new()),
        }
    }
}

impl FakeState {
    fn count(&mut self, method: &str) {
        *self.calls.entry(method.to_owned()).or_default() += 1;
    }
}

/// A running fake and a client pointed at it.
pub struct Fake {
    pub state: Arc<Mutex<FakeState>>,
    pub base: String,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fake {
    /// Start a fake with `page_cap` and the given state.
    pub async fn start(mut state: FakeState, page_cap: usize) -> Self {
        state.page_cap = page_cap;
        let state = Arc::new(Mutex::new(state));
        let shared = Arc::clone(&state);
        let app = Router::new().fallback(move |uri: Uri, headers: HeaderMap, body: Bytes| {
            let shared = Arc::clone(&shared);
            async move {
                let hold = take_hold(&shared, &uri, &body);
                let delay = shared.lock().expect("state").delay;
                if let Some(delay) = delay {
                    tokio::time::sleep(delay).await;
                }
                if let Some(delay) = post_delay(&shared, &uri) {
                    tokio::time::sleep(delay).await;
                }
                let response = handle(&shared, &uri, &headers, &body);
                if let Some(hold) = hold {
                    hold.arrived.notify_one();
                    hold.release.notified().await;
                }
                response
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let base = format!("http://{}/api/", listener.local_addr().expect("address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        Self {
            state,
            base,
            server,
        }
    }

    /// The configuration a test client uses against this fake.
    pub fn config(&self) -> SlackConfig {
        SlackConfig {
            provider_name: "Slack".to_owned(),
            api_base: self.base.clone(),
            token: Secret::new(TOKEN),
            owner_user_id: Some("UOWNER001".to_owned()),
            request_timeout_seconds: 10,
            channel_registration: true,
            registered_channels_writable: true,
            channel_discovery: true,
            upstream_clear: false,
            max_post_chars: None,
            max_post_bytes: None,
            workspace_url: None,
        }
    }

    /// A client against this fake with clearing unread in Slack turned on.
    pub fn clearing_client(&self) -> HttpSlackClient {
        HttpSlackClient::new(&SlackConfig {
            upstream_clear: true,
            ..self.config()
        })
        .expect("client")
    }

    /// A client against this fake.
    pub fn client(&self) -> HttpSlackClient {
        HttpSlackClient::new(&self.config()).expect("client")
    }

    /// Calls made to `method` so far.
    pub fn calls(&self, method: &str) -> usize {
        self.state
            .lock()
            .expect("state")
            .calls
            .get(method)
            .copied()
            .unwrap_or(0)
    }

    /// Every request made to `method`, in order.
    pub fn requests(&self, method: &str) -> Vec<HashMap<String, String>> {
        self.state
            .lock()
            .expect("state")
            .requests
            .iter()
            .filter(|(name, _)| name == method)
            .map(|(_, params)| params.clone())
            .collect()
    }

    /// Run `f` against the state.
    pub fn with<T>(&self, f: impl FnOnce(&mut FakeState) -> T) -> T {
        f(&mut self.state.lock().expect("state"))
    }
}

/// `BASE_SECS + seconds`, with a microsecond fraction, as a canonical ts.
pub fn ts(seconds: i64, micros: i64) -> String {
    format!("{}.{micros:06}", BASE_SECS + seconds)
}

/// An ordinary message from a user whose profile is NOT carried, so a lookup is needed.
pub fn plain(ts: &str, user: &str, text: &str) -> Value {
    json!({"type": "message", "ts": ts, "user": user, "text": text})
}

/// A message carrying its author's profile.
pub fn profiled(ts: &str, user: &str, display: &str, text: &str) -> Value {
    json!({"type": "message", "ts": ts, "user": user, "text": text,
        "user_profile": {"display_name": display, "real_name": format!("{display} Real")}})
}

/// A file object as Slack describes one in a message's `files`: `id`, `name`, `pretty_type` and the
/// `permalink` of its page in Slack, plus the fields that ARE the file (`url_private`, a
/// thumbnail), which the client must never fetch, pointed at `file_host`.
/// `#313 slack-file-posts`.
pub fn file(id: &str, name: &str, pretty_type: &str, file_host: &str) -> Value {
    json!({"id": id, "created": BASE_SECS, "timestamp": BASE_SECS, "name": name, "title": name,
        "mimetype": "application/octet-stream", "filetype": pretty_type.to_ascii_lowercase(),
        "pretty_type": pretty_type, "user": "UADA00001", "mode": "hosted", "size": 1024,
        "url_private": format!("{file_host}files-pri/T0000TEAM-{id}/{name}"),
        "url_private_download": format!("{file_host}files-pri/T0000TEAM-{id}/download/{name}"),
        "thumb_360": format!("{file_host}files-tmb/T0000TEAM-{id}/{name}_360.png"),
        "permalink": format!("https://example.slack.com/files/UADA00001/{id}/{name}")})
}

/// A message sharing `files`, with `text` beside them (empty for a post that is only files), as
/// Slack's own client posts an upload: no subtype, `upload: true`. `#313 slack-file-posts`.
pub fn file_post(ts: &str, user: &str, display: &str, text: &str, files: Vec<Value>) -> Value {
    json!({"type": "message", "ts": ts, "user": user, "text": text, "upload": true,
        "files": files, "user_profile": {"display_name": display}})
}

/// A system event of `subtype`.
pub fn system(ts: &str, subtype: &str) -> Value {
    json!({"type": "message", "subtype": subtype, "ts": ts, "user": "USYSTEM01",
        "text": format!("<@USYSTEM01> did {subtype}")})
}

/// Turn `root` into a thread root with `replies` (each `(ts, text)`), registering them in `state`.
pub fn thread(
    state: &mut FakeState,
    conversation: &str,
    root: &mut Value,
    replies: &[(String, String)],
) {
    let root_ts = root["ts"].as_str().expect("root ts").to_owned();
    root["thread_ts"] = json!(root_ts);
    root["reply_count"] = json!(replies.len());
    if let Some((last, _)) = replies.last() {
        root["latest_reply"] = json!(last);
    }
    let list = replies
        .iter()
        .map(|(ts, text)| {
            json!({"type": "message", "ts": ts, "thread_ts": root_ts, "user": "UREPLIER1",
                "text": text, "user_profile": {"display_name": "Replier"}})
        })
        .collect();
    state
        .replies
        .insert((conversation.to_owned(), root_ts), list);
}

/// Post a reply in the existing thread rooted on `root` of `conversation`, as somebody else would,
/// and make its root say what Slack's then says: one more reply, the latest at `ts`.
pub fn add_reply(state: &mut FakeState, conversation: &str, root: &str, ts: &str, text: &str) {
    add_reply_message(
        state,
        conversation,
        root,
        json!({"type": "message", "ts": ts, "thread_ts": root, "user": "UREPLIER1",
            "text": text, "user_profile": {"display_name": "Replier"}}),
    );
}

/// [`add_reply`] with the whole reply given, such as one sharing a file. Its `thread_ts` is set to
/// `root`.
pub fn add_reply_message(state: &mut FakeState, conversation: &str, root: &str, mut reply: Value) {
    reply["thread_ts"] = json!(root);
    let replies = state
        .replies
        .entry((conversation.to_owned(), root.to_owned()))
        .or_default();
    replies.push(reply);
    let count = replies.len();
    let latest = replies
        .iter()
        .map(|reply| reply["ts"].as_str().expect("ts").to_owned())
        .max_by_key(|ts| micros(ts))
        .expect("a reply");
    let message = state
        .history
        .get_mut(conversation)
        .expect("conversation")
        .iter_mut()
        .find(|message| message["ts"] == json!(root))
        .expect("root");
    message["thread_ts"] = json!(root);
    message["reply_count"] = json!(count);
    message["latest_reply"] = json!(latest);
}

fn micros(ts: &str) -> i64 {
    let (secs, frac) = ts.split_once('.').unwrap_or((ts, "0"));
    let frac = format!("{frac:0<6}");
    secs.parse::<i64>().expect("secs") * 1_000_000 + frac[..6].parse::<i64>().expect("frac")
}

fn decode(component: &str) -> String {
    let bytes = component.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).expect("hex");
                out.push(u8::from_str_radix(hex, 16).expect("hex digit"));
                i += 2;
            }
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8(out).expect("utf8 form value")
}

fn form(body: &[u8]) -> HashMap<String, String> {
    std::str::from_utf8(body)
        .expect("utf8 body")
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(key), decode(value))
        })
        .collect()
}

fn ok(mut value: Value) -> Response {
    value["ok"] = json!(true);
    axum::Json(value).into_response()
}

fn err(error: &str) -> Response {
    axum::Json(json!({"ok": false, "error": error})).into_response()
}

/// Bounds and paging common to the two message feeds.
fn window(
    list: Vec<Value>,
    params: &HashMap<String, String>,
    cap: usize,
    newest_first: bool,
) -> Value {
    let inclusive = matches!(
        params.get("inclusive").map(String::as_str),
        Some("true" | "1")
    );
    let oldest = params.get("oldest").map(|ts| micros(ts));
    let latest = params.get("latest").map(|ts| micros(ts));
    let mut list: Vec<Value> = list
        .into_iter()
        .filter(|message| {
            let at = micros(message["ts"].as_str().expect("ts"));
            oldest.is_none_or(|bound| at > bound || (inclusive && at == bound))
                && latest.is_none_or(|bound| at < bound || (inclusive && at == bound))
        })
        .collect();
    list.sort_by_key(|message| micros(message["ts"].as_str().expect("ts")));
    if newest_first {
        list.reverse();
    }
    page(list, "messages", params, cap)
}

fn page(list: Vec<Value>, key: &str, params: &HashMap<String, String>, cap: usize) -> Value {
    let offset = params
        .get("cursor")
        .map(|cursor| {
            cursor
                .strip_prefix("off:")
                .and_then(|n| n.parse::<usize>().ok())
                .expect("a cursor this fake issued")
        })
        .unwrap_or(0);
    let limit = params
        .get("limit")
        .and_then(|limit| limit.parse::<usize>().ok())
        .unwrap_or(100)
        .min(cap)
        .max(1);
    let end = (offset + limit).min(list.len());
    let has_more = end < list.len();
    let slice: Vec<Value> = list.get(offset..end).unwrap_or_default().to_vec();
    let mut answer = json!({
        "has_more": has_more,
        "response_metadata": {"next_cursor": if has_more { format!("off:{end}") } else { String::new() }},
    });
    answer[key] = Value::Array(slice);
    answer
}

/// How long a `chat.postMessage` request waits before it is handled: [`FakeState::post_delay`].
fn post_delay(shared: &Mutex<FakeState>, uri: &Uri) -> Option<std::time::Duration> {
    if uri.path().trim_start_matches("/api/") != "chat.postMessage" {
        return None;
    }
    shared.lock().expect("state").post_delay
}

/// The [`Hold`] this request matches, taken so that it holds no later request.
fn take_hold(shared: &Mutex<FakeState>, uri: &Uri, body: &[u8]) -> Option<Hold> {
    if uri.path().trim_start_matches("/api/") != "conversations.replies" {
        return None;
    }
    let mut state = shared.lock().expect("state");
    let thread = form(body).get("ts").cloned();
    if state.hold_replies.as_ref().map(|hold| &hold.thread) == thread.as_ref() {
        state.hold_replies.take()
    } else {
        None
    }
}

fn handle(shared: &Mutex<FakeState>, uri: &Uri, headers: &HeaderMap, body: &[u8]) -> Response {
    let mut state = shared.lock().expect("state");
    let method = uri.path().trim_start_matches("/api/").to_owned();
    let params = form(body);
    state.count(&method);
    state.requests.push((method.clone(), params.clone()));
    if state.echo_token_500 {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("upstream refused Authorization: Bearer {TOKEN}"),
        )
            .into_response();
    }
    assert_eq!(
        headers
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/x-www-form-urlencoded"),
        "every call is a form POST"
    );
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(format!("Bearer {TOKEN}").as_str())
    {
        return err("invalid_auth");
    }
    if let Some((limited, seconds)) = state.rate_limit_once.clone() {
        if limited == method {
            state.rate_limit_once = None;
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [("retry-after", seconds.to_string())],
                axum::Json(json!({"ok": false, "error": "ratelimited"})),
            )
                .into_response();
        }
    }
    if let Some((status, body)) = state.http_fail.get(&method).cloned() {
        return (StatusCode::from_u16(status).expect("an HTTP status"), body).into_response();
    }
    if method == "chat.postMessage" && state.fail_posts.contains(&state.calls[&method]) {
        return (StatusCode::BAD_GATEWAY, "upstream send timed out").into_response();
    }
    if let Some(error) = state.fail.get(&method) {
        let mut answer = json!({"ok": false, "error": error});
        if error == "missing_scope" {
            answer["needed"] = json!("channels:history");
            answer["provided"] = json!("chat:write");
        }
        return axum::Json(answer).into_response();
    }
    if method == "conversations.replies" {
        if let Some(status) = params
            .get("ts")
            .and_then(|thread| state.fail_threads.get(thread))
        {
            let status = StatusCode::from_u16(*status).expect("an HTTP status");
            return (status, "the bridge could not read this thread").into_response();
        }
    }
    let cap = state.page_cap;
    let channel = params.get("channel").cloned().unwrap_or_default();
    match method.as_str() {
        "auth.test" => {
            let mut answer = json!({"user_id": SELF_USER,
                "user": state.auth_user.as_deref().unwrap_or("vibe-bot"), "team_id": TEAM,
                "team": "Example", "url": "https://example.slack.com/", "bot_id": "BBOTSELF1"});
            match state.team.as_deref() {
                Some("") => drop(answer.as_object_mut().expect("object").remove("team_id")),
                Some(team) => answer["team_id"] = json!(team),
                None => {}
            }
            ok(answer)
        }
        "conversations.info" => {
            if state.history.contains_key(&channel) {
                let name = state
                    .conversation_names
                    .get(&channel)
                    .map_or("general", String::as_str);
                let mut info = json!({"id": channel, "name": name, "is_archived": false});
                if let Some(cursor) = state.last_read.get(&channel) {
                    info["last_read"] = json!(cursor);
                }
                if state.not_member.contains(&channel) {
                    info["is_member"] = json!(false);
                }
                ok(json!({ "channel": info }))
            } else {
                err("channel_not_found")
            }
        }
        "conversations.mark" => {
            if !state.history.contains_key(&channel) {
                return err("channel_not_found");
            }
            let ts = params.get("ts").cloned().unwrap_or_default();
            // Slack's own behaviour: no comparison, backwards included.
            state.last_read.insert(channel.clone(), ts.clone());
            state.marks.push((channel, ts));
            ok(json!({}))
        }
        "conversations.history" => match state.history.get(&channel) {
            Some(list) => {
                let mut answer = window(list.clone(), &params, cap, true);
                strip_metadata(&mut answer, &params);
                ok(answer)
            }
            None => err("channel_not_found"),
        },
        "conversations.replies" => {
            let Some(list) = state.history.get(&channel) else {
                return err("channel_not_found");
            };
            let ts = params.get("ts").cloned().unwrap_or_default();
            let Some(root) = list
                .iter()
                .find(|message| message["ts"] == json!(ts))
                .cloned()
            else {
                return err("thread_not_found");
            };
            let replies = state
                .replies
                .get(&(channel.clone(), ts.clone()))
                .cloned()
                .unwrap_or_default();
            let unfiltered = !params.contains_key("oldest")
                && !params.contains_key("latest")
                && !params.contains_key("cursor");
            let mut all = vec![root.clone()];
            all.extend(replies);
            let mut bounds = params.clone();
            if state.replies_ignore_oldest {
                bounds.remove("oldest");
            }
            let mut answer = window(all, &bounds, cap, false);
            if state.replies_omit_has_more {
                let fields = answer.as_object_mut().expect("an object");
                fields.remove("has_more");
                fields.remove("response_metadata");
            }
            if state.root_always && !unfiltered {
                let messages = answer["messages"].as_array_mut().expect("messages");
                if messages.first() != Some(&root) {
                    messages.insert(0, root);
                }
            }
            strip_metadata(&mut answer, &params);
            ok(answer)
        }
        "chat.postMessage" => {
            if !state.history.contains_key(&channel) {
                return err("channel_not_found");
            }
            state.post_counter += 1;
            let newest = state
                .history
                .values()
                .flatten()
                .chain(state.replies.values().flatten())
                .map(|message| micros(message["ts"].as_str().expect("ts")))
                .max()
                .unwrap_or(BASE_SECS * 1_000_000);
            let at = newest + 1_000_000 + state.post_counter;
            let ts = format!("{}.{:06}", at / 1_000_000, at % 1_000_000);
            let mut message = json!({"type": "message", "ts": ts, "user": SELF_USER,
                "bot_id": "BBOTSELF1", "text": params.get("text").cloned().unwrap_or_default(),
                "bot_profile": {"name": "vibe-bot"}});
            if let Some(metadata) = params.get("metadata").filter(|_| !state.drops_metadata) {
                message["metadata"] = serde_json::from_str(metadata).expect("metadata is JSON");
            }
            match params.get("thread_ts") {
                Some(thread) => {
                    message["thread_ts"] = json!(thread);
                    state
                        .replies
                        .entry((channel.clone(), thread.clone()))
                        .or_default()
                        .push(message.clone());
                }
                None => state
                    .history
                    .get_mut(&channel)
                    .expect("checked")
                    .push(message.clone()),
            }
            if state
                .lose_post_answers
                .contains(&state.calls["chat.postMessage"])
            {
                return (StatusCode::BAD_GATEWAY, "upstream send timed out").into_response();
            }
            ok(json!({"channel": channel, "ts": ts, "message": message}))
        }
        "users.info" => {
            let user = params.get("user").cloned().unwrap_or_default();
            match state.users.get(&user) {
                Some(found) => ok(json!({"user": found})),
                None => err("user_not_found"),
            }
        }
        "conversations.members" => match state.members.get(&channel) {
            Some(members) => {
                let list = members.iter().map(|member| json!(member)).collect();
                ok(page(list, "members", &params, cap))
            }
            None => err("channel_not_found"),
        },
        "users.conversations" => {
            assert_eq!(
                params.get("types").map(String::as_str),
                Some("public_channel,private_channel,mpim,im")
            );
            assert_eq!(
                params.get("exclude_archived").map(String::as_str),
                Some("true")
            );
            ok(page(state.memberships.clone(), "channels", &params, cap))
        }
        _ => err("unknown_method"),
    }
}
