//! The typed event log of a headless agent (`#565 headless-coordinator`, milestone M1).
//!
//! A headless agent has no screen to read, so its wrapper describes what the harness does as
//! one JSON object per line. Events **describe**; they never **act**: replies and messages keep
//! going through `agentctl chat reply` and `agentctl send`, so re-reading a log cannot repeat an
//! effect.
//!
//! ```text
//! <registry>/<agent>/events/<generation>.jsonl      first segment of a generation
//! <registry>/<agent>/events/<generation>.<n>.jsonl  later segments (n = 1, 2, ...)
//! <registry>/<agent>/events/index.json              written only by the wrapper
//! ```
//!
//! Files are 0600 in a 0700 directory and append-only. A generation's current segment is
//! closed once it reaches [`SEGMENT_BYTES`] and the next one opened, so a wrapper that runs for
//! months never grows one file without bound; retention prunes whole segments, oldest first.
//! Segments sort by `(generation, n)`, and that order is the order of `seq`.
//!
//! Each event is written with a single `write(2)`. `seq` grows by one per event for the agent,
//! across generations, so `events#<seq>` is a stable address. A starting writer takes the larger
//! of `index.json`'s `last_seq` and the last complete line of the newest generation file, after
//! truncating a partial final line a crash left behind. Readers skip lines they cannot parse and
//! ignore fields and event types they do not know.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Envelope version written in every event's `v` field.
pub const VERSION: u64 = 1;
/// Schema of `index.json`.
pub const INDEX_SCHEMA: &str = "agentctl-events-index/v1";
/// Directory name of the log inside an agent's registry directory.
pub const EVENTS_DIR: &str = "events";
/// File name of the index inside the log directory.
pub const INDEX_FILE: &str = "index.json";
/// Generations older than this are pruned when a writer starts.
pub const RETENTION: Duration = Duration::from_secs(90 * 24 * 60 * 60);
/// Total size above which the oldest generations are pruned when a writer starts.
pub const RETENTION_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Size at which the writer closes the current segment and opens the next.
pub const SEGMENT_BYTES: u64 = 64 * 1024 * 1024;
/// Longest excerpt of an input or a cited text kept in an event.
pub const EXCERPT_BYTES: usize = 600;
/// Longest tool summary kept in an event.
pub const SUMMARY_BYTES: usize = 200;

/// One event, as written on one line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Envelope version ([`VERSION`]).
    pub v: u64,
    /// Position in the agent's log, one more than the previous event's.
    pub seq: u64,
    /// Unique identifier: `evt-<generation>-<seq>`.
    pub id: String,
    /// Unix time in milliseconds.
    pub ts: u64,
    /// Agent name.
    pub agent: String,
    /// Wrapper generation that wrote the event.
    pub generation: u64,
    /// Harness session identifier, when known.
    #[serde(default)]
    pub session: Option<String>,
    /// Turn number within the agent's log; 0 before the first turn.
    #[serde(default)]
    pub turn: u64,
    /// The queue item that caused the event, when one did.
    #[serde(default)]
    pub causation: Option<String>,
    /// Event type, such as `turn.started` or `tool.completed`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Type-specific data.
    #[serde(default)]
    pub data: Value,
}

/// Unix time now, in milliseconds.
#[must_use]
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// The log directory of the agent whose registry directory is `agent_directory`.
#[must_use]
pub fn directory(agent_directory: &Path) -> PathBuf {
    agent_directory.join(EVENTS_DIR)
}

/// `text` cut to at most `limit` bytes at a character boundary, with `…` appended when cut.
#[must_use]
pub fn excerpt(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// One file of the log: a segment of one wrapper generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Segment {
    /// Wrapper generation.
    pub generation: u64,
    /// Segment within the generation, from 0.
    pub segment: u64,
    /// The file.
    pub path: PathBuf,
}

impl Segment {
    /// The sort key: the order of the events the segments hold.
    #[must_use]
    pub fn key(&self) -> (u64, u64) {
        (self.generation, self.segment)
    }
}

fn digits(text: &str) -> Option<u64> {
    (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

/// The file name of segment `segment` of `generation`.
#[must_use]
pub fn segment_name(generation: u64, segment: u64) -> String {
    if segment == 0 {
        format!("{generation}.jsonl")
    } else {
        format!("{generation}.{segment}.jsonl")
    }
}

/// Every segment in `dir`, in event order.
pub fn segments(dir: &Path) -> io::Result<Vec<Segment>> {
    let mut files = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(files),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(stem) = name.strip_suffix(".jsonl") else {
            continue;
        };
        let parsed = match stem.split_once('.') {
            None => digits(stem).map(|generation| (generation, 0)),
            Some((generation, segment)) => digits(generation)
                .zip(digits(segment))
                .filter(|(_, segment)| *segment > 0),
        };
        if let Some((generation, segment)) = parsed {
            files.push(Segment {
                generation,
                segment,
                path: entry.path(),
            });
        }
    }
    files.sort_by_key(Segment::key);
    Ok(files)
}

/// The generations in `dir`, each with the path of its last segment, in ascending order.
pub fn generation_files(dir: &Path) -> io::Result<Vec<(u64, PathBuf)>> {
    let mut files: Vec<(u64, PathBuf)> = Vec::new();
    for segment in segments(dir)? {
        match files.last_mut() {
            Some((generation, path)) if *generation == segment.generation => {
                *path = segment.path;
            }
            _ => files.push((segment.generation, segment.path)),
        }
    }
    Ok(files)
}

/// The `seq` of the first complete event in `path`, reading only its first line.
pub fn first_seq(path: &Path) -> io::Result<Option<u64>> {
    use std::io::BufRead;
    let mut reader = io::BufReader::new(File::open(path)?);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 || line.last() != Some(&b'\n') {
            return Ok(None);
        }
        if let Ok(event) = serde_json::from_slice::<Event>(&line) {
            return Ok(Some(event.seq));
        }
    }
}

/// The events of one generation file from byte `offset`: the parsed events, the offset just
/// past the last complete line, and the number of complete lines that did not parse.
pub fn read_from(path: &Path, offset: u64) -> io::Result<(Vec<Event>, u64, usize)> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let mut events = Vec::new();
    let mut unreadable = 0;
    for line in bytes[..complete].split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        match serde_json::from_slice::<Event>(line) {
            Ok(event) => events.push(event),
            Err(_) => unreadable += 1,
        }
    }
    Ok((events, offset + complete as u64, unreadable))
}

/// Every readable event in `dir`, oldest first.
pub fn read_all(dir: &Path) -> io::Result<Vec<Event>> {
    read_segments(&segments(dir)?)
}

/// The readable events of `segments`, in order; a segment pruned meanwhile is skipped.
pub fn read_segments(segments: &[Segment]) -> io::Result<Vec<Event>> {
    let mut events = Vec::new();
    for segment in segments {
        match read_from(&segment.path, 0) {
            Ok((read, _, _)) => events.extend(read),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(events)
}

/// The readable events in the last `budget` bytes of the log, across segment boundaries, oldest
/// first. A line cut by the window's start is skipped, and so is a partial final line.
pub fn read_recent(dir: &Path, budget: u64) -> io::Result<Vec<Event>> {
    let mut chunks: Vec<Vec<Event>> = Vec::new();
    let mut left = budget;
    for segment in segments(dir)?.iter().rev() {
        if left == 0 {
            break;
        }
        let mut file = match File::open(&segment.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let length = file.metadata()?.len();
        let start = length.saturating_sub(left);
        left -= length - start;
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let first = if start > 0 {
            bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |index| index + 1)
        } else {
            0
        };
        let end = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1)
            .max(first);
        chunks.push(
            bytes[first..end]
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .filter_map(|line| serde_json::from_slice::<Event>(line).ok())
                .collect(),
        );
    }
    chunks.reverse();
    Ok(chunks.into_iter().flatten().collect())
}

/// The index into `segments` of the one holding `seq`: the newest whose first event is at or
/// before it (0 when none is). Searches from the newest segment back, reading one line of each,
/// so a recent `seq` never opens old segments.
pub fn segment_holding(segments: &[Segment], seq: u64) -> io::Result<usize> {
    for (index, segment) in segments.iter().enumerate().rev() {
        match first_seq(&segment.path) {
            Ok(Some(first)) if first <= seq => return Ok(index),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(0)
}

/// Read `index.json`, or `None` when it is absent or unreadable.
#[must_use]
pub fn read_index(dir: &Path) -> Option<Value> {
    let bytes = fs::read(dir.join(INDEX_FILE)).ok()?;
    serde_json::from_slice(&bytes)
        .ok()
        .filter(|value: &Value| value["schema"] == INDEX_SCHEMA)
}

/// A read-only follower of one agent's log across generations and their segments. It takes no
/// lock; any number may run beside the writer.
#[derive(Debug)]
pub struct Follower {
    dir: PathBuf,
    key: (u64, u64),
    offset: u64,
}

impl Follower {
    /// A follower that starts at the beginning of the oldest segment in `dir`.
    #[must_use]
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            key: (0, 0),
            offset: 0,
        }
    }

    /// A follower that starts at the beginning of `segment`.
    #[must_use]
    pub fn at(dir: &Path, segment: &Segment) -> Self {
        Self {
            dir: dir.to_path_buf(),
            key: segment.key(),
            offset: 0,
        }
    }

    /// A follower that starts after everything now in `dir`: past the end of its newest segment.
    pub fn at_end(dir: &Path) -> io::Result<Self> {
        let mut follower = Self::new(dir);
        if let Some(last) = segments(dir)?.last() {
            follower.key = last.key();
            follower.offset = complete_length(&last.path)?;
        }
        Ok(follower)
    }

    /// The events written since the previous call, oldest first. A segment is finished once a
    /// newer one exists, since only one writer runs at a time and it writes in order; one
    /// pruned before it was read is skipped (the reader sees the hole in `seq`).
    pub fn poll(&mut self) -> io::Result<Vec<Event>> {
        let mut events = Vec::new();
        for segment in segments(&self.dir)? {
            if segment.key() < self.key {
                continue;
            }
            if segment.key() > self.key {
                self.key = segment.key();
                self.offset = 0;
            }
            match read_from(&segment.path, self.offset) {
                Ok((read, offset, _)) => {
                    self.offset = offset;
                    events.extend(read);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(events)
    }
}

/// Bytes of `path` up to and including its last newline.
fn complete_length(path: &Path) -> io::Result<u64> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    // Look back a little at a time for the last newline; a partial line is not yet an event.
    let mut end = length;
    let mut buffer = vec![0u8; 64 * 1024];
    while end > 0 {
        let start = end.saturating_sub(buffer.len() as u64);
        let chunk = &mut buffer[..(end - start) as usize];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(chunk)?;
        if let Some(index) = chunk.iter().rposition(|byte| *byte == b'\n') {
            return Ok(start + index as u64 + 1);
        }
        end = start;
    }
    Ok(0)
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(dir)?;
            if !metadata.is_dir() {
                return Err(io::Error::other(format!(
                    "event log path is not a directory: {}",
                    dir.display()
                )));
            }
            if metadata.permissions().mode() & 0o077 != 0 {
                fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
            }
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Truncate a partial final line of `path` and return the `seq` of its last complete event.
fn repair_tail(path: &Path) -> io::Result<Option<u64>> {
    let bytes = fs::read(path)?;
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    if complete < bytes.len() {
        let file = OpenOptions::new().write(true).open(path)?;
        file.set_len(complete as u64)?;
        file.sync_all()?;
    }
    Ok(bytes[..complete]
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_slice::<Event>(line).ok())
        .map(|event| event.seq)
        .max())
}

/// Delete segments older than `retention`, then the oldest while the total exceeds `cap`, never
/// touching `keep` (the segment being written). Returns the generations of which a segment was
/// removed, each once.
fn prune(dir: &Path, retention: Duration, cap: u64, keep: Option<&Path>) -> io::Result<Vec<u64>> {
    let now = SystemTime::now();
    let mut files: Vec<(Segment, u64, SystemTime)> = Vec::new();
    for segment in segments(dir)? {
        let metadata = match fs::metadata(&segment.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let modified = metadata.modified().unwrap_or(now);
        files.push((segment, metadata.len(), modified));
    }
    let mut removed: Vec<u64> = Vec::new();
    let mut total: u64 = files.iter().map(|file| file.1).sum();
    for (segment, size, modified) in &files {
        if keep == Some(segment.path.as_path()) {
            continue;
        }
        let old = now.duration_since(*modified).unwrap_or_default() > retention;
        if old || total > cap {
            match fs::remove_file(&segment.path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            total = total.saturating_sub(*size);
            if removed.last() != Some(&segment.generation) {
                removed.push(segment.generation);
            }
        }
    }
    Ok(removed)
}

/// The only writer of one agent's log for one wrapper generation.
#[derive(Debug)]
pub struct EventWriter {
    dir: PathBuf,
    file: File,
    agent: String,
    generation: u64,
    session: Option<String>,
    turn: u64,
    next_seq: u64,
    first_seq: u64,
    pruned: Vec<u64>,
    segment: u64,
    segment_bytes: u64,
    segment_limit: u64,
    retention: Duration,
    cap: u64,
}

impl EventWriter {
    /// Start a new generation of the log in `dir` for `agent`. The caller must hold the agent's
    /// headless lock, which makes this the only writer.
    pub fn open(dir: &Path, agent: &str) -> io::Result<Self> {
        Self::open_with_retention(dir, agent, RETENTION, RETENTION_BYTES)
    }

    /// [`Self::open`] with explicit retention bounds.
    pub fn open_with_retention(
        dir: &Path,
        agent: &str,
        retention: Duration,
        cap: u64,
    ) -> io::Result<Self> {
        create_private_dir(dir)?;
        let index = read_index(dir);
        let mut last_seq = index
            .as_ref()
            .and_then(|index| index["last_seq"].as_u64())
            .unwrap_or(0);
        let mut last_generation = index
            .as_ref()
            .and_then(|index| index["last_generation"].as_u64())
            .unwrap_or(0);
        let mut turn = index
            .as_ref()
            .and_then(|index| index["last_turn"].as_u64())
            .unwrap_or(0);
        let files = segments(dir)?;
        if let Some(last) = files.last() {
            last_generation = last_generation.max(last.generation);
            repair_tail(&last.path)?;
        }
        // The newest segment may hold no event yet (a crash right after a rotation opened it,
        // before the index was rewritten), so look back to the newest segment that has one.
        for segment in files.iter().rev() {
            let (events, _, _) = read_from(&segment.path, 0)?;
            if let Some(seq) = events.iter().map(|event| event.seq).max() {
                last_seq = last_seq.max(seq);
                if let Some(max_turn) = events.iter().map(|event| event.turn).max() {
                    turn = turn.max(max_turn);
                }
                break;
            }
        }
        let pruned = prune(dir, retention, cap, None)?;
        let generation = last_generation + 1;
        let path = dir.join(segment_name(generation, 0));
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .mode(0o600)
            .open(&path)?;
        let writer = Self {
            dir: dir.to_path_buf(),
            file,
            agent: agent.to_owned(),
            generation,
            session: None,
            turn,
            next_seq: last_seq + 1,
            first_seq: last_seq + 1,
            pruned,
            segment: 0,
            segment_bytes: 0,
            segment_limit: SEGMENT_BYTES,
            retention,
            cap,
        };
        writer.write_index()?;
        Ok(writer)
    }

    /// This writer's generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Close the current segment once it reaches `bytes` (default [`SEGMENT_BYTES`]).
    pub fn set_segment_limit(&mut self, bytes: u64) {
        self.segment_limit = bytes.max(1);
    }

    /// The segment being written within this generation.
    #[must_use]
    pub fn segment(&self) -> u64 {
        self.segment
    }

    /// Generations pruned when this writer started.
    #[must_use]
    pub fn pruned(&self) -> &[u64] {
        &self.pruned
    }

    /// The current turn number.
    #[must_use]
    pub fn turn(&self) -> u64 {
        self.turn
    }

    /// The `seq` the next event gets.
    #[must_use]
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Record the harness session for later events.
    pub fn set_session(&mut self, session: Option<String>) {
        self.session = session;
    }

    /// Start the next turn and return its number.
    pub fn begin_turn(&mut self) -> u64 {
        self.turn += 1;
        self.turn
    }

    /// Append one event and return its `seq`. Input, turn and lifecycle events are synced to
    /// disk, and turn and lifecycle events also rewrite the index.
    pub fn append(&mut self, kind: &str, causation: Option<&str>, data: Value) -> io::Result<u64> {
        let seq = self.next_seq;
        let event = Event {
            v: VERSION,
            seq,
            id: format!("evt-{}-{seq}", self.generation),
            ts: now_millis(),
            agent: self.agent.clone(),
            generation: self.generation,
            session: self.session.clone(),
            turn: self.turn,
            causation: causation.map(str::to_owned),
            kind: kind.to_owned(),
            data,
        };
        let mut line = serde_json::to_vec(&event).map_err(io::Error::other)?;
        line.push(b'\n');
        // One write(2) on an O_APPEND file: a reader never sees two events interleaved.
        let written = self.file.write(&line)?;
        if written != line.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                format!("short event write: {written} of {} bytes", line.len()),
            ));
        }
        self.next_seq += 1;
        self.segment_bytes += line.len() as u64;
        let durable = ["input.", "turn.", "lifecycle."]
            .iter()
            .any(|prefix| kind.starts_with(prefix));
        if durable {
            self.file.sync_data()?;
        }
        if self.segment_bytes >= self.segment_limit {
            self.rotate()?;
        } else if kind.starts_with("turn.") || kind.starts_with("lifecycle.") {
            self.write_index()?;
        }
        Ok(seq)
    }

    /// Close the current segment and continue in the next one, pruning old segments so a
    /// long-running generation stays within retention.
    fn rotate(&mut self) -> io::Result<()> {
        self.file.sync_all()?;
        let segment = self.segment + 1;
        let path = self.dir.join(segment_name(self.generation, segment));
        // The new file exists before the index names it, and the old one is complete before
        // the new one exists, so a reader that sees the new segment has the old one whole.
        self.file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .mode(0o600)
            .open(&path)?;
        File::open(&self.dir)?.sync_all()?;
        self.segment = segment;
        self.segment_bytes = 0;
        for generation in prune(&self.dir, self.retention, self.cap, Some(&path))? {
            if !self.pruned.contains(&generation) {
                self.pruned.push(generation);
            }
        }
        self.write_index()
    }

    /// Rewrite `index.json` atomically.
    pub fn write_index(&self) -> io::Result<()> {
        let all = segments(&self.dir)?;
        let name = |path: &Path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        };
        let generations: Vec<Value> = generation_files(&self.dir)?
            .into_iter()
            .map(|(generation, path)| json!({"generation": generation, "file": name(&path)}))
            .collect();
        let segment_list: Vec<Value> = all
            .iter()
            .map(|segment| {
                json!({"generation": segment.generation, "segment": segment.segment,
                       "file": name(&segment.path)})
            })
            .collect();
        let index = json!({
            "schema": INDEX_SCHEMA,
            "agent": self.agent,
            "last_seq": self.next_seq.saturating_sub(1),
            "last_generation": self.generation,
            "last_turn": self.turn,
            "current_first_seq": self.first_seq,
            "current_segment": self.segment,
            "generations": generations,
            "segments": segment_list,
            "updated_at": now_millis(),
        });
        let mut bytes = serde_json::to_vec_pretty(&index).map_err(io::Error::other)?;
        bytes.push(b'\n');
        let temporary = self.dir.join(format!(".index.{}.tmp", std::process::id()));
        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        fs::rename(&temporary, self.dir.join(INDEX_FILE))?;
        File::open(&self.dir)?.sync_all()
    }

    /// Sync the current generation file.
    pub fn sync(&self) -> io::Result<()> {
        self.file.sync_data()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agentctl-events-{label}-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn seq_continues_across_generations_and_index_is_written() {
        let root = scratch("seq");
        let dir = root.join(EVENTS_DIR);
        let mut first = EventWriter::open(&dir, "coordinator").unwrap();
        assert_eq!(first.generation(), 1);
        first.append("lifecycle.started", None, json!({})).unwrap();
        first.begin_turn();
        first.append("turn.started", Some("q1"), json!({})).unwrap();
        drop(first);
        let mut second = EventWriter::open(&dir, "coordinator").unwrap();
        assert_eq!(second.generation(), 2);
        assert_eq!(second.turn(), 1);
        let seq = second.append("lifecycle.started", None, json!({})).unwrap();
        assert_eq!(seq, 3);
        let events = read_all(&dir).unwrap();
        let seqs: Vec<u64> = events.iter().map(|event| event.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3]);
        assert_eq!(events[1].causation.as_deref(), Some("q1"));
        assert_eq!(events[2].id, "evt-2-3");
        let index = read_index(&dir).unwrap();
        assert_eq!(index["last_seq"], 3);
        assert_eq!(index["last_generation"], 2);
        let mode = fs::metadata(dir.join("2.jsonl"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let mode = fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn partial_final_line_is_truncated_and_stale_index_is_overruled() {
        let root = scratch("partial");
        let dir = root.join(EVENTS_DIR);
        let mut writer = EventWriter::open(&dir, "a").unwrap();
        for _ in 0..3 {
            writer
                .append("assistant.text", None, json!({"text": "x"}))
                .unwrap();
        }
        drop(writer);
        // A crash: the index lags behind and the last line is half written.
        let mut file = OpenOptions::new()
            .append(true)
            .open(dir.join("1.jsonl"))
            .unwrap();
        file.write_all(b"{\"v\":1,\"seq\":4,\"id\":").unwrap();
        fs::write(
            dir.join(INDEX_FILE),
            serde_json::to_vec(
                &json!({"schema": INDEX_SCHEMA, "last_seq": 1, "last_generation": 1}),
            )
            .unwrap(),
        )
        .unwrap();
        let mut writer = EventWriter::open(&dir, "a").unwrap();
        assert_eq!(writer.next_seq(), 4);
        writer
            .append("status", None, json!({"status": "idle"}))
            .unwrap();
        let first = fs::read(dir.join("1.jsonl")).unwrap();
        assert_eq!(first.last(), Some(&b'\n'));
        let seqs: Vec<u64> = read_all(&dir).unwrap().iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3, 4]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn follower_sees_new_lines_and_new_generations_but_not_partial_lines() {
        let root = scratch("follow");
        let dir = root.join(EVENTS_DIR);
        let mut follower = Follower::new(&dir);
        assert!(follower.poll().unwrap().is_empty());
        let mut writer = EventWriter::open(&dir, "a").unwrap();
        writer.append("status", None, json!({})).unwrap();
        assert_eq!(follower.poll().unwrap().len(), 1);
        assert!(follower.poll().unwrap().is_empty());
        let mut file = OpenOptions::new()
            .append(true)
            .open(dir.join("1.jsonl"))
            .unwrap();
        file.write_all(b"{\"partial\":").unwrap();
        assert!(follower.poll().unwrap().is_empty());
        drop(writer);
        let mut writer = EventWriter::open(&dir, "a").unwrap();
        writer.append("status", None, json!({})).unwrap();
        let events = follower.poll().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].generation, 2);
        assert_eq!(events[0].seq, 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retention_prunes_oldest_generations_over_the_cap() {
        let root = scratch("prune");
        let dir = root.join(EVENTS_DIR);
        for _ in 0..3 {
            let mut writer = EventWriter::open(&dir, "a").unwrap();
            writer
                .append("assistant.text", None, json!({"text": "y".repeat(100)}))
                .unwrap();
        }
        // Every generation holds one event of the same size: a cap of one file keeps one.
        let cap = fs::metadata(dir.join("3.jsonl")).unwrap().len();
        let writer = EventWriter::open_with_retention(&dir, "a", RETENTION, cap).unwrap();
        assert_eq!(writer.pruned(), &[1, 2]);
        let generations: Vec<u64> = generation_files(&dir)
            .unwrap()
            .into_iter()
            .map(|(generation, _)| generation)
            .collect();
        assert_eq!(generations, vec![3, 4]);
        assert_eq!(writer.next_seq(), 4);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn excerpt_cuts_at_character_boundaries() {
        assert_eq!(excerpt("short", 10), "short");
        assert_eq!(excerpt("ééé", 3), "é…");
    }

    fn segment_names(dir: &Path) -> Vec<String> {
        segments(dir)
            .unwrap()
            .iter()
            .map(|segment| {
                segment
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    #[test]
    fn a_long_generation_rotates_into_segments_with_continuous_seq() {
        let root = scratch("rotate");
        let dir = root.join(EVENTS_DIR);
        let mut writer = EventWriter::open(&dir, "a").unwrap();
        writer.set_segment_limit(600);
        let mut follower = Follower::new(&dir);
        let mut followed = Vec::new();
        for n in 0..20 {
            writer
                .append(
                    "assistant.text",
                    None,
                    json!({"text": format!("event {n} {}", "x".repeat(100))}),
                )
                .unwrap();
            if n % 3 == 0 {
                followed.extend(follower.poll().unwrap().into_iter().map(|event| event.seq));
            }
        }
        followed.extend(follower.poll().unwrap().into_iter().map(|event| event.seq));
        let names = segment_names(&dir);
        assert!(names.len() >= 4, "{names:?}");
        assert_eq!(names[0], "1.jsonl");
        assert_eq!(names[1], "1.1.jsonl");
        assert!(writer.segment() >= 3);
        // Every file but the newest reached the limit before it was closed.
        for segment in &segments(&dir).unwrap()[..names.len() - 1] {
            assert!(fs::metadata(&segment.path).unwrap().len() >= 600);
        }
        let seqs: Vec<u64> = read_all(&dir).unwrap().iter().map(|e| e.seq).collect();
        assert_eq!(seqs, (1..=20).collect::<Vec<u64>>());
        assert_eq!(
            followed, seqs,
            "the follower lost or repeated events across rotations"
        );
        let index = read_index(&dir).unwrap();
        assert_eq!(index["current_segment"], writer.segment());
        assert_eq!(index["segments"].as_array().unwrap().len(), names.len());
        // The generation list names each generation once, by its newest segment.
        assert_eq!(generation_files(&dir).unwrap().len(), 1);
        // A new generation starts at segment 0 and continues seq.
        drop(writer);
        let mut next = EventWriter::open(&dir, "a").unwrap();
        assert_eq!(next.append("status", None, json!({})).unwrap(), 21);
        assert!(segment_names(&dir).contains(&"2.jsonl".to_owned()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_crash_right_after_rotation_keeps_seq() {
        let root = scratch("rotcrash");
        let dir = root.join(EVENTS_DIR);
        let mut writer = EventWriter::open(&dir, "a").unwrap();
        for _ in 0..5 {
            writer
                .append("assistant.text", None, json!({"text": "y"}))
                .unwrap();
        }
        drop(writer);
        // The crash: the next segment was created (empty but for a torn line) and the index
        // still says seq 1.
        fs::write(dir.join("1.1.jsonl"), b"{\"v\":1,\"seq\":6,").unwrap();
        fs::write(
            dir.join(INDEX_FILE),
            serde_json::to_vec(
                &json!({"schema": INDEX_SCHEMA, "last_seq": 1, "last_generation": 1}),
            )
            .unwrap(),
        )
        .unwrap();
        let mut writer = EventWriter::open(&dir, "a").unwrap();
        assert_eq!(writer.generation(), 2);
        assert_eq!(writer.append("status", None, json!({})).unwrap(), 6);
        assert_eq!(fs::metadata(dir.join("1.1.jsonl")).unwrap().len(), 0);
        let seqs: Vec<u64> = read_all(&dir).unwrap().iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3, 4, 5, 6]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rotation_prunes_old_segments_but_never_the_current_one() {
        let root = scratch("rotprune");
        let dir = root.join(EVENTS_DIR);
        let mut writer = EventWriter::open_with_retention(&dir, "a", RETENTION, 2000).unwrap();
        writer.set_segment_limit(500);
        for _ in 0..40 {
            writer
                .append("assistant.text", None, json!({"text": "z".repeat(120)}))
                .unwrap();
        }
        let all = segments(&dir).unwrap();
        let total: u64 = all
            .iter()
            .map(|s| fs::metadata(&s.path).unwrap().len())
            .sum();
        assert!(total <= 2000 + 500, "{total}");
        assert_eq!(all.last().unwrap().segment, writer.segment());
        assert_eq!(writer.pruned(), &[1]);
        // What is left is the newest stretch of the log, still in order and contiguous.
        let seqs: Vec<u64> = read_all(&dir).unwrap().iter().map(|e| e.seq).collect();
        assert_eq!(*seqs.last().unwrap(), 40);
        assert!(
            seqs.windows(2).all(|pair| pair[1] == pair[0] + 1),
            "{seqs:?}"
        );
        assert!(seqs[0] > 1);
        // A follower that was on a pruned segment moves on without an error.
        let mut follower = Follower::at(
            &dir,
            &Segment {
                generation: 1,
                segment: 0,
                path: dir.join("1.jsonl"),
            },
        );
        assert_eq!(follower.poll().unwrap().len(), seqs.len());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn segment_names_parse_and_sort_in_event_order() {
        let root = scratch("names");
        let dir = root.join(EVENTS_DIR);
        fs::create_dir_all(&dir).unwrap();
        for name in [
            "10.jsonl",
            "2.jsonl",
            "2.10.jsonl",
            "2.2.jsonl",
            "2.0.jsonl",
            "x.jsonl",
            "3.a.jsonl",
            "index.json",
        ] {
            fs::write(dir.join(name), b"").unwrap();
        }
        assert_eq!(
            segment_names(&dir),
            vec!["2.jsonl", "2.2.jsonl", "2.10.jsonl", "10.jsonl"]
        );
        assert_eq!(segment_name(4, 0), "4.jsonl");
        assert_eq!(segment_name(4, 3), "4.3.jsonl");
        fs::remove_dir_all(root).unwrap();
    }
}
