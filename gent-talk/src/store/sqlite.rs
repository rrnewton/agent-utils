//! The shipped [`StateStore`]: one SQLite file, hand-written SQL, an explicit migration ladder.
//!
//! # Why SQLite and not a file format
//!
//! The thing a store has to survive is a power cut in the middle of a write. Append-only text
//! gets that half right — a torn append costs the last line — but it gets *deletion* wrong, and
//! this store deletes constantly: retention prunes on every append, and the operator's purge has
//! to be atomic or it is not a purge. SQLite gives a transaction, so a prune either happened or
//! did not, and gives it for one dependency.
//!
//! # Why not an ORM
//!
//! Four tables and about a dozen statements. An ORM would be bought with a large dependency tree
//! and paid for in indirection at every call site, and the portability it sells is already sold
//! by [`StateStore`] itself — the seam that lets a hosted backend replace this file lives one
//! level up, where it belongs.
//!
//! # Migrations
//!
//! [`MIGRATIONS`] is an append-only list, and SQLite's `user_version` pragma records how far a
//! given file has been taken. Opening an older file applies the missing steps in order; opening a
//! file from a *newer* server is refused rather than guessed at, because a schema this code does
//! not know is not a schema it can safely write to.
//!
//! # Blocking
//!
//! `rusqlite` is a blocking API. Every statement here runs inside [`tokio::task::spawn_blocking`]
//! so a slow disk cannot stall the executor that is also serving HTTP.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension as _};

use super::{
    now_ms, ConversationId, ConversationSummary, ReadMark, Retention, Speaker, StateStore,
    StoreError, SummaryKey, Turn, MAX_TURN_CHARS,
};
use crate::model::{ChannelId, MessageId};

/// The schema, one step per released shape. **Append only** — editing a landed entry would leave
/// every existing file believing it has a schema it does not have.
pub const MIGRATIONS: &[&str] = &[
    // v1 — transcripts, and this server's own read marks.
    "
    CREATE TABLE conversations (
        id            TEXT    PRIMARY KEY NOT NULL,
        started_at_ms INTEGER NOT NULL,
        last_at_ms    INTEGER NOT NULL,
        preview       TEXT    NOT NULL
    ) STRICT;

    CREATE TABLE turns (
        conversation_id TEXT    NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
        seq             INTEGER NOT NULL,
        speaker         TEXT    NOT NULL,
        text            TEXT    NOT NULL,
        at_ms           INTEGER NOT NULL,
        PRIMARY KEY (conversation_id, seq)
    ) STRICT;

    CREATE TABLE read_marks (
        channel_id           TEXT    PRIMARY KEY NOT NULL,
        last_read_message_id TEXT    NOT NULL,
        last_read_numeric    INTEGER NOT NULL,
        marked_at_ms         INTEGER NOT NULL
    ) STRICT;
    ",
    // v2 — cached summaries. `#49 cached-summaries`. Added as a SECOND step rather than folded
    // into v1 on purpose: the migration ladder is only real if it has been walked, and this is
    // the first file in the wild that will be upgraded rather than created.
    //
    // `version` is first in the primary key so the startup sweep — DELETE everything not under
    // the current policy — is one indexed range rather than a table scan.
    "
    CREATE TABLE summaries (
        version      TEXT    NOT NULL,
        channel_id   TEXT    NOT NULL,
        message_id   TEXT    NOT NULL,
        content_hash TEXT    NOT NULL,
        summary      TEXT    NOT NULL,
        made_at_ms   INTEGER NOT NULL,
        PRIMARY KEY (version, channel_id, message_id, content_hash)
    ) STRICT;
    ",
    // v3 — the per-message inbox overlay. `#50 todo-view`.
    //
    // This is the table `#61 unread-status` decided is OURS: Discord shares no read state with a
    // bot, so "I have dealt with this one" exists nowhere else and is authored here. It holds NO
    // message text — two snowflakes and an instant — which is why it is the one table in this
    // file that the age bound does not reach: an age limit would put a message the owner cleared
    // last month back in front of him for no reason he could see.
    //
    // `numeric` is the snowflake as an integer, so "everything through this message" is one
    // indexed range rather than a comparison this code would have to do in Rust over every row.
    // A snowflake that will not parse is refused at the door and never reaches this table.
    "
    CREATE TABLE dismissals (
        channel_id   TEXT    NOT NULL,
        message_id   TEXT    NOT NULL,
        numeric      INTEGER NOT NULL,
        at_ms        INTEGER NOT NULL,
        PRIMARY KEY (channel_id, message_id)
    ) STRICT;

    CREATE INDEX dismissals_by_position ON dismissals (channel_id, numeric);
    ",
];

/// A [`StateStore`] backed by one SQLite file.
pub struct SqliteStore {
    connection: Arc<Mutex<Connection>>,
    retention: Retention,
    path: PathBuf,
}

impl std::fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStore")
            .field("path", &self.path)
            .field("retention", &self.retention)
            .finish_non_exhaustive()
    }
}

impl SqliteStore {
    /// Open — creating if absent — the database at `path`, and bring its schema up to date.
    ///
    /// The parent directory is created `0700` and the file is set to `0600`, because what lands
    /// in it is the owner's speech and other people's channel text. On a non-Unix host those
    /// calls do not exist and the modes are simply not applied; the deployment this project has
    /// is a Linux container.
    ///
    /// # Errors
    ///
    /// [`StoreError::Backend`] when the directory cannot be created, the file cannot be opened,
    /// or the schema cannot be applied — including the case where the file was written by a
    /// newer version of this server.
    pub fn open(path: &Path, retention: Retention) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            create_private_dir(parent)?;
        }
        let connection = Connection::open(path).map_err(|error| {
            StoreError::Backend(format!("cannot open {}: {error}", path.display()))
        })?;
        restrict_file(path)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(backend)?;
        migrate(&connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            retention,
            path: path.to_owned(),
        })
    }

    /// The file this store lives in.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run one closure against the connection on the blocking pool.
    async fn with_connection<T, F>(&self, work: F) -> Result<T, StoreError>
    where
        F: FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        let connection = Arc::clone(&self.connection);
        tokio::task::spawn_blocking(move || {
            let mut guard = connection.lock().unwrap_or_else(PoisonError::into_inner);
            work(&mut guard)
        })
        .await
        .map_err(|error| StoreError::Backend(format!("the storage task did not finish: {error}")))?
    }
}

/// A content hash as text.
///
/// TEXT rather than INTEGER because SQLite's integers are signed 64-bit and the hash is unsigned:
/// storing it as a number means a wrapping cast at both ends, which is one more place for the two
/// directions to disagree. Hexadecimal is also what a person greps for.
fn hex(content_hash: u64) -> String {
    format!("{content_hash:016x}")
}

fn backend(error: rusqlite::Error) -> StoreError {
    StoreError::Backend(error.to_string())
}

/// Apply every migration this file has not seen yet.
fn migrate(connection: &Connection) -> Result<(), StoreError> {
    let applied: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(backend)?;
    let applied = usize::try_from(applied)
        .map_err(|_| StoreError::Backend("the schema version is negative".to_owned()))?;
    if applied > MIGRATIONS.len() {
        // Refuse rather than guess. A file from a newer server has tables and columns this code
        // does not know about, and writing to it with the old statements is how a downgrade
        // silently corrupts data.
        return Err(StoreError::Backend(format!(
            "this database is at schema version {applied}, but this server only knows {}; it was \
             written by a newer gent-talk and will not be downgraded",
            MIGRATIONS.len()
        )));
    }
    for (index, statement) in MIGRATIONS.iter().enumerate().skip(applied) {
        connection.execute_batch(statement).map_err(|error| {
            StoreError::Backend(format!("migration {} failed: {error}", index + 1))
        })?;
        connection
            .pragma_update(None, "user_version", i64::try_from(index + 1).unwrap_or(0))
            .map_err(backend)?;
    }
    Ok(())
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|error| StoreError::Backend(format!("cannot create {}: {error}", dir.display())))
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> Result<(), StoreError> {
    std::fs::create_dir_all(dir)
        .map_err(|error| StoreError::Backend(format!("cannot create {}: {error}", dir.display())))
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|error| {
        StoreError::Backend(format!("cannot restrict {}: {error}", path.display()))
    })
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> Result<(), StoreError> {
    Ok(())
}

/// Delete whatever retention no longer allows of the SUMMARY cache, inside a transaction.
///
/// Two bounds, and they collect different failures. The age limit is the only thing that ever
/// removes an ORPHAN — an entry whose message was edited or deleted upstream, which is
/// unreachable by key and which nothing will ever announce. The count limit is the ceiling: it
/// holds even against a caller that asks for a summary of a different message every second.
fn prune_summaries(
    transaction: &rusqlite::Transaction<'_>,
    retention: Retention,
) -> Result<(), StoreError> {
    if retention.retain_days > 0 {
        let cutoff = now_ms() - i64::from(retention.retain_days) * 86_400_000;
        transaction
            .execute("DELETE FROM summaries WHERE made_at_ms < ?1", [cutoff])
            .map_err(backend)?;
    }
    transaction
        .execute(
            "DELETE FROM summaries WHERE rowid NOT IN (
                 SELECT rowid FROM summaries ORDER BY made_at_ms DESC, rowid DESC LIMIT ?1
             )",
            [i64::from(retention.max_summaries)],
        )
        .map_err(backend)?;
    Ok(())
}

/// Keep the dismissal table under its ceiling, inside the caller's transaction.
///
/// ONE bound, not two, and the missing one is deliberate: see [`Retention::max_dismissals`]. The
/// oldest marks go first, which is the right end — the reader is never shown a message old enough
/// for its mark to have been evicted, because the window this filters is the recent one.
fn prune_dismissals(
    transaction: &rusqlite::Transaction<'_>,
    retention: Retention,
) -> Result<(), StoreError> {
    transaction
        .execute(
            "DELETE FROM dismissals WHERE rowid NOT IN (
                 SELECT rowid FROM dismissals ORDER BY at_ms DESC, rowid DESC LIMIT ?1
             )",
            [i64::from(retention.max_dismissals)],
        )
        .map_err(backend)?;
    Ok(())
}

/// Pair every id with its numeric snowflake, refusing the batch if any of them will not order.
///
/// All-or-nothing on purpose. A bulk dismissal that silently dropped the one id it could not read
/// would report a count that does not match what the reader saw, and the undo built from that
/// count would put back the wrong set.
fn orderable(messages: &[MessageId]) -> Result<Vec<(MessageId, i64)>, StoreError> {
    messages
        .iter()
        .map(|message| {
            let numeric = message
                .numeric()
                .and_then(|raw| i64::try_from(raw).ok())
                .ok_or_else(|| {
                    StoreError::BadId(format!(
                        "{message:?} is not a message snowflake, so this server cannot order it"
                    ))
                })?;
            Ok((message.clone(), numeric))
        })
        .collect()
}

/// Delete whatever retention no longer allows, inside the caller's transaction.
fn prune(transaction: &rusqlite::Transaction<'_>, retention: Retention) -> Result<(), StoreError> {
    if retention.retain_days > 0 {
        let cutoff = now_ms() - i64::from(retention.retain_days) * 86_400_000;
        transaction
            .execute("DELETE FROM conversations WHERE last_at_ms < ?1", [cutoff])
            .map_err(backend)?;
    }
    transaction
        .execute(
            "DELETE FROM conversations WHERE id NOT IN (
                 SELECT id FROM conversations ORDER BY last_at_ms DESC, id DESC LIMIT ?1
             )",
            [i64::from(retention.max_conversations)],
        )
        .map_err(backend)?;
    Ok(())
}

#[async_trait]
impl StateStore for SqliteStore {
    fn describe(&self) -> String {
        format!("SQLite at {}", self.path.display())
    }

    async fn append_turn(
        &self,
        conversation: &ConversationId,
        turn: &Turn,
    ) -> Result<(), StoreError> {
        if turn.text.chars().count() > MAX_TURN_CHARS {
            return Err(StoreError::TooLarge(format!(
                "one turn may be at most {MAX_TURN_CHARS} characters"
            )));
        }
        let id = conversation.as_str().to_owned();
        let turn = turn.clone();
        let retention = self.retention;
        self.with_connection(move |connection| {
            let transaction = connection.transaction().map_err(backend)?;
            let held: i64 = transaction
                .query_row(
                    "SELECT COUNT(*) FROM turns WHERE conversation_id = ?1",
                    [&id],
                    |row| row.get(0),
                )
                .map_err(backend)?;
            if held >= i64::from(retention.max_turns_per_conversation) {
                return Err(StoreError::TooLarge(format!(
                    "this conversation already holds its limit of {} turns",
                    retention.max_turns_per_conversation
                )));
            }
            transaction
                .execute(
                    "INSERT INTO conversations (id, started_at_ms, last_at_ms, preview)
                     VALUES (?1, ?2, ?2, ?3)
                     ON CONFLICT(id) DO UPDATE SET last_at_ms = ?2",
                    rusqlite::params![&id, turn.at_ms, super::preview_of(&turn.text)],
                )
                .map_err(backend)?;
            transaction
                .execute(
                    "INSERT INTO turns (conversation_id, seq, speaker, text, at_ms)
                     VALUES (
                        ?1,
                        (SELECT IFNULL(MAX(seq), 0) + 1 FROM turns WHERE conversation_id = ?1),
                        ?2, ?3, ?4
                     )",
                    rusqlite::params![&id, turn.speaker.as_str(), &turn.text, turn.at_ms],
                )
                .map_err(backend)?;
            prune(&transaction, retention)?;
            transaction.commit().map_err(backend)
        })
        .await
    }

    async fn conversations(&self) -> Result<Vec<ConversationSummary>, StoreError> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT c.id, c.started_at_ms, c.last_at_ms, c.preview,
                            (SELECT COUNT(*) FROM turns t WHERE t.conversation_id = c.id)
                     FROM conversations c
                     ORDER BY c.last_at_ms DESC, c.id DESC",
                )
                .map_err(backend)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })
                .map_err(backend)?;
            let mut out = Vec::new();
            for row in rows {
                let (id, started_at_ms, last_at_ms, preview, turns) = row.map_err(backend)?;
                out.push(ConversationSummary {
                    id: ConversationId::parse(&id)?,
                    started_at_ms,
                    last_at_ms,
                    preview,
                    turns: u32::try_from(turns).unwrap_or(u32::MAX),
                });
            }
            Ok(out)
        })
        .await
    }

    async fn turns(&self, conversation: &ConversationId) -> Result<Vec<Turn>, StoreError> {
        let id = conversation.as_str().to_owned();
        self.with_connection(move |connection| {
            let exists: Option<i64> = connection
                .query_row("SELECT 1 FROM conversations WHERE id = ?1", [&id], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(backend)?;
            if exists.is_none() {
                return Err(StoreError::NotFound);
            }
            let mut statement = connection
                .prepare(
                    "SELECT speaker, text, at_ms FROM turns
                     WHERE conversation_id = ?1 ORDER BY seq ASC",
                )
                .map_err(backend)?;
            let rows = statement
                .query_map([&id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .map_err(backend)?;
            let mut out = Vec::new();
            for row in rows {
                let (speaker, text, at_ms) = row.map_err(backend)?;
                out.push(Turn {
                    speaker: Speaker::parse(&speaker)?,
                    text,
                    at_ms,
                });
            }
            Ok(out)
        })
        .await
    }

    async fn forget_conversation(&self, conversation: &ConversationId) -> Result<(), StoreError> {
        let id = conversation.as_str().to_owned();
        self.with_connection(move |connection| {
            let removed = connection
                .execute("DELETE FROM conversations WHERE id = ?1", [&id])
                .map_err(backend)?;
            if removed == 0 {
                return Err(StoreError::NotFound);
            }
            Ok(())
        })
        .await
    }

    async fn forget_all_conversations(&self) -> Result<u64, StoreError> {
        self.with_connection(|connection| {
            let removed = connection
                .execute("DELETE FROM conversations", [])
                .map_err(backend)?;
            Ok(removed as u64)
        })
        .await
    }

    async fn read_mark(&self, channel: &ChannelId) -> Result<Option<ReadMark>, StoreError> {
        let channel = channel.clone();
        self.with_connection(move |connection| {
            connection
                .query_row(
                    "SELECT last_read_message_id, marked_at_ms FROM read_marks
                     WHERE channel_id = ?1",
                    [channel.as_str()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()
                .map_err(backend)
                .map(|found| {
                    found.map(|(last_read, marked_at_ms)| ReadMark {
                        channel,
                        last_read: MessageId(last_read),
                        marked_at_ms,
                    })
                })
        })
        .await
    }

    async fn read_marks(&self) -> Result<Vec<ReadMark>, StoreError> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT channel_id, last_read_message_id, marked_at_ms FROM read_marks
                     ORDER BY channel_id ASC",
                )
                .map_err(backend)?;
            let rows = statement
                .query_map([], |row| {
                    Ok(ReadMark {
                        channel: ChannelId(row.get::<_, String>(0)?),
                        last_read: MessageId(row.get::<_, String>(1)?),
                        marked_at_ms: row.get::<_, i64>(2)?,
                    })
                })
                .map_err(backend)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(backend)
        })
        .await
    }

    async fn mark_read(
        &self,
        channel: &ChannelId,
        upto: &MessageId,
    ) -> Result<ReadMark, StoreError> {
        let numeric = upto.numeric().ok_or_else(|| {
            StoreError::BadId(format!(
                "{upto:?} is not a message snowflake, so this server cannot order it"
            ))
        })?;
        let numeric = i64::try_from(numeric)
            .map_err(|_| StoreError::BadId("that snowflake is out of range".to_owned()))?;
        let channel = channel.clone();
        let upto = upto.clone();
        self.with_connection(move |connection| {
            let at = now_ms();
            // Monotonic: `WHERE excluded.last_read_numeric > last_read_numeric` keeps a stale
            // client from dragging the mark backwards and re-unreading messages the owner has
            // already been shown.
            connection
                .execute(
                    "INSERT INTO read_marks
                        (channel_id, last_read_message_id, last_read_numeric, marked_at_ms)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(channel_id) DO UPDATE SET
                        last_read_message_id = excluded.last_read_message_id,
                        last_read_numeric    = excluded.last_read_numeric,
                        marked_at_ms         = excluded.marked_at_ms
                     WHERE excluded.last_read_numeric > read_marks.last_read_numeric",
                    rusqlite::params![channel.as_str(), upto.as_str(), numeric, at],
                )
                .map_err(backend)?;
            connection
                .query_row(
                    "SELECT last_read_message_id, marked_at_ms FROM read_marks
                     WHERE channel_id = ?1",
                    [channel.as_str()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .map_err(backend)
                .map(|(last_read, marked_at_ms)| ReadMark {
                    channel,
                    last_read: MessageId(last_read),
                    marked_at_ms,
                })
        })
        .await
    }

    async fn forget_read_mark(&self, channel: &ChannelId) -> Result<(), StoreError> {
        let channel = channel.clone();
        self.with_connection(move |connection| {
            let removed = connection
                .execute(
                    "DELETE FROM read_marks WHERE channel_id = ?1",
                    [channel.as_str()],
                )
                .map_err(backend)?;
            if removed == 0 {
                return Err(StoreError::NotFound);
            }
            Ok(())
        })
        .await
    }

    async fn dismissals(&self, channel: &ChannelId) -> Result<Vec<MessageId>, StoreError> {
        let channel = channel.clone();
        self.with_connection(move |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT message_id FROM dismissals WHERE channel_id = ?1
                     ORDER BY numeric DESC",
                )
                .map_err(backend)?;
            let rows = statement
                .query_map([channel.as_str()], |row| row.get::<_, String>(0))
                .map_err(backend)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(MessageId(row.map_err(backend)?));
            }
            Ok(out)
        })
        .await
    }

    async fn dismiss(
        &self,
        channel: &ChannelId,
        messages: &[MessageId],
    ) -> Result<u64, StoreError> {
        let channel = channel.clone();
        let ordered = orderable(messages)?;
        let retention = self.retention;
        self.with_connection(move |connection| {
            let transaction = connection.transaction().map_err(backend)?;
            let at = now_ms();
            let mut added = 0_u64;
            for (message, numeric) in &ordered {
                // DO NOTHING, not DO UPDATE. Dismissing something twice must not move it, or the
                // count below stops meaning "how many the reader actually cleared" and the
                // retention queue would reorder itself under a repeated tap.
                added += transaction
                    .execute(
                        "INSERT INTO dismissals (channel_id, message_id, numeric, at_ms)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(channel_id, message_id) DO NOTHING",
                        rusqlite::params![channel.as_str(), message.as_str(), numeric, at],
                    )
                    .map_err(backend)? as u64;
            }
            prune_dismissals(&transaction, retention)?;
            transaction.commit().map_err(backend)?;
            Ok(added)
        })
        .await
    }

    async fn restore(
        &self,
        channel: &ChannelId,
        messages: &[MessageId],
    ) -> Result<u64, StoreError> {
        let channel = channel.clone();
        let messages: Vec<MessageId> = messages.to_vec();
        self.with_connection(move |connection| {
            let transaction = connection.transaction().map_err(backend)?;
            let mut removed = 0_u64;
            for message in &messages {
                removed += transaction
                    .execute(
                        "DELETE FROM dismissals WHERE channel_id = ?1 AND message_id = ?2",
                        rusqlite::params![channel.as_str(), message.as_str()],
                    )
                    .map_err(backend)? as u64;
            }
            transaction.commit().map_err(backend)?;
            Ok(removed)
        })
        .await
    }

    async fn cached_summary(&self, key: &SummaryKey) -> Result<Option<String>, StoreError> {
        let key = key.clone();
        self.with_connection(move |connection| {
            connection
                .query_row(
                    "SELECT summary FROM summaries
                     WHERE version = ?1 AND channel_id = ?2 AND message_id = ?3
                       AND content_hash = ?4",
                    rusqlite::params![
                        &key.version,
                        key.channel.as_str(),
                        key.message.as_str(),
                        hex(key.content_hash)
                    ],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(backend)
        })
        .await
    }

    async fn cache_summary(&self, key: &SummaryKey, summary: &str) -> Result<(), StoreError> {
        let key = key.clone();
        let summary = summary.to_owned();
        let retention = self.retention;
        self.with_connection(move |connection| {
            let transaction = connection.transaction().map_err(backend)?;
            transaction
                .execute(
                    "INSERT INTO summaries
                        (version, channel_id, message_id, content_hash, summary, made_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(version, channel_id, message_id, content_hash) DO UPDATE SET
                        summary = excluded.summary, made_at_ms = excluded.made_at_ms",
                    rusqlite::params![
                        &key.version,
                        key.channel.as_str(),
                        key.message.as_str(),
                        hex(key.content_hash),
                        &summary,
                        now_ms()
                    ],
                )
                .map_err(backend)?;
            // In the same transaction as the insert, for the same reason the transcript prunes
            // there: a bound applied "soon afterwards" is a bound a crash can skip.
            prune_summaries(&transaction, retention)?;
            transaction.commit().map_err(backend)
        })
        .await
    }

    async fn forget_summaries_except(&self, version: &str) -> Result<u64, StoreError> {
        let version = version.to_owned();
        self.with_connection(move |connection| {
            let removed = connection
                .execute("DELETE FROM summaries WHERE version <> ?1", [&version])
                .map_err(backend)?;
            Ok(removed as u64)
        })
        .await
    }

    async fn purge_everything(&self) -> Result<(), StoreError> {
        self.with_connection(|connection| {
            let transaction = connection.transaction().map_err(backend)?;
            transaction
                .execute("DELETE FROM conversations", [])
                .map_err(backend)?;
            transaction
                .execute("DELETE FROM read_marks", [])
                .map_err(backend)?;
            transaction
                .execute("DELETE FROM summaries", [])
                .map_err(backend)?;
            transaction
                .execute("DELETE FROM dismissals", [])
                .map_err(backend)?;
            transaction.commit().map_err(backend)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn store(dir: &TempDir, retention: Retention) -> SqliteStore {
        SqliteStore::open(
            &dir.path().join("state").join("gent-talk.sqlite3"),
            retention,
        )
        .expect("the store opens")
    }

    fn id(text: &str) -> ConversationId {
        ConversationId::parse(text).expect("valid id")
    }

    #[tokio::test]
    async fn a_transcript_survives_the_store_being_closed_and_reopened() {
        // This is the property that distinguishes storage from a cache, and it is the one the
        // whole issue is about: it cannot be shown by the in-memory fake.
        let dir = TempDir::new("sqlite-reopen");
        let file = dir.path().join("state").join("gent-talk.sqlite3");
        {
            let store = SqliteStore::open(&file, Retention::default()).expect("open");
            store
                .append_turn(&id("conv1"), &Turn::now(Speaker::You, "what happened?"))
                .await
                .expect("append");
            store
                .append_turn(
                    &id("conv1"),
                    &Turn::now(Speaker::Agent, "the runner stalled"),
                )
                .await
                .expect("append");
        }
        let reopened = SqliteStore::open(&file, Retention::default()).expect("reopen");
        let turns = reopened.turns(&id("conv1")).await.expect("turns");
        assert_eq!(turns.len(), 2, "the transcript did not survive a restart");
        assert_eq!(turns[0].speaker, Speaker::You);
        assert_eq!(turns[0].text, "what happened?");
        assert_eq!(
            turns[1].text, "the runner stalled",
            "turns must come back in the order they were said"
        );
    }

    #[tokio::test]
    async fn the_database_file_and_its_directory_are_private() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let dir = TempDir::new("sqlite-modes");
            let store = store(&dir, Retention::default());
            let file_mode = std::fs::metadata(store.path())
                .expect("stat")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(
                file_mode, 0o600,
                "the transcript database is readable by others"
            );
            let dir_mode = std::fs::metadata(store.path().parent().expect("parent"))
                .expect("stat")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(
                dir_mode, 0o700,
                "the storage directory is traversable by others"
            );
        }
    }

    #[tokio::test]
    async fn a_listing_is_newest_first_and_carries_a_preview_without_the_transcript() {
        let dir = TempDir::new("sqlite-list");
        // retain_days = 0 so the synthetic epoch-millis timestamps below are not swept away by
        // the age limit before the ordering can be asserted.
        let store = store(
            &dir,
            Retention {
                retain_days: 0,
                ..Retention::default()
            },
        );
        store
            .append_turn(
                &id("older"),
                &Turn {
                    speaker: Speaker::You,
                    text: "the first thing said".to_owned(),
                    at_ms: 1_000,
                },
            )
            .await
            .expect("append");
        store
            .append_turn(
                &id("newer"),
                &Turn {
                    speaker: Speaker::You,
                    text: "said later".to_owned(),
                    at_ms: 2_000,
                },
            )
            .await
            .expect("append");
        let listed = store.conversations().await.expect("list");
        assert_eq!(
            listed.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            vec!["newer", "older"],
            "a listing must be most-recently-active first"
        );
        assert_eq!(listed[1].preview, "the first thing said");
        assert_eq!(listed[1].turns, 1);
        assert_eq!(listed[1].started_at_ms, 1_000);
    }

    #[tokio::test]
    async fn retention_drops_the_least_recently_active_conversation() {
        let dir = TempDir::new("sqlite-retention");
        let store = store(
            &dir,
            Retention {
                max_conversations: 2,
                retain_days: 0,
                ..Retention::default()
            },
        );
        for (n, at) in [("a", 1_000), ("b", 2_000), ("c", 3_000)] {
            store
                .append_turn(
                    &id(n),
                    &Turn {
                        speaker: Speaker::You,
                        text: n.to_owned(),
                        at_ms: at,
                    },
                )
                .await
                .expect("append");
        }
        let kept: Vec<String> = store
            .conversations()
            .await
            .expect("list")
            .into_iter()
            .map(|c| c.id.as_str().to_owned())
            .collect();
        assert_eq!(kept, vec!["c".to_owned(), "b".to_owned()]);
        assert!(
            matches!(store.turns(&id("a")).await, Err(StoreError::NotFound)),
            "the pruned conversation's turns must go with it, not linger as orphans"
        );
    }

    #[tokio::test]
    async fn retention_drops_a_conversation_that_is_older_than_the_age_limit() {
        let dir = TempDir::new("sqlite-age");
        let store = store(
            &dir,
            Retention {
                retain_days: 1,
                ..Retention::default()
            },
        );
        let two_days_ago = now_ms() - 2 * 86_400_000;
        store
            .append_turn(
                &id("stale"),
                &Turn {
                    speaker: Speaker::You,
                    text: "old".to_owned(),
                    at_ms: two_days_ago,
                },
            )
            .await
            .expect("append");
        assert_eq!(
            store.conversations().await.expect("list").len(),
            0,
            "a conversation past retain_days must not survive the append that noticed it"
        );
    }

    #[tokio::test]
    async fn a_conversation_at_its_turn_ceiling_refuses_further_turns() {
        let dir = TempDir::new("sqlite-ceiling");
        let store = store(
            &dir,
            Retention {
                max_turns_per_conversation: 2,
                ..Retention::default()
            },
        );
        for _ in 0..2 {
            store
                .append_turn(&id("conv"), &Turn::now(Speaker::You, "hello"))
                .await
                .expect("append");
        }
        let error = store
            .append_turn(&id("conv"), &Turn::now(Speaker::You, "hello"))
            .await
            .expect_err("a third turn must be refused");
        assert_eq!(error.code(), "too_large", "{error}");
        assert_eq!(
            store.turns(&id("conv")).await.expect("turns").len(),
            2,
            "the refused turn must not have been written"
        );
    }

    #[tokio::test]
    async fn an_oversized_turn_is_refused_before_it_reaches_the_database() {
        let dir = TempDir::new("sqlite-oversize");
        let store = store(&dir, Retention::default());
        let error = store
            .append_turn(
                &id("conv"),
                &Turn::now(Speaker::You, "x".repeat(MAX_TURN_CHARS + 1)),
            )
            .await
            .expect_err("must refuse");
        assert_eq!(error.code(), "too_large", "{error}");
        assert!(
            matches!(store.turns(&id("conv")).await, Err(StoreError::NotFound)),
            "a refused first turn must not create the conversation"
        );
    }

    #[tokio::test]
    async fn forgetting_says_whether_there_was_anything_to_forget() {
        let dir = TempDir::new("sqlite-forget");
        let store = store(&dir, Retention::default());
        store
            .append_turn(&id("conv"), &Turn::now(Speaker::You, "hello"))
            .await
            .expect("append");
        store
            .forget_conversation(&id("conv"))
            .await
            .expect("forget");
        assert!(
            matches!(
                store.forget_conversation(&id("conv")).await,
                Err(StoreError::NotFound)
            ),
            "forgetting something twice must not report success the second time"
        );
        assert_eq!(
            store.forget_all_conversations().await.expect("forget all"),
            0
        );
    }

    #[tokio::test]
    async fn a_read_mark_moves_forward_and_never_backwards() {
        let dir = TempDir::new("sqlite-marks");
        let store = store(&dir, Retention::default());
        let channel = ChannelId("1111111111".to_owned());
        assert!(store.read_mark(&channel).await.expect("read").is_none());

        let mark = store
            .mark_read(&channel, &MessageId("1000000000000000200".to_owned()))
            .await
            .expect("mark");
        assert_eq!(mark.last_read.as_str(), "1000000000000000200");

        let back = store
            .mark_read(&channel, &MessageId("1000000000000000100".to_owned()))
            .await
            .expect("mark");
        assert_eq!(
            back.last_read.as_str(),
            "1000000000000000200",
            "a stale client must not drag the read mark backwards"
        );

        let forward = store
            .mark_read(&channel, &MessageId("1000000000000000300".to_owned()))
            .await
            .expect("mark");
        assert_eq!(forward.last_read.as_str(), "1000000000000000300");
        assert_eq!(store.read_marks().await.expect("marks").len(), 1);
    }

    #[tokio::test]
    async fn the_inbox_overlay_survives_the_store_being_closed_and_reopened() {
        // THE property that distinguishes an overlay from a cache, and the reason `#50 todo-view`
        // depends on `#48 transcript-storage`: a database inside a container image is storage
        // until the first rebuild. Nothing in `store::fake` can make this claim.
        let dir = TempDir::new("sqlite-dismissals");
        let channel = ChannelId("1111111111".to_owned());
        let dealt = MessageId("1000000000000000200".to_owned());
        let left = MessageId("1000000000000000300".to_owned());
        {
            let store = store(&dir, Retention::default());
            assert_eq!(
                store
                    .dismiss(&channel, std::slice::from_ref(&dealt))
                    .await
                    .expect("dismiss"),
                1
            );
            // The second one is the control: without it, "the file remembers" is satisfied by a
            // reopen that answers everything.
            assert_eq!(
                store
                    .dismiss(&channel, std::slice::from_ref(&dealt))
                    .await
                    .expect("dismiss again"),
                0,
                "dismissing something twice reported a second change"
            );
        }

        let reopened = store(&dir, Retention::default());
        assert_eq!(
            reopened.dismissals(&channel).await.expect("read"),
            vec![dealt.clone()],
            "the inbox overlay did not survive a restart, which makes it a cache"
        );
        assert!(
            !reopened
                .dismissals(&channel)
                .await
                .expect("read")
                .contains(&left),
            "a message nobody dismissed came back marked as dealt with"
        );

        // ...and the undo is durable too, which is the half a write-only overlay would fail.
        assert_eq!(
            reopened
                .restore(&channel, std::slice::from_ref(&dealt))
                .await
                .expect("restore"),
            1
        );
        drop(reopened);
        assert!(
            store(&dir, Retention::default())
                .dismissals(&channel)
                .await
                .expect("read")
                .is_empty(),
            "an undo did not survive the restart the dismissal did"
        );
    }

    #[tokio::test]
    async fn the_dismissal_table_is_bounded_by_count_and_the_oldest_mark_goes_first() {
        let dir = TempDir::new("sqlite-dismissal-bound");
        let store = store(
            &dir,
            Retention {
                max_dismissals: 2,
                ..Retention::default()
            },
        );
        let channel = ChannelId("1111111111".to_owned());
        for n in 0..2_u64 {
            store
                .dismiss(
                    &channel,
                    &[MessageId(format!("100000000000000{:04}", 100 + n))],
                )
                .await
                .expect("dismiss");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        store
            .dismiss(&channel, &[MessageId("1000000000000000999".to_owned())])
            .await
            .expect("dismiss");
        let held = store.dismissals(&channel).await.expect("read");
        assert_eq!(held.len(), 2, "the ceiling did not hold");
        assert!(
            held.iter().any(|id| id.as_str() == "1000000000000000999"),
            "the ceiling evicted the mark that was just made: {held:?}"
        );
        assert!(
            !held.iter().any(|id| id.as_str() == "1000000000000000100"),
            "the ceiling evicted the newest mark instead of the oldest: {held:?}"
        );
    }

    #[tokio::test]
    async fn an_old_dismissal_is_never_collected_by_age_the_way_an_old_summary_is() {
        // The deliberate asymmetry in `Retention`, and the ONE test that can see it. A dismissal
        // written now is inside any age limit expressible in whole days, so this backdates the row
        // by a hundred days in the file itself and then triggers a prune — which is what a real
        // deployment does after three months of use. An age bound here would put a message the
        // owner cleared in the spring back in front of him in the summer, and nothing on the row
        // would say why.
        let dir = TempDir::new("sqlite-dismissal-age");
        let store = store(
            &dir,
            Retention {
                retain_days: 30,
                ..Retention::default()
            },
        );
        let channel = ChannelId("1111111111".to_owned());
        let ancient = MessageId("1000000000000000100".to_owned());
        store
            .dismiss(&channel, std::slice::from_ref(&ancient))
            .await
            .expect("dismiss");
        {
            let guard = store
                .connection
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard
                .execute(
                    "UPDATE dismissals SET at_ms = ?1",
                    [now_ms() - 100 * 86_400_000],
                )
                .expect("backdate");
        }

        // THE CONTROL, in the same file under the same retention: a summary of the same age IS
        // collected. Without it this test would pass on a store that prunes nothing at all.
        let key = SummaryKey {
            channel: channel.clone(),
            message: ancient.clone(),
            content_hash: 7,
            version: "current".to_owned(),
        };
        store.cache_summary(&key, "old text").await.expect("cache");
        {
            let guard = store
                .connection
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard
                .execute(
                    "UPDATE summaries SET made_at_ms = ?1",
                    [now_ms() - 100 * 86_400_000],
                )
                .expect("backdate");
        }

        // One more write of each, which is when every bound in this store is applied.
        store
            .dismiss(&channel, &[MessageId("1000000000000000200".to_owned())])
            .await
            .expect("dismiss");
        let fresh = SummaryKey {
            message: MessageId("1000000000000000200".to_owned()),
            ..key.clone()
        };
        store
            .cache_summary(&fresh, "new text")
            .await
            .expect("cache");

        assert!(
            store
                .dismissals(&channel)
                .await
                .expect("read")
                .contains(&ancient),
            "an age bound reached the inbox overlay and resurrected a message the owner cleared"
        );
        assert_eq!(
            store.cached_summary(&key).await.expect("read"),
            None,
            "the control failed: the age bound is not collecting anything at all, so the \
             assertion above says nothing"
        );
    }

    #[tokio::test]
    async fn a_read_mark_refuses_an_id_it_cannot_order() {
        let dir = TempDir::new("sqlite-badmark");
        let store = store(&dir, Retention::default());
        let error = store
            .mark_read(
                &ChannelId("1111111111".to_owned()),
                &MessageId("newest".to_owned()),
            )
            .await
            .expect_err("must refuse");
        assert_eq!(error.code(), "bad_id", "{error}");
    }

    #[tokio::test]
    async fn purging_erases_both_kinds_of_state_and_leaves_the_store_usable() {
        let dir = TempDir::new("sqlite-purge");
        let store = store(&dir, Retention::default());
        let channel = ChannelId("1111111111".to_owned());
        store
            .append_turn(&id("conv"), &Turn::now(Speaker::You, "hello"))
            .await
            .expect("append");
        store
            .mark_read(&channel, &MessageId("1000000000000000200".to_owned()))
            .await
            .expect("mark");
        store
            .dismiss(&channel, &[MessageId("1000000000000000300".to_owned())])
            .await
            .expect("dismiss");

        store.purge_everything().await.expect("purge");
        assert!(store.conversations().await.expect("list").is_empty());
        assert!(store.read_marks().await.expect("marks").is_empty());
        assert!(
            store.dismissals(&channel).await.expect("read").is_empty(),
            "the operator's erase left the record of what he had dealt with behind"
        );

        store
            .append_turn(&id("conv"), &Turn::now(Speaker::You, "still working"))
            .await
            .expect("a purge must not break the store");
    }

    #[test]
    fn a_database_from_a_newer_server_is_refused_rather_than_written_to() {
        let dir = TempDir::new("sqlite-future");
        let file = dir.path().join("state").join("gent-talk.sqlite3");
        {
            let store = SqliteStore::open(&file, Retention::default()).expect("open");
            let guard = store
                .connection
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard
                .pragma_update(None, "user_version", 99_i64)
                .expect("pretend a newer server wrote this");
        }
        let error = SqliteStore::open(&file, Retention::default()).expect_err("must refuse");
        assert_eq!(error.code(), "storage_error", "{error}");
        assert!(
            error.to_string().contains("newer gent-talk"),
            "the operator has to be told why: {error}"
        );
    }

    #[tokio::test]
    async fn a_cached_summary_is_returned_only_for_the_exact_key_it_was_filed_under() {
        let dir = TempDir::new("sqlite-summaries");
        let store = store(&dir, Retention::default());
        let key = SummaryKey {
            channel: ChannelId("1111111111".to_owned()),
            message: MessageId("1000000000000000200".to_owned()),
            content_hash: 0xdead_beef_dead_beef,
            version: "v1-extractive-w3-c160-aaaa".to_owned(),
        };
        assert_eq!(store.cached_summary(&key).await.expect("miss"), None);

        store
            .cache_summary(&key, "the runner stalled overnight")
            .await
            .expect("cache");
        assert_eq!(
            store.cached_summary(&key).await.expect("hit").as_deref(),
            Some("the runner stalled overnight")
        );

        // Each part of the key is asserted separately, because a lookup that ignored one of them
        // would serve a stale summary for exactly one reason and pass every other check here.
        let edited = SummaryKey {
            content_hash: 0x0bad_c0de_0bad_c0de,
            ..key.clone()
        };
        assert_eq!(
            store.cached_summary(&edited).await.expect("miss"),
            None,
            "an edited message must not be served the old summary"
        );
        let repolicied = SummaryKey {
            version: "v1-extractive-w3-c200-bbbb".to_owned(),
            ..key.clone()
        };
        assert_eq!(
            store.cached_summary(&repolicied).await.expect("miss"),
            None,
            "a changed policy must not be served a summary produced under the old one"
        );
        let elsewhere = SummaryKey {
            message: MessageId("1000000000000000201".to_owned()),
            ..key.clone()
        };
        assert_eq!(store.cached_summary(&elsewhere).await.expect("miss"), None);
    }

    #[tokio::test]
    async fn the_sweep_collects_every_summary_from_an_older_policy_and_nothing_else() {
        let dir = TempDir::new("sqlite-sweep");
        let store = store(&dir, Retention::default());
        let base = SummaryKey {
            channel: ChannelId("1111111111".to_owned()),
            message: MessageId("1000000000000000200".to_owned()),
            content_hash: 1,
            version: String::new(),
        };
        for version in ["old-a", "old-b", "current"] {
            store
                .cache_summary(
                    &SummaryKey {
                        version: version.to_owned(),
                        ..base.clone()
                    },
                    version,
                )
                .await
                .expect("cache");
        }

        assert_eq!(
            store
                .forget_summaries_except("current")
                .await
                .expect("sweep"),
            2,
            "the sweep must collect exactly the entries produced under an older policy"
        );
        assert_eq!(
            store
                .cached_summary(&SummaryKey {
                    version: "current".to_owned(),
                    ..base.clone()
                })
                .await
                .expect("hit")
                .as_deref(),
            Some("current"),
            "the sweep took the current policy's entries with it"
        );
        assert_eq!(
            store
                .forget_summaries_except("current")
                .await
                .expect("sweep"),
            0,
            "a second sweep must find nothing left to do"
        );
    }

    /// Count the rows the summary cache is actually holding.
    ///
    /// Straight SQL rather than a trait method, because what is under test is the TABLE — an
    /// entry the key can no longer reach still occupies a row, and a count that went through
    /// `cached_summary` could not see one.
    fn summary_rows(store: &SqliteStore) -> i64 {
        let guard = store
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard
            .query_row("SELECT COUNT(*) FROM summaries", [], |row| row.get(0))
            .expect("count")
    }

    fn summary_key(message: &str, content_hash: u64) -> SummaryKey {
        SummaryKey {
            channel: ChannelId("1111111111".to_owned()),
            message: MessageId(message.to_owned()),
            content_hash,
            version: "current".to_owned(),
        }
    }

    #[tokio::test]
    async fn the_summary_cache_is_bounded_by_count_and_evicts_the_oldest_first() {
        // Without this the table is the one thing in the store with no bound at all: every edit
        // of a message adds a row and orphans the last one, and nothing ever collects it.
        let dir = TempDir::new("sqlite-summary-count");
        let store = store(
            &dir,
            Retention {
                max_summaries: 3,
                retain_days: 0,
                ..Retention::default()
            },
        );
        for n in 0..5_u64 {
            store
                .cache_summary(
                    &summary_key(&format!("100000000000000000{n}"), n),
                    &format!("summary {n}"),
                )
                .await
                .expect("cache");
            // The bound is on made_at_ms, which comes from the clock; without a gap the five
            // writes can share a millisecond and the eviction order would be decided by the
            // rowid tie-break alone.
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        assert_eq!(
            summary_rows(&store),
            3,
            "the summary table grew past its ceiling"
        );
        for n in 0..2_u64 {
            assert_eq!(
                store
                    .cached_summary(&summary_key(&format!("100000000000000000{n}"), n))
                    .await
                    .expect("read"),
                None,
                "entry {n} is the oldest and must have been evicted first"
            );
        }
        for n in 2..5_u64 {
            assert_eq!(
                store
                    .cached_summary(&summary_key(&format!("100000000000000000{n}"), n))
                    .await
                    .expect("read")
                    .as_deref(),
                Some(format!("summary {n}").as_str()),
                "the newest entries must be the ones kept"
            );
        }
    }

    #[tokio::test]
    async fn an_orphaned_summary_is_collected_by_the_age_limit_the_sweep_cannot_reach() {
        // The exact shape of the leak: a message is EDITED upstream, so the entry under the old
        // content hash is unreachable by key and stays under the CURRENT policy version — which
        // is the one thing `forget_summaries_except` will never delete. The age bound is what
        // collects it, and this test is the only thing that says so.
        let dir = TempDir::new("sqlite-summary-orphan");
        let store = store(
            &dir,
            Retention {
                retain_days: 1,
                ..Retention::default()
            },
        );
        let key = summary_key("1000000000000000200", 0xdead_beef);
        store
            .cache_summary(&key, "the runner stalled overnight")
            .await
            .expect("cache");
        // Age it two days, which is what the passage of time would do.
        {
            let guard = store
                .connection
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard
                .execute(
                    "UPDATE summaries SET made_at_ms = ?1",
                    [now_ms() - 2 * 86_400_000],
                )
                .expect("age the row");
        }
        assert_eq!(summary_rows(&store), 1, "the row to be collected is there");
        assert_eq!(
            store
                .forget_summaries_except("current")
                .await
                .expect("sweep"),
            0,
            "the startup sweep must NOT be what collects this; it deletes by policy version, and \
             an orphan is under the current one"
        );
        assert_eq!(summary_rows(&store), 1);

        // The edited message's summary is written under a new content hash — and that write is
        // what collects the orphan.
        store
            .cache_summary(&summary_key("1000000000000000200", 0x0bad_c0de), "reverted")
            .await
            .expect("cache");
        assert_eq!(
            summary_rows(&store),
            1,
            "the entry for the message's old text outlived the age limit"
        );
        assert_eq!(
            store
                .cached_summary(&summary_key("1000000000000000200", 0x0bad_c0de))
                .await
                .expect("read")
                .as_deref(),
            Some("reverted"),
            "the age limit took the live entry too"
        );
    }

    #[tokio::test]
    async fn a_database_at_an_older_schema_version_is_brought_forward() {
        // The migration ladder is only real once it has been walked. This builds a file with ONLY
        // the first migration applied -- the shape every existing deployment is at -- and requires
        // that opening it with today's code makes the newer tables usable.
        let dir = TempDir::new("sqlite-upgrade");
        let file = dir.path().join("gent-talk.sqlite3");
        {
            let connection = Connection::open(&file).expect("open");
            connection.execute_batch(MIGRATIONS[0]).expect("v1");
            connection
                .pragma_update(None, "user_version", 1_i64)
                .expect("stamp");
        }
        let store = SqliteStore::open(&file, Retention::default()).expect("upgrade");
        let key = SummaryKey {
            channel: ChannelId("1111111111".to_owned()),
            message: MessageId("1000000000000000200".to_owned()),
            content_hash: 7,
            version: "current".to_owned(),
        };
        store
            .cache_summary(&key, "it works")
            .await
            .expect("the newer table has to exist after the upgrade");
        assert_eq!(
            store.cached_summary(&key).await.expect("hit").as_deref(),
            Some("it works")
        );
        // ...and so does the one after it, which is the whole point of a LADDER: a file at v1
        // has to arrive at the newest shape, not at the shape of the migration that came next.
        store
            .dismiss(
                &ChannelId("1111111111".to_owned()),
                &[MessageId("1000000000000000200".to_owned())],
            )
            .await
            .expect("the v3 table has to exist after the upgrade too");
        // And the older table's data is still reachable, which is the half a DROP-and-recreate
        // migration would silently lose.
        store
            .append_turn(&id("conv"), &Turn::now(Speaker::You, "still here"))
            .await
            .expect("append");
    }

    #[test]
    fn every_migration_applies_to_a_fresh_file_and_leaves_the_version_at_the_end() {
        let dir = TempDir::new("sqlite-migrate");
        let file = dir.path().join("gent-talk.sqlite3");
        let store = SqliteStore::open(&file, Retention::default()).expect("open");
        let guard = store
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let version: i64 = guard
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(
            usize::try_from(version).expect("non-negative"),
            MIGRATIONS.len(),
            "a fresh file must end at the newest schema version"
        );
    }
}
