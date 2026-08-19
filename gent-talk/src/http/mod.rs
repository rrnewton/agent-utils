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
//! * `/mcp` is the Streamable HTTP MCP endpoint. It requires a bearer token too, and answers a
//!   credential-less caller with a bland 401 before it reads the body at all.

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
        .route("/api/v1/channels/{channel_id}/digest", get(api::digest))
        .route("/api/v1/channels/{channel_id}/resolve", post(api::resolve))
        .route("/api/v1/channels/{channel_id}/reply", post(api::reply))
        .route("/api/v1/channels/{channel_id}/ask", post(api::ask))
        // One path, three methods: POST carries the whole protocol, and GET/DELETE — which exist
        // in the spec for server-initiated streams and session teardown — are refused plainly
        // because this endpoint is stateless and has nothing to push.
        .route(
            crate::mcp::transport::MCP_PATH,
            post(crate::mcp::transport::post).fallback(crate::mcp::transport::method_not_allowed),
        )
        .fallback(api::not_found)
        .with_state(state)
}
