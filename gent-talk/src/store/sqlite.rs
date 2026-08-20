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
    StoreError, Turn, MAX_TURN_CHARS,
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

    async fn purge_everything(&self) -> Result<(), StoreError> {
        self.with_connection(|connection| {
            let transaction = connection.transaction().map_err(backend)?;
            transaction
                .execute("DELETE FROM conversations", [])
                .map_err(backend)?;
            transaction
                .execute("DELETE FROM read_marks", [])
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

        store.purge_everything().await.expect("purge");
        assert!(store.conversations().await.expect("list").is_empty());
        assert!(store.read_marks().await.expect("marks").is_empty());

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
