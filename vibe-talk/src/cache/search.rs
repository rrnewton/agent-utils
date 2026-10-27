//! The search queries over the cache, exactly as the search milestone will run them.
//!
//! Defined here, before any route uses them, so that the store's benchmarks measure the real SQL
//! and the schema's indexes are chosen for it. Two shapes, because neither is fast for every
//! term:
//!
//! - **Rare terms: the full-text index drives**, forced with `CROSS JOIN`, and the scope check
//!   goes through `message_scope_by_row`. Measured at a fraction of a millisecond on 110k rows;
//!   the planner's own choice of joining the other way round took seconds.
//! - **Common or short terms: a newest-first scan** of the channel through `message_by_time`,
//!   which stops at the limit. A term shorter than three characters has no trigram at all.
//!
//! [`choose`] picks between them from the index's own document counts. Every candidate is matched
//! again in Rust ([`rematch`]) against the full text, so the 8 KB cap on `search_text` costs recall
//! only past 8 KB.
//!
//! Search terms are never logged, and an FTS error is reported by result code only: SQLite's
//! FTS syntax errors quote the query.
//!
//! [`MessageCache::search`](super::MessageCache::search) is the first caller (`#521
//! server-side-search`), reachable from no route yet: it scans newest first and matches every row
//! with the shared language of [`crate::search_query`], case- and accent-insensitively. It does not
//! use the index yet. The stored `search_text` is lower-cased but not folded, so a trigram of
//! `cafe` does not find "café"; the index can drive once the column is rebuilt folded (a new
//! [`super::search_text::SearchTextSource`] version), and until then a scan is the only path that
//! keeps the language's promise.

use rusqlite::{params, Connection, OptionalExtension as _};

use super::coverage::Horizon;
use super::rows::{CachedMessage, Scope};
use crate::search_query::{Day, Query, Subject};

/// Rare terms: the index first. Bind `:q`, `:chan`, `:from_ms`, `:before_ms`, `:limit`.
pub const RARE_SQL: &str = "
SELECT m.row_id FROM (SELECT rowid AS r FROM message_fts WHERE message_fts MATCH :q) AS f
CROSS JOIN message AS m ON m.row_id = f.r
WHERE m.chan = :chan AND m.created_ms >= :from_ms AND m.created_ms < :before_ms
  AND EXISTS (SELECT 1 FROM message_scope s WHERE s.row_id = m.row_id AND s.chan = :chan
              AND (s.scope_key = 'main' OR s.scope_key GLOB 'thread:*'))
ORDER BY m.created_ms DESC, m.numeric_id DESC LIMIT :limit";

/// The bridge's variant of [`RARE_SQL`]: its replies are listed in `flat`.
pub const RARE_SQL_FLAT: &str = "
SELECT m.row_id FROM (SELECT rowid AS r FROM message_fts WHERE message_fts MATCH :q) AS f
CROSS JOIN message AS m ON m.row_id = f.r
WHERE m.chan = :chan AND m.created_ms >= :from_ms AND m.created_ms < :before_ms
  AND EXISTS (SELECT 1 FROM message_scope s WHERE s.row_id = m.row_id AND s.chan = :chan
              AND s.scope_key = 'flat')
ORDER BY m.created_ms DESC, m.numeric_id DESC LIMIT :limit";

/// Thread search: threads whose messages match, newest hit first. Bind as [`RARE_SQL`].
pub const THREADS_SQL: &str = "
SELECT m.thread_id, COUNT(*) AS hits, MAX(m.created_ms) AS last_hit
FROM (SELECT rowid AS r FROM message_fts WHERE message_fts MATCH :q) AS f
CROSS JOIN message AS m ON m.row_id = f.r
WHERE m.chan = :chan AND m.thread_id IS NOT NULL
GROUP BY m.thread_id ORDER BY last_hit DESC LIMIT 50";

/// Common or short terms: the newest-first scan, one `instr` per term. Bind `:chan`, `:from_ms`,
/// `:before_ms`, `:limit`, and `:t1` … `:tN`.
#[must_use]
pub fn scan_sql(terms: usize) -> String {
    let mut sql = String::from(
        "
SELECT m.row_id FROM message AS m
WHERE m.chan = :chan AND m.created_ms >= :from_ms AND m.created_ms < :before_ms",
    );
    for n in 1..=terms.max(1) {
        sql.push_str(&format!("\n  AND instr(m.search_text, :t{n}) > 0"));
    }
    sql.push_str(
        "
  AND EXISTS (SELECT 1 FROM message_scope s WHERE s.row_id = m.row_id AND s.chan = :chan
              AND (s.scope_key = 'main' OR s.scope_key GLOB 'thread:*'))
ORDER BY m.created_ms DESC, m.numeric_id DESC LIMIT :limit",
    );
    sql
}

/// The FTS5 query for terms: each one quoted, `"` doubled, so nothing in a term is FTS syntax.
/// Terms are lowercased, as `search_text` is.
#[must_use]
pub fn fts_query(terms: &[String]) -> String {
    terms
        .iter()
        .map(|term| format!("\"{}\"", term.to_lowercase().replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Which query shape a search takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Path {
    /// The index drives: every term is rare.
    Index,
    /// The newest-first scan: a term is short, or common.
    Scan,
}

/// The share of rows at or above which a term counts as common.
pub const COMMON_SHARE: f64 = 0.02;

/// Choose the shape for a search of the channel interned as `chan`. The scan when any term is
/// shorter than three characters, or when the rarest trigram of the rarest term is in at least
/// [`COMMON_SHARE`] of the CHANNEL's rows; the index otherwise.
///
/// The index's document counts are for the whole file, so a trigram's count is set against the
/// channel's own size: the index path costs one probe per document the trigram is in, wherever
/// it is, and is chosen only while that is a small part of what a scan of the channel would read.
/// A term common in another channel can therefore send a small channel to the scan, whose cost
/// that channel's size bounds. A short term (or a common one) that is rare in a large channel
/// reads the whole channel; the search milestone bounds that scan and says how far back it got.
///
/// # Errors
///
/// When the vocabulary cannot be read.
pub fn choose(connection: &Connection, chan: i64, terms: &[String]) -> rusqlite::Result<Path> {
    if terms.is_empty() || terms.iter().any(|t| t.chars().count() < 3) {
        return Ok(Path::Scan);
    }
    let rows: i64 = connection.query_row(
        "SELECT COUNT(*) FROM message WHERE chan = ?1",
        [chan],
        |r| r.get(0),
    )?;
    if rows == 0 {
        return Ok(Path::Index);
    }
    let mut statement = connection.prepare("SELECT doc FROM message_fts_vocab WHERE term = ?1")?;
    let mut rarest = i64::MAX;
    for term in terms {
        let chars: Vec<char> = term.to_lowercase().chars().collect();
        for window in chars.windows(3) {
            let trigram: String = window.iter().collect();
            let docs: i64 = statement
                .query_row(params![trigram], |r| r.get(0))
                .optional()?
                .unwrap_or(0);
            rarest = rarest.min(docs);
        }
    }
    // Counts are small next to 2^52, so the conversion is exact.
    #[allow(clippy::cast_precision_loss)]
    let share = rarest as f64 / rows as f64;
    Ok(if share >= COMMON_SHARE {
        Path::Scan
    } else {
        Path::Index
    })
}

/// The page's rule, applied to a candidate's full text: every term is a substring of it,
/// case-insensitively.
#[must_use]
pub fn rematch(text: &str, terms: &[String]) -> bool {
    let text = text.to_lowercase();
    terms.iter().all(|term| text.contains(&term.to_lowercase()))
}

/// Which messages of a channel a search covers, as the page's views name them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchView {
    /// Top-level messages.
    Main,
    /// Slack's All: top-level messages and the replies of every thread the cache holds.
    All,
    /// A bridge's space-level list, replies included.
    Flat,
}

impl SearchView {
    /// The condition on `s.scope_key` selecting the view's rows.
    pub(crate) const fn scope_sql(self) -> &'static str {
        match self {
            Self::Main => "s.scope_key = 'main'",
            Self::All => "s.scope_key = 'main' OR s.scope_key GLOB 'thread:*'",
            Self::Flat => "s.scope_key = 'flat'",
        }
    }

    /// The scope whose coverage is the view's horizon. On Slack's All that is Main's, so the
    /// horizon is of top-level messages only: no claim says which threads' replies are all held.
    #[must_use]
    pub const fn horizon_scope(self) -> Scope {
        match self {
            Self::Main | Self::All => Scope::Main,
            Self::Flat => Scope::Flat,
        }
    }
}

/// A search of what the cache holds: the newest rows first, below `below_ms`.
pub struct SearchRequest<'a> {
    /// The parsed query; an empty one finds nothing.
    pub query: &'a Query,
    /// Which rows.
    pub view: SearchView,
    /// Only rows created before this provider time (exclusive): the page's loaded floor.
    pub below_ms: i64,
    /// Stop after this many hits (more when the last hit shares its millisecond with others:
    /// a search always stops on a whole millisecond). Zero tests nothing.
    pub limit: usize,
    /// Stop after testing about this many rows (more when rows share the last one's millisecond).
    /// Zero tests nothing.
    pub budget: usize,
    /// The start of a calendar day in the reader's time zone, for `before:` and `after:`.
    pub day_start_ms: &'a dyn Fn(Day) -> i64,
}

/// What a search of the cache found, and how far it can vouch for its absence of hits.
#[derive(Clone, Debug)]
pub struct SearchOutcome {
    /// Matching rows, newest first. A row held only as a pushed snippet (`content_partial`) is
    /// matched on the snippet and is never served as the message: a route fetches it whole first.
    pub hits: Vec<CachedMessage>,
    /// Rows tested.
    pub examined: usize,
    /// Where to continue: every row held in `[resume_below_ms, below_ms)` was tested, and older
    /// ones were not. A search stops only on a whole millisecond, so a continuation with
    /// `below_ms = resume_below_ms` skips nothing and repeats nothing. `None` when the search
    /// reached the oldest row the view holds.
    pub resume_below_ms: Option<i64>,
    /// Rows whose `search_text` was cut at its cap: a term only past the cap is not found in them.
    pub capped: usize,
    /// Rows held only as a pushed snippet, not yet read whole: matched on the snippet.
    pub partial: usize,
    /// The view's horizon, when its claims reach the head.
    pub horizon: Option<Horizon>,
}

impl SearchOutcome {
    /// How far back this search can say it looked: the oldest provider time from which every
    /// message of the view, created before `below_ms`, is both held (the horizon) and was tested.
    /// `None` without a horizon, or when the search stopped above it. On Slack's All, of
    /// top-level messages only ([`SearchView::horizon_scope`]).
    #[must_use]
    pub fn searched_back_to_ms(&self, below_ms: i64) -> Option<i64> {
        let horizon = self.horizon?;
        let from = self
            .resume_below_ms
            .unwrap_or(i64::MIN)
            .max(horizon.from_ms);
        (from < horizon.to_ms.min(below_ms)).then_some(from)
    }
}

/// Test `rows` (newest first, as [`super::reader::search_rows`] lists them: ending on a whole
/// millisecond) against the request, stopping at its limit on a whole millisecond; `more` says
/// whether older rows remained past the budget.
pub(crate) fn test_rows(
    rows: Vec<super::reader::SearchRow>,
    more: bool,
    request: &SearchRequest<'_>,
) -> SearchOutcome {
    let mut outcome = SearchOutcome {
        hits: Vec::new(),
        examined: 0,
        resume_below_ms: None,
        capped: 0,
        partial: 0,
        horizon: None,
    };
    if request.query.is_empty() || request.limit == 0 || request.budget == 0 {
        // Nothing tested, so nothing below `below_ms` is vouched for.
        outcome.resume_below_ms = Some(request.below_ms);
        return outcome;
    }
    let total = rows.len();
    let mut last_ms = None;
    for row in rows {
        let created_ms = row.message.created_ms;
        if outcome.hits.len() >= request.limit && last_ms != Some(created_ms) {
            break;
        }
        outcome.examined += 1;
        last_ms = Some(created_ms);
        outcome.capped += usize::from(super::search_text::was_capped(&row.search_text));
        outcome.partial += usize::from(row.message.content_partial);
        let message = &row.message.message;
        // The server has no drawn form of a cached row yet: the haystack is `search_text`, which
        // also holds the raw content (a parity gap with the page, design section 3, closed when
        // the column is rebuilt from the page's haystack), and a link is approximated as "://"
        // anywhere in it.
        let subject = Subject::new(
            &row.search_text,
            &message.author,
            message.author_id.as_str(),
            created_ms,
            row.in_thread,
            row.search_text.contains("://"),
        );
        if request.query.matches(&subject, request.day_start_ms) {
            outcome.hits.push(row.message);
        }
    }
    if outcome.examined < total || more {
        outcome.resume_below_ms = Some(last_ms.unwrap_or(request.below_ms));
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terms_are_quoted_so_nothing_in_one_is_syntax() {
        assert_eq!(
            fts_query(&["Ab\"c".to_owned(), "x OR y".to_owned()]),
            "\"ab\"\"c\" \"x or y\""
        );
    }

    #[test]
    fn a_term_is_judged_against_the_channels_own_size() {
        let connection = Connection::open_in_memory().expect("memory");
        connection
            .execute_batch(crate::cache::schema::STEPS[0].sql)
            .expect("schema");
        connection
            .execute_batch(
                "INSERT INTO channel (chan, provider_key, channel_id) VALUES (1, 'p', 'big');
                 INSERT INTO channel (chan, provider_key, channel_id) VALUES (2, 'p', 'small');",
            )
            .expect("channels");
        let mut insert = connection
            .prepare(
                "INSERT INTO message (chan, message_id, created_ms, observed_ms, home_scope,
                        author_id, author, author_is_bot, content, content_hash, search_text)
                 VALUES (?1, ?2, ?3, 0, 'main', 'u', 'a', 0, ?4, '', ?4)",
            )
            .expect("insert");
        for i in 0..1_000_i64 {
            let text = if i < 15 {
                format!("zebracrossing {i}")
            } else {
                format!("plain words {i}")
            };
            insert
                .execute(rusqlite::params![1, format!("b{i}"), i, text])
                .expect("row");
        }
        for i in 0..10_i64 {
            insert
                .execute(rusqlite::params![
                    2,
                    format!("s{i}"),
                    i,
                    format!("small {i}")
                ])
                .expect("row");
        }
        let terms = vec!["zebracrossing".to_owned()];
        // 15 documents: 1.5 % of the big channel, but more than the small one holds.
        assert_eq!(choose(&connection, 1, &terms).expect("choose"), Path::Index);
        assert_eq!(choose(&connection, 2, &terms).expect("choose"), Path::Scan);
        assert_eq!(
            choose(&connection, 1, &["ze".to_owned()]).expect("choose"),
            Path::Scan,
            "no trigram in it"
        );
    }

    #[test]
    fn the_scan_has_one_instr_per_term() {
        let sql = scan_sql(3);
        assert_eq!(sql.matches("instr(").count(), 3);
        assert!(sql.contains(":t3"));
        assert!(rematch(
            "Hello World",
            &["world".to_owned(), "HELLO".to_owned()]
        ));
        assert!(!rematch("Hello", &["world".to_owned()]));
    }
}
