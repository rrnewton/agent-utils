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
//! * The write scope is ALSO required for everything that touches durable state, reads included.
//!   A stored transcript is the owner's own speech, and moving a read mark changes what another
//!   device will be shown. Neither is a read of a channel he already allowlisted.
//! * Every request — routed or not, authorized or not — leaves exactly ONE line in the access
//!   log at INFO. See [`crate::access`] for why that is load-bearing rather than nice to have.
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
        .route("/api/v1/inbox", get(api::inbox))
        .route(
            "/api/v1/channels/{channel_id}/read",
            post(api::mark_read).delete(api::forget_read_mark),
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
