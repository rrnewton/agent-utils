//! The single writer: one thread that owns the cache's read-write connection and applies every
//! queued operation in sequence order.
//!
//! - **Batches are time-bounded.** The writer applies queued operations in one transaction until
//!   the queue is empty, [`BATCH`] has passed, or a barrier raised the flush flag, then commits.
//!   Each operation runs inside its own savepoint, so one that fails is rolled back alone.
//! - **Hot statements are prepared once per connection**, in this thread's frame, and live as
//!   long as the connection does. Transactions are begun and ended by statement rather than by
//!   `rusqlite::Transaction`, which would need the connection exclusively.
//! - **Every merge rule is in [`crate::cache::merge`]**; this file applies them. It never uses a
//!   replacing insert: a conflicting row would be deleted without its delete trigger, leaving its
//!   old text in the full-text index for good. Every upsert is a read, a decision, and an
//!   `INSERT` or an `UPDATE`, and `search_text` is set only when it changed.
//! - **After a batch that deleted anything**, a `PASSIVE` checkpoint, and a `TRUNCATE` one when no
//!   reader is in a transaction, so deleted text leaves the write-ahead log as well as the file.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension as _, Statement};

use super::coverage::{self, Head, Interval, Params};
use super::merge::{self, Arrival, Held, Offered, Verdict};
use super::rows::{self, Extra};
use super::schema::sqlite_code;
use super::{
    file, lock, CacheError, ChanKey, Envelope, Incoming, ListEnd, ListStart, Listing, Maintenance,
    Observation, Op, Push, RouteSnapshot, Scope, Shared, Stamp, Stats, ThreadRecord, BATCH,
    DELETE_CHUNK, REVALIDATE_WITHIN_MS, TOMBSTONE_KEEP_MS,
};
use crate::model::MessageId;

/// How often the hourly pass runs.
const HOURLY: Duration = Duration::from_secs(60 * 60);
/// How often the weekly merge runs, in local milliseconds.
const WEEKLY_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Rows whose `search_text` is rebuilt per idle step.
const REBUILD_CHUNK: i64 = 500;
/// [`DELETE_CHUNK`] as SQLite binds it.
#[allow(clippy::cast_possible_wrap)]
const CHUNK_LIMIT: i64 = DELETE_CHUNK as i64;
/// A deletion of at least one row in this many takes the marker-and-compact way.
const MARKER_RATIO: usize = 75;
/// Pages of full-text index each compaction slice rewrites: tens of milliseconds of work.
const COMPACT_PAGES: u32 = 100;
/// How long one deletion chunk may run before it commits, whatever is left of its rows.
const CHUNK_BUDGET: Duration = Duration::from_millis(25);
/// A ceiling on slices, so a compaction cannot spin.
const MAX_COMPACT_SLICES: u32 = 1_000_000;
/// The writer's page cache, as SQLite spells a size in KiB: negative.
const WRITER_CACHE_KIB: i64 = -32_768;
/// The most measure-delete-compact rounds one size check runs.
const EVICTION_ROUNDS: usize = 6;

/// Why rows are being deleted in bulk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Purpose {
    /// A channel or provider erase, or a sweep.
    Erase,
    /// The age limit, or a week without revalidation.
    Expire,
    /// The size limit.
    Evict,
}

/// Open the read-write connection. The file already exists: it was created `0600` first.
pub(crate) fn open_connection(path: &Path) -> Result<Connection, CacheError> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| CacheError::Sqlite(sqlite_code(&e)))
}

/// The `search_text` rules a file was built with.
pub(crate) fn search_text_version(connection: &Connection) -> Option<String> {
    connection
        .query_row(
            "SELECT value FROM meta WHERE key = 'search_text_version'",
            [],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()
}

/// The writer thread's body.
pub(crate) fn run(
    connection: Connection,
    rx: Receiver<Envelope>,
    shared: Arc<Shared>,
    path: PathBuf,
) {
    let ack = match Hot::prepare(&connection) {
        Ok(hot) => {
            let mut writer = Writer {
                conn: &connection,
                shared: &shared,
                hot,
                path: &path,
                checkpoint_due: false,
                last_hourly: Instant::now(),
            };
            writer.start();
            writer.run(&rx)
        }
        Err(error) => {
            tracing::warn!(
                code = %sqlite_code(&error),
                "the message cache writer could not prepare its statements; nothing will be cached"
            );
            None
        }
    };
    let _ = connection.close();
    // Whatever is still queued will never be applied: give its bytes back.
    while let Ok(envelope) = rx.try_recv() {
        shared.budget.release(envelope.bytes);
        if let Op::Close { ack } = envelope.op {
            let _ = ack.send(());
        }
    }
    if let Some(ack) = ack {
        let _ = ack.send(());
    }
}

/// The statements every observation uses, prepared once per connection.
struct Hot<'c> {
    chan_select: Statement<'c>,
    chan_insert: Statement<'c>,
    row_select: Statement<'c>,
    row_insert: Statement<'c>,
    row_update: Statement<'c>,
    row_update_search: Statement<'c>,
    row_delete: Statement<'c>,
    scope_insert: Statement<'c>,
    tomb_select: Statement<'c>,
    tomb_upsert: Statement<'c>,
    tomb_delete: Statement<'c>,
    thread_upsert: Statement<'c>,
}

impl<'c> Hot<'c> {
    fn prepare(conn: &'c Connection) -> rusqlite::Result<Self> {
        Ok(Self {
            chan_select: conn
                .prepare("SELECT chan FROM channel WHERE provider_key = ?1 AND channel_id = ?2")?,
            chan_insert: conn.prepare(
                "INSERT INTO channel (provider_key, channel_id) VALUES (?1, ?2)
                 ON CONFLICT (provider_key, channel_id) DO NOTHING",
            )?,
            row_select: conn.prepare(
                "SELECT row_id, numeric_id, created_ms, edited_ms, observed_ms, home_scope,
                        thread_id, is_root, reply_to, author_id, author, author_is_bot, content,
                        content_hash, content_partial, search_text, extra
                 FROM message WHERE chan = ?1 AND message_id = ?2",
            )?,
            row_insert: conn.prepare(
                "INSERT INTO message (chan, message_id, numeric_id, created_ms, edited_ms,
                        observed_ms, home_scope, thread_id, is_root, reply_to, author_id, author,
                        author_is_bot, content, content_hash, content_partial, search_text, extra)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                         ?17, ?18)",
            )?,
            row_update: conn.prepare(
                "UPDATE message SET numeric_id = ?2, edited_ms = ?3, observed_ms = ?4,
                        thread_id = ?5, is_root = ?6, reply_to = ?7, author_id = ?8, author = ?9,
                        author_is_bot = ?10, content = ?11, content_hash = ?12,
                        content_partial = ?13, extra = ?14
                 WHERE row_id = ?1",
            )?,
            row_update_search: conn
                .prepare("UPDATE message SET search_text = ?2 WHERE row_id = ?1")?,
            row_delete: conn.prepare("DELETE FROM message WHERE row_id = ?1")?,
            scope_insert: conn.prepare(
                "INSERT INTO message_scope (chan, scope_key, created_ms, row_id)
                 VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING",
            )?,
            tomb_select: conn
                .prepare("SELECT deleted_ms FROM tombstone WHERE chan = ?1 AND message_id = ?2")?,
            tomb_upsert: conn.prepare(
                "INSERT INTO tombstone (chan, message_id, created_ms, deleted_ms)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (chan, message_id)
                 DO UPDATE SET deleted_ms = MAX(tombstone.deleted_ms, excluded.deleted_ms)",
            )?,
            tomb_delete: conn
                .prepare("DELETE FROM tombstone WHERE chan = ?1 AND message_id = ?2")?,
            thread_upsert: conn.prepare(
                "INSERT INTO thread (chan, thread_id, root_id, reply_count, reply_count_exact,
                        last_activity_ms, verified_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (chan, thread_id) DO UPDATE SET
                    root_id = COALESCE(excluded.root_id, thread.root_id),
                    reply_count = CASE
                        WHEN excluded.reply_count IS NOT NULL
                         AND excluded.verified_ms >= thread.verified_ms
                        THEN excluded.reply_count ELSE thread.reply_count END,
                    reply_count_exact = CASE
                        WHEN excluded.reply_count IS NOT NULL
                         AND excluded.verified_ms >= thread.verified_ms
                        THEN excluded.reply_count_exact ELSE thread.reply_count_exact END,
                    last_activity_ms = MAX(thread.last_activity_ms, excluded.last_activity_ms),
                    verified_ms = MAX(thread.verified_ms, excluded.verified_ms)",
            )?,
        })
    }
}

/// A stored message row, as the writer reads it to decide.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    row_id: i64,
    numeric_id: Option<i64>,
    created_ms: i64,
    edited_ms: Option<i64>,
    observed_ms: i64,
    home_scope: String,
    thread_id: Option<String>,
    is_root: bool,
    reply_to: Option<String>,
    author_id: String,
    author: String,
    author_is_bot: bool,
    content: String,
    content_hash: String,
    content_partial: bool,
    search_text: String,
    extra: String,
}

impl Row {
    fn from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            row_id: row.get(0)?,
            numeric_id: row.get(1)?,
            created_ms: row.get(2)?,
            edited_ms: row.get(3)?,
            observed_ms: row.get(4)?,
            home_scope: row.get(5)?,
            thread_id: row.get(6)?,
            is_root: row.get(7)?,
            reply_to: row.get(8)?,
            author_id: row.get(9)?,
            author: row.get(10)?,
            author_is_bot: row.get(11)?,
            content: row.get(12)?,
            content_hash: row.get(13)?,
            content_partial: row.get(14)?,
            search_text: row.get(15)?,
            extra: row.get(16)?,
        })
    }
}

/// The scope key a row's home scope stands for.
fn home_key(home_scope: &str, thread_id: Option<&str>) -> String {
    match home_scope {
        "thread" => format!("thread:{}", thread_id.unwrap_or_default()),
        other => other.to_owned(),
    }
}

fn later(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, None) => a,
        (None, b) => b,
    }
}

fn micros(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)
}

fn count(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

struct Writer<'c> {
    conn: &'c Connection,
    shared: &'c Shared,
    hot: Hot<'c>,
    path: &'c Path,
    checkpoint_due: bool,
    last_hourly: Instant,
}

impl Writer<'_> {
    fn now(&self) -> i64 {
        self.shared.clock.now_ms()
    }

    fn run(&mut self, rx: &Receiver<Envelope>) -> Option<mpsc::Sender<()>> {
        loop {
            // Checked on every turn, not only when the queue goes quiet: a writer that is never
            // idle for an hour must still expire what has aged out.
            if self.last_hourly.elapsed() >= HOURLY {
                self.hourly();
            }
            if !self.shared.search_text_current.load(Ordering::SeqCst) {
                self.rebuild_chunk();
            }
            let wait =
                if self.checkpoint_due || !self.shared.search_text_current.load(Ordering::SeqCst) {
                    Duration::from_millis(250)
                } else {
                    HOURLY.saturating_sub(self.last_hourly.elapsed())
                };
            let first = match rx.recv_timeout(wait) {
                Ok(envelope) => envelope,
                Err(RecvTimeoutError::Timeout) => {
                    if self.checkpoint_due {
                        self.checkpoint();
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return None,
            };
            if let Some(ack) = self.batch(first, rx) {
                return Some(ack);
            }
        }
    }

    /// Apply one batch, starting with `first`. Observations are applied in one transaction;
    /// anything else (an erase, a sweep, maintenance, a close) ends the batch and runs on its own
    /// after the commit, in transactions of its own, so its order relative to the observations
    /// around it is kept. Returns the close acknowledgement when the batch ended with a close.
    fn batch(&mut self, first: Envelope, rx: &Receiver<Envelope>) -> Option<mpsc::Sender<()>> {
        let started = Instant::now();
        let began = self.conn.execute_batch("BEGIN IMMEDIATE").is_ok();
        let mut last_seq = first.seq;
        let mut standalone = None;
        let mut next = Some(first);
        while let Some(envelope) = next.take() {
            last_seq = envelope.seq;
            self.shared.budget.release(envelope.bytes);
            match envelope.op {
                Op::Observe {
                    chan,
                    stamp,
                    observation,
                } if began => self.observe_op(&chan, stamp, *observation),
                Op::Observe { .. } => Stats::add(&self.shared.stats.ops_failed, 1),
                other => {
                    standalone = Some(other);
                    break;
                }
            }
            if started.elapsed() >= BATCH || self.shared.seq.flush.swap(false, Ordering::SeqCst) {
                break;
            }
            next = rx.try_recv().ok();
        }
        if began {
            if let Err(error) = self.conn.execute_batch("COMMIT") {
                tracing::warn!(code = %sqlite_code(&error), "a message cache batch did not commit");
                let _ = self.conn.execute_batch("ROLLBACK");
            }
        }
        Stats::add(&self.shared.stats.batches, 1);
        Stats::max(&self.shared.stats.batch_max_us, micros(started.elapsed()));
        let mut close = None;
        match standalone {
            Some(Op::Close { ack }) => close = Some(ack),
            Some(op) => self.standalone(op),
            None => {}
        }
        if close.is_none() {
            self.after_batch();
        }
        self.shared.seq.mark_applied(last_seq);
        close
    }

    fn observe_op(&mut self, chan: &ChanKey, stamp: Stamp, observation: Observation) {
        if !self.shared.epochs.current(chan, stamp) {
            Stats::add(&self.shared.stats.ops_stale, 1);
            return;
        }
        if !self.routed(chan) {
            Stats::add(&self.shared.stats.ops_unrouted, 1);
            return;
        }
        self.guarded(|w| w.observe(chan, observation));
        Stats::add(&self.shared.stats.ops_applied, 1);
    }

    fn standalone(&mut self, op: Op) {
        let (what, result) = match op {
            Op::Erase { chans } => ("erase", self.erase(&chans)),
            Op::EraseProvider { provider_key } => {
                ("provider erase", self.erase_provider(&provider_key))
            }
            Op::RetainOnly { routes } => ("sweep", self.retain_only(&routes)),
            Op::Maintain { kind } => {
                self.maintain(kind);
                ("maintenance", Ok(()))
            }
            #[cfg(test)]
            Op::Probe { reply } => {
                let _ = reply.send(super::probe(self.conn));
                ("probe", Ok(()))
            }
            Op::Observe { .. } | Op::Close { .. } => ("nothing", Ok(())),
        };
        if result.is_err() {
            Stats::add(&self.shared.stats.ops_failed, 1);
        }
        self.report(what, result);
        Stats::add(&self.shared.stats.ops_applied, 1);
    }

    fn routed(&self, chan: &ChanKey) -> bool {
        self.shared
            .routes
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|routes| routes.allows(chan))
    }

    /// Run one operation in a savepoint, so a failure rolls back that operation alone.
    fn guarded(&mut self, work: impl FnOnce(&mut Self) -> rusqlite::Result<()>) {
        if self.conn.execute_batch("SAVEPOINT op").is_err() {
            Stats::add(&self.shared.stats.ops_failed, 1);
            return;
        }
        match work(self) {
            Ok(()) => {
                let _ = self.conn.execute_batch("RELEASE op");
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK TO op; RELEASE op");
                Stats::add(&self.shared.stats.ops_failed, 1);
                tracing::warn!(code = %sqlite_code(&error), "a message cache write was rolled back");
            }
        }
    }

    /// Run `work` in its own transaction. For maintenance, outside a batch.
    fn transaction<T>(
        &self,
        work: impl FnOnce(&Self) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        match work(self) {
            Ok(value) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    fn find_chan(&mut self, chan: &ChanKey) -> rusqlite::Result<Option<i64>> {
        self.hot
            .chan_select
            .query_row(params![chan.provider_key, chan.channel_id], |row| {
                row.get(0)
            })
            .optional()
    }

    fn intern(&mut self, chan: &ChanKey) -> rusqlite::Result<i64> {
        if let Some(id) = self.find_chan(chan)? {
            return Ok(id);
        }
        self.hot
            .chan_insert
            .execute(params![chan.provider_key, chan.channel_id])?;
        self.find_chan(chan)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    fn observe(&mut self, chan: &ChanKey, observation: Observation) -> rusqlite::Result<()> {
        let chan_id = self.intern(chan)?;
        let cutoff = self.shared.limits.cutoff_ms(&chan.channel_id, self.now());
        match observation {
            Observation::Listing(listing) => self.listing(chan, chan_id, cutoff, listing),
            Observation::Rows {
                rows,
                scope,
                observed_ms,
            } => {
                for row in &rows {
                    self.upsert(
                        chan,
                        chan_id,
                        cutoff,
                        row,
                        observed_ms,
                        Arrival::Rows,
                        false,
                        scope.as_ref(),
                    )?;
                }
                Ok(())
            }
            Observation::Deleted { ids, observed_ms } => {
                for id in &ids {
                    self.delete_id(chan_id, id, None, observed_ms)?;
                }
                Ok(())
            }
            Observation::Push { push, observed_ms } => match push {
                Push::Upsert {
                    message,
                    update,
                    partial,
                } => {
                    let arrival = if update {
                        Arrival::PushUpdate
                    } else {
                        Arrival::PushCreate
                    };
                    self.upsert(
                        chan,
                        chan_id,
                        cutoff,
                        &message,
                        observed_ms,
                        arrival,
                        partial,
                        None,
                    )
                }
                Push::Delete { id, created_ms } => {
                    self.delete_id(chan_id, &id, created_ms, observed_ms)
                }
            },
            Observation::Threads {
                threads,
                observed_ms,
            } => {
                for thread in threads.iter().filter(|t| t.reply_count != Some(0)) {
                    self.upsert_thread(chan_id, thread, observed_ms)?;
                }
                Ok(())
            }
        }
    }

    fn listing(
        &mut self,
        chan: &ChanKey,
        chan_id: i64,
        cutoff: i64,
        listing: Listing,
    ) -> rusqlite::Result<()> {
        let mut seen: HashSet<&str> = HashSet::with_capacity(listing.rows.len());
        let mut newest: Option<i64> = None;
        for row in &listing.rows {
            let Some(created) = rows::created_ms(&row.message) else {
                continue;
            };
            seen.insert(row.message.id.as_str());
            newest = Some(newest.map_or(created, |n| n.max(created)));
            self.upsert(
                chan,
                chan_id,
                cutoff,
                row,
                listing.observed_ms,
                Arrival::Listing,
                false,
                Some(&listing.scope),
            )?;
        }
        // No proof, no claim; and an empty listing proves nothing even with one, because a
        // provider can answer a proven stretch with an empty page that is not its last.
        let (Some(proof), Some(newest)) = (listing.proof, newest) else {
            return Ok(());
        };
        let scope_key = listing.scope.key();
        let stored = self.load_head(chan_id, &scope_key)?;
        let head = match proof.until {
            ListEnd::Newest => {
                let head = coverage::advance_head(stored, newest, listing.observed_ms);
                if Some(head) != stored {
                    self.store_head(chan_id, &scope_key, &head)?;
                }
                self.clear_suspect(chan, listing.observed_ms);
                Some(head)
            }
            ListEnd::Before(_) => stored,
        };
        let from = match proof.from {
            ListStart::ScopeStart => 0,
            ListStart::At(ms) => ms,
        };
        let until = match proof.until {
            ListEnd::Newest => head
                .map_or(newest, |h| h.head_ms.max(newest))
                .saturating_add(1),
            ListEnd::Before(ms) => ms,
        };
        let settled = coverage::settled_bound(head.as_ref(), Some(newest)).unwrap_or(until);
        let end = until.min(settled);
        if from < end {
            self.absent(chan_id, &scope_key, from, end, listing.observed_ms, &seen)?;
        }
        let t0 = from.max(cutoff);
        if t0 < end {
            let claim = Interval {
                t0_ms: t0,
                t1_ms: end,
                listed_ms: listing.observed_ms,
                full_ms: proof.full.then_some(listing.observed_ms),
                complete: proof.complete,
                source: proof.source,
            };
            let existing = self.load_coverage(chan_id, &scope_key)?;
            let stamped = coverage::stamp(
                &existing,
                claim,
                head.map(|h| h.head_ms),
                &Params::default(),
            );
            if stamped != existing {
                self.store_coverage(chan_id, &scope_key, &stamped)?;
            }
        }
        Ok(())
    }

    fn clear_suspect(&self, chan: &ChanKey, observed_ms: i64) {
        let mut suspects = lock(&self.shared.suspects);
        if suspects
            .get(chan)
            .is_some_and(|&dropped_at| observed_ms > dropped_at)
        {
            suspects.remove(chan);
        }
    }

    /// Rule 4: rows of this scope in `[from, to)` that the listing did not carry.
    fn absent(
        &mut self,
        chan_id: i64,
        scope_key: &str,
        from: i64,
        to: i64,
        observed_ms: i64,
        seen: &HashSet<&str>,
    ) -> rusqlite::Result<()> {
        let mut statement = self.conn.prepare(
            "SELECT m.row_id, m.message_id, m.created_ms, m.observed_ms, m.home_scope, m.thread_id
             FROM message_scope s JOIN message m ON m.row_id = s.row_id
             WHERE s.chan = ?1 AND s.scope_key = ?2 AND s.created_ms >= ?3 AND s.created_ms < ?4",
        )?;
        let candidates = statement
            .query_map(params![chan_id, scope_key, from, to], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        for (row_id, message_id, created_ms, row_observed, home_scope, thread_id) in candidates {
            if seen.contains(message_id.as_str())
                || !merge::absent_is_deleted(row_observed, observed_ms, created_ms, to)
            {
                continue;
            }
            if home_key(&home_scope, thread_id.as_deref()) == scope_key {
                self.tombstone(chan_id, row_id, &message_id, created_ms, observed_ms)?;
            } else {
                self.conn.execute(
                    "DELETE FROM message_scope WHERE chan = ?1 AND scope_key = ?2 AND row_id = ?3",
                    params![chan_id, scope_key, row_id],
                )?;
            }
        }
        Ok(())
    }

    /// Delete a row and leave a tombstone of ids and times only.
    fn tombstone(
        &mut self,
        chan_id: i64,
        row_id: i64,
        message_id: &str,
        created_ms: i64,
        deleted_ms: i64,
    ) -> rusqlite::Result<()> {
        self.hot.row_delete.execute([row_id])?;
        self.hot
            .tomb_upsert
            .execute(params![chan_id, message_id, created_ms, deleted_ms])?;
        Stats::add(&self.shared.stats.tombstoned, 1);
        self.checkpoint_due = true;
        Ok(())
    }

    /// A deletion by id, from a push or a complete delta.
    fn delete_id(
        &mut self,
        chan_id: i64,
        id: &MessageId,
        created_hint: Option<i64>,
        observed_ms: i64,
    ) -> rusqlite::Result<()> {
        let held = self
            .hot
            .row_select
            .query_row(params![chan_id, id.as_str()], Row::from_sql)
            .optional()?;
        match held {
            // Content set by an observation that started after this deletion was seen stands.
            Some(row) if row.observed_ms > observed_ms => Ok(()),
            Some(row) => self.tombstone(
                chan_id,
                row.row_id,
                id.as_str(),
                row.created_ms,
                observed_ms,
            ),
            None => {
                let created = created_hint
                    .or_else(|| id.created_at_ms())
                    .unwrap_or(observed_ms);
                self.hot.tomb_upsert.execute(params![
                    chan_id,
                    id.as_str(),
                    created,
                    observed_ms
                ])?;
                Ok(())
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn upsert(
        &mut self,
        chan: &ChanKey,
        chan_id: i64,
        cutoff: i64,
        incoming: &Incoming,
        observed_ms: i64,
        arrival: Arrival,
        offered_partial: bool,
        listed_in: Option<&Scope>,
    ) -> rusqlite::Result<()> {
        let message = &incoming.message;
        let Some(created) = rows::created_ms(message) else {
            return Ok(());
        };
        if created < cutoff {
            Stats::add(&self.shared.stats.rows_too_old, 1);
            return Ok(());
        }
        let id = message.id.as_str();
        if let Some(deleted_ms) = self
            .hot
            .tomb_select
            .query_row(params![chan_id, id], |row| row.get::<_, i64>(0))
            .optional()?
        {
            if !merge::revives(deleted_ms, observed_ms) {
                Stats::add(&self.shared.stats.rows_held_by_tombstone, 1);
                return Ok(());
            }
            self.hot.tomb_delete.execute(params![chan_id, id])?;
        }
        let hash = rows::content_hash(&message.content);
        let offered_thread = match &incoming.home {
            Scope::Thread(thread) => Some(thread.clone()),
            _ => rows::thread_id(message),
        };
        let held = self
            .hot
            .row_select
            .query_row(params![chan_id, id], Row::from_sql)
            .optional()?;
        let (row_id, created) = match held {
            None => {
                let partial = merge::new_row_partial(arrival, offered_partial);
                let extra = serde_json::to_string(&Extra {
                    timestamp: message.timestamp.clone(),
                    thread: message.thread.clone(),
                    reactions: message.reactions.clone(),
                })
                .unwrap_or_else(|_| "{}".to_owned());
                let search_text = self.search_text(&chan.provider_key, message);
                self.hot.row_insert.execute(params![
                    chan_id,
                    id,
                    rows::numeric_id(message),
                    created,
                    incoming.edited_ms,
                    observed_ms,
                    incoming.home.kind(),
                    offered_thread,
                    message.thread.as_ref().is_some_and(|t| t.is_root),
                    message.reply_to.as_ref().map(|r| r.0.clone()),
                    message.author_id.as_str(),
                    message.author,
                    message.author_is_bot,
                    message.content,
                    hash,
                    partial,
                    search_text,
                    extra,
                ])?;
                Stats::add(&self.shared.stats.rows_written, 1);
                (self.conn.last_insert_rowid(), created)
            }
            Some(row) => {
                let (row_id, created) = (row.row_id, row.created_ms);
                self.update(
                    chan,
                    row,
                    incoming,
                    &hash,
                    offered_thread,
                    observed_ms,
                    arrival,
                    offered_partial,
                )?;
                (row_id, created)
            }
        };
        let mut memberships: Vec<String> = vec![incoming.home.key()];
        if let Some(scope) = listed_in {
            memberships.push(scope.key());
        }
        memberships.sort();
        memberships.dedup();
        for scope_key in memberships {
            self.hot
                .scope_insert
                .execute(params![chan_id, scope_key, created, row_id])?;
        }
        self.thread_from_row(chan_id, incoming, created, observed_ms)
    }

    #[allow(clippy::too_many_arguments)]
    fn update(
        &mut self,
        chan: &ChanKey,
        held: Row,
        incoming: &Incoming,
        hash: &str,
        offered_thread: Option<String>,
        observed_ms: i64,
        arrival: Arrival,
        offered_partial: bool,
    ) -> rusqlite::Result<()> {
        let message = &incoming.message;
        let verdict = merge::content_verdict(
            &Held {
                content: &held.content,
                content_hash: &held.content_hash,
                observed_ms: held.observed_ms,
                edited_ms: held.edited_ms,
                partial: held.content_partial,
            },
            &Offered {
                content: &message.content,
                content_hash: hash,
                observed_ms,
                edited_ms: incoming.edited_ms,
                arrival,
                partial: offered_partial,
            },
        );
        let mut next = held.clone();
        match verdict {
            Verdict::Keep => {}
            Verdict::Confirm => next.content_partial = false,
            Verdict::Replace { partial } => {
                next.content.clone_from(&message.content);
                hash.clone_into(&mut next.content_hash);
                next.content_partial = partial;
                next.observed_ms = observed_ms;
                next.edited_ms = later(held.edited_ms, incoming.edited_ms);
            }
        }
        // The rest of the message follows the newer observation; an older one changes nothing
        // but what only ever accumulates.
        if observed_ms >= held.observed_ms || matches!(verdict, Verdict::Replace { .. }) {
            let held_extra: Extra = serde_json::from_str(&held.extra).unwrap_or_default();
            next.author.clone_from(&message.author);
            message.author_id.0.clone_into(&mut next.author_id);
            next.author_is_bot = message.author_is_bot;
            if let Some(reply_to) = &message.reply_to {
                next.reply_to = Some(reply_to.0.clone());
            }
            next.is_root = message.thread.as_ref().map_or(held.is_root, |t| t.is_root);
            next.numeric_id = rows::numeric_id(message).or(held.numeric_id);
            next.extra = serde_json::to_string(&Extra {
                timestamp: message.timestamp.clone(),
                thread: message.thread.clone().or(held_extra.thread),
                // A copy that could not see reactions never erases ones a read brought.
                reactions: message.reactions.clone().or(held_extra.reactions),
            })
            .unwrap_or(held.extra.clone());
        }
        // Once a thread is known to have replies, it stays known.
        next.thread_id = held.thread_id.clone().or(offered_thread);
        if next.content != held.content || next.author != held.author {
            let rebuilt = rows::reconstruct(
                &chan.channel_id,
                message.id.as_str(),
                &next.author_id,
                &next.author,
                next.author_is_bot,
                next.reply_to.clone(),
                next.content.clone(),
                &next.extra,
            );
            next.search_text = self.search_text(&chan.provider_key, &rebuilt);
        }
        if next == held {
            Stats::add(&self.shared.stats.rows_unchanged, 1);
            return Ok(());
        }
        self.hot.row_update.execute(params![
            next.row_id,
            next.numeric_id,
            next.edited_ms,
            next.observed_ms,
            next.thread_id,
            next.is_root,
            next.reply_to,
            next.author_id,
            next.author,
            next.author_is_bot,
            next.content,
            next.content_hash,
            next.content_partial,
            next.extra,
        ])?;
        if next.search_text != held.search_text {
            self.hot
                .row_update_search
                .execute(params![next.row_id, next.search_text])?;
        }
        Stats::add(&self.shared.stats.rows_written, 1);
        Ok(())
    }

    fn search_text(&self, provider_key: &str, message: &crate::model::Message) -> String {
        let rendered = self.shared.search_text.rendered(provider_key, message);
        super::search_text::build(&message.author, &message.content, &rendered)
    }

    /// Thread records from a message: a root that has replies, or a reply.
    fn thread_from_row(
        &mut self,
        chan_id: i64,
        incoming: &Incoming,
        created: i64,
        observed_ms: i64,
    ) -> rusqlite::Result<()> {
        let message = &incoming.message;
        let record = match (&message.thread, &incoming.home) {
            (Some(thread), _) if thread.is_root => {
                if thread.reply_count.unwrap_or(0) == 0 {
                    return Ok(());
                }
                ThreadRecord {
                    thread_id: thread.id.clone(),
                    root_id: Some(message.id.clone()),
                    reply_count: thread.reply_count,
                    reply_count_exact: thread.reply_count_exact,
                    last_activity_ms: created,
                }
            }
            (Some(thread), _) => ThreadRecord {
                thread_id: thread.id.clone(),
                root_id: thread.root_message_id.clone(),
                reply_count: None,
                reply_count_exact: false,
                last_activity_ms: created,
            },
            (None, Scope::Thread(thread_id)) => ThreadRecord {
                thread_id: thread_id.clone(),
                root_id: None,
                reply_count: None,
                reply_count_exact: false,
                last_activity_ms: created,
            },
            (None, _) => return Ok(()),
        };
        self.upsert_thread(chan_id, &record, observed_ms)
    }

    fn upsert_thread(
        &mut self,
        chan_id: i64,
        record: &ThreadRecord,
        observed_ms: i64,
    ) -> rusqlite::Result<()> {
        self.hot.thread_upsert.execute(params![
            chan_id,
            record.thread_id,
            record.root_id.as_ref().map(|r| r.0.clone()),
            record
                .reply_count
                .map(|n| i64::try_from(n).unwrap_or(i64::MAX)),
            record.reply_count_exact,
            record.last_activity_ms,
            observed_ms,
        ])?;
        Ok(())
    }

    fn load_head(&self, chan_id: i64, scope_key: &str) -> rusqlite::Result<Option<Head>> {
        self.conn
            .query_row(
                "SELECT head_ms, unsettled_from_ms, head_listed_ms FROM scope_head
                 WHERE chan = ?1 AND scope_key = ?2",
                params![chan_id, scope_key],
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

    fn store_head(&self, chan_id: i64, scope_key: &str, head: &Head) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO scope_head (chan, scope_key, head_ms, unsettled_from_ms, head_listed_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (chan, scope_key) DO UPDATE SET head_ms = excluded.head_ms,
                unsettled_from_ms = excluded.unsettled_from_ms,
                head_listed_ms = excluded.head_listed_ms",
            params![
                chan_id,
                scope_key,
                head.head_ms,
                head.unsettled_from_ms,
                head.head_listed_ms
            ],
        )?;
        Ok(())
    }

    fn load_coverage(&self, chan_id: i64, scope_key: &str) -> rusqlite::Result<Vec<Interval>> {
        super::reader::coverage_by_id(self.conn, chan_id, scope_key)
    }

    fn store_coverage(
        &self,
        chan_id: i64,
        scope_key: &str,
        intervals: &[Interval],
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM coverage WHERE chan = ?1 AND scope_key = ?2",
            params![chan_id, scope_key],
        )?;
        let mut insert = self.conn.prepare(
            "INSERT INTO coverage (chan, scope_key, t0_ms, t1_ms, listed_ms, full_ms, complete, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for interval in intervals {
            insert.execute(params![
                chan_id,
                scope_key,
                interval.t0_ms,
                interval.t1_ms,
                interval.listed_ms,
                interval.full_ms,
                interval.complete,
                interval.source.as_str()
            ])?;
        }
        Ok(())
    }

    fn holds_anything(&self, chan_id: i64) -> rusqlite::Result<bool> {
        self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM message WHERE chan = ?1)
                 OR EXISTS (SELECT 1 FROM tombstone WHERE chan = ?1)",
            [chan_id],
            |row| row.get(0),
        )
    }

    /// Erase channels: their messages (in chunks), then the channel rows, whose cascades take
    /// tombstones, memberships, coverage, heads and thread records with them. A channel the
    /// file never held costs nothing.
    fn erase(&mut self, chans: &[ChanKey]) -> rusqlite::Result<()> {
        let mut ids = Vec::new();
        for chan in chans {
            if let Some(id) = self.find_chan(chan)? {
                ids.push(id);
            }
        }
        if ids.is_empty() {
            return Ok(());
        }
        let mut held = false;
        for &id in &ids {
            held |= self.holds_anything(id)?;
        }
        self.transaction(|w| {
            w.conn.execute("DELETE FROM temp.doomed", [])?;
            for &id in &ids {
                w.conn
                    .execute("INSERT INTO temp.doomed (chan) VALUES (?1)", [id])?;
            }
            Ok(())
        })?;
        let removed = self.delete_rows(
            "chan IN (SELECT chan FROM temp.doomed)",
            &[],
            None,
            Purpose::Erase,
        )?;
        Stats::add(&self.shared.stats.erased_rows, count(removed));
        self.transaction(|w| {
            w.conn.execute(
                "DELETE FROM channel WHERE chan IN (SELECT chan FROM temp.doomed)",
                [],
            )?;
            w.conn.execute("DELETE FROM temp.doomed", [])?;
            Ok(())
        })?;
        Stats::add(&self.shared.stats.erased_channels, count(ids.len()));
        self.checkpoint_due |= held;
        Ok(())
    }

    fn erase_provider(&mut self, provider_key: &str) -> rusqlite::Result<()> {
        let doomed: Vec<ChanKey> = self
            .channels()?
            .into_iter()
            .map(|(_, chan)| chan)
            .filter(|chan| chan.provider_key == provider_key)
            .collect();
        self.erase(&doomed)?;
        self.conn.execute(
            "DELETE FROM account WHERE provider_key = ?1",
            [provider_key],
        )?;
        Ok(())
    }

    fn retain_only(&mut self, routes: &RouteSnapshot) -> rusqlite::Result<()> {
        let doomed: Vec<ChanKey> = self
            .channels()?
            .into_iter()
            .map(|(_, chan)| chan)
            .filter(|chan| !routes.allows(chan))
            .collect();
        for chan in &doomed {
            self.shared.epochs.bump_channel(chan);
        }
        self.erase(&doomed)?;
        let accounts: Vec<String> = {
            let mut statement = self.conn.prepare("SELECT provider_key FROM account")?;
            let keys = statement
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            keys
        };
        for key in accounts.iter().filter(|key| !routes.has_provider(key)) {
            self.conn
                .execute("DELETE FROM account WHERE provider_key = ?1", [key])?;
        }
        Ok(())
    }

    fn channels(&self) -> rusqlite::Result<Vec<(i64, ChanKey)>> {
        let mut statement = self
            .conn
            .prepare("SELECT chan, provider_key, channel_id FROM channel")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    ChanKey::new(row.get::<_, String>(1)?, row.get::<_, String>(2)?),
                ))
            })?
            .collect();
        rows
    }

    /// Delete the message rows `filter` selects (a condition on `message`, binding `args` as
    /// `?1`, `?2`…), oldest first, at most `limit`, in chunks of [`DELETE_CHUNK`] rows, one
    /// transaction each. Returns how many went.
    ///
    /// The full-text index is kept honest one of two ways, chosen by size. A few rows are deleted
    /// with FTS5 `secure-delete` on, which takes each one's entries out of the index there and
    /// then, at a millisecond or two a row. Many rows are deleted with it off, which makes each
    /// delete a marker a hundred times cheaper, and the index is then compacted: rewritten
    /// without the deleted entries, the old pages freed and zeroed by `secure_delete`. A crash in
    /// between leaves `compaction_due` set, and the next open compacts before anything else.
    /// Eviction always takes the second way, because compaction is also what gives the index's
    /// space back.
    fn delete_rows(
        &mut self,
        filter: &str,
        args: &[i64],
        limit: Option<usize>,
        purpose: Purpose,
    ) -> rusqlite::Result<usize> {
        let candidates: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM message WHERE {filter}"),
            rusqlite::params_from_iter(args.iter()),
            |row| row.get(0),
        )?;
        let candidates = usize::try_from(candidates).unwrap_or(0);
        let wanted = limit.map_or(candidates, |l| l.min(candidates));
        if wanted == 0 {
            return Ok(0);
        }
        let total: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM message", [], |row| row.get(0))?;
        let total = usize::try_from(total).unwrap_or(0);
        let markers = purpose == Purpose::Evict || wanted.saturating_mul(MARKER_RATIO) >= total;
        if markers {
            self.fts_secure_delete(false)?;
        }
        // Only eviction cares which rows go first; anything else lets the planner use the index
        // its filter names, rather than walking every row in age order to find a channel's.
        let order = if purpose == Purpose::Evict {
            "ORDER BY created_ms, row_id"
        } else {
            ""
        };
        let chunk_sql = format!(
            "SELECT row_id, chan, created_ms FROM message WHERE {filter} {order} LIMIT {CHUNK_LIMIT}"
        );
        let mut deleted = 0;
        let outcome = loop {
            if deleted >= wanted {
                break Ok(());
            }
            let take = (wanted - deleted).min(DELETE_CHUNK);
            let started = Instant::now();
            let chunk = self.transaction(|w| {
                let mut select = w.conn.prepare(&chunk_sql)?;
                let picked = select
                    .query_map(rusqlite::params_from_iter(args.iter()), |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    })?
                    .take(take)
                    .collect::<rusqlite::Result<Vec<(i64, i64, i64)>>>()?;
                drop(select);
                // A chunk is bounded by time as well as by rows, so no one transaction holds
                // the writer for long, however costly each row's index entries are to remove.
                let mut delete = w.conn.prepare("DELETE FROM message WHERE row_id = ?1")?;
                let mut floors: std::collections::HashMap<i64, i64> =
                    std::collections::HashMap::new();
                let mut done = 0;
                for (row_id, chan, created) in &picked {
                    delete.execute([row_id])?;
                    let floor = floors.entry(*chan).or_insert(*created);
                    *floor = (*floor).max(*created);
                    done += 1;
                    if started.elapsed() >= CHUNK_BUDGET {
                        break;
                    }
                }
                drop(delete);
                if purpose == Purpose::Evict {
                    for (chan, newest) in floors {
                        w.raise_floor(chan, newest.saturating_add(1))?;
                    }
                }
                Ok(done)
            });
            if purpose == Purpose::Evict {
                Stats::add(&self.shared.stats.eviction_chunks, 1);
                Stats::max(
                    &self.shared.stats.eviction_chunk_max_us,
                    micros(started.elapsed()),
                );
            }
            match chunk {
                Ok(0) => break Ok(()),
                Ok(n) => deleted += n,
                Err(error) => break Err(error),
            }
        };
        if markers {
            let restored = self.fts_secure_delete(true);
            self.compact();
            restored?;
        }
        self.checkpoint_due = true;
        outcome.map(|()| deleted)
    }

    /// Turn FTS5 `secure-delete` on or off. Off marks the file as needing compaction until
    /// [`Writer::compact`] has run.
    fn fts_secure_delete(&self, on: bool) -> rusqlite::Result<()> {
        self.transaction(|w| {
            w.conn.execute(
                "INSERT INTO message_fts (message_fts, rank) VALUES ('secure-delete', ?1)",
                [i64::from(on)],
            )?;
            if !on {
                w.conn.execute(
                    "INSERT INTO meta (key, value) VALUES ('compaction_due', '1')
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                    [],
                )?;
            }
            Ok(())
        })
    }

    /// Rewrite the full-text index without the entries deleted by marker, in slices of
    /// [`COMPACT_PAGES`] pages, one transaction each, until a slice finds nothing to do. Then
    /// give the freed pages back and checkpoint.
    fn compact(&mut self) {
        let started = Instant::now();
        let mut slices = 0_u32;
        loop {
            let before = self.conn.total_changes();
            let slice_started = Instant::now();
            let merged = self.transaction(|w| {
                w.conn.execute(
                    &format!(
                        "INSERT INTO message_fts (message_fts, rank) VALUES ('merge', -{COMPACT_PAGES})"
                    ),
                    [],
                )?;
                Ok(())
            });
            Stats::max(
                &self.shared.stats.compaction_slice_max_us,
                micros(slice_started.elapsed()),
            );
            slices += 1;
            if merged.is_err() {
                self.report("compaction", merged);
                return;
            }
            // The command itself counts as one change; any more means a slice did work.
            if self.conn.total_changes().saturating_sub(before) <= 1 || slices >= MAX_COMPACT_SLICES
            {
                break;
            }
        }
        let cleared = self.transaction(|w| {
            w.conn
                .execute("DELETE FROM meta WHERE key = 'compaction_due'", [])?;
            Ok(())
        });
        self.report("compaction", cleared);
        Stats::add(&self.shared.stats.compactions, 1);
        Stats::max(
            &self.shared.stats.compaction_max_us,
            micros(started.elapsed()),
        );
        let vacuum = self.drain("PRAGMA incremental_vacuum");
        self.report("incremental vacuum", vacuum);
        self.checkpoint_due = true;
        self.checkpoint();
    }

    /// What every new writer does first: make sure `secure-delete` is on whatever a crash left,
    /// make its scratch tables, and finish a compaction a crash interrupted.
    fn start(&mut self) {
        // The writer alone gets a larger page cache: it walks the index's segments on every
        // insert, merge and delete, and re-reading them from the file was a tenth of its time.
        let cache = self
            .conn
            .pragma_update(None, "cache_size", WRITER_CACHE_KIB.to_string());
        self.report("page cache", cache);
        let prepared = self.conn.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS doomed (chan INTEGER PRIMARY KEY);
             CREATE TEMP TABLE IF NOT EXISTS cutoff (chan INTEGER PRIMARY KEY, ms INTEGER NOT NULL);",
        );
        self.report("scratch tables", prepared);
        let secure = self.fts_secure_delete(true);
        self.report("secure delete", secure);
        let due = self
            .conn
            .query_row(
                "SELECT 1 FROM meta WHERE key = 'compaction_due'",
                [],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some();
        if due {
            self.compact();
        }
    }

    fn after_batch(&mut self) {
        if self.checkpoint_due {
            self.checkpoint();
        }
        self.enforce_bytes();
    }

    fn maintain(&mut self, kind: Maintenance) {
        match kind {
            Maintenance::Hourly => self.hourly(),
            Maintenance::Weekly => self.weekly(),
            Maintenance::RebuildSearchText => {
                self.shared
                    .search_text_current
                    .store(false, Ordering::SeqCst);
                while !self.shared.search_text_current.load(Ordering::SeqCst) {
                    if !self.rebuild_chunk() {
                        break;
                    }
                }
            }
            Maintenance::Bounds => self.enforce_bytes(),
        }
    }

    fn report(&self, what: &str, result: rusqlite::Result<()>) {
        if let Err(error) = result {
            tracing::warn!(code = %sqlite_code(&error), step = what, "message cache maintenance failed");
        }
    }

    /// The hourly pass.
    fn hourly(&mut self) {
        self.last_hourly = Instant::now();
        let now = self.now();
        let aged = self.expire_by_age(now);
        self.report("age limit", aged);
        let unrevalidated = self.expire_unrevalidated(now);
        self.report("revalidation", unrevalidated);
        let tombstones = self.transaction(|w| {
            w.conn.execute(
                "DELETE FROM tombstone WHERE deleted_ms < ?1",
                [now.saturating_sub(TOMBSTONE_KEEP_MS)],
            )?;
            Ok(())
        });
        self.report("tombstones", tombstones);
        self.integrity();
        let vacuum = self.drain("PRAGMA incremental_vacuum(256)");
        self.report("incremental vacuum", vacuum);
        self.checkpoint();
        let last_merge: i64 = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'last_merge_ms'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if now.saturating_sub(last_merge) >= WEEKLY_MS {
            self.weekly();
        }
    }

    /// The weekly bounded merge of the full-text index.
    fn weekly(&mut self) {
        let now = self.now();
        let merged = self.transaction(|w| {
            w.conn.execute(
                "INSERT INTO message_fts (message_fts, rank) VALUES ('merge', 500)",
                [],
            )?;
            w.conn.execute(
                "INSERT INTO meta (key, value) VALUES ('last_merge_ms', ?1)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                [now.to_string()],
            )?;
            Ok(())
        });
        self.report("merge", merged);
    }

    /// The FTS integrity check against the content table; on failure, a rebuild from it, which
    /// needs nothing from upstream.
    fn integrity(&mut self) {
        let checked = self.conn.execute(
            "INSERT INTO message_fts (message_fts, rank) VALUES ('integrity-check', 1)",
            [],
        );
        if let Err(error) = checked {
            Stats::add(&self.shared.stats.integrity_failures, 1);
            tracing::warn!(
                code = %sqlite_code(&error),
                "the message cache's full-text index failed its integrity check; rebuilding it"
            );
            let rebuilt = self.transaction(|w| {
                w.conn.execute(
                    "INSERT INTO message_fts (message_fts) VALUES ('rebuild')",
                    [],
                )?;
                Ok(())
            });
            self.report("index rebuild", rebuilt);
        }
    }

    /// Run a statement that may answer rows, and discard them.
    fn drain(&self, sql: &str) -> rusqlite::Result<()> {
        let mut statement = self.conn.prepare(sql)?;
        let mut rows = statement.query([])?;
        while rows.next()?.is_some() {}
        Ok(())
    }

    /// Rows older than each channel's age limit go, and coverage below it with them.
    fn expire_by_age(&mut self, now: i64) -> rusqlite::Result<()> {
        let channels = self.channels()?;
        let cutoffs: Vec<(i64, i64)> = channels
            .iter()
            .map(|(id, chan)| (*id, self.shared.limits.cutoff_ms(&chan.channel_id, now)))
            .collect();
        self.transaction(|w| {
            w.conn.execute("DELETE FROM temp.cutoff", [])?;
            for (id, cutoff) in &cutoffs {
                w.conn.execute(
                    "INSERT INTO temp.cutoff (chan, ms) VALUES (?1, ?2)",
                    [id, cutoff],
                )?;
            }
            Ok(())
        })?;
        let removed = self.delete_rows(
            "created_ms < (SELECT ms FROM temp.cutoff c WHERE c.chan = message.chan)",
            &[],
            None,
            Purpose::Expire,
        )?;
        Stats::add(&self.shared.stats.expired_rows, count(removed));
        self.transaction(|w| {
            for (id, cutoff) in &cutoffs {
                w.raise_floor(*id, *cutoff)?;
            }
            w.conn.execute("DELETE FROM temp.cutoff", [])?;
            Ok(())
        })
    }

    /// Revalidate or expire: a row neither observed nor covered by an edit-and-delete-aware read
    /// within a week is deleted.
    fn expire_unrevalidated(&mut self, now: i64) -> rusqlite::Result<()> {
        let week_ago = now.saturating_sub(REVALIDATE_WITHIN_MS);
        let removed = self.delete_rows(
            "observed_ms < ?1 AND NOT EXISTS (
                 SELECT 1 FROM message_scope s
                 JOIN coverage c ON c.chan = s.chan AND c.scope_key = s.scope_key
                 WHERE s.row_id = message.row_id
                   AND c.t0_ms <= message.created_ms AND message.created_ms < c.t1_ms
                   AND c.full_ms >= ?1)",
            &[week_ago],
            None,
            Purpose::Expire,
        )?;
        Stats::add(&self.shared.stats.expired_rows, count(removed));
        Ok(())
    }

    /// Coverage of a channel never reaches below `floor`: the rows there are gone.
    fn raise_floor(&self, chan_id: i64, floor: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM coverage WHERE chan = ?1 AND t1_ms <= ?2",
            params![chan_id, floor],
        )?;
        self.conn.execute(
            "UPDATE coverage SET t0_ms = ?2 WHERE chan = ?1 AND t0_ms < ?2 AND t1_ms > ?2",
            params![chan_id, floor],
        )?;
        Ok(())
    }

    /// The size limit. Above it, delete the oldest rows across channels — as many as the excess
    /// over 90 % of the limit suggests — compact the index so their space is really free, and
    /// measure again; a few rounds at most. Then checkpoint and give the pages back.
    fn enforce_bytes(&mut self) {
        let max = self.shared.limits.max_bytes;
        if file::bytes_on_disk(self.path) <= max {
            return;
        }
        let target = max / 10 * 9;
        for _round in 0..EVICTION_ROUNDS {
            let measured = self.page_counted_bytes().and_then(|used| {
                let rows: i64 = self
                    .conn
                    .query_row("SELECT COUNT(*) FROM message", [], |row| row.get(0))?;
                Ok((used, u64::try_from(rows).unwrap_or(0)))
            });
            let (used, rows) = match measured {
                Ok(pair) => pair,
                Err(error) => {
                    self.report("size check", Err(error));
                    break;
                }
            };
            if used < target || rows == 0 {
                break;
            }
            // The share of rows to go is the share of bytes over the target, plus one percent.
            let share = (used - target).saturating_mul(rows) / used.max(1) + rows / 100 + 1;
            let wanted = usize::try_from(share).unwrap_or(usize::MAX);
            match self.delete_rows("1", &[], Some(wanted), Purpose::Evict) {
                Ok(n) => Stats::add(&self.shared.stats.evicted_rows, count(n)),
                Err(error) => {
                    self.report("eviction", Err(error));
                    break;
                }
            }
        }
        self.checkpoint_due = true;
        self.checkpoint();
        let vacuum = self.drain("PRAGMA incremental_vacuum");
        self.report("incremental vacuum", vacuum);
        self.checkpoint();
    }

    /// The size of the pages in use: the file's pages minus its free list.
    fn page_counted_bytes(&self) -> rusqlite::Result<u64> {
        let pages: i64 = self.conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let free: i64 = self
            .conn
            .query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
        let size: i64 = self.conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        Ok(u64::try_from((pages - free).max(0) * size).unwrap_or(0))
    }

    /// A `PASSIVE` checkpoint always; a `TRUNCATE` one only when no read is running, and never
    /// waiting on one: the busy timeout is zero for that statement, and a busy answer leaves the
    /// checkpoint due for the next batch.
    fn checkpoint(&mut self) {
        let passive = self.drain("PRAGMA wal_checkpoint(PASSIVE)");
        self.report("checkpoint", passive);
        if self.shared.readers_in_use.load(Ordering::SeqCst) != 0 {
            return;
        }
        let _ = self.conn.pragma_update(None, "busy_timeout", 0);
        let truncated = self
            .conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                row.get::<_, i64>(0)
            });
        let _ = self.conn.pragma_update(None, "busy_timeout", 2000);
        if truncated == Ok(0) {
            Stats::add(&self.shared.stats.checkpoints_truncated, 1);
            self.checkpoint_due = false;
        }
    }

    /// Rebuild `search_text` for the next chunk of rows with the current rendering rules.
    /// Returns `false` when it could not make progress.
    fn rebuild_chunk(&mut self) -> bool {
        let version = self.shared.search_text.version();
        let source = Arc::clone(&self.shared.search_text);
        let result = self.transaction(|w| {
            let after: i64 = w
                .conn
                .query_row(
                    "SELECT value FROM meta WHERE key = 'search_text_rebuild_after'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let rows: Vec<(i64, String, String, crate::model::Message, String)> = {
                let mut statement = w.conn.prepare(
                    "SELECT m.row_id, c.provider_key, c.channel_id, m.message_id, m.author_id,
                            m.author, m.author_is_bot, m.reply_to, m.content, m.extra,
                            m.search_text
                     FROM message m JOIN channel c ON c.chan = m.chan
                     WHERE m.row_id > ?1 ORDER BY m.row_id LIMIT ?2",
                )?;
                let rows = statement
                    .query_map(params![after, REBUILD_CHUNK], |row| {
                        let channel_id: String = row.get(2)?;
                        let message = rows::reconstruct(
                            &channel_id,
                            &row.get::<_, String>(3)?,
                            &row.get::<_, String>(4)?,
                            &row.get::<_, String>(5)?,
                            row.get(6)?,
                            row.get(7)?,
                            row.get(8)?,
                            &row.get::<_, String>(9)?,
                        );
                        Ok((row.get(0)?, row.get(1)?, channel_id, message, row.get(10)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            let mut last = after;
            for (row_id, provider_key, _, message, stored) in &rows {
                let rendered = source.rendered(provider_key, message);
                let text = super::search_text::build(&message.author, &message.content, &rendered);
                if &text != stored {
                    w.conn.execute(
                        "UPDATE message SET search_text = ?2 WHERE row_id = ?1",
                        params![row_id, text],
                    )?;
                }
                last = *row_id;
            }
            let done = i64::try_from(rows.len()).unwrap_or(0) < REBUILD_CHUNK;
            if done {
                w.conn.execute(
                    "INSERT INTO meta (key, value) VALUES ('search_text_version', ?1)
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                    [&version],
                )?;
                w.conn.execute(
                    "DELETE FROM meta WHERE key = 'search_text_rebuild_after'",
                    [],
                )?;
            } else {
                w.conn.execute(
                    "INSERT INTO meta (key, value) VALUES ('search_text_rebuild_after', ?1)
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                    [last.to_string()],
                )?;
            }
            Ok(done)
        });
        match result {
            Ok(done) => {
                if done {
                    self.shared
                        .search_text_current
                        .store(true, Ordering::SeqCst);
                }
                true
            }
            Err(error) => {
                self.report("search text rebuild", Err(error));
                false
            }
        }
    }
}
