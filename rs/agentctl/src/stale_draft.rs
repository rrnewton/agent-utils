//! Telling a draft agentctl typed from one a person is typing, for the stale-draft recovery
//! (#720 agentctl-muse-stale-draft).
//!
//! A delivery proves the editor empty before it types, so a draft left in an agent's editor holds
//! every later prompt, the owner's included, for as long as it stays. Every harness agentctl
//! drives can leave one there after agentctl's proof of delivery. Muse shows a prompt steered
//! into a running turn at once as a user turn, and when the steer later fails (as it does around
//! a context compaction) puts the prompt back in the editor, unsent. Codex moves the inputs it
//! held for a running turn back into its composer when that turn is interrupted. Any harness can
//! miss a submission key. Nothing else will ever submit such a draft.
//!
//! A delivery that finds a draft therefore asks whether agentctl typed it: the draft must match,
//! as the harness draws it, the text of one of the last [`RECENT_DELIVERIES`] prompts this
//! agent's queue records as typed (in `processed/` or `failed/`; `inflight/` holds the prompt
//! being delivered now, and a prompt quarantined as typed into the wrong program never counts),
//! or several of them in the order they were typed, and each must open with a sender header (or
//! be a batch of queued messages, which do). Only such a draft is submitted. Any other draft may
//! be a person typing, so it is never submitted or cleared, only reported. A prompt a harness
//! puts back comes back soon after it was typed, and every later delivery is refused while it is
//! there, so it is among the last few; an older prompt a person recalled into the editor with
//! the harness's history keys is not.
//!
//! The newest prompt is a different matter where Up on an empty composer recalls it, as it does
//! in Claude Code and Codex, which never take back a prompt they showed as taken. There
//! ([`Vouching::Unproven`]) one prompt vouches for a draft only when agentctl quarantined it in
//! `failed/`, not proved delivered (a prompt still staged when its key was given up on, say); a
//! run of two or more, which the history keys do not recall, may also be prompts it delivered,
//! as Codex moves the inputs it held for a turn back into the composer when the turn is
//! interrupted. Muse ([`Vouching::Recent`]) can take a steer back after it showed it, so any of
//! the latest prompts vouches there.
//!
//! A harness draws a long paste as one placeholder, which shows only the paste's size: Muse and
//! Codex draw `[Pasted Content N chars]`, Claude Code (and Codex for a paste of many lines)
//! `[Pasted text #K +M lines]`. A character-count draft matches only the very last prompt
//! typed, of exactly that size, within [`PLACEHOLDER_LOOKBACK`], a shorter window than
//! [`TEXT_LOOKBACK`] for a draft whose text shows. A numbered draft matches, among the last
//! [`RECENT_DELIVERIES`] prompts, the one holding its exact recorded `paste_label` (#808),
//! or, when a record holds no label (its paste never showed in the composer, as on a busy
//! pane), one whose size the one shared line-count matcher accepts; looking only at the
//! latest delivery stranded an older delivery's chip behind every later prompt (#884
//! follow-up busy-evidence). A
//! composer holding several returned prompts at once — concatenated placeholders, or a
//! placeholder beside the literal start of another delivery — is agentctl's when every piece
//! matches a different recent delivery that way (#825 agentctl-stranded-multi-paste); any
//! piece that matches nothing makes the whole draft unattributed.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};

/// How far back a draft whose text shows is looked for among the prompts agentctl typed.
pub(crate) const TEXT_LOOKBACK: Duration = Duration::from_secs(48 * 3600);
/// How far back a draft that shows only a paste placeholder is looked for.
pub(crate) const PLACEHOLDER_LOOKBACK: Duration = Duration::from_secs(6 * 3600);
/// The most queue records one lookup reads, most recently changed first.
const MAX_RECORDS: usize = 200;
/// How many of the latest deliveries (a batch is one) a draft whose text shows may be.
pub(crate) const RECENT_DELIVERIES: usize = 5;
/// The queue phases that hold prompts agentctl typed, or may have typed, before the one being
/// delivered now.
const TYPED_PHASES: [&str; 2] = ["processed", "failed"];

/// Which typed prompts may vouch for a draft: see the module documentation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Vouching {
    /// Any of the latest prompts typed, delivered or quarantined (Muse).
    Recent,
    /// One prompt only when quarantined, not proved delivered; a run of them however recorded
    /// (Claude Code, Codex).
    Unproven,
}

/// A draft in an agent's editor, as the screen shows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Draft {
    /// The draft's rows, one per line, as drawn.
    Text(String),
    /// `[Pasted Content N chars]`, Muse's and Codex's placeholder for a long paste: only its
    /// length in characters shows.
    Placeholder(usize),
    /// `[Pasted text #K +M lines]`, Claude Code's (and Codex's) placeholder for a paste of many
    /// lines, without its blanks. It matches a recent prompt agentctl typed whose recorded
    /// `paste_label` is this label, or, when the record holds no label, whose size the
    /// shared line-count matcher accepts for the label's `+M` count (#884 follow-up
    /// busy-evidence).
    PastedLines(String),
}

impl Draft {
    /// The draft a composer holds, given its text without the prompt marker: a paste
    /// placeholder when that is all it holds, its text otherwise.
    pub(crate) fn from_composer(text: &str) -> Self {
        let text = text.trim();
        let number = |value: &str| {
            (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| value.parse::<usize>().ok())
                .flatten()
        };
        if let Some(length) = text
            .strip_prefix("[Pasted Content ")
            .and_then(|rest| rest.strip_suffix(" chars]"))
            .and_then(number)
        {
            return Self::Placeholder(length);
        }
        let numbered = text
            .strip_prefix("[Pasted text #")
            .and_then(|rest| rest.strip_suffix(" lines]"))
            .and_then(|rest| rest.split_once(" +"))
            .is_some_and(|(paste, lines)| number(paste).is_some() && number(lines).is_some());
        if numbered {
            Self::PastedLines(compact(text))
        } else {
            Self::Text(text.to_owned())
        }
    }

    /// The draft in words, without its text: `412 characters on 6 rows`, `a pasted 1635
    /// characters` or `a pasted 12 lines`. A draft a person typed is never quoted in a log or a
    /// notice.
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Text(text) => format!(
                "{} characters on {} row(s)",
                text.chars().count(),
                text.lines().count().max(1)
            ),
            Self::Placeholder(length) => format!("a pasted {length} characters"),
            Self::PastedLines(label) => {
                let breaks = label
                    .rsplit_once('+')
                    .and_then(|(_, rest)| rest.strip_suffix("lines]"))
                    .and_then(|count| count.parse::<usize>().ok())
                    .unwrap_or_default();
                format!("a pasted {} lines", breaks + 1)
            }
        }
    }
}

/// A prompt agentctl typed whose text matches a draft, or the last of several typed one after
/// another that together match it.
#[derive(Clone, Debug)]
pub(crate) struct TypedPrompt {
    /// The queue message ID.
    pub id: String,
    /// The record's path, in the phase directory that holds it.
    pub path: PathBuf,
    /// The other prompts the draft holds, typed before this one, oldest first: their IDs and
    /// record paths.
    pub others: Vec<(String, PathBuf)>,
    /// The sender its header names, when it opens with one.
    pub sender: Option<String>,
    /// When it was typed (`inflight_at`), in Unix seconds.
    pub typed_at: Option<f64>,
}

impl TypedPrompt {
    /// `typed by agentctl for tickhub-ops-2 at 06:45Z (message 0179...)`.
    pub(crate) fn describe(&self) -> String {
        let at = self.typed_at.map_or(String::new(), |seconds| {
            let millis = (seconds.max(0.0) * 1000.0) as u64;
            let clock = crate::inbox::utc_clock(millis);
            format!(" at {}Z", clock.get(..5).unwrap_or(&clock))
        });
        let sender = self.sender.as_deref().unwrap_or("an unnamed sender");
        if self.others.is_empty() {
            return format!("typed by agentctl for {sender}{at} (message {})", self.id);
        }
        let ids: Vec<&str> = self
            .others
            .iter()
            .map(|(id, _)| id.as_str())
            .chain([self.id.as_str()])
            .collect();
        format!(
            "{} prompts typed by agentctl, the last for {sender}{at} (messages {})",
            ids.len(),
            ids.join(", ")
        )
    }
}

// Python's `str.isspace`, as the Muse editor proofs use it.
fn is_space(value: char) -> bool {
    value.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&value)
}

/// `text` with every whitespace character removed. Muse wraps a long line at any character,
/// inside a word as well as between words, and the screen does not say which it did, so a
/// draft is compared with its whitespace gone.
fn compact(text: &str) -> String {
    text.chars().filter(|value| !is_space(*value)).collect()
}

/// A compacted text with the `sent:` field (whose stated delay depends on when the prompt was
/// typed) taken out of every sender header in it, each header's other fields kept.
fn without_sent_fields(compacted: &str) -> String {
    let mut out = String::with_capacity(compacted.len());
    let mut rest = compacted;
    while let Some(start) = rest.find("[from") {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(end) = tail.find(']') else {
            out.push_str(tail);
            return out;
        };
        let header = &tail[..end];
        match header.find("·sent:") {
            Some(at) => {
                let after = &header[at + "·sent:".len()..];
                out.push_str(&header[..at]);
                out.push_str(after.find('·').map_or("", |next| &after[next..]));
            }
            None => out.push_str(header),
        }
        out.push(']');
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Whether a recorded prompt is one agentctl framed: its first line is a sender header, or it is
/// a batch of queued messages, each of which opens with one.
fn framed(text: &str) -> bool {
    let first = text.lines().next().unwrap_or_default();
    crate::sender_header::parse(first).is_some()
        || first
            .strip_suffix(" queued messages:")
            .is_some_and(|count| {
                !count.is_empty() && count.bytes().all(|value| value.is_ascii_digit())
            })
}

/// Whether a draft whose text shows is `candidate`, a prompt agentctl recorded typing.
pub(crate) fn text_matches(draft: &str, candidate: &str) -> bool {
    let draft = without_sent_fields(&compact(draft));
    !draft.is_empty() && draft == without_sent_fields(&compact(candidate))
}

/// The shortest draft, in whitespace-free characters, that may count as a
/// fragment of a typed prompt (#731 agentctl-paste-tail-flush): below this a
/// person's own short text is too likely to coincide with words agentctl
/// typed, so it is never submitted on this evidence alone.
pub(crate) const MIN_FRAGMENT_CHARS: usize = 16;

/// Whether a draft whose text shows is a fragment of `candidate`: a suffix
/// or contiguous substring of it, wrapping and whitespace ignored, shorter
/// than the whole prompt (the whole prompt is [`text_matches`]). This is
/// what a split multi-line paste leaves behind: the last line or lines of a
/// message, without its sender header (#731 agentctl-paste-tail-flush).
pub(crate) fn fragment_matches(draft: &str, candidate: &str) -> bool {
    let draft = without_sent_fields(&compact(draft));
    if draft.chars().count() < MIN_FRAGMENT_CHARS {
        return false;
    }
    let candidate = without_sent_fields(&compact(candidate));
    draft.len() < candidate.len() && candidate.contains(&draft)
}

/// The character count Muse and Codex show for a paste of `text`: code points, CRLF as one.
fn paste_length(text: &str) -> usize {
    text.chars().count() - text.matches("\r\n").count()
}

/// Line count a placeholder label shows, for `submission`'s stranded check.
pub(crate) fn pasted_line_count_pub(label: &str) -> Option<usize> {
    pasted_line_count(label)
}

/// Line count a `[Pasted text #K +M lines]` label shows (M = lines - 1).
fn pasted_line_count(label: &str) -> Option<usize> {
    let compacted = compact(label);
    let after = compacted.rsplit_once('+')?.1;
    after.strip_suffix("lines]")?.parse().ok()
}

/// Whether a placeholder draft is the paste of `text` that `document` records.
fn placeholder_matches(draft: &Draft, text: &str, document: &Map<String, Value>) -> bool {
    placeholder_matches_q(draft, text, document, false)
}

fn placeholder_matches_q(
    draft: &Draft,
    text: &str,
    document: &Map<String, Value>,
    _quarantined: bool,
) -> bool {
    match draft {
        Draft::Text(_) => false,
        Draft::Placeholder(length) => paste_length(text) == *length,
        Draft::PastedLines(label) => {
            // The exact recorded label is the strongest proof (#808): a
            // record holding a different label names a different paste. A
            // record holding no label (its paste was never drawn in the
            // composer, as on a busy pane) vouches by size through the one
            // shared line-count matcher, among the recent deliveries, not
            // only the latest (#884 follow-up busy-evidence).
            document.get("paste_label").and_then(Value::as_str) == Some(label.as_str())
                || (document.get("paste_label").is_none()
                    && pasted_line_count(label).is_some_and(|m| {
                        crate::submission::expected_line_counts(text).contains(&m)
                    }))
        }
    }
}

/// The texts a record says were typed: what the drain typed, when it recorded that, and the
/// queued text.
fn typed_texts(document: &Map<String, Value>) -> Vec<&str> {
    ["typed_as", "text"]
        .iter()
        .filter_map(|field| document.get(*field).and_then(Value::as_str))
        .collect()
}

/// The records in `queue`'s typed phases changed within `lookback` of `now`, at most
/// [`MAX_RECORDS`] of them, most recently changed first.
fn recent_records(queue: &Path, now: SystemTime, lookback: Duration) -> Vec<PathBuf> {
    let mut found: Vec<(SystemTime, PathBuf)> = Vec::new();
    for phase in TYPED_PHASES {
        let Ok(entries) = fs::read_dir(queue.join(phase)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
                continue;
            };
            if now.duration_since(modified).unwrap_or_default() <= lookback {
                found.push((modified, path));
            }
        }
    }
    found.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    found.truncate(MAX_RECORDS);
    found.into_iter().map(|(_, path)| path).collect()
}

/// A queue record of a typed prompt: when it was typed, its path, and its document.
type Record = (f64, PathBuf, Map<String, Value>);

/// The prompt agentctl typed into this agent that `draft` is, or the run of prompts it typed one
/// after another that `draft` holds in that order, looked for in the queue at `queue` among the
/// records changed within the draft's lookback of `now`; `None` when agentctl typed no such
/// prompt, so the draft may be a person's.
pub(crate) fn find(
    queue: &Path,
    draft: &Draft,
    vouching: Vouching,
    now: SystemTime,
) -> Option<TypedPrompt> {
    let (lookback, deliveries) = match draft {
        Draft::Text(_) => (TEXT_LOOKBACK, RECENT_DELIVERIES),
        // A numbered chip may be an older delivery's paste returned to the
        // composer, not only the latest one's (#884 follow-up busy-evidence);
        // a bare character count still vouches only for the very last (#720).
        Draft::PastedLines(_) => (PLACEHOLDER_LOOKBACK, RECENT_DELIVERIES),
        Draft::Placeholder(_) => (PLACEHOLDER_LOOKBACK, 1),
    };
    // The records of the latest deliveries, latest first: a batch's members share one
    // `inflight_at`, so they count as one delivery.
    let mut records: Vec<Record> = recent_records(queue, now, lookback)
        .into_iter()
        .filter_map(|path| match crate::agent::read_private_json(&path) {
            // A prompt typed into the wrong program was never this agent's.
            Ok(Value::Object(document))
                if document.get("probable_misroute") != Some(&Value::Bool(true)) =>
            {
                let at = document.get("inflight_at").and_then(Value::as_f64)?;
                Some((at, path, document))
            }
            _ => None,
        })
        .collect();
    records.sort_by(|left, right| right.0.total_cmp(&left.0));
    let mut times: Vec<f64> = records.iter().map(|record| record.0).collect();
    times.dedup();
    let &oldest = times.get(deliveries - 1).or(times.last())?;
    records.retain(|record| record.0 >= oldest);
    for (_, path, document) in &records {
        let quarantined = path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|phase| phase == "failed");
        let matched = typed_texts(document).into_iter().any(|text| {
            if !framed(text) {
                return false;
            }
            match draft {
                // A whole prompt, or a fragment of one (the tail a split
                // paste leaves, #731 agentctl-paste-tail-flush). A whole
                // prompt under `Unproven` vouches only when quarantined:
                // a person may have recalled it with the history keys. A
                // fragment is not what recall produces (that is the whole
                // prompt), so a fragment vouches either way.
                Draft::Text(rows) => {
                    fragment_matches(rows, text)
                        || (text_matches(rows, text)
                            && (vouching == Vouching::Recent || quarantined))
                }
                // A numbered paste placeholder matches a record holding
                // that exact label (#808), or a recent record holding no
                // label whose size the shared line-count matcher accepts
                // for its count (#884 follow-up busy-evidence). A
                // character-count placeholder carries no such proof and
                // keeps the rule above.
                Draft::PastedLines(_) => placeholder_matches_q(draft, text, document, quarantined),
                _ => {
                    placeholder_matches(draft, text, document)
                        && (vouching == Vouching::Recent || quarantined)
                }
            }
        });
        if matched {
            return Some(typed_prompt(path, document, Vec::new()));
        }
    }
    let Draft::Text(rows) = draft else {
        return None;
    };
    // A composer made of several pieces agentctl typed (#825): concatenated
    // paste placeholders, or a placeholder beside the literal start of
    // another delivery, as a working Claude Code pane puts back the prompts
    // it had queued behind a turn that ended without taking them.
    if let Some(found) = composite_match(&records, rows, vouching) {
        return Some(found);
    }
    // Several prompts in one draft, as Codex moves the inputs it held for an interrupted turn
    // back into its composer: a run of consecutive deliveries, oldest first, each framed, whose
    // texts the draft holds one after another.
    let wanted = without_sent_fields(&compact(rows));
    // Each delivery's typed text, when framed, and its records.
    let mut runs: Vec<(Option<String>, Vec<&Record>)> = Vec::new();
    for record in records.iter().rev() {
        match runs.last_mut() {
            Some((_, members)) if members[0].0 == record.0 => members.push(record),
            _ => {
                let typed = typed_texts(&record.2)
                    .first()
                    .filter(|text| framed(text))
                    .map(|text| without_sent_fields(&compact(text)));
                runs.push((typed, vec![record]));
            }
        }
    }
    for first in 0..runs.len() {
        let mut joined = String::new();
        for last in first..runs.len() {
            let Some(text) = runs[last].0.as_deref() else {
                break;
            };
            joined.push_str(text);
            if joined.len() > wanted.len() || !wanted.starts_with(&joined) {
                break;
            }
            if last > first && joined == wanted {
                // Named by the last prompt typed, as a single match is; the others are noted too.
                let (_, path, document) = runs[last].1[0];
                let others = runs[first..=last]
                    .iter()
                    .flat_map(|(_, members)| members.iter())
                    .filter(|member| member.1 != *path)
                    .map(|(_, path, _)| (message_id(path), path.clone()))
                    .collect();
                return Some(typed_prompt(path, document, others));
            }
        }
    }
    None
}

/// One piece of a composer draft, for [`composite_match`].
enum Segment {
    /// `[Pasted text #K +M lines]`, as its compact recorded label.
    Paste(String),
    /// `[Pasted Content N chars]`.
    Chars(usize),
    /// Literal text between placeholders.
    Literal(String),
}

/// Split a draft whose text shows into its placeholder and literal pieces,
/// or `None` when it holds no placeholder at all.
fn segments(rows: &str) -> Option<Vec<Segment>> {
    let mut out: Vec<Segment> = Vec::new();
    let mut literal = String::new();
    let mut rest = rows;
    let mut placeholders = 0;
    while !rest.is_empty() {
        if rest.starts_with('[') {
            if let Some(end) = rest.find(']') {
                let token = &rest[..=end];
                match Draft::from_composer(token) {
                    Draft::PastedLines(label) => {
                        let text = std::mem::take(&mut literal);
                        if !text.trim().is_empty() {
                            out.push(Segment::Literal(text));
                        }
                        out.push(Segment::Paste(label));
                        placeholders += 1;
                        rest = &rest[end + 1..];
                        continue;
                    }
                    Draft::Placeholder(length) => {
                        let text = std::mem::take(&mut literal);
                        if !text.trim().is_empty() {
                            out.push(Segment::Literal(text));
                        }
                        out.push(Segment::Chars(length));
                        placeholders += 1;
                        rest = &rest[end + 1..];
                        continue;
                    }
                    Draft::Text(_) => {}
                }
            }
        }
        let mut chars = rest.chars();
        let Some(first) = chars.next() else { break };
        literal.push(first);
        rest = chars.as_str();
    }
    if !literal.trim().is_empty() {
        out.push(Segment::Literal(literal));
    }
    (placeholders > 0 && out.len() > 1).then_some(out)
}

/// Whether a composer made only of pieces agentctl typed is agentctl's
/// (#825): every placeholder must name a recorded delivery (a numbered
/// label exactly, as in [`find`]; a character count under the same rule as
/// a single placeholder), and every literal piece must be the start of a
/// framed recorded delivery, each piece a different delivery. Anything
/// else in the composer attributes nothing.
fn composite_match(records: &[Record], rows: &str, vouching: Vouching) -> Option<TypedPrompt> {
    // `records` is already bounded: the latest few deliveries whose files
    // changed within the text lookback.
    let segments = segments(rows)?;
    let mut used: Vec<&PathBuf> = Vec::new();
    let mut matched: Vec<&Record> = Vec::new();
    for segment in &segments {
        let found = records.iter().find(|record| {
            if used.contains(&&record.1) {
                return false;
            }
            let quarantined = record
                .1
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|phase| phase == "failed");
            typed_texts(&record.2).into_iter().any(|text| {
                if !framed(text) {
                    return false;
                }
                match segment {
                    Segment::Paste(label) => {
                        record.2.get("paste_label").and_then(Value::as_str) == Some(label.as_str())
                            || (quarantined
                                && record.2.get("paste_label").is_none()
                                && pasted_line_count(label).is_some_and(|m| {
                                    crate::submission::expected_line_counts(text).contains(&m)
                                }))
                    }
                    Segment::Chars(length) => {
                        paste_length(text) == *length
                            && (vouching == Vouching::Recent || quarantined)
                    }
                    Segment::Literal(literal) => {
                        let start = without_sent_fields(&compact(literal));
                        !start.is_empty() && without_sent_fields(&compact(text)).starts_with(&start)
                    }
                }
            })
        })?;
        used.push(&found.1);
        matched.push(found);
    }
    let last = matched
        .iter()
        .max_by(|left, right| left.0.total_cmp(&right.0))?;
    let mut rest: Vec<&Record> = matched
        .iter()
        .copied()
        .filter(|record| record.1 != last.1)
        .collect();
    rest.sort_by(|left, right| left.0.total_cmp(&right.0));
    let others: Vec<(String, PathBuf)> = rest
        .iter()
        .map(|record| (message_id(&record.1), record.1.clone()))
        .collect();
    Some(typed_prompt(&last.1, &last.2, others))
}

/// The queue message ID a record's file name holds.
fn message_id(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_owned()
}

/// The sender a record's sender header names.
fn sender_of(document: &Map<String, Value>) -> Option<String> {
    document
        .get("text")
        .and_then(Value::as_str)
        .and_then(|text| crate::sender_header::parse(text.lines().next()?))
        .map(|header| header.sender)
}

/// When a record's prompt was typed, in Unix seconds.
fn typed_at(document: &Map<String, Value>) -> Option<f64> {
    ["inflight_at", "confirmed_at", "delivery_failed_at"]
        .iter()
        .find_map(|field| document.get(*field).and_then(Value::as_f64))
}

fn typed_prompt(
    path: &Path,
    document: &Map<String, Value>,
    others: Vec<(String, PathBuf)>,
) -> TypedPrompt {
    TypedPrompt {
        id: message_id(path),
        path: path.to_path_buf(),
        others,
        sender: sender_of(document),
        typed_at: typed_at(document),
    }
}

/// Record on the queue record of each prompt the draft held that it was found left unsent in
/// the editor and submitted again, when and how; a failure to record it is ignored, since the
/// record is only the audit trail.
pub(crate) fn note_recovered(prompt: &TypedPrompt, evidence: &str, now_seconds: f64) {
    let paths = prompt
        .others
        .iter()
        .map(|(_, path)| path)
        .chain([&prompt.path]);
    for path in paths {
        let Ok(Value::Object(mut document)) = crate::agent::read_private_json(path) else {
            continue;
        };
        document.insert("draft_recovered_at".to_owned(), json!(now_seconds));
        document.insert("draft_recovered_evidence".to_owned(), json!(evidence));
        let _ = crate::agent::atomic_json(path, &Value::Object(document));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "[from tickhub-ops-2 via agentctl · reply: agentctl send tickhub-ops-2]";

    /// A fresh private directory, removed when the value is dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            use std::os::unix::fs::DirBuilderExt;
            let path = std::env::temp_dir().join(format!(
                "agentctl-stale-draft-{tag}-{}-{:?}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn record(queue: &Path, phase: &str, id: &str, document: &Value) -> PathBuf {
        let directory = queue.join(phase);
        if !directory.is_dir() {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .unwrap();
        }
        let path = directory.join(format!("{id}.json"));
        crate::agent::atomic_json(&path, document).unwrap();
        path
    }

    #[test]
    fn a_wrapped_draft_matches_the_prompt_it_was_typed_from() {
        let prompt = format!("{HEADER}\nvalidate at 87daeb34 is LAUNCHED but QUEUED, not admitted");
        let drawn = "[from tickhub-ops-2 via agentctl · reply: agentctl send tickhu\nb-ops-2]\n\
                     validate at 87daeb34 is LAUNCHED but QUEUED,\nnot admitted";
        assert!(text_matches(drawn, &prompt));
        assert!(!text_matches("validate at 87daeb34", &prompt));
        assert!(!text_matches(
            &drawn.replace("not admitted", "admitted"),
            &prompt
        ));
    }

    #[test]
    fn a_chat_prompt_retimed_when_typed_still_matches_its_record() {
        let queued = "[from Ada via slack #team · sent: 2026.11.01:03:12 EDT · reply: <r1>]\nhello";
        let typed = "[from Ada via slack #team · sent: 2026.11.01:03:12 EDT, delivered 9 min \
                     later · reply: <r1>]\nhello";
        assert!(text_matches(typed, queued));
        assert!(!text_matches(&typed.replace("Ada", "Bob"), queued));
        assert!(!text_matches(&typed.replace("hello", "bye"), queued));
        // Two requests with the same words are told apart by their reply markers.
        assert!(!text_matches(&typed.replace("<r1>", "<r2>"), queued));
    }

    #[test]
    fn only_the_latest_deliveries_vouch_for_a_draft_and_only_the_last_for_a_paste() {
        let directory = Scratch::new("latest");
        let queue = directory.0.as_path();
        let text = |index: usize| {
            format!(
                "{HEADER}
message number {index:02}"
            )
        };
        for index in 0..8 {
            record(
                queue,
                "processed",
                &format!("m{index}"),
                &json!({"text": text(index), "inflight_at": 100.0 + index as f64}),
            );
        }
        let now = SystemTime::now();
        // The last five deliveries (m3..m7) can be a draft Muse put back; older ones, which a
        // person could only have recalled, cannot.
        for index in 3..8 {
            assert!(
                find(queue, &Draft::Text(text(index)), Vouching::Recent, now).is_some(),
                "m{index}"
            );
        }
        for index in 0..3 {
            assert!(
                find(queue, &Draft::Text(text(index)), Vouching::Recent, now).is_none(),
                "m{index}"
            );
        }
        // Every one has the same length, but a placeholder vouches only for the very last.
        let length = text(0).chars().count();
        assert_eq!(
            find(queue, &Draft::Placeholder(length), Vouching::Recent, now).map(|found| found.id),
            Some("m7".to_owned())
        );
        record(
            queue,
            "processed",
            "longer",
            &json!({"text": format!("{}!", text(9)), "inflight_at": 200.0}),
        );
        assert!(find(queue, &Draft::Placeholder(length), Vouching::Recent, now).is_none());
        // The prompt being delivered now, in `inflight/`, never vouches for a draft.
        record(
            queue,
            "inflight",
            "now",
            &json!({"text": text(10), "inflight_at": 300.0}),
        );
        assert!(find(queue, &Draft::Text(text(10)), Vouching::Recent, now).is_none());
    }

    #[test]
    fn a_paste_tail_fragment_is_ours_but_short_or_foreign_text_is_not() {
        // #731 agentctl-paste-tail-flush: a split multi-line paste leaves
        // its last line(s) in the editor, without the sender header.
        let directory = Scratch::new("fragment");
        let queue = directory.0.as_path();
        let prompt = format!(
            "{HEADER}\nfirst line of the message\nsecond line follows here\nafter. Needs your disposition (ledger-tree file). Details in the queue."
        );
        record(
            queue,
            "processed",
            "m1",
            &json!({"text": prompt, "inflight_at": 100.0}),
        );
        let now = SystemTime::now();
        // The tail line alone (the live shape) is ours.
        let tail = "after. Needs your disposition (ledger-tree file). Details in the queue.";
        let found = find(queue, &Draft::Text(tail.to_owned()), Vouching::Recent, now)
            .expect("tail fragment found");
        assert_eq!(found.id, "m1");
        // A fragment also vouches under Unproven (Claude Code / Codex):
        // history recall produces a whole prompt, never a tail.
        assert!(find(
            queue,
            &Draft::Text(tail.to_owned()),
            Vouching::Unproven,
            now
        )
        .is_some());
        // A middle slice, wrapped differently, is ours too.
        assert!(find(
            queue,
            &Draft::Text("second line follows\nhere after. Needs".to_owned()),
            Vouching::Recent,
            now
        )
        .is_some());
        // A person's short text that merely shares words is not.
        assert!(find(
            queue,
            &Draft::Text("in the queue.".to_owned()),
            Vouching::Recent,
            now
        )
        .is_none());
        // Text agentctl never typed is not.
        assert!(find(
            queue,
            &Draft::Text("an entirely different sentence the owner typed".to_owned()),
            Vouching::Recent,
            now
        )
        .is_none());
        // The whole prompt still matches as before.
        assert!(find(queue, &Draft::Text(prompt.clone()), Vouching::Recent, now).is_some());
    }

    #[test]
    fn only_a_framed_prompt_typed_recently_is_found() {
        let directory = Scratch::new("framed");
        let queue = directory.0.as_path();
        let text = format!("{HEADER}\nstatus: green");
        record(
            queue,
            "processed",
            "01793515539048909152-524156",
            &json!({"text": text, "inflight_at": 1_793_515_539.158}),
        );
        record(
            queue,
            "processed",
            "plain",
            &json!({"text": "a prompt with no header", "inflight_at": 1.0}),
        );
        let now = SystemTime::now();
        let found = find(queue, &Draft::Text(text.clone()), Vouching::Recent, now).expect("found");
        assert_eq!(found.id, "01793515539048909152-524156");
        assert_eq!(found.sender.as_deref(), Some("tickhub-ops-2"));
        assert_eq!(
            found.describe(),
            "typed by agentctl for tickhub-ops-2 at 06:45Z (message 01793515539048909152-524156)"
        );
        // An unframed record never vouches for a draft, nor does text agentctl never typed.
        assert!(find(
            queue,
            &Draft::Text("a prompt with no header".into()),
            Vouching::Recent,
            now
        )
        .is_none());
        assert!(find(
            queue,
            &Draft::Text(format!("{HEADER}\nother")),
            Vouching::Recent,
            now
        )
        .is_none());
        // A placeholder matches the last prompt typed, of exactly its length.
        let length = text.chars().count();
        assert!(find(queue, &Draft::Placeholder(length), Vouching::Recent, now).is_some());
        assert!(find(
            queue,
            &Draft::Placeholder(length + 1),
            Vouching::Recent,
            now
        )
        .is_none());
        // Outside the lookback, nothing is found.
        let later = now + PLACEHOLDER_LOOKBACK + Duration::from_secs(60);
        assert!(find(queue, &Draft::Placeholder(length), Vouching::Recent, later).is_none());
        assert!(find(queue, &Draft::Text(text.clone()), Vouching::Recent, later).is_some());
        let much_later = now + TEXT_LOOKBACK + Duration::from_secs(60);
        assert!(find(queue, &Draft::Text(text), Vouching::Recent, much_later).is_none());
    }

    #[test]
    fn a_batch_is_found_by_what_the_drain_typed() {
        let directory = Scratch::new("batch");
        let queue = directory.0.as_path();
        let first = format!("{HEADER}\none");
        let second = format!("{HEADER}\ntwo");
        let combined = format!("2 queued messages:\n\n{first}\n\n{second}");
        for (id, text) in [("a", &first), ("b", &second)] {
            record(
                queue,
                "processed",
                id,
                &json!({"text": text, "typed_as": combined, "inflight_at": 7.0}),
            );
        }
        let found = find(
            queue,
            &Draft::Text(combined),
            Vouching::Recent,
            SystemTime::now(),
        )
        .expect("found");
        assert!(["a", "b"].contains(&found.id.as_str()));
        let path = found.path.clone();
        note_recovered(&found, "Muse queued the prompt", 5.0);
        let document = crate::agent::read_private_json(&path).unwrap();
        assert_eq!(document["draft_recovered_at"], 5.0);
        assert_eq!(
            document["draft_recovered_evidence"],
            "Muse queued the prompt"
        );
    }

    #[test]
    fn each_harness_paste_placeholder_is_read_as_the_size_it_shows() {
        assert_eq!(
            Draft::from_composer("  [Pasted Content 1635 chars] "),
            Draft::Placeholder(1635)
        );
        assert_eq!(
            Draft::from_composer("[Pasted text #923 +10 lines]"),
            Draft::PastedLines("[Pastedtext#923+10lines]".to_owned())
        );
        // Anything else, a placeholder beside typed text included, is text that must match.
        for text in [
            "note [Pasted text #3 +4 lines]",
            "[Pasted text #x +4 lines]",
            "[Pasted Content many chars]",
            "[Pasted text #3]",
        ] {
            assert_eq!(Draft::from_composer(text), Draft::Text(text.to_owned()));
        }
    }

    #[test]
    fn a_claude_paste_placeholder_matches_a_recent_prompt_of_its_size() {
        // #884 follow-up busy-evidence: the exact recorded label still names
        // its own delivery among the recent ones (not only the latest), and
        // a record holding no label vouches by the shared line-count size.
        // A chip of a size no recent prompt has is a person's paste.
        let directory = Scratch::new("lines");
        let queue = directory.0.as_path();
        let text = format!("{HEADER}\none\ntwo\nthree\nfour");
        record(
            queue,
            "processed",
            "older",
            &json!({"text": text, "inflight_at": 1.0, "paste_label": "[Pastedtext#3+4lines]"}),
        );
        record(
            queue,
            "failed",
            "last",
            &json!({"text": text, "inflight_at": 2.0, "paste_label": "[Pastedtext#4+4lines]"}),
        );
        let now = SystemTime::now();
        let draft = |label: &str| Draft::from_composer(label);
        assert_eq!(
            find(
                queue,
                &draft("[Pasted text #4 +4 lines]"),
                Vouching::Recent,
                now
            )
            .map(|found| found.id),
            Some("last".to_owned())
        );
        // The older delivery's exact label is still found among the recent
        // deliveries; a number no record holds, while every record holds a
        // label, names a different paste.
        assert!(find(
            queue,
            &draft("[Pasted text #3 +4 lines]"),
            Vouching::Recent,
            now
        )
        .is_some());
        assert!(find(
            queue,
            &draft("[Pasted text #5 +4 lines]"),
            Vouching::Recent,
            now
        )
        .is_none());
        // A record holding no label vouches by size too.
        record(
            queue,
            "processed",
            "unlabelled",
            &json!({"text": text, "inflight_at": 3.0}),
        );
        assert!(find(
            queue,
            &draft("[Pasted text #4 +4 lines]"),
            Vouching::Recent,
            now
        )
        .is_some());
        // A size no recent prompt has is not ours.
        assert!(find(
            queue,
            &draft("[Pasted text #9 +999 lines]"),
            Vouching::Recent,
            now
        )
        .is_none());
        assert_eq!(
            draft("[Pasted text #4 +4 lines]").describe(),
            "a pasted 5 lines",
            "never the text"
        );
    }

    #[test]
    fn a_renumbered_chip_matches_an_older_delivery_not_only_the_latest() {
        // The stranded watcher chip is rarely the latest delivery: later
        // prompts piled up behind it. Its size matches the older delivery.
        let directory = Scratch::new("lines-older");
        let queue = directory.0.as_path();
        let older_text = format!("{HEADER}\nshort notice");
        let latest_text = format!("{HEADER}\none\ntwo\nthree\nfour");
        record(
            queue,
            "failed",
            "older",
            &json!({"text": older_text, "inflight_at": 1.0}),
        );
        record(
            queue,
            "processed",
            "latest",
            &json!({"text": latest_text, "inflight_at": 2.0, "paste_label": "[Pastedtext#9+4lines]"}),
        );
        let now = SystemTime::now();
        assert_eq!(
            find(
                queue,
                &Draft::from_composer("[Pasted text #264 +1 lines]"),
                Vouching::Unproven,
                now
            )
            .map(|found| found.id),
            Some("older".to_owned())
        );
    }

    #[test]
    fn a_stranded_paste_placeholder_with_its_recorded_number_is_ours_even_when_processed() {
        // #808: a drain typed a long prompt into a working Claude Code pane
        // and proved delivery, so its record sits in `processed/`; Claude
        // later left the paste itself unsubmitted in the composer. The
        // placeholder carries the paste number agentctl recorded when it
        // typed it, which a history recall cannot reproduce (a recalled
        // prompt is drawn as a new paste with a new number), so the exact
        // label attributes the draft even under `Unproven`.
        let directory = Scratch::new("stranded");
        let queue = directory.0.as_path();
        let text = format!("{HEADER}\none\ntwo\nthree\nfour");
        record(
            queue,
            "processed",
            "stranded",
            &json!({"text": text, "inflight_at": 2.0, "paste_label": "[Pastedtext#178+4lines]"}),
        );
        let now = SystemTime::now();
        assert_eq!(
            find(
                queue,
                &Draft::from_composer("[Pasted text #178 +4 lines]"),
                Vouching::Unproven,
                now
            )
            .map(|found| found.id),
            Some("stranded".to_owned())
        );
        // A different paste number, while the record holds its label, is a
        // person's paste, never ours; so is a size we never typed.
        assert!(find(
            queue,
            &Draft::from_composer("[Pasted text #179 +4 lines]"),
            Vouching::Unproven,
            now
        )
        .is_none());
        assert!(find(
            queue,
            &Draft::from_composer("[Pasted text #179 +999 lines]"),
            Vouching::Unproven,
            now
        )
        .is_none());
    }

    #[test]
    fn two_stranded_placeholders_from_two_deliveries_are_ours_together() {
        // #825: two prompts queued behind a working Claude Code turn came
        // back into the composer as two concatenated paste placeholders.
        let directory = Scratch::new("multi-stranded");
        let queue = directory.0.as_path();
        let first = format!("{HEADER}\none");
        let second = format!("{HEADER}\none\ntwo\nthree\nfour");
        record(
            queue,
            "processed",
            "first",
            &json!({"text": first, "inflight_at": 1.0, "paste_label": "[Pastedtext#231+1lines]"}),
        );
        record(
            queue,
            "processed",
            "second",
            &json!({"text": second, "inflight_at": 2.0, "paste_label": "[Pastedtext#232+4lines]"}),
        );
        let found = find(
            queue,
            &Draft::from_composer("[Pasted text #231 +1 lines][Pasted text #232 +4 lines]"),
            Vouching::Unproven,
            SystemTime::now(),
        )
        .expect("both placeholders are recorded deliveries");
        assert_eq!(found.id, "second");
        assert_eq!(found.others.len(), 1);
        assert_eq!(found.others[0].0, "first");
    }

    #[test]
    fn composite_placeholders_without_labels_match_failed_by_line_count() {
        // Regression 184e410b: stranded pastes that timed out at the stage
        // window have no recorded paste_label; their failed records still
        // reconcile by line count so the composite is not refused forever.
        let directory = Scratch::new("composite-no-label");
        let queue = directory.0.as_path();
        let first = format!("{HEADER}\n{}", "x\n".repeat(21) + "x");
        let second = format!("{HEADER}\n{}", "y\n".repeat(14) + "y");
        record(
            queue,
            "failed",
            "first",
            &json!({"text": first, "inflight_at": 1.0}),
        );
        record(
            queue,
            "failed",
            "second",
            &json!({"text": second, "inflight_at": 2.0}),
        );
        let found = find(
            queue,
            &Draft::from_composer("[Pasted text #580 +22 lines][Pasted text #613 +15 lines]"),
            Vouching::Unproven,
            SystemTime::now(),
        )
        .expect("composite of failed pastes reconciles by line count");
        assert_eq!(found.others.len(), 1);
    }

    #[test]
    fn a_placeholder_followed_by_the_start_of_another_delivery_is_ours() {
        // #825: a stranded paste, then the literal start of a second
        // delivery that came back into the composer as text.
        let directory = Scratch::new("paste-then-text");
        let queue = directory.0.as_path();
        let first = format!("{HEADER}\none\ntwo\nthree\nfour");
        let second = format!("{HEADER}\nbody of the second prompt follows here");
        record(
            queue,
            "processed",
            "first",
            &json!({"text": first, "inflight_at": 1.0, "paste_label": "[Pastedtext#233+4lines]"}),
        );
        record(
            queue,
            "processed",
            "second",
            &json!({"text": second, "inflight_at": 2.0}),
        );
        let draft = format!("[Pasted text #233 +4 lines]{second}");
        let found = find(
            queue,
            &Draft::from_composer(&draft),
            Vouching::Unproven,
            SystemTime::now(),
        )
        .expect("placeholder plus delivered text is ours");
        assert_eq!(found.id, "second");
        assert_eq!(found.others.len(), 1);
        // The same placeholder in front of a person's own words attributes
        // nothing: the literal part is no delivery's start.
        let foreign = "[Pasted text #233 +4 lines]my own half-written note";
        assert!(find(
            queue,
            &Draft::from_composer(foreign),
            Vouching::Unproven,
            SystemTime::now()
        )
        .is_none());
    }

    #[test]
    fn prompts_codex_moved_back_into_its_composer_together_are_found_in_order() {
        let directory = Scratch::new("run");
        let queue = directory.0.as_path();
        let text = |word: &str| format!("{HEADER}\n{word}");
        let owner = "[from Ada via slack #team · sent: 2026.11.01:03:12 EDT · reply: <r1>]\nhi";
        for (index, (id, prompt)) in [
            ("a", text("alpha")),
            ("b", text("bravo")),
            ("c", owner.to_owned()),
            ("d", text("delta")),
        ]
        .into_iter()
        .enumerate()
        {
            record(
                queue,
                "processed",
                id,
                &json!({"text": prompt, "inflight_at": 10.0 + index as f64}),
            );
        }
        let now = SystemTime::now();
        // Codex draws each held input on rows of its own, and the chat prompt retimed.
        let retimed = owner.replace("EDT", "EDT, delivered 2 min later");
        let drawn = format!("{}\n{retimed}\n{}", text("bravo"), text("delta"));
        let found = find(queue, &Draft::Text(drawn.clone()), Vouching::Recent, now).expect("found");
        assert_eq!(found.id, "d");
        assert_eq!(
            found
                .others
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            ["b", "c"]
        );
        assert!(
            found
                .describe()
                .starts_with("3 prompts typed by agentctl, the last for tickhub-ops-2"),
            "{}",
            found.describe()
        );
        note_recovered(&found, "submitted", 9.0);
        for id in ["b", "c", "d"] {
            let document =
                crate::agent::read_private_json(&queue.join(format!("processed/{id}.json")))
                    .unwrap();
            assert_eq!(document["draft_recovered_at"], 9.0, "{id}");
        }
        // Out of order, with a gap, with a word of a person's added, or with one left out of
        // the recent deliveries: not agentctl's.
        for draft in [
            format!("{}\n{}", text("delta"), text("bravo")),
            format!("{}\n{}", text("alpha"), text("delta")),
            format!("{drawn}\nplease"),
            format!("{}\n{}", text("bravo"), text("bravo")),
        ] {
            assert!(
                find(queue, &Draft::Text(draft.clone()), Vouching::Recent, now).is_none(),
                "{draft}"
            );
        }
    }

    #[test]
    fn where_history_keys_recall_prompts_only_unproven_ones_vouch_alone() {
        let directory = Scratch::new("unproven");
        let queue = directory.0.as_path();
        let text = |word: &str| format!("{HEADER}\n{word}");
        record(
            queue,
            "processed",
            "a",
            &json!({"text": text("alpha"), "inflight_at": 1.0}),
        );
        record(
            queue,
            "processed",
            "b",
            &json!({"text": text("bravo"), "inflight_at": 2.0}),
        );
        record(
            queue,
            "failed",
            "c",
            &json!({"text": text("charlie"), "inflight_at": 3.0}),
        );
        record(
            queue,
            "failed",
            "m",
            &json!({"text": text("mike"), "inflight_at": 4.0, "probable_misroute": true}),
        );
        let now = SystemTime::now();
        let unproven = |draft: String| find(queue, &Draft::Text(draft), Vouching::Unproven, now);
        // A delivered prompt alone is what Up recalls; a quarantined one was never proved taken.
        assert!(unproven(text("bravo")).is_none());
        assert_eq!(
            unproven(text("charlie")).map(|found| found.id),
            Some("c".into())
        );
        // A run of delivered prompts is not what Up recalls.
        let run = format!("{}\n{}", text("alpha"), text("bravo"));
        assert_eq!(unproven(run).map(|found| found.id), Some("b".into()));
        // Any of them vouches where a harness can take back a prompt it showed taken.
        assert!(find(queue, &Draft::Text(text("bravo")), Vouching::Recent, now).is_some());
        // A prompt typed into the wrong program never vouches, under either rule.
        for vouching in [Vouching::Recent, Vouching::Unproven] {
            assert!(find(queue, &Draft::Text(text("mike")), vouching, now).is_none());
        }
    }

    #[test]
    fn a_draft_is_described_without_its_text() {
        assert_eq!(
            Draft::Text("secret\nplan".into()).describe(),
            "11 characters on 2 row(s)"
        );
        assert_eq!(
            Draft::Placeholder(1635).describe(),
            "a pasted 1635 characters"
        );
    }
}
