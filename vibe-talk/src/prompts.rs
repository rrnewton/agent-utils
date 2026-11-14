//! `vibe-talk prompts`: the owner's own messages over a window, in one go. `#1529 vibe-talk-prompts-command`.
//!
//! Cache-only by design: this reads the local message cache file directly and makes no
//! provider API calls, so it never competes with the live instance for the provider's
//! rate limit. Where the cache may be incomplete for a thread in the window, the output
//! says so and names the thread, rather than fetching upstream.

use std::path::PathBuf;

use anyhow::Context as _;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

use crate::config::Config;

/// Help for `vibe-talk prompts --help`.
pub const USAGE: &str = "\
vibe-talk prompts — the owner's own messages over a window, oldest first

USAGE:
    vibe-talk prompts [--hours N | --since WHEN] [--channel <id|name|#name|label|alias>]
                      [--json] [--config PATH] [--config-overlay PATH]... [--cache PATH]

Prints ONLY the owner's messages (the configured owner user ids), thread replies
included, oldest first, from the local message cache. No server and no provider API
calls are made. Times are shown in ET (America/New_York).

CONFIGURATION DISCOVERY (first match wins):
    --config PATH, then $VIBE_TALK_CONFIG, then the first of
    app.toml / vibe-talk.toml / config.toml under $XDG_CONFIG_HOME/vibe-talk,
    $XDG_CONFIG_HOME/vibe-talk-slack, ~/.config/vibe-talk or ~/.config/vibe-talk-slack,
    then ./vibe-talk.toml. A *-overlay.toml beside the file found layers over it. --config-overlay and
    $VIBE_TALK_CONFIG_OVERLAY layer over the file found, as for the server.
    --cache PATH names the message cache file directly (else the configuration's
    [message_cache] path, else the default beside [storage] path).

OPTIONS:
    --hours N              window of the last N hours, 1 to 168 (default: 24)
    --since WHEN           window from an RFC 3339 instant, or YYYY-MM-DD or
                           \"YYYY-MM-DD HH:MM\" in ET (inclusive)
    --channel WHICH        only this channel: its id, label, alias, or name with or
                           without a leading '#'. Default: every configured channel
    --json                 print structured JSON instead of text
    --help                 print this message and exit

EXAMPLES:
    vibe-talk prompts
    vibe-talk prompts --hours 60
    vibe-talk prompts --channel \"#lead team\" --json
";

/// The opening line of every `prompts` answer.
pub const HEADER: &str = "These are the owner's own messages — the single source of their intent and requests. Check them before saying everything asked is done.";

/// What `prompts` was asked to do.
#[derive(Debug, Default)]
pub struct Options {
    /// `--config`.
    pub config: Option<PathBuf>,
    /// `--config-overlay`, in order.
    pub overlays: Vec<PathBuf>,
    /// `--cache`.
    pub cache: Option<PathBuf>,
    /// `--hours`.
    pub hours: Option<u64>,
    /// `--since`, as written.
    pub since: Option<String>,
    /// `--channel`, as written.
    pub channel: Option<String>,
    /// `--json`.
    pub json: bool,
}

/// Parse the arguments after `prompts`. `Ok(None)` means `--help` was printed.
///
/// # Errors
///
/// An unknown argument, a flag without its value, `--hours` with `--since`, or a bad number.
pub fn parse(args: impl IntoIterator<Item = String>) -> anyhow::Result<Option<Options>> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "--config" => {
                options.config = Some(args.next().context("--config needs a path")?.into())
            }
            "--config-overlay" => options
                .overlays
                .push(args.next().context("--config-overlay needs a path")?.into()),
            "--cache" => options.cache = Some(args.next().context("--cache needs a path")?.into()),
            "--hours" => {
                let value = args.next().context("--hours needs a number")?;
                options.hours = Some(
                    value
                        .parse::<u64>()
                        .with_context(|| format!("--hours takes whole hours, not {value:?}"))?,
                );
            }
            "--since" => options.since = Some(args.next().context("--since needs a value")?),
            "--channel" => options.channel = Some(args.next().context("--channel needs a value")?),
            "--json" => options.json = true,
            other => anyhow::bail!("unrecognized argument {other:?}\n\n{USAGE}"),
        }
    }
    if options.hours.is_some() && options.since.is_some() {
        anyhow::bail!("--hours and --since name two windows; give one\n\n{USAGE}");
    }
    if let Some(hours) = options.hours {
        if !(1..=168).contains(&hours) {
            anyhow::bail!("--hours must be between 1 and 168, not {hours}");
        }
    }
    Ok(Some(options))
}

/// Whether `want` names `channel`: its id, label, display name, or alias, with any
/// leading '#' on `want` ignored. Shared channel-name matching for the CLI readers.
#[must_use]
pub fn channel_matches(want: &str, channel: &crate::model::ChannelInfo) -> bool {
    let want = want.trim().trim_start_matches('#').trim();
    channel.id.as_str().eq_ignore_ascii_case(want)
        || channel.display_name().eq_ignore_ascii_case(want)
        || channel.label.eq_ignore_ascii_case(want)
        || channel
            .alias
            .as_deref()
            .is_some_and(|alias| alias.eq_ignore_ascii_case(want))
}

/// The configuration directory candidates, in discovery order.
fn config_candidates() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        dirs.push(PathBuf::from(xdg.clone()).join("vibe-talk"));
        dirs.push(PathBuf::from(xdg).join("vibe-talk-slack"));
    }
    if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        dirs.push(PathBuf::from(&home).join(".config").join("vibe-talk"));
        dirs.push(PathBuf::from(home).join(".config").join("vibe-talk-slack"));
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        for name in ["app.toml", "vibe-talk.toml", "config.toml"] {
            out.push(dir.join(name));
        }
    }
    out.push(PathBuf::from("vibe-talk.toml"));
    out
}

/// Load the configuration the way `prompts` finds it: an explicit flag or environment
/// variable wins, else the documented discovery order under the user's config dir.
///
/// # Errors
///
/// Nothing found (the error names every path looked for), or the file found is unusable.
pub fn discover_config(options: &Options) -> anyhow::Result<Config> {
    if options.config.is_some() || std::env::var_os(crate::config::ENV_CONFIG_PATH).is_some() {
        return Config::discover(options.config.clone(), options.overlays.clone())
            .context("loading the configuration named by flag or environment");
    }
    let candidates = config_candidates();
    for candidate in &candidates {
        if candidate.exists() {
            let mut overlays = options.overlays.clone();
            if let Some(dir) = candidate.parent() {
                let mut beside: Vec<PathBuf> = std::fs::read_dir(dir)
                    .map(|entries| {
                        entries
                            .filter_map(Result::ok)
                            .map(|entry| entry.path())
                            .filter(|path| {
                                path.file_name()
                                    .and_then(|name| name.to_str())
                                    .is_some_and(|name| name.ends_with("-overlay.toml"))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                beside.sort();
                overlays.extend(beside.into_iter().take(1));
            }
            return Config::discover(Some(candidate.clone()), overlays)
                .with_context(|| format!("loading configuration from {}", candidate.display()));
        }
    }
    anyhow::bail!(
        "no vibe-talk configuration found; looked for: {}. Pass --config PATH or set {}",
        candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
        crate::config::ENV_CONFIG_PATH
    )
}

/// The owner's user ids for `config`: `[owner] user_ids` plus each provider's owner id.
#[must_use]
pub fn owner_ids(config: &Config) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let push = |id: &str, ids: &mut Vec<String>| {
        let id = id.trim().to_owned();
        if !id.is_empty() && !ids.contains(&id) {
            ids.push(id);
        }
    };
    for id in &config.owner.user_ids {
        push(id, &mut ids);
    }
    for provider in &config.providers {
        if let Some(id) = provider.owner_user_id() {
            push(id, &mut ids);
        }
    }
    ids
}

/// One owner message from the cache.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PromptMessage {
    /// Provider channel id.
    pub channel_id: String,
    /// Configured channel label, when the channel is configured.
    pub channel: Option<String>,
    /// Provider message id.
    pub message_id: String,
    /// Creation instant, Unix ms.
    pub created_ms: i64,
    /// ET time label, e.g. `2026-11-12 14:24 ET`.
    pub et: String,
    /// Author display name as cached.
    pub author: String,
    /// Message text.
    pub content: String,
    /// Thread id, when the message is in a thread.
    pub thread_id: Option<String>,
    /// Whether the message is its thread's root.
    pub is_root: bool,
    /// Whether the cached text may be partial.
    pub content_partial: bool,
}

/// The answer: header, the messages, and any cache-completeness warnings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PromptsAnswer {
    /// [`HEADER`].
    pub header: String,
    /// The window start, Unix ms.
    pub since_ms: i64,
    /// The window end, Unix ms.
    pub until_ms: i64,
    /// The owner's messages, oldest first.
    pub messages: Vec<PromptMessage>,
    /// Named threads/channels whose cached rows may be incomplete in the window.
    pub incomplete: Vec<String>,
}

/// An instant as an ET wall-time label.
#[must_use]
pub fn et_label(ms: i64) -> String {
    crate::recap::et_label(ms)
}

/// Parse `--since` as an instant: RFC 3339, or `YYYY-MM-DD[ HH:MM]` in ET.
///
/// # Errors
///
/// The value is neither form.
pub fn parse_since(raw: &str) -> anyhow::Result<i64> {
    let raw = raw.trim();
    if let Ok(instant) = raw.parse::<jiff::Timestamp>() {
        return Ok(instant.as_millisecond());
    }
    // Reuse the recap window parser's ET wall-time rule via a date-only probe.
    let (date, time) = match raw.split_once([' ', 'T']) {
        Some((date, time)) => (date, Some(time)),
        None => (raw, None),
    };
    let mut parts = date.split('-');
    let year: i32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let month: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let day: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    if year == 0 || parts.next().is_some() {
        anyhow::bail!("--since {raw:?} is not an RFC 3339 instant or an ET date/time");
    }
    let (hour, minute) = match time {
        None => (0, 0),
        Some(time) => {
            let mut t = time.split(':');
            (
                t.next().and_then(|p| p.parse().ok()).unwrap_or(0),
                t.next().and_then(|p| p.parse().ok()).unwrap_or(0),
            )
        }
    };
    let zone = jiff::tz::TimeZone::get("America/New_York").context("loading the ET zone")?;
    let date = jiff::civil::Date::new(
        i16::try_from(year).map_err(|_| anyhow::anyhow!("--since year out of range"))?,
        i8::try_from(month).map_err(|_| anyhow::anyhow!("--since month out of range"))?,
        i8::try_from(day).map_err(|_| anyhow::anyhow!("--since day out of range"))?,
    )
    .map_err(|_| anyhow::anyhow!("--since {raw:?} is not a date"))?;
    let time = jiff::civil::Time::new(
        i8::try_from(hour).map_err(|_| anyhow::anyhow!("--since hour out of range"))?,
        i8::try_from(minute).map_err(|_| anyhow::anyhow!("--since minute out of range"))?,
        0,
        0,
    )
    .map_err(|_| anyhow::anyhow!("--since {raw:?} is not a time"))?;
    Ok(date
        .to_datetime(time)
        .to_zoned(zone)
        .map_err(|_| anyhow::anyhow!("--since {raw:?} has no ET instant"))?
        .timestamp()
        .as_millisecond())
}

/// The cache file `prompts` reads: `--cache`, else the configuration's cache path,
/// else the default beside the store.
fn cache_path(config: &Config, options: &Options) -> anyhow::Result<PathBuf> {
    if let Some(path) = &options.cache {
        return Ok(path.clone());
    }
    if let Some(cache) = &config.message_cache {
        if let Some(path) = &cache.path {
            return Ok(path.clone());
        }
    }
    let store = config
        .storage
        .path
        .as_ref()
        .context("the configuration names no [storage] path, so no default cache file exists; pass --cache PATH")?;
    Ok(crate::cache::file::default_path(
        store,
        &crate::cache::file::host_name(),
    ))
}

/// Read the owner's messages in `[since_ms, until_ms)` from the cache at `cache_file`.
///
/// Cache-only: the file is opened read-only and no provider is contacted. `wanted_channel`
/// is matched with [`channel_matches`] against the configured channels first; a channel
/// present in the cache but not configured still answers to its id.
///
/// # Errors
///
/// The cache cannot be opened or read, or no owner user id is configured.
pub fn read_at(
    config: &Config,
    cache_file: &std::path::Path,
    wanted_channel: Option<&str>,
    since_ms: i64,
    until_ms: i64,
) -> anyhow::Result<PromptsAnswer> {
    let owners = owner_ids(config);
    if owners.is_empty() {
        anyhow::bail!(
            "the configuration names no owner: set [owner] user_ids or a provider owner_user_id"
        );
    }
    let selected: Option<Vec<String>> = wanted_channel.map(|want| {
        config
            .channels
            .iter()
            .filter(|channel| channel_matches(want, channel))
            .map(|channel| channel.id.as_str().to_owned())
            .collect()
    });
    if let Some(ids) = &selected {
        if ids.is_empty() {
            // Fall back to the id/name as written: it may name a cached channel by id.
            // An unknown name matches nothing in the query below and answers empty.
        }
    }
    let connection = Connection::open_with_flags(
        cache_file,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("cannot open cache {} read-only", cache_file.display()))?;
    let has_dropped_ms: bool = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('channel') WHERE name = 'dropped_ms'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false);
    let placeholders = owners.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let channel_filter = match &selected {
        Some(ids) if !ids.is_empty() => ids.clone(),
        _ => Vec::new(),
    };
    let mut sql = format!(
        "SELECT c.channel_id, m.message_id, m.created_ms, m.author, m.content, m.thread_id, \
         m.is_root, m.content_partial FROM message m JOIN channel c ON c.chan = m.chan \
         WHERE m.author_id IN ({placeholders}) AND m.created_ms >= ? AND m.created_ms < ?"
    );
    if !channel_filter.is_empty() {
        let marks = channel_filter
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        sql.push_str(&format!(" AND c.channel_id IN ({marks})"));
    } else if let Some(want) = wanted_channel {
        let _ = want;
        sql.push_str(" AND 1 = 0");
        if selected.as_ref().is_some_and(|ids| ids.is_empty()) {
            // Named channel not configured: still allow an exact id/name match in the cache.
            sql = sql.replace(" AND 1 = 0", " AND c.channel_id = ?");
        }
    }
    sql.push_str(" ORDER BY m.created_ms, m.numeric_id");
    let mut statement = connection
        .prepare(&sql)
        .context("preparing the cache query")?;
    let mut params: Vec<rusqlite::types::Value> = owners
        .iter()
        .cloned()
        .map(rusqlite::types::Value::from)
        .collect();
    params.push(since_ms.into());
    params.push(until_ms.into());
    if !channel_filter.is_empty() {
        params.extend(
            channel_filter
                .iter()
                .cloned()
                .map(rusqlite::types::Value::from),
        );
    } else if selected.as_ref().is_some_and(|ids| ids.is_empty()) {
        if let Some(want) = wanted_channel {
            params.push(want.trim().trim_start_matches('#').trim().to_owned().into());
        }
    }
    let rows = statement
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })
        .context("reading the cache")?;
    let labels: std::collections::HashMap<String, String> = config
        .channels
        .iter()
        .map(|channel| {
            (
                channel.id.as_str().to_owned(),
                channel.display_name().to_owned(),
            )
        })
        .collect();
    let mut messages: Vec<PromptMessage> = Vec::new();
    for row in rows {
        let (channel_id, message_id, created_ms, author, content, thread_id, is_root, partial) =
            row.context("reading a cached message")?;
        messages.push(PromptMessage {
            channel: labels.get(&channel_id).cloned(),
            channel_id,
            message_id,
            created_ms,
            et: et_label(created_ms),
            author,
            content,
            thread_id,
            is_root: is_root != 0,
            content_partial: partial != 0,
        });
    }
    // Completeness: a thread whose recorded reply count exceeds its cached replies in
    // the window, partial rows, and channels with a dropped update inside the window.
    let mut incomplete: Vec<String> = Vec::new();
    let thread_ids: Vec<String> = {
        let mut seen: Vec<String> = Vec::new();
        for message in &messages {
            if let Some(thread) = &message.thread_id {
                if !seen.contains(thread) {
                    seen.push(thread.clone());
                }
            }
        }
        seen
    };
    for thread_id in &thread_ids {
        let cached: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM message WHERE thread_id = ? AND is_root = 0",
                [thread_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let recorded: Option<i64> = connection
            .query_row(
                "SELECT reply_count FROM thread WHERE thread_id = ?",
                [thread_id],
                |row| row.get(0),
            )
            .unwrap_or(None);
        if let Some(recorded) = recorded {
            if recorded > cached {
                incomplete.push(format!(
                    "thread {thread_id}: cache holds {cached} of {recorded} replies"
                ));
            }
        }
    }
    for message in &messages {
        if message.content_partial {
            incomplete.push(format!(
                "message {} in thread {}: cached text is partial",
                message.message_id,
                message.thread_id.as_deref().unwrap_or("-")
            ));
        }
    }
    if has_dropped_ms {
        let dropped: Vec<(String, i64)> = connection
            .prepare(
                "SELECT channel_id, dropped_ms FROM channel WHERE dropped_ms IS NOT NULL \
                 AND dropped_ms >= ? AND dropped_ms < ?",
            )
            .and_then(|mut stmt| {
                let rows = stmt.query_map([since_ms], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?;
                Ok(rows.filter_map(Result::ok).collect::<Vec<_>>())
            })
            .unwrap_or_default();
        for (channel_id, dropped_ms) in dropped {
            if dropped_ms < until_ms {
                incomplete.push(format!(
                    "channel {channel_id}: cache had a dropped update at {} (rows before it may be stale)",
                    et_label(dropped_ms)
                ));
            }
        }
    }
    incomplete.sort();
    incomplete.dedup();
    Ok(PromptsAnswer {
        header: HEADER.to_owned(),
        since_ms,
        until_ms,
        messages,
        incomplete,
    })
}

/// Render the answer as readable text.
#[must_use]
pub fn render(answer: &PromptsAnswer) -> String {
    let mut out = String::new();
    out.push_str(&answer.header);
    out.push_str(&format!(
        "\nWindow: {} to {} · {} owner message(s)\n",
        et_label(answer.since_ms),
        et_label(answer.until_ms),
        answer.messages.len()
    ));
    for message in &answer.messages {
        let channel = message.channel.as_deref().unwrap_or(&message.channel_id);
        let thread = match &message.thread_id {
            Some(thread) if message.is_root => format!(" · thread {thread} (root)"),
            Some(thread) => format!(" · thread {thread}"),
            None => String::new(),
        };
        let partial = if message.content_partial {
            " [partial]"
        } else {
            ""
        };
        out.push_str(&format!(
            "- {} · #{} · {} · {}{}{}\n  {}\n",
            message.et,
            channel,
            message.author,
            message.message_id,
            thread,
            partial,
            message.content.trim().replace('\n', "\n  ")
        ));
    }
    for note in &answer.incomplete {
        out.push_str(&format!("Cache may be incomplete: {note}.\n"));
    }
    out
}

/// Run `vibe-talk prompts` at instant `now_ms` and return what to print.
///
/// # Errors
///
/// Discovery, cache, or window errors as in [`discover_config`] and [`read_at`].
pub fn run_at(options: Options, now_ms: i64) -> anyhow::Result<String> {
    let config = discover_config(&options)?;
    let until_ms = now_ms;
    let since_ms = match &options.since {
        Some(raw) => parse_since(raw)?,
        None => {
            let hours = options.hours.unwrap_or(24);
            until_ms - i64::try_from(hours).unwrap_or(24) * 3_600_000
        }
    };
    if since_ms >= until_ms {
        anyhow::bail!("the window is empty: --since is not before now");
    }
    let cache_file = cache_path(&config, &options)?;
    let answer = read_at(
        &config,
        &cache_file,
        options.channel.as_deref(),
        since_ms,
        until_ms,
    )?;
    if options.json {
        return serde_json::to_string_pretty(&answer).context("rendering JSON");
    }
    Ok(render(&answer))
}

/// Run `vibe-talk prompts` and return what to print.
///
/// # Errors
///
/// As [`run_at`].
pub fn run(options: Options) -> anyhow::Result<String> {
    run_at(options, crate::ops::now_ms())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_matching_ignores_a_leading_hash() {
        let channel = crate::model::ChannelInfo {
            id: crate::model::ChannelId("C123".to_owned()),
            label: "lead team".to_owned(),
            writable: true,
            alias: None,
            added: false,
            provider: None,
            write_source: None,
        };
        assert!(channel_matches("#lead team", &channel));
        assert!(channel_matches("lead team", &channel));
        assert!(channel_matches("C123", &channel));
        assert!(!channel_matches("#other", &channel));
    }

    #[test]
    fn since_parses_et_wall_times() {
        // 2026-11-12 09:00 ET (EST, UTC-5) is 14:00 UTC.
        let ms = parse_since("2026-11-12 09:00").expect("since");
        assert_eq!(crate::clock::iso_from_ms(ms), "2026-11-12T14:00:00Z");
    }
}
