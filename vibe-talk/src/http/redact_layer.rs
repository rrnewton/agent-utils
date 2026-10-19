//! Pairing codes are taken out of everything a READ-scope caller is sent. `#369 pairing-link`.
//!
//! A pairing link redeems once for the write token, and the natural place to send one is the chat
//! the person reads, which may well be a channel this server bridges. Every read-scope caller can
//! read such a channel: the hosted voice agent holds the read token, and reads it over `/mcp`; any
//! other holder can read it over `/api/`, live streams included. Whoever redeemed the code first
//! would hold the write token, and "no read-scope credential ever posts" would be false.
//!
//! So every answer to a read-scope caller under `/api/` and `/mcp` passes through
//! [`redact_pairing_codes`], which replaces the code after any `pair=` with `[redacted]`. It works
//! on the bytes as they leave, whatever produced them — a message body, its rendered HTML, a
//! thread's name, a summary, an MCP tool's text, a stream event — so no read path has to remember
//! to do it. A write-scope caller is sent everything as it is: it holds the token the code
//! redeems for already.
//!
//! The pattern is `pair=` followed by at least [`CODE_CHARS`] base64url characters, which is what
//! [`crate::pairing`] mints, wherever it appears (after `#`, after `%23`, anywhere). Text that is
//! not a code and happens to match loses those characters, which is the right side to err on.
//! Streams are rewritten as they flow: an event is held back only while its last bytes could be the
//! start of a code that the next chunk completes, so a live stream is not delayed.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::Response;
use futures_util::StreamExt as _;

use crate::auth::{self, Scope};
use crate::state::AppState;

/// The characters in a minted code: 32 bytes, base64url without padding.
pub const CODE_CHARS: usize = 43;
const MARKER: &[u8] = b"pair=";
const REDACTED: &[u8] = b"[redacted]";

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
                        let whole = redact_pairing_codes(&carry);
                        let keep = partial_code_start(&whole);
                        let out = bytes::Bytes::copy_from_slice(&whole[..keep]);
                        carry = whole[keep..].to_vec();
                        if !out.is_empty() {
                            return Some((Ok(out), (stream, carry, false)));
                        }
                    }
                    Some(Err(error)) => return Some((Err(error), (stream, carry, true))),
                    None if carry.is_empty() => return None,
                    // What was held back was not completed into a code; it goes as it is.
                    None => {
                        let out = bytes::Bytes::from(redact_pairing_codes(&carry));
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

const fn is_code_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

/// `text` with the run of code characters after every `pair=` replaced by `[redacted]`, when the
/// run is at least [`CODE_CHARS`] long.
#[must_use]
pub fn redact_pairing_codes(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        if text[at..].starts_with(MARKER) {
            let start = at + MARKER.len();
            let run = text[start..]
                .iter()
                .take_while(|byte| is_code_byte(**byte))
                .count();
            if run >= CODE_CHARS {
                out.extend_from_slice(MARKER);
                out.extend_from_slice(REDACTED);
                at = start + run;
                continue;
            }
        }
        out.push(text[at]);
        at += 1;
    }
    out
}

/// Where the unfinished start of a code begins at the end of `text`, or `text.len()` when it ends
/// with none: a tail that is a prefix of `pair=`, or `pair=` and fewer than [`CODE_CHARS`] code
/// characters, could become a code when the next chunk arrives, and must not be sent yet.
#[must_use]
pub fn partial_code_start(text: &[u8]) -> usize {
    let longest = MARKER.len() + CODE_CHARS - 1;
    let from = text.len().saturating_sub(longest);
    for start in from..text.len() {
        let tail = &text[start..];
        let marker = tail.len().min(MARKER.len());
        if tail[..marker] == MARKER[..marker]
            && tail[marker..].iter().all(|byte| is_code_byte(*byte))
        {
            return start;
        }
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "Pq3x_Zr-7AbCdEfGhIjKlMnOpQrStUvWxYz01234567";

    #[test]
    fn a_code_after_pair_is_redacted_wherever_it_appears() {
        for (text, want) in [
            (
                format!("open https://h.example/#pair={CODE} now"),
                "open https://h.example/#pair=[redacted] now".to_owned(),
            ),
            (
                format!("{{\"content\":\"<https://h/%23pair={CODE}|link>\"}}"),
                "{\"content\":\"<https://h/%23pair=[redacted]|link>\"}".to_owned(),
            ),
            (format!("pair={CODE}extra"), "pair=[redacted]".to_owned()),
            // Too short to be a code: left alone.
            (
                "pair=short and #pair=".to_owned(),
                "pair=short and #pair=".to_owned(),
            ),
        ] {
            assert_eq!(
                String::from_utf8(redact_pairing_codes(text.as_bytes())).expect("utf8"),
                want
            );
        }
    }

    #[test]
    fn only_a_tail_that_could_still_become_a_code_is_held_back() {
        assert_eq!(partial_code_start(b"data: done\n\n"), 12);
        assert_eq!(partial_code_start(b"see #pai"), 5);
        assert_eq!(partial_code_start(b"see #pair=Pq3x"), 5);
        assert_eq!(partial_code_start(b"see #pair=[redacted]"), 20);
    }

    #[tokio::test]
    async fn a_code_split_across_chunks_at_any_point_is_still_redacted() {
        let text = format!("data: {{\"content\":\"go to https://h/#pair={CODE} please\"}}\n\n");
        for split in 0..text.len() {
            let chunks: Vec<Result<bytes::Bytes, std::io::Error>> = vec![
                Ok(bytes::Bytes::copy_from_slice(&text.as_bytes()[..split])),
                Ok(bytes::Bytes::copy_from_slice(&text.as_bytes()[split..])),
            ];
            let body = redact_body(Body::from_stream(futures_util::stream::iter(chunks)));
            let mut out = Vec::new();
            let mut stream = body.into_data_stream();
            while let Some(chunk) = stream.next().await {
                out.extend_from_slice(&chunk.expect("chunk"));
            }
            let out = String::from_utf8(out).expect("utf8");
            assert!(!out.contains(&CODE[..12]), "split at {split}: {out}");
            assert!(
                out.contains("#pair=[redacted] please"),
                "split at {split}: {out}"
            );
        }
    }
}
