//! Pairing codes are taken out of everything a READ-scope caller is sent. `#369 pairing-link`.
//!
//! A pairing link redeems once for the write token, and the natural place to send one is the chat
//! the person reads, which may well be a channel this server bridges. Every read-scope caller can
//! read such a channel: the hosted voice agent holds the read token, and reads it over `/mcp`; any
//! other holder can read it over `/api/`, live streams included. Whoever redeemed the code first
//! would hold the write token, and "no read-scope credential ever posts" would be false.
//!
//! So every answer to a read-scope caller under `/api/` and `/mcp` passes through
//! [`crate::pairing::redact_codes`], which replaces the code after any `pair=` with `[redacted]`. It works
//! on the bytes as they leave, whatever produced them — a message body, its rendered HTML, a
//! thread's name, a summary, an MCP tool's text, a stream event — so no read path has to remember
//! to do it. A write-scope caller is sent everything as it is: it holds the token the code
//! redeems for already.
//!
//! The pattern is `pair=` followed by any run of base64url characters, wherever it appears (after
//! `#`, after `%23`, anywhere), so a code cut short by a preview is redacted too. Text that is not a
//! code and happens to match loses those characters, which is the right side to err on. Streams are
//! rewritten as they flow: bytes are held back only while the end of what has arrived could still
//! be part of a code, so a live stream, whose events end in a blank line, is not delayed. A
//! read-scope answer therefore has no `Content-Length`, the connection framing it instead.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::Response;
use futures_util::StreamExt as _;

use crate::auth::{self, Scope};
use crate::pairing::{redact_codes, unfinished_code_start};
use crate::state::AppState;

/// Rewrite the answer to a read-scope request so it carries no pairing code.
pub async fn redact_for_read_scope(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let private = path == "/mcp" || path.starts_with("/api/");
    let header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let read_only = private && auth::scope_of(header, &state.config.auth) == Some(Scope::Read);
    let response = next.run(request).await;
    if !read_only {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    // The length may change; the connection frames the body instead.
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, redact_body(body))
}

/// The most a stream holds back waiting to see where a run after `pair=` ends. No code is this
/// long; past it the run is redacted and sent, so a body cannot make this layer buffer it whole.
const MAX_HELD: usize = 4096;

/// The body, with every pairing code replaced as it streams.
fn redact_body(body: Body) -> Body {
    let stream = body.into_data_stream();
    let redacted = futures_util::stream::unfold(
        (stream, Vec::<u8>::new(), false),
        |(mut stream, mut carry, finished)| async move {
            if finished {
                return None;
            }
            loop {
                match stream.next().await {
                    Some(Ok(chunk)) => {
                        carry.extend_from_slice(&chunk);
                        // The tail that the next chunk could still extend into a code is held back
                        // UNREDACTED, so the whole run is redacted once its end is known: redacting
                        // it now would send the rest of the code as ordinary text.
                        let mut keep = unfinished_code_start(&carry);
                        if carry.len() - keep > MAX_HELD {
                            keep = carry.len();
                        }
                        let out = bytes::Bytes::from(redact_codes(&carry[..keep]));
                        carry.drain(..keep);
                        if !out.is_empty() {
                            return Some((Ok(out), (stream, carry, false)));
                        }
                    }
                    Some(Err(error)) => return Some((Err(error), (stream, carry, true))),
                    None if carry.is_empty() => return None,
                    // The body ended: what was held back is complete now.
                    None => {
                        let out = bytes::Bytes::from(redact_codes(&carry));
                        return Some((Ok(out), (stream, Vec::new(), true)));
                    }
                }
            }
        },
    );
    // Fused: a body may be polled again after it ended (the compression layer does), and an
    // unfolded stream panics if it is.
    Body::from_stream(redacted.fuse())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "Pq3x_Zr-7AbCdEfGhIjKlMnOpQrStUvWxYz01234567";

    async fn through(chunks: Vec<&[u8]>) -> String {
        let chunks: Vec<Result<bytes::Bytes, std::io::Error>> = chunks
            .into_iter()
            .map(|chunk| Ok(bytes::Bytes::copy_from_slice(chunk)))
            .collect();
        let body = redact_body(Body::from_stream(futures_util::stream::iter(chunks)));
        let mut out = Vec::new();
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            out.extend_from_slice(&chunk.expect("chunk"));
        }
        String::from_utf8(out).expect("utf8")
    }

    #[tokio::test]
    async fn a_code_split_across_chunks_at_any_point_is_still_redacted_whole() {
        let text = format!("data: {{\"content\":\"go to https://h/#pair={CODE} please\"}}\n\n");
        for split in 0..text.len() {
            let bytes = text.as_bytes();
            let out = through(vec![&bytes[..split], &bytes[split..]]).await;
            for piece in [&CODE[..4], &CODE[20..24], &CODE[39..]] {
                assert!(!out.contains(piece), "split at {split}: {out}");
            }
            assert!(
                out.contains("#pair=[redacted] please"),
                "split at {split}: {out}"
            );
        }
    }

    #[tokio::test]
    async fn a_complete_event_is_sent_at_once_and_a_held_tail_is_bounded() {
        let first = b"data: one\n\n".as_slice();
        // Nothing held back from an event that ends in a blank line.
        assert_eq!(unfinished_code_start(first), first.len());
        let endless = format!("pair={}", "A".repeat(MAX_HELD * 2));
        let out = through(vec![endless.as_bytes(), b" end"]).await;
        assert!(out.starts_with("pair=[redacted]"), "{}", &out[..40]);
    }
}
