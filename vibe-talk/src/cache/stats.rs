//! `vibe-talk cache-stats`: what a message cache file holds and what its storage costs, read
//! without a running server. `#595 cache-full-history`.
//!
//! Opens the file READ-ONLY, through the SQLite this binary bundles (a host's own `sqlite3` may
//! be too old for the file's `STRICT` tables), and prints counts and sizes only: the channel ids
//! the file names, never a word of message text. Safe beside a running server: a reader never
//! blocks the server's writer in write-ahead-log mode.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

/// What one file holds.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FileStats {
    /// The database file, its `-wal` and its `-shm`, in bytes.
    pub file_bytes: u64,
    /// See `file_bytes`.
    pub wal_bytes: u64,
    /// See `file_bytes`.
    pub shm_bytes: u64,
    /// SQLite's page size, the pages in the file, and how many of them are free.
    pub page_size: i64,
    /// See `page_size`.
    pub pages: i64,
    /// See `page_size`.
    pub free_pages: i64,
    /// Messages held, and their text and search text in bytes.
    pub messages: u64,
    /// See `messages`.
    pub content_bytes: u64,
    /// See `messages`.
    pub search_text_bytes: u64,
    /// The bytes of the pages in use (all but the free ones), per message held: what each message
    /// costs, its indexes, search index and bookkeeping included.
    pub bytes_per_message: Option<u64>,
    /// Each table and index, by the bytes of its pages, largest first.
    pub objects: Vec<ObjectBytes>,
    /// Each channel the file names, erased ones aside.
    pub channels: Vec<ChannelFileStats>,
}

/// One table or index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ObjectBytes {
    /// Its name.
    pub name: String,
    /// The bytes of its pages.
    pub bytes: u64,
}

/// What a file holds of one channel.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChannelFileStats {
    /// `[[providers]] key` and the channel's id.
    pub provider: String,
    /// See `provider`.
    pub channel: String,
    /// Messages held, and of those, thread replies.
    pub messages: u64,
    /// See `messages`.
    pub replies: u64,
    /// Threads the file records, each costing at least one read of its own per full walk.
    pub threads: u64,
    /// The bytes of their text.
    pub content_bytes: u64,
    /// The oldest and newest held, RFC 3339.
    pub oldest: Option<String>,
    /// See `oldest`.
    pub newest: Option<String>,
    /// Days between the two, and messages a day over them.
    pub days: Option<f64>,
    /// See `days`.
    pub messages_per_day: Option<f64>,
    /// Where the full-history walk stands, when the file records one (schema 4).
    pub frontier: Option<super::Frontier>,
}

fn size(path: &Path, suffix: &str) -> u64 {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    std::fs::metadata(name).map_or(0, |metadata| metadata.len())
}

fn count(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

/// Measure the cache file at `path`, opened read-only.
///
/// # Errors
///
/// A sentence when the file cannot be opened or read as a cache file.
pub fn measure(path: &Path) -> Result<FileStats, String> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("cannot open {} read-only: {e}", path.display()))?;
    let failed = |e: rusqlite::Error| format!("cannot read the file: {e}");
    let pragma = |name: &str| -> Result<i64, String> {
        connection
            .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
            .map_err(failed)
    };
    let page_size = pragma("page_size")?;
    let pages = pragma("page_count")?;
    let free_pages = pragma("freelist_count")?;
    let (messages, content_bytes, search_text_bytes): (i64, i64, i64) = connection
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(LENGTH(content)), 0), COALESCE(SUM(LENGTH(search_text)), 0)
             FROM message",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(failed)?;
    let objects = {
        let mut statement = connection
            .prepare("SELECT name, SUM(pgsize) FROM dbstat GROUP BY name ORDER BY SUM(pgsize) DESC")
            .map_err(failed)?;
        let rows = statement
            .query_map([], |row| {
                Ok(ObjectBytes {
                    name: row.get(0)?,
                    bytes: count(row.get(1)?),
                })
            })
            .map_err(failed)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(failed)?
    };
    let has_backfill: bool = connection
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE name = 'backfill')",
            [],
            |row| row.get(0),
        )
        .map_err(failed)?;
    let channels = {
        let mut statement = connection
            .prepare(
                "SELECT ch.provider_key, ch.channel_id, COUNT(m.row_id),
                        COALESCE(SUM(m.home_scope = 'thread' AND m.is_root = 0), 0),
                        COALESCE(SUM(LENGTH(m.content)), 0), MIN(m.created_ms), MAX(m.created_ms),
                        (SELECT COUNT(*) FROM thread t WHERE t.chan = ch.chan)
                 FROM channel ch LEFT JOIN message m ON m.chan = ch.chan
                 WHERE ch.provider_key <> ?1
                 GROUP BY ch.chan ORDER BY ch.provider_key, ch.channel_id",
            )
            .map_err(failed)?;
        let rows = statement
            .query_map([super::writer::ERASED_PROVIDER], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })
            .map_err(failed)?;
        let rows = rows.collect::<rusqlite::Result<Vec<_>>>().map_err(failed)?;
        let mut channels = Vec::with_capacity(rows.len());
        for (provider, channel, messages, replies, content, oldest, newest, threads) in rows {
            #[allow(clippy::cast_precision_loss)]
            let days = oldest
                .zip(newest)
                .map(|(oldest, newest)| (newest - oldest) as f64 / 86_400_000.0)
                .filter(|days| *days > 0.0);
            let frontier = if has_backfill {
                super::reader::frontier(
                    &connection,
                    &super::ChanKey::new(provider.clone(), channel.clone()),
                )
                .map_err(failed)?
            } else {
                None
            };
            #[allow(clippy::cast_precision_loss)]
            channels.push(ChannelFileStats {
                provider,
                channel,
                messages: count(messages),
                replies: count(replies),
                threads: count(threads),
                content_bytes: count(content),
                oldest: oldest.map(crate::clock::iso_from_ms),
                newest: newest.map(crate::clock::iso_from_ms),
                days: days.map(|d| (d * 10.0).round() / 10.0),
                messages_per_day: days.map(|d| (messages as f64 / d * 10.0).round() / 10.0),
                frontier,
            });
        }
        channels
    };
    let file_bytes = size(path, "");
    let wal_bytes = size(path, "-wal");
    let messages = count(messages);
    Ok(FileStats {
        file_bytes,
        wal_bytes,
        shm_bytes: size(path, "-shm"),
        page_size,
        pages,
        free_pages,
        messages,
        content_bytes: count(content_bytes),
        search_text_bytes: count(search_text_bytes),
        bytes_per_message: (messages > 0)
            .then(|| count((pages - free_pages).max(0) * page_size) / messages),
        objects,
        channels,
    })
}
