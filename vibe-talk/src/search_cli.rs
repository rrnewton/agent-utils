//! `vibe-talk search`: search the conversation the server holds and print linkable hits.
//! `#1338 vibe-talk-search-cli`.
//!
//! The client half of the server search (`#605 server-side-search`): it talks to a running
//! server's `GET /api/v1/search` (or one channel's `/search`) with the READ token and prints
//! one line per hit — ET time, author, snippet, the message's own permalink in its service
//! (`upstream_url`), and its vibe-talk deep link — newest first, so an agent asked to "find
//! where we discussed X" can answer with links instead of reconstructions. The scope lines
//! ride along: which channels were searched, how far back each reached, and which were not
//! searched and why, so "not in the cached window" is said out loud rather than implied by
//! an empty answer. Search covers only what the server's message cache holds; until the
//! retention decision lands, that is a recent window, and the footer says so whenever a
//! channel's reach is not its whole history.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context as _;
use serde::Deserialize;

use crate::config::Config;
use crate::model::{ChannelInfo, Message};

/// Help for `vibe-talk search --help`.
pub const USAGE: &str = "\
vibe-talk search — search the chat history the server holds, with links

USAGE:
    vibe-talk search \"<query>\" [--channel <id|label|alias>] [--since <date>]
                     [--limit N] [--json]
                     [--config PATH] [--config-overlay PATH]...
                     [--server URL] [--base-url URL] [--token-file PATH]

Searches what the running server's message cache holds, in the server search language
(the same terms, phrases, from:/in:/before:/after:/has: as the page's search), and prints
one line per hit, newest first: ET time · author · snippet · the message's permalink in
its own service · its vibe-talk deep link. The token is the READ token: from --token-file
when given, else the server's own configuration (the file, then VIBE_TALK_READ_TOKEN,
which wins); it is never printed. The server is reached on the configuration's [server]
bind address; 0.0.0.0 and [::] are reached on loopback. Deep links are built on --base-url,
else the configuration's [server] public_url, else the address the server was reached at —
never a hard-coded host.

Only the cached window is searched: the lines after the hits say how far back each
channel was searched, name any channel that was not searched and why, and note when the
cache does not yet hold a channel's whole history. No hits means no match in that window,
not no match ever.

OPTIONS:
    --channel WHICH        only this channel: its id, label, or alias. Default: every
                           channel the read token sees
    --since WHEN           only messages on or after this day: YYYY-MM-DD, YYYY/MM/DD or
                           MM/DD/YYYY, an RFC 3339 instant, or \"YYYY-MM-DD HH:MM\" in ET.
                           Folded into the query as its after: term
    --limit N              hits at most (default: 50; the server allows 1 to 200)
    --json                 print the server's answer as structured JSON instead of lines
    --config PATH          the server's configuration file, found exactly as the server
                           finds it (default: $VIBE_TALK_CONFIG, else ./vibe-talk.toml;
                           with neither, the configuration comes from the environment
                           alone). Not needed when --server and --token-file are both given
    --config-overlay PATH  a TOML file laid over the configuration, as for the server.
                           Repeatable; VIBE_TALK_CONFIG_OVERLAY=PATH[:PATH...] is applied
                           after the flags
    --server URL           reach the server at this http(s) address instead of the
                           configuration's bind address (for a test server)
    --base-url URL         build deep links on this address instead of the configuration's
                           [server] public_url
    --token-file PATH      read the READ token from this file (its whole contents, trimmed)
                           instead of the configuration
    --help                 print this message and exit

EXAMPLES:
    vibe-talk search \"deploy freeze\"
    vibe-talk search \"runner offline\" --channel \"lead team\" --since 2026-10-30
    vibe-talk search \"from:ada has:link\" --limit 20 --json
";

/// What `search` was asked to do.
#[derive(Debug, Default)]
pub struct Options {
    /// The query, in the server search language.
    pub query: String,
    /// `--config`.
    pub config: Option<PathBuf>,
    /// `--config-overlay`, in order.
    pub overlays: Vec<PathBuf>,
    /// `--server`.
    pub server: Option<String>,
    /// `--base-url`.
    pub base_url: Option<String>,
    /// `--token-file`.
    pub token_file: Option<PathBuf>,
    /// `--channel`, as written.
    pub channel: Option<String>,
    /// `--since`, as written.
    pub since: Option<String>,
    /// `--limit`.
    pub limit: Option<usize>,
    /// `--json`.
    pub json: bool,
}

/// Parse the arguments after `search`. `Ok(None)` means `--help` was printed.
///
/// # Errors
///
/// No query, an unknown argument, a flag without its value, or a `--limit` that does not
/// parse or is out of range.
pub fn parse(args: impl IntoIterator<Item = String>) -> anyhow::Result<Option<Options>> {
    let mut options = Options::default();
    let mut words: Vec<String> = Vec::new();
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
            "--server" => options.server = Some(args.next().context("--server needs a URL")?),
            "--base-url" => options.base_url = Some(args.next().context("--base-url needs a URL")?),
            "--token-file" => {
                options.token_file = Some(args.next().context("--token-file needs a path")?.into())
            }
            "--channel" => options.channel = Some(args.next().context("--channel needs a value")?),
            "--since" => options.since = Some(args.next().context("--since needs a value")?),
            "--limit" => {
                let value = args.next().context("--limit needs a number")?;
                let limit = value
                    .parse::<usize>()
                    .with_context(|| format!("--limit takes a hit count, not {value:?}"))?;
                if !(1..=200).contains(&limit) {
                    anyhow::bail!("--limit must be between 1 and 200, not {limit}");
                }
                options.limit = Some(limit);
            }
            "--json" => options.json = true,
            other if other.starts_with("--") => {
                anyhow::bail!("unrecognized argument {other:?}\n\n{USAGE}")
            }
            other => words.push(other.to_owned()),
        }
    }
    options.query = words.join(" ").trim().to_owned();
    if options.query.is_empty() {
        anyhow::bail!("a query is required\n\n{USAGE}");
    }
    Ok(Some(options))
}

/// The query as sent: `--since` folded in as the language's `after:` term (ET day).
///
/// # Errors
///
/// `--since` names no day this command can read.
fn folded_query(options: &Options) -> anyhow::Result<String> {
    let Some(since) = &options.since else {
        return Ok(options.query.clone());
    };
    let day = since_day(since)
        .with_context(|| format!("--since {since:?} is not a date this command can read"))?;
    Ok(format!("{} after:{day}", options.query))
}

/// The ET calendar day `--since` names, as `YYYY-MM-DD`.
///
/// Accepts the search language's own day forms, an RFC 3339 instant, and `YYYY-MM-DD HH:MM`
/// in ET (its day). Instants are read in ET, the zone the hits are printed in.
fn since_day(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if let Some(day) = crate::search_query::Day::parse(raw) {
        return Some(day.to_string());
    }
    if let Some((date, _time)) = raw.split_once([' ', 'T']) {
        // `YYYY-MM-DD HH:MM` in ET names its own day; an RFC 3339 instant names one in ET.
        if raw.contains(['+', 'Z']) || raw.matches(':').count() >= 2 && !raw.contains(' ') {
            if let Ok(instant) = raw.parse::<jiff::Timestamp>() {
                let zone =
                    jiff::tz::TimeZone::get("America/New_York").unwrap_or(jiff::tz::TimeZone::UTC);
                return Some(instant.to_zoned(zone).strftime("%Y-%m-%d").to_string());
            }
        }
        if let Some(day) = crate::search_query::Day::parse(date) {
            return Some(day.to_string());
        }
    }
    if let Ok(instant) = raw.parse::<jiff::Timestamp>() {
        let zone = jiff::tz::TimeZone::get("America/New_York").unwrap_or(jiff::tz::TimeZone::UTC);
        return Some(instant.to_zoned(zone).strftime("%Y-%m-%d").to_string());
    }
    None
}

/// Percent-encode one URL part (a query value, or one part of a deep link's `m`).
pub(crate) fn encoded(part: &str) -> String {
    let mut out = String::with_capacity(part.len());
    for byte in part.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// A message's vibe-talk deep link (`#462 webapp-deep-links`): the page's address naming its
/// channel and focusing the message, `?c=<channel>&v=main&m=<provider>:<channel>:<message>`
/// with the thread appended for a reply. Built on `link_base`, the configured public base —
/// never a hard-coded host. Without a known provider the link names the channel only.
#[must_use]
pub fn deep_link(link_base: &str, channel: &ChannelInfo, message: &Message) -> String {
    let base = link_base.trim_end_matches('/');
    let channel_id = message.channel_id.as_str();
    let place = format!("{base}/?c={}&v=main", encoded(channel_id));
    let Some(provider) = channel.provider.as_deref().filter(|p| !p.is_empty()) else {
        return place;
    };
    let mut parts = vec![
        encoded(provider),
        encoded(channel_id),
        encoded(message.id.as_str()),
    ];
    if let Some(thread) = &message.thread {
        if !thread.is_root {
            parts.push(encoded(&thread.id));
        }
    }
    format!("{place}&m={}", encoded(&parts.join(":")))
}

/// One hit as one line: ET time, author, snippet, permalink, deep link. Shared with the
/// generated tool subcommands (`crate::tool_cli`), which print answers the same way.
pub(crate) fn line_for(link_base: &str, channels: &[ChannelInfo], message: &Message) -> String {
    let channel = channels
        .iter()
        .find(|c| c.id == message.channel_id)
        .cloned()
        .unwrap_or_else(|| ChannelInfo {
            id: message.channel_id.clone(),
            label: message.channel_id.as_str().to_owned(),
            writable: false,
            alias: None,
            added: false,
            provider: None,
            write_source: None,
        });
    let when = message
        .created_ms()
        .map_or_else(|| message.timestamp.clone(), crate::recap::et_label);
    let snippet = {
        let first = message.content.lines().next().unwrap_or("").trim();
        let cut: String = first.chars().take(160).collect();
        if first.chars().count() > 160 || message.content.lines().count() > 1 {
            format!("{cut} …")
        } else {
            cut
        }
    };
    let permalink = message
        .upstream_url
        .as_deref()
        .map_or_else(|| format!("#{}", message.id.as_str()), str::to_owned);
    format!(
        "{when} · {} · {snippet} · {permalink} · {}",
        message.author,
        deep_link(link_base, &channel, message)
    )
}

/// One channel's part of the answer, as the server sends it.
#[derive(Debug, Deserialize)]
struct ScopeAnswer {
    channel: ChannelInfo,
    state: String,
    reason: Option<String>,
    searched_back_to_ms: Option<i64>,
    held_from_ms: Option<i64>,
    whole: bool,
    expanding: bool,
    capped: u64,
    partial: u64,
    withheld: u64,
}

/// The server's search answer, as this client reads it.
#[derive(Debug, Deserialize)]
struct SearchAnswer {
    hits: Vec<Message>,
    scopes: Vec<ScopeAnswer>,
    resume_below_ms: Option<i64>,
}

/// What the search covered, as lines after the hits: per channel, how far back it reached or
/// why it was not searched; then the cached-window note whenever any reach is partial.
fn coverage_lines(answer: &SearchAnswer) -> Vec<String> {
    let mut lines = Vec::new();
    let mut partial_window = false;
    for scope in &answer.scopes {
        let name = scope.channel.display_name();
        if scope.state != "searched" {
            partial_window = true;
            lines.push(format!(
                "not searched: {name} ({})",
                scope.reason.as_deref().unwrap_or("not_searched")
            ));
            continue;
        }
        let back = scope
            .searched_back_to_ms
            .map_or_else(|| "the oldest held".to_owned(), crate::recap::et_label);
        let mut line = format!("searched: {name} back to {back}");
        if scope.whole {
            line.push_str(" (whole history held)");
        } else {
            partial_window = true;
            if scope.expanding {
                line.push_str(" (history still being gathered)");
            }
        }
        if scope.capped > 0 {
            line.push_str(&format!("; {} hit the text cap", scope.capped));
        }
        if scope.partial > 0 {
            line.push_str(&format!("; {} snippet-only withheld", scope.partial));
        }
        if scope.withheld > 0 {
            line.push_str(&format!("; {} withheld (thread in doubt)", scope.withheld));
        }
        lines.push(line);
    }
    if answer.resume_below_ms.is_some() {
        partial_window = true;
        lines.push("more hits below; raise --limit or narrow the query".to_owned());
    }
    if partial_window {
        lines.push(
            "note: search covers only what the server's cache holds (a recent window until \
             full-history retention lands); absence here is not absence ever"
                .to_owned(),
        );
    }
    lines
}

/// Render the answer as printed lines: hits newest first, then the coverage lines. With no
/// hits, the coverage lines still print, headed by the window's own verdict.
#[must_use]
fn render(link_base: &str, answer: &SearchAnswer) -> String {
    let channels: Vec<ChannelInfo> = answer
        .scopes
        .iter()
        .map(|scope| scope.channel.clone())
        .collect();
    let mut out = String::new();
    if answer.hits.is_empty() {
        out.push_str("No hits in the window searched.\n");
    }
    for hit in &answer.hits {
        out.push_str(&line_for(link_base, &channels, hit));
        out.push('\n');
    }
    for line in coverage_lines(answer) {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// The server's address: `--server`, else the configuration's bind address on this host.
fn server_base(options: &Options, config: Option<&Config>) -> anyhow::Result<String> {
    if let Some(server) = &options.server {
        let parsed =
            url::Url::parse(server).with_context(|| format!("--server {server:?} is not a URL"))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            anyhow::bail!("--server {server:?} must be an http:// or https:// address");
        }
        return Ok(server.trim_end_matches('/').to_owned());
    }
    let config = config.context(
        "no server address: give --server, or run with the server's --config so its bind \
         address can be read",
    )?;
    let local: SocketAddr = crate::pair_link::local_address(config.bind);
    if !crate::pair_link::is_this_host(local.ip()) {
        anyhow::bail!(
            "the configuration's bind address {} is not an address of this host, and the read \
             token is sent only to this host; run search where the server runs, or pass --server",
            local.ip()
        );
    }
    Ok(format!("http://{local}"))
}

/// One failed HTTP answer, in the server's own words where it gave them. A 401 or 403 names
/// the credential, so an agent can tell an auth failure from an empty result.
async fn read_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    what: &str,
) -> anyhow::Result<T> {
    let status = response.status();
    let body = response.bytes().await.context("reading the answer")?;
    if !status.is_success() {
        #[derive(serde::Deserialize)]
        struct Refusal {
            error: String,
            detail: String,
        }
        let detail = serde_json::from_slice::<Refusal>(&body)
            .map(|error| format!("{}: {}", error.error, error.detail))
            .unwrap_or_else(|_| "no detail".to_owned());
        if status.as_u16() == 401 || status.as_u16() == 403 {
            anyhow::bail!(
                "{what} refused ({status}): {detail} — check the READ token (config, \
                 VIBE_TALK_READ_TOKEN, or --token-file)"
            );
        }
        anyhow::bail!("{what} failed ({status}): {detail}");
    }
    serde_json::from_slice(&body).with_context(|| format!("{what} answered unexpectedly"))
}

/// Run `vibe-talk search` and return what to print.
///
/// # Errors
///
/// The configuration cannot be loaded, the token file cannot be read, `--since` names no
/// readable day, the server cannot be reached, or it refuses (an auth refusal says so).
pub async fn run(options: Options) -> anyhow::Result<String> {
    let query = folded_query(&options)?;
    // The configuration is needed for the token and the addresses, unless the caller named
    // both the server and a token file.
    let need_config =
        options.server.is_none() || options.token_file.is_none() || options.base_url.is_none();
    let config = if need_config {
        Some(
            Config::discover(options.config.clone(), options.overlays.clone()).context(
                "search reads the server's configuration for its read token and addresses, as \
             the server does: run it with the server's --config and environment",
            )?,
        )
    } else {
        None
    };
    let token = match &options.token_file {
        Some(path) => std::fs::read_to_string(path)
            .with_context(|| format!("reading the token file {}", path.display()))?
            .trim()
            .to_owned(),
        None => config
            .as_ref()
            .context("no READ token: pass --token-file, or run with the server's --config")?
            .auth
            .read_token
            .expose()
            .to_owned(),
    };
    if token.is_empty() {
        anyhow::bail!("the READ token is empty; check the token file or configuration");
    }
    let base = server_base(&options, config.as_ref())?;
    let link_base = options
        .base_url
        .clone()
        .or_else(|| config.as_ref().and_then(|c| c.public_base_url.clone()))
        .unwrap_or_else(|| base.clone());
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .build()
        .context("building the HTTP client")?;
    // The channel list answers --channel and names each hit's provider for its deep link.
    let channels: Vec<ChannelInfo> = read_json(
        client
            .get(format!("{base}/api/v1/channels"))
            .bearer_auth(&token)
            .send()
            .await
            .context("could not reach the vibe-talk server; is it running?")?,
        "listing channels",
    )
    .await
    .and_then(|body: serde_json::Value| {
        serde_json::from_value(body["channels"].clone()).context("listing channels")
    })?;
    if channels.is_empty() {
        anyhow::bail!("the server lists no channels for this read token");
    }
    let limit = options.limit.unwrap_or(50).to_string();
    let answer: SearchAnswer = match &options.channel {
        Some(want) => {
            let channel = channels
                .iter()
                .find(|channel| {
                    channel.id.as_str().eq_ignore_ascii_case(want)
                        || channel.display_name().eq_ignore_ascii_case(want)
                        || channel.label.eq_ignore_ascii_case(want)
                })
                .with_context(|| {
                    format!(
                        "no channel matches {want:?}; the read token sees: {}",
                        channels
                            .iter()
                            .map(|c| format!("{} ({})", c.display_name(), c.id))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?;
            read_json(
                client
                    .get(format!("{base}/api/v1/channels/{}/search", channel.id))
                    .query(&[
                        ("q", query.as_str()),
                        ("limit", limit.as_str()),
                        ("tz", "America/New_York"),
                    ])
                    .bearer_auth(&token)
                    .send()
                    .await
                    .with_context(|| format!("searching {}", channel.display_name()))?,
                "the search",
            )
            .await?
        }
        None => {
            read_json(
                client
                    .get(format!("{base}/api/v1/search"))
                    .query(&[
                        ("q", query.as_str()),
                        ("limit", limit.as_str()),
                        ("tz", "America/New_York"),
                    ])
                    .bearer_auth(&token)
                    .send()
                    .await
                    .context("searching every channel")?,
                "the search",
            )
            .await?
        }
    };
    if options.json {
        return serde_json::to_string_pretty(&serde_json::json!({
            "hits": answer.hits,
            "scopes": answer.scopes.iter().map(|scope| serde_json::json!({
                "channel": scope.channel,
                "state": scope.state,
                "reason": scope.reason,
                "searched_back_to_ms": scope.searched_back_to_ms,
                "held_from_ms": scope.held_from_ms,
                "whole": scope.whole,
                "expanding": scope.expanding,
                "capped": scope.capped,
                "partial": scope.partial,
                "withheld": scope.withheld,
            })).collect::<Vec<_>>(),
            "resume_below_ms": answer.resume_below_ms,
        }))
        .context("rendering JSON");
    }
    Ok(render(&link_base, &answer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChannelId, MessageId, UserId};

    fn message(id: &str, channel: &str, content: &str) -> Message {
        Message {
            edited_timestamp: None,
            thread: None,
            id: MessageId(id.to_owned()),
            channel_id: ChannelId(channel.to_owned()),
            author: "Ada".to_owned(),
            author_id: UserId("U1".to_owned()),
            author_is_bot: false,
            timestamp: "2026-11-11T17:00:00Z".to_owned(),
            spoken_time: String::new(),
            reply_to: None,
            content: content.to_owned(),
            spoken_content: String::new(),
            content_html: String::new(),
            noise: false,
            reactions: None,
            heading: None,
            mentions: Vec::new(),
            upstream_url: Some("https://example.slack.com/archives/C1/p1".to_owned()),
            upstream_app_url: None,
            op_key: None,
            observed_ms: None,
        }
    }

    fn channel(id: &str, provider: Option<&str>) -> ChannelInfo {
        ChannelInfo {
            id: ChannelId(id.to_owned()),
            label: id.to_owned(),
            writable: false,
            alias: None,
            added: false,
            provider: provider.map(str::to_owned),
            write_source: None,
        }
    }

    #[test]
    fn parse_joins_the_query_and_reads_the_flags() {
        let options = parse([
            "deploy".to_owned(),
            "freeze".to_owned(),
            "--channel".to_owned(),
            "lead team".to_owned(),
            "--since".to_owned(),
            "2026-10-30".to_owned(),
            "--limit".to_owned(),
            "20".to_owned(),
            "--json".to_owned(),
        ])
        .expect("parses")
        .expect("no --help");
        assert_eq!(options.query, "deploy freeze");
        assert_eq!(options.channel.as_deref(), Some("lead team"));
        assert_eq!(options.limit, Some(20));
        assert!(options.json);
        assert!(parse(["--limit".to_owned(), "0".to_owned(), "x".to_owned()]).is_err());
        assert!(parse(Vec::<String>::new()).is_err());
    }

    #[test]
    fn since_folds_to_an_et_day() {
        assert_eq!(since_day("2026-10-30").as_deref(), Some("2026-10-30"));
        assert_eq!(since_day("10/30/2026").as_deref(), Some("2026-10-30"));
        assert_eq!(since_day("2026-10-30 09:15").as_deref(), Some("2026-10-30"));
        assert_eq!(since_day("next friday"), None);
        let mut options = Options {
            query: "deploy".to_owned(),
            since: Some("2026-10-30".to_owned()),
            ..Options::default()
        };
        assert_eq!(
            folded_query(&options).expect("folds"),
            "deploy after:2026-10-30"
        );
        options.since = None;
        assert_eq!(folded_query(&options).expect("plain"), "deploy");
    }

    #[test]
    fn deep_links_use_the_configured_base_and_name_the_message() {
        let hit = message("M 1", "C1", "deploy freeze notes");
        let link = deep_link("https://vibe.example/", &channel("C1", Some("slack")), &hit);
        assert_eq!(
            link,
            "https://vibe.example/?c=C1&v=main&m=slack%3AC1%3AM%25201"
        );
        // No provider known: the link still lands on the channel, never a guessed host part.
        let link = deep_link("https://vibe.example", &channel("C1", None), &hit);
        assert_eq!(link, "https://vibe.example/?c=C1&v=main");
    }

    #[test]
    fn render_says_the_window_when_nothing_hit() {
        let answer = SearchAnswer {
            hits: Vec::new(),
            scopes: vec![ScopeAnswer {
                channel: channel("C1", Some("slack")),
                state: "not_searched".to_owned(),
                reason: Some("serve_off".to_owned()),
                searched_back_to_ms: None,
                held_from_ms: None,
                whole: false,
                expanding: false,
                capped: 0,
                partial: 0,
                withheld: 0,
            }],
            resume_below_ms: None,
        };
        let out = render("https://vibe.example", &answer);
        assert!(out.contains("No hits in the window searched."), "{out}");
        assert!(out.contains("not searched: C1 (serve_off)"), "{out}");
        assert!(out.contains("only what the server's cache holds"), "{out}");
    }
}
