//! The typed event log of a headless agent (`#565 headless-coordinator`, milestone M1).
//!
//! A headless agent has no screen to read, so its wrapper describes what the harness does as
//! one JSON object per line. Events **describe**; they never **act**: replies and messages keep
//! going through `agentctl chat reply` and `agentctl send`, so re-reading a log cannot repeat an
//! effect.
//!
//! ```text
//! <registry>/<agent>/events/<generation>.jsonl   0600, directory 0700, append-only
//! <registry>/<agent>/events/index.json           written only by the wrapper
//! ```
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

/// The generation files in `dir`, as `(generation, path)` in ascending generation order.
pub fn generation_files(dir: &Path) -> io::Result<Vec<(u64, PathBuf)>> {
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
        if stem.is_empty() || !stem.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        if let Ok(generation) = stem.parse::<u64>() {
            files.push((generation, entry.path()));
        }
    }
    files.sort();
    Ok(files)
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
    let mut events = Vec::new();
    for (_, path) in generation_files(dir)? {
        events.extend(read_from(&path, 0)?.0);
    }
    Ok(events)
}

/// Read `index.json`, or `None` when it is absent or unreadable.
#[must_use]
pub fn read_index(dir: &Path) -> Option<Value> {
    let bytes = fs::read(dir.join(INDEX_FILE)).ok()?;
    serde_json::from_slice(&bytes)
        .ok()
        .filter(|value: &Value| value["schema"] == INDEX_SCHEMA)
}

/// A read-only follower of one agent's log across generations. It takes no lock; any number may
/// run beside the writer.
#[derive(Debug)]
pub struct Follower {
    dir: PathBuf,
    generation: u64,
    offset: u64,
}

impl Follower {
    /// A follower that starts at the beginning of the oldest generation in `dir`.
    #[must_use]
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            generation: 0,
            offset: 0,
        }
    }

    /// The events written since the previous call, oldest first. A generation is finished once
    /// a newer one exists, since only one writer runs at a time.
    pub fn poll(&mut self) -> io::Result<Vec<Event>> {
        let mut events = Vec::new();
        for (generation, path) in generation_files(&self.dir)? {
            if generation < self.generation {
                continue;
            }
            if generation > self.generation {
                self.generation = generation;
                self.offset = 0;
            }
            let (read, offset, _) = read_from(&path, self.offset)?;
            self.offset = offset;
            events.extend(read);
        }
        Ok(events)
    }
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

/// Delete generations older than [`RETENTION`], then the oldest while the total exceeds
/// [`RETENTION_BYTES`]. Returns the generations removed.
fn prune(dir: &Path, retention: Duration, cap: u64) -> io::Result<Vec<u64>> {
    let now = SystemTime::now();
    let mut files: Vec<(u64, PathBuf, u64, SystemTime)> = Vec::new();
    for (generation, path) in generation_files(dir)? {
        let metadata = fs::metadata(&path)?;
        files.push((
            generation,
            path,
            metadata.len(),
            metadata.modified().unwrap_or(now),
        ));
    }
    let mut removed = Vec::new();
    let mut total: u64 = files.iter().map(|file| file.2).sum();
    for (generation, path, size, modified) in &files {
        let old = now.duration_since(*modified).unwrap_or_default() > retention;
        if old || total > cap {
            fs::remove_file(path)?;
            total = total.saturating_sub(*size);
            removed.push(*generation);
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
        let files = generation_files(dir)?;
        if let Some((generation, path)) = files.last() {
            last_generation = last_generation.max(*generation);
            if let Some(seq) = repair_tail(path)? {
                last_seq = last_seq.max(seq);
            }
            if let Ok((events, _, _)) = read_from(path, 0) {
                if let Some(max_turn) = events.iter().map(|event| event.turn).max() {
                    turn = turn.max(max_turn);
                }
            }
        }
        let pruned = prune(dir, retention, cap)?;
        let generation = last_generation + 1;
        let path = dir.join(format!("{generation}.jsonl"));
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
        };
        writer.write_index()?;
        Ok(writer)
    }

    /// This writer's generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
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
        let durable = ["input.", "turn.", "lifecycle."]
            .iter()
            .any(|prefix| kind.starts_with(prefix));
        if durable {
            self.file.sync_data()?;
        }
        if kind.starts_with("turn.") || kind.starts_with("lifecycle.") {
            self.write_index()?;
        }
        Ok(seq)
    }

    /// Rewrite `index.json` atomically.
    pub fn write_index(&self) -> io::Result<()> {
        let generations: Vec<Value> = generation_files(&self.dir)?
            .into_iter()
            .map(|(generation, path)| {
                json!({
                    "generation": generation,
                    "file": path.file_name().map(|name| name.to_string_lossy().into_owned()),
                })
            })
            .collect();
        let index = json!({
            "schema": INDEX_SCHEMA,
            "agent": self.agent,
            "last_seq": self.next_seq.saturating_sub(1),
            "last_generation": self.generation,
            "last_turn": self.turn,
            "current_first_seq": self.first_seq,
            "generations": generations,
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
}
