//! Muse's ruled editor: its regions, its status and the proofs that input reached it.
//!
//! Muse draws its input editor between the last two horizontal rules on screen, with one status
//! footer below them. Everything here is a pure function of the visible screen text.
//!
//! Lines break at `\n`, `\r\n`, `\r`, `\x0b`, `\x0c`, `\x1c`-`\x1e`, `\u{85}`, `\u{2028}` and
//! `\u{2029}`, and whitespace is Unicode `White_Space` plus `\x1c`-`\x1f`, so a frame reads the
//! same whichever agentctl edition reads it.
//!
//! The "verified process" variants drop the `Muse Code X.Y.Z` header requirement. Muse 1.4 scrolls
//! the header off screen, so they are only meaningful after the caller has pinned the live Muse
//! process generation, which is the independent proof that the pane is still Muse.

// A line-for-line port of py/agentctl/client.py, from `_muse_composer_regions` through
// `muse_verified_process_prompt_transcript_count`, with the status order of the Muse branch of
// `pane_info` in py/agentctl/subagents.py. The helpers below reproduce Python's
// `str.splitlines`, `str.split()` and `str.strip()` exactly rather than approximating them with
// `str::lines` and `char::is_whitespace`: a proof one edition accepts and the other refuses would
// make delivery depend on which edition is installed.

use std::collections::BTreeSet;

use super::muse_effort;

/// Markers Muse draws in front of the editor's first line and in front of each user turn.
const PROMPT_MARKERS: [&str; 2] = ["❯", "›"];
/// Markers that end one rendered user turn: the answer marker, the next user turn, or the progress
/// markers Muse 1.4 draws below a submitted turn while it works (`◇ Thinking (0s · esc to
/// interrupt)`, `◈ Double checking`).
const TURN_END_MARKERS: [&str; 5] = ["◆", "❯", "›", "◇", "◈"];
/// The only footer line that reports a paused goal.
const GOAL_PAUSED: &str = "Goal (paused)";

// Python's `str.isspace`: Unicode `White_Space`, plus the four ASCII information separators
// `\x1c`-`\x1f` that Python also counts.
fn is_space(value: char) -> bool {
    value.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&value)
}

// Python's `text.strip()`.
fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

// Python's `" ".join(text.split())`: words separated by single spaces.
fn normalize(text: &str) -> String {
    text.split(is_space)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

// Python's `text.splitlines()`: no trailing empty line, and `\r\n` is one boundary.
fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut characters = text.char_indices().peekable();
    while let Some((index, value)) = characters.next() {
        if !matches!(
            value,
            '\n' | '\r'
                | '\u{0b}'
                | '\u{0c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            continue;
        }
        lines.push(&text[start..index]);
        start = index + value.len_utf8();
        if value == '\r' && characters.next_if(|(_, next)| *next == '\n').is_some() {
            start += 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// One of Muse's horizontal rules: at least three rule characters and nothing else.
fn is_divider(line: &str) -> bool {
    let stripped = strip(line);
    stripped.chars().count() >= 3
        && stripped
            .chars()
            .all(|value| matches!(value, '─' | '━' | '═'))
}

/// Whether `line` is exactly `Muse Code X.Y.Z`, with ASCII digits in each version part.
fn is_versioned_header(line: &str) -> bool {
    line.strip_prefix("Muse Code ").is_some_and(|version| {
        let parts = version.split('.').collect::<Vec<_>>();
        parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|value| value.is_ascii_digit()))
    })
}

/// Whether `line` is exactly one of `markers`, or one of them followed by a space.
fn starts_with_marker(line: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| {
        line == *marker
            || line
                .strip_prefix(marker)
                .is_some_and(|rest| rest.starts_with(' '))
    })
}

/// Return the explicit mode from one structurally valid Muse footer.
///
/// Muse right-aligns transient notices on the footer line itself, after a gap of three or more
/// spaces: `Paste again to expand for editing` for a few seconds after a paste, and
/// `Hook · UserPromptSubmit · ...` after a submission. A line that is no footer as a whole is
/// read again without one of those notices; any other text keeps it no footer, so an unknown
/// status never passes for an idle editor.
fn muse_status_mode(line: &str) -> Option<&'static str> {
    muse_status_fields_mode(line).or_else(|| without_notice(line).and_then(muse_status_fields_mode))
}

/// The notice Muse 1.4 right-aligns on its footer line for a few seconds after a paste.
const PASTE_NOTICE: &str = "Paste again to expand for editing";
/// How the notice Muse 1.4 right-aligns on its footer line after a submission starts.
const HOOK_NOTICE: &str = "Hook · ";

/// `line` before its first run of three whitespace characters, when the rest of the line is
/// one of the notices Muse 1.4 was seen to draw there: [`PASTE_NOTICE`] or a [`HOOK_NOTICE`].
fn without_notice(line: &str) -> Option<&str> {
    let (mut start, mut run) = (0, 0);
    for (index, value) in line.char_indices() {
        if is_space(value) {
            if run == 0 {
                start = index;
            }
            run += 1;
            continue;
        }
        if run >= 3 {
            let notice = &line[index..];
            return (notice == PASTE_NOTICE || notice.starts_with(HOOK_NOTICE))
                .then_some(&line[..start]);
        }
        run = 0;
    }
    None
}

/// Return the mode of a line that is exactly one Muse footer.
fn muse_status_fields_mode(line: &str) -> Option<&'static str> {
    let fields = line.split('·').map(strip).collect::<Vec<_>>();
    if !matches!(fields.len(), 3 | 4)
        || fields.iter().any(|field| field.is_empty())
        || !muse_effort(fields[1])
    {
        return None;
    }
    if fields.len() == 3 {
        return Some("standard");
    }
    match fields[3] {
        "YOLO" => Some("YOLO"),
        "Auto-review" => Some("Auto-review"),
        // A launch whose flags override saved settings reports this instead of
        // its approval mode (#382 muse-approval-never-ready).
        "Launch overrides" => Some("Launch overrides"),
        _ => None,
    }
}

/// Return all structurally valid footer modes; callers require one authority.
fn muse_footer_modes(lines: &[&str]) -> Vec<&'static str> {
    lines
        .iter()
        .filter(|line| !strip(line).is_empty())
        .filter_map(|line| muse_status_mode(strip(line)))
        .collect()
}

/// Split transcript/composer using Muse's ruled editor and status footer.
fn muse_composer_regions(screen: &str, require_header: bool) -> Option<(String, String)> {
    let lines = splitlines(screen);
    let dividers = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| is_divider(line).then_some(index))
        .collect::<Vec<_>>();
    if dividers.len() < 2 {
        return None;
    }
    let (top, bottom) = (dividers[dividers.len() - 2], dividers[dividers.len() - 1]);
    let footer = &lines[bottom + 1..];
    let versioned_header = lines[..top]
        .iter()
        .any(|line| is_versioned_header(strip(line)));
    if !((versioned_header || !require_header) && muse_footer_modes(footer).len() == 1) {
        return None;
    }
    Some((lines[..top].join("\n"), lines[top + 1..bottom].join("\n")))
}

/// Markers of the progress line Muse 1.4 draws right above its editor while a turn runs.
const PROGRESS_MARKERS: [&str; 2] = ["◇", "◈"];

/// Whether the transcript ends in Muse's progress line, so a turn is still running.
///
/// Muse 1.4 keeps its editor empty and drawn while it works, with `◇ Thinking (3s · esc to
/// interrupt)` or `◈ Double checking (1s · esc to interrupt)` as the last transcript line, so
/// that line, not the editor, says whether a turn is running.
fn muse_turn_running(transcript: &str) -> bool {
    splitlines(transcript)
        .into_iter()
        .map(strip)
        .rfind(|line| !line.is_empty())
        .is_some_and(|line| {
            starts_with_marker(line, &PROGRESS_MARKERS) && line.ends_with("esc to interrupt)")
        })
}

/// An empty editor below a transcript that does not end in Muse's progress line.
fn muse_editor_idle(transcript: &str, composer: &str) -> bool {
    PROMPT_MARKERS.contains(&strip(composer)) && !muse_turn_running(transcript)
}

/// Recognize Muse's progress line above the editor after live-Muse process verification.
pub(crate) fn muse_verified_process_turn_running(screen: &str) -> bool {
    muse_composer_regions(screen, false)
        .is_some_and(|(transcript, _)| muse_turn_running(&transcript))
}

/// Recognize Muse's idle composer without mistaking a choice prompt for it.
pub(crate) fn muse_idle_composer(screen: &str) -> bool {
    muse_composer_regions(screen, true)
        .is_some_and(|(transcript, composer)| muse_editor_idle(&transcript, &composer))
}

/// Require the Auto-review footer on the active empty composer.
pub(crate) fn muse_auto_review_idle_composer(screen: &str) -> bool {
    let Some((transcript, composer)) = muse_composer_regions(screen, true) else {
        return false;
    };
    if !muse_editor_idle(&transcript, &composer) {
        return false;
    }
    let lines = splitlines(screen);
    let Some(bottom) = lines.iter().rposition(|line| is_divider(line)) else {
        return false;
    };
    muse_footer_modes(&lines[bottom + 1..]) == ["Auto-review"]
}

/// Recognize an idle editor after exact live-Muse process verification.
pub(crate) fn muse_verified_process_idle_composer(screen: &str) -> bool {
    muse_composer_regions(screen, false)
        .is_some_and(|(transcript, composer)| muse_editor_idle(&transcript, &composer))
}

/// Recognize a possibly nonempty editor after live-Muse process verification.
pub(crate) fn muse_verified_process_composer(screen: &str) -> bool {
    muse_composer_regions(screen, false)
        .is_some_and(|(_, composer)| muse_editor_segments(&composer).is_some())
}

/// Recognize Muse's explicit paused-goal state in a verified UI frame.
///
/// Only the footer region below the last rule counts, so a transcript line that merely quotes
/// `Goal (paused)` never pauses delivery.
pub(crate) fn muse_verified_process_goal_paused(screen: &str) -> bool {
    if muse_composer_regions(screen, false).is_none() {
        return false;
    }
    let lines = splitlines(screen);
    let Some(bottom) = lines.iter().rposition(|line| is_divider(line)) else {
        return false;
    };
    lines[bottom + 1..]
        .iter()
        .any(|line| strip(line) == GOAL_PAUSED)
}

/// The status a pinned Muse pane reports: paused, idle, staged or working, in that order.
///
/// A frame whose transcript ends in Muse's progress line is working, never idle or staged.
/// `reported_status` is Herdr's own report for the pane. A paused goal outranks an empty editor, and only `idle` admits input, so `paused` and
/// `staged` leave queued input queued until the owner resumes the goal or clears the editor. A
/// headerless (verified-process) frame counts only while Herdr itself reports the pane idle or
/// done; a frame with the versioned header is idle on its own evidence.
pub(crate) fn muse_verified_pane_status(reported_status: &str, screen: &str) -> &'static str {
    let reported_idle = matches!(reported_status, "idle" | "done");
    if reported_idle && muse_verified_process_goal_paused(screen) {
        "paused"
    } else if muse_idle_composer(screen)
        || (reported_idle && muse_verified_process_idle_composer(screen))
    {
        "idle"
    } else if reported_idle
        && muse_verified_process_composer(screen)
        && !muse_verified_process_turn_running(screen)
    {
        "staged"
    } else {
        "working"
    }
}

/// Return normalized editor lines after one leading Muse prompt marker.
fn muse_editor_segments(editor: &str) -> Option<Vec<String>> {
    let lines = splitlines(editor)
        .into_iter()
        .map(normalize)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let first = lines.first()?;
    let marker = PROMPT_MARKERS.iter().find(|candidate| {
        first == *candidate
            || first
                .strip_prefix(*candidate)
                .is_some_and(|rest| rest.starts_with(' '))
    })?;
    let initial = strip(&first[marker.len()..]);
    let mut segments = Vec::with_capacity(lines.len());
    if !initial.is_empty() {
        segments.push(initial.to_owned());
    }
    segments.extend(lines[1..].iter().cloned());
    Some(segments)
}

/// Match a logical prompt against one conservatively rendered Muse editor.
///
/// Each rendered line joins the previous one with a space, or with nothing after a line that ends
/// in a hyphen, because Muse may wrap a hyphenated word there. `exact` requires the whole editor
/// to be the prompt; otherwise the editor need only start with it as whole words.
fn muse_editor_matches(editor: &str, text: &str, exact: bool) -> bool {
    let wanted = normalize(text);
    let segments = muse_editor_segments(editor);
    let Some(segments) = segments.filter(|_| !wanted.is_empty()) else {
        return false;
    };
    let wanted_word = format!("{wanted} ");
    let mut prefixes = BTreeSet::from([String::new()]);
    let mut previous = "";
    for (index, segment) in segments.iter().enumerate() {
        let separators: &[&str] = if index == 0 {
            &[""]
        } else if previous.ends_with('-') {
            &[" ", ""]
        } else {
            &[" "]
        };
        let mut next_prefixes = BTreeSet::new();
        for prefix in &prefixes {
            for separator in separators {
                let rendered = format!("{prefix}{separator}{segment}");
                if !exact && (rendered == wanted || rendered.starts_with(&wanted_word)) {
                    return true;
                }
                if wanted.starts_with(&rendered) {
                    next_prefixes.insert(rendered);
                }
            }
        }
        if next_prefixes.is_empty() {
            return false;
        }
        prefixes = next_prefixes;
        previous = segment;
    }
    prefixes.contains(&wanted)
}

fn muse_prompt_in_composer_region(
    screen: &str,
    text: &str,
    require_header: bool,
    exact: bool,
) -> bool {
    muse_composer_regions(screen, require_header)
        .is_some_and(|(_, composer)| muse_editor_matches(&composer, text, exact))
}

/// Conservatively detect a prompt retained in Muse's bottom editor.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "only tests call it, as in Python")
)]
pub(crate) fn muse_prompt_in_composer(screen: &str, text: &str) -> bool {
    muse_prompt_in_composer_region(screen, text, true, false)
}

/// Detect retained input after exact live-Muse process verification.
// Like the other verified-process prompt proofs and `muse_prompt_in_transcript`, nothing outside
// the tests calls it, in this edition or in py/agentctl/client.py; it completes the ported Muse
// API whose meaning those tests pin.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "only tests call it, as in Python")
)]
pub(crate) fn muse_verified_process_prompt_in_composer(screen: &str, text: &str) -> bool {
    muse_prompt_in_composer_region(screen, text, false, false)
}

/// The character count Muse shows for a paste of `text`: code points, CRLF as one.
fn muse_paste_length(text: &str) -> usize {
    text.chars().count() - text.matches("\r\n").count()
}

/// Whether the editor is exactly Muse's collapsed placeholder for a paste of `text`.
///
/// Muse 1.4 collapses a long paste into one editor line, `[Pasted Content N chars]`.
fn muse_editor_holds_paste(editor: &str, text: &str) -> bool {
    let Some(segments) = muse_editor_segments(editor) else {
        return false;
    };
    let [segment] = segments.as_slice() else {
        return false;
    };
    segment
        .strip_prefix("[Pasted Content ")
        .and_then(|rest| rest.strip_suffix(" chars]"))
        .filter(|count| !count.is_empty() && count.bytes().all(|value| value.is_ascii_digit()))
        .and_then(|count| count.parse::<usize>().ok())
        == Some(muse_paste_length(text))
}

/// Require the whole editor to be this paste after pinning the live Muse process.
///
/// The editor shows the literal text, or, for a long paste, only Muse's placeholder with the
/// paste's exact character count. Delivery proves the editor empty before its one paste.
pub(crate) fn muse_verified_process_paste_is_exact_composer(screen: &str, text: &str) -> bool {
    muse_composer_regions(screen, false).is_some_and(|(_, editor)| {
        muse_editor_matches(&editor, text, true) || muse_editor_holds_paste(&editor, text)
    })
}

/// Detect this paste retained in the editor, as text or as its placeholder.
pub(crate) fn muse_verified_process_paste_in_composer(screen: &str, text: &str) -> bool {
    muse_composer_regions(screen, false).is_some_and(|(_, editor)| {
        muse_editor_matches(&editor, text, false) || muse_editor_holds_paste(&editor, text)
    })
}

/// Require the entire active Muse editor to be this literal prompt.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "only tests call it, as in Python")
)]
pub(crate) fn muse_prompt_is_exact_composer(screen: &str, text: &str) -> bool {
    muse_prompt_in_composer_region(screen, text, true, true)
}

/// Require exact editor text after pinning the live Muse process.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "only tests call it, as in Python")
)]
pub(crate) fn muse_verified_process_prompt_is_exact_composer(screen: &str, text: &str) -> bool {
    muse_prompt_in_composer_region(screen, text, false, true)
}

/// Count complete marker-delimited user turns, never prompt prefixes.
///
/// A turn starts at a prompt marker and runs to the next answer or prompt marker, or to Muse's
/// `Working...`/`Thinking...` line. An assistant answer that echoes the prompt is never a turn.
fn muse_transcript_prompt_count(transcript: &str, text: &str) -> usize {
    if normalize(text).is_empty() {
        return 0;
    }
    let lines = splitlines(transcript);
    let mut count = 0;
    let mut index = 0;
    while index < lines.len() {
        if !starts_with_marker(strip(lines[index]), &PROMPT_MARKERS) {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while end < lines.len() {
            let next_line = strip(lines[end]);
            if starts_with_marker(next_line, &TURN_END_MARKERS)
                || matches!(
                    next_line,
                    "Working..." | "Working\u{2026}" | "Thinking..." | "Thinking\u{2026}"
                )
            {
                break;
            }
            end += 1;
        }
        if muse_editor_matches(&lines[index..end].join("\n"), text, true) {
            count += 1;
        }
        index = end;
    }
    count
}

/// Require the exact prompt as a user turn above the active Muse editor.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "only tests call it, as in Python")
)]
pub(crate) fn muse_prompt_in_transcript(screen: &str, text: &str) -> bool {
    muse_prompt_transcript_count(screen, text) > 0
}

/// Count bounded prompt renderings above the composer for transition proofs.
pub(crate) fn muse_prompt_transcript_count(screen: &str, text: &str) -> usize {
    muse_composer_regions(screen, true).map_or(0, |(transcript, _)| {
        muse_transcript_prompt_count(&transcript, text)
    })
}

/// Count exact user turns after pinning the live Muse process.
pub(crate) fn muse_verified_process_prompt_transcript_count(screen: &str, text: &str) -> usize {
    muse_composer_regions(screen, false).map_or(0, |(transcript, _)| {
        muse_transcript_prompt_count(&transcript, text)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from py/tests/test_agentctl_client_launch.py.

    #[test]
    fn current_muse_yolo_footer_is_an_idle_composer() {
        let screen = "Muse Code at Meta (https://fb.workplace.com/groups/27315719428107177)\n\
                      Using AI Gateway (Meta Model API upstream)\n\n  Muse Code 1.4.0\n\n\
                      ────────────────\n❯\n────────────────\n  \
                      kiki_gb300_mxfp8_6p2_840_nwr · xhigh · /work/project · YOLO\n";
        assert!(muse_idle_composer(screen));
        assert!(!muse_auto_review_idle_composer(&screen.replace(
            "Using AI Gateway",
            "Auto-review was previously enabled"
        )));
        let ambiguous = screen.replace(
            "  kiki_gb300_mxfp8_6p2_840_nwr · xhigh · /work/project · YOLO\n",
            "  kiki · xhigh · /work/project · Auto-review\n  \
             kiki_gb300_mxfp8_6p2_840_nwr · xhigh · /work/project · YOLO\n",
        );
        assert!(!muse_idle_composer(&ambiguous));
        assert!(!muse_auto_review_idle_composer(&ambiguous));
        let choice = screen.replace("❯\n", "❯ Yes, continue\n  No, exit\n");
        assert!(!muse_idle_composer(&choice));
    }

    #[test]
    fn muse_launch_overrides_footer_is_an_idle_composer() {
        // Captured 2026-10-19 from a live Muse launched with extra harness
        // arguments (`--approval-mode never` among them): the status footer
        // reports `Launch overrides` where a settings-default launch reports
        // its approval mode. This frame timed out `start` as launch_failed
        // (#382 muse-approval-never-ready).
        let screen = "Muse Code 1.4.0\n\
                      ◆ prior answer\n\n\
                      ────────────────\n❯\n────────────────\n  \
                      watermelon-full-preview · xhigh · /work/project · Launch overrides\n";
        assert!(muse_idle_composer(screen));
        assert!(muse_verified_process_idle_composer(screen));
        assert!(!muse_auto_review_idle_composer(screen));
        assert_eq!(muse_verified_pane_status("working", screen), "idle");
        let ambiguous = screen.replace(
            "Launch overrides\n",
            "Launch overrides\n  kiki · xhigh · /work/project · YOLO\n",
        );
        assert!(!muse_idle_composer(&ambiguous));
        let choice = screen.replace("❯\n", "❯ Yes, continue\n  No, exit\n");
        assert!(!muse_idle_composer(&choice));
    }

    /// The captured frames of `testdata/muse-frames.json` and the exact texts pasted into them.
    fn captured() -> (serde_json::Value, serde_json::Value) {
        let corpus: serde_json::Value =
            serde_json::from_str(include_str!("../../testdata/muse-frames.json")).unwrap();
        (corpus["frames"].clone(), corpus["briefs"].clone())
    }

    #[test]
    fn captured_launch_overrides_frames_carry_the_delivery_proofs() {
        // Muse 1.4.12 launched with --trust-workspace --approval-mode never (#395
        // muse-launch-overrides-input): an empty editor, then a 3214-character brief that Muse
        // collapses to a placeholder, then the submitted turn.
        let (frames, briefs) = captured();
        let frame = |name: &str| frames[name].as_str().unwrap().to_owned();
        let brief = briefs["long"].as_str().unwrap();
        let idle = frame("launch-overrides-idle");
        assert!(muse_idle_composer(&idle));
        assert!(muse_verified_process_idle_composer(&idle));
        assert!(!muse_auto_review_idle_composer(&idle));
        assert_eq!(muse_verified_pane_status("working", &idle), "idle");
        // The same editor under the Auto-review footer.
        let auto_review = idle.replace("· Launch overrides", "· Auto-review");
        assert!(muse_auto_review_idle_composer(&auto_review));
        assert!(muse_verified_process_idle_composer(&auto_review));

        let staged = frame("launch-overrides-placeholder");
        assert!(staged.contains("[Pasted Content 3214 chars]"));
        assert_eq!(muse_paste_length(brief), 3214);
        assert!(muse_verified_process_paste_is_exact_composer(
            &staged, brief
        ));
        assert!(muse_verified_process_paste_in_composer(&staged, brief));
        // The literal-text proofs cannot see a collapsed paste.
        assert!(!muse_prompt_is_exact_composer(&staged, brief));
        // The count must be exact.
        assert!(!muse_verified_process_paste_is_exact_composer(
            &staged,
            &brief[1..]
        ));
        assert!(!muse_verified_process_paste_is_exact_composer(
            &staged,
            &format!("{brief}x")
        ));
        assert!(!muse_verified_process_idle_composer(&staged));
        assert_eq!(muse_verified_pane_status("idle", &staged), "staged");
        assert_eq!(
            muse_verified_process_prompt_transcript_count(&staged, brief),
            0
        );

        // Right after Enter a hook notice shares the footer line, and the turn ends at Muse's
        // progress marker instead of an answer.
        let submitted = frame("launch-overrides-submitted");
        assert!(submitted.contains("Launch overrides   Hook · UserPromptSubmit"));
        assert!(submitted.contains("◈ Thinking (3s · esc to interrupt)"));
        assert_eq!(
            muse_verified_process_prompt_transcript_count(&submitted, brief),
            1
        );
        assert!(!muse_verified_process_paste_in_composer(&submitted, brief));
        // The empty editor below Muse's progress line is a running turn, never idle input.
        assert!(!muse_idle_composer(&submitted));
        assert!(!muse_verified_process_idle_composer(&submitted));
        assert!(muse_verified_process_turn_running(&submitted));
        assert_eq!(muse_verified_pane_status("idle", &submitted), "working");
        assert!(!muse_auto_review_idle_composer(
            &submitted.replace("· Launch overrides", "· Auto-review")
        ));
        let double_checking = submitted.replace(
            "◈ Thinking (3s · esc to interrupt)",
            "◈ Double checking (1s · esc to interrupt)",
        );
        assert!(!muse_verified_process_idle_composer(&double_checking));
        // A finished answer below the turn leaves the editor idle again.
        assert!(muse_idle_composer(
            &submitted.replace("◈ Thinking (3s · esc to interrupt)", "◆ RECEIVED 40")
        ));
        assert_eq!(
            muse_verified_process_prompt_transcript_count(
                &submitted.replace("◈ Thinking", "Still thinking"),
                brief
            ),
            0
        );

        let answered = frame("launch-overrides-answered");
        assert_eq!(muse_prompt_transcript_count(&answered, brief), 1);
        assert_eq!(
            muse_verified_process_prompt_transcript_count(&answered, brief),
            1
        );
        assert!(!muse_verified_process_paste_in_composer(&answered, brief));
        assert!(muse_idle_composer(&answered));
    }

    #[test]
    fn captured_headerless_frames_count_pastes_in_code_points() {
        // After a few turns Muse 1.4 has scrolled its versioned header out of the frame, so only
        // the verified-process proofs, which rely on the pinned process instead, can apply.
        let (frames, briefs) = captured();
        let frame = |name: &str| frames[name].as_str().unwrap().to_owned();
        let staged = frame("headerless-placeholder");
        let brief = briefs["headerless"].as_str().unwrap();
        assert!(!staged.contains("Muse Code 1."));
        assert!(!muse_idle_composer(
            &staged.replace("❯ [Pasted Content 2939 chars]", "❯")
        ));
        assert!(muse_verified_process_idle_composer(
            &staged.replace("❯ [Pasted Content 2939 chars]", "❯")
        ));
        assert!(muse_verified_process_paste_is_exact_composer(
            &staged, brief
        ));

        // Read while Muse's paste hint shared the footer line.
        let hinted = frame("headerless-placeholder-paste-hint");
        let hint_brief = briefs["paste-hint"].as_str().unwrap();
        assert!(hinted.contains("Launch overrides             Paste again to expand for editing"));
        assert!(muse_verified_process_paste_is_exact_composer(
            &hinted, hint_brief
        ));
        assert_eq!(muse_verified_pane_status("idle", &hinted), "staged");
        // Only a known notice after a gap of three spaces is set apart, and the footer still
        // needs one mode; any other text keeps the line no footer at all.
        let footer = "watermelon-full-preview · xhigh · ~/work/project · Launch overrides";
        for line in [
            format!("{footer}  Paste again to expand for editing"),
            format!("{footer}X   Paste again to expand for editing"),
            format!("{footer}   Goal (paused)"),
            format!("{footer}   Paste again to expand"),
            format!("{footer}   Hook"),
            "m · high · /my   dir · Auto-review   Paste again to expand for editing".to_owned(),
            "   Paste again to expand for editing".to_owned(),
        ] {
            assert_eq!(muse_status_mode(strip(&line)), None, "{line}");
        }
        assert_eq!(
            muse_status_mode("model · high · /work · YOLO\u{3000}\t Hook · UserPromptSubmit"),
            Some("YOLO")
        );

        // 3175 code points, 3824 bytes and 3234 UTF-16 units with one CRLF: Muse shows 3174.
        let unicode = briefs["unicode-crlf"].as_str().unwrap();
        assert_eq!(unicode.chars().count(), 3175);
        assert_eq!(unicode.matches("\r\n").count(), 1);
        let staged = frame("headerless-unicode-placeholder");
        assert!(staged.contains("[Pasted Content 3174 chars]"));
        assert!(muse_verified_process_paste_is_exact_composer(
            &staged, unicode
        ));
        assert!(muse_verified_process_paste_is_exact_composer(
            &staged,
            &unicode.replace("\r\n", "\n")
        ));
        assert!(!muse_verified_process_paste_is_exact_composer(
            &staged,
            &unicode.replace("\r\n", "\n\n")
        ));
        let answered = frame("headerless-unicode-answered");
        assert_eq!(muse_prompt_transcript_count(&answered, unicode), 0);
        assert_eq!(
            muse_verified_process_prompt_transcript_count(&answered, unicode),
            1
        );
        assert!(!muse_verified_process_paste_in_composer(&answered, unicode));
    }

    #[test]
    fn verified_headerless_muse_composer_distinguishes_buffered_input() {
        let divider = "─".repeat(40);
        let footer = "kiki · xhigh · /work/project · YOLO\n";
        let idle = format!("old transcript\n{divider}\n❯\n{divider}\n{footer}");
        assert!(!muse_idle_composer(&idle));
        assert!(muse_verified_process_idle_composer(&idle));
        assert!(muse_verified_process_composer(&idle));

        let prompt = "require the full deterministic-scheduling-review skill";
        let staged = idle.replace("❯\n", &format!("❯ {prompt}\n"));
        assert!(!muse_prompt_is_exact_composer(&staged, prompt));
        assert!(muse_verified_process_prompt_is_exact_composer(
            &staged, prompt
        ));
        assert!(muse_verified_process_prompt_in_composer(&staged, prompt));
        let accepted = idle.replace("old transcript\n", &format!("❯ {prompt}\n◆ Working\n"));
        assert_eq!(
            muse_verified_process_prompt_transcript_count(&accepted, prompt),
            1
        );
    }

    #[test]
    fn verified_muse_paused_goal_requires_footer_region() {
        let divider = "─".repeat(40);
        let footer = "kiki · xhigh · /work/project · YOLO";
        let paused = format!("Muse Code 1.4.0\n{divider}\n❯\n{divider}\nGoal (paused)\n{footer}\n");
        let transcript_echo =
            format!("Muse Code 1.4.0\nGoal (paused)\n{divider}\n❯\n{divider}\n{footer}\n");
        assert!(muse_verified_process_goal_paused(&paused));
        assert!(!muse_verified_process_goal_paused(&transcript_echo));
    }

    #[test]
    fn muse_prompt_must_move_from_composer_to_transcript() {
        let prompt = "literal $(unexpanded) delivery\nsecond line";
        let header = "Muse Code 1.3.0\n";
        let divider = "────────────────\n";
        let footer = "watermelon-preview · xhigh · /work/project · Auto-review\n";
        let staged = format!("{header}{divider}❯ {prompt}\n{divider}{footer}");
        let accepted = format!("{header}❯ {prompt}\nWorking...\n{divider}❯\n{divider}{footer}");
        let accepted_current =
            format!("{header}❯ {prompt}\n◆ Working...\n{divider}❯\n{divider}{footer}");
        let error_redraw = format!("{header}{divider}❯ {prompt}\nError: retry\n{divider}{footer}");
        assert!(muse_prompt_in_composer(&staged, prompt));
        assert!(!muse_prompt_in_transcript(&staged, prompt));
        assert!(muse_prompt_in_transcript(&accepted, prompt));
        assert!(muse_prompt_in_transcript(&accepted_current, prompt));
        assert!(!muse_prompt_in_composer(&accepted, prompt));
        assert!(muse_prompt_in_composer(&error_redraw, prompt));
        assert!(!muse_prompt_in_transcript(&error_redraw, prompt));
        let repeated_staged =
            format!("{header}❯ {prompt}\n◆ prior answer\n{divider}❯ {prompt}\n{divider}{footer}");
        let cleared_without_submit =
            format!("{header}❯ {prompt}\n◆ prior answer\n{divider}❯\n{divider}{footer}");
        assert_eq!(muse_prompt_transcript_count(&repeated_staged, prompt), 1);
        assert_eq!(
            muse_prompt_transcript_count(&cleared_without_submit, prompt),
            1
        );
    }

    #[test]
    fn custom_muse_launch_frames_never_read_as_idle() {
        // The screens of test_custom_muse_launch_never_accepts_a_trust_prompt and
        // test_custom_muse_launch_rejects_bare_prompt_without_idle_footer.
        for screen in [
            "Do you trust this workspace?\n❯ Yes\n  No\n",
            "A choice dialog\n❯\n",
        ] {
            assert!(!muse_idle_composer(screen));
            assert!(!muse_verified_process_idle_composer(screen));
            assert!(!muse_verified_process_composer(screen));
            assert_eq!(muse_verified_pane_status("idle", screen), "working");
        }
    }

    // New: the proofs the Rust edition used to approximate.

    #[test]
    fn an_echoed_prompt_does_not_satisfy_the_transcript_proof() {
        let prompt = "summarize the open review threads";
        let header = "Muse Code 1.4.0\n";
        let divider = "────────────────\n";
        let footer = "kiki · xhigh · /work/project · Auto-review\n";
        // The answer quotes the prompt: no user turn shows it, so nothing was submitted.
        let echoed = format!(
            "{header}❯ an earlier prompt\n◆ You asked me to {prompt}, later.\n\
             {prompt}\n{divider}❯\n{divider}{footer}"
        );
        assert_eq!(muse_prompt_transcript_count(&echoed, prompt), 0);
        assert!(!muse_prompt_in_transcript(&echoed, prompt));
        // A turn that starts with the prompt but continues is a different prompt.
        let longer =
            format!("{header}❯ {prompt} and the rest\n◆ ok\n{divider}❯\n{divider}{footer}");
        assert_eq!(muse_prompt_transcript_count(&longer, prompt), 0);
        // The prompt quoted later in the editor is not retained input.
        let quoted = format!("{header}{divider}❯ please {prompt}\n{divider}{footer}");
        assert!(!muse_prompt_in_composer(&quoted, prompt));
        let real = format!("{header}❯ {prompt}\n◆ Working...\n{divider}❯\n{divider}{footer}");
        assert_eq!(muse_prompt_transcript_count(&real, prompt), 1);
        let thinking =
            format!("{header}❯ {prompt}\nThinking\u{2026}\n{divider}❯\n{divider}{footer}");
        assert_eq!(muse_prompt_transcript_count(&thinking, prompt), 1);
    }

    #[test]
    fn a_hyphenated_word_wrapped_by_muse_still_matches_exactly() {
        let header = "Muse Code 1.4.0\n";
        let divider = "────────────────\n";
        let footer = "kiki · xhigh · /work/project · Auto-review\n";
        let frame = |editor: &str| format!("{header}{divider}{editor}\n{divider}{footer}");
        let prompt = "keep the self-contained proof";
        assert!(muse_prompt_is_exact_composer(
            &frame("❯ keep the self-\ncontained proof"),
            prompt
        ));
        assert!(muse_prompt_is_exact_composer(
            &frame("❯ keep the self-\n  contained   proof"),
            prompt
        ));
        // Only a hyphen allows the unspaced join.
        assert!(!muse_prompt_is_exact_composer(
            &frame("❯ keep the self\ncontained proof"),
            prompt
        ));
        // A spaced join of the hyphenated line is still the literal text.
        assert!(muse_prompt_is_exact_composer(&frame("❯ a -\nb"), "a - b"));
        assert!(!muse_prompt_is_exact_composer(&frame("❯"), prompt));
        assert!(!muse_prompt_is_exact_composer(&frame("❯keep"), "keep"));
        assert!(!muse_prompt_is_exact_composer(&frame("❯ keep"), " \t "));
        // Retained input starts with the prompt as whole words.
        assert!(muse_prompt_in_composer(
            &frame("❯ keep the self-\ncontained proof then more"),
            prompt
        ));
        assert!(!muse_prompt_in_composer(
            &frame("❯ keep the self-\ncontained proof, then more"),
            prompt
        ));
    }

    #[test]
    fn muse_status_order_is_paused_idle_staged_working() {
        let divider = "─".repeat(40);
        let footer = "kiki · xhigh · /work · YOLO\n";
        let idle = format!("old transcript\n{divider}\n❯\n{divider}\n{footer}");
        let staged = idle.replace("❯\n", "❯ queued owner prompt\n");
        let paused = idle.replace(footer, &format!("Goal (paused)\n{footer}"));
        let paused_staged = staged.replace(footer, &format!("Goal (paused)\n{footer}"));
        let headed = format!("Muse Code 1.4.0\n{idle}");
        let headed_paused = format!("Muse Code 1.4.0\n{paused}");
        for reported in ["idle", "done"] {
            assert_eq!(muse_verified_pane_status(reported, &idle), "idle");
            assert_eq!(muse_verified_pane_status(reported, &staged), "staged");
            assert_eq!(muse_verified_pane_status(reported, &paused), "paused");
            assert_eq!(
                muse_verified_pane_status(reported, &paused_staged),
                "paused"
            );
            assert_eq!(
                muse_verified_pane_status(reported, &headed_paused),
                "paused"
            );
        }
        for reported in ["working", "unknown", "blocked"] {
            // Headerless evidence counts only while Herdr agrees the pane is idle.
            assert_eq!(muse_verified_pane_status(reported, &idle), "working");
            assert_eq!(muse_verified_pane_status(reported, &staged), "working");
            assert_eq!(muse_verified_pane_status(reported, &paused), "working");
            // The versioned header is evidence on its own, but never pauses.
            assert_eq!(muse_verified_pane_status(reported, &headed), "idle");
            assert_eq!(muse_verified_pane_status(reported, &headed_paused), "idle");
        }
        let echo = format!("Goal (paused)\n{idle}");
        assert_eq!(muse_verified_pane_status("done", &echo), "idle");
    }

    #[test]
    fn python_string_semantics_are_reproduced() {
        assert_eq!(splitlines(""), Vec::<&str>::new());
        assert_eq!(splitlines("\n"), vec![""]);
        assert_eq!(splitlines("a\n\nb\n\n"), vec!["a", "", "b", ""]);
        assert_eq!(
            splitlines("a\r\nb\rc\u{0b}d\u{0c}e"),
            vec!["a", "b", "c", "d", "e"]
        );
        assert_eq!(
            splitlines("a\u{1c}b\u{1d}c\u{1e}d\u{85}e\u{2028}f\u{2029}g\u{1f}h"),
            vec!["a", "b", "c", "d", "e", "f", "g\u{1f}h"]
        );
        assert_eq!(normalize("\u{1f} a\u{a0}\u{3000}b\t\u{1c}"), "a b");
        assert_eq!(strip("\u{1d} ❯ \u{2009}"), "❯");
        // A line separator inside a prompt is a line break to both editions.
        let header = "Muse Code 1.4.0\n";
        let divider = "────────────────\n";
        let footer = "kiki · xhigh · /work/project · Auto-review\n";
        let staged = format!("{header}{divider}❯ first\u{2028}second\n{divider}{footer}");
        assert!(muse_prompt_is_exact_composer(&staged, "first second"));
        assert!(muse_prompt_is_exact_composer(&staged, "first\u{1f}second"));
    }
}
