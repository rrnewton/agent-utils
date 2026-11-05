//! Requests that agents raise in their status files (#862 watch-status-requests).
//!
//! A team can have its agents ask the coordinator for something (land this, decide that) by
//! writing a request marker, such as `COORDINATOR_RESPOND_PLEASE`, into the newest heading of
//! their status file, or into the text under it. The coordinator answers by editing that heading
//! to start with an answered word, such as `ANSWERED`, `LANDED` or `[answered: ...]`. A request
//! that nobody watches can wait for hours, so `agentctl watch-agents` pages the designated
//! coordinator about each new one and again until it is answered, `agentctl requests` lists the
//! ones waiting, and `agentctl send` from the coordinator ends with that list.
//!
//! The registry's `status-requests.json` turns this on and says where the status files are; see
//! [`StatusConfig`]. A request is read by the rules below, which every reader shares.
//!
//! * A section is a `#` or `##` heading and the text under it, up to the next such heading; a
//!   heading inside a fenced code block is not one.
//! * A section is an open request when its heading or text carries the marker and its heading
//!   carries no answered word. An answered word counts at the start of the heading, after its
//!   timestamp (`07:40 AM EDT Sun 2026-10-25 — LANDED ...`), or anywhere in brackets
//!   (`(ANSWERED, coordinator)`, `[answered: ...]`). `landed` counts only at the start, since
//!   agents write `(not landed)` themselves. A marker on a line that a bracketed answered word
//!   precedes, one introduced by `was`, and one a sentence names (`the marker heading`, `the
//!   marker comes with ...`) are not live requests.
//! * Only the newest sections are read: the newest three by file order, and the newest by the
//!   time its heading gives. The file's newest end is the top unless its dated headings mostly
//!   grow downward. A request is aged from its heading's time when one parses (Eastern, Central,
//!   Pacific and UTC spellings), else from the file's last write; a time with no date is placed
//!   on the latest day that does not put it after that write.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The registry file that turns status requests on.
pub const CONFIG_FILE: &str = "status-requests.json";
/// The placeholder in [`StatusConfig::status_files`] that stands for an agent's name.
const AGENT_PLACEHOLDER: &str = "{agent}";
/// How many sections at the newest end of a status file are always read.
const WINDOW: usize = 3;
/// A time written with no date is placed on the latest day that puts it no more than this many
/// seconds after the file's last write.
const DATELESS_SLACK_SECONDS: i64 = 5 * 60;
/// How many characters of a heading a page or a list entry shows.
const HEADING_CHARS: usize = 120;
/// How many characters of a heading one entry of the waiting list shows.
const EXCERPT_CHARS: usize = 60;
/// A heading stamped further than this in the future is a misparse, and is aged from the file.
const FUTURE_SLACK_SECONDS: i64 = 5 * 60;

/// What `status-requests.json` in a registry says (#862 watch-status-requests).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StatusConfig {
    /// Where each agent's status file is, relative to the directory that holds the registry,
    /// with `{agent}` once in its file name, such as `temp/{agent}-status.md`.
    pub status_files: String,
    /// The text that marks a request.
    #[serde(default = "default_marker")]
    pub marker: String,
    /// Words that mark a heading answered when it starts with one, after its timestamp.
    #[serde(default = "default_ack_words")]
    pub ack_words: Vec<String>,
    /// Words that mark a heading answered anywhere in it, after `[` or `(`.
    #[serde(default = "default_bracketed_ack_words")]
    pub bracketed_ack_words: Vec<String>,
    /// Minutes between two pages about one request that is still open.
    #[serde(default = "default_repage_minutes")]
    pub repage_minutes: u64,
}

fn default_marker() -> String {
    "COORDINATOR_RESPOND_PLEASE".to_owned()
}

fn default_ack_words() -> Vec<String> {
    [
        "answered",
        "landed",
        "acknowledged",
        "acked",
        "ack",
        "decided",
        "superseded",
        "resolved",
        "handled",
    ]
    .map(str::to_owned)
    .to_vec()
}

fn default_bracketed_ack_words() -> Vec<String> {
    [
        "answered",
        "acknowledged",
        "acked",
        "decided",
        "superseded",
        "resolved",
        "handled",
    ]
    .map(str::to_owned)
    .to_vec()
}

const fn default_repage_minutes() -> u64 {
    30
}

impl StatusConfig {
    /// The registry's configuration, or `None` when it has no `status-requests.json`.
    ///
    /// # Errors
    ///
    /// The file cannot be read, is not a valid configuration, or names its status files in a
    /// way that cannot be followed.
    pub fn load(registry: &Path) -> Result<Option<Self>, String> {
        let path = registry.join(CONFIG_FILE);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        let config: Self = serde_json::from_str(&text)
            .map_err(|error| format!("{} is not a valid configuration: {error}", path.display()))?;
        config
            .validate()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(Some(config))
    }

    fn validate(&self) -> Result<(), String> {
        let pattern = Path::new(&self.status_files);
        let name = pattern
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if self.status_files.matches(AGENT_PLACEHOLDER).count() != 1
            || name.matches(AGENT_PLACEHOLDER).count() != 1
        {
            return Err(format!(
                "status_files must name {AGENT_PLACEHOLDER} once, in its file name"
            ));
        }
        if pattern.is_absolute()
            || pattern
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err("status_files must be a relative path without ..".to_owned());
        }
        if self.marker.trim().is_empty() {
            return Err("marker must not be empty".to_owned());
        }
        if self.repage_minutes == 0 {
            return Err("repage_minutes must be at least 1".to_owned());
        }
        Ok(())
    }

    /// The directory the status files are in, for a registry at `registry`.
    #[must_use]
    pub fn directory(&self, registry: &Path) -> PathBuf {
        let path = base(registry).join(&self.status_files);
        path.parent().map(Path::to_path_buf).unwrap_or(path)
    }

    /// `agent`'s status file, for a registry at `registry`.
    #[must_use]
    pub fn status_file(&self, registry: &Path, agent: &str) -> PathBuf {
        base(registry).join(self.status_files.replace(AGENT_PLACEHOLDER, agent))
    }
}

/// The directory that holds the registry, which status file paths are relative to.
fn base(registry: &Path) -> PathBuf {
    registry
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// One open request in an agent's status file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OpenRequest {
    /// The agent whose status file holds it.
    pub agent: String,
    /// The status file.
    pub path: PathBuf,
    /// The heading, on one line, at most 120 characters.
    pub heading: String,
    /// The first 12 hex digits of the SHA-256 of the heading's text: it names the request, so an
    /// edit to the heading is a new request.
    pub heading_hash: String,
    /// The heading's line in the status file.
    pub line: usize,
    /// Minutes the request has waited.
    pub age_minutes: u64,
    /// `heading` when its age is read from the heading's time, `mtime` from the file's.
    pub age_source: &'static str,
}

impl OpenRequest {
    /// The key a page about this request is recorded under: the agent and the heading's hash.
    #[must_use]
    pub fn key(&self) -> String {
        format!("{} {}", self.agent, self.heading_hash)
    }
}

/// The open requests in every live agent's status file, each agent's oldest first, agents with
/// the oldest request first. `skip` names agents not read, such as the coordinator itself.
#[must_use]
pub fn scan(
    registry: &Path,
    config: &StatusConfig,
    skip: &BTreeSet<String>,
    now: SystemTime,
) -> Vec<OpenRequest> {
    let mut found = Vec::new();
    for agent in live_agents(registry) {
        if skip.contains(&agent) {
            continue;
        }
        let path = config.status_file(registry, &agent);
        found.extend(requests_in(&agent, &path, config, now));
    }
    found.sort_by(|left, right| {
        right
            .age_minutes
            .cmp(&left.age_minutes)
            .then_with(|| left.agent.cmp(&right.agent))
            .then_with(|| left.line.cmp(&right.line))
    });
    found
}

/// Agents with a record in the registry whose lifecycle is not `stopped`, and that have a pane or
/// a working directory.
fn live_agents(registry: &Path) -> Vec<String> {
    let Ok(names) = agent_queue::registry::names(registry) else {
        return Vec::new();
    };
    names
        .into_iter()
        .filter(|name| {
            let Ok(directory) = agent_queue::registry::directory(registry, name) else {
                return false;
            };
            let Ok(text) = fs::read_to_string(directory.join("agent.json")) else {
                return false;
            };
            let Ok(record) = serde_json::from_str::<serde_json::Value>(&text) else {
                return false;
            };
            // An agent with neither a pane nor a working directory has nowhere to work from.
            let placed = record["pane_id"]
                .as_str()
                .is_some_and(|pane| !pane.is_empty())
                || record["cwd"]
                    .as_str()
                    .is_some_and(|cwd| Path::new(cwd).is_dir());
            record["lifecycle"] != "stopped" && placed
        })
        .collect()
}

/// The open requests in `agent`'s status file at `path`, oldest first; none when it is missing.
#[must_use]
pub fn requests_in(
    agent: &str,
    path: &Path,
    config: &StatusConfig,
    now: SystemTime,
) -> Vec<OpenRequest> {
    let Ok(metadata) = fs::metadata(path) else {
        return Vec::new();
    };
    let Ok(text) = fs::read(path) else {
        return Vec::new();
    };
    let written = metadata.modified().map_or(0, unix_seconds);
    let now = unix_seconds(now);
    let text = String::from_utf8_lossy(&text);
    let found = sections(&text);
    let mut aged: Vec<(i64, &'static str, &Section)> = open_requests(&found, written, config)
        .into_iter()
        .map(|(section, stamp)| match stamp {
            Some(stamp) if stamp <= now + FUTURE_SLACK_SECONDS => (stamp, "heading", section),
            _ => (written, "mtime", section),
        })
        .collect();
    aged.sort_by_key(|(stamp, _, section)| (*stamp, section.index));
    aged.into_iter()
        .map(|(stamp, source, section)| {
            let heading = clean(&section.heading);
            OpenRequest {
                agent: agent.to_owned(),
                path: path.to_owned(),
                heading_hash: digest(&section.heading),
                heading: truncate(&heading, HEADING_CHARS),
                line: section.line,
                age_minutes: u64::try_from((now - stamp).max(0) / 60).unwrap_or(0),
                age_source: source,
            }
        })
        .collect()
}

/// The waiting list as one line: `0 waiting`, or `N waiting: A 12m (heading), B 4m (heading)`,
/// one entry per agent, its oldest request, with how many more it has. With `more`, the count
/// reads `N more waiting`, as at the end of a reply the coordinator just sent.
#[must_use]
pub fn waiting_line(requests: &[OpenRequest], more: bool) -> String {
    let mut by_agent: Vec<(&OpenRequest, usize)> = Vec::new();
    for request in requests {
        match by_agent
            .iter_mut()
            .find(|(first, _)| first.agent == request.agent)
        {
            Some((_, count)) => *count += 1,
            None => by_agent.push((request, 1)),
        }
    }
    if by_agent.is_empty() {
        return "0 waiting".to_owned();
    }
    let entries: Vec<String> = by_agent
        .iter()
        .map(|(request, count)| {
            let extra = if *count > 1 {
                format!(", +{} more", count - 1)
            } else {
                String::new()
            };
            format!(
                "{} {}m ({}{extra})",
                request.agent,
                request.age_minutes,
                truncate(&request.heading, EXCERPT_CHARS)
            )
        })
        .collect();
    format!(
        "{} {}waiting: {}",
        requests.len(),
        if more { "more " } else { "" },
        entries.join(", ")
    )
}

/// One page the watcher sent about a request, as its state records it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PageRecord {
    /// The agent whose status file holds the request.
    pub agent: String,
    /// The request's heading.
    pub heading: String,
    /// When the first page was sent, in seconds since the Unix epoch.
    pub first_paged: f64,
    /// When the latest page was sent.
    pub last_paged: f64,
    /// How many pages were sent.
    pub pages: u32,
}

/// Why a request is paged now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageReason {
    /// It was never paged.
    New,
    /// Its last page is older than the re-page interval.
    Repage,
    /// Its agent just went from working to idle with it open.
    Idle,
}

impl PageReason {
    /// How a page says why it was sent.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Repage => "still open",
            Self::Idle => "agent went idle with it open",
        }
    }
}

/// An agent that goes idle is paged about its open request at once, unless a page went out
/// this recently, such as the one sent when the request was written moments before.
pub const IDLE_PAGE_FLOOR_SECONDS: f64 = 120.0;

/// The requests in `open` to page about at `now`: each never paged, each last paged
/// `repage_seconds` or longer ago, and each of an agent in `went_idle` last paged at least
/// [`IDLE_PAGE_FLOOR_SECONDS`] ago. Records of requests no longer open, answered or edited, are
/// dropped from `records`, so their paging stops.
#[must_use]
pub fn due_pages(
    open: &[OpenRequest],
    records: &mut BTreeMap<String, PageRecord>,
    now: f64,
    repage_seconds: f64,
    went_idle: &BTreeSet<String>,
) -> Vec<(OpenRequest, PageReason)> {
    let keys: BTreeSet<String> = open.iter().map(OpenRequest::key).collect();
    records.retain(|key, _| keys.contains(key));
    let mut due = Vec::new();
    let mut seen = BTreeSet::new();
    for request in open {
        if !seen.insert(request.key()) {
            continue;
        }
        let reason = match records.get(&request.key()) {
            None => Some(PageReason::New),
            Some(record) if now - record.last_paged >= repage_seconds => Some(PageReason::Repage),
            Some(record)
                if went_idle.contains(&request.agent)
                    && now - record.last_paged >= IDLE_PAGE_FLOOR_SECONDS =>
            {
                Some(PageReason::Idle)
            }
            Some(_) => None,
        };
        if let Some(reason) = reason {
            due.push((request.clone(), reason));
        }
    }
    due
}

/// Record that a page about `request` went out at `now`.
pub fn record_page(records: &mut BTreeMap<String, PageRecord>, request: &OpenRequest, now: f64) {
    let record = records.entry(request.key()).or_insert_with(|| PageRecord {
        agent: request.agent.clone(),
        heading: request.heading.clone(),
        first_paged: now,
        last_paged: now,
        pages: 0,
    });
    record.last_paged = now;
    record.pages += 1;
}

/// The text of a page about `request`.
#[must_use]
pub fn page_text(request: &OpenRequest, reason: PageReason, config: &StatusConfig) -> String {
    let answered = config
        .ack_words
        .iter()
        .take(2)
        .map(|word| word.to_uppercase())
        .collect::<Vec<_>>()
        .join(" or ");
    format!(
        "Status request waiting for you ({}): {} asks in {} line {}, {}m ago: {}\nAnswer it, then \
         mark the heading answered: start it with {answered}, or add [answered: ...]. Until then \
         this repeats every {} minutes.",
        reason.as_str(),
        request.agent,
        request.path.display(),
        request.line,
        request.age_minutes,
        request.heading,
        config.repage_minutes
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Section {
    index: usize,
    line: usize,
    heading: String,
    body: String,
}

/// The `#` and `##` sections of a status file; a heading in a fenced code block is not one.
fn sections(text: &str) -> Vec<Section> {
    let mut found = Vec::new();
    let mut current: Option<Section> = None;
    let mut body: Vec<&str> = Vec::new();
    let mut in_fence = false;
    for (number, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
        }
        let heading = if in_fence { None } else { heading_of(line) };
        if let Some(heading) = heading {
            if let Some(mut section) = current.take() {
                section.body = body.join("\n");
                found.push(section);
            }
            body.clear();
            current = Some(Section {
                index: found.len(),
                line: number + 1,
                heading: heading.to_owned(),
                body: String::new(),
            });
            continue;
        }
        if current.is_some() {
            body.push(line);
        }
    }
    if let Some(mut section) = current {
        section.body = body.join("\n");
        found.push(section);
    }
    found
}

/// The text of a `#` or `##` heading line.
fn heading_of(line: &str) -> Option<&str> {
    let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=2).contains(&hashes) {
        return None;
    }
    let rest = &line[hashes..];
    rest.starts_with(char::is_whitespace).then(|| rest.trim())
}

/// The open requests among the newest sections, each with its heading's time.
fn open_requests<'a>(
    found: &'a [Section],
    written: i64,
    config: &StatusConfig,
) -> Vec<(&'a Section, Option<i64>)> {
    let window: Vec<&Section> = if newest_first(found, written) {
        found.iter().take(WINDOW).collect()
    } else {
        found
            .iter()
            .skip(found.len().saturating_sub(WINDOW))
            .collect()
    };
    let mut chosen: BTreeMap<usize, &Section> = window
        .into_iter()
        .map(|section| (section.index, section))
        .collect();
    if let Some(section) = newest(found, written) {
        chosen.insert(section.index, section);
    }
    chosen
        .into_values()
        .filter(|section| is_open_request(section, config))
        .map(|section| (section, heading_time(&section.heading, written)))
        .collect()
}

/// The newest section by its heading's time; the first one when no heading has a time.
fn newest(found: &[Section], written: i64) -> Option<&Section> {
    found
        .iter()
        .filter_map(|section| heading_time(&section.heading, written).map(|stamp| (stamp, section)))
        .max_by(|(left, left_section), (right, right_section)| {
            left.cmp(right)
                .then_with(|| right_section.index.cmp(&left_section.index))
        })
        .map(|(_, section)| section)
        .or_else(|| found.first())
}

/// Whether the file's newest end is its top: each pair of neighbouring headings with an explicit
/// date votes for the way its times move; the top wins ties and files with no vote.
fn newest_first(found: &[Section], written: i64) -> bool {
    let stamps: Vec<i64> = found
        .iter()
        .filter(|section| date_in(&section.heading, written).is_some())
        .filter_map(|section| heading_time(&section.heading, written))
        .collect();
    let down = stamps.windows(2).filter(|pair| pair[1] > pair[0]).count();
    let up = stamps.windows(2).filter(|pair| pair[1] < pair[0]).count();
    down <= up
}

fn is_open_request(section: &Section, config: &StatusConfig) -> bool {
    asks_coordinator(section, config) && !is_answered(&section.heading, config)
}

/// Whether a heading carries an answered word: see the module documentation.
fn is_answered(heading: &str, config: &StatusConfig) -> bool {
    bracketed_answer(heading, config).is_some() || leading_answer(heading, config)
}

fn leading_answer(heading: &str, config: &StatusConfig) -> bool {
    let rest = strip_time_prefix(heading);
    config
        .ack_words
        .iter()
        .any(|word| starts_with_word(rest, word))
}

/// Where the first bracketed answered word of `text` starts.
fn bracketed_answer(text: &str, config: &StatusConfig) -> Option<usize> {
    text.char_indices()
        .filter(|(_, character)| matches!(character, '[' | '('))
        .find(|(at, character)| {
            let rest = text[at + character.len_utf8()..].trim_start();
            config
                .bracketed_ack_words
                .iter()
                .any(|word| starts_with_word(rest, word))
        })
        .map(|(at, _)| at)
}

/// Whether `text` starts with `word`, in any letter case, as a whole word.
fn starts_with_word(text: &str, word: &str) -> bool {
    text.get(..word.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(word))
        && !text[word.len()..]
            .chars()
            .next()
            .is_some_and(|next| next.is_alphanumeric() || next == '_')
}

/// Whether the heading or text of `section` carries a live marker: see the module documentation.
fn asks_coordinator(section: &Section, config: &StatusConfig) -> bool {
    let text = format!("{}\n{}", section.heading, section.body);
    text.lines().any(|line| live_marker_in(line, config))
}

fn live_marker_in(line: &str, config: &StatusConfig) -> bool {
    let marker = config.marker.as_str();
    let lower = line.to_lowercase();
    let lower_marker = marker.to_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(&lower_marker) {
        let at = from + found;
        from = at + lower_marker.len();
        if !line.is_char_boundary(at) || !line[at..].starts_with(marker) {
            // The marker counts only in the case it is written in; a lowercase mention is prose.
            continue;
        }
        let before = line[..at].trim_end().to_lowercase();
        let before = before.strip_suffix(':').unwrap_or(&before);
        if before.strip_suffix("was").is_some_and(|rest| {
            !rest
                .chars()
                .next_back()
                .is_some_and(|previous| previous.is_alphanumeric() || previous == '_')
        }) {
            continue;
        }
        let rest = &line[at + marker.len()..];
        let after = rest.trim_start().to_lowercase();
        let mentioned = rest.starts_with(char::is_whitespace)
            && [
                "heading", "line", "section", "block", "tag", "marker", "come", "follow", "when",
                "once", "only", "after", "outside",
            ]
            .iter()
            .any(|word| {
                after.starts_with(word)
                    && after[word.len()..]
                        .trim_start_matches('s')
                        .chars()
                        .next()
                        .is_none_or(|next| !next.is_alphanumeric())
            });
        if mentioned {
            continue;
        }
        if bracketed_answer(&line[..at], config).is_some() {
            continue;
        }
        return true;
    }
    false
}

/// `heading` without its leading timestamp and the separators around it.
fn strip_time_prefix(heading: &str) -> &str {
    let mut rest = heading;
    loop {
        let skipped = prefix_token(rest);
        if skipped == 0 {
            return rest;
        }
        rest = &rest[skipped..];
    }
}

/// The length of the timestamp token at the start of `text`, or 0.
fn prefix_token(text: &str) -> usize {
    let first = text.chars().next();
    let Some(first) = first else { return 0 };
    if first.is_whitespace() {
        return text.len() - text.trim_start().len();
    }
    let separators = "()[],:;|*_~—–-";
    if separators.contains(first) {
        return text
            .char_indices()
            .find(|(_, character)| !separators.contains(*character))
            .map_or(text.len(), |(at, _)| at);
    }
    if let Some(length) = iso_date_at(text).map(|(_, length)| length) {
        return length;
    }
    if let Some(length) = clock_length(text) {
        return length;
    }
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=2).contains(&digits) {
        let mut length = digits;
        for suffix in ["st", "nd", "rd", "th"] {
            if text[length..].starts_with(suffix) {
                length += 2;
                break;
            }
        }
        if !text[length..]
            .chars()
            .next()
            .is_some_and(|next| next.is_alphanumeric() || next == '_' || next == ':')
        {
            return length;
        }
        return 0;
    }
    for word in ["AM", "PM", "am", "pm"]
        .into_iter()
        .chain(ZONES.iter().map(|(name, _)| *name))
    {
        if text.starts_with(word) && !next_is_letter(&text[word.len()..]) {
            return word.len();
        }
    }
    for names in [&WEEKDAYS[..], &MONTHS[..]] {
        for name in names {
            if text.starts_with(name) {
                let mut length = name.len();
                length += text[length..]
                    .bytes()
                    .take_while(u8::is_ascii_lowercase)
                    .count();
                if text[length..].starts_with('.') {
                    length += 1;
                }
                if !next_is_letter(&text[length..]) {
                    return length;
                }
            }
        }
    }
    0
}

fn next_is_letter(text: &str) -> bool {
    text.chars().next().is_some_and(char::is_alphabetic)
}

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A US time zone family, or UTC: its standard offset from UTC in hours, and whether it keeps
/// daylight saving time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Zone {
    standard_hours: i64,
    daylight: bool,
}

const EASTERN: Zone = Zone {
    standard_hours: -5,
    daylight: true,
};
const CENTRAL: Zone = Zone {
    standard_hours: -6,
    daylight: true,
};
const PACIFIC: Zone = Zone {
    standard_hours: -8,
    daylight: true,
};
const UTC: Zone = Zone {
    standard_hours: 0,
    daylight: false,
};
/// Times shown to the owner are Eastern, so a time with no zone is read as Eastern.
const DEFAULT_ZONE: Zone = EASTERN;

/// Zone spellings, longest first so `EDT` is not read as a prefix of something longer.
const ZONES: [(&str, Zone); 12] = [
    ("EDT", EASTERN),
    ("EST", EASTERN),
    ("PDT", PACIFIC),
    ("PST", PACIFIC),
    ("CDT", CENTRAL),
    ("CST", CENTRAL),
    ("UTC", UTC),
    ("GMT", UTC),
    ("ET", EASTERN),
    ("PT", PACIFIC),
    ("CT", CENTRAL),
    ("Z", UTC),
];

/// A time of day found in a heading.
#[derive(Clone, Copy, Debug)]
struct Clock {
    seconds: i64,
    zone: Option<Zone>,
}

/// The length of the `H:MM[:SS]` at the start of `text`, whether or not it is a valid time.
fn clock_length(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let digits_at = |at: usize| {
        bytes
            .get(at..at + 2)
            .is_some_and(|two| two.iter().all(u8::is_ascii_digit))
    };
    let hour_digits = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if !(1..=2).contains(&hour_digits) || bytes.get(hour_digits) != Some(&b':') {
        return None;
    }
    if !digits_at(hour_digits + 1) {
        return None;
    }
    let mut length = hour_digits + 3;
    if bytes.get(length) == Some(&b':') && digits_at(length + 1) {
        length += 3;
    }
    (!bytes
        .get(length)
        .is_some_and(|next| next.is_ascii_digit() || *next == b':'))
    .then_some(length)
}

/// The time of day at the start of `text`, `H:MM[:SS]`, an optional AM or PM and an optional
/// zone; `None` when `text` does not start with a valid one.
fn clock_at(text: &str) -> Option<Clock> {
    let length = clock_length(text)?;
    let hour_digits = text.bytes().take_while(u8::is_ascii_digit).count();
    let mut hour = text[..hour_digits].parse::<i64>().ok()?;
    let minute = text[hour_digits + 1..hour_digits + 3].parse::<i64>().ok()?;
    let second = if length > hour_digits + 3 {
        text[hour_digits + 4..length].parse::<i64>().ok()?
    } else {
        0
    };
    let mut rest = &text[length..];
    let spaced = rest.trim_start();
    let mut meridiem = None;
    if let Some(after_noon) = match spaced.as_bytes().first() {
        Some(b'a' | b'A') => Some(false),
        Some(b'p' | b'P') => Some(true),
        _ => None,
    } {
        // `PM`, `p.m.`, `pm.`: a letter, an optional dot, `m`, an optional dot.
        let tail = &spaced[1..];
        let tail = tail.strip_prefix('.').unwrap_or(tail);
        if let Some(tail) = tail.strip_prefix(['m', 'M']) {
            meridiem = Some(after_noon);
            rest = tail.strip_prefix('.').unwrap_or(tail);
        }
    }
    let spaced = rest.trim_start();
    let zone = ZONES
        .iter()
        .find(|(name, _)| spaced.starts_with(name) && !next_is_letter(&spaced[name.len()..]))
        .map(|(_, zone)| *zone);
    if let Some(after_noon) = meridiem {
        if !(1..=12).contains(&hour) {
            return None;
        }
        hour = hour % 12 + if after_noon { 12 } else { 0 };
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(Clock {
        seconds: hour * 3600 + minute * 60 + second,
        zone,
    })
}

/// Every time of day in `heading`, with where it starts.
fn clocks_in(heading: &str) -> Vec<(usize, Clock)> {
    let mut found = Vec::new();
    for (at, _) in heading.char_indices() {
        let preceded = heading[..at]
            .chars()
            .next_back()
            .is_some_and(|previous| previous.is_ascii_digit() || previous == ':');
        if preceded {
            continue;
        }
        if let Some(clock) = clock_at(&heading[at..]) {
            found.push((at, clock));
        }
    }
    found
}

/// A calendar date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Date {
    year: i64,
    month: i64,
    day: i64,
}

/// A `YYYY-MM-DD` date at the start of `text`, with its length.
fn iso_date_at(text: &str) -> Option<(Date, usize)> {
    iso_shape_at(text)
        .filter(|date| valid(*date))
        .map(|date| (date, 10))
}

/// The `YYYY-MM-DD` at the start of `text`, valid or not.
fn iso_shape_at(text: &str) -> Option<Date> {
    let bytes = text.as_bytes();
    if bytes.len() < 10
        || !bytes[..4].iter().all(u8::is_ascii_digit)
        || bytes[4] != b'-'
        || !bytes[5..7].iter().all(u8::is_ascii_digit)
        || bytes[7] != b'-'
        || !bytes[8..10].iter().all(u8::is_ascii_digit)
        || bytes.get(10).is_some_and(u8::is_ascii_digit)
    {
        return None;
    }
    Some(Date {
        year: text[..4].parse().ok()?,
        month: text[5..7].parse().ok()?,
        day: text[8..10].parse().ok()?,
    })
}

fn valid(date: Date) -> bool {
    (1..=12).contains(&date.month)
        && date.day >= 1
        && date.day <= days_in_month(date.year, date.month)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        _ => 28,
    }
}

/// The first date in `heading`, `YYYY-MM-DD` or `Oct 25[, 2026]`, with where it starts and
/// whether its year was written; a month and day take the year of the file's last write.
fn date_in(heading: &str, written: i64) -> Option<(usize, Date, bool)> {
    let mut candidates = Vec::new();
    for (at, _) in heading.char_indices() {
        if heading[..at]
            .chars()
            .next_back()
            .is_some_and(|previous| previous.is_ascii_digit())
        {
            continue;
        }
        if let Some(date) = iso_shape_at(&heading[at..]) {
            // The first date-shaped text decides, as a reader would take it; an impossible date
            // such as 2026-02-30 is none.
            if valid(date) {
                candidates.push((at, date, true));
            }
            break;
        }
    }
    let written_year = civil_from_days(written.div_euclid(86_400)).year;
    for (at, _) in heading.char_indices() {
        if heading[..at]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric)
        {
            continue;
        }
        let Some(month) = MONTHS
            .iter()
            .position(|name| heading[at..].starts_with(name))
        else {
            continue;
        };
        let mut rest = &heading[at + 3..];
        rest = rest.trim_start_matches(|character: char| character.is_ascii_lowercase());
        rest = rest.strip_prefix('.').unwrap_or(rest);
        let spaced = rest.trim_start();
        if spaced.len() == rest.len() {
            continue;
        }
        let digits = spaced.bytes().take_while(u8::is_ascii_digit).count();
        if !(1..=2).contains(&digits) {
            continue;
        }
        let Ok(day) = spaced[..digits].parse::<i64>() else {
            continue;
        };
        let mut after = &spaced[digits..];
        for suffix in ["st", "nd", "rd", "th"] {
            if let Some(stripped) = after.strip_prefix(suffix) {
                after = stripped;
                break;
            }
        }
        if after.chars().next().is_some_and(char::is_alphanumeric) {
            continue;
        }
        let after_comma = after.strip_prefix(',').unwrap_or(after);
        let year_text = after_comma.trim_start();
        let year = (year_text.len() < after_comma.len()
            && year_text.len() >= 4
            && year_text.as_bytes()[..4].iter().all(u8::is_ascii_digit)
            && !year_text
                .as_bytes()
                .get(4)
                .is_some_and(u8::is_ascii_alphanumeric))
        .then(|| year_text[..4].parse::<i64>().ok())
        .flatten();
        let date = Date {
            year: year.unwrap_or(written_year),
            month: i64::try_from(month).unwrap_or(0) + 1,
            day,
        };
        if valid(date) {
            candidates.push((at, date, year.is_some()));
        }
        break;
    }
    candidates.into_iter().min_by_key(|(at, _, _)| *at)
}

/// The time a heading gives, in seconds since the Unix epoch, or `None` when it has no time of
/// day. `written`, the file's last write, gives the day of a time with no date and the year of a
/// month and day.
fn heading_time(heading: &str, written: i64) -> Option<i64> {
    let times = clocks_in(heading);
    if times.is_empty() {
        return None;
    }
    let zoned: Vec<(usize, Clock)> = times
        .iter()
        .copied()
        .filter(|(_, clock)| clock.zone.is_some())
        .collect();
    let zoned = if zoned.is_empty() {
        times.clone()
    } else {
        zoned
    };
    let date = date_in(heading, written);
    let chosen = match date {
        // `2026-10-26 00:33 UTC (8:33 PM ET Oct 25)`: the time written next to the date.
        Some((position, _, _)) => *zoned
            .iter()
            .min_by_key(|(at, _)| at.abs_diff(position))
            .expect("zoned times are not empty"),
        None => {
            // A heading that starts with its time uses that one; otherwise the stamp is the
            // trailing one, and an annotation in front is not the heading's time.
            let prefix_end = heading.len() - strip_time_prefix(heading).len();
            let leading: Vec<(usize, Clock)> = times
                .iter()
                .copied()
                .filter(|(at, _)| *at < prefix_end)
                .collect();
            if leading.is_empty() {
                *zoned.last().expect("zoned times are not empty")
            } else {
                leading
                    .iter()
                    .copied()
                    .find(|(_, clock)| clock.zone.is_some())
                    .unwrap_or(leading[0])
            }
        }
    };
    let (_, clock) = chosen;
    let zone = clock.zone.unwrap_or(DEFAULT_ZONE);
    match date {
        None => {
            let local_day = civil_from_days(local_seconds(written, zone).div_euclid(86_400));
            let stamp = to_utc(local_day, clock.seconds, zone);
            if stamp > written + DATELESS_SLACK_SECONDS {
                let previous = civil_from_days(days_from_civil(local_day) - 1);
                Some(to_utc(previous, clock.seconds, zone))
            } else {
                Some(stamp)
            }
        }
        Some((_, date, year_known)) => {
            let stamp = to_utc(date, clock.seconds, zone);
            if !year_known && stamp > written + 86_400 {
                Some(to_utc(
                    Date {
                        year: date.year - 1,
                        ..date
                    },
                    clock.seconds,
                    zone,
                ))
            } else {
                Some(stamp)
            }
        }
    }
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
fn days_from_civil(date: Date) -> i64 {
    let year = if date.month <= 2 {
        date.year - 1
    } else {
        date.year
    };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = (date.month + 9) % 12;
    let day_of_year = (153 * month + 2) / 5 + date.day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The civil date of a day count since 1970-01-01.
fn civil_from_days(days: i64) -> Date {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month + 2) / 5 + 1;
    let month = if month < 10 { month + 3 } else { month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    Date { year, month, day }
}

/// The day of the month of the `nth` Sunday of `month` in `year` (US daylight saving rules).
fn nth_sunday(year: i64, month: i64, nth: i64) -> i64 {
    let first = days_from_civil(Date {
        year,
        month,
        day: 1,
    });
    // 1970-01-01 was a Thursday; Sunday is 3 days later.
    let weekday = (first + 4).rem_euclid(7);
    let first_sunday = 1 + (7 - weekday) % 7;
    first_sunday + 7 * (nth - 1)
}

/// Whether a local wall-clock time in a daylight-saving zone is daylight time: from 03:00 on the
/// second Sunday in March, the first wall-clock time after the gap, to 02:00 on the first Sunday
/// in November, so an ambiguous time is read as daylight, as a first occurrence is.
fn daylight_at_local(date: Date, seconds: i64) -> bool {
    let start = (3, nth_sunday(date.year, 3, 2), 3 * 3600);
    let end = (11, nth_sunday(date.year, 11, 1), 2 * 3600);
    let here = (date.month, date.day, seconds);
    here >= start && here < end
}

/// The UTC time of a local wall-clock time in `zone`.
fn to_utc(date: Date, seconds: i64, zone: Zone) -> i64 {
    let mut offset = zone.standard_hours * 3600;
    if zone.daylight && daylight_at_local(date, seconds) {
        offset += 3600;
    }
    days_from_civil(date) * 86_400 + seconds - offset
}

/// The local wall-clock time, as seconds since the local epoch, of a UTC time in `zone`.
fn local_seconds(utc: i64, zone: Zone) -> i64 {
    let standard = utc + zone.standard_hours * 3600;
    if !zone.daylight {
        return standard;
    }
    let date = civil_from_days(standard.div_euclid(86_400));
    let start = to_utc(
        Date {
            month: 3,
            day: nth_sunday(date.year, 3, 2),
            ..date
        },
        3 * 3600,
        zone,
    );
    let end = to_utc(
        Date {
            month: 11,
            day: nth_sunday(date.year, 11, 1),
            ..date
        },
        3600,
        zone,
    );
    if utc >= start && utc < end + 3600 {
        standard + 3600
    } else {
        standard
    }
}

fn unix_seconds(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |elapsed| {
        i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
    })
}

/// One line, with braces and double quotes replaced, so it can sit inside other text.
fn clean(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('{', "(")
        .replace('}', ")")
        .replace('"', "'")
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(limit.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))[..12].to_owned()
}

/// A watch on the status files' directory, so a new request is paged when it is written rather
/// than at the next pass.
pub struct DirectoryWatch {
    fd: std::os::fd::OwnedFd,
    directory: PathBuf,
}

/// How long a wait for a status file change sleeps between looks at whether the watcher was
/// asked to stop.
const WATCH_SLICE: std::time::Duration = std::time::Duration::from_millis(100);
/// After a change, how long further changes are gathered, so one save that writes a file in
/// several steps is read once, complete.
const WATCH_SETTLE: std::time::Duration = std::time::Duration::from_millis(200);

impl DirectoryWatch {
    /// Watch `directory` for files written, created or moved in.
    ///
    /// # Errors
    ///
    /// The directory does not exist, or the kernel refused the watch.
    pub fn new(directory: &Path) -> std::io::Result<Self> {
        use std::os::fd::FromRawFd;
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(directory.as_os_str().as_bytes())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        // SAFETY: inotify_init1 takes flags only.
        let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if raw < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `raw` is a new descriptor that nothing else owns.
        let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) };
        let mask = libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO | libc::IN_CREATE | libc::IN_ONLYDIR;
        // SAFETY: the descriptor is open and the path is a NUL-terminated string.
        let added = unsafe {
            libc::inotify_add_watch(std::os::fd::AsRawFd::as_raw_fd(&fd), path.as_ptr(), mask)
        };
        if added < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self {
            fd,
            directory: directory.to_owned(),
        })
    }

    /// The directory watched.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Wait up to `timeout` for a change; `true` when one came. The wait ends early, `false`,
    /// once `stop` says so.
    pub fn wait(&self, timeout: std::time::Duration, stop: &dyn Fn() -> bool) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if stop() {
                return false;
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return false;
            }
            if self.poll(left.min(WATCH_SLICE)) && self.drain() {
                // Gather the rest of the save before anyone reads the file.
                while self.poll(WATCH_SETTLE) && self.drain() {}
                return true;
            }
        }
    }

    fn poll(&self, timeout: std::time::Duration) -> bool {
        let mut entry = libc::pollfd {
            fd: std::os::fd::AsRawFd::as_raw_fd(&self.fd),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = libc::c_int::try_from(timeout.as_millis()).unwrap_or(libc::c_int::MAX);
        // SAFETY: one valid pollfd, and its count.
        let ready = unsafe { libc::poll(&raw mut entry, 1, millis) };
        ready > 0 && entry.revents & libc::POLLIN != 0
    }

    /// Read every pending event; whether there was any.
    fn drain(&self) -> bool {
        let mut buffer = [0_u8; 4096];
        let mut any = false;
        loop {
            // SAFETY: the buffer is valid for its length, and the descriptor is open.
            let read = unsafe {
                libc::read(
                    std::os::fd::AsRawFd::as_raw_fd(&self.fd),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                )
            };
            if read <= 0 {
                return any;
            }
            any = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> StatusConfig {
        serde_json::from_str(r#"{"status_files": "temp/{agent}-status.md"}"#).expect("config")
    }

    /// Seconds since the epoch of a UTC time written as `YYYY-MM-DD HH:MM`.
    fn utc(text: &str) -> i64 {
        let (date, _) = iso_date_at(text).expect("date");
        let clock = clock_at(&text[11..]).expect("time");
        days_from_civil(date) * 86_400 + clock.seconds
    }

    fn section(heading: &str, body: &str) -> Section {
        Section {
            index: 0,
            line: 1,
            heading: heading.to_owned(),
            body: body.to_owned(),
        }
    }

    #[test]
    fn a_heading_or_text_with_the_marker_is_open_until_an_answered_word_marks_it() {
        let config = config();
        let open = |heading: &str, body: &str| is_open_request(&section(heading, body), &config);
        assert!(open("Land A0 please COORDINATOR_RESPOND_PLEASE", ""));
        assert!(open("Land A0", "COORDINATOR_RESPOND_PLEASE"));
        assert!(!open("Land A0", "nothing asked"));
        for answered in [
            "ANSWERED: Land A0 COORDINATOR_RESPOND_PLEASE",
            "LANDED as 856eaf1e COORDINATOR_RESPOND_PLEASE",
            "07:40 AM EDT Sun 2026-10-25 — LANDED (COORDINATOR_RESPOND_PLEASE)",
            "(1:37 AM ET Oct 15): ANSWERED COORDINATOR_RESPOND_PLEASE",
            "Land A0 COORDINATOR_RESPOND_PLEASE [answered: yes]",
            "(ANSWERED, coordinator) Land A0 COORDINATOR_RESPOND_PLEASE",
            "SUPERSEDED (coordinator 8:35 PM ET) COORDINATOR_RESPOND_PLEASE",
        ] {
            assert!(!open(answered, ""), "{answered}");
        }
        // Agents write `(not landed)` themselves; only a leading `landed` answers.
        assert!(open("Land A0 (not landed) COORDINATOR_RESPOND_PLEASE", ""));
        // A marker kept as history, or named in prose, is no request.
        assert!(!open("Land A0", "Was: COORDINATOR_RESPOND_PLEASE"));
        assert!(!open(
            "Plan",
            "the SHAs come back as a TOP COORDINATOR_RESPOND_PLEASE heading"
        ));
        assert!(!open(
            "Plan",
            "I hand back with COORDINATOR_RESPOND_PLEASE when both verdicts arrive"
        ));
        assert!(!open(
            "Plan",
            "(ANSWERED, coordinator) COORDINATOR_RESPOND_PLEASE for A0 was handled"
        ));
    }

    #[test]
    fn headings_seen_in_real_status_files_read_as_answered_or_open() {
        let config = config();
        for answered in [
            "07:40 AM EDT Sun 2026-10-25 — LANDED (e2f08ae2 under ffbcf012): slice 4 hand-back",
            "03:14 PM EDT Sat 2026-10-24 — [answered: 1 = C defer, plan B recorded]",
            "(ANSWERED, OWNER: option A) slice P design done (12:19 UTC, 8:19 AM ET)",
            "10:06 AM ET — msg1449 HAND-BACK: refused-validate publication fix. [answered]",
            "2026-10-16 2:28 AM ET — ACK GitHub budget window",
            "ACKNOWLEDGED: the plan",
            "2026-10-25 1:40 AM EDT: SUPERSEDED by the sub-slice A hand-back above",
            "2026-10-17 1:07 AM ET — DECIDED (coord 1:12 AM: go ahead as written)",
            "(LANDED as main e9774006, fast-forward) package K slice F0a ready to land",
            "(10:38 PM ET Oct 16): superseded by TOP (owner decided)",
        ] {
            assert!(is_answered(answered, &config), "{answered}");
        }
        for open in [
            "19:12 EDT — DONE. COORDINATOR_RESPOND_PLEASE",
            "2026-10-17 07:34 PM EDT: COORDINATOR_RESPOND_PLEASE: ack prep 1-4 landed",
            "2026-10-15 1:42 AM ET — pressure-test series.py fix: NOT landed yet (no SHA)",
            "04:33 PM ET: FF14 written and pushed for review (not landed)",
            "ANSWEREDX is not the word",
        ] {
            assert!(!is_answered(open, &config), "{open}");
        }
        let asks = |text: &str| asks_coordinator(&section(text, ""), &config);
        for mention in [
            "No reply needed yet: COORDINATOR_RESPOND_PLEASE comes with the reviewed head",
            "msg1350 answered all three COORDINATOR_RESPOND_PLEASE lines in the block below",
            "I will post COORDINATOR_RESPOND_PLEASE only after the merged-tree checks",
            "a lowercase coordinator_respond_please is prose",
        ] {
            assert!(!asks(mention), "{mention}");
        }
        for raised in [
            "D4c-1 LANDED; COORDINATOR_RESPOND_PLEASE on the Track B question below.",
            "COORDINATOR_RESPOND_PLEASE HAND-BACK: slice 2",
            "COORDINATOR_RESPOND_PLEASE: heads-up",
        ] {
            assert!(asks(raised), "{raised}");
        }
        // A real request beside a prose mention is still a request.
        assert!(asks_coordinator(
            &section(
                "COORDINATOR_RESPOND_PLEASE: land F1",
                "its COORDINATOR_RESPOND_PLEASE line"
            ),
            &config
        ));
    }

    #[test]
    fn heading_time_formats_seen_in_real_status_files() {
        let written = utc("2026-10-26 01:00");
        for (heading, expected) in [
            ("07:23 PM EDT Sun 2026-10-25 — ask", "2026-10-25 23:23"),
            ("2026-10-25 04:24 PM EDT ready", "2026-10-25 20:24"),
            (
                "2026-10-25 09:59 ET — scope document ready",
                "2026-10-25 13:59",
            ),
            ("2026-10-14 11:42 PT — LANDED", "2026-10-14 18:42"),
            (
                "(10:25 PM ET Oct 15): overnight row acknowledged",
                "2026-10-16 02:25",
            ),
            (
                "canonical test result — Oct 17, 4:35 PM ET",
                "2026-10-17 20:35",
            ),
            (
                "01:31 PM ET — a dateless time-first heading",
                "2026-10-25 17:31",
            ),
            // An annotation in front is not the heading's time; the trailing stamp is.
            (
                "SUPERSEDED (coordinator 8:35 PM ET: see above) ask (00:10 UTC)",
                "2026-10-26 00:10",
            ),
            // Later in the day than the file's last write: the day before.
            ("11:50 PM ET — late", "2026-10-25 03:50"),
            ("12:00 AM ET midnight", "2026-10-25 04:00"),
        ] {
            assert_eq!(
                heading_time(heading, written),
                Some(utc(expected)),
                "{heading}"
            );
        }
        let seconds = heading_time("A0 handoff — 2026-10-24 19:41:35 EDT", written);
        assert_eq!(seconds, Some(utc("2026-10-24 23:41") + 35));
        // Daylight saving time: the first 01:30 of 2026-11-01 is daylight time, and 02:30 on
        // 2026-03-08, which never happened, reads as standard time.
        assert_eq!(
            heading_time("2026-11-01 01:30 ET", written),
            Some(utc("2026-11-01 05:30"))
        );
        assert_eq!(
            heading_time("2026-03-08 02:30 ET", written),
            Some(utc("2026-03-08 07:30"))
        );
        assert_eq!(heading_time("13:00 PM ET is no time", written), None);
        assert_eq!(
            heading_time("2026-02-30 10:00 UTC", written),
            Some(utc("2026-10-25 10:00"))
        );
    }

    #[test]
    fn heading_times_read_eastern_pacific_and_utc_spellings() {
        let written = utc("2026-10-26 12:00");
        // Daylight time: Eastern is UTC-4 on 2026-10-25.
        assert_eq!(
            heading_time("07:40 AM EDT Sun 2026-10-25 — LANDED", written),
            Some(utc("2026-10-25 11:40"))
        );
        assert_eq!(
            heading_time("2026-10-26 00:33 UTC (8:33 PM ET Oct 25)", written),
            Some(utc("2026-10-26 00:33"))
        );
        assert_eq!(
            heading_time("Status (Oct 25, 9:15 PM PT)", written),
            Some(utc("2026-10-26 04:15"))
        );
        // Standard time after 2026-11-01: Eastern is UTC-5.
        assert_eq!(
            heading_time("2026-11-04 9:00 PM ET hand-back", utc("2026-11-05 03:00")),
            Some(utc("2026-11-05 02:00"))
        );
        // A time with no date goes on the latest day that is not after the file's write.
        assert_eq!(
            heading_time("hand-back (23:02 UTC)", utc("2026-10-26 10:00")),
            Some(utc("2026-10-25 23:02"))
        );
        assert_eq!(heading_time("no time here", written), None);
    }

    #[test]
    fn the_newest_sections_are_read_whichever_end_is_newest() {
        let config = config();
        let top_newest = "# 2026-10-26 10:00 ET newest COORDINATOR_RESPOND_PLEASE\n\
                          # 2026-10-26 09:00 ET\n# 2026-10-26 08:00 ET\n\
                          # 2026-10-26 07:00 ET\n# 2026-10-26 06:00 ET old COORDINATOR_RESPOND_PLEASE\n";
        let found = sections(top_newest);
        let written = utc("2026-10-26 15:00");
        let open: Vec<usize> = open_requests(&found, written, &config)
            .iter()
            .map(|(section, _)| section.line)
            .collect();
        assert_eq!(open, [1], "only the newest end is read");
        let bottom_newest = "# 2026-10-26 06:00 ET\n# 2026-10-26 07:00 ET\n\
                             # 2026-10-26 08:00 ET\n\
                             # 2026-10-26 09:00 ET ask COORDINATOR_RESPOND_PLEASE\n";
        let found = sections(bottom_newest);
        assert_eq!(
            open_requests(&found, written, &config)
                .iter()
                .map(|(section, _)| section.line)
                .collect::<Vec<_>>(),
            [4]
        );
        // A fenced heading is not a section.
        assert_eq!(sections("```\n# not a heading\n```\n## real\n").len(), 1);
    }

    #[test]
    fn the_waiting_list_names_each_agent_never_a_bare_count() {
        let request = |agent: &str, age: u64, heading: &str| OpenRequest {
            agent: agent.to_owned(),
            path: PathBuf::from(format!("temp/{agent}-status.md")),
            heading: heading.to_owned(),
            heading_hash: digest(heading),
            line: 1,
            age_minutes: age,
            age_source: "heading",
        };
        assert_eq!(waiting_line(&[], true), "0 waiting");
        let list = [
            request("lane-a", 12, "land A0"),
            request("lane-b", 4, "decide B"),
            request("lane-a", 3, "and A1"),
        ];
        assert_eq!(
            waiting_line(&list, true),
            "3 more waiting: lane-a 12m (land A0, +1 more), lane-b 4m (decide B)"
        );
        assert_eq!(
            waiting_line(&list[1..2], false),
            "1 waiting: lane-b 4m (decide B)"
        );
    }

    #[test]
    fn a_status_file_is_found_per_agent_and_aged_from_its_heading() {
        let root = std::env::temp_dir().join(format!("status-requests-{}", std::process::id()));
        let registry = root.join(".agentctl");
        fs::create_dir_all(root.join("temp")).expect("status directory");
        let config = config();
        let path = config.status_file(&registry, "lane");
        assert_eq!(path, root.join("temp/lane-status.md"));
        assert_eq!(config.directory(&registry), root.join("temp"));
        fs::write(
            &path,
            "## 2026-11-05 01:00 ET land it COORDINATOR_RESPOND_PLEASE\nbody\n## older\n",
        )
        .expect("status file");
        let now = UNIX_EPOCH + std::time::Duration::from_secs(utc("2026-11-05 06:30") as u64);
        let found = requests_in("lane", &path, &config, now);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].age_minutes, 30);
        assert_eq!(found[0].age_source, "heading");
        assert_eq!(found[0].line, 1);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn pages_go_once_per_request_again_after_the_interval_and_stop_once_answered() {
        let request = OpenRequest {
            agent: "lane".to_owned(),
            path: PathBuf::from("temp/lane-status.md"),
            heading: "land it COORDINATOR_RESPOND_PLEASE".to_owned(),
            heading_hash: digest("land it COORDINATOR_RESPOND_PLEASE"),
            line: 1,
            age_minutes: 0,
            age_source: "heading",
        };
        let open = vec![request.clone()];
        let mut records = BTreeMap::new();
        let none = BTreeSet::new();
        let due = due_pages(&open, &mut records, 1000.0, 1800.0, &none);
        assert_eq!(due, [(request.clone(), PageReason::New)]);
        record_page(&mut records, &request, 1000.0);
        // The same request again, read twice in a row, is not paged twice.
        assert!(due_pages(&open, &mut records, 1001.0, 1800.0, &none).is_empty());
        assert!(due_pages(&open, &mut records, 2799.0, 1800.0, &none).is_empty());
        assert_eq!(
            due_pages(&open, &mut records, 2800.0, 1800.0, &none),
            [(request.clone(), PageReason::Repage)]
        );
        // The agent going idle pages at once, but not right after a page.
        let idle = BTreeSet::from(["lane".to_owned()]);
        assert!(due_pages(&open, &mut records, 1050.0, 1800.0, &idle).is_empty());
        assert_eq!(
            due_pages(&open, &mut records, 1200.0, 1800.0, &idle),
            [(request.clone(), PageReason::Idle)]
        );
        // Answered: the request leaves the open list, and its record goes with it.
        assert!(due_pages(&[], &mut records, 5000.0, 1800.0, &idle).is_empty());
        assert!(records.is_empty());
    }

    #[test]
    fn a_directory_watch_wakes_for_a_write_and_not_otherwise() {
        let root = std::env::temp_dir().join(format!("status-watch-{}", std::process::id()));
        fs::create_dir_all(&root).expect("directory");
        let watch = DirectoryWatch::new(&root).expect("watch");
        let short = std::time::Duration::from_millis(150);
        assert!(!watch.wait(short, &|| false));
        fs::write(root.join("lane-status.md"), "# hello\n").expect("write");
        assert!(watch.wait(std::time::Duration::from_secs(5), &|| false));
        assert!(!watch.wait(short, &|| false), "one write wakes once");
        fs::write(root.join("lane-status.md"), "# again\n").expect("write");
        assert!(
            !watch.wait(std::time::Duration::from_secs(5), &|| true),
            "a stop ends the wait"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn a_configuration_must_name_the_agent_once_in_a_relative_file_name() {
        for bad in [
            r#"{"status_files": "temp/status.md"}"#,
            r#"{"status_files": "{agent}/status.md"}"#,
            r#"{"status_files": "/abs/{agent}.md"}"#,
            r#"{"status_files": "../{agent}.md"}"#,
            r#"{"status_files": "temp/{agent}.md", "repage_minutes": 0}"#,
        ] {
            let config: StatusConfig = serde_json::from_str(bad).expect("parses");
            assert!(config.validate().is_err(), "{bad}");
        }
        assert!(serde_json::from_str::<StatusConfig>(
            r#"{"status_files": "x/{agent}", "extra": 1}"#
        )
        .is_err());
        config().validate().expect("valid");
    }
}
