//! Every `/api/` refusal answers JSON. `#1844 vibe-talk-discuss-followup-422`.
//!
//! Handlers answer with [`crate::contract::ApiErrorBody`], but an extractor
//! refusal (a body that is not JSON, JSON of the wrong shape, a missing
//! content type, a bad query or path value) is answered by axum itself, in
//! plain text. The page parses every error body as JSON, so a plain-text
//! refusal reached the reader as "vibe-talk returned non-JSON (HTTP 422)"
//! and hid the real reason — a malformed follow-up body hid there for the
//! route's whole life. This layer rewrites any non-JSON 4xx under `/api/`
//! into the one error contract, keeping the status, headers (a 405's
//! `allow` included) and the refusal's own words as the detail. Successes,
//! streams and already-JSON answers pass through untouched, and a 5xx keeps
//! the handler's own answer and cause.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::contract::ApiErrorBody;

/// The largest refusal body worth carrying into the JSON detail. Extractor
/// refusals are one sentence; anything longer is not one.
const MAX_REFUSAL_BYTES: usize = 64 * 1024;

/// The code a rewritten refusal carries, by its status. Handler refusals
/// name their own codes; these are only for refusals nobody named.
fn code_for(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "bad_request",
        StatusCode::NOT_FOUND => "not_found",
        StatusCode::METHOD_NOT_ALLOWED => "method_not_allowed",
        StatusCode::UNPROCESSABLE_ENTITY => "invalid_request_body",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "unsupported_media_type",
        _ => "request_refused",
    }
}

/// Rewrite a non-JSON `/api/` client refusal as [`ApiErrorBody`]; pass
/// everything else through as it was answered.
pub async fn json_errors(request: Request, next: Next) -> Response {
    let api = request.uri().path().starts_with("/api/");
    let response = next.run(request).await;
    if !api || !response.status().is_client_error() {
        return response;
    }
    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("application/json"));
    if is_json {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_REFUSAL_BYTES).await else {
        return Response::from_parts(parts, Body::empty());
    };
    let said = String::from_utf8_lossy(&bytes).trim().to_owned();
    let detail = if said.is_empty() {
        parts
            .status
            .canonical_reason()
            .unwrap_or("the request was refused")
            .to_owned()
    } else {
        said
    };
    let mut answer = (
        parts.status,
        Json(ApiErrorBody {
            error: code_for(parts.status),
            detail,
        }),
    )
        .into_response();
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    *answer.headers_mut() = parts.headers;
    answer
}
