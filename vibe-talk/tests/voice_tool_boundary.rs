//! The voice agent's tools reach only this server (`#2115 vibe-talk-voice-tool-audit`).
//!
//! The owner asked to confirm that every voice-agent tool reads only through this server and its
//! synced state, and never goes to the chat provider directly. The bridge pins its own half: it
//! sends its credential only to this server's origin and registers only the tools this server
//! lists. This file pins the server's half:
//!
//! 1. **No MCP tool handler holds the provider client.** Every tool runs through the operations
//!    layer, server search or the documents index, so the channel allowlist, the read budget and the
//!    cache rules apply to every call whatever front door it came through.
//! 2. **Every tool a voice session is offered is classified here**, as one that never reaches the
//!    provider or as a KNOWN fallback read. A new tool fails this test until it is classified.
//! 3. **On a cold cache with serving on**, a tool classified as never reaching the provider makes
//!    no provider call at all, and each known fallback still makes one. When a fallback is removed
//!    (cache-only voice reads, the audit's first finding), its entry must go too, so the list only
//!    shrinks, and when it is empty this test is the plain server-only rule.
//!
//! Calls are counted, never timed: nothing here depends on how loaded the machine is.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use vibe_talk::auth::Scope;
use vibe_talk::model::{ChannelId, MessageId};
use vibe_talk::state::AppState;
use vibe_talk::testing::{self, CountingChat, ManualClock, TempDir, TimelineChat, WRITE_CHANNEL};

/// 2026-08-18T12:00:00Z: where the fixture's messages are written, a minute apart.
const T0: i64 = 1_787_054_400_000;
const MINUTE: i64 = 60_000;
/// A generous ceiling for the cache writer and the shadow to catch up; never a budget.
const WAIT: Duration = Duration::from_secs(60);

/// Tools that never reach the chat provider, whatever the cache holds. `post_reply` is here for
/// the shipped default (the owner confirms each send on the voice page), under which it only
/// proposes; a confirmed send goes out through the server's post gate, as `explicit_send.rs`
/// tests.
const NEVER_UPSTREAM: &[&str] = &[
    "list_channels",
    "search",
    "fetch_document",
    "fetch_document_section",
    "mark_read",
    "post_reply",
];

/// Tools that still fall back to a provider read when the cache does not hold the answer (the
/// voice tool audit's first finding, 2026-11-22). An entry goes when its fallback does; this list
/// never grows.
const KNOWN_FALLBACK: &[&str] = &[
    "recent_activity",
    "digest_channel",
    "read_page",
    "count_messages",
    "find_message",
    "read_message",
];

/// The source of every MCP module, which must never name the provider client.
const MCP_SOURCES: &[(&str, &str)] = &[
    ("src/mcp/mod.rs", include_str!("../src/mcp/mod.rs")),
    (
        "src/mcp/protocol.rs",
        include_str!("../src/mcp/protocol.rs"),
    ),
    (
        "src/mcp/transport.rs",
        include_str!("../src/mcp/transport.rs"),
    ),
];

#[test]
fn no_mcp_module_reaches_the_provider_client_directly() {
    for (path, source) in MCP_SOURCES {
        for forbidden in ["state.chat", "ChatClient", ".chat()"] {
            assert!(
                !source.contains(forbidden),
                "{path} names `{forbidden}`: a voice tool must reach the provider only through \
                 the operations layer (crate::ops), never by holding the provider client"
            );
        }
    }
}

/// One MCP tool call at write scope, as the deployment's voice bridge makes it: whether the result
/// is an error, and its text.
async fn call_tool(state: &AppState, name: &str, arguments: &Value) -> (bool, String) {
    let outcome = vibe_talk::mcp::protocol::dispatch(
        state,
        Scope::Write,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments },
        }),
    )
    .await;
    let vibe_talk::mcp::protocol::Outcome::Reply(response) = outcome else {
        panic!("{name}: no reply");
    };
    let result = response
        .result
        .as_ref()
        .unwrap_or_else(|| panic!("{name}: no result"));
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    (result["isError"] == json!(true), text)
}

/// Let every task a call started run as far as it can, then wait for the cache writer and the
/// shadow: a provider call a tool started behind its answer is counted before the next tool runs.
async fn drain(state: &AppState) {
    for _ in 0..256 {
        tokio::task::yield_now().await;
    }
    assert!(
        state.message_cache.shadow_settled(WAIT).await,
        "every evaluation was counted"
    );
    if let Some(cache) = state.message_cache.cache() {
        assert!(cache.quiesce_blocking(WAIT), "the writer caught up");
    }
}

/// A deployment whose message cache holds nothing yet, with serving on and every provider call
/// counted: the seeded messages are the provider's, and the cache has read none of them.
struct Cold {
    state: AppState,
    counting: Arc<CountingChat>,
    ids: Vec<MessageId>,
    _dir: TempDir,
}

async fn cold(tag: &str) -> Cold {
    let dir = TempDir::new(tag);
    let (mut state, fake, _voice) = testing::state_from_toml(&testing::config_toml());
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    let ids: Vec<MessageId> = (0..6)
        .map(|n| {
            fake.seed_at(
                &channel,
                "Ada",
                &format!("message {n} about the mac runner"),
                T0 + n * MINUTE,
            )
        })
        .collect();
    let counting = Arc::new(CountingChat::new(Arc::new(TimelineChat::new(fake))));
    state.replace_chat(counting.clone());
    let state =
        testing::with_message_cache(state, dir.path(), ManualClock::at(T0 + 60 * MINUTE)).await;
    state.message_cache.set_serve(true);
    // The owner's own words, as the voice page heard them, for the two tools that need an ask.
    state
        .send_intent
        .hear("turn-1", "okay, mark that read please");
    state
        .send_intent
        .hear("turn-2", "Tell the coding agent to rebase onto main.");
    drain(&state).await;
    Cold {
        state,
        counting,
        ids,
        _dir: dir,
    }
}

/// The arguments each tool is called with, on a deployment whose messages are `ids`.
fn arguments(name: &str, ids: &[MessageId]) -> Value {
    let document = "github:octo/widgets@main:docs/report.md";
    match name {
        "list_channels" | "recent_activity" => json!({}),
        "search" => json!({ "query": "mac runner" }),
        "fetch_document" => json!({ "document_key": document }),
        "fetch_document_section" => json!({ "document_key": document, "heading": "Methods" }),
        "mark_read" => json!({
            "channel_id": WRITE_CHANNEL,
            "message_ids": [ids[1].as_str()],
            "user_request": "mark that read",
        }),
        "post_reply" => json!({
            "channel_id": WRITE_CHANNEL,
            "text": "Please rebase onto main.",
            "user_request": "tell the coding agent to rebase onto main",
        }),
        "digest_channel" | "count_messages" => json!({ "channel_id": WRITE_CHANNEL }),
        "read_page" => json!({ "channel_id": WRITE_CHANNEL, "limit": 5 }),
        "find_message" => json!({ "channel_id": WRITE_CHANNEL, "query": "mac runner" }),
        "read_message" => json!({ "channel_id": WRITE_CHANNEL, "message_id": ids[2].as_str() }),
        other => panic!("{other} has no arguments here: classify it and give it some"),
    }
}

#[tokio::test]
async fn every_voice_tool_a_session_can_call_is_classified() {
    let never: BTreeSet<&str> = NEVER_UPSTREAM.iter().copied().collect();
    let fallback: BTreeSet<&str> = KNOWN_FALLBACK.iter().copied().collect();
    assert!(
        never.is_disjoint(&fallback),
        "a tool is classified twice: {:?}",
        never.intersection(&fallback).collect::<Vec<_>>()
    );
    let deployment = cold("voice-tool-classes").await;
    let mut offered: BTreeSet<String> =
        vibe_talk::mcp::protocol::tools_for(&deployment.state, Scope::Write)
            .await
            .iter()
            .map(|tool| tool["name"].as_str().expect("a name").to_owned())
            .collect();
    // No longer listed, and still answered by name (`#437 voice-full-text-catchup`).
    offered.insert("digest_channel".to_owned());
    let classified: BTreeSet<String> = never
        .union(&fallback)
        .map(|name| (*name).to_owned())
        .collect();
    assert_eq!(
        offered, classified,
        "every tool a voice session can call is classified here: add a new tool to NEVER_UPSTREAM \
         (and prove it) rather than to KNOWN_FALLBACK, which only shrinks"
    );
}

/// Each tool on a deployment of its own, so no read another tool made can warm the cache for it or
/// be counted against it.
#[tokio::test]
async fn on_a_cold_cache_only_the_known_fallbacks_reach_the_provider() {
    for name in NEVER_UPSTREAM {
        let deployment = cold(&format!("voice-tool-never-{name}")).await;
        let before = deployment.counting.calls().len();
        let (_, text) = call_tool(&deployment.state, name, &arguments(name, &deployment.ids)).await;
        drain(&deployment.state).await;
        let made: Vec<&str> = deployment.counting.calls()[before..]
            .iter()
            .map(|call| call.method)
            .collect();
        assert!(
            made.is_empty(),
            "{name} reached the chat provider ({made:?}); a voice tool reads only this server's \
             own state. It answered: {text}"
        );
    }
    for name in KNOWN_FALLBACK {
        let deployment = cold(&format!("voice-tool-fallback-{name}")).await;
        let before = deployment.counting.calls().len();
        let (_, text) = call_tool(&deployment.state, name, &arguments(name, &deployment.ids)).await;
        drain(&deployment.state).await;
        assert!(
            deployment.counting.calls().len() > before,
            "{name} no longer reads the provider on a cold cache: move it from KNOWN_FALLBACK to \
             NEVER_UPSTREAM. It answered: {text}"
        );
    }
}
