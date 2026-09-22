use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{params, Connection, OpenFlags, Transaction, TransactionBehavior};
use rustix::fs::{renameat_with, RenameFlags, CWD};

use crate::canonical::canonical_json;
use crate::replay::{canonical_payload, replay_stream, ReplayGuard, ReplaySummary, ReplayedLog};
use crate::ObserverError;

const INDEX_SCHEMA: u64 = 1;
const PRIVATE_INDEX_ATTEMPTS: u64 = 128;
static PRIVATE_INDEX_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const INDEX_TABLES: &[&str] = &[
    "active_records",
    "archive_records",
    "event_log",
    "observer_metadata",
];

/// Replay an event directory and atomically replace its disposable SQLite index.
///
/// One immediate SQLite transaction serializes rebuilders, streams validated
/// events directly into replacement tables, and commits only after semantic
/// replay finishes. A first index is built under a private sibling name and
/// atomically renamed into place with no-replace semantics only when complete.
/// Initial publication therefore requires Linux `renameat2(RENAME_NOREPLACE)`
/// support from the filesystem containing the index.
/// An existing index is replaced only by the same chain or a strict extension
/// of its recorded tip. The terminal index path is opened with no-follow
/// semantics and must have one hard link before and after opening; callers must
/// still place the disposable database in a directory they trust against
/// concurrent directory-entry replacement.
pub fn rebuild_index(events_dir: &Path, index_path: &Path) -> Result<ReplaySummary, ObserverError> {
    reject_index_in_event_directory(events_dir, index_path)?;
    let before = inspect_index_path(index_path)?;
    match before {
        Some(identity) => rebuild_database(events_dir, index_path, Some(identity)),
        None => rebuild_initial_index(events_dir, index_path, || {}, |_| {}),
    }
}

fn rebuild_initial_index(
    events_dir: &Path,
    index_path: &Path,
    before_publish: impl FnOnce(),
    after_publish: impl FnOnce(&Path),
) -> Result<ReplaySummary, ObserverError> {
    let private = PrivateIndex::create(index_path)?;
    let identity = private.identity;
    let summary = rebuild_database(events_dir, &private.path, Some(identity))?;
    before_publish();

    match renameat_with(CWD, &private.path, CWD, index_path, RenameFlags::NOREPLACE) {
        Ok(()) => {
            // RENAME_NOREPLACE moves the completed inode into place in one
            // operation: the private name is gone and the terminal name has
            // exactly one link as soon as it becomes visible.
            after_publish(&private.path);
            drop(private);
            let published = inspect_index_path(index_path)?.ok_or_else(|| {
                ObserverError::invalid(format!(
                    "derived index disappeared while publishing: {}",
                    index_path.display()
                ))
            })?;
            if published != identity {
                return Err(ObserverError::invalid(format!(
                    "derived index identity changed while publishing: {}",
                    index_path.display()
                )));
            }
            Ok(summary)
        }
        Err(error) if error == rustix::io::Errno::EXIST => {
            // Another initial rebuilder won publication. Re-enter through the
            // existing-index path so its chain and schema guards remain the
            // authority for whether this replay may replace it.
            drop(private);
            rebuild_index(events_dir, index_path)
        }
        Err(error) => Err(ObserverError::with_source(
            format!(
                "cannot publish completed derived index {} with renameat2(RENAME_NOREPLACE); the index filesystem must support it",
                index_path.display()
            ),
            error,
        )),
    }
}

#[cfg(test)]
pub(crate) fn rebuild_index_with_prepublication_hook(
    events_dir: &Path,
    index_path: &Path,
    before_publish: impl FnOnce(),
) -> Result<ReplaySummary, ObserverError> {
    reject_index_in_event_directory(events_dir, index_path)?;
    if inspect_index_path(index_path)?.is_some() {
        return Err(ObserverError::invalid(
            "prepublication hook requires an absent index",
        ));
    }
    rebuild_initial_index(events_dir, index_path, before_publish, |_| {})
}

#[cfg(test)]
pub(crate) fn rebuild_index_with_postpublication_hook(
    events_dir: &Path,
    index_path: &Path,
    after_publish: impl FnOnce(&Path),
) -> Result<ReplaySummary, ObserverError> {
    reject_index_in_event_directory(events_dir, index_path)?;
    if inspect_index_path(index_path)?.is_some() {
        return Err(ObserverError::invalid(
            "postpublication hook requires an absent index",
        ));
    }
    rebuild_initial_index(events_dir, index_path, || {}, after_publish)
}

fn rebuild_database(
    events_dir: &Path,
    index_path: &Path,
    before: Option<FileIdentity>,
) -> Result<ReplaySummary, ObserverError> {
    let mut connection = Connection::open_with_flags(
        index_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|error| {
        ObserverError::with_source(
            format!("cannot open derived index {}", index_path.display()),
            error,
        )
    })?;
    verify_opened_index(index_path, before)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| ObserverError::with_source("cannot begin index rebuild", error))?;
    let existing = existing_index(&transaction)?;
    replace_schema(&transaction)?;

    let guard = existing.as_ref().map(|summary| ReplayGuard {
        replay_count: summary.replay_count,
        tip_sha256: summary.tip_sha256.as_str(),
    });
    let replayed = {
        let mut statement = transaction
            .prepare(
                "INSERT INTO event_log (
                    sequence, machine, previous_sha256, recorded_at, kind, payload_json, sha256
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .map_err(|error| ObserverError::with_source("cannot prepare event indexing", error))?;
        replay_stream(events_dir, guard, |event| {
            statement
                .execute(params![
                    to_sql_integer(event.sequence, "event sequence")?,
                    event.machine,
                    event.previous_sha256,
                    event.recorded_at,
                    event.kind,
                    canonical_payload(event)?,
                    event.sha256,
                ])
                .map_err(|error| ObserverError::with_source("cannot index an event", error))?;
            Ok(())
        })?
    };
    if existing
        .as_ref()
        .is_some_and(|summary| summary.machine != replayed.summary.machine)
    {
        return Err(ObserverError::invalid(
            "refusing to replace an index for another machine",
        ));
    }
    materialize_state(&transaction, &replayed)?;
    write_summary(&transaction, &replayed.summary)?;
    transaction
        .commit()
        .map_err(|error| ObserverError::with_source("cannot commit index rebuild", error))?;
    Ok(replayed.summary)
}

struct PrivateIndex {
    path: PathBuf,
    identity: FileIdentity,
}

impl PrivateIndex {
    fn create(index_path: &Path) -> Result<Self, ObserverError> {
        let file_name = index_path
            .file_name()
            .ok_or_else(|| ObserverError::invalid("derived index path has no file name"))?;
        for _ in 0..PRIVATE_INDEX_ATTEMPTS {
            let sequence = PRIVATE_INDEX_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let mut private_name = OsString::from(".");
            private_name.push(file_name);
            private_name.push(format!(
                ".wrkslotsd-private-{}-{sequence}",
                std::process::id()
            ));
            let path = index_path.with_file_name(private_name);
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(file) => {
                    let metadata = file.metadata().map_err(|error| {
                        ObserverError::with_source(
                            format!("cannot inspect private index {}", path.display()),
                            error,
                        )
                    })?;
                    if !metadata.is_file() || metadata.nlink() != 1 {
                        return Err(ObserverError::invalid(format!(
                            "private index is not a singly linked regular file: {}",
                            path.display()
                        )));
                    }
                    return Ok(Self {
                        path,
                        identity: FileIdentity {
                            device: metadata.dev(),
                            inode: metadata.ino(),
                        },
                    });
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(ObserverError::with_source(
                        format!(
                            "cannot create private derived index beside {}",
                            index_path.display()
                        ),
                        error,
                    ))
                }
            }
        }
        Err(ObserverError::invalid(format!(
            "cannot allocate a private derived index beside {}",
            index_path.display()
        )))
    }
}

impl Drop for PrivateIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        for suffix in ["-journal", "-wal", "-shm"] {
            let mut sidecar = self.path.as_os_str().to_os_string();
            sidecar.push(suffix);
            let _ = fs::remove_file(PathBuf::from(sidecar));
        }
    }
}

/// Read and cross-check an existing derived index in one SQLite snapshot.
pub fn read_index(index_path: &Path) -> Result<ReplaySummary, ObserverError> {
    let before = inspect_index_path(index_path)?.ok_or_else(|| {
        ObserverError::invalid(format!(
            "derived index does not exist: {}",
            index_path.display()
        ))
    })?;
    let mut connection = Connection::open_with_flags(
        index_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|error| {
        ObserverError::with_source(
            format!(
                "cannot open derived index read-only: {}",
                index_path.display()
            ),
            error,
        )
    })?;
    verify_opened_index(index_path, Some(before))?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(|error| ObserverError::with_source("cannot begin index snapshot", error))?;
    let summary = existing_index(&transaction)?.ok_or_else(|| {
        ObserverError::invalid(format!("derived index is empty: {}", index_path.display()))
    })?;
    transaction
        .commit()
        .map_err(|error| ObserverError::with_source("cannot close index snapshot", error))?;
    Ok(summary)
}

fn existing_index(transaction: &Transaction<'_>) -> Result<Option<ReplaySummary>, ObserverError> {
    let mut statement = transaction
        .prepare(
            "SELECT name, type FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_%' AND type IN ('table', 'view')
             ORDER BY name",
        )
        .map_err(|error| ObserverError::with_source("cannot inspect index schema", error))?;
    let objects = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| ObserverError::with_source("cannot list index schema", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ObserverError::with_source("cannot read index schema", error))?;
    drop(statement);
    if objects.is_empty() {
        return Ok(None);
    }
    let expected = INDEX_TABLES.iter().copied().collect::<BTreeSet<_>>();
    let actual = objects
        .iter()
        .filter(|(_, kind)| kind == "table")
        .map(|(name, _)| name.as_str())
        .collect::<BTreeSet<_>>();
    if actual != expected || objects.iter().any(|(_, kind)| kind != "table") {
        return Err(ObserverError::invalid(
            "existing database is not a compatible observer index",
        ));
    }
    let (
        schema,
        machine,
        replay_count,
        tip_sha256,
        active_revision,
        active_count,
        archive_revision,
        archive_count,
    ) = transaction
        .query_row(
            "SELECT schema_version, machine, replay_count, tip_sha256,
             active_revision, active_count, archive_revision, archive_count
             FROM observer_metadata WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, u64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, u64>(7)?,
                ))
            },
        )
        .map_err(|error| ObserverError::with_source("cannot read index metadata", error))?;
    let summary = ReplaySummary {
        machine,
        replay_count,
        tip_sha256,
        active_revision: parse_index_u64(&active_revision, "active revision")?,
        active_count,
        archive_revision: parse_index_u64(&archive_revision, "archive revision")?,
        archive_count,
    };
    if schema != INDEX_SCHEMA {
        return Err(ObserverError::invalid(format!(
            "unsupported derived index schema {schema}"
        )));
    }
    if summary.replay_count == 0 || !is_digest(&summary.tip_sha256) {
        return Err(ObserverError::invalid("derived index metadata is invalid"));
    }
    let event_count = table_count(transaction, "event_log")?;
    let active_count = table_count(transaction, "active_records")?;
    let archive_count = table_count(transaction, "archive_records")?;
    if (event_count, active_count, archive_count)
        != (
            summary.replay_count,
            summary.active_count,
            summary.archive_count,
        )
    {
        return Err(ObserverError::invalid(
            "derived index counters do not match its materialized rows",
        ));
    }
    let indexed_tip: String = transaction
        .query_row(
            "SELECT sha256 FROM event_log WHERE sequence = ?1",
            [to_sql_integer(summary.replay_count, "replay count")?],
            |row| row.get(0),
        )
        .map_err(|error| ObserverError::with_source("cannot read indexed tip", error))?;
    if indexed_tip != summary.tip_sha256 {
        return Err(ObserverError::invalid(
            "derived index metadata does not match its event tip",
        ));
    }
    Ok(Some(summary))
}

fn replace_schema(transaction: &Transaction<'_>) -> Result<(), ObserverError> {
    // Python accepts revisions through u64::MAX, while SQLite INTEGER is
    // signed. Canonical decimal TEXT preserves the complete authority domain.
    transaction
        .execute_batch(
            "DROP TABLE IF EXISTS observer_metadata;
             CREATE TABLE observer_metadata (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 schema_version INTEGER NOT NULL,
                 machine TEXT NOT NULL,
                 replay_count INTEGER NOT NULL,
                 tip_sha256 TEXT NOT NULL,
                 active_revision TEXT NOT NULL,
                 active_count INTEGER NOT NULL,
                 archive_revision TEXT NOT NULL,
                 archive_count INTEGER NOT NULL
             );
             DROP TABLE IF EXISTS event_log;
             CREATE TABLE event_log (
                 sequence INTEGER PRIMARY KEY,
                 machine TEXT NOT NULL,
                 previous_sha256 TEXT NOT NULL,
                 recorded_at TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 payload_json TEXT NOT NULL,
                 sha256 TEXT NOT NULL UNIQUE
             );
             DROP TABLE IF EXISTS active_records;
             CREATE TABLE active_records (
                 slot TEXT PRIMARY KEY,
                 record_json TEXT NOT NULL
             );
             DROP TABLE IF EXISTS archive_records;
             CREATE TABLE archive_records (
                 archive_id TEXT PRIMARY KEY,
                 slot TEXT NOT NULL UNIQUE,
                 record_json TEXT NOT NULL
             );",
        )
        .map_err(|error| ObserverError::with_source("cannot replace index schema", error))
}

fn materialize_state(
    transaction: &Transaction<'_>,
    replayed: &ReplayedLog,
) -> Result<(), ObserverError> {
    {
        let mut statement = transaction
            .prepare("INSERT INTO active_records (slot, record_json) VALUES (?1, ?2)")
            .map_err(|error| {
                ObserverError::with_source("cannot prepare active record indexing", error)
            })?;
        for (slot, record) in &replayed.active_records {
            statement
                .execute(params![slot, canonical_json(record)?])
                .map_err(|error| {
                    ObserverError::with_source("cannot index an active record", error)
                })?;
        }
    }
    {
        let mut statement = transaction
            .prepare(
                "INSERT INTO archive_records (archive_id, slot, record_json) VALUES (?1, ?2, ?3)",
            )
            .map_err(|error| {
                ObserverError::with_source("cannot prepare archive record indexing", error)
            })?;
        for record in &replayed.archive_records {
            let object = record.as_object().ok_or_else(|| {
                ObserverError::invalid("replayed archive record is not an object")
            })?;
            let archive_id = object["archive_id"].as_str().ok_or_else(|| {
                ObserverError::invalid("replayed archive record has no archive_id")
            })?;
            let slot = object["slot"]
                .as_str()
                .ok_or_else(|| ObserverError::invalid("replayed archive record has no slot"))?;
            statement
                .execute(params![archive_id, slot, canonical_json(record)?])
                .map_err(|error| {
                    ObserverError::with_source("cannot index an archive record", error)
                })?;
        }
    }
    Ok(())
}

fn write_summary(
    transaction: &Transaction<'_>,
    summary: &ReplaySummary,
) -> Result<(), ObserverError> {
    transaction
        .execute(
            "INSERT INTO observer_metadata (
                singleton, schema_version, machine, replay_count, tip_sha256,
                active_revision, active_count, archive_revision, archive_count
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                to_sql_integer(INDEX_SCHEMA, "index schema")?,
                summary.machine,
                to_sql_integer(summary.replay_count, "replay count")?,
                summary.tip_sha256,
                summary.active_revision.to_string(),
                to_sql_integer(summary.active_count, "active count")?,
                summary.archive_revision.to_string(),
                to_sql_integer(summary.archive_count, "archive count")?,
            ],
        )
        .map_err(|error| ObserverError::with_source("cannot write index metadata", error))?;
    Ok(())
}

fn table_count(connection: &Connection, table: &str) -> Result<u64, ObserverError> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    connection
        .query_row(&sql, [], |row| row.get(0))
        .map_err(|error| ObserverError::with_source(format!("cannot count {table}"), error))
}

fn to_sql_integer(value: u64, label: &str) -> Result<i64, ObserverError> {
    i64::try_from(value)
        .map_err(|_| ObserverError::invalid(format!("{label} does not fit in SQLite INTEGER")))
}

fn parse_index_u64(value: &str, label: &str) -> Result<u64, ObserverError> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| ObserverError::invalid(format!("derived index {label} is invalid")))?;
    if parsed.to_string() != value {
        return Err(ObserverError::invalid(format!(
            "derived index {label} is not canonical"
        )));
    }
    Ok(parsed)
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

fn inspect_index_path(path: &Path) -> Result<Option<FileIdentity>, ObserverError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ObserverError::with_source(
                format!("cannot inspect derived index {}", path.display()),
                error,
            ))
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ObserverError::invalid(format!(
            "derived index is not a real regular file: {}",
            path.display()
        )));
    }
    if metadata.nlink() != 1 {
        return Err(ObserverError::invalid(format!(
            "derived index has multiple hard links: {}",
            path.display()
        )));
    }
    Ok(Some(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }))
}

fn verify_opened_index(path: &Path, before: Option<FileIdentity>) -> Result<(), ObserverError> {
    let after = inspect_index_path(path)?.ok_or_else(|| {
        ObserverError::invalid(format!(
            "derived index disappeared while opening: {}",
            path.display()
        ))
    })?;
    if before.is_some_and(|before| before.device != after.device || before.inode != after.inode) {
        return Err(ObserverError::invalid(format!(
            "derived index identity changed while opening: {}",
            path.display()
        )));
    }
    Ok(())
}

fn reject_index_in_event_directory(
    events_dir: &Path,
    index_path: &Path,
) -> Result<(), ObserverError> {
    let event_directory = events_dir.canonicalize().map_err(|error| {
        ObserverError::with_source(
            format!("cannot resolve event directory {}", events_dir.display()),
            error,
        )
    })?;
    let resolved_index = resolve_target(index_path)?;
    if resolved_index.starts_with(&event_directory) {
        return Err(ObserverError::invalid(format!(
            "derived index must not be placed in the event directory: {}",
            index_path.display()
        )));
    }
    Ok(())
}

fn resolve_target(path: &Path) -> Result<PathBuf, ObserverError> {
    if path.exists() {
        return path.canonicalize().map_err(|error| {
            ObserverError::with_source(format!("cannot resolve {}", path.display()), error)
        });
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let resolved_parent = parent.canonicalize().map_err(|error| {
        ObserverError::with_source(
            format!("cannot resolve index parent {}", parent.display()),
            error,
        )
    })?;
    let name = path
        .file_name()
        .ok_or_else(|| ObserverError::invalid("derived index path has no file name"))?;
    Ok(resolved_parent.join(name))
}
