//! Facts about coding-agent harnesses, starting with their transcripts.
//!
//! A coding agent running in a terminal (Claude Code, Codex, Muse) shows its work on screen, and
//! it also appends the same conversation, exactly as the model wrote it, to a session file of its
//! own. The screen is a rendering: Markdown markers are drawn as styles, long lines are wrapped
//! at the terminal's width, tables become box drawing, and text written in the middle of a turn
//! can be collapsed to one line. The session file keeps the original text.
//!
//! This crate finds the session file of a running agent and reads it as it grows:
//!
//! * [`Transcript`] is what one harness knows about its file: how to find it
//!   ([`Transcript::locate`]) from a [`Hints`] (the harness process, its working directory, a
//!   session id) and how to decode its lines into [`Event`]s ([`Transcript::decoder`]).
//! * [`transcript`] returns the [`Transcript`] of a harness by name: [`claude`], [`codex`] and
//!   [`muse`].
//! * [`Follower`] reads a file incrementally: complete lines only, from where it stopped, and
//!   starts over when the file is replaced or truncated.
//! * [`replies`] finds the reply blocks an agent writes between `<CHAT_REPLY_ID>` and
//!   `</CHAT_REPLY_ID>` lines, and tells whether a block read from a screen is a rendering of a
//!   block read from a transcript.
//!
//! Everything here is a fact about a harness; what to do with the text is the caller's policy.
//!
//! ```
//! use agent_harness::{transcript, Follower, Hints};
//!
//! let hints = Hints::for_process(std::process::id());
//! if let Some(found) = transcript("claude").and_then(|reader| reader.locate(&hints)) {
//!     let mut follower = Follower::open(&found.path, transcript("claude").unwrap().decoder(), 1 << 20);
//!     for event in follower.poll().unwrap_or_default() {
//!         if let agent_harness::Event::Text { text, .. } = event {
//!             println!("{text}");
//!         }
//!     }
//! }
//! ```

pub mod claude;
pub mod claude_records;
pub mod codex;
mod event;
mod follow;
pub mod history;
mod locate;
pub mod muse;
pub mod muse_records;
mod proc;
pub mod replies;
pub mod rollout;

pub use event::{Event, Via};
pub use follow::{Follower, DEFAULT_MAX_READ};
pub use history::{parse_history_line, HistoryError, HistoryPrompt};
pub use locate::{Hints, Located};

/// One harness's session file: where it is and how its lines read.
pub trait Transcript: Sync {
    /// The harness's name, as agent managers and terminal multiplexers report it (`claude`,
    /// `codex`, `muse`).
    fn name(&self) -> &'static str;

    /// The session file of the agent `hints` describe, with the evidence that chose it; `None`
    /// when nothing identifies one.
    fn locate(&self, hints: &Hints) -> Option<Located>;

    /// A fresh decoder for this harness's lines. Decoders keep state across lines (open
    /// transactions, tool calls), so use one per file and feed it lines in order.
    fn decoder(&self) -> Box<dyn Decoder>;
}

/// Turns one harness's transcript lines into [`Event`]s.
pub trait Decoder: Send {
    /// Decode one complete line (without its newline), appending what it holds to `out`. A line
    /// that does not parse adds nothing.
    fn feed(&mut self, line: &[u8], out: &mut Vec<Event>);
}

/// Every harness whose transcript this crate reads.
#[must_use]
pub fn all() -> [&'static dyn Transcript; 3] {
    [&claude::CLAUDE, &codex::CODEX, &muse::MUSE]
}

/// The [`Transcript`] of the harness named `name`, when this crate reads it.
#[must_use]
pub fn transcript(name: &str) -> Option<&'static dyn Transcript> {
    all().into_iter().find(|reader| reader.name() == name)
}

/// Decode every complete line of `bytes` with a fresh decoder of `reader`: a whole transcript, or
/// a fixture, at once.
#[must_use]
pub fn decode_all(reader: &dyn Transcript, bytes: &[u8]) -> Vec<Event> {
    let mut decoder = reader.decoder();
    let mut out = Vec::new();
    for line in bytes.split(|byte| *byte == b'\n') {
        if !line.is_empty() {
            decoder.feed(line, &mut out);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reader_is_found_by_its_name_and_others_are_not() {
        for reader in all() {
            assert_eq!(
                transcript(reader.name()).map(|r| r.name()),
                Some(reader.name())
            );
        }
        assert!(transcript("unknown").is_none());
    }
}
