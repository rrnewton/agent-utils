//! Telling a draft agentctl typed from one a person is typing, for the stale-draft recovery
//! (#720 agentctl-muse-stale-draft).
//!
//! A delivery proves the editor empty before it types, so a draft left in an agent's editor holds
//! every later prompt, the owner's included, for as long as it stays. Muse can leave one there
//! after agentctl's proof of delivery: a prompt steered into a running turn shows at once as a
//! user turn, and when the steer later fails (as it does around a context compaction) Muse puts
//! the prompt back in the editor, unsent. Nothing else will ever submit it.
//!
//! A delivery that finds a draft therefore asks whether agentctl typed it: the draft must match,
//! as Muse draws it, the text of one of the last [`RECENT_DELIVERIES`] prompts this agent's queue
//! records as typed (in `processed/` or `failed/`; `inflight/` holds the prompt being delivered
//! now), and that text must open with a sender header (or be a batch of queued messages, which
//! are). Only such a draft is submitted. Any other draft may be a person typing, so it is never
//! submitted or cleared, only reported. A prompt Muse puts back comes back soon after it was
//! typed, and every later delivery is refused while it is there, so it is among the last few;
//! an older prompt a person recalled into the editor with Muse's history keys is not.
//!
//! Muse draws a long paste as one placeholder row, `[Pasted Content N chars]`, which shows only
//! the paste's length. A person's paste of the same length is not rare among many long prompts,
//! so such a draft matches only the very last prompt typed, of exactly that length, within
//! [`PLACEHOLDER_LOOKBACK`], a shorter window than [`TEXT_LOOKBACK`] for a draft whose text shows.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};

/// How far back a draft whose text shows is looked for among the prompts agentctl typed.
pub(crate) const TEXT_LOOKBACK: Duration = Duration::from_secs(48 * 3600);
/// How far back a draft that shows only Muse's paste placeholder is looked for.
pub(crate) const PLACEHOLDER_LOOKBACK: Duration = Duration::from_secs(6 * 3600);
/// The most queue records one lookup reads, most recently changed first.
const MAX_RECORDS: usize = 200;
/// How many of the latest deliveries (a batch is one) a draft whose text shows may be.
pub(crate) const RECENT_DELIVERIES: usize = 5;
/// The queue phases that hold prompts agentctl typed, or may have typed, before the one being
/// delivered now.
const TYPED_PHASES: [&str; 2] = ["processed", "failed"];

/// A draft in an agent's editor, as the screen shows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Draft {
    /// The draft's rows, one per line, as drawn.
    Text(String),
    /// Muse's placeholder for a long paste: only its length in characters shows.
    Placeholder(usize),
}

impl Draft {
    /// The draft in words, without its text: `412 characters on 6 rows`, or `a pasted 1635
    /// characters`. A draft a person typed is never quoted in a log or a notice.
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Text(text) => format!(
                "{} characters on {} row(s)",
                text.chars().count(),
                text.lines().count().max(1)
            ),
            Self::Placeholder(length) => format!("a pasted {length} characters"),
        }
    }
}

/// A prompt agentctl typed whose text matches a draft.
#[derive(Clone, Debug)]
pub(crate) struct TypedPrompt {
    /// The queue message ID.
    pub id: String,
    /// The record's path, in the phase directory that holds it.
    pub path: PathBuf,
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
        format!(
            "typed by agentctl for {}{at} (message {})",
            self.sender.as_deref().unwrap_or("an unnamed sender"),
            self.id
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

/// A compacted prompt split into its sender header, without the `sent:` field (whose stated
/// delay depends on when the prompt was typed) but with every other field, and the rest.
fn header_and_body(compacted: &str) -> Option<(String, &str)> {
    let rest = compacted.strip_prefix("[from")?;
    let end = rest.find(']')?;
    let header = &rest[..end];
    let header = match header.find("·sent:") {
        Some(at) => {
            let after = &header[at + "·sent:".len()..];
            let tail = after.find('·').map_or("", |next| &after[next..]);
            format!("{}{tail}", &header[..at])
        }
        None => header.to_owned(),
    };
    Some((header, &rest[end + 1..]))
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
    let (draft, candidate) = (compact(draft), compact(candidate));
    if draft.is_empty() {
        return false;
    }
    if draft == candidate {
        return true;
    }
    matches!(
        (header_and_body(&draft), header_and_body(&candidate)),
        (Some(left), Some(right)) if left == right
    )
}

/// The character count Muse shows for a paste of `text`: code points, CRLF as one.
fn paste_length(text: &str) -> usize {
    text.chars().count() - text.matches("\r\n").count()
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

/// The prompt agentctl typed into this agent that `draft` is, looked for in the queue at `queue`
/// among the records changed within the draft's lookback of `now`; `None` when agentctl typed
/// no such prompt, so the draft may be a person's.
pub(crate) fn find(queue: &Path, draft: &Draft, now: SystemTime) -> Option<TypedPrompt> {
    let (lookback, deliveries) = match draft {
        Draft::Text(_) => (TEXT_LOOKBACK, RECENT_DELIVERIES),
        Draft::Placeholder(_) => (PLACEHOLDER_LOOKBACK, 1),
    };
    // The records of the latest deliveries, latest first: a batch's members share one
    // `inflight_at`, so they count as one delivery.
    let mut records: Vec<(f64, PathBuf, Map<String, Value>)> = recent_records(queue, now, lookback)
        .into_iter()
        .filter_map(|path| match crate::agent::read_private_json(&path) {
            Ok(Value::Object(document)) => {
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
    for (_, path, document) in records.into_iter().filter(|record| record.0 >= oldest) {
        let matched = typed_texts(&document).into_iter().any(|text| {
            framed(text)
                && match draft {
                    Draft::Text(rows) => text_matches(rows, text),
                    Draft::Placeholder(length) => paste_length(text) == *length,
                }
        });
        if !matched {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_owned();
        let sender = document
            .get("text")
            .and_then(Value::as_str)
            .and_then(|text| crate::sender_header::parse(text.lines().next()?))
            .map(|header| header.sender);
        let typed_at = ["inflight_at", "confirmed_at", "delivery_failed_at"]
            .iter()
            .find_map(|field| document.get(*field).and_then(Value::as_f64));
        return Some(TypedPrompt {
            id,
            path,
            sender,
            typed_at,
        });
    }
    None
}

/// Record on the prompt's queue record that it was found left unsent in the editor and
/// submitted again, when and how; a failure to record it is ignored, since the record is only
/// the audit trail.
pub(crate) fn note_recovered(prompt: &TypedPrompt, evidence: &str, now_seconds: f64) {
    let Ok(Value::Object(mut document)) = crate::agent::read_private_json(&prompt.path) else {
        return;
    };
    document.insert("draft_recovered_at".to_owned(), json!(now_seconds));
    document.insert("draft_recovered_evidence".to_owned(), json!(evidence));
    let _ = crate::agent::atomic_json(&prompt.path, &Value::Object(document));
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
                find(queue, &Draft::Text(text(index)), now).is_some(),
                "m{index}"
            );
        }
        for index in 0..3 {
            assert!(
                find(queue, &Draft::Text(text(index)), now).is_none(),
                "m{index}"
            );
        }
        // Every one has the same length, but a placeholder vouches only for the very last.
        let length = text(0).chars().count();
        assert_eq!(
            find(queue, &Draft::Placeholder(length), now).map(|found| found.id),
            Some("m7".to_owned())
        );
        record(
            queue,
            "processed",
            "longer",
            &json!({"text": format!("{}!", text(9)), "inflight_at": 200.0}),
        );
        assert!(find(queue, &Draft::Placeholder(length), now).is_none());
        // The prompt being delivered now, in `inflight/`, never vouches for a draft.
        record(
            queue,
            "inflight",
            "now",
            &json!({"text": text(10), "inflight_at": 300.0}),
        );
        assert!(find(queue, &Draft::Text(text(10)), now).is_none());
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
        let found = find(queue, &Draft::Text(text.clone()), now).expect("found");
        assert_eq!(found.id, "01793515539048909152-524156");
        assert_eq!(found.sender.as_deref(), Some("tickhub-ops-2"));
        assert_eq!(
            found.describe(),
            "typed by agentctl for tickhub-ops-2 at 06:45Z (message 01793515539048909152-524156)"
        );
        // An unframed record never vouches for a draft, nor does text agentctl never typed.
        assert!(find(queue, &Draft::Text("a prompt with no header".into()), now).is_none());
        assert!(find(queue, &Draft::Text(format!("{HEADER}\nother")), now).is_none());
        // A placeholder matches the last prompt typed, of exactly its length.
        let length = text.chars().count();
        assert!(find(queue, &Draft::Placeholder(length), now).is_some());
        assert!(find(queue, &Draft::Placeholder(length + 1), now).is_none());
        // Outside the lookback, nothing is found.
        let later = now + PLACEHOLDER_LOOKBACK + Duration::from_secs(60);
        assert!(find(queue, &Draft::Placeholder(length), later).is_none());
        assert!(find(queue, &Draft::Text(text.clone()), later).is_some());
        let much_later = now + TEXT_LOOKBACK + Duration::from_secs(60);
        assert!(find(queue, &Draft::Text(text), much_later).is_none());
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
        let found = find(queue, &Draft::Text(combined), SystemTime::now()).expect("found");
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
