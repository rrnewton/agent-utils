//! Reading a transcript as it grows.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::{Decoder, Event};

/// Most bytes one [`Follower::poll`] reads unless the follower says otherwise. A file that grew
/// by more is read from this far before its end, and the lines skipped are counted in
/// [`Follower::skipped_bytes`].
pub const DEFAULT_MAX_READ: u64 = 32 * 1024 * 1024;

/// Reads one transcript incrementally and decodes each complete line once.
///
/// The first read starts `tail` bytes before the end of the file (at the next line start), so a
/// long transcript is not read whole. Each later [`poll`](Self::poll) reads what was appended. A
/// last line without its newline is still being written and is kept until it is complete. A
/// file that shrank or was replaced (another inode at the path) is read again from `tail` bytes
/// before its end.
pub struct Follower {
    path: PathBuf,
    decoder: Box<dyn Decoder>,
    tail: u64,
    max_read: u64,
    identity: Option<(u64, u64)>,
    offset: u64,
    partial: Vec<u8>,
    skipped: u64,
}

impl Follower {
    /// Follow `path`, decoding with `decoder`, starting `tail` bytes before its current end.
    #[must_use]
    pub fn open(path: &Path, decoder: Box<dyn Decoder>, tail: u64) -> Self {
        Self {
            path: path.to_owned(),
            decoder,
            tail,
            max_read: DEFAULT_MAX_READ,
            identity: None,
            offset: 0,
            partial: Vec::new(),
            skipped: 0,
        }
    }

    /// Read at most `bytes` per poll.
    #[must_use]
    pub fn with_max_read(mut self, bytes: u64) -> Self {
        self.max_read = bytes.max(1);
        self
    }

    /// The file followed.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Bytes passed over without decoding because the file grew faster than one poll reads.
    #[must_use]
    pub fn skipped_bytes(&self) -> u64 {
        self.skipped
    }

    /// The events of every line completed since the last poll.
    ///
    /// # Errors
    ///
    /// The file cannot be opened, inspected or read.
    pub fn poll(&mut self) -> io::Result<Vec<Event>> {
        let mut file = fs::File::open(&self.path)?;
        let meta = file.metadata()?;
        let identity = (meta.dev(), meta.ino());
        let length = meta.len();
        let mut aligned = true;
        if self.identity != Some(identity) || length < self.offset {
            self.identity = Some(identity);
            self.partial.clear();
            self.offset = length.saturating_sub(self.tail);
            aligned = self.offset == 0;
        }
        if length - self.offset > self.max_read {
            let start = length - self.max_read;
            self.skipped += start - self.offset;
            self.offset = start;
            self.partial.clear();
            aligned = false;
        }
        if !aligned && self.offset > 0 {
            // Start at a line boundary: the byte before the window says whether it is one.
            self.offset -= 1;
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        file.take(length - self.offset).read_to_end(&mut bytes)?;
        self.offset += bytes.len() as u64;
        let mut start = 0;
        if !aligned {
            start = bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |index| index + 1);
        }
        let mut events = Vec::new();
        let mut rest = &bytes[start..];
        while let Some(end) = rest.iter().position(|byte| *byte == b'\n') {
            self.partial.extend_from_slice(&rest[..end]);
            if !self.partial.is_empty() {
                self.decoder.feed(&self.partial, &mut events);
            }
            self.partial.clear();
            rest = &rest[end + 1..];
        }
        self.partial.extend_from_slice(rest);
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc::fake::scratch;
    use std::io::Write;

    /// Decodes each line as one text event.
    struct Lines;

    impl Decoder for Lines {
        fn feed(&mut self, line: &[u8], out: &mut Vec<Event>) {
            out.push(Event::Text {
                at: None,
                text: String::from_utf8_lossy(line).into_owned(),
            });
        }
    }

    fn texts(events: Vec<Event>) -> Vec<String> {
        events
            .into_iter()
            .map(|event| match event {
                Event::Text { text, .. } => text,
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn complete_lines_are_read_once_and_a_partial_line_waits() {
        let dir = scratch("follow");
        let path = dir.join("t.jsonl");
        fs::write(&path, "one\ntwo\nthr").unwrap();
        let mut follower = Follower::open(&path, Box::new(Lines), 1 << 20);
        assert_eq!(texts(follower.poll().unwrap()), ["one", "two"]);
        assert!(follower.poll().unwrap().is_empty());
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"ee\nfour\n").unwrap();
        assert_eq!(texts(follower.poll().unwrap()), ["three", "four"]);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_first_read_starts_at_a_line_inside_the_tail() {
        let dir = scratch("follow-tail");
        let path = dir.join("t.jsonl");
        fs::write(&path, "aaaa\nbbbb\ncccc\n").unwrap();
        // The last 10 bytes start exactly at "bbbb"; 9 start inside it.
        assert_eq!(
            texts(Follower::open(&path, Box::new(Lines), 10).poll().unwrap()),
            ["bbbb", "cccc"]
        );
        assert_eq!(
            texts(Follower::open(&path, Box::new(Lines), 9).poll().unwrap()),
            ["cccc"]
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_replaced_or_truncated_file_is_read_again_and_a_burst_is_skipped() {
        let dir = scratch("follow-replace");
        let path = dir.join("t.jsonl");
        fs::write(&path, "old-1\nold-2\n").unwrap();
        let mut follower = Follower::open(&path, Box::new(Lines), 1 << 20);
        assert_eq!(follower.poll().unwrap().len(), 2);
        let next = dir.join("next.jsonl");
        fs::write(&next, "new\n").unwrap();
        fs::rename(&next, &path).unwrap();
        assert_eq!(texts(follower.poll().unwrap()), ["new"]);
        fs::write(&path, "").unwrap();
        assert!(follower.poll().unwrap().is_empty());
        let mut follower = follower.with_max_read(8);
        fs::write(&path, "first\nsecond\nthird\n").unwrap();
        assert_eq!(texts(follower.poll().unwrap()), ["third"]);
        assert!(follower.skipped_bytes() > 0);
        assert!(Follower::open(&dir.join("absent"), Box::new(Lines), 1)
            .poll()
            .is_err());
        fs::remove_dir_all(dir).ok();
    }
}
