//! The two read-only connections, and the queries they run.
//!
//! Each query here is plain SQL against one connection; the generation and epoch checks that
//! decide whether its answer may be served are in [`super::MessageCache::read`], around it.

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use rusqlite::{params, Connection, InterruptHandle, OpenFlags, OptionalExtension as _};

use super::coverage::{Head, Interval, Source};
use super::rows::{self, CachedMessage, ChanKey, Scope};
use super::schema::{apply_per_open, sqlite_code};
use super::shadow::{Query, View};
use super::{Account, CacheError, Counts, ThreadRecord};
use crate::model::MessageId;

/// Two read-only connections. A read takes whichever is free.
pub(crate) struct Readers {
    connections: [Mutex<Connection>; 2],
    interrupts: [InterruptHandle; 2],
}

impl Readers {
    pub(crate) fn open(path: &Path) -> Result<Self, CacheError> {
        let open = || -> Result<Connection, CacheError> {
            let connection = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|e| CacheError::Sqlite(sqlite_code(&e)))?;
            apply_per_open(&connection).map_err(|e| CacheError::Sqlite(sqlite_code(&e)))?;
            connection
                .pragma_update(None, "query_only", "ON")
                .map_err(|e| CacheError::Sqlite(sqlite_code(&e)))?;
            Ok(connection)
        };
        let first = open()?;
        let second = open()?;
        let interrupts = [first.get_interrupt_handle(), second.get_interrupt_handle()];
        Ok(Self {
            connections: [Mutex::new(first), Mutex::new(second)],
            interrupts,
        })
    }

    pub(crate) fn with<T>(
        &self,
        work: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        for slot in &self.connections {
            if let Ok(connection) = slot.try_lock() {
                return work(&connection);
            }
        }
        let connection = self.connections[0]
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        work(&connection)
    }

    /// Interrupt whatever statement either connection is running.
    pub(crate) fn interrupt_all(&self) {
        for handle in &self.interrupts {
            handle.interrupt();
        }
    }

    /// Run `work` on each connection in turn: the per-open assertion checks both.
    #[cfg(test)]
    pub(crate) fn each<T>(
        &self,
        mut work: impl FnMut(&Connection) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<Vec<T>> {
        self.connections
            .iter()
            .map(|slot| work(&slot.lock().unwrap_or_else(PoisonError::into_inner)))
            .collect()
    }
}

/// The interned id of a channel, if it has one.
fn chan_id(connection: &Connection, chan: &ChanKey) -> rusqlite::Result<Option<i64>> {
    connection
        .query_row(
            "SELECT chan FROM channel WHERE provider_key = ?1 AND channel_id = ?2",
            params![chan.provider_key, chan.channel_id],
            |row| row.get(0),
        )
        .optional()
}

const MESSAGE_COLUMNS: &str = "m.message_id, m.author_id, m.author, m.author_is_bot, m.reply_to,
     m.content, m.extra, m.created_ms, m.observed_ms, m.content_partial";

fn cached(channel_id: &str, row: &rusqlite::Row<'_>) -> rusqlite::Result<CachedMessage> {
    let message = rows::reconstruct(
        channel_id,
        &row.get::<_, String>(0)?,
        &row.get::<_, String>(1)?,
        &row.get::<_, String>(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        &row.get::<_, String>(6)?,
    );
    Ok(CachedMessage {
        message,
        created_ms: row.get(7)?,
        observed_ms: row.get(8)?,
        content_partial: row.get(9)?,
    })
}

pub(crate) fn newest(
    connection: &Connection,
    chan: &ChanKey,
    scope_key: &str,
    limit: usize,
) -> rusqlite::Result<Vec<CachedMessage>> {
    let Some(id) = chan_id(connection, chan)? else {
        return Ok(Vec::new());
    };
    let mut statement = connection.prepare(&format!(
        "SELECT {MESSAGE_COLUMNS} FROM message_scope s JOIN message m ON m.row_id = s.row_id
         WHERE s.chan = ?1 AND s.scope_key = ?2
         ORDER BY s.created_ms DESC, m.numeric_id DESC, m.message_id DESC LIMIT ?3"
    ))?;
    let mut rows = statement
        .query_map(
            params![id, scope_key, i64::try_from(limit).unwrap_or(i64::MAX)],
            |row| cached(&chan.channel_id, row),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.reverse();
    Ok(rows)
}

/// A scope's oldest `limit` messages created in `[from_ms, until_ms)`, oldest first.
fn span(
    connection: &Connection,
    chan: &ChanKey,
    chan_id: i64,
    scope_key: &str,
    (from_ms, until_ms): (i64, i64),
    limit: usize,
) -> rusqlite::Result<Vec<CachedMessage>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {MESSAGE_COLUMNS} FROM message_scope s JOIN message m ON m.row_id = s.row_id
         WHERE s.chan = ?1 AND s.scope_key = ?2 AND s.created_ms >= ?3 AND s.created_ms < ?4
         ORDER BY s.created_ms, m.numeric_id, m.message_id LIMIT ?5"
    ))?;
    let rows = statement
        .query_map(
            params![
                chan_id,
                scope_key,
                from_ms,
                until_ms,
                i64::try_from(limit).unwrap_or(i64::MAX)
            ],
            |row| cached(&chan.channel_id, row),
        )?
        .collect();
    rows
}

/// What a shadow evaluation reads, in ONE read transaction: every part of it is of the same
/// moment, so a batch the writer commits meanwhile is either wholly in it or not at all.
pub(crate) fn shadow_view(
    connection: &Connection,
    chan: &ChanKey,
    query: &Query,
) -> rusqlite::Result<View> {
    let snapshot = connection.unchecked_transaction()?;
    let Some(chan_id) = chan_id(&snapshot, chan)? else {
        return Ok(View::default());
    };
    let view = match query {
        Query::Newest { scope, limit } => {
            let key = scope.key();
            View {
                head: head(&snapshot, chan, &key)?,
                coverage: coverage_by_id(&snapshot, chan_id, &key)?,
                rows: newest(&snapshot, chan, &key, *limit)?,
                home: None,
            }
        }
        Query::Span {
            scope,
            from_ms,
            until_ms,
            limit,
        } => {
            let key = scope.key();
            View {
                head: head(&snapshot, chan, &key)?,
                coverage: coverage_by_id(&snapshot, chan_id, &key)?,
                rows: span(
                    &snapshot,
                    chan,
                    chan_id,
                    &key,
                    (*from_ms, until_ms.unwrap_or(i64::MAX)),
                    *limit,
                )?,
                home: None,
            }
        }
        Query::Message { id } => match message_and_home(&snapshot, chan, id)? {
            Some((row, home)) => View {
                head: None,
                coverage: coverage_by_id(&snapshot, chan_id, &home)?,
                rows: vec![row],
                home: Scope::parse(&home),
            },
            None => View::default(),
        },
    };
    snapshot.finish()?;
    Ok(view)
}

/// One message, and the `scope_key` of its home scope.
pub(crate) fn message_and_home(
    connection: &Connection,
    chan: &ChanKey,
    id: &MessageId,
) -> rusqlite::Result<Option<(CachedMessage, String)>> {
    let Some(chan_id) = chan_id(connection, chan)? else {
        return Ok(None);
    };
    connection
        .query_row(
            &format!(
                "SELECT {MESSAGE_COLUMNS}, m.home_scope, m.thread_id FROM message m
                 WHERE m.chan = ?1 AND m.message_id = ?2"
            ),
            params![chan_id, id.as_str()],
            |row| {
                let home = match row.get::<_, String>(10)?.as_str() {
                    "thread" => {
                        Scope::Thread(row.get::<_, Option<String>>(11)?.unwrap_or_default())
                    }
                    "flat" => Scope::Flat,
                    _ => Scope::Main,
                };
                Ok((cached(&chan.channel_id, row)?, home.key()))
            },
        )
        .optional()
}

pub(crate) fn tombstoned(
    connection: &Connection,
    chan: &ChanKey,
    id: &MessageId,
) -> rusqlite::Result<bool> {
    let Some(chan_id) = chan_id(connection, chan)? else {
        return Ok(false);
    };
    connection.query_row(
        "SELECT EXISTS (SELECT 1 FROM tombstone WHERE chan = ?1 AND message_id = ?2)",
        params![chan_id, id.as_str()],
        |row| row.get(0),
    )
}

pub(crate) fn coverage_by_id(
    connection: &Connection,
    chan_id: i64,
    scope_key: &str,
) -> rusqlite::Result<Vec<Interval>> {
    let mut statement = connection.prepare(
        "SELECT t0_ms, t1_ms, listed_ms, full_ms, complete, source FROM coverage
         WHERE chan = ?1 AND scope_key = ?2 ORDER BY t0_ms",
    )?;
    let intervals = statement
        .query_map(params![chan_id, scope_key], |row| {
            Ok(Interval {
                t0_ms: row.get(0)?,
                t1_ms: row.get(1)?,
                listed_ms: row.get(2)?,
                full_ms: row.get(3)?,
                complete: row.get(4)?,
                source: Source::parse(&row.get::<_, String>(5)?).unwrap_or(Source::Read),
            })
        })?
        .collect();
    intervals
}

pub(crate) fn coverage(
    connection: &Connection,
    chan: &ChanKey,
    scope_key: &str,
) -> rusqlite::Result<Vec<Interval>> {
    match chan_id(connection, chan)? {
        Some(id) => coverage_by_id(connection, id, scope_key),
        None => Ok(Vec::new()),
    }
}

pub(crate) fn head(
    connection: &Connection,
    chan: &ChanKey,
    scope_key: &str,
) -> rusqlite::Result<Option<Head>> {
    let Some(id) = chan_id(connection, chan)? else {
        return Ok(None);
    };
    connection
        .query_row(
            "SELECT head_ms, unsettled_from_ms, head_listed_ms FROM scope_head
             WHERE chan = ?1 AND scope_key = ?2",
            params![id, scope_key],
            |row| {
                Ok(Head {
                    head_ms: row.get(0)?,
                    unsettled_from_ms: row.get(1)?,
                    head_listed_ms: row.get(2)?,
                })
            },
        )
        .optional()
}

pub(crate) fn memberships(
    connection: &Connection,
    chan: &ChanKey,
    id: &MessageId,
) -> rusqlite::Result<Vec<Scope>> {
    let Some(chan_id) = chan_id(connection, chan)? else {
        return Ok(Vec::new());
    };
    let mut statement = connection.prepare(
        "SELECT s.scope_key FROM message_scope s JOIN message m ON m.row_id = s.row_id
         WHERE m.chan = ?1 AND m.message_id = ?2 ORDER BY s.scope_key",
    )?;
    let scopes = statement
        .query_map(params![chan_id, id.as_str()], |row| row.get::<_, String>(0))?
        .filter_map(|key| key.map(|k| Scope::parse(&k)).transpose())
        .collect();
    scopes
}

pub(crate) fn thread(
    connection: &Connection,
    chan: &ChanKey,
    thread_id: &str,
) -> rusqlite::Result<Option<ThreadRecord>> {
    let Some(chan_id) = chan_id(connection, chan)? else {
        return Ok(None);
    };
    connection
        .query_row(
            "SELECT root_id, reply_count, reply_count_exact, last_activity_ms FROM thread
             WHERE chan = ?1 AND thread_id = ?2",
            params![chan_id, thread_id],
            |row| {
                Ok(ThreadRecord {
                    thread_id: thread_id.to_owned(),
                    root_id: row.get::<_, Option<String>>(0)?.map(MessageId),
                    reply_count: row
                        .get::<_, Option<i64>>(1)?
                        .and_then(|n| u64::try_from(n).ok()),
                    reply_count_exact: row.get(2)?,
                    last_activity_ms: row.get(3)?,
                })
            },
        )
        .optional()
}

pub(crate) fn account(
    connection: &Connection,
    provider_key: &str,
) -> rusqlite::Result<Option<Account>> {
    connection
        .query_row(
            "SELECT account_id, workspace_id, verified_ms FROM account WHERE provider_key = ?1",
            [provider_key],
            |row| {
                Ok(Account {
                    account_id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    verified_ms: row.get(2)?,
                })
            },
        )
        .optional()
}

/// The earliest time any coverage interval of a channel still on the file's list starts at: how
/// far back the cache has proven it holds a scope's whole listing, somewhere. `None` with no
/// coverage. A channel being erased is not counted: no read can reach it any more.
pub(crate) fn oldest_covered(connection: &Connection) -> rusqlite::Result<Option<i64>> {
    connection.query_row(
        "SELECT MIN(c.t0_ms) FROM coverage c JOIN channel ch ON ch.chan = c.chan
         WHERE ch.provider_key <> ?1",
        [super::writer::ERASED_PROVIDER],
        |row| row.get(0),
    )
}

/// The milliseconds of message time the coverage of every listed channel's `main` and `flat`
/// scopes spans, summed. A channel being erased is not counted.
pub(crate) fn covered_ms(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT COALESCE(SUM(c.t1_ms - c.t0_ms), 0) FROM coverage c
         JOIN channel ch ON ch.chan = c.chan
         WHERE ch.provider_key <> ?1 AND c.scope_key IN ('main', 'flat')",
        [super::writer::ERASED_PROVIDER],
        |row| row.get(0),
    )
}

pub(crate) fn counts(connection: &Connection) -> rusqlite::Result<Counts> {
    let number = |sql: &str| -> rusqlite::Result<u64> {
        connection
            .query_row(sql, [], |row| row.get::<_, i64>(0))
            .map(|n| u64::try_from(n).unwrap_or(0))
    };
    Ok(Counts {
        messages: number("SELECT COUNT(*) FROM message")?,
        channels: number("SELECT COUNT(*) FROM channel")?,
        tombstones: number("SELECT COUNT(*) FROM tombstone")?,
        intervals: number("SELECT COUNT(*) FROM coverage")?,
    })
}
