//! Session lineage: the harness sessions one agent name has run, in order (#686 agent-turnover;
//! design in `ai_docs/transient/2026-10-29-agent-turnover/README.md`).
//!
//! ```text
//! REGISTRY/.lineage/<name>/entries.jsonl   append-only, one JSON object per line
//! ```
//!
//! The directory sits beside the records, not inside an agent's directory, because `stop` and
//! `revive` archive and replace that directory at every generation while the lineage outlives
//! them. Its leading dot keeps it out of every build's scan for layout-1 agents, so no build
//! mistakes it for an agent named `lineage`.
//!
//! The lineage is advisory. Routing, liveness and locks read `agent.json` alone; an entry that
//! cannot be written is a warning and changes no command's outcome, and `history` fills any gap
//! from `REGISTRY/archive/` and the live record, marking those generations `derived`.
//!
//! Entries are written after a lifecycle command ([`Observation`]): the records the command can
//! touch are read before it runs and again after, and the difference becomes entries. A
//! generation (`kind: generation`) has a `relation` to the one before it: `start`, `adopt`,
//! `resume` (revived on the same session), `fresh` (revived on another or no session),
//! `rebound` (the record's session changed in place), `takeover` or `handback`. A `line` groups
//! the generations of one continuous piece of work; `start` begins a new one, so a reused name
//! does not run two unrelated agents together. `ended` closes a generation and `renamed` links
//! two names.
//!
//! Readers ignore keys and kinds they do not know. A writer refuses to append to a file holding a
//! line of another major schema (`agentctl-lineage/v2`), and says so.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde_json::{json, Map, Value};

use agent_queue::registry;

/// The lineage directory inside a registry.
pub const DIRECTORY: &str = ".lineage";
/// The schema every entry this build writes carries.
pub const SCHEMA: &str = "agentctl-lineage/v1";
const SCHEMA_FAMILY: &str = "agentctl-lineage/";
const ENTRIES: &str = "entries.jsonl";
/// How many renames `history` follows back.
const RENAME_DEPTH: usize = 8;
/// How long an append waits for another writer's lock before giving up with a warning.
const LOCK_WAIT: Duration = Duration::from_secs(5);

type Result<T> = std::result::Result<T, String>;

/// `REGISTRY/.lineage/<name>/entries.jsonl`.
#[must_use]
pub fn entries_path(registry_root: &Path, name: &str) -> PathBuf {
    registry_root.join(DIRECTORY).join(name).join(ENTRIES)
}

/// `REGISTRY/.lineage/<name>/turnover.json`: the journal of a turnover in progress.
#[must_use]
pub fn turnover_path(registry_root: &Path, name: &str) -> PathBuf {
    registry_root
        .join(DIRECTORY)
        .join(name)
        .join("turnover.json")
}

/// `REGISTRY/.lineage/<name>`, created (mode 0700) when missing.
///
/// # Errors
///
/// An invalid name, or a directory that cannot be created safely.
pub fn name_directory(registry_root: &Path, name: &str) -> Result<PathBuf> {
    if !registry::is_agent_name(name) {
        return Err(format!("{name:?} is not an agent name"));
    }
    let directory = registry_root.join(DIRECTORY);
    agent_queue::fsio::create_private_directory(&directory, "agent lineage", false, true)
        .map_err(|error| error.to_string())?;
    let own = directory.join(name);
    agent_queue::fsio::create_private_directory(&own, "agent lineage", false, true)
        .map_err(|error| error.to_string())?;
    Ok(own)
}

/// `REGISTRY/.lineage/<name>/handoffs`, created (mode 0700) when missing: one handoff per
/// turnover, `<seq>.md`.
///
/// # Errors
///
/// As for [`name_directory`].
pub fn handoffs_directory(registry_root: &Path, name: &str) -> Result<PathBuf> {
    let handoffs = name_directory(registry_root, name)?.join("handoffs");
    agent_queue::fsio::create_private_directory(&handoffs, "agent handoffs", false, true)
        .map_err(|error| error.to_string())?;
    Ok(handoffs)
}

/// One generation of an agent as its record shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Generation {
    /// The record's token, which names the generation.
    pub token: String,
    /// The harness (`claude`, `codex`, ...).
    pub harness: Option<String>,
    /// The model, when recorded.
    pub model: Option<String>,
    /// The harness's conversation ID, when recorded.
    pub session: Option<String>,
    /// The agent's working directory.
    pub cwd: Option<String>,
    /// When the record was created (Unix seconds).
    pub created_at: Option<f64>,
}

fn text(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// The conversation a record names: a headless agent's session, else its native session, else
/// its flat session or resume value, else a session bound in place (`goal_session_id`), else
/// the one its launch specification names.
#[must_use]
pub fn session_of(record: &Value) -> Option<String> {
    [
        "/headless/session",
        "/native_session/value",
        "/session_value",
        "/resume",
        "/goal_session_id",
        "/launch_spec/native_session/value",
        "/launch/resume",
    ]
    .iter()
    .find_map(|pointer| text(record, pointer))
}

/// The generation a record (flat or nested row) describes, or `None` without a token.
#[must_use]
pub fn generation_of(record: &Value) -> Option<Generation> {
    Some(Generation {
        token: text(record, "/token")?,
        harness: text(record, "/harness").or_else(|| text(record, "/launch/harness")),
        model: text(record, "/model").or_else(|| text(record, "/launch/model")),
        session: session_of(record),
        cwd: text(record, "/cwd").or_else(|| text(record, "/launch/cwd")),
        created_at: record.get("created_at").and_then(Value::as_f64),
    })
}

fn read_record(directory: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(directory.join("agent.json")).ok()?).ok()
}

/// The live generation of `name`, read without a lock (records are replaced atomically).
fn live(registry_root: &Path, name: &str) -> Option<Generation> {
    let directory = registry::directory(registry_root, name).ok()?;
    generation_of(&read_record(&directory)?)
}

/// A lifecycle command whose effect on the records becomes lineage entries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    /// `start NAME`.
    Start(String),
    /// `adopt NAME`.
    Adopt(String),
    /// `stop NAME`.
    Stop(String),
    /// `revive NAME`, or `revive --all` when `None`.
    Revive(Option<String>),
    /// `rename FROM TO`.
    Rename(String, String),
    /// `restore`: revives any agent.
    Restore,
    /// `takeover NAME`.
    Takeover(String),
    /// `handback NAME`.
    Handback(String),
    /// `bind-session` or `anchor`: the session may change in place.
    Bind(String),
    /// `retire-stale-registration NAME`: the record is archived without stopping anything.
    Retire(String),
}

impl Action {
    /// The names whose records the command can change; `None` means every agent.
    fn scope(&self) -> Option<Vec<String>> {
        match self {
            Self::Start(name)
            | Self::Adopt(name)
            | Self::Stop(name)
            | Self::Takeover(name)
            | Self::Handback(name)
            | Self::Bind(name)
            | Self::Retire(name)
            | Self::Revive(Some(name)) => Some(vec![name.clone()]),
            Self::Rename(from, to) => Some(vec![from.clone(), to.clone()]),
            Self::Revive(None) | Self::Restore => None,
        }
    }
}

/// The records a command can touch, read before it runs; [`Observation::after`] writes the
/// difference as entries.
#[derive(Debug)]
pub struct Observation {
    registry: PathBuf,
    action: Action,
    before: BTreeMap<String, Generation>,
}

fn snapshot(registry_root: &Path, scope: Option<&[String]>) -> BTreeMap<String, Generation> {
    let names = match scope {
        Some(names) => names.to_vec(),
        None => registry::names(registry_root).unwrap_or_default(),
    };
    names
        .into_iter()
        .filter(|name| registry::is_agent_name(name))
        .filter_map(|name| live(registry_root, &name).map(|generation| (name, generation)))
        .collect()
}

/// One entry to append, before [`append`] gives it a sequence number, a time and a line.
#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
    /// The name whose file receives it.
    pub name: String,
    /// The entry's fields.
    pub entry: Map<String, Value>,
}

fn generation_fields(generation: &Generation) -> Map<String, Value> {
    let mut fields = Map::new();
    fields.insert("token".to_owned(), json!(generation.token));
    for (key, value) in [
        ("harness", &generation.harness),
        ("model", &generation.model),
        ("session", &generation.session),
        ("cwd", &generation.cwd),
    ] {
        if let Some(value) = value {
            fields.insert(key.to_owned(), json!(value));
        }
    }
    fields
}

fn generation_entry(
    name: &str,
    generation: &Generation,
    relation: &str,
    predecessor: Option<&Generation>,
) -> Planned {
    let mut entry = generation_fields(generation);
    entry.insert("kind".to_owned(), json!("generation"));
    entry.insert("relation".to_owned(), json!(relation));
    if let Some(previous) = predecessor {
        entry.insert(
            "predecessor".to_owned(),
            json!({"token": previous.token, "session": previous.session}),
        );
    }
    Planned {
        name: name.to_owned(),
        entry,
    }
}

fn ended_entry(name: &str, generation: &Generation, reason: &str) -> Planned {
    let mut entry = generation_fields(generation);
    entry.insert("kind".to_owned(), json!("ended"));
    entry.insert("reason".to_owned(), json!(reason));
    Planned {
        name: name.to_owned(),
        entry,
    }
}

impl Observation {
    /// Read the records `action` can touch.
    #[must_use]
    pub fn before(registry_root: &Path, action: Action) -> Self {
        let before = snapshot(registry_root, action.scope().as_deref());
        Self {
            registry: registry_root.to_path_buf(),
            action,
            before,
        }
    }

    /// Read the records again and append what changed. `succeeded` says whether the command
    /// reported success: a takeover or handback changes no record field, so it is recorded only
    /// then. Returns one warning per entry that could not be written; the lineage is advisory,
    /// so the caller only prints them.
    #[must_use]
    pub fn after(self, succeeded: bool) -> Vec<String> {
        let after = snapshot(&self.registry, self.action.scope().as_deref());
        let mut warnings = Vec::new();
        for planned in self.plan(&after, succeeded) {
            if let Err(error) = append(&self.registry, &planned.name, planned.entry) {
                warnings.push(format!(
                    "agentctl: lineage of {} not recorded: {error}",
                    planned.name
                ));
            }
        }
        warnings
    }

    /// The entries the change from `self.before` to `after` calls for, in the order to write.
    #[must_use]
    pub fn plan(&self, after: &BTreeMap<String, Generation>, succeeded: bool) -> Vec<Planned> {
        if let Action::Rename(from, to) = &self.action {
            return match (self.before.get(from), after.get(from), after.get(to)) {
                (Some(old), None, Some(new)) => {
                    let mut away = generation_fields(old);
                    away.insert("kind".to_owned(), json!("renamed"));
                    away.insert("to".to_owned(), json!(to));
                    let mut here = generation_fields(new);
                    here.insert("kind".to_owned(), json!("renamed"));
                    here.insert("from".to_owned(), json!(from));
                    vec![
                        Planned {
                            name: from.clone(),
                            entry: away,
                        },
                        Planned {
                            name: to.clone(),
                            entry: here,
                        },
                    ]
                }
                _ => Vec::new(),
            };
        }
        let names: BTreeSet<&String> = self.before.keys().chain(after.keys()).collect();
        let mut planned = Vec::new();
        for name in names {
            match (self.before.get(name), after.get(name)) {
                (None, Some(new)) => {
                    let relation = match &self.action {
                        Action::Start(_) => "start",
                        Action::Adopt(_) => "adopt",
                        // Revive and restore do not create names: another process did.
                        _ => continue,
                    };
                    planned.push(generation_entry(name, new, relation, None));
                }
                (Some(old), None) => match &self.action {
                    Action::Stop(_) => planned.push(ended_entry(name, old, "stopped")),
                    Action::Retire(_) => planned.push(ended_entry(name, old, "retired")),
                    _ => {}
                },
                (Some(old), Some(new)) if old.token != new.token => {
                    // Revive replaces only a dead generation.
                    planned.push(ended_entry(name, old, "died"));
                    let relation = if old.session.is_some() && old.session == new.session {
                        "resume"
                    } else if matches!(self.action, Action::Restore) {
                        // Restore writes this entry itself, with its reason; this one repeats it.
                        "fresh-after-restore"
                    } else {
                        "fresh"
                    };
                    planned.push(generation_entry(name, new, relation, Some(old)));
                }
                (Some(old), Some(new)) if old.session != new.session => {
                    planned.push(generation_entry(name, new, "rebound", Some(old)));
                }
                (Some(old), Some(new)) => match &self.action {
                    Action::Takeover(_) if succeeded => {
                        planned.push(generation_entry(name, new, "takeover", Some(old)));
                    }
                    Action::Handback(_) if succeeded => {
                        planned.push(generation_entry(name, new, "handback", Some(old)));
                    }
                    _ => {}
                },
                (None, None) => {}
            }
        }
        planned
    }
}

/// What a lineage file holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recorded {
    /// Every readable entry, in file order.
    pub entries: Vec<Value>,
    /// Lines that are not JSON objects (a torn last line, or damage).
    pub unreadable: usize,
    /// Schemas of another major version found in the file.
    pub foreign: BTreeSet<String>,
}

fn parse(bytes: &[u8]) -> Recorded {
    let mut recorded = Recorded::default();
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<Value>(line) {
            Ok(value) if value.is_object() => {
                if let Some(schema) = value.get("schema").and_then(Value::as_str) {
                    if schema.starts_with(SCHEMA_FAMILY) && schema != SCHEMA {
                        recorded.foreign.insert(schema.to_owned());
                    }
                }
                recorded.entries.push(value);
            }
            _ => recorded.unreadable += 1,
        }
    }
    recorded
}

/// Read `name`'s lineage file; an absent file is an empty lineage.
///
/// # Errors
///
/// The file exists but cannot be read.
pub fn read(registry_root: &Path, name: &str) -> Result<Recorded> {
    let path = entries_path(registry_root, name);
    match fs::read(&path) {
        Ok(bytes) => Ok(parse(&bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Recorded::default()),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

/// A new line identifier, `ln-` and twelve hexadecimal digits.
fn new_line() -> String {
    let mut bytes = [0_u8; 6];
    let random = fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes));
    if random.is_err() {
        let seed = now().to_bits() ^ u64::from(std::process::id()).rotate_left(32);
        bytes.copy_from_slice(&seed.to_le_bytes()[..6]);
    }
    let digits: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("ln-{digits}")
}

fn last_line(entries: &[Value]) -> Option<String> {
    entries.iter().rev().find_map(|entry| text(entry, "/line"))
}

/// The line a new entry joins: a `start` begins one; an `adopt` continues the last line only
/// when it adopts the session that line last ran; a `renamed` entry continues the old name's
/// line; anything else continues the current line, or begins one when the name has none.
fn line_for(registry_root: &Path, entries: &[Value], entry: &Map<String, Value>) -> String {
    let kind = entry
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let relation = entry
        .get("relation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if kind == "renamed" {
        if let Some(from) = entry.get("from").and_then(Value::as_str) {
            if let Ok(previous) = read(registry_root, from) {
                if let Some(line) = last_line(&previous.entries) {
                    return line;
                }
            }
        }
    }
    match (kind, relation) {
        ("generation", "start") => new_line(),
        ("generation", "adopt") => {
            let last_session = entries
                .iter()
                .rev()
                .find(|entry| entry.get("kind").and_then(Value::as_str) == Some("generation"))
                .and_then(|entry| text(entry, "/session"));
            match last_session {
                Some(session) if entry.get("session").and_then(Value::as_str) == Some(&session) => {
                    last_line(entries).unwrap_or_else(new_line)
                }
                _ => new_line(),
            }
        }
        _ => last_line(entries).unwrap_or_else(new_line),
    }
}

/// The sequence number of an entry that already records the change `entry` describes. Two
/// commands acting on one agent at once (a boot restore and a manual revive) each see the same
/// change and would otherwise record it twice. A takeover or handback can recur on one
/// generation, so those are never treated as repeats.
fn already_recorded(entries: &[Value], entry: &Map<String, Value>) -> Option<u64> {
    let field = |key: &str| entry.get(key).and_then(Value::as_str);
    let same = |existing: &Value, keys: &[&str]| {
        keys.iter()
            .all(|key| existing.get(*key).and_then(Value::as_str) == field(key))
    };
    let keys: &[&str] = match (field("kind")?, field("relation")) {
        ("generation", Some("takeover" | "handback")) => return None,
        ("generation", _) => &["kind", "relation", "token", "session"],
        ("ended", _) => &["kind", "token", "session"],
        ("renamed", _) => &["kind", "token", "from", "to"],
        _ => return None,
    };
    entries
        .iter()
        .rev()
        .find(|existing| same(existing, keys))
        .and_then(|existing| existing.get("seq").and_then(Value::as_u64))
}

/// Append `entry` to `name`'s lineage, giving it the schema, a sequence number, a time, a line
/// and the writing build. Returns the sequence number, or that of an entry already recording the
/// same change.
///
/// # Errors
///
/// An invalid name, a lineage directory that cannot be created safely, a file holding entries of
/// another major schema, or a failed write.
pub fn append(registry_root: &Path, name: &str, mut entry: Map<String, Value>) -> Result<u64> {
    let path = name_directory(registry_root, name)?.join(ENTRIES);
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    // A FIFO or device here would block the read below and hold up a finished command.
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err(format!("{} is not a regular file", path.display()));
    }
    let deadline = Instant::now() + LOCK_WAIT;
    while let Err(error) = file.try_lock_exclusive() {
        if Instant::now() >= deadline {
            return Err(format!(
                "{} stayed locked for {} s ({error})",
                path.display(),
                LOCK_WAIT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut bytes = Vec::new();
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.read_to_end(&mut bytes))
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let recorded = parse(&bytes);
    if let Some(schema) = recorded.foreign.iter().next() {
        return Err(format!(
            "{} holds {schema} entries, which this build does not write beside; upgrade agentctl",
            path.display()
        ));
    }
    if let Some(seq) = already_recorded(&recorded.entries, &entry) {
        return Ok(seq);
    }
    let seq = recorded
        .entries
        .iter()
        .filter_map(|entry| entry.get("seq").and_then(Value::as_u64))
        .max()
        .unwrap_or(0)
        + 1;
    let line = line_for(registry_root, &recorded.entries, &entry);
    entry.insert("schema".to_owned(), json!(SCHEMA));
    entry.insert("seq".to_owned(), json!(seq));
    entry.insert("at".to_owned(), json!(now()));
    entry.insert("name".to_owned(), json!(name));
    entry.insert("line".to_owned(), json!(line));
    entry.insert(
        "by".to_owned(),
        json!(format!("agentctl {}", env!("CARGO_PKG_VERSION"))),
    );
    let mut text = serde_json::to_vec(&Value::Object(entry)).map_err(|error| error.to_string())?;
    text.push(b'\n');
    // A torn last line would swallow this entry; end it first.
    if bytes.last().is_some_and(|byte| *byte != b'\n') {
        text.insert(0, b'\n');
    }
    file.write_all(&text)
        .and_then(|()| file.sync_data())
        .map_err(|error| format!("cannot append to {}: {error}", path.display()))?;
    Ok(seq)
}

/// Generations of `name` that only `REGISTRY/archive/` and, with `include_live`, the live
/// record show.
fn archived(
    registry_root: &Path,
    name: &str,
    include_live: bool,
) -> Vec<(Generation, &'static str, String)> {
    let mut found = Vec::new();
    let archive = registry_root.join("archive");
    if let Ok(entries) = fs::read_dir(&archive) {
        for entry in entries.flatten() {
            let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !file_name.starts_with(&format!("{name}-")) {
                continue;
            }
            let Some(record) = read_record(&entry.path()) else {
                continue;
            };
            if record.get("name").and_then(Value::as_str) != Some(name) {
                continue;
            }
            if let Some(generation) = generation_of(&record) {
                found.push((generation, "archived", name.to_owned()));
            }
        }
    }
    if include_live {
        if let Some(generation) = live(registry_root, name) {
            found.push((generation, "live", name.to_owned()));
        }
    }
    found
}

fn transcript(generation_entry: &Value) -> Option<String> {
    let harness = text(generation_entry, "/harness")?;
    let session = text(generation_entry, "/session")?;
    let cwd = text(generation_entry, "/cwd").map(PathBuf::from);
    let home = crate::harness::home();
    let hints = crate::harness::transcript::Hints {
        session: Some(&session),
        cwd: cwd.as_deref(),
        home: home.as_deref(),
        pid: None,
        proc: None,
    };
    crate::harness::adapter(&harness)?
        .transcript_path(&hints)
        .map(|path| path.display().to_string())
}

/// Entries of `name`, with what each name it was renamed from ran before the rename. `names`
/// collects every name visited, each once, so a rename back and forth is read only once.
fn recorded_with_renames(
    registry_root: &Path,
    name: &str,
    depth: usize,
    names: &mut Vec<String>,
    problems: &mut Vec<String>,
) -> Vec<Value> {
    if names.iter().any(|seen| seen == name) {
        return Vec::new();
    }
    names.push(name.to_owned());
    let recorded = match read(registry_root, name) {
        Ok(recorded) => recorded,
        Err(error) => {
            problems.push(error);
            return Vec::new();
        }
    };
    if recorded.unreadable > 0 {
        problems.push(format!(
            "{} unreadable line(s) in {}",
            recorded.unreadable,
            entries_path(registry_root, name).display()
        ));
    }
    let mut entries = Vec::new();
    if depth < RENAME_DEPTH {
        let sources: Vec<String> = recorded
            .entries
            .iter()
            .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("renamed"))
            .filter_map(|entry| text(entry, "/from"))
            .collect();
        for from in sources {
            let earlier = recorded_with_renames(registry_root, &from, depth + 1, names, problems);
            // Only what the old name ran until it last became this one.
            let cut = earlier
                .iter()
                .rposition(|entry| {
                    entry.get("kind").and_then(Value::as_str) == Some("renamed")
                        && entry.get("to").and_then(Value::as_str) == Some(name)
                })
                .map_or(earlier.len(), |index| index + 1);
            entries.extend(earlier.into_iter().take(cut));
        }
    }
    entries.extend(recorded.entries);
    entries
}

/// `name`'s history: recorded entries, generations derived from the archive and the live record
/// where nothing recorded them, each generation's status and transcript. Without `all`, only the
/// current line, unless that line did not begin with `start` or `adopt` (its earlier history is
/// then unknown, and everything is shown).
///
/// # Errors
///
/// An invalid name.
pub fn history(registry_root: &Path, name: &str, all: bool) -> Result<Value> {
    if !registry::is_agent_name(name) {
        return Err(format!(
            "invalid agent name {name:?}: use 1-32 lowercase letters, digits, and hyphens"
        ));
    }
    let mut problems = Vec::new();
    let mut names = Vec::new();
    let mut entries = recorded_with_renames(registry_root, name, 0, &mut names, &mut problems);
    let recorded_tokens: BTreeSet<String> = entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.get("kind").and_then(Value::as_str),
                Some("generation" | "renamed")
            )
        })
        .filter_map(|entry| text(entry, "/token"))
        .collect();
    // A generation is its token and session: a turnover keeps a headless agent's token.
    let key = |entry: &Value| (text(entry, "/token"), text(entry, "/session"));
    let ended: BTreeMap<(Option<String>, Option<String>), String> = entries
        .iter()
        .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("ended"))
        .filter_map(|entry| Some((key(entry), text(entry, "/reason")?)))
        .collect();
    let mut statuses: BTreeMap<String, &str> = BTreeMap::new();
    // Live generations: this name's, and those of names it was renamed to.
    let mut live_as: BTreeMap<(Option<String>, Option<String>), String> = BTreeMap::new();
    let targets: BTreeSet<String> = entries
        .iter()
        .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("renamed"))
        .filter_map(|entry| text(entry, "/to"))
        .chain(std::iter::once(name.to_owned()))
        .collect();
    for target in targets {
        if let Some(generation) = live(registry_root, &target) {
            live_as.insert((Some(generation.token), generation.session), target);
        }
    }
    let found = names
        .iter()
        .flat_map(|former| archived(registry_root, former, former == name))
        .collect::<Vec<_>>();
    for (generation, status, ran_as) in found {
        statuses.insert(generation.token.clone(), status);
        if recorded_tokens.contains(&generation.token) {
            continue;
        }
        let mut entry = generation_fields(&generation);
        entry.insert("kind".to_owned(), json!("generation"));
        entry.insert("relation".to_owned(), json!("unknown"));
        entry.insert("derived".to_owned(), json!(true));
        entry.insert("name".to_owned(), json!(ran_as));
        if let Some(at) = generation.created_at {
            entry.insert("at".to_owned(), json!(at));
        }
        entries.push(Value::Object(entry));
    }
    let at = |entry: &Value| entry.get("at").and_then(Value::as_f64).unwrap_or(0.0);
    entries.sort_by(|left, right| at(left).total_cmp(&at(right)));
    let current_line = last_line(&entries);
    let line_begins_fresh = current_line.as_ref().is_some_and(|line| {
        entries
            .iter()
            .find(|entry| text(entry, "/line").as_deref() == Some(line))
            .is_some_and(|first| {
                matches!(
                    first.get("relation").and_then(Value::as_str),
                    Some("start" | "adopt")
                )
            })
    });
    if !all && line_begins_fresh {
        // The current line, and what a build without lineage did since it began.
        let line = current_line.clone();
        let began = entries
            .iter()
            .find(|entry| text(entry, "/line") == line)
            .map_or(0.0, at);
        entries.retain(|entry| {
            text(entry, "/line") == line || (entry["derived"] == true && at(entry) >= began)
        });
    }
    for entry in &mut entries {
        if entry.get("kind").and_then(Value::as_str) != Some("generation") {
            continue;
        }
        let token = text(entry, "/token").unwrap_or_default();
        let status = if let Some(target) = live_as.get(&key(entry)) {
            if target == name {
                "live".to_owned()
            } else {
                format!("live as {target}")
            }
        } else if let Some(reason) = ended.get(&key(entry)) {
            reason.clone()
        } else {
            statuses
                .get(&token)
                .copied()
                .unwrap_or("unknown")
                .to_owned()
        };
        let transcript = transcript(entry);
        if let Some(object) = entry.as_object_mut() {
            object.insert("status".to_owned(), json!(status));
            if let Some(path) = transcript {
                object.insert("transcript".to_owned(), json!(path));
            }
        }
    }
    let sessions = entries
        .iter()
        .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("generation"))
        .filter_map(|entry| text(entry, "/session"))
        .collect::<BTreeSet<_>>()
        .len();
    Ok(json!({
        "name": name,
        "line": current_line,
        "all": all,
        "sessions": sessions,
        "entries": entries,
        "problems": problems,
    }))
}

/// Every agent that ran conversation `session`, from the lineage files, the archive and the live
/// records.
///
/// # Errors
///
/// None today; reserved for unreadable registries.
pub fn session_owners(registry_root: &Path, session: &str) -> Result<Value> {
    let mut owners: BTreeMap<(String, String), Value> = BTreeMap::new();
    let mut add = |name: &str, token: String, source: &str| {
        owners
            .entry((name.to_owned(), token.clone()))
            .or_insert_with(|| json!({"name": name, "token": token, "source": source}));
    };
    if let Ok(names) = fs::read_dir(registry_root.join(DIRECTORY)) {
        for entry in names.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !registry::is_agent_name(&name) {
                continue;
            }
            for recorded in read(registry_root, &name).unwrap_or_default().entries {
                if text(&recorded, "/session").as_deref() == Some(session) {
                    if let Some(token) = text(&recorded, "/token") {
                        add(&name, token, "lineage");
                    }
                }
            }
        }
    }
    if let Ok(entries) = fs::read_dir(registry_root.join("archive")) {
        for entry in entries.flatten() {
            if let Some(record) = read_record(&entry.path()) {
                if session_of(&record).as_deref() == Some(session) {
                    if let (Some(name), Some(token)) =
                        (text(&record, "/name"), text(&record, "/token"))
                    {
                        add(&name, token, "archive");
                    }
                }
            }
        }
    }
    for name in registry::names(registry_root).unwrap_or_default() {
        if let Some(generation) = live(registry_root, &name) {
            if generation.session.as_deref() == Some(session) {
                add(&name, generation.token, "live");
            }
        }
    }
    Ok(json!({"session": session, "owners": owners.into_values().collect::<Vec<_>>()}))
}

/// Local `YYYY-MM-DD HH:MM ZONE` of a Unix time.
fn local_time(seconds: f64) -> String {
    let time = seconds as libc::time_t;
    // SAFETY: localtime_r writes only into the zeroed `tm` it is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        return "????-??-?? ??:??".to_owned();
    }
    let zone = crate::prompt_time::process_zone(seconds as i64)
        .map(|zone| format!(" {}", zone.name))
        .unwrap_or_default();
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}{zone}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min
    )
}

fn short(token: &str) -> &str {
    token.get(..12).unwrap_or(token)
}

/// `history` as text: one line per entry, oldest first, with each generation's transcript under
/// it.
#[must_use]
pub fn render(history: &Value) -> String {
    let mut out = String::new();
    let name = history["name"].as_str().unwrap_or_default();
    let entries = history["entries"].as_array().cloned().unwrap_or_default();
    out.push_str(&format!(
        "{name}: {} session(s){}{}\n",
        history["sessions"].as_u64().unwrap_or(0),
        history["line"]
            .as_str()
            .map(|line| format!(", line {line}"))
            .unwrap_or_default(),
        if history["all"] == true {
            ", all lines"
        } else {
            ""
        },
    ));
    if entries.is_empty() {
        out.push_str("  no lineage entries, archived generations or live record\n");
    }
    for entry in &entries {
        let seq = entry["seq"]
            .as_u64()
            .map_or_else(|| "-".to_owned(), |seq| format!("#{seq}"));
        let when = entry["at"]
            .as_f64()
            .map_or_else(|| "?".to_owned(), local_time);
        let kind = entry["kind"].as_str().unwrap_or("?");
        let token = short(entry["token"].as_str().unwrap_or("-"));
        let session = entry["session"].as_str().unwrap_or("-");
        let detail = match kind {
            "generation" => format!(
                "{:<8} {} {session}  token {token}  {}{}",
                entry["relation"].as_str().unwrap_or("?"),
                entry["harness"].as_str().unwrap_or("?"),
                entry["status"].as_str().unwrap_or("?"),
                if entry["derived"] == true {
                    " (unrecorded)"
                } else {
                    ""
                },
            ),
            "ended" => format!(
                "ended    {session}  token {token}  {}",
                entry["reason"].as_str().unwrap_or("?")
            ),
            "renamed" => match (entry["from"].as_str(), entry["to"].as_str()) {
                (Some(from), _) => format!("renamed  from {from}"),
                (_, Some(to)) => format!("renamed  to {to}"),
                _ => "renamed".to_owned(),
            },
            other => other.to_owned(),
        };
        out.push_str(&format!("  {seq:>4}  {when}  {detail}\n"));
        if let Some(path) = entry["transcript"].as_str() {
            out.push_str(&format!("               transcript {path}\n"));
        }
        if let Some(path) = entry["handoff"]["path"].as_str() {
            out.push_str(&format!("               handoff {path}\n"));
        }
    }
    for problem in history["problems"].as_array().into_iter().flatten() {
        out.push_str(&format!("  warning: {}\n", problem.as_str().unwrap_or("?")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agentctl-lineage-{label}-{}-{}",
            std::process::id(),
            new_line()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn generation(token: &str, session: Option<&str>) -> Generation {
        Generation {
            token: token.to_owned(),
            harness: Some("claude".to_owned()),
            model: None,
            session: session.map(str::to_owned),
            cwd: Some("/work".to_owned()),
            created_at: Some(100.0),
        }
    }

    fn observation(action: Action, before: &[(&str, Generation)]) -> Observation {
        Observation {
            registry: PathBuf::from("/nonexistent"),
            action,
            before: before
                .iter()
                .map(|(name, generation)| ((*name).to_owned(), generation.clone()))
                .collect(),
        }
    }

    fn kinds(planned: &[Planned]) -> Vec<String> {
        planned
            .iter()
            .map(|planned| {
                format!(
                    "{}:{}:{}",
                    planned.name,
                    planned.entry["kind"].as_str().unwrap(),
                    planned
                        .entry
                        .get("relation")
                        .or_else(|| planned.entry.get("reason"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                )
            })
            .collect()
    }

    fn map(pairs: &[(&str, Generation)]) -> BTreeMap<String, Generation> {
        pairs
            .iter()
            .map(|(name, generation)| ((*name).to_owned(), generation.clone()))
            .collect()
    }

    #[test]
    fn plans_follow_what_each_command_changed() {
        let a = generation("t1", Some("s1"));
        let start = observation(Action::Start("to-a".to_owned()), &[]);
        assert_eq!(
            kinds(&start.plan(&map(&[("to-a", a.clone())]), true)),
            ["to-a:generation:start"]
        );
        // A failed start that left nothing behind records nothing.
        assert!(start.plan(&BTreeMap::new(), false).is_empty());

        let stop = observation(Action::Stop("to-a".to_owned()), &[("to-a", a.clone())]);
        assert_eq!(
            kinds(&stop.plan(&BTreeMap::new(), true)),
            ["to-a:ended:stopped"]
        );

        let revive = observation(Action::Revive(None), &[("to-a", a.clone())]);
        let resumed = generation("t2", Some("s1"));
        let planned = revive.plan(&map(&[("to-a", resumed)]), true);
        assert_eq!(
            kinds(&planned),
            ["to-a:ended:died", "to-a:generation:resume"]
        );
        assert_eq!(
            planned[1].entry["predecessor"],
            json!({"token": "t1", "session": "s1"})
        );
        let fresh = generation("t3", Some("s9"));
        assert_eq!(
            kinds(&revive.plan(&map(&[("to-a", fresh)]), true)),
            ["to-a:ended:died", "to-a:generation:fresh"]
        );
        // A name another process started during `revive --all` is not this command's.
        assert!(revive
            .plan(
                &map(&[("to-a", a.clone()), ("to-b", generation("t4", None))]),
                true
            )
            .is_empty());

        let bind = observation(
            Action::Bind("to-a".to_owned()),
            &[("to-a", generation("t1", None))],
        );
        assert_eq!(
            kinds(&bind.plan(&map(&[("to-a", a.clone())]), true)),
            ["to-a:generation:rebound"]
        );

        let takeover = observation(Action::Takeover("to-a".to_owned()), &[("to-a", a.clone())]);
        assert_eq!(
            kinds(&takeover.plan(&map(&[("to-a", a.clone())]), true)),
            ["to-a:generation:takeover"]
        );
        assert!(takeover
            .plan(&map(&[("to-a", a.clone())]), false)
            .is_empty());

        let rename = observation(
            Action::Rename("to-a".to_owned(), "to-b".to_owned()),
            &[("to-a", a.clone())],
        );
        let planned = rename.plan(&map(&[("to-b", a)]), true);
        assert_eq!(kinds(&planned), ["to-a:renamed:", "to-b:renamed:"]);
        assert_eq!(planned[0].entry["to"], "to-b");
        assert_eq!(planned[1].entry["from"], "to-a");
    }

    fn entry(kind: &str, relation: &str, token: &str, session: &str) -> Map<String, Value> {
        let mut entry = generation_fields(&generation(token, Some(session)));
        entry.insert("kind".to_owned(), json!(kind));
        entry.insert("relation".to_owned(), json!(relation));
        entry
    }

    #[test]
    fn appends_number_entries_keep_lines_and_survive_a_torn_line() {
        let registry = scratch("append");
        assert_eq!(
            append(&registry, "to-a", entry("generation", "start", "t1", "s1")).unwrap(),
            1
        );
        let path = entries_path(&registry, "to-a");
        let mode = fs::metadata(&path).unwrap().permissions();
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
            0o600
        );
        // A crash left half a line.
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"{\"schema\":\"agentctl-li").unwrap();
        assert_eq!(
            append(&registry, "to-a", entry("generation", "resume", "t2", "s1")).unwrap(),
            2
        );
        let recorded = read(&registry, "to-a").unwrap();
        assert_eq!(recorded.unreadable, 1);
        assert_eq!(recorded.entries.len(), 2);
        assert_eq!(recorded.entries[0]["line"], recorded.entries[1]["line"]);
        assert_eq!(recorded.entries[1]["schema"], SCHEMA);
        // A new start begins a new line; adopting the session the line last ran continues it.
        append(&registry, "to-a", entry("generation", "start", "t3", "s3")).unwrap();
        append(&registry, "to-a", entry("generation", "adopt", "t4", "s3")).unwrap();
        let recorded = read(&registry, "to-a").unwrap();
        assert_ne!(recorded.entries[2]["line"], recorded.entries[1]["line"]);
        assert_eq!(recorded.entries[3]["line"], recorded.entries[2]["line"]);
        assert!(append(
            &registry,
            "Bad Name",
            entry("generation", "start", "t", "s")
        )
        .is_err());
        fs::remove_dir_all(registry).unwrap();
    }

    #[test]
    fn a_newer_major_schema_stops_writers_but_not_readers() {
        let registry = scratch("foreign");
        append(&registry, "to-a", entry("generation", "start", "t1", "s1")).unwrap();
        let path = entries_path(&registry, "to-a");
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"{\"schema\":\"agentctl-lineage/v2\",\"seq\":2,\"kind\":\"novel\"}\n")
            .unwrap();
        let error =
            append(&registry, "to-a", entry("generation", "resume", "t2", "s1")).unwrap_err();
        assert!(error.contains("agentctl-lineage/v2"), "{error}");
        let shown = history(&registry, "to-a", true).unwrap();
        assert_eq!(shown["entries"].as_array().unwrap().len(), 2);
        assert!(render(&shown).contains("novel"));
        fs::remove_dir_all(registry).unwrap();
    }

    fn write_record(directory: &Path, name: &str, token: &str, session: &str, created: f64) {
        fs::create_dir_all(directory).unwrap();
        fs::write(
            directory.join("agent.json"),
            serde_json::to_vec(&json!({
                "name": name, "token": token, "harness": "claude", "cwd": "/nowhere",
                "created_at": created, "schema": 1,
                "native_session": {"schema": "agentctl-native-session/v1", "agent": "claude",
                                   "source": "asserted", "value": session},
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn history_fills_gaps_from_the_archive_and_follows_renames() {
        let registry = scratch("history");
        // Before lineage existed: one archived generation, and a similar name that is not it.
        write_record(&registry.join("archive/to-a-t0"), "to-a", "t0", "s0", 10.0);
        write_record(
            &registry.join("archive/to-a-b-t9"),
            "to-a-b",
            "t9",
            "s9",
            11.0,
        );
        // A rename keeps the token.
        write_record(&registry.join("agents/to-b"), "to-b", "t1", "s1", 30.0);
        append(&registry, "to-a", entry("generation", "resume", "t1", "s1")).unwrap();
        append(&registry, "to-a", {
            let mut away = generation_fields(&generation("t1", Some("s1")));
            away.insert("kind".to_owned(), json!("renamed"));
            away.insert("to".to_owned(), json!("to-b"));
            away
        })
        .unwrap();
        append(&registry, "to-b", {
            let mut here = generation_fields(&generation("t1", Some("s1")));
            here.insert("kind".to_owned(), json!("renamed"));
            here.insert("from".to_owned(), json!("to-a"));
            here
        })
        .unwrap();
        let shown = history(&registry, "to-b", false).unwrap();
        let entries = shown["entries"].as_array().unwrap();
        let kinds: Vec<_> = entries
            .iter()
            .map(|entry| {
                (
                    entry["kind"].as_str().unwrap(),
                    entry["token"].as_str().unwrap(),
                )
            })
            .collect();
        // The line began with a resume, so its earlier, unrecorded history is shown too.
        assert!(kinds.contains(&("generation", "t0")), "{kinds:?}");
        assert!(kinds.contains(&("generation", "t1")), "{kinds:?}");
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| **kind == ("renamed", "t1"))
                .count(),
            2
        );
        assert_eq!(
            kinds
                .iter()
                .filter(|(kind, _)| *kind == "generation")
                .count(),
            2
        );
        assert!(!kinds.iter().any(|(_, token)| *token == "t9"), "{kinds:?}");
        let derived = entries.iter().find(|entry| entry["token"] == "t0").unwrap();
        assert_eq!(derived["derived"], true);
        assert_eq!(derived["status"], "archived");
        let live = entries
            .iter()
            .find(|entry| entry["token"] == "t1" && entry["kind"] == "generation")
            .unwrap();
        assert_eq!(live["status"], "live");
        assert_eq!(live["name"], "to-a");
        assert_eq!(shown["line"], live["line"]);
        let text = render(&shown);
        assert!(text.starts_with("to-b: 2 session(s), line ln-"), "{text}");
        assert!(text.contains("(unrecorded)"), "{text}");

        let owners = session_owners(&registry, "s1").unwrap();
        let names: BTreeSet<_> = owners["owners"]
            .as_array()
            .unwrap()
            .iter()
            .map(|owner| owner["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            names,
            BTreeSet::from(["to-a".to_owned(), "to-b".to_owned()])
        );
        fs::remove_dir_all(registry).unwrap();
    }

    #[test]
    fn a_fresh_start_hides_the_previous_line_unless_all() {
        let registry = scratch("lines");
        append(&registry, "to-a", entry("generation", "start", "t1", "s1")).unwrap();
        append(&registry, "to-a", {
            let mut ended = generation_fields(&generation("t1", Some("s1")));
            ended.insert("kind".to_owned(), json!("ended"));
            ended.insert("reason".to_owned(), json!("stopped"));
            ended
        })
        .unwrap();
        append(&registry, "to-a", entry("generation", "start", "t2", "s2")).unwrap();
        let current = history(&registry, "to-a", false).unwrap();
        assert_eq!(current["entries"].as_array().unwrap().len(), 1);
        let every = history(&registry, "to-a", true).unwrap();
        let entries = every["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0]["status"], "stopped");
        assert_eq!(entries[2]["status"], "unknown");
        fs::remove_dir_all(registry).unwrap();
    }

    #[test]
    fn observations_write_entries_for_real_records() {
        let registry = scratch("observe");
        let observed = Observation::before(&registry, Action::Start("to-a".to_owned()));
        write_record(&registry.join("agents/to-a"), "to-a", "t1", "s1", 5.0);
        assert!(observed.after(true).is_empty());
        let observed = Observation::before(&registry, Action::Stop("to-a".to_owned()));
        fs::rename(registry.join("agents/to-a"), registry.join("archive-to-a")).unwrap();
        assert!(observed.after(true).is_empty());
        let recorded = read(&registry, "to-a").unwrap();
        let kinds: Vec<_> = recorded
            .entries
            .iter()
            .map(|entry| entry["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["generation", "ended"]);
        assert_eq!(recorded.entries[0]["session"], "s1");
        assert_eq!(recorded.entries[1]["line"], recorded.entries[0]["line"]);
        fs::remove_dir_all(registry).unwrap();
    }

    #[test]
    fn sessions_are_found_in_headless_and_bound_records() {
        let headless = json!({"token": "t", "headless": {"schema": "x", "session": "h1"},
                              "launch_spec": {"native_session": {"value": "other"}}});
        assert_eq!(session_of(&headless).as_deref(), Some("h1"));
        let bound = json!({"token": "t", "goal_session_id": "g1"});
        assert_eq!(session_of(&bound).as_deref(), Some("g1"));
        let retire = observation(
            Action::Retire("to-a".to_owned()),
            &[("to-a", generation("t1", None))],
        );
        assert_eq!(
            kinds(&retire.plan(&BTreeMap::new(), true)),
            ["to-a:ended:retired"]
        );
    }

    #[test]
    fn a_change_recorded_twice_is_written_once_and_odd_files_are_refused() {
        let registry = scratch("repeat");
        let ended = || {
            let mut ended = generation_fields(&generation("t1", Some("s1")));
            ended.insert("kind".to_owned(), json!("ended"));
            ended.insert("reason".to_owned(), json!("died"));
            ended
        };
        append(&registry, "to-a", entry("generation", "start", "t1", "s1")).unwrap();
        assert_eq!(append(&registry, "to-a", ended()).unwrap(), 2);
        assert_eq!(append(&registry, "to-a", ended()).unwrap(), 2);
        assert_eq!(
            append(&registry, "to-a", entry("generation", "resume", "t2", "s1")).unwrap(),
            3
        );
        assert_eq!(
            append(&registry, "to-a", entry("generation", "resume", "t2", "s1")).unwrap(),
            3
        );
        // A takeover can happen to one generation more than once.
        append(
            &registry,
            "to-a",
            entry("generation", "takeover", "t2", "s1"),
        )
        .unwrap();
        append(
            &registry,
            "to-a",
            entry("generation", "takeover", "t2", "s1"),
        )
        .unwrap();
        assert_eq!(read(&registry, "to-a").unwrap().entries.len(), 5);
        // A FIFO in place of the file would block the read: refused at once.
        let fifo_dir = registry.join(DIRECTORY).join("to-f");
        fs::create_dir_all(&fifo_dir).unwrap();
        let fifo = std::ffi::CString::new(fifo_dir.join(ENTRIES).to_str().unwrap()).unwrap();
        // SAFETY: mkfifo reads the NUL-terminated path only.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let error = append(&registry, "to-f", entry("generation", "start", "t", "s")).unwrap_err();
        assert!(error.contains("not a regular file"), "{error}");
        fs::remove_dir_all(registry).unwrap();
    }

    #[test]
    fn the_default_view_keeps_what_an_unrecording_build_did_since_the_line_began() {
        let registry = scratch("mixed");
        append(&registry, "to-a", entry("generation", "start", "t1", "s1")).unwrap();
        // Another build revived the agent: archived old generation, a new live record, no entries.
        write_record(&registry.join("archive/to-a-t1"), "to-a", "t1", "s1", 1.0);
        write_record(
            &registry.join("agents/to-a"),
            "to-a",
            "t2",
            "s1",
            now() + 10.0,
        );
        let shown = history(&registry, "to-a", false).unwrap();
        let entries = shown["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2, "{shown:#}");
        assert_eq!(entries[1]["token"], "t2");
        assert_eq!(entries[1]["status"], "live");
        fs::remove_dir_all(registry).unwrap();
    }

    fn renamed(token: &str, key: &str, other: &str) -> Map<String, Value> {
        let mut entry = generation_fields(&generation(token, Some("s1")));
        entry.insert("kind".to_owned(), json!("renamed"));
        entry.insert(key.to_owned(), json!(other));
        entry
    }

    #[test]
    fn renames_back_and_forth_and_reused_names_read_correctly() {
        let registry = scratch("renames");
        // to-a becomes to-b and back again.
        append(&registry, "to-a", entry("generation", "start", "t1", "s1")).unwrap();
        append(&registry, "to-a", renamed("t1", "to", "to-b")).unwrap();
        append(&registry, "to-b", renamed("t1", "from", "to-a")).unwrap();
        append(&registry, "to-b", renamed("t1", "to", "to-a")).unwrap();
        append(&registry, "to-a", renamed("t1", "from", "to-b")).unwrap();
        write_record(&registry.join("agents/to-a"), "to-a", "t1", "s1", 1.0);
        let shown = history(&registry, "to-a", false).unwrap();
        let entries = shown["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 5, "{shown:#}");
        assert_eq!(entries[0]["status"], "live");

        // to-x became to-y and stopped; later to-z became to-y.
        append(&registry, "to-x", entry("generation", "start", "tx", "sx")).unwrap();
        append(&registry, "to-x", renamed("tx", "to", "to-y")).unwrap();
        append(&registry, "to-y", renamed("tx", "from", "to-x")).unwrap();
        append(&registry, "to-z", entry("generation", "start", "tz", "sz")).unwrap();
        append(&registry, "to-z", renamed("tz", "to", "to-y")).unwrap();
        append(&registry, "to-y", renamed("tz", "from", "to-z")).unwrap();
        write_record(&registry.join("agents/to-y"), "to-y", "tz", "sz", 2.0);
        let shown = history(&registry, "to-y", false).unwrap();
        let generations: Vec<_> = shown["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["kind"] == "generation")
            .map(|entry| {
                (
                    entry["token"].as_str().unwrap(),
                    entry["status"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(generations, [("tz", "live")], "{shown:#}");
        // Under its former name the generation shows where it lives now.
        let former = history(&registry, "to-z", false).unwrap();
        assert_eq!(former["entries"][0]["status"], "live as to-y", "{former:#}");
        fs::remove_dir_all(registry).unwrap();
    }
}
