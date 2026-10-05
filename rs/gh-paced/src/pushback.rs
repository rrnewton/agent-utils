//! Streaming detector for GitHub pushback in gh's stderr.
//!
//! gh reports server refusals on stderr, for example `HTTP 403: API rate limit exceeded for
//! user ID ...`, `You have exceeded a secondary rate limit`, or `was submitted too quickly`.
//! The scanner sees stderr in chunks as it streams to the terminal, keeps a 4 KiB overlap so a
//! phrase split across two reads is still found, and records which patterns matched.

use crate::config::Config;

const OVERLAP: usize = 4096;

/// What the scanner saw.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scanner {
    carry: Vec<u8>,
    /// `secondary rate limit`.
    pub secondary: bool,
    /// `rate limit` (includes `API rate limit exceeded`).
    pub rate_limit: bool,
    /// `HTTP 429`.
    pub http_429: bool,
    /// `HTTP 403`.
    pub http_403: bool,
    /// The word `abuse`.
    pub abuse: bool,
    /// `submitted too quickly`.
    pub too_quickly: bool,
    /// Largest `Retry-After: N` value seen, seconds.
    pub retry_after: Option<f64>,
}

/// The cooldown a scan calls for.
#[derive(Debug, Clone, PartialEq)]
pub struct Pushback {
    /// Seconds to pause every paced call for this account on this host.
    pub cooldown_secs: f64,
    /// Matched pattern names, comma separated.
    pub reason: String,
}

fn find_all(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    if needle.is_empty() || hay.len() < needle.len() {
        return out;
    }
    for i in 0..=hay.len() - needle.len() {
        if &hay[i..i + needle.len()] == needle {
            out.push(i);
        }
    }
    out
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

impl Scanner {
    /// A scanner that has seen nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan the next chunk of stderr.
    pub fn feed(&mut self, chunk: &[u8]) {
        let mut text = std::mem::take(&mut self.carry);
        text.extend(chunk.iter().map(u8::to_ascii_lowercase));
        self.secondary |= !find_all(&text, b"secondary rate limit").is_empty();
        self.rate_limit |= !find_all(&text, b"rate limit").is_empty();
        self.http_429 |= !find_all(&text, b"http 429").is_empty();
        self.http_403 |= !find_all(&text, b"http 403").is_empty();
        self.too_quickly |= !find_all(&text, b"submitted too quickly").is_empty();
        for at in find_all(&text, b"abuse") {
            let before = at == 0 || !is_word_byte(text[at - 1]);
            let end = at + 5;
            let after = end >= text.len() || !is_word_byte(text[end]);
            if before && after {
                self.abuse = true;
            }
        }
        for at in find_all(&text, b"retry-after:") {
            let digits: String = text[at + 12..]
                .iter()
                .skip_while(|b| **b == b' ' || **b == b'\t')
                .take_while(|b| b.is_ascii_digit())
                .map(|b| *b as char)
                .collect();
            if let Ok(n) = digits.parse::<f64>() {
                self.retry_after = Some(self.retry_after.map_or(n, |old| old.max(n)));
            }
        }
        let keep = text.len().saturating_sub(OVERLAP);
        self.carry = text.split_off(keep);
    }

    /// Matched pattern names.
    pub fn matched(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.secondary {
            out.push("secondary rate limit");
        }
        if self.rate_limit && !self.secondary {
            out.push("rate limit");
        }
        if self.http_429 {
            out.push("HTTP 429");
        }
        if self.http_403 {
            out.push("HTTP 403");
        }
        if self.abuse {
            out.push("abuse");
        }
        if self.too_quickly {
            out.push("submitted too quickly");
        }
        if self.retry_after.is_some() {
            out.push("Retry-After");
        }
        out
    }

    /// The cooldown this output calls for, if any: `max(Retry-After, cooldown_secs)` for a
    /// rate-limit signal, `plain_403_cooldown_secs` for an HTTP 403 with no rate-limit wording.
    pub fn verdict(&self, cfg: &Config) -> Option<Pushback> {
        let limit_signal = self.secondary
            || self.rate_limit
            || self.http_429
            || self.abuse
            || self.too_quickly
            || self.retry_after.is_some();
        let reason = self.matched().join(", ");
        if limit_signal {
            let secs = self.retry_after.unwrap_or(0.0).max(cfg.cooldown_secs);
            return Some(Pushback {
                cooldown_secs: secs,
                reason,
            });
        }
        if self.http_403 {
            return Some(Pushback {
                cooldown_secs: cfg.plain_403_cooldown_secs,
                reason,
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&str]) -> Scanner {
        let mut s = Scanner::new();
        for c in chunks {
            s.feed(c.as_bytes());
        }
        s
    }

    #[test]
    fn detects_github_messages() {
        let cfg = Config::default();
        let s = scan(&["gh: API rate limit exceeded for user ID 1. (HTTP 403)\n"]);
        assert!(s.rate_limit);
        let v = s.verdict(&cfg).expect("pushback");
        assert_eq!(v.cooldown_secs, 900.0);
        let s = scan(&["HTTP 403: You have exceeded a secondary rate limit.\n"]);
        assert!(s.secondary && s.http_403);
        let s = scan(&["... was submitted too quickly ..."]);
        assert!(s.verdict(&cfg).is_some());
        let s = scan(&["triggered an abuse detection mechanism"]);
        assert!(s.abuse);
        let s = scan(&["the word abused or disabuse is not a match"]);
        assert!(!s.abuse);
        assert!(s.verdict(&cfg).is_none());
    }

    #[test]
    fn retry_after_extends_cooldown() {
        let cfg = Config::default();
        let s = scan(&["HTTP 429\nRetry-After: 3600\n"]);
        assert_eq!(s.verdict(&cfg).expect("pushback").cooldown_secs, 3600.0);
        let s = scan(&["HTTP 429\nretry-after:\t60\n"]);
        assert_eq!(s.verdict(&cfg).expect("pushback").cooldown_secs, 900.0);
    }

    #[test]
    fn plain_403_uses_its_own_cooldown() {
        let cfg = Config {
            plain_403_cooldown_secs: 120.0,
            ..Config::default()
        };
        let s = scan(&["HTTP 403: Resource not accessible by integration\n"]);
        let v = s.verdict(&cfg).expect("pushback");
        assert_eq!(v.cooldown_secs, 120.0);
        assert_eq!(v.reason, "HTTP 403");
    }

    #[test]
    fn phrase_split_across_chunks() {
        let s = scan(&["... secondary ra", "te limit ..."]);
        assert!(s.secondary);
        let filler = "x".repeat(10_000);
        let s = scan(&[&filler, "Rate Lim", "it exceeded"]);
        assert!(s.rate_limit);
        let s = scan(&["HTTP 4", "29"]);
        assert!(s.http_429);
    }
}
