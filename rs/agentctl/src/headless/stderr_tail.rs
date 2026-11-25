//! A bounded tail of the harness's stderr (`#2248 agentctl-headless-wrapper-keeps-harness-stderr`).
//!
//! The wrapper pipes the harness's stderr through [`spawn`]: a reader thread copies every byte
//! on to the wrapper's own stderr, so the journal keeps all of it as it did when stderr was
//! inherited, and keeps the last lines in a [`Tail`] bounded by line count and bytes. The work
//! per line is constant and nothing grows with the harness's output. On every harness exit the
//! wrapper puts [`Shared::finish`] on `lifecycle.harness_exited` as `stderr_tail`:
//!
//! ```json
//! "stderr_tail": {"lines": ["usage: ..."], "omitted_lines": 0}
//! ```
//!
//! `lines` are oldest first, masked ([`claude::mask_output_line`]) and cut to
//! [`MAX_LINE_BYTES`], at most [`MAX_LINES`] and [`MAX_BYTES`] in all; `omitted_lines` counts
//! the earlier lines that fell out of the bound. The field is absent when the harness wrote
//! nothing to stderr, so a reader that does not know it ignores it.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use super::claude;

/// Most lines kept per exit.
pub const MAX_LINES: usize = 20;
/// Most bytes of one kept line, its cut mark included.
pub const MAX_LINE_BYTES: usize = 512;
/// Most bytes of all kept lines together.
pub const MAX_BYTES: usize = 4096;
/// Bytes of one line read before the rest of it is skipped: enough to know it was cut.
const READ_LINE_BYTES: usize = MAX_LINE_BYTES + 8;

/// The last lines of a stream, within [`MAX_LINES`] and [`MAX_BYTES`].
#[derive(Debug, Default)]
pub struct Tail {
    lines: VecDeque<String>,
    bytes: usize,
    omitted: u64,
}

impl Tail {
    /// Keep one line, masked and cut, letting the oldest go past the bounds. Blank lines are
    /// not kept: they would only spend the line budget.
    pub fn push(&mut self, raw: &str) {
        let text = raw.trim_end_matches(['\r', '\n']);
        if text.trim().is_empty() {
            return;
        }
        let line = if text.contains("PRIVATE KEY") {
            "***".to_owned()
        } else {
            cut(&claude::mask_output_line(text), MAX_LINE_BYTES)
        };
        self.bytes += line.len();
        self.lines.push_back(line);
        while self.lines.len() > MAX_LINES || self.bytes > MAX_BYTES {
            let Some(oldest) = self.lines.pop_front() else {
                break;
            };
            self.bytes -= oldest.len();
            self.omitted += 1;
        }
    }

    /// The `stderr_tail` event field, or `None` when nothing was kept.
    #[must_use]
    pub fn to_value(&self) -> Option<Value> {
        if self.lines.is_empty() && self.omitted == 0 {
            return None;
        }
        Some(json!({"lines": self.lines, "omitted_lines": self.omitted}))
    }
}

/// `text` cut to at most `limit` bytes on a character boundary, a cut marked with `…`.
fn cut(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit - '…'.len_utf8();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// One harness's stderr tail, shared between its reader thread and the wrapper.
#[derive(Debug, Default)]
pub struct Shared {
    /// The tail, and whether the reader reached the end of the stream.
    state: Mutex<(Tail, bool)>,
    ended: Condvar,
}

impl Shared {
    fn push(&self, raw: &[u8]) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.0.push(&String::from_utf8_lossy(raw));
    }

    fn end(&self) {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).1 = true;
        self.ended.notify_all();
    }

    /// The tail once the reader reached the end of the stream, waiting at most `wait` for it
    /// (a process outside the harness's group can hold the stream open): `None` when the
    /// harness wrote nothing.
    pub fn finish(&self, wait: Duration) -> Option<Value> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let (state, _) = self
            .ended
            .wait_timeout_while(state, wait, |(_, ended)| !*ended)
            .unwrap_or_else(PoisonError::into_inner);
        state.0.to_value()
    }
}

/// Read `source` to its end on a thread of its own: every byte goes on to `forward` (the
/// wrapper's stderr) and the lines into the returned tail.
pub fn spawn(
    mut source: impl Read + Send + 'static,
    mut forward: impl Write + Send + 'static,
) -> io::Result<Arc<Shared>> {
    let shared = Arc::new(Shared::default());
    let reader = Arc::clone(&shared);
    thread::Builder::new()
        .name("headless-harness-stderr".to_owned())
        .spawn(move || {
            let mut buffer = [0u8; 8192];
            let mut line: Vec<u8> = Vec::with_capacity(READ_LINE_BYTES);
            loop {
                let count = match source.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => count,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                };
                let chunk = &buffer[..count];
                let _ = forward.write_all(chunk).and_then(|()| forward.flush());
                let mut rest = chunk;
                while let Some(newline) = rest.iter().position(|byte| *byte == b'\n') {
                    let room = READ_LINE_BYTES.saturating_sub(line.len());
                    line.extend_from_slice(&rest[..newline.min(room)]);
                    reader.push(&line);
                    line.clear();
                    rest = &rest[newline + 1..];
                }
                let room = READ_LINE_BYTES.saturating_sub(line.len());
                line.extend_from_slice(&rest[..rest.len().min(room)]);
            }
            if !line.is_empty() {
                reader.push(&line);
            }
            reader.end();
        })?;
    Ok(shared)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tail_keeps_the_last_lines_within_its_bounds() {
        let mut tail = Tail::default();
        for index in 0..100 {
            tail.push(&format!("line {index}\n"));
        }
        let value = tail.to_value().unwrap();
        let lines = value["lines"].as_array().unwrap();
        assert_eq!(lines.len(), MAX_LINES);
        assert_eq!(lines[0], "line 80");
        assert_eq!(lines[MAX_LINES - 1], "line 99");
        assert_eq!(value["omitted_lines"], 80);
        // Long lines are cut and the byte bound wins over the line bound.
        let mut tail = Tail::default();
        for _ in 0..30 {
            tail.push(&"é".repeat(2000));
        }
        let value = tail.to_value().unwrap();
        let lines = value["lines"].as_array().unwrap();
        let total: usize = lines.iter().map(|line| line.as_str().unwrap().len()).sum();
        assert!(total <= MAX_BYTES, "{total}");
        assert!(lines
            .iter()
            .all(|line| line.as_str().unwrap().len() <= MAX_LINE_BYTES
                && line.as_str().unwrap().ends_with('…')));
        assert_eq!(
            value["omitted_lines"].as_u64().unwrap() + lines.len() as u64,
            30
        );
    }

    #[test]
    fn the_tail_masks_credentials_and_skips_blank_lines() {
        let mut tail = Tail::default();
        tail.push("");
        tail.push("   ");
        tail.push("auth failed: token=abc123secret Bearer xyz");
        tail.push("key AbCdEfGh1234567890AbCdEfGh12345678 refused");
        tail.push("-----BEGIN OPENSSH PRIVATE KEY-----");
        let value = tail.to_value().unwrap();
        assert_eq!(
            value["lines"],
            json!([
                "auth failed: token=*** Bearer ***",
                "key *** refused",
                "***"
            ])
        );
        assert!(Tail::default().to_value().is_none());
    }

    #[test]
    fn the_reader_forwards_every_byte_and_keeps_the_tail() {
        let mut text = String::new();
        for index in 0..50 {
            text.push_str(&format!("e{index}\n"));
        }
        text.push_str(&"x".repeat(20_000));
        text.push_str("\nusage: muse serve [OPTIONS]");
        let forwarded = Arc::new(Mutex::new(Vec::new()));
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let shared = spawn(
            io::Cursor::new(text.clone().into_bytes()),
            Sink(Arc::clone(&forwarded)),
        )
        .unwrap();
        // The cursor ends at once; the wait only bounds a stream held open elsewhere.
        let value = shared.finish(Duration::from_secs(60)).unwrap();
        assert_eq!(*forwarded.lock().unwrap(), text.as_bytes());
        let lines = value["lines"].as_array().unwrap();
        assert_eq!(lines.last().unwrap(), "usage: muse serve [OPTIONS]");
        let long = &lines[lines.len() - 2];
        assert!(long.as_str().unwrap().ends_with('…'), "{long}");
        assert!(long.as_str().unwrap().len() <= MAX_LINE_BYTES);
    }
}
