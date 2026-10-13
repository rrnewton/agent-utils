//! `#208 persistent-cache`, milestone `bridge-recent-flat`: a recent read a provider dates with
//! `as_of` says so on `/messages` and in the MCP text, and starts the held window then; a shadowed
//! `/messages` read is checked against the Flat view, and the differences are counted in
//! diagnostics. Off by default.
//!
//! The provider here wraps [`FakeDiscord`]: its recent read carries whatever `as_of` the test sets,
//! and its Flat view answers from the same messages with whatever the test changes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt as _;

use vibe_talk::auth::Scope;
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity, RecentRead};
use vibe_talk::config::{RecentReads, RecentView};
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::mcp::protocol::{dispatch, Outcome};
use vibe_talk::model::{ChannelId, Message, MessageId};
use vibe_talk::ops;
use vibe_talk::recent_view::{self, Tally};
use vibe_talk::state::AppState;
use vibe_talk::testing::{self, READ_CHANNEL, READ_TOKEN, WRITE_CHANNEL};
use vibe_talk::threads::{TimelinePage, TimelineRequest, TimelineView};

/// A bridge whose recent read and Flat view the test can make disagree.
struct Bridge {
    inner: Arc<FakeDiscord>,
    as_of: Mutex<Option<String>>,
    flat_text: Mutex<BTreeMap<MessageId, String>>,
    flat_missing: Mutex<BTreeSet<MessageId>>,
    recent_missing: Mutex<BTreeSet<MessageId>>,
    flat_reads: AtomicUsize,
}

impl Bridge {
    fn new(inner: Arc<FakeDiscord>) -> Self {
        Self {
            inner,
            as_of: Mutex::new(None),
            flat_text: Mutex::new(BTreeMap::new()),
            flat_missing: Mutex::new(BTreeSet::new()),
            recent_missing: Mutex::new(BTreeSet::new()),
            flat_reads: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl ChatClient for Bridge {
    fn provider_name(&self) -> &str {
        "Example Chat"
    }

    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        self.inner.identity().await
    }

    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError> {
        self.inner.fetch_page(channel, limit, before, after).await
    }

    async fn fetch_recent_dated(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<RecentRead, ChatError> {
        let missing = self.recent_missing.lock().expect("missing").clone();
        let mut messages = self.inner.fetch_recent(channel, limit).await?;
        messages.retain(|m| !missing.contains(&m.id));
        Ok(RecentRead {
            messages,
            as_of: self.as_of.lock().expect("as_of").clone(),
            exhausted: false,
        })
    }

    async fn fetch_timeline(
        &self,
        channel: &ChannelId,
        request: &TimelineRequest,
    ) -> Result<TimelinePage, ChatError> {
        assert_eq!(request.view, TimelineView::Flat, "only Flat is read here");
        assert!(request.before.is_none() && request.after.is_none());
        self.flat_reads.fetch_add(1, Ordering::SeqCst);
        let text = self.flat_text.lock().expect("text").clone();
        let missing = self.flat_missing.lock().expect("missing").clone();
        let messages = self
            .inner
            .fetch_recent(channel, request.limit)
            .await?
            .into_iter()
            .filter(|m| !missing.contains(&m.id))
            .map(|mut m| {
                if let Some(changed) = text.get(&m.id) {
                    m.content.clone_from(changed);
                }
                m
            })
            .collect();
        Ok(TimelinePage {
            messages,
            ..TimelinePage::default()
        })
    }

    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        self.inner.post_message(channel, content, reply_to).await
    }
}

struct Harness {
    state: AppState,
    fake: Arc<FakeDiscord>,
    bridge: Arc<Bridge>,
}

/// The test deployment, its `[discord]` section a bridge with `extra` settings.
fn harness(extra: &str) -> Harness {
    let toml = testing::config_toml().replace(
        "[discord]",
        &format!("[discord]\nthread_api = \"bridge\"\napi_base = \"http://127.0.0.1:9\"\n{extra}"),
    );
    let (mut state, fake, _elevenlabs) = testing::state_from_toml(&toml);
    for text in ["build is green", "deploy started", "deploy finished"] {
        fake.seed(&channel(READ_CHANNEL), "agent", text);
    }
    let bridge = Arc::new(Bridge::new(Arc::clone(&fake)));
    let client: Arc<dyn ChatClient> = bridge.clone();
    state.replace_chat(client);
    Harness {
        state,
        fake,
        bridge,
    }
}

fn channel(id: &str) -> ChannelId {
    ChannelId(id.to_owned())
}

fn minutes_ago(minutes: i64) -> String {
    vibe_talk::clock::iso_from_ms(ops::now_ms() - minutes * 60_000)
}

async fn get(state: &AppState, uri: &str) -> Value {
    let response = vibe_talk::http::router(state.clone())
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("authorization", format!("Bearer {READ_TOKEN}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("the route answers");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("JSON")
}

async fn tool(state: &AppState, name: &str, arguments: Value) -> String {
    let call = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    let outcome = dispatch(state, Scope::Read, &call).await;
    let Outcome::Reply(response) = outcome else {
        panic!("expected a reply, got {outcome:?}");
    };
    let result = response.result.as_ref().expect("a result");
    assert_eq!(result["isError"], false, "{result}");
    result["content"][0]["text"]
        .as_str()
        .expect("text")
        .to_owned()
}

/// A provider's `as_of` reaches every reader of the read: the `/messages` body, the digest the
/// agent reads, the window held for catch-ups — whose age then counts from `as_of` — and the
/// catch-up itself. A read without one says nothing.
#[tokio::test]
async fn a_read_the_provider_dates_says_as_of_on_messages_and_in_the_mcp_text() {
    let h = harness("");
    let as_of = minutes_ago(10);
    *h.bridge.as_of.lock().expect("as_of") = Some(as_of.clone());

    let body = get(
        &h.state,
        &format!("/api/v1/channels/{READ_CHANNEL}/messages"),
    )
    .await;
    assert_eq!(body["as_of"], json!(as_of), "{body}");
    assert_eq!(body["messages"].as_array().expect("messages").len(), 3);
    assert!(vibe_talk::contract::schema_document()["$defs"]["MessagesResponse"].is_object());

    let held = h
        .state
        .live
        .recent()
        .snapshot(&channel(READ_CHANNEL))
        .held
        .expect("the read was held");
    assert_eq!(held.as_of.as_deref(), Some(as_of.as_str()));
    assert!(held.age() >= Duration::from_secs(590), "{:?}", held.age());

    let digest = tool(
        &h.state,
        "digest_channel",
        json!({ "channel_id": READ_CHANNEL }),
    )
    .await;
    let header = digest.lines().next().expect("a header");
    assert!(header.contains("AS OF 10 minutes ago"), "{header}");
    assert!(header.contains(&format!("exact {as_of}")), "{header}");
    assert!(header.contains("may be missing newer messages"), "{header}");

    // The window the digest just read is young enough to answer the catch-up as it stands.
    let catch_up = tool(&h.state, "recent_activity", json!({})).await;
    let block = catch_up
        .lines()
        .find(|line| line.starts_with("build noise"))
        .expect("the channel has a block");
    assert!(block.contains("AS OF 10 minutes ago"), "{block}");

    // A current read says none of it.
    *h.bridge.as_of.lock().expect("as_of") = None;
    let body = get(
        &h.state,
        &format!("/api/v1/channels/{READ_CHANNEL}/messages"),
    )
    .await;
    assert!(body.get("as_of").is_none(), "{body}");
    let digest = tool(
        &h.state,
        "digest_channel",
        json!({ "channel_id": READ_CHANNEL }),
    )
    .await;
    assert!(!digest.contains("AS OF"), "{digest}");
    let held = h
        .state
        .live
        .recent()
        .snapshot(&channel(READ_CHANNEL))
        .held
        .expect("held");
    assert_eq!(held.as_of, None);
}

/// Off by default: no shadow read, nothing counted, no diagnostics section.
#[tokio::test]
async fn the_shadow_is_off_unless_a_bridge_turns_it_on() {
    let h = harness("");
    let discord = h.state.config.discord().expect("discord provider");
    assert_eq!(discord.recent, RecentReads::default());
    assert_eq!(discord.recent.view, RecentView::Messages);
    assert!(!recent_view::shadowed(&h.state, &channel(READ_CHANNEL)));

    ops::messages(&h.state, READ_CHANNEL, None)
        .await
        .expect("read");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.bridge.flat_reads.load(Ordering::SeqCst), 0);
    let report = get(&h.state, "/api/v1/diagnostics").await;
    assert!(report.get("recent_view_shadow").is_none(), "{report}");
}

/// Wait, briefly, for the detached Flat read to be folded into the counts.
async fn compared(state: &AppState, id: &str, times: u64) -> Tally {
    for _ in 0..200 {
        let tally = state.recent_view_shadow.tally(&channel(id));
        if tally.compared + tally.failed >= times {
            return tally;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the shadow read never finished");
}

/// Shadowed, every `/messages` read is followed by a Flat read of the same size, and where the
/// two disagree is counted per channel: a message only one listed, and a message whose text
/// differs. The caller's answer is the `/messages` read, untouched.
#[tokio::test]
async fn a_shadowed_read_counts_where_the_flat_view_disagrees() {
    let h = harness("recent_view_shadow = true");
    assert!(recent_view::shadowed(&h.state, &channel(READ_CHANNEL)));

    let window = ops::messages(&h.state, READ_CHANNEL, None)
        .await
        .expect("read");
    let agreed = compared(&h.state, READ_CHANNEL, 1).await;
    assert_eq!(
        agreed,
        Tally {
            reads: 1,
            compared: 1,
            ..Tally::default()
        }
    );

    let ids: Vec<MessageId> = window.messages.iter().map(|m| m.id.clone()).collect();
    h.bridge
        .flat_text
        .lock()
        .expect("text")
        .insert(ids[2].clone(), "deploy finish".to_owned());
    h.bridge
        .flat_missing
        .lock()
        .expect("missing")
        .insert(ids[1].clone());
    let window = ops::messages(&h.state, READ_CHANNEL, None)
        .await
        .expect("read");
    assert_eq!(window.messages.len(), 3, "the caller gets /messages");
    assert_eq!(window.messages[2].content, "deploy finished");
    let disagreed = compared(&h.state, READ_CHANNEL, 2).await;
    assert_eq!(
        disagreed,
        Tally {
            reads: 2,
            compared: 2,
            mismatched: 1,
            id_mismatches: 1,
            hash_mismatches: 1,
            ..Tally::default()
        }
    );
    assert_eq!(h.bridge.flat_reads.load(Ordering::SeqCst), 2);

    // A message newer than everything the /messages read returned arrived after it: in Flat only,
    // and not a mismatch.
    h.bridge.flat_missing.lock().expect("missing").clear();
    h.bridge.flat_text.lock().expect("text").clear();
    let late = h.fake.seed(&channel(READ_CHANNEL), "agent", "too late");
    h.bridge
        .recent_missing
        .lock()
        .expect("missing")
        .insert(late);
    ops::messages(&h.state, READ_CHANNEL, None)
        .await
        .expect("read");
    let later = compared(&h.state, READ_CHANNEL, 3).await;
    assert_eq!((later.mismatched, later.id_mismatches), (1, 1));

    // Every channel of the shadowed provider is listed, counted or not; counts and ids only.
    let report = get(&h.state, "/api/v1/diagnostics").await;
    let rows = report["recent_view_shadow"]
        .as_array()
        .expect("a shadow section");
    let row = |id: &str| {
        rows.iter()
            .find(|row| row["channel_id"] == json!(id))
            .unwrap_or_else(|| panic!("{id} is not listed: {report}"))
    };
    assert_eq!(row(READ_CHANNEL)["compared"], json!(3));
    assert_eq!(row(READ_CHANNEL)["id_mismatches"], json!(1));
    assert_eq!(row(READ_CHANNEL)["hash_mismatches"], json!(1));
    assert_eq!(row(WRITE_CHANNEL)["reads"], json!(0));
    let text = report["recent_view_shadow"].to_string();
    assert!(
        !text.contains("deploy"),
        "message text in diagnostics: {text}"
    );
}
