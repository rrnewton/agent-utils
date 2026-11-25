//! `vibe-talk wtf-replay`: re-run logged WTF lookups against a LOCAL server and
//! measure the engine (`#2232 vibe-talk-wtf-lookup-log-and-replay`).
//!
//! `run` reads the private lookup log ([`crate::glossary_lookup_log`]) or a
//! terms file, and posts each lookup, misses first, to a local test server
//! started with `[summaries] wtf_probe = true`, as an operator probe:
//! `--mode retrieval` stops before the model (no model call, deterministic),
//! `--mode full` runs the model step but never saves an entry. It writes one run
//! report; `compare` sets two reports side by side: hit rate, the lookups that
//! flipped, and median and p95 lookup time.
//!
//! Guards: the server must be a loopback address (`127.0.0.1`, `::1`,
//! `localhost`) and its port must not be one of the `--deny-port` values
//! (an operator wrapper passes its live servers' ports). The token is read
//! from a file and never printed; it must be the local test server's own token.
//! Requests are paced (`--pace-ms`, default 3000) and serial.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::glossary_lookup_log::LookupRecord;

/// Usage text.
pub const USAGE: &str = "\
vibe-talk wtf-replay: re-run logged WTF lookups against a LOCAL server (#2232)

USAGE:
    vibe-talk wtf-replay run --server http://127.0.0.1:PORT --token-file PATH
                             (--log PATH | --terms PATH) [--mode retrieval|full]
                             [--limit N] [--pace-ms N] [--deny-port PORT]...
                             [--out PATH] [--json]
    vibe-talk wtf-replay compare BEFORE.json AFTER.json [--json]

run       posts each lookup, misses first, to the local server as a probe
          (the server needs [summaries] wtf_probe = true). --log reads the
          lookup log (a day file, the wtf-lookups directory or the state
          directory); --terms reads JSON lines {\"selected\", \"channel\",
          \"message_id\"?, \"context\"?}. --mode retrieval (default) makes no
          model call; --mode full runs the model step and saves nothing.
          The server must be loopback and not a --deny-port. The token file
          holds the LOCAL server's own token; it is never printed.
compare   hit rate before and after, the lookups that flipped, and median
          and p95 lookup time.
";

/// One lookup to replay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayCase {
    /// Channel id the lookup was pressed in.
    pub channel: String,
    /// The selected words.
    pub selected: String,
    /// The message the selection stands in.
    #[serde(default)]
    pub message_id: Option<String>,
    /// The containing line.
    #[serde(default)]
    pub context: Option<String>,
    /// The logged outcome, when the case came from the log.
    #[serde(default)]
    pub logged_outcome: Option<String>,
}

impl ReplayCase {
    /// Identity across runs: channel plus folded selection.
    #[must_use]
    pub fn key(&self) -> String {
        format!(
            "{}\u{1f}{}",
            self.channel,
            crate::search_query::fold(self.selected.trim())
        )
    }
}

/// One replayed lookup's result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseResult {
    /// The case.
    pub case: ReplayCase,
    /// The answer's state (`found`, `evidence`, `not_found`) or `error`.
    pub state: String,
    /// The term answered.
    #[serde(default)]
    pub term: Option<String>,
    /// The logged outcome on the server (`diagnostics.outcome`).
    #[serde(default)]
    pub outcome: Option<String>,
    /// The engine tag the server reported.
    #[serde(default)]
    pub engine: Option<String>,
    /// Lookup time in ms: the server's own total, else the round trip.
    pub ms: u64,
    /// Searches the lookup ran.
    #[serde(default)]
    pub searches: usize,
    /// HTTP status and error code for an `error` state.
    #[serde(default)]
    pub error: Option<String>,
}

impl CaseResult {
    /// Whether the lookup hit.
    #[must_use]
    pub fn is_hit(&self) -> bool {
        matches!(self.state.as_str(), "found" | "evidence")
    }
}

/// Totals of one run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    /// Lookups replayed.
    pub total: usize,
    /// Hits.
    pub hits: usize,
    /// Hits over total (0 when empty).
    pub hit_rate: f64,
    /// Median lookup ms.
    pub median_ms: u64,
    /// 95th percentile lookup ms (nearest rank).
    pub p95_ms: u64,
}

/// One run's report, as written by `--out`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunReport {
    /// Report version.
    pub v: u32,
    /// `retrieval` or `full`.
    pub mode: String,
    /// Engine tag reported by the server (from the first answer).
    #[serde(default)]
    pub engine: Option<String>,
    /// Per-lookup results, in replay order.
    pub results: Vec<CaseResult>,
    /// Totals.
    pub summary: Summary,
}

/// `compare`'s result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    /// The first run's totals.
    pub before: Summary,
    /// The second run's totals.
    pub after: Summary,
    /// Lookups that missed before and hit after (selected words).
    pub gained: Vec<String>,
    /// Lookups that hit before and missed after.
    pub lost: Vec<String>,
    /// Lookups in both runs.
    pub common: usize,
}

/// Nearest-rank percentile of `values` (`pct` in 0..=100); 0 when empty.
#[must_use]
pub fn percentile(values: &[u64], pct: u32) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = (u64::from(pct) * sorted.len() as u64).div_ceil(100).max(1);
    let index = usize::try_from(rank - 1).unwrap_or(0).min(sorted.len() - 1);
    sorted[index]
}

/// Totals of a result list.
#[must_use]
pub fn summarize(results: &[CaseResult]) -> Summary {
    let total = results.len();
    let hits = results.iter().filter(|r| r.is_hit()).count();
    let ms: Vec<u64> = results.iter().map(|r| r.ms).collect();
    Summary {
        total,
        hits,
        hit_rate: if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        },
        median_ms: percentile(&ms, 50),
        p95_ms: percentile(&ms, 95),
    }
}

/// Replay cases from log records: one per (channel, selection), the latest
/// record's outcome kept, misses (and errors) first, each group oldest first.
/// Probe records are skipped: they are replays themselves.
#[must_use]
pub fn cases_from_records(records: &[LookupRecord]) -> Vec<ReplayCase> {
    let mut latest: BTreeMap<String, (usize, ReplayCase, bool)> = BTreeMap::new();
    for (index, record) in records.iter().enumerate() {
        if record.probe.is_some() || record.selected.trim().is_empty() {
            continue;
        }
        let case = ReplayCase {
            channel: record.channel.clone(),
            selected: record.selected.trim().to_owned(),
            message_id: record.message_id.clone(),
            context: Some(record.context.clone()).filter(|c| !c.is_empty()),
            logged_outcome: Some(record.outcome.clone()),
        };
        let key = case.key();
        let first = latest.get(&key).map_or(index, |(first, _, _)| *first);
        latest.insert(key, (first, case, record.is_hit()));
    }
    let mut ordered: Vec<(bool, usize, ReplayCase)> = latest
        .into_values()
        .map(|(first, case, hit)| (hit, first, case))
        .collect();
    ordered.sort_by_key(|(hit, first, _)| (*hit, *first));
    ordered.into_iter().map(|(_, _, case)| case).collect()
}

/// Replay cases from a terms file: JSON lines with `selected` (or `term`),
/// `channel`, and optional `message_id` and `context`. Bad lines are skipped.
#[must_use]
pub fn cases_from_terms(text: &str) -> Vec<ReplayCase> {
    #[derive(Deserialize)]
    struct Line {
        #[serde(default)]
        selected: Option<String>,
        #[serde(default)]
        term: Option<String>,
        channel: String,
        #[serde(default)]
        message_id: Option<String>,
        #[serde(default)]
        context: Option<String>,
    }
    let mut out: Vec<ReplayCase> = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(parsed) = serde_json::from_str::<Line>(line) else {
            continue;
        };
        let Some(selected) = parsed.selected.or(parsed.term) else {
            continue;
        };
        let case = ReplayCase {
            channel: parsed.channel,
            selected: selected.trim().to_owned(),
            message_id: parsed.message_id,
            context: parsed.context,
            logged_outcome: None,
        };
        if !case.selected.is_empty() && !out.iter().any(|held| held.key() == case.key()) {
            out.push(case);
        }
    }
    out
}

/// Refuse anything but a loopback server on an allowed port. Returns the base
/// URL without a trailing slash.
///
/// # Errors
///
/// A message naming why the address is refused.
pub fn checked_server(server: &str, deny_ports: &[u16]) -> Result<String, String> {
    let url = url::Url::parse(server).map_err(|e| format!("--server {server:?}: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "--server must be http or https, not {}",
            url.scheme()
        ));
    }
    let loopback = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    if !loopback {
        return Err(format!(
            "--server must be a loopback address (127.0.0.1, ::1 or localhost); refusing {server}"
        ));
    }
    let Some(port) = url.port_or_known_default() else {
        return Err(format!("--server {server} names no port"));
    };
    if deny_ports.contains(&port) {
        return Err(format!(
            "--server port {port} is a denied (live) port; replay only against a local test server"
        ));
    }
    if url.path() != "/" || url.query().is_some() {
        return Err("--server takes a base URL only (scheme, host, port)".to_owned());
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// Parsed `run` options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunOptions {
    /// Base URL, checked.
    pub server: String,
    /// The local server's token file.
    pub token_file: PathBuf,
    /// Lookup log path.
    pub log: Option<PathBuf>,
    /// Terms file path.
    pub terms: Option<PathBuf>,
    /// `retrieval` or `full`.
    pub mode: String,
    /// The most lookups replayed.
    pub limit: Option<usize>,
    /// Milliseconds between requests.
    pub pace_ms: u64,
    /// Report path.
    pub out: Option<PathBuf>,
    /// JSON on stdout.
    pub json: bool,
}

/// What the command line asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Print usage.
    Help,
    /// Replay.
    Run(RunOptions),
    /// Compare two reports.
    Compare {
        /// First report.
        before: PathBuf,
        /// Second report.
        after: PathBuf,
        /// JSON on stdout.
        json: bool,
    },
}

/// Parse `vibe-talk wtf-replay …` arguments (after the subcommand name).
///
/// # Errors
///
/// A usage message for a malformed command line or a refused server.
pub fn parse_args(args: Vec<String>) -> Result<Command, String> {
    let mut args = args.into_iter();
    let verb = args.next();
    match verb.as_deref() {
        None | Some("--help" | "-h" | "help") => Ok(Command::Help),
        Some("compare") => {
            let mut paths: Vec<PathBuf> = Vec::new();
            let mut json = false;
            for arg in args {
                match arg.as_str() {
                    "--json" => json = true,
                    "--help" | "-h" => return Ok(Command::Help),
                    flag if flag.starts_with("--") => {
                        return Err(format!("wtf-replay compare: unknown flag {flag}"));
                    }
                    path => paths.push(PathBuf::from(path)),
                }
            }
            let [before, after] = <[PathBuf; 2]>::try_from(paths)
                .map_err(|_| "wtf-replay compare takes exactly two report files".to_owned())?;
            Ok(Command::Compare {
                before,
                after,
                json,
            })
        }
        Some("run") => {
            let mut server: Option<String> = None;
            let mut token_file: Option<PathBuf> = None;
            let mut log: Option<PathBuf> = None;
            let mut terms: Option<PathBuf> = None;
            let mut mode = "retrieval".to_owned();
            let mut limit: Option<usize> = None;
            let mut pace_ms: u64 = 3_000;
            let mut deny: Vec<u16> = Vec::new();
            let mut out: Option<PathBuf> = None;
            let mut json = false;
            while let Some(arg) = args.next() {
                let mut value = |name: &str| {
                    args.next()
                        .ok_or_else(|| format!("wtf-replay run: {name} needs a value"))
                };
                match arg.as_str() {
                    "--help" | "-h" => return Ok(Command::Help),
                    "--server" => server = Some(value("--server")?),
                    "--token-file" => token_file = Some(PathBuf::from(value("--token-file")?)),
                    "--log" => log = Some(PathBuf::from(value("--log")?)),
                    "--terms" => terms = Some(PathBuf::from(value("--terms")?)),
                    "--mode" => mode = value("--mode")?,
                    "--limit" => {
                        limit = Some(
                            value("--limit")?
                                .parse()
                                .map_err(|_| "--limit takes a number".to_owned())?,
                        );
                    }
                    "--pace-ms" => {
                        pace_ms = value("--pace-ms")?
                            .parse()
                            .map_err(|_| "--pace-ms takes a number".to_owned())?;
                    }
                    "--deny-port" => deny.push(
                        value("--deny-port")?
                            .parse()
                            .map_err(|_| "--deny-port takes a port number".to_owned())?,
                    ),
                    "--out" => out = Some(PathBuf::from(value("--out")?)),
                    "--json" => json = true,
                    other => return Err(format!("wtf-replay run: unknown argument {other}")),
                }
            }
            if !matches!(mode.as_str(), "retrieval" | "full") {
                return Err("--mode is retrieval or full".to_owned());
            }
            if log.is_some() == terms.is_some() {
                return Err("wtf-replay run takes exactly one of --log or --terms".to_owned());
            }
            let server = checked_server(
                &server.ok_or_else(|| "wtf-replay run needs --server".to_owned())?,
                &deny,
            )?;
            let token_file =
                token_file.ok_or_else(|| "wtf-replay run needs --token-file".to_owned())?;
            Ok(Command::Run(RunOptions {
                server,
                token_file,
                log,
                terms,
                mode,
                limit,
                pace_ms,
                out,
                json,
            }))
        }
        Some(other) => Err(format!(
            "wtf-replay: unknown command {other}; see vibe-talk wtf-replay --help"
        )),
    }
}

/// The server's answer to a probe, as far as the replay reads it.
#[derive(Debug, Deserialize)]
struct ProbeReply {
    state: String,
    #[serde(default)]
    term: Option<String>,
    #[serde(default)]
    diagnostics: Option<LookupRecord>,
}

/// An error answer's code.
#[derive(Debug, Deserialize)]
struct ErrorReply {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// One probe request; never fails, an error becomes an `error` result.
async fn replay_one(
    client: &reqwest::Client,
    options: &RunOptions,
    token: &str,
    case: &ReplayCase,
) -> CaseResult {
    let url = format!(
        "{}/api/v1/channels/{}/glossary/lookup?probe={}",
        options.server,
        url::form_urlencoded::byte_serialize(case.channel.as_bytes()).collect::<String>(),
        options.mode
    );
    let body = serde_json::json!({
        "selected": case.selected,
        "message_id": case.message_id,
        "context": case.context,
    });
    let began = std::time::Instant::now();
    let sent = client.post(url).bearer_auth(token).json(&body).send().await;
    let round_trip = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut result = CaseResult {
        case: case.clone(),
        state: "error".to_owned(),
        term: None,
        outcome: None,
        engine: None,
        ms: round_trip,
        searches: 0,
        error: None,
    };
    match sent {
        Err(error) => {
            result.error = Some(format!("request failed: {}", error.without_url()));
        }
        Ok(response) => {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            if status.is_success() {
                match serde_json::from_str::<ProbeReply>(&text) {
                    Ok(reply) => {
                        result.state = reply.state;
                        result.term = reply.term;
                        if let Some(diag) = reply.diagnostics {
                            result.outcome = Some(diag.outcome);
                            result.engine = Some(diag.engine);
                            result.searches = diag.queries.len();
                            if diag.ms.total > 0 {
                                result.ms = diag.ms.total;
                            }
                        }
                    }
                    Err(error) => result.error = Some(format!("unreadable answer: {error}")),
                }
            } else {
                let code = serde_json::from_str::<ErrorReply>(&text)
                    .ok()
                    .and_then(|e| e.code.or(e.error))
                    .unwrap_or_default();
                result.error = Some(format!("HTTP {} {code}", status.as_u16()));
            }
        }
    }
    result
}

/// Replay `cases` against the server and build the report.
///
/// # Errors
///
/// When the HTTP client cannot be built.
pub async fn replay(
    options: &RunOptions,
    token: &str,
    cases: &[ReplayCase],
) -> anyhow::Result<RunReport> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(180))
        .build()
        .context("building the HTTP client")?;
    let mut results: Vec<CaseResult> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        if index > 0 && options.pace_ms > 0 {
            tokio::time::sleep(Duration::from_millis(options.pace_ms)).await;
        }
        let result = replay_one(&client, options, token, case).await;
        let stop = result
            .error
            .as_deref()
            .is_some_and(|e| e.contains("probe_disabled") || e.starts_with("HTTP 401"));
        results.push(result);
        if stop {
            break;
        }
    }
    let engine = results.iter().find_map(|r| r.engine.clone());
    let summary = summarize(&results);
    Ok(RunReport {
        v: 1,
        mode: options.mode.clone(),
        engine,
        results,
        summary,
    })
}

/// Side-by-side of two reports.
#[must_use]
pub fn compare(before: &RunReport, after: &RunReport) -> Comparison {
    let index: BTreeMap<String, &CaseResult> =
        before.results.iter().map(|r| (r.case.key(), r)).collect();
    let mut gained = Vec::new();
    let mut lost = Vec::new();
    let mut common = 0;
    for result in &after.results {
        let Some(old) = index.get(&result.case.key()) else {
            continue;
        };
        common += 1;
        match (old.is_hit(), result.is_hit()) {
            (false, true) => gained.push(result.case.selected.clone()),
            (true, false) => lost.push(result.case.selected.clone()),
            _ => {}
        }
    }
    Comparison {
        before: before.summary.clone(),
        after: after.summary.clone(),
        gained,
        lost,
        common,
    }
}

fn summary_line(label: &str, summary: &Summary) -> String {
    format!(
        "{label}: {}/{} hits ({:.1}%), median {} ms, p95 {} ms",
        summary.hits,
        summary.total,
        summary.hit_rate * 100.0,
        summary.median_ms,
        summary.p95_ms
    )
}

/// Human rendering of a run.
#[must_use]
pub fn render_run(report: &RunReport) -> String {
    let mut out = String::new();
    for result in &report.results {
        let mark = if result.is_hit() { "HIT " } else { "miss" };
        let answer = result
            .error
            .clone()
            .or_else(|| result.term.clone())
            .unwrap_or_default();
        out.push_str(&format!(
            "{mark} {:>6} ms  {:<14} {}  -> {}\n",
            result.ms,
            result.outcome.as_deref().unwrap_or(&result.state),
            result.case.selected,
            answer
        ));
    }
    out.push_str(&summary_line(
        &format!(
            "{} ({})",
            report.mode,
            report.engine.as_deref().unwrap_or("engine unknown")
        ),
        &report.summary,
    ));
    out.push('\n');
    out
}

/// Human rendering of a comparison.
#[must_use]
pub fn render_comparison(comparison: &Comparison) -> String {
    let mut out = String::new();
    out.push_str(&summary_line("before", &comparison.before));
    out.push('\n');
    out.push_str(&summary_line("after ", &comparison.after));
    out.push('\n');
    out.push_str(&format!("in both runs: {}\n", comparison.common));
    out.push_str(&format!("gained ({}):\n", comparison.gained.len()));
    for selected in &comparison.gained {
        out.push_str(&format!("  + {selected}\n"));
    }
    out.push_str(&format!("lost ({}):\n", comparison.lost.len()));
    for selected in &comparison.lost {
        out.push_str(&format!("  - {selected}\n"));
    }
    out
}

/// Read the token file: one line, the local server's token.
fn read_token(path: &std::path::Path) -> anyhow::Result<String> {
    let token = std::fs::read_to_string(path)
        .with_context(|| format!("reading the token file {}", path.display()))?
        .trim()
        .to_owned();
    anyhow::ensure!(
        !token.is_empty(),
        "the token file {} is empty",
        path.display()
    );
    Ok(token)
}

fn read_report(path: &std::path::Path) -> anyhow::Result<RunReport> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading the report {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing the report {}", path.display()))
}

/// Run the command line; returns what to print.
///
/// # Errors
///
/// Usage errors, unreadable inputs, or an HTTP client that cannot be built.
pub async fn run_cli(args: Vec<String>) -> anyhow::Result<String> {
    match parse_args(args).map_err(anyhow::Error::msg)? {
        Command::Help => Ok(USAGE.to_owned()),
        Command::Compare {
            before,
            after,
            json,
        } => {
            let comparison = compare(&read_report(&before)?, &read_report(&after)?);
            if json {
                Ok(serde_json::to_string_pretty(&comparison)?)
            } else {
                Ok(render_comparison(&comparison))
            }
        }
        Command::Run(options) => {
            let token = read_token(&options.token_file)?;
            let mut cases = if let Some(log) = &options.log {
                cases_from_records(&crate::glossary_lookup_log::read_records(log))
            } else if let Some(terms) = &options.terms {
                let text = std::fs::read_to_string(terms)
                    .with_context(|| format!("reading the terms file {}", terms.display()))?;
                cases_from_terms(&text)
            } else {
                Vec::new()
            };
            if let Some(limit) = options.limit {
                cases.truncate(limit);
            }
            anyhow::ensure!(!cases.is_empty(), "no lookups to replay");
            let report = replay(&options, &token, &cases).await?;
            if let Some(out) = &options.out {
                std::fs::write(out, serde_json::to_string_pretty(&report)?)
                    .with_context(|| format!("writing the report {}", out.display()))?;
            }
            if options.json {
                Ok(serde_json::to_string_pretty(&report)?)
            } else {
                Ok(render_run(&report))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(selected: &str, outcome: &str, probe: Option<&str>) -> LookupRecord {
        LookupRecord {
            v: 1,
            channel: "c1".to_owned(),
            selected: selected.to_owned(),
            outcome: outcome.to_owned(),
            probe: probe.map(str::to_owned),
            ..LookupRecord::default()
        }
    }

    fn result(selected: &str, state: &str, ms: u64) -> CaseResult {
        CaseResult {
            case: ReplayCase {
                channel: "c1".to_owned(),
                selected: selected.to_owned(),
                message_id: None,
                context: None,
                logged_outcome: None,
            },
            state: state.to_owned(),
            term: None,
            outcome: None,
            engine: None,
            ms,
            searches: 0,
            error: None,
        }
    }

    #[test]
    fn only_loopback_servers_off_the_denied_ports_are_accepted() {
        assert_eq!(
            checked_server("http://127.0.0.1:18555/", &[]).unwrap(),
            "http://127.0.0.1:18555"
        );
        assert!(checked_server("http://localhost:9000", &[]).is_ok());
        assert!(checked_server("http://[::1]:9000", &[]).is_ok());
        assert!(checked_server("http://10.1.2.3:9000", &[]).is_err());
        assert!(checked_server("https://example.com:443", &[]).is_err());
        let denied = checked_server("http://127.0.0.1:9000", &[8000, 9000]).unwrap_err();
        assert!(denied.contains("denied"), "{denied}");
        assert!(checked_server("http://127.0.0.1:9000/api", &[]).is_err());
        assert!(checked_server("ftp://127.0.0.1:21", &[]).is_err());
    }

    #[test]
    fn log_records_become_cases_misses_first_deduped_without_probes() {
        let records = vec![
            record("Alpha One", "found", None),
            record("Beta Two", "miss_no_evidence", None),
            record("alpha one", "miss_model_declined", None),
            record("Gamma", "evidence", Some("retrieval")),
            record("Delta", "error", None),
        ];
        let cases = cases_from_records(&records);
        let selected: Vec<&str> = cases.iter().map(|c| c.selected.as_str()).collect();
        // Latest outcome wins (Alpha One is now a miss), probes are skipped,
        // misses come first in first-seen order.
        assert_eq!(selected, vec!["alpha one", "Beta Two", "Delta"]);
        assert_eq!(
            cases[0].logged_outcome.as_deref(),
            Some("miss_model_declined")
        );
    }

    #[test]
    fn terms_files_take_selected_or_term_and_skip_bad_lines() {
        let text = "{\"term\":\"Row 4\",\"channel\":\"c1\"}\nnot json\n{\"selected\":\"x\",\"channel\":\"c2\",\"message_id\":\"9\"}\n{\"channel\":\"c3\"}\n{\"term\":\"row 4\",\"channel\":\"c1\"}\n";
        let cases = cases_from_terms(text);
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].selected, "Row 4");
        assert_eq!(cases[1].message_id.as_deref(), Some("9"));
    }

    #[test]
    fn percentiles_use_the_nearest_rank() {
        assert_eq!(percentile(&[], 50), 0);
        assert_eq!(percentile(&[7], 95), 7);
        let values: Vec<u64> = (1..=20).collect();
        assert_eq!(percentile(&values, 50), 10);
        assert_eq!(percentile(&values, 95), 19);
        assert_eq!(percentile(&[5, 1, 9, 3], 50), 3);
    }

    #[test]
    fn compare_reports_rates_flips_and_times() {
        let before_results = vec![
            result("a", "not_found", 9_000),
            result("b", "found", 100),
            result("c", "not_found", 800),
        ];
        let after_results = vec![
            result("a", "evidence", 120),
            result("b", "not_found", 90),
            result("c", "not_found", 70),
            result("d", "found", 10),
        ];
        let before = RunReport {
            v: 1,
            mode: "retrieval".to_owned(),
            engine: None,
            summary: summarize(&before_results),
            results: before_results,
        };
        let after = RunReport {
            v: 1,
            mode: "retrieval".to_owned(),
            engine: None,
            summary: summarize(&after_results),
            results: after_results,
        };
        let comparison = compare(&before, &after);
        assert_eq!(comparison.gained, vec!["a"]);
        assert_eq!(comparison.lost, vec!["b"]);
        assert_eq!(comparison.common, 3);
        assert_eq!(comparison.before.hits, 1);
        assert_eq!(comparison.after.hits, 2);
        assert_eq!(comparison.before.median_ms, 800);
        assert_eq!(comparison.before.p95_ms, 9_000);
        let text = render_comparison(&comparison);
        assert!(
            text.contains("before: 1/3 hits (33.3%), median 800 ms, p95 9000 ms"),
            "{text}"
        );
        assert!(text.contains("  + a"), "{text}");
    }

    #[test]
    fn the_command_line_needs_one_source_and_a_checked_server() {
        let args = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(parse_args(args("")).unwrap(), Command::Help);
        assert!(parse_args(args("run --server http://127.0.0.1:1 --token-file t")).is_err());
        assert!(parse_args(args(
            "run --server http://127.0.0.1:1 --token-file t --log a --terms b"
        ))
        .is_err());
        assert!(parse_args(args(
            "run --server http://127.0.0.1:1 --token-file t --log a --deny-port 1"
        ))
        .is_err());
        assert!(parse_args(args(
            "run --server http://127.0.0.1:1 --token-file t --log a --mode x"
        ))
        .is_err());
        let Command::Run(options) = parse_args(args(
            "run --server http://127.0.0.1:1 --token-file t --log a --mode full --limit 3 --pace-ms 0",
        ))
        .unwrap() else {
            panic!("run");
        };
        assert_eq!(options.mode, "full");
        assert_eq!(options.limit, Some(3));
        assert_eq!(options.pace_ms, 0);
        assert!(matches!(
            parse_args(args("compare a.json b.json --json")).unwrap(),
            Command::Compare { json: true, .. }
        ));
        assert!(parse_args(args("compare a.json")).is_err());
    }
}
