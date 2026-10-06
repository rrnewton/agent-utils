//! Streaming detector for GitHub pushback in gh's output.
//!
//! gh reports server refusals on stderr, for example `HTTP 403: API rate limit exceeded for
//! user ID ...`, `You have exceeded a secondary rate limit`, or `was submitted too quickly`.
//! The scanner sees stderr in chunks as it streams to the terminal, keeps a 4 KiB overlap so a
//! phrase split across two reads is still found, and records which patterns matched.
//!
//! `gh api --include` prints the HTTP status line and response headers on STDOUT instead, so for
//! that form the wrapper also feeds stdout through [`Scanner::feed_headers`], which reads only
//! header blocks: the status code, `Retry-After`, and `X-RateLimit-Remaining`. gh writes a block
//! as a `HTTP/<version> <status>` line, then `Name: value` lines each ending in CR LF, then a
//! blank CR LF line.
//!
//! A call that fetches one response (no `--paginate`) uses [`Scanner::single_response`]: gh's
//! status line is then the first stdout line, and only that first block is read. Whatever ends
//! it (the blank line, a line of another shape, an over-long line, or the end of the output)
//! commits what it reported, and nothing after it is read, so body text can never start a
//! cooldown, whatever its line ends.
//!
//! A header line longer than [`MAX_HEADER_LINE`] is not kept, so a `Retry-After` in it cannot be
//! read (and for a call without `--paginate` the line also ends the block). In a 403 or 429 block
//! such a line therefore starts a cooldown with no end time ([`NO_END`]) instead of the shorter
//! one the rest of the block would give; in any other block it ends the block. A status line that
//! long (HTTP/1.1 lets the server choose the reason phrase) still opens its block, read from its
//! first [`MAX_HEADER_LINE`] bytes. On stderr, a `Retry-After` value that runs on past the
//! [`OVERLAP`] bytes kept between chunks, so that its digits can no longer be read whole, also
//! gives a cooldown with no end time.
//!
//! A paginated call prints one block per page, so [`Scanner::new`] reads every block: a block
//! counts once its blank CR LF line arrives, provided every header line in it ended in CR LF;
//! a block interrupted by a line of another shape (a body printed by `--jq`, with plain LF line
//! ends) is dropped. When the output ends inside a block that already holds a CR LF header
//! line, [`Scanner::end_of_stdout`] commits it, so a cut-short response keeps its
//! `Retry-After`.

use crate::config::Config;

/// Bytes of stderr kept between chunks, so a phrase split across two reads is still found.
pub const OVERLAP: usize = 4096;

/// Longest stdout line kept while looking for header lines. A longer line is not kept: in a
/// 403 or 429 block it gives a cooldown with no end time (module docs).
pub const MAX_HEADER_LINE: usize = 8192;

/// Whether a response with this status is one GitHub sends `Retry-After` with: 403 and 429.
fn can_ask_for_a_wait(status: u16) -> bool {
    matches!(status, 403 | 429)
}

/// Longest `Retry-After` gh-paced counts down, seconds (365 days). Any value up to this is
/// honoured in full. A longer one, including one too long to represent, is never shortened to
/// fit: it starts a cooldown with no end time ([`NO_END`]), and every paced call is refused until
/// a person removes that cooldown from the state files.
pub const MAX_RETRY_AFTER_SECS: f64 = 31_536_000.0;

/// The cooldown length, and the cooldown end time (`until`), of a cooldown with no end time.
/// Adding it to any clock reading gives the same value, so such a cooldown never expires.
pub const NO_END: f64 = f64::MAX;

/// Whether a cooldown ending at `until` has no end time (see [`NO_END`]).
pub fn has_no_end(until: f64) -> bool {
    until >= NO_END
}

/// A `Retry-After` value in seconds: `None` unless it is a non-negative number. A number too
/// large to represent reads as infinity, which [`Scanner::verdict`] turns into [`NO_END`].
fn retry_after_secs(text: &str) -> Option<f64> {
    let n = text.parse::<f64>().ok()?;
    if n.is_nan() || n < 0.0 {
        return None;
    }
    Some(n)
}

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
    /// A response header block reported `X-RateLimit-Remaining: 0`.
    pub remaining_zero: bool,
    /// A 403 or 429 response block held a header line longer than [`MAX_HEADER_LINE`], so the
    /// wait it asked for is not known.
    pub overlong_header: bool,
    /// A `Retry-After` value on stderr ran on past the bytes kept between chunks, so it could not
    /// be read whole.
    pub unread_retry_after: bool,
    /// Partial stdout line carried between [`Scanner::feed_headers`] calls.
    line: Vec<u8>,
    /// The current stdout line is longer than [`MAX_HEADER_LINE`] and is being skipped.
    skipping: bool,
    /// The last byte of the line being skipped was a CR.
    skipped_cr: bool,
    /// The line being skipped is a header line of the pending block of a paginated call, whose
    /// line end decides whether the block is still gh's header format.
    skipping_header: bool,
    /// The stdout header block being read, not yet confirmed by its blank CR LF line.
    pending: Option<HeaderBlock>,
    /// Only the first stdout block is a response header block (see [`Scanner::single_response`]).
    single_response: bool,
    /// At least one stdout line has been read.
    started: bool,
    /// No further stdout header block will be read.
    closed: bool,
}

/// What one stdout header block reported, kept until the block is confirmed.
#[derive(Debug, Clone, Default, PartialEq)]
struct HeaderBlock {
    status: u16,
    retry_after: Option<f64>,
    remaining_zero: bool,
    /// Header lines read that ended in CR LF, as gh writes them.
    crlf_headers: u32,
    /// A header line longer than [`MAX_HEADER_LINE`] was seen in this 403 or 429 block.
    overlong: bool,
}

/// The cooldown a scan calls for.
#[derive(Debug, Clone, PartialEq)]
pub struct Pushback {
    /// Seconds to pause every paced call for this account on this host; [`NO_END`] when GitHub
    /// asked for longer than [`MAX_RETRY_AFTER_SECS`].
    pub cooldown_secs: f64,
    /// Matched pattern names, comma separated.
    pub reason: String,
}

impl Pushback {
    /// When the cooldown ends, for a pushback seen at `now`: [`NO_END`] for a cooldown with no
    /// end time.
    pub fn until(&self, now: f64) -> f64 {
        if has_no_end(self.cooldown_secs) {
            NO_END
        } else {
            now + self.cooldown_secs
        }
    }

    /// Whether this cooldown has no end time.
    pub fn has_no_end(&self) -> bool {
        has_no_end(self.cooldown_secs)
    }
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

/// Drop ANSI escape sequences (gh colours header names when stdout is a terminal).
fn strip_ansi(line: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        if line[i] == 0x1b {
            i += 1;
            if line.get(i) == Some(&b'[') {
                i += 1;
                while i < line.len() && !(0x40..=0x7e).contains(&line[i]) {
                    i += 1;
                }
            }
            i += 1;
        } else {
            out.push(line[i]);
            i += 1;
        }
    }
    out
}

/// The status code of a lower-cased `http/<version> <code> ...` status line.
fn http_status(lower: &[u8]) -> Option<u16> {
    let rest = lower.strip_prefix(b"http/")?;
    let space = rest.iter().position(|b| *b == b' ')?;
    let version = &rest[..space];
    if version.is_empty() || !version.iter().all(|b| b.is_ascii_digit() || *b == b'.') {
        return None;
    }
    let after = &rest[space + 1..];
    if after.len() < 3 || !after[..3].iter().all(u8::is_ascii_digit) {
        return None;
    }
    if after.get(3).is_some_and(|b| *b != b' ') {
        return None;
    }
    std::str::from_utf8(&after[..3]).ok()?.parse().ok()
}

impl Scanner {
    /// A scanner that has seen nothing, reading every stdout header block (`--paginate`).
    pub fn new() -> Self {
        Self::default()
    }

    /// A scanner for a call that receives one response: only a header block that starts on the
    /// first stdout line is read, and nothing after it.
    pub fn single_response() -> Self {
        Self {
            single_response: true,
            ..Self::default()
        }
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
            let value = &text[at + 12..];
            let blanks = value
                .iter()
                .take_while(|b| **b == b' ' || **b == b'\t')
                .count();
            let len = value[blanks..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
            // A value still running at the end of what is held is read again with the next
            // chunk, while its name is within the OVERLAP bytes kept. Past that, the rest of the
            // number would arrive without its name, so the wait it asks for is not known.
            if blanks + len == value.len() && text.len() - at > OVERLAP {
                self.unread_retry_after = true;
            }
            let digits = String::from_utf8_lossy(&value[blanks..blanks + len]);
            if let Some(n) = retry_after_secs(&digits) {
                self.retry_after = Some(self.retry_after.map_or(n, |old| old.max(n)));
            }
        }
        let keep = text.len().saturating_sub(OVERLAP);
        self.carry = text.split_off(keep);
    }

    /// Scan the next chunk of `gh api --include` stdout for response header blocks.
    pub fn feed_headers(&mut self, chunk: &[u8]) {
        for &b in chunk {
            if self.closed {
                return;
            }
            if b == b'\n' {
                if self.skipping {
                    self.end_overlong_line();
                } else {
                    let line = std::mem::take(&mut self.line);
                    self.header_line(&line);
                }
                self.line.clear();
                self.skipping = false;
            } else if self.skipping {
                self.skipped_cr = b == b'\r';
            } else if self.line.len() >= MAX_HEADER_LINE {
                self.skipping = true;
                self.skipped_cr = b == b'\r';
                self.start_overlong_line();
                self.line.clear();
            } else {
                self.line.push(b);
            }
        }
    }

    /// The current stdout line has just passed [`MAX_HEADER_LINE`]; `self.line` holds its first
    /// [`MAX_HEADER_LINE`] bytes. The rest of the line is skipped.
    fn start_overlong_line(&mut self) {
        let first = !self.started;
        self.started = true;
        let prefix = strip_ansi(&self.line);
        let lower = prefix.trim_ascii_start().to_ascii_lowercase();
        if let Some(status) = http_status(&lower) {
            // A status line with a long reason phrase opens its block as a short one does; the
            // reason phrase holds no header, so nothing in the block is lost.
            if self.single_response && !first {
                self.end_block(false);
                return;
            }
            self.pending = Some(HeaderBlock {
                status,
                ..HeaderBlock::default()
            });
            return;
        }
        match self.pending.as_mut() {
            Some(block) if can_ask_for_a_wait(block.status) => {
                // The line, or one after it, may hold the Retry-After: the wait is not known.
                block.overlong = true;
                if self.single_response {
                    // gh's own block: commit it now; nothing after it can shorten the cooldown.
                    self.end_block(false);
                } else {
                    // A paginated block counts only in gh's header format: decided at the CR LF.
                    self.skipping_header = true;
                }
            }
            // Any other block ends here, as before.
            _ => self.end_block(false),
        }
    }

    /// The LF of a skipped over-long line has arrived.
    fn end_overlong_line(&mut self) {
        if std::mem::take(&mut self.skipping_header) {
            if self.skipped_cr {
                if let Some(block) = self.pending.as_mut() {
                    block.crlf_headers += 1;
                }
            } else {
                // Not gh's header format: the block was body text and is dropped, as for a
                // short line.
                self.end_block(false);
            }
        }
        self.skipped_cr = false;
    }

    /// The stdout output has ended (or will no longer be read). A block it interrupted is
    /// committed when its headers were verified: always for the first block of a
    /// single-response call, and otherwise once it holds at least one CR LF header line.
    ///
    /// An unterminated last line is handled by mode. In a single-response call the open block
    /// is gh's own (it began on the first stdout line), so the last line is read as a header
    /// line whether or not its CR LF arrived: `HTTP/2.0 429 ...` alone, or a final
    /// `Retry-After: 3600`, still counts. A line that is not a header ends the block, which
    /// keeps what it already reported. In a paginated call the line may be body text, so it is
    /// ignored rather than allowed to spoil the block.
    pub fn end_of_stdout(&mut self) {
        if self.single_response && !self.closed && !self.skipping && !self.line.is_empty() {
            let mut line = std::mem::take(&mut self.line);
            if line.last() != Some(&b'\r') {
                line.push(b'\r');
            }
            self.header_line(&line);
        }
        self.line.clear();
        self.skipping = false;
        self.skipping_header = false;
        if let Some(block) = self.pending.take() {
            if self.single_response || block.crlf_headers > 0 {
                self.commit(&block);
            }
        }
        self.closed = true;
    }

    /// The current block ended without its blank CR LF line. In single-response mode the
    /// block is gh's own (it began on the first stdout line), so what it reported is kept;
    /// otherwise it may have been body text and is dropped.
    fn end_block(&mut self, confirmed: bool) {
        if let Some(block) = self.pending.take() {
            if confirmed || self.single_response {
                self.commit(&block);
            }
        }
        if self.single_response {
            self.closed = true;
        }
    }

    /// One stdout line, without its LF. gh ends header lines and the blank line after them with
    /// CR LF, so `crlf` (a CR before the LF) is part of what makes a line a header line.
    fn header_line(&mut self, raw: &[u8]) {
        let first = !self.started;
        self.started = true;
        let crlf = raw.last() == Some(&b'\r');
        let line = strip_ansi(raw);
        let line = line.trim_ascii();
        if line.is_empty() {
            self.end_block(crlf);
            return;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(status) = http_status(&lower) {
            if self.single_response && !first {
                // Only the first line can open the response's block.
                self.end_block(false);
                return;
            }
            // A new block starts; an unconfirmed earlier one is dropped.
            self.pending = Some(HeaderBlock {
                status,
                ..HeaderBlock::default()
            });
            return;
        }
        let Some(block) = self.pending.as_mut() else {
            if self.single_response {
                self.closed = true;
            }
            return;
        };
        let colon = lower.iter().position(|b| *b == b':');
        let (true, Some(colon)) = (crlf, colon) else {
            // Not gh's header format: the block has ended (or this was body text).
            self.end_block(false);
            return;
        };
        block.crlf_headers += 1;
        let name = lower[..colon].trim_ascii();
        let value = lower[colon + 1..].trim_ascii();
        let text = std::str::from_utf8(value).ok();
        let number = text
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|n| n.is_finite() && *n >= 0.0);
        match (name, number) {
            (b"retry-after", _) => {
                if let Some(n) = text.and_then(retry_after_secs) {
                    block.retry_after = Some(block.retry_after.map_or(n, |old| old.max(n)));
                }
            }
            (b"x-ratelimit-remaining", Some(0.0)) => block.remaining_zero = true,
            _ => {}
        }
    }

    /// Record a confirmed header block.
    fn commit(&mut self, block: &HeaderBlock) {
        match block.status {
            429 => self.http_429 = true,
            403 => self.http_403 = true,
            _ => {}
        }
        if let Some(n) = block.retry_after {
            self.retry_after = Some(self.retry_after.map_or(n, |old| old.max(n)));
        }
        self.remaining_zero |= block.remaining_zero;
        self.overlong_header |= block.overlong;
    }

    /// A value that changes whenever the pushback signals seen so far change (a new pattern, or
    /// a larger `Retry-After`), so a caller can tell when a verdict may have changed.
    pub fn signal_key(&self) -> String {
        format!("{:?} {:?}", self.matched(), self.retry_after)
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
        if self.remaining_zero {
            out.push("X-RateLimit-Remaining: 0");
        }
        if self.overlong_header {
            out.push("over-long header line");
        }
        if self.unread_retry_after {
            out.push("unreadable Retry-After");
        }
        out
    }

    /// The cooldown this output calls for, if any: `max(Retry-After, cooldown_secs)` for a
    /// rate-limit signal (wording, HTTP 429, `Retry-After`, or `X-RateLimit-Remaining: 0`),
    /// `plain_403_cooldown_secs` for an HTTP 403 with no rate-limit signal. A `Retry-After`
    /// longer than [`MAX_RETRY_AFTER_SECS`], or one that could not be read (an over-long header
    /// line in a 403 or 429 response, or a stderr value cut off), gives a cooldown with no
    /// end time ([`NO_END`]), never a shorter one.
    pub fn verdict(&self, cfg: &Config) -> Option<Pushback> {
        let reason = self.matched().join(", ");
        if self.overlong_header || self.unread_retry_after {
            let why = if self.overlong_header {
                format!(
                    "a header line of a 403 or 429 response is longer than the \
                     {MAX_HEADER_LINE} bytes gh-paced reads"
                )
            } else {
                format!(
                    "a Retry-After value on stderr runs on past the {OVERLAP} bytes gh-paced \
                     keeps"
                )
            };
            return Some(Pushback {
                cooldown_secs: NO_END,
                reason: format!(
                    "{reason}; {why}, so the wait GitHub asked for is not known and this \
                     cooldown has no end time"
                ),
            });
        }
        let limit_signal = self.secondary
            || self.rate_limit
            || self.http_429
            || self.abuse
            || self.too_quickly
            || self.retry_after.is_some()
            || self.remaining_zero;
        if limit_signal {
            let secs = self.retry_after.unwrap_or(0.0).max(cfg.cooldown_secs);
            if secs > MAX_RETRY_AFTER_SECS {
                return Some(Pushback {
                    cooldown_secs: NO_END,
                    reason: format!(
                        "{reason}; Retry-After is longer than the {} s (365 days) gh-paced \
                         counts down, so this cooldown has no end time",
                        MAX_RETRY_AFTER_SECS as u64
                    ),
                });
            }
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

    /// A long `Retry-After` is never shortened. Up to 365 days it is honoured in full (two days
    /// once read as one day); a longer one, or one too large to represent (400 nines parse to
    /// infinity), gives a cooldown with no end time, on stderr and in headers.
    #[test]
    fn a_long_retry_after_is_never_shortened() {
        let cfg = Config::default();
        let s = headers(&["HTTP/2.0 429 Too Many Requests\r\nRetry-After: 172800\r\n\r\n"]);
        assert_eq!(s.retry_after, Some(172_800.0));
        let v = s.verdict(&cfg).expect("pushback");
        assert_eq!(v.cooldown_secs, 172_800.0);
        assert!(!v.has_no_end());
        assert_eq!(v.until(1000.0), 1000.0 + 172_800.0);
        let s = scan(&["HTTP 429\nRetry-After: 31536000\n"]);
        assert_eq!(
            s.verdict(&cfg).expect("pushback").cooldown_secs,
            MAX_RETRY_AFTER_SECS
        );
        let nines = "9".repeat(400);
        for s in [
            scan(&["HTTP 429\nRetry-After: 31536001\n"]),
            scan(&[&format!("HTTP 429\nRetry-After: {nines}\n")]),
            headers(&[&format!(
                "HTTP/2.0 429 Too Many Requests\r\nRetry-After: {nines}\r\n\r\n"
            )]),
            headers(&["HTTP/2.0 429 Too Many Requests\r\nRetry-After: 1e400\r\n\r\n"]),
        ] {
            let v = s.verdict(&cfg).expect("pushback");
            assert!(v.has_no_end(), "{s:?}");
            assert_eq!(v.cooldown_secs, NO_END);
            assert!(v.reason.contains("no end time"), "{}", v.reason);
            // The end time is the same, finite value whenever the pushback is seen.
            assert_eq!(v.until(1_791_126_252.0), NO_END);
            assert!(has_no_end(v.until(0.0)) && v.until(0.0).is_finite());
        }
        // An ordinary value is unchanged.
        let s = scan(&["HTTP 429\nRetry-After: 3600\n"]);
        assert_eq!(s.retry_after, Some(3600.0));
    }

    #[test]
    fn plain_403_uses_its_own_cooldown() {
        // 1800 s: a value a configuration file may set (the floor is 900 s).
        let cfg = Config {
            plain_403_cooldown_secs: 1800.0,
            ..Config::default()
        };
        let s = scan(&["HTTP 403: Resource not accessible by integration\n"]);
        let v = s.verdict(&cfg).expect("pushback");
        assert_eq!(v.cooldown_secs, 1800.0);
        assert_eq!(v.reason, "HTTP 403");
        assert_eq!(
            Config::default().plain_403_cooldown_secs,
            900.0,
            "the default plain-403 cooldown is the 15-minute floor"
        );
    }

    /// Feed a whole stdout stream, then its end, to `s`.
    fn stream(mut s: Scanner, chunks: &[&str]) -> Scanner {
        for c in chunks {
            s.feed_headers(c.as_bytes());
        }
        s.end_of_stdout();
        s
    }

    /// A paginated call's stdout: every header block is read.
    fn headers(chunks: &[&str]) -> Scanner {
        stream(Scanner::new(), chunks)
    }

    /// A single-response call's stdout: only the block on the first line is read.
    fn single(chunks: &[&str]) -> Scanner {
        stream(Scanner::single_response(), chunks)
    }

    /// `gh api --include` puts the status line and headers on stdout; a long `Retry-After`
    /// there must set the cooldown, not the 900 s floor.
    #[test]
    fn include_headers_on_stdout_are_read() {
        let cfg = Config::default();
        let s = headers(&[
            "HTTP/2.0 403 Forbidden\r\nContent-Type: application/json\r\nRetry-After: 3600\r\n",
            "X-Ratelimit-Remaining: 0\r\n\r\n{\"message\":\"You have exceeded a secondary rate limit\"}\n",
        ]);
        assert!(s.http_403 && s.remaining_zero);
        assert_eq!(s.retry_after, Some(3600.0));
        assert_eq!(s.verdict(&cfg).expect("pushback").cooldown_secs, 3600.0);
        // Split mid-line, coloured header names (stdout is a terminal), HTTP/1.1.
        let s = headers(&[
            "HTTP/1.1 429 Too Many Requests\r\n\u{1b}[1;34mRetry-",
            "After\u{1b}[m: 120\r\n\r\n",
        ]);
        assert!(s.http_429);
        assert_eq!(s.retry_after, Some(120.0));
        assert_eq!(s.verdict(&cfg).expect("pushback").cooldown_secs, 900.0);
        // Remaining 0 on a successful response still pauses: the hourly pool is empty.
        let s = headers(&["HTTP/2.0 200 OK\r\nX-Ratelimit-Remaining: 0\r\n\r\n[]\n"]);
        assert_eq!(s.verdict(&cfg).expect("pushback").cooldown_secs, 900.0);
        // A healthy response is quiet.
        let s = headers(&["HTTP/2.0 200 OK\r\nX-Ratelimit-Remaining: 4999\r\n\r\n[]\n"]);
        assert!(s.verdict(&cfg).is_none());
    }

    /// Body text after the blank line is never read as headers, even when it looks like one.
    #[test]
    fn include_body_text_is_not_a_header() {
        let cfg = Config::default();
        let s = headers(&[
            "HTTP/2.0 200 OK\r\nX-Ratelimit-Remaining: 10\r\n\r\n",
            "Retry-After: 99999\nx-ratelimit-remaining: 0\nsee HTTP/2.0 403 in the docs\n",
        ]);
        assert_eq!(s.retry_after, None);
        assert!(!s.remaining_zero && !s.http_403);
        assert!(s.verdict(&cfg).is_none());
        // An over-long line is skipped and ends the header block.
        let long = format!(
            "HTTP/2.0 200 OK\r\n{}\r\nRetry-After: 50\r\n\r\n",
            "x".repeat(10_000)
        );
        let s = headers(&[&long]);
        assert_eq!(s.retry_after, None);
    }

    /// `gh api --include --jq .body` prints the real header block, then the body through jq with
    /// plain LF line ends. A body that holds a status line and headers at line starts is still
    /// body text: it must not start a cooldown.
    #[test]
    fn jq_body_that_looks_like_headers_is_not_a_header() {
        let cfg = Config::default();
        let s = headers(&[
            "HTTP/2.0 200 OK\nX-Ratelimit-Remaining: 4000\r\n\r\n",
            "HTTP/2.0 403 Forbidden\nRetry-After: 99999\nX-Ratelimit-Remaining: 0\n\n",
        ]);
        assert!(!s.http_403 && !s.remaining_zero, "{s:?}");
        assert_eq!(s.retry_after, None);
        assert!(s.verdict(&cfg).is_none());
        // One header line without its CR spoils the block, even if the rest have it.
        let s = headers(&["HTTP/2.0 429 Too Many Requests\nRetry-After: 60\r\nVia: x\n\r\n"]);
        assert!(s.verdict(&cfg).is_none(), "{s:?}");
        // A block cut short by the end of the output, after a CR LF header line, still counts,
        // with the Retry-After it had already reported. (Before the end-of-output commit this
        // block was dropped and gave no verdict.)
        let s = headers(&["HTTP/2.0 429 Too Many Requests\nRetry-After: 3600\r\n"]);
        assert!(s.http_429, "{s:?}");
        assert_eq!(s.verdict(&cfg).expect("pushback").cooldown_secs, 3600.0);
        // So does one cut inside its next header line: the unfinished line is ignored.
        let s = headers(&["HTTP/2.0 429 Too Many Requests\nRetry-After: 3600\r\nX-Rate"]);
        assert_eq!(s.verdict(&cfg).expect("pushback").cooldown_secs, 3600.0);
        // A status-shaped last line of body text, with no CR LF header line after it, does not.
        let s = headers(&["[]\nHTTP/2.0 429 Too Many Requests\n"]);
        assert!(s.verdict(&cfg).is_none(), "{s:?}");
        // The exact form gh writes does count: an LF status line, CR LF headers and blank line.
        let s = headers(&["HTTP/2.0 429 Too Many Requests\nRetry-After: 1200\r\n\r\n[]"]);
        assert!(s.http_429);
        assert_eq!(s.verdict(&cfg).expect("pushback").cooldown_secs, 1200.0);
    }

    /// Without `--paginate` gh prints one response: its header block starts on the first stdout
    /// line and everything after it is body. Body text is never read, whatever its line ends.
    #[test]
    fn single_response_reads_only_the_first_block() {
        let cfg = Config::default();
        let healthy = "HTTP/2.0 200 OK\nX-Ratelimit-Remaining: 10\r\n\r\n";
        let fake = "HTTP/2.0 403 Forbidden\nRetry-After: 99999\r\nX-Ratelimit-Remaining: 0\r\n\r\n";
        // A body holding a CR LF status line and headers (a string with embedded CR LF printed
        // by --jq or --template) does not start a cooldown.
        let s = single(&[healthy, fake]);
        assert!(s.verdict(&cfg).is_none(), "{s:?}");
        // The paginated reader does read it: there, a later block is the next page's headers.
        assert!(headers(&[healthy, fake]).verdict(&cfg).is_some());
        // A body that comes first (no status line on line 1) is never read either.
        let s = single(&["[]\n", fake]);
        assert!(s.verdict(&cfg).is_none(), "{s:?}");
        // The real block counts, with its Retry-After.
        let s = single(&[
            "HTTP/2.0 429 Too Many Requests\nRetry-After: 1200\r\n\r\n",
            fake,
        ]);
        assert!(s.http_429 && !s.http_403, "{s:?}");
        assert_eq!(s.verdict(&cfg).expect("pushback").cooldown_secs, 1200.0);
        // Cut short by the end of the output, it still counts, even with no header line yet.
        let s = single(&["HTTP/2.0 429 Too Many Requests\nRetry-After: 3600\r\nX-Rat"]);
        assert_eq!(s.verdict(&cfg).expect("pushback").cooldown_secs, 3600.0);
        let s = single(&["HTTP/2.0 429 Too Many Requests\n"]);
        assert!(s.http_429, "{s:?}");
        // Ended by a line of another shape, it keeps what it reported, and nothing later counts.
        let s = single(&[
            "HTTP/2.0 429 Too Many\nRetry-After: 1200\r\nodd line\n",
            fake,
        ]);
        assert_eq!(s.verdict(&cfg).expect("pushback").cooldown_secs, 1200.0);
        assert!(!s.http_403 && !s.remaining_zero, "{s:?}");
        // Ended by an over-long line, nothing later counts; the Retry-After that line may hold
        // is not known, so the cooldown has no end time.
        let long = format!("HTTP/2.0 429 Too Many\r\n{}\r\n{fake}", "x".repeat(10_000));
        let s = single(&[&long]);
        assert!(s.http_429 && !s.http_403, "{s:?}");
        assert!(s.verdict(&cfg).expect("pushback").has_no_end(), "{s:?}");
    }

    /// An over-long header line in a 403 or 429 response never shortens the wait the server
    /// asked for: the `Retry-After` it may hold cannot be read, so the cooldown has no end time,
    /// with and without `--paginate`, and whether the long line is the `Retry-After` itself (two
    /// days behind 9,000 leading zeros) or comes before a valid one.
    #[test]
    fn an_overlong_header_line_never_shortens_a_wait() {
        let cfg = Config::default();
        let zeros = "0".repeat(9000);
        let filler = "x".repeat(9000);
        for status in ["429 Too Many Requests", "403 Forbidden"] {
            let padded = format!("HTTP/2.0 {status}\nRetry-After: {zeros}172800\r\n\r\n[]\n");
            let before =
                format!("HTTP/2.0 {status}\nX-Long: {filler}\r\nRetry-After: 172800\r\n\r\n[]\n");
            for text in [&padded, &before] {
                // Split into 1000-byte chunks as well as whole.
                let chunks: Vec<&str> = text
                    .as_bytes()
                    .chunks(1000)
                    .map(|c| std::str::from_utf8(c).expect("ascii"))
                    .collect();
                for s in [
                    single(&[text]),
                    headers(&[text]),
                    single(&chunks),
                    headers(&chunks),
                ] {
                    assert!(s.overlong_header, "{status}: {s:?}");
                    let v = s.verdict(&cfg).expect("pushback");
                    assert!(v.has_no_end(), "{status}: {v:?}");
                    assert_eq!(v.cooldown_secs, NO_END);
                    assert!(v.reason.contains("no end time"), "{}", v.reason);
                    assert!(v.reason.contains("8192 bytes"), "{}", v.reason);
                    assert!(s.matched().contains(&"over-long header line"), "{s:?}");
                }
            }
        }
        // A Retry-After line that fits (8,192 bytes with its CR) is read in full; one byte more
        // and it cannot be.
        let line = |zeros: usize| format!("Retry-After: {}172800\r", "0".repeat(zeros));
        assert_eq!(line(8172).len(), 8192);
        for (line, no_end) in [(line(8172), false), (line(8173), true)] {
            let text = format!("HTTP/2.0 429 Too Many Requests\n{line}\n\r\n");
            for s in [single(&[&text]), headers(&[&text])] {
                let v = s.verdict(&cfg).expect("pushback");
                assert_eq!(v.has_no_end(), no_end, "{} bytes: {v:?}", line.len());
                if !no_end {
                    assert_eq!(v.cooldown_secs, 172_800.0);
                }
            }
        }
        // A long status line (HTTP/1.1 lets the server choose the reason phrase) holds no
        // header: its block is still read, and its Retry-After is honoured exactly.
        let text = format!("HTTP/1.1 429 {filler}\nRetry-After: 172800\r\n\r\n[]\n");
        for s in [single(&[&text]), headers(&[&text])] {
            assert!(s.http_429 && !s.overlong_header, "{s:?}");
            let v = s.verdict(&cfg).expect("pushback");
            assert_eq!(v.cooldown_secs, 172_800.0, "{v:?}");
        }
        // Any other status: an over-long line ends the block, which starts no cooldown.
        let text = format!("HTTP/2.0 200 OK\nX-Long: {filler}\r\nRetry-After: 172800\r\n\r\n");
        for s in [single(&[&text]), headers(&[&text])] {
            assert!(s.verdict(&cfg).is_none(), "{s:?}");
        }
        // Body text of a paginated call that looks like a 429 block, with an over-long line
        // that does not end in CR LF, is not a header block.
        let text = format!("[]\nHTTP/2.0 429 Too Many Requests\n{filler}\nRetry-After: 1\r\n");
        assert!(headers(&[&text]).verdict(&cfg).is_none());
        // Body text after a complete block is never read as part of it.
        let text = format!("HTTP/2.0 429 Too Many Requests\nRetry-After: 1200\r\n\r\n{filler}\n");
        for s in [single(&[&text]), headers(&[&text])] {
            assert_eq!(s.verdict(&cfg).expect("pushback").cooldown_secs, 1200.0);
        }
    }

    /// On stderr a `Retry-After` value is read whole while its name is within the bytes kept
    /// between chunks. A value that runs on past them (two days behind 9,000 leading zeros,
    /// arriving in 1000-byte reads) cannot be, so the cooldown has no end time.
    #[test]
    fn an_unreadable_stderr_retry_after_never_shortens_a_wait() {
        let cfg = Config::default();
        let text = format!("HTTP 429\nRetry-After: {}172800\n", "0".repeat(9000));
        let s = scan(&[&text]);
        assert!(!s.unread_retry_after, "{s:?}");
        assert_eq!(s.verdict(&cfg).expect("pushback").cooldown_secs, 172_800.0);
        let chunks: Vec<&str> = text
            .as_bytes()
            .chunks(1000)
            .map(|c| std::str::from_utf8(c).expect("ascii"))
            .collect();
        let s = scan(&chunks);
        assert!(s.unread_retry_after, "{s:?}");
        let v = s.verdict(&cfg).expect("pushback");
        assert!(v.has_no_end(), "{v:?}");
        assert!(v.reason.contains("no end time"), "{}", v.reason);
        assert!(s.matched().contains(&"unreadable Retry-After"), "{s:?}");
        // A short value split across two reads is still read whole.
        let s = scan(&["HTTP 429\nRetry-After: 17", "2800\n"]);
        assert!(!s.unread_retry_after, "{s:?}");
        assert_eq!(s.verdict(&cfg).expect("pushback").cooldown_secs, 172_800.0);
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

    /// A single-response call's header block is gh's own, so a last line whose LF never
    /// arrived is still read: a final `Retry-After` sets the cooldown, and a status line alone
    /// still reports the 429.
    #[test]
    fn single_response_reads_an_unterminated_last_line() {
        let cfg = Config::default();
        let s = single(&["HTTP/2.0 429 Too Many Requests\nRetry-After: 3600"]);
        assert_eq!(s.retry_after, Some(3600.0), "{s:?}");
        assert_eq!(s.verdict(&cfg).expect("pushback").cooldown_secs, 3600.0);
        let s = single(&["HTTP/2.0 429 Too Many Requests"]);
        assert!(s.http_429, "{s:?}");
        assert_eq!(s.verdict(&cfg).expect("pushback").cooldown_secs, 900.0);
        let s = single(&["HTTP/2.0 403 Forbidden\nX-Ratelimit-Remaining: 0"]);
        assert!(s.remaining_zero, "{s:?}");
        // Body text after the block is never read, terminated or not.
        let s = single(&["HTTP/2.0 200 OK\nX-Ratelimit-Remaining: 10\r\n\r\nRetry-After: 99999"]);
        assert!(s.verdict(&cfg).is_none(), "{s:?}");
        // Nor is a body that came first.
        let s = single(&["[]\nHTTP/2.0 429 Too Many Requests"]);
        assert!(s.verdict(&cfg).is_none(), "{s:?}");
        // The paginated reader still ignores an unterminated last line (it may be body text).
        let s = headers(&["[]\nHTTP/2.0 429 Too Many Requests"]);
        assert!(s.verdict(&cfg).is_none(), "{s:?}");
    }
}
