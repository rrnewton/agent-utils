//! The harness-agnostic stuck-paste backstop (#1560): one general watcher
//! path that submits a delivered prompt sitting unsubmitted in any pane,
//! whatever TUI draws it.
//!
//! The per-harness composer readers (`submission::composer_view`, the Muse
//! editor reader) know where one harness draws its input box; an unknown
//! TUI, and any busy layout a reader cannot split, has no such reader, so a
//! sweep gated on them never presses Enter there (#1546). This module needs
//! no composer recognition: it remembers the prompts agentctl delivered
//! (its own queue records), reads the pane's visible lower region as screen
//! text only, and fuzzy-matches the two.
//!
//! Match rule: both sides are normalised by dropping box-drawing and prompt
//! glyphs and removing every whitespace character (a TUI wraps a long line
//! at any character, inside a word as well as between words, and the screen
//! does not say which, so wrapping and re-flow cannot hide a stuck paste).
//! A prompt of at most [`BackstopConfig::short_max`] normalised characters
//! matches only whole; a longer one matches when its first and last
//! [`BackstopConfig::anchor`] normalised characters both appear, in order,
//! or — when a tall prompt in a narrow pane has scrolled its head out of
//! the visible frame (#1897) — when a long exact run of its tail (at
//! least [`TAIL_RUN_MIN`] normalised characters, the prompt's own ending)
//! appears. Foreign text cannot contain that exact run of a delivered
//! prompt, so the tail-only shape is still ours by construction.
//! Three guards keep a submitted prompt from ever counting, because every
//! TUI echoes a submitted prompt into its transcript:
//!
//! * Position: the match must end within the last
//!   [`BackstopConfig::tail_rows`] content rows of the region, ignoring
//!   footer and hint rows below a blank or rule row. A stuck paste sits at
//!   the bottom; an echoed prompt has the agent's output after it.
//! * Static screen: the normalised region, with digit runs and spinner
//!   glyphs neutralised, must be identical across the polls that count. A
//!   working agent's screen changes; a stuck paste does not.
//! * Busy: where a harness reader knows the agent is working, the caller
//!   holds (see `sweep_paste_backstop` in `agent_watch`).
//!
//! A long paste drawn only as a placeholder (`[Pasted Content N chars]`,
//! `[Pasted text #K +M lines]`) has no text on screen to match, so it stays
//! with the per-harness readers; this backstop covers every text-visible
//! stuck paste, on every harness including an unknown one.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

/// Knobs for the backstop, in one place so tests drive poll counts and fake
/// times instead of wall-clock budgets. The production defaults are sized
/// so a normal submit never triggers: submission clears the lower region
/// within a poll or two, long before three polls spanning 45 s accumulate.
#[derive(Clone, Copy, Debug)]
pub struct BackstopConfig {
    /// Visible rows read at the bottom of the pane.
    pub region_rows: usize,
    /// Consecutive matching polls required before a press.
    pub polls: u32,
    /// Minimum span those polls must cover.
    pub span: Duration,
    /// Enter presses per prompt before the pane is reported blocked.
    pub max_presses: u32,
    /// Minimum time between two presses for one prompt.
    pub press_spacing: Duration,
    /// How long after delivery a prompt may still be pressed.
    pub expire: Duration,
    /// Normalised characters matched at each end of a long prompt.
    pub anchor: usize,
    /// Longest normalised prompt matched whole instead of by anchors.
    pub short_max: usize,
    /// Content rows below the match end that still count as the bottom.
    pub tail_rows: usize,
}

impl Default for BackstopConfig {
    fn default() -> Self {
        Self {
            region_rows: 24,
            polls: 3,
            span: Duration::from_secs(45),
            max_presses: 3,
            press_spacing: Duration::from_secs(20),
            expire: Duration::from_secs(2 * 3600),
            anchor: 40,
            short_max: 80,
            tail_rows: 4,
        }
    }
}

/// Shortest exact tail run, in normalised characters, that alone
/// identifies a long prompt whose head has scrolled off screen (#1897).
/// Longer runs are tried first, so a pane showing more of the tail
/// matches on more of the prompt.
pub const TAIL_RUN_MIN: usize = 48;

/// Tail-run lengths tried, longest first, when a long prompt's head
/// anchor is not visible.
const TAIL_RUNS: [usize; 6] = [160, 128, 96, 80, 64, TAIL_RUN_MIN];

/// One prompt agentctl delivered, from its queue records: the only texts
/// the backstop may ever match, so someone else's text cannot be submitted
/// by construction.
#[derive(Clone, Debug)]
pub struct DeliveredPrompt {
    /// The queue message ID.
    pub id: String,
    /// The text as typed (`typed_as`, else the queued `text`).
    pub text: String,
    /// When it was typed, in Unix seconds.
    pub typed_at: f64,
    /// Whether a pass left it stranded, not proved delivered: it may sit
    /// in a composer long after newer deliveries were typed (#1905).
    pub stranded: bool,
}

/// Whether `text` is one agentctl framed: a sender header, one of its own
/// `[agentctl ...]` notices, or a batch of queued messages. Mirrors the
/// framing rule the stale-draft attribution uses.
fn framed(text: &str) -> bool {
    let first = text.lines().next().unwrap_or_default();
    crate::sender_header::parse(first).is_some()
        || first.starts_with("[agentctl ")
        || first
            .strip_suffix(" queued messages:")
            .is_some_and(|count| {
                !count.is_empty() && count.bytes().all(|value| value.is_ascii_digit())
            })
}

/// Whether a queue record says a pass left its prompt stranded, not
/// proved delivered (#1905): the prompt may have been typed long before
/// it was stranded, and dozens of newer deliveries may follow it, yet
/// it can only now be sitting in the composer. Records in `processed/`
/// as possibly submitted and in `failed/` as a lost paste are the same
/// shape as one still in its inbox: agentctl gave up on the key, so the
/// text is treated as typed until a recovery notes it.
pub(crate) fn stranded_marked(document: &serde_json::Map<String, Value>) -> bool {
    document
        .get("stranded_at")
        .is_some_and(|value| !value.is_null())
        || document.get("possibly_submitted") == Some(&Value::Bool(true))
        || ["delivery_error", "error"].iter().any(|field| {
            document
                .get(*field)
                .and_then(Value::as_str)
                .is_some_and(|error| error.contains("late_paste_lost"))
        })
}

/// The prompts agentctl typed for the agent whose queue is at `queue`,
/// newest first, timed within `lookback` of `now` (Unix seconds). Typed
/// lanes are `processed/` and `failed/`; a pending prompt a pass left
/// stranded was typed too and may be drawn late. The sender named in a
/// prompt's header never filters this set: the recipient's queue is the
/// attribution (#1905). A prompt timed by its stranding (see
/// [`stranded_marked`]) counts from that later moment and is never
/// evicted by the newest-candidates cap below; every other prompt is
/// timed by when it was typed (`inflight_at`) and the cap keeps the
/// match set recent on a busy lane. A prompt quarantined as typed into
/// the wrong program never counts.
pub fn delivered_prompts(queue: &Path, now: f64, lookback: Duration) -> Vec<DeliveredPrompt> {
    let oldest = now - lookback.as_secs_f64();
    let mut found: Vec<DeliveredPrompt> = Vec::new();
    for (phase, pending) in [
        ("processed", false),
        ("failed", false),
        ("inbox", true),
        ("inbox.v2", true),
    ] {
        let Ok(entries) = std::fs::read_dir(queue.join(phase)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let Ok(Value::Object(document)) = crate::agent::read_private_json(&path) else {
                continue;
            };
            if document.get("probable_misroute") == Some(&Value::Bool(true)) {
                continue;
            }
            let stranded = pending || stranded_marked(&document);
            let stranded_at = document.get("stranded_at").and_then(Value::as_f64);
            if pending && stranded_at.is_none() {
                continue;
            }
            // A stranded prompt is timed by when it was left, which can
            // be long after it was typed (#1905); others by the typing.
            let typed_at = match (
                document.get("inflight_at").and_then(Value::as_f64),
                stranded_at,
            ) {
                (Some(typed), Some(left)) if stranded => Some(typed.max(left)),
                (Some(typed), _) => Some(typed),
                (None, Some(left)) => Some(left),
                (None, None) => None,
            };
            let Some(typed_at) = typed_at.filter(|at| *at >= oldest && *at <= now + 1.0) else {
                continue;
            };
            let Some(id) = document.get("id").and_then(Value::as_str) else {
                continue;
            };
            let text = ["typed_as", "text"]
                .iter()
                .filter_map(|field| document.get(*field).and_then(Value::as_str))
                .find(|text| framed(text));
            let Some(text) = text else {
                continue;
            };
            found.push(DeliveredPrompt {
                id: id.to_owned(),
                text: text.to_owned(),
                typed_at,
                stranded,
            });
        }
    }
    found.sort_by(|left, right| right.typed_at.total_cmp(&left.typed_at));
    found.dedup_by(|next, kept| next.id == kept.id);
    // The cap bounds the unmarked set (delivered prompts, whose echoes
    // must stay recent to be safe to compare); stranded prompts keep
    // their place however many deliveries followed them (#1905).
    let mut unmarked = 0usize;
    found.retain(|prompt| {
        if prompt.stranded {
            return true;
        }
        unmarked += 1;
        unmarked <= 20
    });
    found
}

/// Box-drawing characters a TUI frames its composer with: decoration, never
/// prompt text.
fn is_box_drawing(value: char) -> bool {
    ('\u{2500}'..='\u{257f}').contains(&value) || ('\u{2580}'..='\u{259f}').contains(&value)
}

/// `text` with the `sent:` field (whose stated delay depends on when the
/// prompt was typed) taken out of every sender header in it, as the
/// stale-draft matcher does: the screen may redraw a header the queue
/// recorded at a different second.
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

/// One screen row normalised for matching or fingerprinting: leading prompt
/// glyphs dropped, box-drawing dropped, every whitespace character removed.
fn normalise_row(row: &str) -> String {
    let body = row.trim_start_matches(|value: char| {
        value.is_whitespace() || matches!(value, '❯' | '›' | '>' | '◆' | '●' | '⏺')
    });
    without_sent_fields(
        &body
            .chars()
            .filter(|value| !value.is_whitespace() && !is_box_drawing(*value))
            .collect::<String>(),
    )
}

/// A prompt normalised the same way rows are.
fn normalise_prompt(text: &str) -> String {
    text.lines().map(normalise_row).collect::<Vec<_>>().join("")
}

/// The last `region_rows` rows of `screen`, as plain text rows.
fn region_rows(screen: &str, config: &BackstopConfig) -> Vec<String> {
    let plain = crate::harness_tui::plain(screen);
    let lines: Vec<&str> = plain.lines().collect();
    let start = lines.len().saturating_sub(config.region_rows);
    lines[start..]
        .iter()
        .map(|line| (*line).to_owned())
        .collect()
}

/// Whether `row` ends the content: blank after normalisation, or a rule a
/// TUI draws under its composer. Footer and hint rows below such a row are
/// chrome, not content.
fn is_separator(row: &str) -> bool {
    row.is_empty()
        || row
            .chars()
            .all(|value| is_box_drawing(value) || matches!(value, '-' | '=' | '_'))
}

/// The row index of the last content row in `rows`: the last non-blank row,
/// moved above the first blank-or-rule separator found scanning up from
/// the bottom, so a footer below the composer's bottom rule (or a terminals
/// panel below a rule) never counts as content after a stuck paste.
fn last_content_row(rows: &[String]) -> Option<usize> {
    let mut last = rows.len();
    while last > 0 && rows[last - 1].trim().is_empty() {
        last -= 1;
    }
    if last == 0 {
        return None;
    }
    let mut index = last - 1;
    loop {
        if is_separator(&rows[index]) && index + 1 < last {
            // Everything below this separator is chrome.
            let mut above = index;
            while above > 0 && rows[above - 1].trim().is_empty() {
                above -= 1;
            }
            return (above > 0).then_some(above - 1);
        }
        if index == 0 {
            return Some(last - 1);
        }
        index -= 1;
    }
}

/// Where a delivered prompt matches in a screen, by the row its match ends
/// on. `None` when it does not match, or when it matches only as an echoed
/// transcript prompt with content after it (the position guard).
pub struct RegionMatch {
    /// Row the match ends on, within the region.
    pub end_row: usize,
}

/// Fuzzy-match `prompt` (a delivered prompt's raw text) against the visible
/// lower region of `screen`, under the module's match and position rules.
pub fn match_region(prompt: &str, screen: &str, config: &BackstopConfig) -> Option<RegionMatch> {
    let rows = region_rows(screen, config);
    let normalised: Vec<String> = rows.iter().map(|row| normalise_row(row)).collect();
    // Character offsets of each row in the joined normalised region.
    let mut offsets: Vec<(usize, usize)> = Vec::with_capacity(normalised.len());
    let mut joined = String::new();
    for row in &normalised {
        offsets.push((joined.len(), joined.len() + row.len()));
        joined.push_str(row);
    }
    let needle = normalise_prompt(prompt);
    if needle.is_empty() {
        return None;
    }
    let needle_chars: Vec<char> = needle.chars().collect();
    let end_char = if needle_chars.len() <= config.short_max {
        joined.find(&needle).map(|at| at + needle.len())
    } else {
        let head: String = needle_chars[..config.anchor].iter().collect();
        let tail: String = needle_chars[needle_chars.len() - config.anchor..]
            .iter()
            .collect();
        let by_anchors = joined.find(&head).and_then(|head_at| {
            joined[head_at + head.len()..]
                .find(&tail)
                .map(|at| head_at + head.len() + at + tail.len())
        });
        by_anchors.or_else(|| {
            // The head has scrolled out of the frame (#1897): the visible
            // tail is the prompt's own ending, matched whole and as long
            // as the pane shows it. The last occurrence counts, so an
            // echoed copy higher up cannot shadow the composer's.
            TAIL_RUNS.iter().find_map(|run| {
                let run = (*run).min(needle_chars.len());
                if run <= config.anchor {
                    return None;
                }
                let suffix: String = needle_chars[needle_chars.len() - run..].iter().collect();
                joined.rfind(&suffix).map(|at| at + suffix.len())
            })
        })
    }?;
    // Map the match end (a byte offset) back to its row.
    let end_row = offsets
        .iter()
        .rposition(|(start, end)| end_char > *start && end_char <= *end)
        .or_else(|| offsets.iter().rposition(|(_, end)| *end <= end_char))?;
    let last_content = last_content_row(&rows)?;
    if end_row > last_content || last_content - end_row > config.tail_rows {
        return None;
    }
    Some(RegionMatch { end_row })
}

/// The visible text of the screen's lower region, up to its last content
/// row: the rows a stuck paste would sit in, without footer or hint chrome
/// below a blank or rule row. `None` when the region holds no text.
///
/// This is the observation that needs no parseable composer (#1683
/// pane-lock-held-by-stuck-sender): the stale-composer sweep reads a draft
/// from it when no harness composer reader recognises the screen, watches
/// it unchanged, and backs it up verbatim if it has to be cleared.
pub fn region_text(screen: &str, config: &BackstopConfig) -> Option<String> {
    let rows = region_rows(screen, config);
    let last = last_content_row(&rows)?;
    let mut text: Vec<&str> = rows[..=last].iter().map(|row| row.trim_end()).collect();
    while text.first().is_some_and(|row| row.trim().is_empty()) {
        text.remove(0);
    }
    if text.iter().all(|row| row.trim().is_empty()) {
        return None;
    }
    Some(text.join("\n"))
}

/// A fingerprint of the screen's lower region that changes only when its
/// text does: normalised like a match, with digit runs collapsed to one
/// digit (a timer or clock ticking) and spinner glyphs removed, so a stuck
/// paste behind a ticking clock still counts as a static screen while any
/// real output resets the count.
pub fn static_fingerprint(screen: &str, config: &BackstopConfig) -> u64 {
    let rows = region_rows(screen, config);
    let mut stable = String::new();
    for row in &rows {
        let mut previous_digit = false;
        for value in row.chars() {
            if ('\u{2800}'..='\u{28ff}').contains(&value)
                || matches!(value, '◐' | '◑' | '◒' | '◓' | '◔' | '◕')
            {
                continue;
            }
            if value.is_ascii_digit() {
                if !previous_digit {
                    stable.push('0');
                }
                previous_digit = true;
            } else {
                previous_digit = false;
                stable.push(value);
            }
        }
        stable.push('\n');
    }
    let normalised = normalise_row(&stable);
    let mut hasher = DefaultHasher::new();
    normalised.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROMPT: &str = "[from lane-a via agentctl · reply: agentctl send lane-a]\nfirst line of the message\nsecond line follows here\nafter. Needs your disposition (ledger-tree file). Details in the queue.";

    fn config() -> BackstopConfig {
        BackstopConfig::default()
    }

    #[test]
    fn wrapped_and_reflowed_text_still_matches() {
        // The same prompt wrapped at another width, with box-drawing and a
        // prompt glyph the queue record does not have.
        let screen = "◆ earlier work\n────────────────\n❯ [from lane-a via agentctl · reply: agentctl send\n  lane-a] first line of the message second line\n  follows here after. Needs your disposition\n  (ledger-tree file). Details in the queue.\n────────────────\n  model · context left\n";
        assert!(match_region(PROMPT, screen, &config()).is_some());
    }

    const LONG_PROMPT: &str = "[from lane-a via agentctl · reply: agentctl send lane-a]\nlane-a: one read-only check alongside the reaper passes. Find which retained run refers to the missing archive, whether the archive is really gone or just at another path, and whether any publication path is affected by its absence. Report which run it is, where the archive actually lives, and whether receipts or ledger rows that name it can still be read, in that order.";

    /// Wrap `text` as a composer at `width` columns draws it, then keep
    /// only the last `visible` composer rows above the bottom rule: a
    /// tall prompt in a narrow pane scrolls its first rows (and its
    /// header) out of the frame, leaving only the tail (#1897).
    fn tail_only_screen(text: &str, width: usize, visible: usize) -> String {
        let avail = width.saturating_sub(2).max(8);
        let mut rows: Vec<String> = Vec::new();
        let mut first = true;
        for line in text.lines() {
            let mut rest = line;
            loop {
                let take = if rest.len() <= avail {
                    rest.len()
                } else {
                    let mut cut = avail;
                    while cut > 0 && !rest.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    if let Some(space) = rest[..cut].rfind(' ') {
                        if space > avail / 2 {
                            cut = space;
                        }
                    }
                    cut
                };
                let (chunk, tail) = rest.split_at(take);
                rows.push(format!(
                    "{}{}",
                    if first { "❯ " } else { "  " },
                    chunk.trim()
                ));
                first = false;
                rest = tail.trim_start();
                if rest.is_empty() {
                    break;
                }
            }
        }
        let kept = &rows[rows.len().saturating_sub(visible)..];
        format!(
            "● earlier output\n● more output\n{}\n{}\n  model · context left\n",
            kept.join("\n"),
            "─".repeat(width)
        )
    }

    #[test]
    fn a_tail_only_narrow_prompt_still_matches() {
        // #1897: at 30 and 42 columns the head anchor has scrolled off;
        // at 80 the whole prompt fits. All three must match.
        for width in [30usize, 42, 80] {
            let screen = tail_only_screen(LONG_PROMPT, width, 8);
            assert!(
                match_region(LONG_PROMPT, &screen, &config()).is_some(),
                "width {width}: {screen}"
            );
        }
    }

    #[test]
    fn a_tail_only_foreign_draft_does_not_match() {
        let foreign = "[from lane-a via agentctl · reply: agentctl send lane-a]\nlane-a: a completely different instruction about pruning old branches and archiving the results so the next reader can find them without asking anyone where they went or what they mean today.";
        for width in [30usize, 42, 80] {
            let screen = tail_only_screen(foreign, width, 8);
            assert!(
                match_region(LONG_PROMPT, &screen, &config()).is_none(),
                "width {width}"
            );
        }
    }

    #[test]
    fn foreign_text_does_not_match() {
        let screen = "❯ a person's own half-written note about the ledger\n────────────────\n";
        assert!(match_region(PROMPT, screen, &config()).is_none());
        // A short shared phrase is not a match either: a short prompt counts
        // only whole, a long one only by both anchors.
        let screen = "❯ first line of the message\n────────────────\n";
        assert!(match_region(PROMPT, screen, &config()).is_none());
    }

    #[test]
    fn an_echoed_prompt_with_output_after_it_does_not_match() {
        // Submitted: the prompt sits in the transcript with the agent's
        // finished output below it, on an idle pane.
        let screen = "❯ [from lane-a via agentctl · reply: agentctl send lane-a]\nfirst line of the message\nsecond line follows here\nafter. Needs your disposition (ledger-tree file). Details in the queue.\n● I read the ledger and filed the disposition.\n  The queue now holds two items.\n  Both are ready for review.\n  Nothing else is pending.\n  Closing the turn now.\n";
        assert!(match_region(PROMPT, screen, &config()).is_none());
    }

    #[test]
    fn a_short_prompt_matches_only_whole() {
        let prompt = "short task: check the gate";
        let screen = "❯ short task: check the gate\n────\n";
        assert!(match_region(prompt, screen, &config()).is_some());
        let screen = "❯ short task: check\n────\n";
        assert!(match_region(prompt, screen, &config()).is_none());
    }

    #[test]
    fn a_footer_below_the_composer_rule_is_not_content_after() {
        let screen = "❯ first line of the message second line follows here after. Needs your disposition (ledger-tree file). Details in the queue.\n────────────────\n  GPT · /work · ← for agents · ? for shortcuts\n";
        // The one-line prompt on screen is the delivered prompt wrapped onto
        // one row; head and tail anchors both show above the rule.
        let prompt = "first line of the message second line follows here after. Needs your disposition (ledger-tree file). Details in the queue.";
        assert!(match_region(prompt, screen, &config()).is_some());
    }

    #[test]
    fn a_ticking_clock_and_spinner_keep_the_fingerprint() {
        let stuck = "❯ first line of the message\n────\n⠋ working 1:24\n";
        let later = "❯ first line of the message\n────\n⠙ working 1:25\n";
        assert_eq!(
            static_fingerprint(stuck, &config()),
            static_fingerprint(later, &config())
        );
        let changed = "❯ first line of the message\n● new output arrived\n────\n⠙ working 1:25\n";
        assert_ne!(
            static_fingerprint(stuck, &config()),
            static_fingerprint(changed, &config())
        );
    }

    #[test]
    fn delivered_prompts_reads_only_framed_typed_records() {
        use std::os::unix::fs::DirBuilderExt;
        let queue = std::env::temp_dir().join(format!("agentctl-backstop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&queue);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(queue.join("processed"))
            .unwrap();
        crate::agent::atomic_json(
            &queue.join("processed/lane.json"),
            &serde_json::json!({"id": "lane", "text": PROMPT,
                                "inflight_at": 1_000.0}),
        )
        .unwrap();
        crate::agent::atomic_json(
            &queue.join("processed/foreign.json"),
            &serde_json::json!({"id": "foreign", "text": "typed by a person",
                                "inflight_at": 1_000.0}),
        )
        .unwrap();
        let found = delivered_prompts(&queue, 1_100.0, Duration::from_secs(3600));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].id, "lane");
        // Outside the lookback it is no longer a candidate.
        assert!(delivered_prompts(&queue, 9_999.0, Duration::from_secs(3600)).is_empty());
        let _ = std::fs::remove_dir_all(&queue);
    }

    #[test]
    fn delivered_prompts_keeps_stranded_records_beyond_the_newest_cap() {
        // #1905: a prompt typed long ago but stranded recently — filed
        // in processed/ as possibly submitted, or in failed/ as a lost
        // paste — stays a candidate, timed by its stranding, however
        // many newer deliveries a busy lane has had since.
        use std::os::unix::fs::DirBuilderExt;
        let queue =
            std::env::temp_dir().join(format!("agentctl-backstop-stranded-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&queue);
        for phase in ["processed", "failed"] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .recursive(true)
                .create(queue.join(phase))
                .unwrap();
        }
        let write = |phase: &str, name: &str, document: serde_json::Value| {
            crate::agent::atomic_json(&queue.join(phase).join(name), &document).unwrap();
        };
        write(
            "processed",
            "old-stranded.json",
            serde_json::json!({"id": "old-stranded", "text": PROMPT,
                               "inflight_at": 100.0, "stranded_at": 3_900.0,
                               "possibly_submitted": true}),
        );
        write(
            "failed",
            "lost.json",
            serde_json::json!({"id": "lost", "text": PROMPT,
                               "inflight_at": 100.0, "stranded_at": 3_800.0,
                               "possibly_submitted": true,
                               "delivery_error": "late_paste_lost: the paste never showed"}),
        );
        write(
            "processed",
            "old-delivered.json",
            serde_json::json!({"id": "old-delivered", "text": PROMPT,
                               "inflight_at": 100.0}),
        );
        for index in 0..25 {
            write(
                "processed",
                &format!("new-{index}.json"),
                serde_json::json!({"id": format!("new-{index}"), "text": PROMPT,
                                   "inflight_at": 3_900.0 - index as f64}),
            );
        }
        let found = delivered_prompts(&queue, 4_000.0, Duration::from_secs(3600));
        let ids: Vec<&str> = found.iter().map(|prompt| prompt.id.as_str()).collect();
        assert!(ids.contains(&"old-stranded"), "{ids:?}");
        assert!(ids.contains(&"lost"), "{ids:?}");
        assert!(!ids.contains(&"old-delivered"), "{ids:?}");
        assert!(
            found
                .iter()
                .find(|prompt| prompt.id == "old-stranded")
                .is_some_and(|prompt| prompt.stranded && prompt.typed_at == 3_900.0),
            "{found:?}"
        );
        // Once the stranding itself is outside the lookback, it goes.
        assert!(
            delivered_prompts(&queue, 99_999.0, Duration::from_secs(3600))
                .iter()
                .all(|prompt| prompt.id != "old-stranded")
        );
        let _ = std::fs::remove_dir_all(&queue);
    }
}
