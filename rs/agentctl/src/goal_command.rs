//! A `/goal` command typed raw into a harness's composer, and the screen proof that the harness
//! took it.
//!
//! Muse, Claude Code and Codex each have a native `/goal` command. None of them echoes it as an
//! ordinary user turn that the usual submission proofs look for, and a command such as `/goal
//! clear` starts no turn, so neither a printed prompt nor a working state can prove it was taken.
//! Each harness does print an answer to it, which is the proof here. Read from live sessions on
//! 2026-10-28 (Muse 1.4.15, Claude Code 2.1.310, Codex 0.163.0):
//!
//! | Harness | `/goal OBJECTIVE` | `/goal clear` |
//! |---|---|---|
//! | Muse | `◆ Goal set — OBJECTIVE`, footer `Goal (0s) 0% …` | `◆ Goal cleared — OBJECTIVE` |
//! | Claude Code | `❯ /goal OBJECTIVE`, then `⎿  Goal set: OBJECTIVE` | `⎿  No goal set` |
//! | Codex | `• Goal active Objective: OBJECTIVE`, footer `Pursuing goal (3s)` | `• Goal cleared` |
//!
//! A subcommand Muse cannot apply is answered on a row of its own too, such as `◆ completed goals
//! cannot be resumed`. When a goal is already set, Muse and Codex first ask whether to replace it;
//! see [`MuseReplacement`].

use crate::submission::compact;

/// Longest argument a `/goal` command may carry, in characters.
pub const MAX_ARGUMENT_CHARS: usize = 2000;

/// The subcommands of `/goal` that name no objective, as Muse lists them (`/goal
/// [<objective>|edit <objective>|clear|cancel|stop|pause|resume]`); Claude Code and Codex know
/// `clear` too.
const SUBCOMMANDS: [&str; 6] = ["clear", "cancel", "stop", "pause", "resume", "edit"];

/// Phrases with which the harnesses confirm a new objective, each followed by the objective.
const SET_PHRASES: [&str; 3] = ["Goal set — ", "Goal set: ", "Goal active Objective: "];

/// Glyphs that start a harness's answer row: Muse's `◆`, Claude Code's `⎿` and Codex's `•`.
const ANSWER_GLYPHS: [char; 3] = ['◆', '⎿', '•'];

/// The argument of `text` when it is exactly one `/goal` command with a nonempty argument: one
/// line, no control characters, at most [`MAX_ARGUMENT_CHARS`] characters. Anything else,
/// including `/goal` alone, which only opens Muse's completion hint, is `None`.
#[must_use]
pub fn argument(text: &str) -> Option<&str> {
    let text = text.trim();
    let rest = text.strip_prefix("/goal")?;
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let argument = rest.trim();
    (!argument.is_empty()
        && argument.chars().count() <= MAX_ARGUMENT_CHARS
        && !argument.chars().any(char::is_control))
    .then_some(argument)
}

/// Whether `argument` is a subcommand rather than a new objective.
#[must_use]
pub fn is_subcommand(argument: &str) -> bool {
    argument
        .split_whitespace()
        .next()
        .is_some_and(|word| SUBCOMMANDS.contains(&word))
}

/// How many times `screen` confirms `objective` as a new goal, compared without whitespace so a
/// confirmation wrapped across rows still counts.
fn set_confirmations(screen: &str, objective: &str) -> usize {
    let screen = compact(screen);
    SET_PHRASES
        .iter()
        .map(|phrase| {
            let wanted = compact(&format!("{phrase}{objective}"));
            screen.matches(&wanted).count()
        })
        .sum()
}

/// The answer rows on `screen`, oldest first, glyph and margin removed: rows that start with
/// one of [`ANSWER_GLYPHS`] and a space and that mention a goal.
fn answer_rows(screen: &str) -> Vec<String> {
    screen
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix(ANSWER_GLYPHS)?;
            if !rest.starts_with(' ') {
                return None;
            }
            let rest = rest.trim();
            rest.to_lowercase()
                .contains("goal")
                .then(|| rest.to_owned())
        })
        .collect()
}

/// The latest answer to a `/goal` command that `screen` shows, if any.
#[must_use]
pub fn latest_answer(screen: &str) -> Option<String> {
    answer_rows(screen).pop()
}

/// The harness's answer to `/goal ARGUMENT` when `after`, read after the command was typed,
/// proves the harness took it, against `before`, read just before typing. A new objective needs
/// one more confirmation of exactly that objective than `before` showed. A subcommand needs a
/// new answer row: more of them than `before` showed, or a different latest one.
#[must_use]
pub fn taken(before: &str, after: &str, argument: &str) -> Option<String> {
    if is_subcommand(argument) {
        let (old, new) = (answer_rows(before), answer_rows(after));
        let changed = new.len() > old.len() || (!new.is_empty() && new.last() != old.last());
        return changed.then(|| new.last().cloned()).flatten();
    }
    if set_confirmations(after, argument) <= set_confirmations(before, argument) {
        return None;
    }
    // The row that starts the confirmation; a long objective wraps onto rows of its own.
    answer_rows(after)
        .into_iter()
        .rev()
        .find(|row| {
            let row = compact(row);
            SET_PHRASES
                .iter()
                .any(|phrase| row.starts_with(&compact(phrase)))
        })
        .or_else(|| Some(format!("Goal set: {argument}")))
}

/// Muse's question whether to replace the goal already set, as one screen shows it.
///
/// ```text
///   Replace the current goal?
///   Current  Wait for the user to send the word GO before doing anything at all; until …
///            paused · 0%
///   New      Wait for the user to send the word GO before doing anything at all; until …
///   Replacing ends the current goal. Its progress is not carried over.
///     1. Keep current goal
///   ⟩ 2. Replace with new goal
///   To end it without a new goal: /goal clear · To reword it: /goal edit
///   Enter to confirm · Esc to cancel
/// ```
///
/// Muse draws each objective on one row and cuts a long one: at a word, followed by `…`, or,
/// as seen in a 170-column pane, mid-word at the pane's right edge with no `…` at all.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MuseReplacement {
    /// The current objective as shown, without a trailing `…`.
    pub current: String,
    /// Whether the current objective was cut.
    pub current_cut: bool,
    /// The current goal's state as shown under it, such as `active · 90%`.
    pub status: String,
    /// The new objective as shown, without a trailing `…`.
    pub new: String,
    /// Whether the new objective was cut.
    pub new_cut: bool,
    /// The selected option: 1 keeps the current goal, 2 replaces it.
    pub selected: Option<u8>,
}

/// What to do with Muse's replacement question for a `/goal OBJECTIVE` command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReplacementChoice {
    /// The new objective is the current one: keep the current goal, so its progress stays.
    Keep,
    /// The question names this new objective and `Replace with new goal` is selected: Enter.
    Replace,
    /// The question cannot be matched to the command: cancel it, for this reason.
    Cancel(String),
}

/// `text` with runs of whitespace as single spaces.
fn words(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether an objective Muse showed, cut or not, is `objective`. A cut one must be a nonempty
/// prefix of it; an uncut one must equal it, whitespace aside.
fn shows(shown: &str, cut: bool, objective: &str) -> bool {
    let (shown, objective) = (words(shown), words(objective));
    if cut {
        !shown.is_empty() && objective.starts_with(&shown)
    } else {
        shown == objective
    }
}

impl MuseReplacement {
    /// Read Muse's replacement question from `screen`, if it shows one.
    #[must_use]
    pub fn read(screen: &str) -> Option<Self> {
        let lines: Vec<&str> = screen.lines().map(str::trim_end).collect();
        let width = lines
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0);
        let start = lines
            .iter()
            .position(|line| line.trim() == "Replace the current goal?")?;
        let rest = &lines[start + 1..];
        if !rest
            .iter()
            .any(|line| line.trim().starts_with("Enter to confirm"))
        {
            return None;
        }
        let shown = |label: &str| -> Option<(usize, String, bool)> {
            rest.iter().enumerate().find_map(|(index, line)| {
                let text = line.trim_start().strip_prefix(label)?;
                if !text.starts_with(' ') {
                    return None;
                }
                let text = text.trim();
                let cut_by_edge = line.chars().count() >= width;
                let (text, cut_by_ellipsis) = match text.strip_suffix('…') {
                    Some(text) => (text.trim_end(), true),
                    None => (text, false),
                };
                Some((index, text.to_owned(), cut_by_edge || cut_by_ellipsis))
            })
        };
        let (current_at, current, current_cut) = shown("Current")?;
        let (_, new, new_cut) = shown("New")?;
        let status = rest
            .get(current_at + 1)
            .map(|line| line.trim())
            .filter(|line| !line.starts_with("New "))
            .unwrap_or_default()
            .to_owned();
        let selected = rest.iter().find_map(|line| {
            let line = line.trim();
            if line.starts_with("⟩ 1.") {
                Some(1)
            } else if line.starts_with("⟩ 2.") {
                Some(2)
            } else {
                None
            }
        });
        Some(Self {
            current,
            current_cut,
            status,
            new,
            new_cut,
            selected,
        })
    }

    /// What a `/goal OBJECTIVE` command should do with this question. The question must name
    /// `objective` as the new goal; when it names it as the current goal too, the goal is
    /// kept. Two objectives cut at the same place count as the same, which only the part Muse
    /// shows can tell.
    #[must_use]
    pub fn choice(&self, objective: &str) -> ReplacementChoice {
        if !shows(&self.new, self.new_cut, objective) {
            return ReplacementChoice::Cancel(
                "Muse asked about replacing the goal, but the new objective it showed is not the \
                 one sent"
                    .to_owned(),
            );
        }
        if shows(&self.current, self.current_cut, objective) {
            return ReplacementChoice::Keep;
        }
        if self.selected == Some(2) {
            ReplacementChoice::Replace
        } else {
            ReplacementChoice::Cancel(
                "Muse asked about replacing the goal with `Replace with new goal` not selected"
                    .to_owned(),
            )
        }
    }
}

/// Codex's question whether to replace the goal already set, as one screen shows it
/// (#1939 agentctl-codex-replace-goal-picker).
///
/// ```text
/// Replace goal?
/// New objective: OBJECTIVE
/// › 1. Replace current goal  Set the new objective and start it now
/// 2. Cancel  Keep the current goal
/// Press enter to confirm or esc to go back
/// ```
///
/// Strict, like the hooks trust dialog: the exact title, objective line,
/// both options and the confirm footer must all show, or this is not the
/// picker. The objective wraps at the pane's right edge, so a showing cut
/// there (or ended with `…`) is read as cut, as Muse's is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexReplacement {
    /// The new objective as shown, wrapped rows joined, without a trailing `…`.
    pub new: String,
    /// Whether the new objective was cut.
    pub new_cut: bool,
    /// The selected option: 1 replaces the current goal, 2 cancels.
    pub selected: Option<u8>,
    /// Option 1's label and description, as displayed.
    pub replace_label: String,
    /// Option 2's label and description, as displayed.
    pub cancel_label: String,
}

impl CodexReplacement {
    /// Read Codex's replacement question from `screen`, if it shows one.
    #[must_use]
    pub fn read(screen: &str) -> Option<Self> {
        let lines: Vec<&str> = screen.lines().map(str::trim_end).collect();
        // The LAST title on screen: a quoted picker in the transcript
        // above a live one is history, never the live question (#1939 D2).
        let start = lines.iter().rposition(|line| line.trim() == "Replace goal?")?;
        let rest = &lines[start + 1..];
        // The footer must end the picker: only status rows may follow
        // it, and no composer may stand below it, or the block is a
        // quote in the transcript rather than a live picker (#1939 D2).
        let footer_at = rest
            .iter()
            .position(|line| line.trim() == "Press enter to confirm or esc to go back")?;
        let status_row = |line: &str| {
            let line = line.trim();
            line.is_empty() || line.contains(" · ")
        };
        if !rest[footer_at + 1..].iter().all(|line| status_row(line)) {
            return None;
        }
        // The pane-width proxy for a cut showing is computed with the
        // objective rows, below (#1939 D3).
        // The objective line, plus any rows it wraps onto, up to option 1.
        let objective_at = rest.iter().position(|line| {
            line.trim_start()
                .strip_prefix("New objective:")
                .is_some_and(|text| text.starts_with(' '))
        })?;
        let mut objective_rows: Vec<&str> = Vec::new();
        for line in &rest[objective_at..] {
            let trimmed = line.trim();
            let body = trimmed
                .strip_prefix('›')
                .map_or(trimmed, |rest| rest.trim_start());
            if body.starts_with("1. ") || body.starts_with("2. ") {
                break;
            }
            objective_rows.push(line);
        }
        let first = objective_rows
            .first()?
            .trim_start()
            .strip_prefix("New objective:")?
            .trim()
            .to_owned();
        let mut new = first;
        for row in &objective_rows[1..] {
            new.push(' ');
            new.push_str(row.trim());
        }
        let last_row = objective_rows.last()?;
        // Cut only on a trailing ellipsis, or when the last objective
        // row reaches the widest non-objective line on screen (the
        // pane edge, as far as screen text can say) (#1939 D3).
        let objective_range =
            (start + 1 + objective_at)..(start + 1 + objective_at + objective_rows.len());
        let width = lines
            .iter()
            .enumerate()
            .filter(|(index, _)| !objective_range.contains(index))
            .map(|(_, line)| line.chars().count())
            .max()
            .unwrap_or(0);
        let new_cut = new.ends_with('…') || (width > 0 && last_row.chars().count() >= width);
        if new.ends_with('…') {
            new = new.trim_end_matches('…').trim_end().to_owned();
        }
        if new.is_empty() {
            return None;
        }
        // Both options, in order, with their exact descriptions.
        let option = |number: u8| -> Option<(String, bool)> {
            rest.iter().find_map(|line| {
                let trimmed = line.trim();
                let (selected, body) = match trimmed.strip_prefix('›') {
                    Some(rest) => (true, rest.trim_start()),
                    None => (false, trimmed),
                };
                let text = body.strip_prefix(&format!("{number}. "))?;
                Some((text.trim().to_owned(), selected))
            })
        };
        let (replace_label, replace_selected) = option(1)?;
        let (cancel_label, cancel_selected) = option(2)?;
        if !replace_label.starts_with("Replace current goal")
            || !replace_label.contains("Set the new objective and start it now")
            || !cancel_label.starts_with("Cancel")
            || !cancel_label.contains("Keep the current goal")
            || (replace_selected && cancel_selected)
        {
            return None;
        }
        Some(Self {
            new,
            new_cut,
            selected: if replace_selected {
                Some(1)
            } else if cancel_selected {
                Some(2)
            } else {
                None
            },
            replace_label,
            cancel_label,
        })
    }

    /// What a `/goal OBJECTIVE` command should do with this question: confirm
    /// only when the offered objective is the one sent and `Replace current
    /// goal` is selected. Anything else is never auto-answered.
    #[must_use]
    pub fn choice(&self, objective: &str) -> ReplacementChoice {
        if !codex_shows(&self.new, self.new_cut, objective) {
            return ReplacementChoice::Cancel(
                "Codex asked about replacing the goal, but the new objective it showed is not \
                 the one sent"
                    .to_owned(),
            );
        }
        if self.selected == Some(1) {
            ReplacementChoice::Replace
        } else {
            ReplacementChoice::Cancel(
                "Codex asked about replacing the goal with `Replace current goal` not selected"
                    .to_owned(),
            )
        }
    }
}

/// Whether Codex's showing, cut or not, is `objective` (#1939 D3): uncut
/// must equal it word for word; cut must agree on every whole word shown
/// except a cut last word, and the whole-word agreement must amount to
/// at least three words or sixteen characters, so a one-word or
/// one-character prefix never confirms a different longer goal.
fn codex_shows(shown: &str, cut: bool, objective: &str) -> bool {
    let (shown, objective) = (words(shown), words(objective));
    if !cut {
        return shown == objective;
    }
    let shown_words: Vec<&str> = shown.split(' ').filter(|word| !word.is_empty()).collect();
    let objective_words: Vec<&str> =
        objective.split(' ').filter(|word| !word.is_empty()).collect();
    let mut whole = 0;
    while whole < shown_words.len()
        && whole < objective_words.len()
        && shown_words[whole] == objective_words[whole]
    {
        whole += 1;
    }
    let whole_chars: usize = shown_words[..whole]
        .iter()
        .map(|word| word.chars().count())
        .sum();
    if whole < 3 && whole_chars < 16 {
        return false;
    }
    // Every shown word agrees whole, or only the last one is cut short.
    whole == shown_words.len()
        || (whole + 1 == shown_words.len()
            && whole < objective_words.len()
            && objective_words[whole].starts_with(shown_words[whole]))
}

/// Whether `screen` shows a question about replacing a goal, Muse's or Codex's, that a command
/// left open.
#[must_use]
pub fn replacement_question_open(screen: &str) -> bool {
    // Strict only (#1939 D5): loose text matching here once let a
    // timeout Escape over transcript text that merely quoted a picker.
    MuseReplacement::read(screen).is_some() || CodexReplacement::read(screen).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULE: &str = "──────────────────────────────────────────";

    fn muse(transcript: &str, footer_notice: &str) -> String {
        format!(
            "  Muse Code 1.4.15\n{transcript}\n{RULE}\n❯\n{RULE}\n  model · xhigh · /work · \
             Launch overrides{footer_notice}\n"
        )
    }

    #[test]
    fn only_a_whole_goal_command_with_an_argument_is_one() {
        assert_eq!(argument("/goal ship it"), Some("ship it"));
        assert_eq!(argument("  /goal   clear  "), Some("clear"));
        assert_eq!(argument("/goal"), None);
        assert_eq!(argument("/goal   "), None);
        assert_eq!(argument("/goals ship"), None);
        assert_eq!(argument("please /goal ship"), None);
        assert_eq!(argument("/goal ship\nand more"), None);
        assert_eq!(
            argument(&format!("/goal {}", "x".repeat(MAX_ARGUMENT_CHARS))).map(str::len),
            Some(MAX_ARGUMENT_CHARS)
        );
        assert_eq!(
            argument(&format!("/goal {}", "x".repeat(MAX_ARGUMENT_CHARS + 1))),
            None
        );
        assert!(is_subcommand("clear"));
        assert!(is_subcommand("edit a new wording"));
        // A first word that is a subcommand reads as one; its proof is then any new answer row,
        // which a harness that took it as an objective prints too.
        assert!(is_subcommand("clear the backlog"));
        assert!(!is_subcommand("ship the release"));
    }

    #[test]
    fn muse_takes_a_new_objective_and_a_clear() {
        let objective = "Reply with the single word HELLO and then consider the goal achieved.";
        let before = muse("◆ HELLO from earlier", "");
        let after = muse(
            &format!("◆ HELLO from earlier\n◆ Goal set — {objective}\n◇ Thinking (0s · esc to interrupt)"),
            "   Goal (0s) 0% ░░░░░░░░░░",
        );
        assert_eq!(
            taken(&before, &after, objective).as_deref(),
            Some(format!("Goal set — {objective}").as_str())
        );
        // Nothing new: no proof, whatever else changed.
        assert_eq!(taken(&after, &after, objective), None);
        // A confirmation of another objective is no proof of this one.
        assert_eq!(taken(&before, &after, "Say BYE once."), None);
        let cleared = muse(
            &format!("◆ Goal set — {objective}\n◆ HELLO\n◆ Goal cleared — {objective}"),
            "   cleared goal",
        );
        assert_eq!(
            taken(&after, &cleared, "clear").as_deref(),
            Some(format!("Goal cleared — {objective}").as_str())
        );
        let refused = format!("{cleared}◆ completed goals cannot be resumed\n");
        assert_eq!(
            taken(&cleared, &refused, "resume").as_deref(),
            Some("completed goals cannot be resumed")
        );
        assert_eq!(taken(&cleared, &cleared, "resume"), None);
    }

    #[test]
    fn a_wrapped_confirmation_still_counts() {
        let objective = "Wait for the user to send the word GO; until then say WAITING once and report that you are blocked.";
        let before = muse("◆ earlier", "");
        let after = muse(
            "◆ earlier\n◆ Goal set — Wait for the user to send the word GO; until then say WAITING once\n  and report that you are blocked.",
            "",
        );
        assert!(taken(&before, &after, objective).is_some());
    }

    #[test]
    fn claude_code_and_codex_answers_are_proof() {
        let objective = "Say HELLO once; the goal is met once you have said it.";
        let claude_before = format!("{RULE}\n❯ \n{RULE}\n  ⏵⏵ auto mode on\n");
        let claude_after = format!(
            "❯ /goal {objective}\n  ⎿  Goal set: {objective}\n● Goal noted.\n{RULE}\n❯ \n{RULE}\n"
        );
        assert_eq!(
            taken(&claude_before, &claude_after, objective).as_deref(),
            Some(format!("Goal set: {objective}").as_str())
        );
        let claude_cleared = format!("{claude_after}❯ /goal clear\n  ⎿  No goal set\n");
        assert_eq!(
            taken(&claude_after, &claude_cleared, "clear").as_deref(),
            Some("No goal set")
        );
        let codex_before = "› Ask Codex to do anything\n  GPT · /work\n";
        let codex_after = format!(
            "• Goal active Objective: {objective}\n↳ Hook · Discovered 19 skills\n› Ask Codex to do anything\n  GPT · /work   Pursuing goal (3s)\n"
        );
        assert_eq!(
            taken(codex_before, &codex_after, objective).as_deref(),
            Some(format!("Goal active Objective: {objective}").as_str())
        );
        let codex_cleared = format!("{codex_after}• Goal cleared\n");
        assert_eq!(
            taken(&codex_after, &codex_cleared, "clear").as_deref(),
            Some("Goal cleared")
        );
        // The echoed command alone is no answer.
        let echoed = format!("{claude_before}❯ /goal {objective}\n");
        assert_eq!(taken(&claude_before, &echoed, objective), None);
    }

    /// The question Muse 1.4.15 drew in a 120-column pane for an objective it already had,
    /// paused, both rows cut at a word and ended with `…`.
    fn probe_question(objective_row: &str) -> String {
        format!(
            "{RULE}\n❯ /goal Wait for the user\n  Replace the current goal?\n  Current  Wait for \
             the user to send the word GO before doing anything at all; until then say WAITING \
             exactly once and …\n           paused · 0%\n  New      {objective_row}\n  Replacing \
             ends the current goal. Its progress is not carried over.\n    1. Keep current goal\n  \
             ⟩ 2. Replace with new goal\n  To end it without a new goal: /goal clear · To reword \
             it: /goal edit\n  Enter to confirm · Esc to cancel\n{RULE}\n"
        )
    }

    #[test]
    fn muse_asks_before_replacing_a_goal() {
        let long =
            "Wait for the user to send the word GO before doing anything at all; until then \
                    say WAITING exactly once and report that you are blocked on the user.";
        let same = probe_question(
            "Wait for the user to send the word GO before doing anything at all; until then say \
             WAITING exactly once and …",
        );
        let question = MuseReplacement::read(&same).expect("question");
        assert!(question.current_cut && question.new_cut);
        assert_eq!(question.status, "paused · 0%");
        assert_eq!(question.selected, Some(2));
        assert_eq!(question.choice(long), ReplacementChoice::Keep);
        let other = probe_question("Say BYE once and consider the goal achieved.");
        let question = MuseReplacement::read(&other).expect("question");
        assert!(!question.new_cut);
        assert_eq!(
            question.choice("Say BYE once and consider the goal achieved."),
            ReplacementChoice::Replace
        );
        assert!(matches!(
            question.choice("Say HI once."),
            ReplacementChoice::Cancel(_)
        ));
        let keep_selected = other
            .replace("⟩ 2.", "  2.")
            .replace("    1. Keep", "  ⟩ 1. Keep");
        assert!(matches!(
            MuseReplacement::read(&keep_selected)
                .expect("question")
                .choice("Say BYE once and consider the goal achieved."),
            ReplacementChoice::Cancel(_)
        ));
        assert!(replacement_question_open(&other));
        assert!(!replacement_question_open(&format!("{RULE}\n❯\n{RULE}\n")));
    }

    #[test]
    fn the_question_from_a_live_coordinator_cut_mid_word_at_the_pane_edge_is_read() {
        // The geometry of a question read from a live Muse coordinator's 170-column pane on
        // 2026-10-28, its objective's private words replaced (testdata/muse-goal-replace-menu.txt
        // is that redaction, not the capture itself): both objectives are cut mid-word at the
        // right edge, with no `…`.
        let screen = include_str!("../testdata/muse-goal-replace-menu.txt");
        let objective = "Adopt ALL subagents and push all open workstreams to completion -- including \
                         but not limited to the drain of the reviewer's PRs, linux kernel debugging, \
                         reprobuilds of work, and the project test fixes plus split of the 3rd party \
                         code and preparation of the 0.5.5 cutover of source of truth. Also get the \
                         chat bridge debugged and functional until you can round trip a test \
                         message.";
        let question = MuseReplacement::read(screen).expect("question");
        assert!(question.current_cut && question.new_cut, "{question:?}");
        assert!(question.current.ends_with("linux kernel debugging, reprob"));
        assert_eq!(question.status, "active · 90%");
        assert_eq!(question.selected, Some(2));
        // The same objective, as the owner sent it: keep the goal and its progress.
        assert_eq!(question.choice(objective), ReplacementChoice::Keep);
        // One that differs within the part shown is no match at all.
        assert!(matches!(
            question.choice(&objective.replace("ALL", "SOME")),
            ReplacementChoice::Cancel(_)
        ));
        assert!(replacement_question_open(screen));
    }

    /// The Codex picker as recorded (#1939): option 1 highlighted.
    fn codex_picker(objective: &str, selected: u8) -> String {
        let mark = |number: u8| if number == selected { "› " } else { "" };
        format!(
            "Replace goal?\nNew objective: {objective}\n{}1. Replace current goal  Set the new \
             objective and start it now\n{}2. Cancel  Keep the current goal\nPress enter to \
             confirm or esc to go back\n",
            mark(1),
            mark(2)
        )
    }

    #[test]
    fn codex_asks_before_replacing_a_goal() {
        let screen = codex_picker("finish this task", 1);
        let question = CodexReplacement::read(&screen).expect("question");
        assert_eq!(question.new, "finish this task");
        assert_eq!(question.selected, Some(1));
        assert_eq!(
            question.choice("finish this task"),
            ReplacementChoice::Replace
        );
        // An offered goal that is not the one sent is never confirmed.
        assert!(matches!(
            question.choice("a different task"),
            ReplacementChoice::Cancel(_)
        ));
        // With Cancel highlighted, even the sent goal is not confirmed.
        let cancel = CodexReplacement::read(&codex_picker("finish this task", 2))
            .expect("question");
        assert_eq!(cancel.selected, Some(2));
        assert!(matches!(
            cancel.choice("finish this task"),
            ReplacementChoice::Cancel(_)
        ));
        assert!(replacement_question_open(&screen));
    }

    #[test]
    fn codex_picker_match_is_strict() {
        let screen = codex_picker("finish this task", 1);
        // No confirm footer, a renamed option, or a missing objective
        // line: none of these is the recorded picker.
        assert!(CodexReplacement::read(
            &screen.replace("Press enter to confirm or esc to go back", "esc to go back")
        )
        .is_none());
        assert!(CodexReplacement::read(
            &screen.replace("Replace current goal", "Replace the goal")
        )
        .is_none());
        assert!(CodexReplacement::read(&screen.replace("New objective: ", "Objective: ")).is_none());
        assert!(CodexReplacement::read("Replace goal?\n").is_none());
    }

    #[test]
    fn a_quoted_picker_in_the_transcript_is_not_a_live_picker() {
        // #1939 D2: a picker quoted above the composer is history.
        let quoted = format!(
            "{}\n› Ask Codex to do anything\n  GPT · /work\n",
            codex_picker("finish this task", 1)
        );
        assert!(CodexReplacement::read(&quoted).is_none());
        // A status row below the footer keeps it live.
        let live = format!(
            "{}\n  GPT-6 · /work   Pursuing goal (3s)\n",
            codex_picker("finish this task", 1)
        );
        assert!(CodexReplacement::read(&live).is_some());
        // Quoted (Cancel marked) above live (Replace marked): the
        // last block is the live one.
        let both = format!(
            "{}\n{}\n",
            codex_picker("an older task", 2),
            codex_picker("finish this task", 1)
        );
        let question = CodexReplacement::read(&both).expect("live picker");
        assert_eq!(question.new, "finish this task");
        assert_eq!(question.selected, Some(1));
        assert_eq!(
            question.choice("finish this task"),
            ReplacementChoice::Replace
        );
    }

    #[test]
    fn a_short_cut_showing_never_confirms_a_different_goal() {
        // #1939 D3: "f…" is not "force-push main", and one whole word
        // is not the longer goal it prefixes.
        let one_char = CodexReplacement::read(&codex_picker("f…", 1)).expect("picker");
        assert!(matches!(
            one_char.choice("force-push main"),
            ReplacementChoice::Cancel(_)
        ));
        let one_word = CodexReplacement::read(&codex_picker("ship…", 1)).expect("picker");
        assert!(matches!(
            one_word.choice("ship the release and report back"),
            ReplacementChoice::Cancel(_)
        ));
        let enough = CodexReplacement::read(&codex_picker("ship the release and report…", 1))
            .expect("picker");
        assert_eq!(
            enough.choice("ship the release and report back when green"),
            ReplacementChoice::Replace
        );
    }

    #[test]
    fn codex_wrapped_objective_still_matches_the_goal_sent() {
        // The objective wraps at the pane edge; the joined showing is
        // the sent objective, and a showing cut mid-way is its prefix.
        let objective = "ship the release and report back when the gate is green";
        let screen = "Replace goal?\nNew objective: ship the release and report back when the \
                      gate is green\n› 1. Replace current goal  Set the new objective and start \
                      it now\n2. Cancel  Keep the current goal\nPress enter to confirm or esc to \
                      go back\n";
        let question = CodexReplacement::read(screen).expect("question");
        assert_eq!(question.choice(objective), ReplacementChoice::Replace);
        let cut = "Replace goal?\nNew objective: ship the release and report back…\n› 1. Replace \
                   current goal  Set the new objective and start it now\n2. Cancel  Keep the \
                   current goal\nPress enter to confirm or esc to go back\n";
        let question = CodexReplacement::read(cut).expect("question");
        assert!(question.new_cut);
        assert_eq!(question.choice(objective), ReplacementChoice::Replace);
        assert!(matches!(
            question.choice("ship the release and report blue"),
            ReplacementChoice::Cancel(_)
        ));
    }
}
