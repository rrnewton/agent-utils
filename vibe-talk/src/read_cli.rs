//! Cache-only read CLI: `search`, `threads` and `thread`. `#2106 vibe-talk-unified-cache-read-cli`.
//!
//! One reader over the local message cache for three jobs the owner kept
//! doing by hand: keyword search at MESSAGE granularity, enumerating the
//! threads active in a window, and reading or searching one thread. It
//! refines the two older readers in place: `prompts` (cache-only, owner
//! messages, no keyword filter) and the server `search` (rich language,
//! but only through a running server with a read token).
//!
//! Cache-only by construction: this module opens the cache file
//! read-only and reads short-ref files from disk. It names no server
//! address, no token and no provider, and makes no network call; the
//! parse rejects the old server flags (`--server`, `--token-file`,
//! `--base-url`) so no caller can drift back to the running server.
//!
//! Search semantics (the owner's rule): a hit is a whole MESSAGE. Bare
//! terms are ANDed — a message holding `foo` in one line and `bar` in
//! another is a hit, which neither line grep nor paragraph grep gives —
//! `--any` makes them OR, `"quoted text"` is one contiguous phrase, and
//! matching is case-insensitive (the shared [`crate::search_query::fold`],
//! so accents fold too). The haystack is the author name and the message
//! text, as the page searches. Times are ET wall times: `--since` (end
//! defaults to now), `--until`, `--range FROM..TO`, `--hours`; a date or
//! `"date HH:MM"` in ET. A date-only `--until`/`TO` covers its whole day.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context as _;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

use crate::prompts::{channel_matches, PromptsConfig};

/// Help for `vibe-talk search --help` (cache-only).
pub const SEARCH_USAGE: &str = "\
vibe-talk search — search the local message cache, whole messages, no server

USAGE:
    vibe-talk search [TERM...] [\"quoted phrase\"...] [--any] [--mine]
                     [--brief] [--channel <id|name|#name|label|alias>]
                     [--hours N | --since WHEN | --until WHEN | --range FROM..TO]
                     [--limit N] [--json]
                     [--config PATH] [--config-overlay PATH]... [--cache PATH]
                     [--short-refs PATH]...

Reads ONLY the local message cache file (opened read-only): no server, no
provider calls, no token. A hit is a whole message holding every term (AND);
--any asks for any term. Terms are case-insensitive substrings of the author
and text; a quoted phrase must appear contiguously. With no terms and --mine
this is `vibe-talk prompts`: the owner's own messages in the window.

Each hit prints its ET time, channel, author, the whole message (or one line
with --brief), and its thread as Slack long id + slug + :thread: short id
when known. --json prints structured hits.

TIME (ET, America/New_York; a date or \"date HH:MM\"):
    --hours N        the last N hours, 1 to 168 (default window: 24h)
    --since WHEN     from WHEN to now
    --until WHEN     up to WHEN (a date alone covers its whole day)
    --range F..T     from F to T (a date-alone T covers its whole day)

OPTIONS:
    --mine             only the owner's own messages (configured owner ids)
    --any              OR the terms instead of AND
    --brief            one line per hit instead of the whole message
    --channel WHICH    only this channel (id, label, alias, or #name)
    --limit N          hits at most, newest first (default: 200; 1 to 1000)
    --cache PATH       the message cache file (else config [message_cache])
    --short-refs PATH  a short-refs.json (agentctl-short-refs/v1); repeatable.
                       Default: the config's [render] short_refs_files
    --json             structured JSON instead of text
    --help             print this message and exit

EXAMPLES:
    vibe-talk search foo bar
    vibe-talk search \"deploy freeze\" --any --since 2026-11-01
    vibe-talk search --mine --hours 48 --brief
";

/// Help for `vibe-talk threads --help`.
pub const THREADS_USAGE: &str = "\
vibe-talk threads — threads active in a window, from the local cache only

USAGE:
    vibe-talk threads [--channel <id|name|#name|label|alias>]
                      [--hours N | --since WHEN | --until WHEN | --range FROM..TO]
                      [--limit N] [--json]
                      [--config PATH] [--config-overlay PATH]... [--cache PATH]
                      [--short-refs PATH]...

Lists every thread with a cached message in the window, newest activity
first: channel, Slack long id, slug and :thread: short id when known, the
message count in the window, and last activity in ET. Cache-only: no
server, no provider calls. Time flags as in `vibe-talk search --help`;
the default window is the last 24 hours. --limit defaults to 200.

EXAMPLES:
    vibe-talk threads --hours 72
    vibe-talk threads --range 2026-11-01..2026-11-07 --json
";

/// Help for `vibe-talk thread --help`.
pub const THREAD_USAGE: &str = "\
vibe-talk thread — read or search one thread, from the local cache only

USAGE:
    vibe-talk thread <long-id|slug|:thread:N|N> [TERM...] [--any] [--mine]
                     [--brief] [--channel <id|name|#name|label|alias>]
                     [--hours N | --since WHEN | --until WHEN | --range FROM..TO]
                     [--json]
                     [--config PATH] [--config-overlay PATH]... [--cache PATH]
                     [--short-refs PATH]...

Names one thread by its Slack long id (its root ts), its slug, or its
:thread: short id (a bare number reads as one). Prints the thread's
messages oldest first, whole (or one line each with --brief); with terms,
only the messages holding them (AND, or --any for OR; quoted phrases as in
`vibe-talk search`). Without time flags the whole cached thread is read.
Cache-only: no server, no provider calls.

EXAMPLES:
    vibe-talk thread 1730000000.000100
    vibe-talk thread runner-wedged --brief
    vibe-talk thread :thread:60 deploy freeze
";

/// One search term: a word, or a quoted phrase that must appear whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Term {
    /// The term as typed (unquoted).
    pub text: String,
    /// Whether it was quoted, and so must match contiguously. A bare word
    /// also matches contiguously (substring), so this only records intent
    /// for display; matching treats both as folded substrings.
    pub phrase: bool,
}

/// Split one query string into terms, honouring `"quoted phrases"`.
/// An unclosed quote runs to the end, as the search language does.
#[must_use]
pub fn split_query(input: &str) -> Vec<Term> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut in_quote = false;
    let mut has = false;
    for c in input.chars() {
        if in_quote {
            if c == '"' {
                in_quote = false;
            } else {
                current.push(c);
            }
        } else if c == '"' {
            if !current.is_empty() {
                // `a"b c"`: the word before the quote stands alone.
                terms.push(Term {
                    text: std::mem::take(&mut current),
                    phrase: false,
                });
            }
            in_quote = true;
            quoted = true;
            has = true;
        } else if c.is_whitespace() {
            if has || !current.is_empty() {
                terms.push(Term {
                    text: std::mem::take(&mut current),
                    phrase: quoted,
                });
                quoted = false;
                has = false;
            }
        } else {
            current.push(c);
            has = true;
        }
    }
    if has || !current.is_empty() {
        terms.push(Term {
            text: current,
            phrase: quoted,
        });
    }
    terms
        .into_iter()
        .filter(|t| !t.text.trim().is_empty())
        .collect()
}

/// Terms from positional arguments: several arguments are one term each
/// (a space inside one makes it a phrase); a single argument is split as
/// a query string so typed quotes survive.
fn terms_from(words: &[String]) -> Vec<Term> {
    if words.len() == 1 {
        return split_query(&words[0]);
    }
    words
        .iter()
        .map(|w| Term {
            text: w.clone(),
            phrase: w.contains(char::is_whitespace),
        })
        .filter(|t| !t.text.trim().is_empty())
        .collect()
}

/// Whether a message matches: every term (or any, with `any`) is a folded
/// substring of the folded author + text haystack. Message granularity:
/// terms may sit anywhere in the one message.
#[must_use]
pub fn message_matches(author: &str, content: &str, terms: &[Term], any: bool) -> bool {
    if terms.is_empty() {
        return true;
    }
    let haystack = crate::search_query::fold(&format!("{author}\n{content}"));
    let hit = |term: &Term| haystack.contains(&crate::search_query::fold(&term.text));
    if any {
        terms.iter().any(hit)
    } else {
        terms.iter().all(hit)
    }
}

/// Flags shared by the three commands.
#[derive(Debug, Default)]
pub struct Common {
    /// `--config`.
    pub config: Option<PathBuf>,
    /// `--config-overlay`, in order.
    pub overlays: Vec<PathBuf>,
    /// `--cache`.
    pub cache: Option<PathBuf>,
    /// `--short-refs`, in order.
    pub short_refs: Vec<PathBuf>,
    /// `--channel`, as written.
    pub channel: Option<String>,
    /// `--hours`.
    pub hours: Option<u64>,
    /// `--since`, as written.
    pub since: Option<String>,
    /// `--until`, as written.
    pub until: Option<String>,
    /// `--range`, as written (`FROM..TO`).
    pub range: Option<String>,
    /// `--json`.
    pub json: bool,
    /// `--brief`.
    pub brief: bool,
    /// `--mine`.
    pub mine: bool,
    /// `--any`.
    pub any: bool,
    /// `--limit`.
    pub limit: Option<usize>,
}

/// What `search` was asked to do.
#[derive(Debug, Default)]
pub struct SearchOptions {
    /// Shared flags.
    pub common: Common,
    /// The positional terms.
    pub terms: Vec<Term>,
}

/// What `thread` was asked to do.
#[derive(Debug, Default)]
pub struct ThreadOptions {
    /// Shared flags.
    pub common: Common,
    /// The thread reference as written.
    pub reference: String,
    /// The positional terms after the reference.
    pub terms: Vec<Term>,
}

fn take_common(
    arg: String,
    args: &mut std::vec::IntoIter<String>,
    common: &mut Common,
    usage: &str,
) -> anyhow::Result<Option<String>> {
    // Returns Some(word) for a positional, None when a flag was consumed.
    match arg.as_str() {
        "--config" => common.config = Some(args.next().context("--config needs a path")?.into()),
        "--config-overlay" => common
            .overlays
            .push(args.next().context("--config-overlay needs a path")?.into()),
        "--cache" => common.cache = Some(args.next().context("--cache needs a path")?.into()),
        "--short-refs" => common
            .short_refs
            .push(args.next().context("--short-refs needs a path")?.into()),
        "--channel" => common.channel = Some(args.next().context("--channel needs a value")?),
        "--since" => common.since = Some(args.next().context("--since needs a value")?),
        "--until" => common.until = Some(args.next().context("--until needs a value")?),
        "--range" => common.range = Some(args.next().context("--range needs FROM..TO")?),
        "--hours" => {
            let value = args.next().context("--hours needs a number")?;
            common.hours = Some(
                value
                    .parse::<u64>()
                    .with_context(|| format!("--hours takes whole hours, not {value:?}"))?,
            );
        }
        "--limit" => {
            let value = args.next().context("--limit needs a number")?;
            let limit = value
                .parse::<usize>()
                .with_context(|| format!("--limit takes a count, not {value:?}"))?;
            if !(1..=1000).contains(&limit) {
                anyhow::bail!("--limit must be between 1 and 1000, not {limit}");
            }
            common.limit = Some(limit);
        }
        "--json" => common.json = true,
        "--brief" => common.brief = true,
        "--mine" => common.mine = true,
        "--any" => common.any = true,
        "--server" | "--token-file" | "--base-url" => anyhow::bail!(
            "{arg} belongs to the old server search; this command is cache-only and never \
             reaches a server\n\n{usage}"
        ),
        other if other.starts_with("--") => {
            anyhow::bail!("unrecognized argument {other:?}\n\n{usage}")
        }
        other => return Ok(Some(other.to_owned())),
    }
    Ok(None)
}

fn check_common(common: &Common) -> anyhow::Result<()> {
    let windows = usize::from(common.hours.is_some())
        + usize::from(common.since.is_some() || common.until.is_some())
        + usize::from(common.range.is_some());
    if common.range.is_some() && (common.since.is_some() || common.until.is_some()) {
        anyhow::bail!("--range already names both ends; drop --since/--until");
    }
    if common.hours.is_some() && windows > 1 {
        anyhow::bail!("--hours names the whole window; drop --since/--until/--range");
    }
    if let Some(hours) = common.hours {
        if !(1..=168).contains(&hours) {
            anyhow::bail!("--hours must be between 1 and 168, not {hours}");
        }
    }
    Ok(())
}

/// Parse the arguments after `search`. `Ok(None)` means `--help` was printed.
///
/// # Errors
///
/// An unknown argument, a flag without its value, or conflicting windows.
pub fn parse_search(
    args: impl IntoIterator<Item = String>,
) -> anyhow::Result<Option<SearchOptions>> {
    let mut options = SearchOptions::default();
    let mut words: Vec<String> = Vec::new();
    // `--help` anywhere prints and exits, as the older commands do.
    let collected: Vec<String> = args.into_iter().collect();
    if collected.iter().any(|a| a == "--help" || a == "-h") {
        print!("{SEARCH_USAGE}");
        return Ok(None);
    }
    let mut iter = collected.into_iter();
    while let Some(arg) = iter.next() {
        if let Some(word) = take_common(arg, &mut iter, &mut options.common, SEARCH_USAGE)? {
            words.push(word);
        }
    }
    check_common(&options.common)?;
    options.terms = terms_from(&words);
    Ok(Some(options))
}

/// Parse the arguments after `threads`. `Ok(None)` means `--help` was printed.
///
/// # Errors
///
/// As [`parse_search`], plus a positional (threads takes none).
pub fn parse_threads(args: impl IntoIterator<Item = String>) -> anyhow::Result<Option<Common>> {
    let mut common = Common::default();
    let collected: Vec<String> = args.into_iter().collect();
    if collected.iter().any(|a| a == "--help" || a == "-h") {
        print!("{THREADS_USAGE}");
        return Ok(None);
    }
    let mut iter = collected.into_iter();
    while let Some(arg) = iter.next() {
        if let Some(word) = take_common(arg, &mut iter, &mut common, THREADS_USAGE)? {
            anyhow::bail!("threads takes no query, got {word:?}\n\n{THREADS_USAGE}");
        }
    }
    check_common(&common)?;
    Ok(Some(common))
}

/// Parse the arguments after `thread`. `Ok(None)` means `--help` was printed.
///
/// # Errors
///
/// As [`parse_search`], plus no thread reference.
pub fn parse_thread(
    args: impl IntoIterator<Item = String>,
) -> anyhow::Result<Option<ThreadOptions>> {
    let mut options = ThreadOptions::default();
    let collected: Vec<String> = args.into_iter().collect();
    if collected.iter().any(|a| a == "--help" || a == "-h") {
        print!("{THREAD_USAGE}");
        return Ok(None);
    }
    let mut iter = collected.into_iter();
    let mut words: Vec<String> = Vec::new();
    while let Some(arg) = iter.next() {
        if let Some(word) = take_common(arg, &mut iter, &mut options.common, THREAD_USAGE)? {
            words.push(word);
        }
    }
    check_common(&options.common)?;
    let Some(reference) = words.first().cloned() else {
        anyhow::bail!("a thread reference is required\n\n{THREAD_USAGE}");
    };
    options.reference = reference;
    options.terms = terms_from(&words[1..]);
    Ok(Some(options))
}

/// Parse an ET instant: RFC 3339, or `YYYY-MM-DD[ HH:MM]` in ET.
/// Reuses the prompts window parser so both commands read times alike.
///
/// # Errors
///
/// The value is neither form.
pub fn parse_instant(raw: &str) -> anyhow::Result<i64> {
    crate::prompts::parse_since(raw)
}

/// Whether `raw` is a bare date (no time of day), and so an `--until`
/// bound covers its whole day.
fn is_date_only(raw: &str) -> bool {
    let raw = raw.trim();
    !raw.contains([' ', 'T']) && crate::search_query::Day::parse(raw).is_some()
}

/// The exclusive end an `--until`/`TO` value names: a bare date ends at
/// the next ET midnight; a date-time is itself.
fn until_bound(raw: &str) -> anyhow::Result<i64> {
    let start = parse_instant(raw)?;
    if is_date_only(raw) {
        Ok(start + 86_400_000)
    } else {
        Ok(start)
    }
}

/// The `[since_ms, until_ms)` window the flags name, at `now_ms`.
///
/// # Errors
///
/// An unreadable time, or an empty or backwards window.
pub fn resolve_window(common: &Common, now_ms: i64) -> anyhow::Result<(i64, i64)> {
    let (since_ms, until_ms) = if let Some(range) = &common.range {
        let Some((from, to)) = range.split_once("..") else {
            anyhow::bail!("--range takes FROM..TO, not {range:?}");
        };
        (parse_instant(from)?, until_bound(to)?)
    } else if common.since.is_some() || common.until.is_some() {
        let since = match &common.since {
            Some(raw) => parse_instant(raw)?,
            None => i64::MIN,
        };
        let until = match &common.until {
            Some(raw) => until_bound(raw)?,
            None => now_ms,
        };
        (since, until)
    } else {
        let hours = common.hours.unwrap_or(24);
        (
            now_ms - i64::try_from(hours).unwrap_or(24) * 3_600_000,
            now_ms,
        )
    };
    if since_ms >= until_ms {
        anyhow::bail!("the window is empty: its start is not before its end");
    }
    Ok((since_ms, until_ms))
}

/// Short references read from `short-refs.json` files: the bridge's
/// `(channel, number)` assignments, kept both ways round. A missing or
/// unparsable file contributes nothing, never a wrong link, exactly as
/// [`crate::short_refs`] treats one.
#[derive(Default)]
struct ShortRefsLite {
    /// `(channel, root ts)` → `:thread:` number.
    by_thread: HashMap<(String, String), u64>,
    /// Number → `(channel, root ts)` candidates.
    by_number: HashMap<u64, Vec<(String, String)>>,
    /// Slug → `(channel, root ts)` candidates, from cached root names.
    by_slug: HashMap<String, Vec<(String, String)>>,
}

impl ShortRefsLite {
    fn load(files: &[PathBuf]) -> Self {
        #[derive(serde::Deserialize)]
        struct File {
            schema: String,
            #[serde(default)]
            threads: Vec<Entry>,
        }
        #[derive(serde::Deserialize)]
        struct Entry {
            n: u64,
            channel: String,
            root_ts: String,
        }
        let mut out = Self::default();
        for path in files {
            let Ok(body) = std::fs::read_to_string(path) else {
                continue;
            };
            let Ok(file) = serde_json::from_str::<File>(&body) else {
                continue;
            };
            if file.schema != crate::short_refs::SCHEMA {
                continue;
            }
            for entry in file.threads {
                out.by_thread
                    .insert((entry.channel.clone(), entry.root_ts.clone()), entry.n);
                out.by_number
                    .entry(entry.n)
                    .or_default()
                    .push((entry.channel, entry.root_ts));
            }
        }
        out
    }

    /// The `:thread:N` label for a thread, when a file assigns one.
    fn label(&self, channel_id: &str, thread_id: &str) -> Option<String> {
        self.by_thread
            .get(&(channel_id.to_owned(), thread_id.to_owned()))
            .map(|n| format!(":thread:{n}"))
    }
}

/// One cached message as the read CLI hands it out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Hit {
    /// Provider channel id.
    pub channel_id: String,
    /// Configured channel label, when configured.
    pub channel: Option<String>,
    /// Provider message id.
    pub message_id: String,
    /// Creation instant, Unix ms.
    pub created_ms: i64,
    /// ET time label, e.g. `2026-11-12 14:24 ET`.
    pub et: String,
    /// Author display name as cached.
    pub author: String,
    /// Author id as cached.
    pub author_id: String,
    /// Message text, whole.
    pub content: String,
    /// Thread long id (Slack root ts), when in a thread.
    pub thread_id: Option<String>,
    /// The thread's slug, when a name is filed for it.
    pub thread_slug: Option<String>,
    /// The thread's `:thread:N` short id, when assigned.
    pub thread_short: Option<String>,
    /// Whether the message is its thread's root.
    pub is_root: bool,
    /// Whether the cached text may be partial.
    pub content_partial: bool,
}

/// A search answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SearchAnswer {
    /// Window start, Unix ms.
    pub since_ms: i64,
    /// Window end, Unix ms.
    pub until_ms: i64,
    /// Matching messages (newest first; oldest first for a thread read).
    pub hits: Vec<Hit>,
    /// Whether `--limit` cut the answer short.
    pub truncated: bool,
}

/// One thread active in a window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ThreadRow {
    /// Provider channel id.
    pub channel_id: String,
    /// Configured channel label, when configured.
    pub channel: Option<String>,
    /// Thread long id (Slack root ts).
    pub thread_id: String,
    /// The thread's slug, when filed.
    pub slug: Option<String>,
    /// The thread's `:thread:N` short id, when assigned.
    pub short_id: Option<String>,
    /// Cached messages of the thread inside the window.
    pub message_count: i64,
    /// Last activity inside the window, Unix ms.
    pub last_activity_ms: i64,
    /// Last activity as an ET label.
    pub last_et: String,
}

/// Open the cache read-only. This is the module's only database door:
/// read-only flags, no writer, no network.
fn open_cache(cache_file: &std::path::Path) -> anyhow::Result<Connection> {
    let connection = Connection::open_with_flags(
        cache_file,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("cannot open cache {} read-only", cache_file.display()))?;
    Ok(connection)
}

fn has_column(connection: &Connection, table: &str, column: &str) -> bool {
    connection
        .query_row(
            &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = '{column}'"),
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false)
}

/// Channel labels: the config's, widened by the state store's learned
/// names when that file is there (as `prompts` reads them).
fn channel_labels(config: &PromptsConfig) -> HashMap<String, String> {
    let mut labels: HashMap<String, String> = config
        .channels
        .iter()
        .map(|c| (c.id.as_str().to_owned(), c.display_name().to_owned()))
        .collect();
    if let Some(store_path) = &config.storage_path {
        if let Ok(store) = Connection::open_with_flags(
            store_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            for sql in [
                "SELECT channel_id, label FROM added_channels",
                "SELECT channel_id, alias FROM channel_aliases",
            ] {
                if let Ok(mut statement) = store.prepare(sql) {
                    if let Ok(rows) = statement.query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    }) {
                        for row in rows.flatten() {
                            labels.insert(row.0, row.1.trim_start_matches('#').to_owned());
                        }
                    }
                }
            }
        }
    }
    labels
}

/// The configured channel ids a `--channel` value selects; an unknown
/// name selects the value itself as a channel id (a cached channel the
/// config does not list still answers to its id, as in `prompts`).
fn selected_channels(config: &PromptsConfig, want: Option<&str>) -> Option<Vec<String>> {
    let want = want?;
    let ids: Vec<String> = config
        .channels
        .iter()
        .filter(|c| channel_matches(want, c))
        .map(|c| c.id.as_str().to_owned())
        .collect();
    if ids.is_empty() {
        Some(vec![want.trim().trim_start_matches('#').trim().to_owned()])
    } else {
        Some(ids)
    }
}

/// Thread slugs from cached roots: a root's stored `thread.display_name`,
/// held to slug shape. Built once per run from the `extra` column.
fn thread_slugs(connection: &Connection) -> HashMap<(String, String), String> {
    let mut out = HashMap::new();
    if !has_column(connection, "message", "extra") {
        return out;
    }
    let Ok(mut statement) = connection.prepare(
        "SELECT c.channel_id, m.thread_id, m.extra FROM message m \
         JOIN channel c ON c.chan = m.chan \
         WHERE m.is_root = 1 AND m.thread_id IS NOT NULL",
    ) else {
        return out;
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    }) else {
        return out;
    };
    for row in rows.flatten() {
        #[derive(serde::Deserialize)]
        struct Extra {
            thread: Option<Thread>,
        }
        #[derive(serde::Deserialize)]
        struct Thread {
            display_name: Option<String>,
        }
        if let Ok(extra) = serde_json::from_str::<Extra>(&row.2) {
            if let Some(name) = extra.thread.and_then(|t| t.display_name) {
                if crate::summarize::threads::valid_thread_slug(&name) {
                    out.insert((row.0, row.1), name);
                }
            }
        }
    }
    out
}

/// One message listing: a window, its filters, its order and its cap.
struct MessageQuery<'a> {
    since_ms: i64,
    until_ms: i64,
    thread: Option<&'a str>,
    terms: &'a [Term],
    any: bool,
    mine: bool,
    newest_first: bool,
    limit: usize,
}

/// The reader's working set, opened once per command.
struct Reader {
    connection: Connection,
    labels: HashMap<String, String>,
    slugs: HashMap<(String, String), String>,
    shorts: ShortRefsLite,
    channels: Option<Vec<String>>,
    owners: Vec<String>,
}

impl Reader {
    fn open(
        config: &PromptsConfig,
        cache_file: &std::path::Path,
        common: &Common,
    ) -> anyhow::Result<Self> {
        let connection = open_cache(cache_file)?;
        let mut shorts = ShortRefsLite::load(&short_ref_files(config, common));
        let slugs = thread_slugs(&connection);
        for ((channel, thread), slug) in &slugs {
            shorts
                .by_slug
                .entry(slug.clone())
                .or_default()
                .push((channel.clone(), thread.clone()));
        }
        Ok(Self {
            labels: channel_labels(config),
            slugs,
            shorts,
            channels: selected_channels(config, common.channel.as_deref()),
            owners: crate::prompts::owner_ids(config),
            connection,
        })
    }

    fn hit(
        &self,
        row: &(
            String,
            String,
            i64,
            String,
            String,
            String,
            Option<String>,
            i64,
            i64,
        ),
    ) -> Hit {
        let (
            channel_id,
            message_id,
            created_ms,
            author,
            author_id,
            content,
            thread_id,
            is_root,
            partial,
        ) = row;
        let (slug, short) = match thread_id {
            Some(thread) => (
                self.slugs
                    .get(&(channel_id.clone(), thread.clone()))
                    .cloned(),
                self.shorts.label(channel_id, thread),
            ),
            None => (None, None),
        };
        Hit {
            channel: self.labels.get(channel_id).cloned(),
            channel_id: channel_id.clone(),
            message_id: message_id.clone(),
            created_ms: *created_ms,
            et: crate::prompts::et_label(*created_ms),
            author: author.clone(),
            author_id: author_id.clone(),
            content: content.clone(),
            thread_id: thread_id.clone(),
            thread_slug: slug,
            thread_short: short,
            is_root: *is_root != 0,
            content_partial: *partial != 0,
        }
    }

    /// Messages in `[since_ms, until_ms)` matching every filter, in the
    /// order named (`newest_first`), capped at `limit` after matching.
    /// Matching happens in Rust over the whole message, so terms split
    /// across lines still hit: that is the message granularity.
    fn messages(&self, query: &MessageQuery<'_>) -> anyhow::Result<(Vec<Hit>, bool)> {
        let (since_ms, until_ms, thread, terms, any, mine, newest_first, limit) = (
            query.since_ms,
            query.until_ms,
            query.thread,
            query.terms,
            query.any,
            query.mine,
            query.newest_first,
            query.limit,
        );
        if mine && self.owners.is_empty() {
            anyhow::bail!(
                "the configuration names no owner: set [owner] user_ids or a provider owner_user_id"
            );
        }
        let mut sql = String::from(
            "SELECT c.channel_id, m.message_id, m.created_ms, m.author, m.author_id, \
             m.content, m.thread_id, m.is_root, m.content_partial \
             FROM message m JOIN channel c ON c.chan = m.chan \
             WHERE m.created_ms >= ? AND m.created_ms < ?",
        );
        let mut params: Vec<rusqlite::types::Value> = vec![since_ms.into(), until_ms.into()];
        if let Some(ids) = &self.channels {
            let marks = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            sql.push_str(&format!(" AND c.channel_id IN ({marks})"));
            params.extend(ids.iter().cloned().map(rusqlite::types::Value::from));
        }
        if mine {
            let marks = self
                .owners
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            sql.push_str(&format!(" AND m.author_id IN ({marks})"));
            params.extend(
                self.owners
                    .iter()
                    .cloned()
                    .map(rusqlite::types::Value::from),
            );
        }
        if let Some(thread) = thread {
            sql.push_str(" AND m.thread_id = ?");
            params.push(thread.to_owned().into());
        }
        sql.push_str(if newest_first {
            " ORDER BY m.created_ms DESC, m.numeric_id DESC"
        } else {
            " ORDER BY m.created_ms, m.numeric_id"
        });
        let mut statement = self
            .connection
            .prepare(&sql)
            .context("preparing the cache query")?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            })
            .context("reading the cache")?;
        let mut hits = Vec::new();
        let mut truncated = false;
        for row in rows {
            let row = row.context("reading a cached message")?;
            if !message_matches(&row.3, &row.5, terms, any) {
                continue;
            }
            if hits.len() >= limit {
                truncated = true;
                break;
            }
            hits.push(self.hit(&row));
        }
        Ok((hits, truncated))
    }

    /// Threads with a cached message in the window.
    fn threads(
        &self,
        since_ms: i64,
        until_ms: i64,
        limit: usize,
    ) -> anyhow::Result<(Vec<ThreadRow>, bool)> {
        let mut sql = String::from(
            "SELECT c.channel_id, m.thread_id, COUNT(*), MAX(m.created_ms) \
             FROM message m JOIN channel c ON c.chan = m.chan \
             WHERE m.thread_id IS NOT NULL AND m.created_ms >= ? AND m.created_ms < ?",
        );
        let mut params: Vec<rusqlite::types::Value> = vec![since_ms.into(), until_ms.into()];
        if let Some(ids) = &self.channels {
            let marks = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            sql.push_str(&format!(" AND c.channel_id IN ({marks})"));
            params.extend(ids.iter().cloned().map(rusqlite::types::Value::from));
        }
        sql.push_str(" GROUP BY c.channel_id, m.thread_id ORDER BY MAX(m.created_ms) DESC");
        let mut statement = self
            .connection
            .prepare(&sql)
            .context("preparing the thread query")?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .context("reading the cache")?;
        let mut out = Vec::new();
        let mut truncated = false;
        for row in rows {
            let (channel_id, thread_id, count, last_ms) = row.context("reading a cached thread")?;
            if out.len() >= limit {
                truncated = true;
                break;
            }
            out.push(ThreadRow {
                channel: self.labels.get(&channel_id).cloned(),
                slug: self
                    .slugs
                    .get(&(channel_id.clone(), thread_id.clone()))
                    .cloned(),
                short_id: self.shorts.label(&channel_id, &thread_id),
                channel_id,
                thread_id,
                message_count: count,
                last_activity_ms: last_ms,
                last_et: crate::prompts::et_label(last_ms),
            });
        }
        Ok((out, truncated))
    }

    /// Resolve a thread reference to `(channel_id, thread_id)`: a long id
    /// verbatim, a `:thread:N`/`thread:N`/bare number through the short
    /// refs, else a slug. Ambiguity refuses rather than guesses.
    fn resolve_thread(&self, reference: &str) -> anyhow::Result<(String, String)> {
        let reference = reference.trim();
        let number: Option<u64> = reference
            .strip_prefix(":thread:")
            .or_else(|| reference.strip_prefix("thread:"))
            .and_then(|n| n.parse().ok())
            .or_else(|| reference.parse().ok());
        if let Some(n) = number {
            let mut candidates: Vec<(String, String)> =
                self.shorts.by_number.get(&n).cloned().unwrap_or_default();
            if let Some(ids) = &self.channels {
                candidates.retain(|(channel, _)| ids.contains(channel));
            }
            return match candidates.len() {
                1 => Ok(candidates.pop().unwrap_or_default()),
                0 => anyhow::bail!("no thread is :thread:{n} in the short refs read"),
                _ => anyhow::bail!(
                    ":thread:{n} names {} threads; --channel settles it: {}",
                    candidates.len(),
                    candidates
                        .iter()
                        .map(|(c, t)| format!("{c}/{t}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
        }
        if let Some(mut candidates) = self.shorts.by_slug.get(reference).cloned() {
            if let Some(ids) = &self.channels {
                candidates.retain(|(channel, _)| ids.contains(channel));
            }
            return match candidates.len() {
                1 => Ok(candidates.pop().unwrap_or_default()),
                0 => anyhow::bail!("no thread is slugged {reference:?} in the cache"),
                _ => anyhow::bail!(
                    "slug {reference:?} names {} threads; --channel settles it",
                    candidates.len()
                ),
            };
        }
        // A long id: it must exist as a thread in the windowless cache.
        let found: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM message WHERE thread_id = ?",
                [reference],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if found == 0 {
            anyhow::bail!(
                "{reference:?} is no thread long id, slug or :thread: number the cache and short refs know"
            );
        }
        let channel_id: String = self
            .connection
            .query_row(
                "SELECT c.channel_id FROM message m JOIN channel c ON c.chan = m.chan \
                 WHERE m.thread_id = ? LIMIT 1",
                [reference],
                |row| row.get(0),
            )
            .context("reading the thread's channel")?;
        Ok((channel_id, reference.to_owned()))
    }
}

/// The short-ref files a run reads: `--short-refs`, else the config's.
fn short_ref_files(config: &PromptsConfig, common: &Common) -> Vec<PathBuf> {
    if common.short_refs.is_empty() {
        config.short_refs_files.clone()
    } else {
        common.short_refs.clone()
    }
}

/// Load config + cache path the way `prompts` does (lenient, tokenless).
fn load(common: &Common) -> anyhow::Result<(PromptsConfig, PathBuf)> {
    let options = crate::prompts::Options {
        config: common.config.clone(),
        overlays: common.overlays.clone(),
        cache: common.cache.clone(),
        hours: None,
        since: None,
        channel: None,
        json: common.json,
    };
    let config = crate::prompts::discover_config(&options)?;
    let cache_file = crate::prompts::cache_file(&config, &options)?;
    Ok((config, cache_file))
}

/// One brief line for a hit.
fn brief_line(hit: &Hit) -> String {
    let channel = hit.channel.as_deref().unwrap_or(&hit.channel_id);
    let first = hit.content.lines().next().unwrap_or("").trim();
    let cut: String = first.chars().take(120).collect();
    let snippet = if first.chars().count() > 120 || hit.content.lines().count() > 1 {
        format!("{cut} …")
    } else {
        cut
    };
    format!(
        "{} · #{} · {} · {}{}",
        hit.et,
        channel,
        hit.author,
        snippet,
        thread_note(hit)
    )
}

/// The ` · thread …` suffix naming a hit's thread, when it has one.
fn thread_note(hit: &Hit) -> String {
    let Some(thread) = &hit.thread_id else {
        return String::new();
    };
    let mut note = format!(" · thread {thread}");
    if let Some(slug) = &hit.thread_slug {
        note.push_str(&format!(" {slug}"));
    }
    if let Some(short) = &hit.thread_short {
        note.push_str(&format!(" {short}"));
    }
    if hit.is_root {
        note.push_str(" (root)");
    }
    note
}

/// Render hits alone (no window header): whole messages, or brief lines.
#[must_use]
pub fn render_hits(hits: &[Hit], brief: bool) -> String {
    let mut out = String::new();
    for hit in hits {
        if brief {
            out.push_str(&brief_line(hit));
            out.push('\n');
        } else {
            let channel = hit.channel.as_deref().unwrap_or(&hit.channel_id);
            let partial = if hit.content_partial {
                " [partial]"
            } else {
                ""
            };
            out.push_str(&format!(
                "- {} · #{} · {} · {}{}{}\n  {}\n",
                hit.et,
                channel,
                hit.author,
                hit.message_id,
                thread_note(hit),
                partial,
                hit.content.trim().replace('\n', "\n  "),
            ));
        }
    }
    out
}

/// Render search hits as text: a window header, then the hits.
#[must_use]
pub fn render_search(answer: &SearchAnswer, brief: bool) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Window: {} to {} · {} hit(s){}\n",
        crate::prompts::et_label(answer.since_ms),
        crate::prompts::et_label(answer.until_ms),
        answer.hits.len(),
        if answer.truncated {
            " (more below; raise --limit)"
        } else {
            ""
        },
    ));
    out.push_str(&render_hits(&answer.hits, brief));
    out
}

/// Render a thread list as text.
#[must_use]
pub fn render_threads(rows: &[ThreadRow], truncated: bool) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} thread(s){}\n",
        rows.len(),
        if truncated {
            " (more below; raise --limit)"
        } else {
            ""
        }
    ));
    for row in rows {
        let channel = row.channel.as_deref().unwrap_or(&row.channel_id);
        let mut line = format!(
            "- {} · #{} · {} message(s) · last {}",
            row.thread_id, channel, row.message_count, row.last_et
        );
        if let Some(slug) = &row.slug {
            line.push_str(&format!(" · {slug}"));
        }
        if let Some(short) = &row.short_id {
            line.push_str(&format!(" · {short}"));
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Run `vibe-talk search` at `now_ms` and return what to print. Cache-only.
///
/// `prompts` equivalence: `--mine` with no terms, full (not `--brief`)
/// output is rendered by `prompts` itself, so the two agree line for line.
///
/// # Errors
///
/// Discovery, cache, window or owner errors as in the prompts reader.
pub fn search_at(options: SearchOptions, now_ms: i64) -> anyhow::Result<String> {
    let common = &options.common;
    if options.terms.is_empty() && !common.mine {
        anyhow::bail!("a query is required (or --mine for the owner's messages)\n\n{SEARCH_USAGE}");
    }
    let (config, cache_file) = load(common)?;
    let (since_ms, until_ms) = resolve_window(common, now_ms)?;
    if common.mine && options.terms.is_empty() && !common.brief && !common.json {
        let answer = crate::prompts::read_at(
            &config,
            &cache_file,
            common.channel.as_deref(),
            since_ms,
            until_ms,
        )?;
        return Ok(crate::prompts::render(&answer));
    }
    let reader = Reader::open(&config, &cache_file, common)?;
    let limit = common.limit.unwrap_or(200);
    let (hits, truncated) = reader.messages(&MessageQuery {
        since_ms,
        until_ms,
        thread: None,
        terms: &options.terms,
        any: common.any,
        mine: common.mine,
        newest_first: true,
        limit,
    })?;
    let answer = SearchAnswer {
        since_ms,
        until_ms,
        hits,
        truncated,
    };
    if common.json {
        return serde_json::to_string_pretty(&answer).context("rendering JSON");
    }
    Ok(render_search(&answer, common.brief))
}

/// Run `vibe-talk threads` at `now_ms` and return what to print.
///
/// # Errors
///
/// As [`search_at`].
pub fn threads_at(common: Common, now_ms: i64) -> anyhow::Result<String> {
    let (config, cache_file) = load(&common)?;
    let (since_ms, until_ms) = resolve_window(&common, now_ms)?;
    let reader = Reader::open(&config, &cache_file, &common)?;
    let (rows, truncated) = reader.threads(since_ms, until_ms, common.limit.unwrap_or(200))?;
    if common.json {
        #[derive(Serialize)]
        struct Answer {
            since_ms: i64,
            until_ms: i64,
            threads: Vec<ThreadRow>,
            truncated: bool,
        }
        return serde_json::to_string_pretty(&Answer {
            since_ms,
            until_ms,
            threads: rows,
            truncated,
        })
        .context("rendering JSON");
    }
    Ok(render_threads(&rows, truncated))
}

/// Run `vibe-talk thread` at `now_ms` and return what to print. Without
/// time flags the whole cached thread is read; with them, that window.
///
/// # Errors
///
/// As [`search_at`], plus an unknown or ambiguous thread reference.
pub fn thread_at(options: ThreadOptions, now_ms: i64) -> anyhow::Result<String> {
    let common = &options.common;
    let (config, cache_file) = load(common)?;
    let windowed = common.hours.is_some()
        || common.since.is_some()
        || common.until.is_some()
        || common.range.is_some();
    let (since_ms, until_ms) = if windowed {
        resolve_window(common, now_ms)?
    } else {
        (i64::MIN, i64::MAX)
    };
    let reader = Reader::open(&config, &cache_file, common)?;
    let (_channel, thread_id) = reader.resolve_thread(&options.reference)?;
    // The resolved thread may sit outside a --channel selection; narrow
    // the reader to its channel so the listing names it rightly.
    let (hits, truncated) = reader.messages(&MessageQuery {
        since_ms,
        until_ms,
        thread: Some(&thread_id),
        terms: &options.terms,
        any: common.any,
        mine: common.mine,
        newest_first: false,
        limit: common.limit.unwrap_or(1000),
    })?;
    if common.json {
        #[derive(Serialize)]
        struct Answer {
            thread_id: String,
            hits: Vec<Hit>,
            truncated: bool,
        }
        return serde_json::to_string_pretty(&Answer {
            thread_id,
            hits,
            truncated,
        })
        .context("rendering JSON");
    }
    let header = if windowed {
        format!(
            "Thread {thread_id} · window {} to {} · {} message(s){}\n",
            crate::prompts::et_label(since_ms),
            crate::prompts::et_label(until_ms),
            hits.len(),
            if truncated {
                " (more below; raise --limit)"
            } else {
                ""
            },
        )
    } else {
        format!(
            "Thread {thread_id} (whole cached thread) · {} message(s){}\n",
            hits.len(),
            if truncated {
                " (more below; raise --limit)"
            } else {
                ""
            },
        )
    };
    Ok(format!("{header}{}", render_hits(&hits, common.brief)))
}

/// Run `vibe-talk search` at the current instant.
///
/// # Errors
///
/// As [`search_at`].
pub fn search(options: SearchOptions) -> anyhow::Result<String> {
    search_at(options, crate::ops::now_ms())
}

/// Run `vibe-talk threads` at the current instant.
///
/// # Errors
///
/// As [`threads_at`].
pub fn threads(common: Common) -> anyhow::Result<String> {
    threads_at(common, crate::ops::now_ms())
}

/// Run `vibe-talk thread` at the current instant.
///
/// # Errors
///
/// As [`thread_at`].
pub fn thread(options: ThreadOptions) -> anyhow::Result<String> {
    thread_at(options, crate::ops::now_ms())
}
