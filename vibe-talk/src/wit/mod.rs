//! Read-aloud speech synthesis through the public Wit.ai speech API.
//!
//! Wit.ai is an already-running public speech service: this server sends one
//! sentence-sized piece of text per request and plays back the audio it
//! returns. Nothing is self-hosted and no GPU is involved.
//!
//! The contract used here is the public one, corroborated against Wit.ai's
//! public HTTP documentation and public client code:
//!
//! * `POST {api_base}/synthesize?v=<version>` with the app token in an
//!   `Authorization: Bearer` header, a JSON body of `q` and `voice` plus
//!   optional `style`, `speed` and `pitch`, and an `Accept` header naming
//!   the audio format. This server asks for `audio/wav`: a self-describing
//!   PCM container the browser already plays, whose sentence pieces can be
//!   merged exactly (same format in, one header out) and checked for
//!   silence sample by sample.
//! * `GET {api_base}/voices?v=<version>` with the same header, answering a
//!   map of locale to voice entries; the Settings picker lists those.
//!
//! Three rules hold in this module, as they do in [`crate::elevenlabs`]:
//!
//! * **The token never appears in an output.** It is read from a file the
//!   operator names (`wit.token_file`), travels only in a request header,
//!   and every error built from a vendor response goes through [`redact`].
//! * **Failures are loud and distinguishable** (`#552 wit-tts-provider`,
//!   `#546 read-aloud-silent-audio`). A rejected token, an exhausted rate
//!   limit, an unreachable service and an answer of pure silence are four
//!   different errors. None of them is ever played back as if it were
//!   speech.
//! * **The vendor is asked politely.** Requests are sentence-sized, at
//!   most two run at once, a 429 is backed off before it is retried, and
//!   synthesized sentences are cached by (text, voice, speed).

pub mod fake;
pub mod http;
pub mod speech;

use async_trait::async_trait;

use crate::config::Secret;

/// The Wit.ai API version every request names, as `?v=`.
pub const API_VERSION: &str = "20240304";

/// The audio format asked of Wit.ai, and the reason: see the module docs.
pub const AUDIO_ACCEPT: &str = "audio/wav";
/// The content type handed to the browser for that audio.
pub const AUDIO_CONTENT_TYPE: &str = "audio/wav";

/// Longest text sent in one synthesize request, in characters.
///
/// Wit.ai reads a sentence well and a paragraph poorly, and its rate limits
/// count requests: sentence-sized pieces are both the quality choice and
/// the politeness choice. Longer sentences are cut at a word boundary.
pub const MAX_CHUNK_CHARS: usize = 280;

/// What [`redact`] leaves behind.
pub const REDACTED: &str = "<redacted>";

/// Replace every occurrence of a secret with a marker.
///
/// A blank secret is not searched for, for the same reason as in
/// [`crate::elevenlabs::redact`]: replacing an empty needle mangles text.
#[must_use]
pub fn redact(text: &str, secret: &Secret) -> String {
    let needle = secret.expose();
    if needle.is_empty() {
        return text.to_owned();
    }
    text.replace(needle, REDACTED)
}

/// One voice Wit.ai offers, as `GET /voices` reports it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Voice {
    /// The voice name a synthesize request names, for example `wit$Remi`.
    pub name: String,
    /// The locale the voice speaks, for example `en`.
    pub locale: String,
    /// The voice's gender as the vendor reports it, when it reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gender: Option<String>,
    /// Delivery styles the voice supports, when the vendor lists them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub styles: Vec<String>,
}

impl Voice {
    /// The name as a person reads it: `Remi` for `wit$Remi`.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.name.strip_prefix("wit$").unwrap_or(&self.name)
    }
}

/// Why one Wit.ai call failed, before the adapter words it for the reader.
///
/// The variants are kept apart because their fixes differ, exactly as in
/// [`crate::elevenlabs::SpeechError`]: re-issue the token, wait out the
/// limit, check the network, or look at the vendor.
#[derive(Debug, thiserror::Error)]
pub enum WitError {
    /// Wit.ai refused the app token (HTTP 401 or 403).
    #[error("Wit.ai refused the app token (HTTP {status}): {body}")]
    Unauthorized {
        /// HTTP status code.
        status: u16,
        /// Response body, truncated and redacted.
        body: String,
    },
    /// Wit.ai rate-limited the app and the retries ran out (HTTP 429).
    #[error("Wit.ai rate limit did not clear after {attempts} attempts: {body}")]
    RateLimited {
        /// How many attempts were made in total.
        attempts: u32,
        /// Last response body, truncated and redacted.
        body: String,
    },
    /// Wit.ai answered with any other non-success status.
    #[error("Wit.ai returned HTTP {status}: {body}")]
    Status {
        /// HTTP status code.
        status: u16,
        /// Response body, truncated and redacted.
        body: String,
    },
    /// The request never completed, or took too long.
    #[error("Wit.ai could not be reached: {0}")]
    Transport(String),
    /// The answer was not usable audio or not the expected shape.
    #[error("Wit.ai answered with something this server cannot use: {0}")]
    Shape(String),
}

impl WitError {
    /// Build a status error from a vendor response body, truncated and
    /// redacted through the one place that does it.
    #[must_use]
    pub fn from_response(status: u16, body: &str, token: &Secret, attempts: u32) -> Self {
        let mut body = redact(body, token);
        // Truncate on a character boundary: the body is vendor text, and
        // `String::truncate` panics mid-character.
        let mut end = body.len().min(500);
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        body.truncate(end);
        match status {
            401 | 403 => Self::Unauthorized { status, body },
            429 => Self::RateLimited { attempts, body },
            _ => Self::Status { status, body },
        }
    }

    /// Stable machine-readable code for the shared speech boundary.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unauthorized { .. } => "wit_unauthorized",
            Self::RateLimited { .. } => "wit_rate_limited",
            Self::Status { .. } => "wit_error",
            Self::Transport(_) => "wit_unreachable",
            Self::Shape(_) => "wit_bad_response",
        }
    }
}

/// The two Wit.ai calls read-aloud needs, behind one trait so tests can
/// stand in a fake that can refuse, stall and stay silent.
#[async_trait]
pub trait WitClient: Send + Sync {
    /// Synthesize ONE sentence-sized piece of text; returns WAV bytes.
    ///
    /// # Errors
    ///
    /// Returns [`WitError`] for a refused token, an exhausted rate limit,
    /// a transport failure, or an unusable answer.
    async fn synthesize(
        &self,
        token: &Secret,
        text: &str,
        voice: &str,
        speed_percent: Option<u32>,
    ) -> Result<Vec<u8>, WitError>;

    /// Every voice the app may use, in the vendor's order.
    ///
    /// # Errors
    ///
    /// As [`WitClient::synthesize`], minus the audio-shape failures.
    async fn voices(&self, token: &Secret) -> Result<Vec<Voice>, WitError>;
}

/// Cut `text` into sentence-sized pieces of at most [`MAX_CHUNK_CHARS`].
///
/// A piece ends after sentence punctuation (`.`, `!`, `?`, `…`) followed
/// by a space, or at a newline. A sentence longer than the cap is cut at
/// its last word boundary inside the cap, and a single word longer than
/// the cap is cut hard: no piece is ever empty and none is over the cap.
#[must_use]
pub fn sentence_chunks(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        if rest.chars().count() <= MAX_CHUNK_CHARS {
            chunks.push(rest.to_owned());
            break;
        }
        // The longest prefix within the cap, its byte length, and the best
        // cut seen inside it: after sentence punctuation, else at a word.
        let mut bytes = 0;
        let mut sentence_cut: Option<usize> = None;
        let mut word_cut: Option<usize> = None;
        let mut previous = '\0';
        for (index, ch) in rest.char_indices() {
            if index > 0 && rest[..index].chars().count() > MAX_CHUNK_CHARS {
                break;
            }
            bytes = index + ch.len_utf8();
            if (matches!(previous, '.' | '!' | '?' | '…') && ch == ' ') || ch == '\n' {
                sentence_cut = Some(index + ch.len_utf8());
            }
            if ch == ' ' || ch == '\n' {
                word_cut = Some(index + ch.len_utf8());
            }
            previous = ch;
        }
        let cut = sentence_cut
            .or(word_cut)
            .filter(|cut| *cut > 0)
            .unwrap_or(bytes);
        let (piece, tail) = rest.split_at(cut);
        let piece = piece.trim();
        if !piece.is_empty() {
            chunks.push(piece.to_owned());
        }
        rest = tail.trim_start();
    }
    chunks
}

/// The pace Wit.ai is asked for: a percentage, 100 being the voice's own.
///
/// Read-aloud speeds are multipliers (0.5 to 2.0, see
/// [`crate::speech::clamp_speed`]); Wit.ai takes a percentage of the same
/// range. `None` asks for the vendor default by omitting the field, which
/// happens exactly when the multiplier is the voice's own 1.0.
#[must_use]
pub fn speed_percent(speed: Option<f64>) -> Option<u32> {
    let speed = crate::speech::clamp_speed(speed)?;
    if (speed - 1.0).abs() < f64::EPSILON {
        return None;
    }
    Some((speed * 100.0).round().clamp(50.0, 200.0) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_split_on_sentences_and_cap_long_ones() {
        let text = "First sentence here. Second one follows! And a third?";
        assert_eq!(
            sentence_chunks(text),
            vec!["First sentence here. Second one follows! And a third?"]
        );
        let long = format!("{} {}", "word ".repeat(80).trim(), "Next sentence.");
        let chunks = sentence_chunks(&long);
        assert!(chunks.len() > 1, "{chunks:?}");
        assert!(chunks
            .iter()
            .all(|c| c.chars().count() <= MAX_CHUNK_CHARS && !c.is_empty()));
        assert_eq!(sentence_chunks("   "), Vec::<String>::new());
        assert_eq!(sentence_chunks("one"), vec!["one"]);
    }

    #[test]
    fn speed_percent_maps_the_multiplier_range() {
        assert_eq!(speed_percent(None), None);
        assert_eq!(speed_percent(Some(1.0)), None);
        assert_eq!(speed_percent(Some(0.5)), Some(50));
        assert_eq!(speed_percent(Some(1.5)), Some(150));
        assert_eq!(speed_percent(Some(9.0)), Some(200));
        assert_eq!(speed_percent(Some(f64::NAN)), None);
    }

    #[test]
    fn a_long_multibyte_error_body_truncates_without_panicking() {
        let token = Secret::new("tok-secret-value");
        let body = "€".repeat(400);
        let error = WitError::from_response(401, &body, &token, 1);
        assert!(error.to_string().len() < 700, "{error}");
    }

    #[test]
    fn a_refused_token_and_a_rate_limit_have_different_codes() {
        let token = Secret::new("tok-secret-value");
        let refused = WitError::from_response(401, "bad tok-secret-value", &token, 1);
        assert_eq!(refused.code(), "wit_unauthorized");
        assert!(!refused.to_string().contains("tok-secret-value"));
        let limited = WitError::from_response(429, "slow down", &token, 4);
        assert_eq!(limited.code(), "wit_rate_limited");
    }
}
