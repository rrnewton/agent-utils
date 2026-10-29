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
//! see [`muse_replacement_pending`].

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

/// Whether `screen` shows Muse's question whether to replace the current goal with
/// `objective`, with `Replace with new goal` selected, so Enter confirms the replacement.
///
/// ```text
///   Replace the current goal?
///   Current  Wait for the user to send the word GO; …
///   New      Say BYE once and consider the goal achieved.
///     1. Keep current goal
///   ⟩ 2. Replace with new goal
///   Enter to confirm · Esc to cancel
/// ```
#[must_use]
pub fn muse_replacement_pending(screen: &str, objective: &str) -> bool {
    let screen = compact(screen);
    screen.contains("Replacethecurrentgoal?")
        && screen.contains(&compact(&format!("New {objective}")))
        && screen.contains("⟩2.Replacewithnewgoal")
        && screen.contains("Entertoconfirm")
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

    #[test]
    fn muse_asks_before_replacing_a_goal() {
        let screen = format!(
            "{RULE}\n❯ /goal Say BYE once and consider the goal achieved.\n  Replace the current goal?\n  \
             Current  Wait for the user to send the word GO.\n           blocked · 0%\n  \
             New      Say BYE once and consider the goal achieved.\n  Replacing ends the current \
             goal. Its progress is not carried over.\n    1. Keep current goal\n  ⟩ 2. Replace with \
             new goal\n  To end it without a new goal: /goal clear · To reword it: /goal edit\n  \
             Enter to confirm · Esc to cancel\n{RULE}\n"
        );
        assert!(muse_replacement_pending(
            &screen,
            "Say BYE once and consider the goal achieved."
        ));
        assert!(!muse_replacement_pending(&screen, "Say HI once."));
        assert!(!muse_replacement_pending(
            &screen.replace("⟩ 2.", "  2.").replace("    1.", "  ⟩ 1."),
            "Say BYE once and consider the goal achieved."
        ));
    }
}
