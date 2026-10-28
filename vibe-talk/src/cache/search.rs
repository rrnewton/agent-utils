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
//! [`MessageCache::search`](super::MessageCache::search) (`#521 server-side-search`, reached by
//! the `/search` routes, `#605`) matches every candidate with the shared language of
//! [`crate::search_query`], case- and accent-insensitively. Since `#604` the stored `search_text` is
//! the page's haystack folded the same way, so a trigram of a folded term is a trigram of every
//! message holding it, and the index can drive: when every row's text is folded
//! ([`super::MessageCache::search_text_current`]) and the query has terms every match must hold
//! ([`required_terms`]) that [`choose`] finds rare in the channel, the candidates are the rows whose
//! text holds them all (`LIKE`, which the trigram index answers without positions, `detail = none`
//! since cache schema 5), and only those are tested. Otherwise, and while the column is being
//! rebuilt, the newest-first scan tests every row.

use rusqlite::{params, Connection, OptionalExtension as _};

use super::coverage::Horizon;
use super::rows::{CachedMessage, Scope};
use crate::search_query::{Day, Node, Query, Subject};

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

/// The FTS5 query for terms: every distinct trigram of every term, each quoted with `"` doubled so
/// nothing in a term is FTS syntax, joined by AND. Terms are lowercased, as `search_text` is.
///
/// Trigrams rather than a phrase per term because the index keeps no positions (`detail = none`,
/// cache schema 5, `#602 history-settings-and-limits`: 86 % smaller), and FTS5 refuses a phrase
/// without them. The rows holding every trigram of a term are a superset of the rows holding the
/// term, and every candidate is matched again in Rust ([`rematch`]), so the answer is the same. A
/// term shorter than three characters has no trigram and adds nothing: [`choose`] sends such a
/// search to the scan.
#[must_use]
pub fn fts_query(terms: &[String]) -> String {
    let mut grams: Vec<String> = Vec::new();
    for term in terms {
        let chars: Vec<char> = term.to_lowercase().chars().collect();
        for window in chars.windows(3) {
            let gram: String = window.iter().collect();
            let quoted = format!("\"{}\"", gram.replace('"', "\"\""));
            if !grams.contains(&quoted) {
                grams.push(quoted);
            }
        }
    }
    grams.join(" AND ")
}

/// The folded terms every message matching `query` must hold, that the index can be asked for
/// (`#604 server-side-search`): the query's own word or phrase, or those among its top-level AND,
/// each of three characters or more (a trigram index has nothing for a shorter one) and without
/// `%` or `_`, which `LIKE` would read as wildcards. Empty when there is none: the search scans.
#[must_use]
pub fn required_terms(query: &Query) -> Vec<String> {
    let indexable = |term: &String| term.chars().count() >= 3 && !term.contains(['%', '_']);
    let terms: Vec<String> = match query.root() {
        Some(Node::Word(term) | Node::Phrase(term)) => vec![term.clone()],
        Some(Node::And(children)) => children
            .iter()
            .filter_map(|child| match child {
                Node::Word(term) | Node::Phrase(term) => Some(term.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut out: Vec<String> = Vec::new();
    for term in terms.into_iter().filter(indexable) {
        if !out.contains(&term) {
            out.push(term);
        }
    }
    out
}

/// The `LIKE` pattern for a folded term: anywhere in `search_text`.
#[must_use]
pub fn like_pattern(term: &str) -> String {
    format!("%{term}%")
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
    /// Matching rows, newest first, that may be handed out: see `withheld` for the rest.
    pub hits: Vec<CachedMessage>,
    /// Rows tested.
    pub examined: usize,
    /// Where to continue: every row held in `[resume_below_ms, below_ms)` was tested, and older
    /// ones were not. A search stops only on a whole millisecond, so a continuation with
    /// `below_ms = resume_below_ms` skips nothing and repeats nothing. `None` when the search
    /// reached the oldest row the view holds.
    pub resume_below_ms: Option<i64>,
    /// Rows whose `search_text` was cut at its cap: a term only past the cap is not found in them.
    /// Of the rows tested on a scan; of every row in the stretch searched when the index drove,
    /// which offers none whose term lies past the cap.
    pub capped: usize,
    /// Rows held only as a pushed snippet, not yet read whole: matched on the snippet.
    pub partial: usize,
    /// The view's horizon, when its claims reach the head.
    pub horizon: Option<Horizon>,
    /// Whether the full-text index chose the candidates (`#604 server-side-search`): every row the
    /// query could match was among them, so `examined` counts candidates, not every row.
    pub indexed: bool,
    /// Matching rows left out of `hits` because a serve would not hand them out (`#605
    /// server-side-search`): a reply whose thread is in doubt or being erased, a row not seen since
    /// the latest offer dropped for the channel, or a text only a push brought.
    pub withheld: usize,
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

/// What a route asks of one channel's copy (`#605 server-side-search`): [`SearchRequest`] less the
/// view, which [`super::Sink::search`] chooses from the channel's provider.
pub struct SearchAsk<'a> {
    /// The parsed query.
    pub query: &'a Query,
    /// Only rows created before this provider time (exclusive).
    pub below_ms: i64,
    /// Stop after this many hits, on a whole millisecond.
    pub limit: usize,
    /// Stop after testing about this many rows.
    pub budget: usize,
    /// The start of a calendar day in the reader's time zone.
    pub day_start_ms: &'a dyn Fn(Day) -> i64,
}

/// One channel's copy searched: what was found, over which view, and how far the server's walk
/// of the channel's history stands.
#[derive(Clone, Debug)]
pub struct ChannelSearch {
    /// What the search found.
    pub outcome: SearchOutcome,
    /// The view searched: the channel's whole list.
    pub view: SearchView,
    /// The full-history walk's record for the channel, when one exists (`#595
    /// cache-full-history`): whether it reached the channel's beginning, or is still under way.
    pub frontier: Option<super::Frontier>,
}

/// Why the server's copy of a channel was not searched. Each is said to the reader, by its code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchRefusal {
    /// No cache file is open.
    NoCache,
    /// The operator has not let the server answer reads from its copy (`[message_cache] serve`).
    ServeOff,
    /// The channel is not on the list the cache is kept for.
    NotRouted,
    /// The provider's account has not been checked against the file yet.
    Unverified,
    /// The provider refused the server's credential.
    Suspended,
    /// Access to the channel is in doubt, or an erase of it is waiting.
    Blocked,
    /// The caller holds the read token, which is never answered from the server's copy.
    ReadScope,
}

impl SearchRefusal {
    /// The code the wire carries.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::NoCache => "no_cache",
            Self::ServeOff => "serve_off",
            Self::NotRouted => "not_kept",
            Self::Unverified => "unverified",
            Self::Suspended => "suspended",
            Self::Blocked => "in_doubt",
            Self::ReadScope => "read_scope",
        }
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
        indexed: false,
        withheld: 0,
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
        // The haystack is `search_text`, the page's haystack folded (`#604`), which keeps no
        // separate list of links: a link is a web address anywhere in it, as the page lists only
        // web links (`anchorsIn`). Text that spells one without linking it counts too.
        let subject = Subject::new(
            &row.search_text,
            &message.author,
            message.author_id.as_str(),
            created_ms,
            row.in_thread,
            row.search_text.contains("http://") || row.search_text.contains("https://"),
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
            "\"ab\"\"\" AND \"b\"\"c\" AND \"x o\" AND \" or\" AND \"or \" AND \"r y\""
        );
        assert_eq!(
            fts_query(&["abab".to_owned()]),
            "\"aba\" AND \"bab\"",
            "each trigram once"
        );
        assert_eq!(
            fts_query(&["go".to_owned()]),
            "",
            "too short for any trigram"
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
    fn the_terms_every_match_must_hold_are_the_ones_the_index_is_asked_for() {
        let terms = |q: &str| required_terms(&crate::search_query::parse(q));
        assert_eq!(terms("Café Zebra"), ["cafe", "zebra"]);
        assert_eq!(terms("\"hand off\" -x from:ada"), ["hand off"]);
        assert_eq!(
            terms("ca zebra zebra"),
            ["zebra"],
            "too short, and once each"
        );
        assert!(terms("a OR b").is_empty() && terms("zebra OR quokka").is_empty());
        assert!(terms("-zebra").is_empty());
        assert!(terms("50%_off").is_empty(), "LIKE wildcards");
        assert_eq!(like_pattern("zebra"), "%zebra%");
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
