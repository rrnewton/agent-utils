//! The HTTP surface: a small JSON API, plus the phone web app that consumes it.
//!
//! Route policy, stated once so it is checkable:
//!
//! * `/healthz` is unauthenticated and returns nothing about the configuration.
//! * The static web app is unauthenticated — it is public code, and it holds no secret. It asks
//!   the operator for an API token at runtime.
//! * Everything under `/api/` requires a bearer token, and the write scope is required for
//!   anything that reaches Discord with a message — and for minting a signed conversation URL,
//!   which is a credential for talking to an agent that can itself post.
//! * **No read-scope credential ever writes anything durable.** The read token is the one pasted
//!   into a hosted voice agent, so it is the least trusted thing that holds a credential here;
//!   every durable write — appending a turn, moving a read mark, filing a cached summary —
//!   requires the write scope. `/summary` is readable at read scope and is served from the cache
//!   when there is a hit, but a read-scope caller's generated summary is NOT filed; see
//!   [`crate::ops::summarize_message`].
//! * The write scope is ALSO required to READ a transcript. A stored transcript is the owner's
//!   own speech plus whatever channel text was read aloud to him, which is not a read of a
//!   channel he already allowlisted. The one durable read a read-scope token may make is
//!   `/inbox`, because how far HE has read is what the agent has to be able to say out loud.
//! * Every request — routed or not, authorized or not — leaves exactly ONE line in the access
//!   log at INFO. See [`crate::access`] for why that is load-bearing rather than nice to have.
//!   **The one line for `/stream` is written at ATTACH, with `millis=0`**, because the middleware
//!   returns as soon as the status is known and a streaming body has not started yet. A stream
//!   held open for an hour still logs zero; see [`api::stream`].
//! * `/api/v1/channels/{id}/stream` is a long-lived Server-Sent Events response and is otherwise
//!   an ordinary read: same bearer token, same read scope, same channel allowlist. A stream is the
//!   easiest thing here to leave accidentally open, so it is deliberately NOT a special case.
//! * `/mcp` is the Streamable HTTP MCP endpoint. It requires a bearer token too, and answers a
//!   credential-less caller with a bland 401 before it reads the body at all.

pub mod access_layer;
pub mod api;

use axum::routing::{get, post};
use axum::Router;

use crate::state::AppState;

/// Build the whole router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(api::healthz))
        .route("/", get(api::index_html))
        .route("/app.js", get(api::app_js))
        .route("/voice", get(api::voice_html))
        .route("/voice.js", get(api::voice_js))
        .route("/voice.css", get(api::voice_css))
        .route("/style.css", get(api::style_css))
        .route("/api/v1/channels", get(api::list_channels))
        .route("/api/v1/agent-tools", get(api::agent_tools))
        .route("/api/v1/client-config", get(api::client_config))
        // READ scope. It reports configuration health and never a credential, it writes nothing
        // durable, and it always answers 200 — the report is the answer, so a failing check is
        // not an HTTP failure. See `api::diagnostics`.
        .route("/api/v1/diagnostics", get(api::diagnostics))
        .route("/api/v1/signed-url", get(api::signed_url))
        .route("/api/v1/channels/{channel_id}/messages", get(api::messages))
        .route(
            "/api/v1/channels/{channel_id}/messages/{message_id}",
            get(api::message_by_id),
        )
        .route(
            "/api/v1/channels/{channel_id}/messages/{message_id}/summary",
            get(api::message_summary),
        )
        .route("/api/v1/channels/{channel_id}/page", get(api::page))
        .route("/api/v1/channels/{channel_id}/count", get(api::count))
        .route("/api/v1/channels/{channel_id}/digest", get(api::digest))
        .route("/api/v1/channels/{channel_id}/resolve", post(api::resolve))
        .route("/api/v1/channels/{channel_id}/reply", post(api::reply))
        .route("/api/v1/channels/{channel_id}/ask", post(api::ask))
        .route("/api/v1/channels/{channel_id}/stream", get(api::stream))
        // Durable state. The conversation routes all require the WRITE scope, reads included —
        // see the block comment above them in `api` for why a transcript is more sensitive than
        // a digest. `/inbox` is a read; moving a mark is a write, because it mutates state
        // another device reads back.
        .route(
            "/api/v1/conversations",
            get(api::list_conversations).delete(api::forget_conversations),
        )
        .route(
            "/api/v1/conversations/{conversation_id}",
            get(api::conversation).delete(api::forget_conversation),
        )
        .route(
            "/api/v1/conversations/{conversation_id}/turns",
            post(api::append_turn),
        )
        // `#46 conversation-replay`. A READ of a transcript, rendered — so it takes the write
        // scope like every other conversation route, for the reason stated above them.
        .route(
            "/api/v1/conversations/{conversation_id}/replay",
            get(api::replay),
        )
        // The operator's erase, and the only COMPLETE one over HTTP: the conversation routes
        // clear transcripts and say so, this one clears everything the store holds.
        .route("/api/v1/storage", axum::routing::delete(api::purge_storage))
        .route("/api/v1/inbox", get(api::inbox))
        // `#50 todo-view`. The channel filtered down to what has not been dealt with, and the two
        // acts that change that. READ scope to look, WRITE scope to change — the same split every
        // durable write on this server takes, and the reason it is not "read scope because it is
        // only a flag" is that the flag outlives the process and another device reads it back.
        // `#50` read-aloud. POST because it spends money at a vendor; see `api::speak`.
        .route(
            "/api/v1/channels/{channel_id}/messages/{message_id}/speak",
            post(api::speak),
        )
        .route("/api/v1/channels/{channel_id}/todo", get(api::todo))
        .route("/api/v1/channels/{channel_id}/dismiss", post(api::dismiss))
        .route("/api/v1/channels/{channel_id}/restore", post(api::restore))
        .route(
            "/api/v1/channels/{channel_id}/read",
            post(api::mark_read).delete(api::forget_read_mark),
        )
        // `#39 channel-alias`. The operator's own local name for a channel. WRITE scope both
        // ways, because it outlives the process — and NO MCP TOOL at all, which is the part that
        // actually keeps a model from renaming the channels it reports on. See `api::set_alias`
        // for why the scope alone would not. Reading an alias needs no route of its own: it rides
        // on every channel this server hands back.
        .route(
            "/api/v1/channels/{channel_id}/alias",
            axum::routing::put(api::set_alias).delete(api::clear_alias),
        )
        // One path, three methods: POST carries the whole protocol, and GET/DELETE — which exist
        // in the spec for server-initiated streams and session teardown — are refused plainly
        // because this endpoint is stateless and has nothing to push.
        .route(
            crate::mcp::transport::MCP_PATH,
            post(crate::mcp::transport::post).fallback(crate::mcp::transport::method_not_allowed),
        )
        .fallback(api::not_found)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            access_layer::log_requests,
        ))
        .with_state(state)
}
