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
//! A paginated call prints one block per page, so [`Scanner::new`] reads every block: a block
//! counts once its blank CR LF line arrives, provided every header line in it ended in CR LF;
//! a block interrupted by a line of another shape (a body printed by `--jq`, with plain LF line
//! ends) is dropped. When the output ends inside a block that already holds a CR LF header
//! line, [`Scanner::end_of_stdout`] commits it, so a cut-short response keeps its
//! `Retry-After`.

use crate::config::Config;

const OVERLAP: usize = 4096;

/// Longest stdout line kept while looking for header lines; longer lines are body text.
const MAX_HEADER_LINE: usize = 8192;

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
    /// Partial stdout line carried between [`Scanner::feed_headers`] calls.
    line: Vec<u8>,
    /// The current stdout line is longer than [`MAX_HEADER_LINE`] and is being skipped.
    skipping: bool,
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

    /// Scan the next chunk of `gh api --include` stdout for response header blocks.
    pub fn feed_headers(&mut self, chunk: &[u8]) {
        for &b in chunk {
            if self.closed {
                return;
            }
            if b == b'\n' {
                if !self.skipping {
                    let line = std::mem::take(&mut self.line);
                    self.header_line(&line);
                }
                self.line.clear();
                self.skipping = false;
            } else if !self.skipping {
                if self.line.len() >= MAX_HEADER_LINE {
                    self.line.clear();
                    self.skipping = true;
                    self.started = true;
                    self.end_block(false);
                } else {
                    self.line.push(b);
                }
            }
        }
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
        let number = std::str::from_utf8(value)
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|n| n.is_finite() && *n >= 0.0);
        match (name, number) {
            (b"retry-after", Some(n)) => {
                block.retry_after = Some(block.retry_after.map_or(n, |old| old.max(n)));
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
        out
    }

    /// The cooldown this output calls for, if any: `max(Retry-After, cooldown_secs)` for a
    /// rate-limit signal (wording, HTTP 429, `Retry-After`, or `X-RateLimit-Remaining: 0`),
    /// `plain_403_cooldown_secs` for an HTTP 403 with no rate-limit signal.
    pub fn verdict(&self, cfg: &Config) -> Option<Pushback> {
        let limit_signal = self.secondary
            || self.rate_limit
            || self.http_429
            || self.abuse
            || self.too_quickly
            || self.retry_after.is_some()
            || self.remaining_zero;
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
        // Ended by an over-long line, likewise.
        let long = format!("HTTP/2.0 429 Too Many\r\n{}\r\n{fake}", "x".repeat(10_000));
        let s = single(&[&long]);
        assert!(s.http_429 && !s.http_403, "{s:?}");
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
