//! Disposable SQLite index over the read authority.
//!
//! The index is never an authority. It tails the authority by cursor,
//! keeps admitted events plus materialized folded rows with posting lists
//! by commit and run, and can always be deleted and rebuilt without
//! changing any verdict. Its schema is versioned; opening an index at any
//! other version discards and rebuilds it. Nothing here is keyed by
//! source-code hashes.

use crate::event::{parse_line, EventKind, RawEvent, Refusal};
use crate::fold::{build_correction_index, emit_terminal_row, row_line_and_hash, CorrectionTarget};
use crate::pyjson::{dumps_sorted, parse, PyValue};
use rusqlite::{params, Connection};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Index schema version stored in the metadata table. `v2` stores sort
/// keys and identity hashes beside each event so refolds stream in union
/// order from disk instead of materialising the history.
pub const INDEX_VERSION: &str = "ci-hub-index/v2";

/// Read position in one shard: bytes consumed and the digest of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShardPos {
    /// Bytes consumed from the shard start.
    pub offset: u64,
    /// SHA-256 hex of the consumed prefix bytes.
    pub prefix_sha256: String,
}

/// Authority cursor: head plus a position per shard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    /// Read-authority connector id.
    pub authority: String,
    /// Authority head token (git commit id for the git connector).
    pub head: String,
    /// Per-shard positions by shard path.
    pub shards: BTreeMap<String, ShardPos>,
}

impl Cursor {
    /// Empty cursor at `head` for `authority` (every shard starts at 0).
    pub fn new(authority: impl Into<String>, head: impl Into<String>) -> Self {
        Self {
            authority: authority.into(),
            head: head.into(),
            shards: BTreeMap::new(),
        }
    }

    /// Serialize for storage and wire transport.
    pub fn to_pyvalue(&self) -> PyValue {
        let shards = PyValue::Object(
            self.shards
                .iter()
                .map(|(path, pos)| {
                    (
                        path.clone(),
                        PyValue::Object(vec![
                            ("offset".to_owned(), PyValue::Int(pos.offset.to_string())),
                            (
                                "prefix_sha256".to_owned(),
                                PyValue::Str(pos.prefix_sha256.clone()),
                            ),
                        ]),
                    )
                })
                .collect(),
        );
        PyValue::Object(vec![
            ("authority".to_owned(), PyValue::Str(self.authority.clone())),
            ("head".to_owned(), PyValue::Str(self.head.clone())),
            ("shards".to_owned(), shards),
        ])
    }

    /// Parse a cursor produced by [`Cursor::to_pyvalue`].
    pub fn from_pyvalue(value: &PyValue) -> Option<Self> {
        let authority = value.get("authority")?.as_str()?.to_owned();
        let head = value.get("head")?.as_str()?.to_owned();
        let mut shards = BTreeMap::new();
        if let Some(PyValue::Object(pairs)) = value.get("shards") {
            let mut by_path: BTreeMap<String, ShardPos> = BTreeMap::new();
            for (path, pos) in pairs {
                let offset = pos.get("offset")?.as_u64()?;
                let prefix = pos.get("prefix_sha256")?.as_str()?.to_owned();
                by_path.insert(
                    path.clone(),
                    ShardPos {
                        offset,
                        prefix_sha256: prefix,
                    },
                );
            }
            shards = by_path;
        }
        Some(Self {
            authority,
            head,
            shards,
        })
    }
}

/// Index failure: a store refusal or a database error. Neither is a verdict.
#[derive(Debug)]
pub enum IndexError {
    /// The fold refused the admitted events.
    Refused(Refusal),
    /// The database failed.
    Database(rusqlite::Error),
    /// The index file could not be managed.
    Io(std::io::Error),
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexError::Refused(refusal) => write!(f, "{refusal}"),
            IndexError::Database(error) => write!(f, "index database: {error}"),
            IndexError::Io(error) => write!(f, "index file: {error}"),
        }
    }
}

impl std::error::Error for IndexError {}

impl From<Refusal> for IndexError {
    fn from(refusal: Refusal) -> Self {
        IndexError::Refused(refusal)
    }
}

impl From<rusqlite::Error> for IndexError {
    fn from(error: rusqlite::Error) -> Self {
        IndexError::Database(error)
    }
}

impl From<std::io::Error> for IndexError {
    fn from(error: std::io::Error) -> Self {
        IndexError::Io(error)
    }
}

/// Result alias for index operations.
pub type Result<T> = std::result::Result<T, IndexError>;

const SCHEMA_SQL: &str = "\
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE shards (path TEXT PRIMARY KEY, offset INTEGER NOT NULL, prefix_sha256 TEXT NOT NULL);
CREATE TABLE events (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  grouping_id TEXT NOT NULL,
  identity TEXT NOT NULL,
  line TEXT NOT NULL,
  sort_emitted TEXT NOT NULL,
  sort_id TEXT NOT NULL,
  event_id_str TEXT,
  correct_candidate INTEGER NOT NULL
);
CREATE INDEX events_by_grouping ON events (grouping_id, seq);
CREATE INDEX events_by_sort ON events (sort_emitted, sort_id, seq);
CREATE INDEX events_by_str_id ON events (event_id_str, seq);
CREATE TABLE rows_out (
  identity TEXT PRIMARY KEY,
  line TEXT NOT NULL,
  commit_id TEXT,
  run_id TEXT,
  host TEXT,
  finished_at TEXT,
  position INTEGER NOT NULL
);
CREATE INDEX rows_by_commit ON rows_out (commit_id, position);
CREATE INDEX rows_by_run ON rows_out (run_id, position);
";

/// Open index over one authority.
pub struct Index {
    conn: Connection,
    workspace_root: String,
    authority: String,
    refold_count: u64,
}

impl Index {
    /// Open (creating when absent) the index at `path` for `authority`.
    ///
    /// An existing database whose stored index version differs from
    /// [`INDEX_VERSION`], or whose authority differs, is discarded and
    /// rebuilt empty: there is no in-place migration, because deleting the
    /// index must always be safe.
    pub fn open(path: &Path, authority: &str, workspace_root: &str) -> Result<Self> {
        if path.exists() && !Self::version_matches(path, authority) {
            std::fs::remove_file(path)?;
            for suffix in ["-wal", "-shm", "-journal"] {
                let sidecar = PathBuf::from(format!("{}{suffix}", path.display()));
                if sidecar.exists() {
                    std::fs::remove_file(sidecar)?;
                }
            }
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let has_meta: bool = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='meta'",
            [],
            |row| row.get::<_, i64>(0),
        )? > 0;
        if !has_meta {
            conn.execute_batch(SCHEMA_SQL)?;
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('index_version', ?1), ('authority', ?2), ('head', ''), ('rows_dirty', '0')",
                params![INDEX_VERSION, authority],
            )?;
        }
        Ok(Self {
            conn,
            workspace_root: workspace_root.to_owned(),
            authority: authority.to_owned(),
            refold_count: 0,
        })
    }

    fn version_matches(path: &Path, authority: &str) -> bool {
        let Ok(conn) = Connection::open(path) else {
            return false;
        };
        let read = |key: &str| -> Option<String> {
            conn.query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .ok()
        };
        read("index_version").as_deref() == Some(INDEX_VERSION)
            && read("authority").as_deref() == Some(authority)
    }

    /// Open an ephemeral in-memory index (tests and one-shot folds).
    pub fn in_memory(authority: &str, workspace_root: &str) -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA_SQL)?;
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('index_version', ?1), ('authority', ?2), ('head', ''), ('rows_dirty', '0')",
            params![INDEX_VERSION, authority],
        )?;
        Ok(Self {
            conn,
            workspace_root: workspace_root.to_owned(),
            authority: authority.to_owned(),
            refold_count: 0,
        })
    }

    /// How many committed refolds (full rematerializations of the folded
    /// rows) this handle has performed. An empty batch never refolds, so a
    /// caller that admits a whole load in one batch refolds exactly once.
    pub fn refold_count(&self) -> u64 {
        self.refold_count
    }

    /// Current cursor: stored head plus per-shard positions.
    pub fn cursor(&self) -> Result<Cursor> {
        let head: String =
            self.conn
                .query_row("SELECT value FROM meta WHERE key = 'head'", [], |row| {
                    row.get(0)
                })?;
        let mut statement = self
            .conn
            .prepare("SELECT path, offset, prefix_sha256 FROM shards ORDER BY path")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                ShardPos {
                    offset: row.get::<_, i64>(1)? as u64,
                    prefix_sha256: row.get::<_, String>(2)?,
                },
            ))
        })?;
        let mut shards = BTreeMap::new();
        for row in rows {
            let (path, pos) = row?;
            shards.insert(path, pos);
        }
        Ok(Cursor {
            authority: self.authority.clone(),
            head,
            shards,
        })
    }

    /// Store one batch inside `tx` without refolding. Identical copies of
    /// an admitted event collapse to the first copy; a conflicting body
    /// refuses and (by transaction rollback) stores nothing. Returns how
    /// many events were newly stored.
    fn append_in_tx(
        tx: &rusqlite::Transaction,
        batch: &[RawEvent],
        next_cursor: &Cursor,
    ) -> Result<usize> {
        let mut inserted = 0usize;
        for event in batch {
            let grouping = event.grouping_key();
            let identity = event.identity_hash();
            let first: Option<String> = tx
                .query_row(
                    "SELECT identity FROM events WHERE grouping_id = ?1 ORDER BY seq LIMIT 1",
                    params![grouping.as_ref()],
                    |row| row.get(0),
                )
                .ok();
            if let Some(first) = first {
                if first != identity {
                    return Err(Refusal::ConflictingEventId {
                        event_id: grouping.into_owned(),
                    }
                    .into());
                }
                continue;
            }
            let (sort_emitted, sort_id) = event.order_key();
            let event_id_str = event.get_str("event_id").filter(|s| !s.is_empty());
            let correct_candidate = i64::from(
                event.kind() == Some(EventKind::RunCorrect)
                    && event.value.contains_key("legacy_row"),
            );
            tx.execute(
                "INSERT INTO events (grouping_id, identity, line, sort_emitted, sort_id, event_id_str, correct_candidate) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    grouping.as_ref(),
                    identity,
                    event.line,
                    sort_emitted,
                    sort_id,
                    event_id_str,
                    correct_candidate
                ],
            )?;
            inserted += 1;
        }
        if inserted > 0 {
            tx.execute(
                "INSERT INTO meta (key, value) VALUES ('rows_dirty', '1') \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [],
            )?;
        }
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('head', ?1) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![next_cursor.head],
        )?;
        tx.execute("DELETE FROM shards", [])?;
        for (path, pos) in &next_cursor.shards {
            tx.execute(
                "INSERT INTO shards (path, offset, prefix_sha256) VALUES (?1, ?2, ?3)",
                params![path, pos.offset as i64, pos.prefix_sha256],
            )?;
        }
        Ok(inserted)
    }

    /// Rematerialize folded rows inside `tx` by streaming admitted events
    /// from disk in union order. Only correction rows, dedup hashes, and
    /// one event/row at a time are retained; each raw event is dropped as
    /// soon as its terminal row has been folded.
    fn refold_in_tx(tx: &rusqlite::Transaction, workspace_root: &str) -> Result<()> {
        let conn: &Connection = tx;
        // Correction candidates are few; retain their lines, nothing else.
        let candidate_lines: Vec<String> = {
            let mut statement = conn.prepare(
                "SELECT line FROM events WHERE correct_candidate = 1 \
                 ORDER BY sort_emitted, sort_id, seq",
            )?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        let mut candidates: Vec<RawEvent> = Vec::with_capacity(candidate_lines.len());
        for line in &candidate_lines {
            candidates.push(parse_line(line)?);
        }
        // Resolve every id the validation pass can ask about up front, so
        // the pass itself is pure map lookups.
        let mut wanted_known: HashSet<String> = HashSet::new();
        let mut wanted_targets: HashSet<String> = HashSet::new();
        for event in &candidates {
            let reference = event
                .get("enriches")
                .filter(|v| v.truthy())
                .or_else(|| event.get("supersedes"));
            if let Some(id) = reference.and_then(|v| v.as_str()) {
                wanted_known.insert(id.to_owned());
            }
            if let Some(id) = event.get_str("supersedes").filter(|s| !s.is_empty()) {
                wanted_known.insert(id.to_owned());
                wanted_targets.insert(id.to_owned());
            }
        }
        let mut known_map: HashMap<String, bool> = HashMap::new();
        for id in &wanted_known {
            let found: bool = conn.query_row(
                "SELECT COUNT(*) FROM events WHERE event_id_str = ?1",
                params![id],
                |row| row.get::<_, i64>(0),
            )? > 0;
            known_map.insert(id.clone(), found);
        }
        let mut target_map: HashMap<String, CorrectionTarget> = HashMap::new();
        for id in &wanted_targets {
            let line: Option<String> = conn
                .query_row(
                    "SELECT line FROM events WHERE event_id_str = ?1 ORDER BY seq DESC LIMIT 1",
                    params![id],
                    |row| row.get(0),
                )
                .ok();
            if let Some(line) = line {
                let target_event = parse_line(&line)?;
                target_map.insert(id.clone(), CorrectionTarget::from_event(&target_event));
            }
        }
        let corrections = build_correction_index(
            candidates.iter(),
            &|id| known_map.get(id).copied().unwrap_or(false),
            &|id| target_map.get(id).cloned(),
        )?;
        drop(candidates);
        drop(target_map);
        drop(known_map);
        conn.execute("DELETE FROM rows_out", [])?;
        let mut select =
            conn.prepare("SELECT line FROM events ORDER BY sort_emitted, sort_id, seq")?;
        let mut insert = conn.prepare(
            "INSERT INTO rows_out (identity, line, commit_id, run_id, host, finished_at, position) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        let mut rows = select.query([])?;
        let mut seen_hashes: HashSet<String> = HashSet::new();
        let mut position: i64 = 0;
        while let Some(row) = rows.next()? {
            let line: String = row.get(0)?;
            let event = parse_line(&line)?;
            let mut emitted: Option<(PyValue, String)> = None;
            emit_terminal_row(
                &corrections,
                &mut seen_hashes,
                &event,
                workspace_root,
                &mut |folded, text| {
                    emitted = Some((folded, text));
                },
            )?;
            // The raw event ends here; only its folded row (if any) lives on.
            drop(event);
            if let Some((folded, text)) = emitted {
                let text_field = |key: &str| {
                    folded
                        .get(key)
                        .and_then(PyValue::as_str)
                        .map(ToOwned::to_owned)
                };
                let (_, hash) = row_line_and_hash(&folded);
                insert.execute(params![
                    hash,
                    text,
                    text_field("commit"),
                    text_field("run_id"),
                    text_field("host"),
                    text_field("finished_at"),
                    position,
                ])?;
                position += 1;
            }
        }
        drop(rows);
        drop(select);
        drop(insert);
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('rows_dirty', '0') \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [],
        )?;
        Ok(())
    }

    /// Admit one batch of expanded events and advance to `next_cursor`
    /// without refolding. Tailing page by page therefore holds one page,
    /// not the whole tail; call [`Index::refold`] once afterwards. A
    /// conflicting `event_id` body refuses the whole batch and stores
    /// nothing. Returns how many events were newly stored.
    pub fn append_batch(&mut self, batch: &[RawEvent], next_cursor: &Cursor) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let inserted = Self::append_in_tx(&tx, batch, next_cursor)?;
        tx.commit()?;
        Ok(inserted)
    }

    /// Whether admitted events have landed since the last refold (for
    /// example pages stored by a load that was killed before refolding).
    pub fn needs_refold(&self) -> Result<bool> {
        let value: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'rows_dirty'",
                [],
                |row| row.get(0),
            )
            .ok();
        Ok(value.as_deref() == Some("1"))
    }

    /// Rematerialize folded rows from every admitted event, streaming from
    /// disk so late arrivals (older `emitted_at` in a new suffix) re-fold
    /// correctly at working-set memory.
    pub fn refold(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        Self::refold_in_tx(&tx, &self.workspace_root.clone())?;
        tx.commit()?;
        self.refold_count += 1;
        Ok(())
    }

    /// Admit one batch of expanded events and advance to `next_cursor`.
    ///
    /// The batch is applied in one transaction: a conflicting `event_id`
    /// body refuses the whole batch and stores nothing. Folded rows are
    /// then rematerialized from every admitted event, so late arrivals
    /// (older `emitted_at` in a new suffix) re-fold correctly. An empty
    /// batch only records the new cursor; it never rematerializes, so
    /// tailing in small pages should accumulate and apply once instead
    /// of refolding per page.
    pub fn apply(&mut self, batch: &[RawEvent], next_cursor: &Cursor) -> Result<()> {
        let tx = self.conn.transaction()?;
        Self::append_in_tx(&tx, batch, next_cursor)?;
        if batch.is_empty() {
            // Cursor-only advance: the materialized rows cannot change.
            tx.commit()?;
            return Ok(());
        }
        Self::refold_in_tx(&tx, &self.workspace_root.clone())?;
        tx.commit()?;
        self.refold_count += 1;
        Ok(())
    }

    /// Visit each folded row's canonical line in fold position order,
    /// one line at a time (never a vector of the whole history).
    pub fn for_each_row_line(&self, mut f: impl FnMut(&str) -> Result<()>) -> Result<()> {
        let mut statement = self
            .conn
            .prepare("SELECT line FROM rows_out ORDER BY position")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let line: String = row.get(0)?;
            f(&line)?;
        }
        Ok(())
    }

    /// Visit each folded row in fold position order, parsing (and
    /// dropping) one row at a time.
    pub fn for_each_row(&self, mut f: impl FnMut(PyValue) -> Result<()>) -> Result<()> {
        self.for_each_row_line(|line| {
            let row = parse(line).map_err(|e| {
                IndexError::Refused(Refusal::Malformed {
                    reason: format!("stored row: {e}"),
                })
            })?;
            f(row)
        })
    }

    /// Visit folded rows grouped by commit (commits ascending, rows in
    /// fold position order within each commit), holding one commit's
    /// rows at a time. Rows without a string commit are skipped, as in
    /// the slice grouping.
    pub fn for_each_commit_rows(
        &self,
        mut f: impl FnMut(String, Vec<PyValue>) -> Result<()>,
    ) -> Result<()> {
        let mut statement = self.conn.prepare(
            "SELECT commit_id, line FROM rows_out WHERE commit_id IS NOT NULL \
             ORDER BY commit_id, position",
        )?;
        let mut rows = statement.query([])?;
        let mut current: Option<(String, Vec<PyValue>)> = None;
        while let Some(row) = rows.next()? {
            let commit: String = row.get(0)?;
            let line: String = row.get(1)?;
            let parsed = parse(&line).map_err(|e| {
                IndexError::Refused(Refusal::Malformed {
                    reason: format!("stored row: {e}"),
                })
            })?;
            match &mut current {
                Some((key, group)) if *key == commit => group.push(parsed),
                _ => {
                    if let Some((key, group)) = current.take() {
                        f(key, group)?;
                    }
                    current = Some((commit, vec![parsed]));
                }
            }
        }
        if let Some((key, group)) = current.take() {
            f(key, group)?;
        }
        Ok(())
    }

    /// Folded rows naming `commit`, in fold position order.
    pub fn rows_for_commit(&self, commit: &str) -> Result<Vec<PyValue>> {
        let mut statement = self
            .conn
            .prepare("SELECT line FROM rows_out WHERE commit_id = ?1 ORDER BY position")?;
        let mut rows_iter = statement.query(params![commit])?;
        let mut rows = Vec::new();
        while let Some(row) = rows_iter.next()? {
            let line: String = row.get(0)?;
            rows.push(parse(&line).map_err(|e| {
                IndexError::Refused(Refusal::Malformed {
                    reason: format!("stored row: {e}"),
                })
            })?);
        }
        Ok(rows)
    }

    /// Every folded row, in fold position order.
    pub fn all_rows(&self) -> Result<Vec<PyValue>> {
        let mut rows = Vec::new();
        self.for_each_row(|row| {
            rows.push(row);
            Ok(())
        })?;
        Ok(rows)
    }

    /// Canonical dump of the index's logical content, for rebuild equality.
    pub fn dump(&self) -> Result<String> {
        let cursor = self.cursor()?;
        let mut out = String::new();
        out.push_str(&format!("index_version={INDEX_VERSION}\n"));
        out.push_str(&format!("cursor={}\n", dumps_sorted(&cursor.to_pyvalue())));
        {
            let mut statement = self
                .conn
                .prepare("SELECT grouping_id, identity, line FROM events ORDER BY seq")?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let grouping: String = row.get(0)?;
                let identity: String = row.get(1)?;
                let line: String = row.get(2)?;
                out.push_str(&format!("event {grouping} {identity} {line}\n"));
            }
        }
        self.for_each_row(|row| {
            out.push_str(&format!("row {}\n", dumps_sorted(&row)));
            Ok(())
        })?;
        Ok(out)
    }

    /// SHA-256 hex of [`Index::dump`].
    pub fn dump_digest(&self) -> Result<String> {
        Ok(crate::digest::sha256_of(self.dump()?.as_bytes()))
    }
}
