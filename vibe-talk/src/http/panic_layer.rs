//! A handler that panics still answers. `#2241 vibe-talk-document-load-502-readable`.
//!
//! Without this layer a panic inside a handler unwinds the connection's task:
//! the connection drops with no answer, and whatever fronts the deployment (a
//! tunnel, a reverse proxy) answers its own error page — a bare 502 the page
//! can only call non-JSON. Here the panic is caught at the request, answered
//! as the one error contract (500 `internal_error`, a fixed sentence: never
//! the panic's text or a backtrace, which may name internals), and logged
//! server-side as exactly one ERROR line carrying the method, the path (no
//! query, as the access log), the cause code and the panic's message, so a
//! caught panic is never silent in the log a post-deploy check counts.
//!
//! The process's panic hook is untouched and still prints its own line to
//! stderr; this layer only decides what the caller is answered.

use std::any::Any;
use std::panic::AssertUnwindSafe;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_util::FutureExt;

use crate::contract::ApiErrorBody;

/// The cause code a caught panic answers and logs with.
pub const PANIC_CODE: &str = "internal_error";

/// The sentence a caught panic answers with: what happened, never the panic's own words.
pub const PANIC_DETAIL: &str =
    "vibe-talk hit an internal error while answering this request; it is logged on the server";

/// The panic's message, when it carried one as text.
fn panic_message(payload: &(dyn Any + Send)) -> &str {
    if let Some(text) = payload.downcast_ref::<&'static str>() {
        text
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.as_str()
    } else {
        "(no message)"
    }
}

/// Answer a panicking handler with a JSON 500 and one ERROR log line.
pub async fn catch_panics(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    match AssertUnwindSafe(next.run(request)).catch_unwind().await {
        Ok(response) => response,
        Err(payload) => {
            tracing::error!(
                method = %method,
                path = %path,
                cause = PANIC_CODE,
                panic = %panic_message(payload.as_ref()),
                "request handler panicked; answered 500 {PANIC_CODE}"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiErrorBody {
                    error: PANIC_CODE,
                    detail: PANIC_DETAIL.to_owned(),
                }),
            )
                .into_response()
        }
    }
}
