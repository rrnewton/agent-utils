//! What agentctl knows about each coding-agent harness, one module per harness (#574
//! harness-adapters).
//!
//! agentctl drives harnesses it does not control through their terminal screens, their command
//! lines, and the files they write. Every such fact is specific to one harness and one of its
//! releases: the glyph Codex draws in front of its composer, the hint Claude Code draws under its
//! input box while messages wait, the argument that resumes a Muse conversation, where each one
//! writes its transcript. Those facts live here, behind [`HarnessAdapter`], with one module per
//! harness:
//!
//! * [`claude`], Claude Code;
//! * [`codex`], Codex;
//! * [`muse`], Muse;
//! * [`agentcloud`], remote agentcloud sessions, which have no local screen at all.
//!
//! Logic that holds for every harness (verified submission, the watcher's episode rules, queue
//! delivery) stays where it was and asks the adapter. Where a cross-harness reader must work on a
//! screen whose harness it does not know, such as the watcher's approval-prompt scan, it reads the
//! union of every adapter's table ([`approval_patterns`], [`queue_hints`], ...), so each phrase
//! still has exactly one owner.
//!
//! The inventory in `ai_docs/transient/2026-10-27-harness-adapters/README.md` lists every special
//! case, where it lives, and which are workarounds for upstream bugs. Forms and their answer keys
//! stay in [`crate::harness_tui`], which each adapter names.

use std::path::PathBuf;
use std::sync::OnceLock;

pub mod agentcloud;
pub mod claude;
pub mod codex;
pub mod muse;
pub mod transcript;

use crate::submission::ComposerView;

/// The conversations recorded arguments select, and the rest of the arguments.
pub type Conversations = Result<(Vec<String>, Vec<String>), String>;

/// When a working harness may be typed into with an ordinary follow-up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FollowupPolicy {
    /// Always: the harness steers or queues a follow-up itself (Codex).
    Always,
    /// Only when the client proves follow-ups reach the editor (Muse).
    WhenVerified,
    /// Only when the caller asked for delivery while working (Claude Code).
    OptIn,
    /// Never: the harness takes no typed input (agentcloud).
    Never,
}

/// Literal screen phrases and glyphs one harness draws, for the cross-harness readers that scan a
/// screen without knowing which harness drew it. Matching is case-sensitive unless a field says
/// otherwise.
#[derive(Debug)]
pub struct ScreenMarkers {
    /// Hints shown only while the harness holds queued input behind a running turn.
    pub queue_hints: &'static [&'static str],
    /// Approval, permission and trust prompt phrases, lowercased.
    pub approval: &'static [&'static str],
    /// Bullets that open an assistant block at column 0.
    pub reply_bullets: &'static [char],
    /// Glyphs that echo a user turn (`❯ text`) in the transcript.
    pub user_turn_glyphs: &'static [char],
    /// Glyphs that open a tool call's output under it.
    pub tool_output_glyphs: &'static [char],
    /// Glyphs that open the running-turn row at column 0.
    pub running_glyphs: &'static [char],
    /// Composer placeholders and hints that are not a draft.
    pub composer_placeholders: &'static [&'static str],
}

/// Facts about one harness's launch, screen and files. Every method has a default that is the
/// behaviour for a harness agentctl knows nothing about.
pub trait HarnessAdapter: Sync {
    /// The record's `harness` and Herdr's agent kind.
    fn name(&self) -> &'static str;
    /// The product name, for messages.
    fn display_name(&self) -> &'static str;
    /// Phrases and glyphs this harness draws.
    fn markers(&self) -> &'static ScreenMarkers;

    // Launch.

    /// Whether agentctl launches this harness locally with structured model, resume and
    /// reasoning presets.
    fn structured_launch(&self) -> bool {
        false
    }
    /// Native arguments for a structured model and conversation to resume: those that go before
    /// the caller's own arguments, and those that go after them.
    fn launch_arguments(
        &self,
        _model: Option<&str>,
        _resume: Option<&str>,
    ) -> (Vec<String>, Vec<String>) {
        (Vec::new(), Vec::new())
    }
    /// Native arguments for a structured reasoning effort, or `None` when unsupported.
    fn reasoning_arguments(&self, _effort: &str) -> Option<Vec<String>> {
        None
    }
    /// Split recorded arguments into the conversations they select and the rest, refusing
    /// anything ambiguous; `None` when launch specs cannot replay this harness.
    fn split_conversation(&self, _argv: &[String]) -> Option<Conversations> {
        None
    }
    /// `rest` (arguments without any conversation selector) starting a new conversation with
    /// the ID `conversation`, when the harness lets its caller choose a new conversation's ID;
    /// `None` otherwise.
    fn fresh_arguments(&self, _rest: Vec<String>, _conversation: &str) -> Option<Vec<String>> {
        None
    }
    /// `rest` (arguments without any conversation selector) with one explicit resume of
    /// `conversation`.
    fn replay_arguments(&self, rest: Vec<String>, conversation: &str) -> Vec<String> {
        let mut arguments = rest;
        arguments.extend(["resume".to_owned(), conversation.to_owned()]);
        arguments
    }
    /// Whether agentctl assigns the conversation id of a new conversation itself, so the record
    /// knows it before the harness starts.
    fn assigns_session_id(&self) -> bool {
        false
    }
    /// The directory under the user's home where the harness looks for skills, when it reads
    /// them from a fixed place.
    fn skill_directory(&self) -> Option<&'static str> {
        None
    }

    /// Native executable allowed to remain the pinned foreground group leader while
    /// helper children share its terminal. Wrappers are deliberately excluded.
    fn native_group_leader(&self, _executable: &std::path::Path) -> bool {
        false
    }

    // Screen.

    /// Whether [`crate::submission::submit_verified`] can read this harness's composer.
    fn verified_submission(&self) -> bool {
        false
    }
    /// Locate the composer among rendered rows, or `None` when it is not recognisable.
    fn composer_view(&self, _rows: &[(Vec<char>, Vec<char>)]) -> Option<ComposerView> {
        None
    }
    /// The key that submits a staged prompt, given the composer as it shows.
    fn submit_key(&self, _view: &ComposerView) -> &'static str {
        "Enter"
    }
    /// Whether `screen` shows this harness's workspace or folder trust dialog.
    fn trust_prompt(&self, _screen: &str) -> bool {
        false
    }
    /// A verdict to replace Herdr's `herdr_state` when the screen settles what Herdr's rules
    /// cannot; `None` keeps Herdr's.
    fn settle_herdr_state(&self, _herdr_state: &str, _screen: &str) -> Option<&'static str> {
        None
    }
    /// When a working pane takes an ordinary follow-up.
    fn followup_policy(&self) -> FollowupPolicy {
        FollowupPolicy::Never
    }
    /// The form classifier and answer-key policy, when agentctl answers this harness's forms.
    fn tui(&self) -> Option<&'static dyn crate::harness_tui::HarnessTui> {
        None
    }
    /// The classifier a queue drain uses to type a follow-up into a working pane.
    fn followup_tui(&self) -> Option<&'static dyn crate::harness_tui::HarnessTui> {
        self.tui()
    }

    /// Whether verified availability failures may receive an opt-in continue prompt.
    fn transient_recovery(&self) -> bool {
        false
    }
    /// Native command to resume the stalled goal visible on this screen, if supported.
    fn stalled_goal_resume(&self, _screen: &str) -> Option<&'static str> {
        None
    }

    // Transcript.

    /// The transcript file of the conversation `hints` describe, when it can be found.
    fn transcript_path(&self, _hints: &transcript::Hints<'_>) -> Option<PathBuf> {
        None
    }
    /// Other transcripts the same agent writes while its main transcript can be quiet (its
    /// sub-agents' sessions and threads), whose growth also shows it moving.
    fn related_transcripts(
        &self,
        _hints: &transcript::Hints<'_>,
        _main: &std::path::Path,
    ) -> Vec<PathBuf> {
        Vec::new()
    }
    /// What the transcript's complete lines `lines`, oldest first, say the agent is doing.
    fn read_transcript(&self, _lines: &[&[u8]], _activity: &mut transcript::Activity) {}
}

/// Every adapter, in the order the cross-harness tables list their phrases.
pub fn all() -> [&'static dyn HarnessAdapter; 4] {
    [
        &claude::ClaudeCode,
        &codex::Codex,
        &muse::Muse,
        &agentcloud::AgentCloud,
    ]
}

/// The adapter for a record's `harness` (Herdr's agent kind), when agentctl knows it.
#[must_use]
pub fn adapter(harness: &str) -> Option<&'static dyn HarnessAdapter> {
    all().into_iter().find(|adapter| adapter.name() == harness)
}

/// [`adapter`] for an optional kind, as Herdr reports it.
#[must_use]
pub fn adapter_for(harness: Option<&str>) -> Option<&'static dyn HarnessAdapter> {
    harness.and_then(adapter)
}

fn union_strs(
    cell: &'static OnceLock<Vec<&'static str>>,
    field: fn(&ScreenMarkers) -> &'static [&'static str],
) -> &'static [&'static str] {
    cell.get_or_init(|| {
        let mut out: Vec<&'static str> = Vec::new();
        for adapter in all() {
            for item in field(adapter.markers()) {
                if !out.contains(item) {
                    out.push(item);
                }
            }
        }
        out
    })
}

fn union_chars(
    cell: &'static OnceLock<Vec<char>>,
    field: fn(&ScreenMarkers) -> &'static [char],
) -> &'static [char] {
    cell.get_or_init(|| {
        let mut out: Vec<char> = Vec::new();
        for adapter in all() {
            for item in field(adapter.markers()) {
                if !out.contains(item) {
                    out.push(*item);
                }
            }
        }
        out
    })
}

/// Every harness's queued-input hints.
pub fn queue_hints() -> &'static [&'static str] {
    static CELL: OnceLock<Vec<&'static str>> = OnceLock::new();
    union_strs(&CELL, |markers| markers.queue_hints)
}

/// Every harness's approval, permission and trust phrases, lowercased.
pub fn approval_patterns() -> &'static [&'static str] {
    static CELL: OnceLock<Vec<&'static str>> = OnceLock::new();
    union_strs(&CELL, |markers| markers.approval)
}

/// Every harness's reply bullets.
pub fn reply_bullets() -> &'static [char] {
    static CELL: OnceLock<Vec<char>> = OnceLock::new();
    union_chars(&CELL, |markers| markers.reply_bullets)
}

/// Every harness's user-turn glyphs.
pub fn user_turn_glyphs() -> &'static [char] {
    static CELL: OnceLock<Vec<char>> = OnceLock::new();
    union_chars(&CELL, |markers| markers.user_turn_glyphs)
}

/// Every harness's tool-output glyphs.
pub fn tool_output_glyphs() -> &'static [char] {
    static CELL: OnceLock<Vec<char>> = OnceLock::new();
    union_chars(&CELL, |markers| markers.tool_output_glyphs)
}

/// Every harness's running-turn glyphs.
pub fn running_glyphs() -> &'static [char] {
    static CELL: OnceLock<Vec<char>> = OnceLock::new();
    union_chars(&CELL, |markers| markers.running_glyphs)
}

/// Every harness's composer placeholders.
pub fn composer_placeholders() -> &'static [&'static str] {
    static CELL: OnceLock<Vec<&'static str>> = OnceLock::new();
    union_strs(&CELL, |markers| markers.composer_placeholders)
}

/// The running-turn phrase every screen-driven harness draws while a turn runs: in Claude Code's
/// footer and spinner row, Codex's progress row, and Muse's running row.
pub const WORKING_MARKER: &str = "esc to interrupt";

/// Whether any harness's trust dialog shows on `screen`.
#[must_use]
pub fn any_trust_prompt(screen: &str) -> bool {
    all().iter().any(|adapter| adapter.trust_prompt(screen))
}

/// The user's home directory, for transcript locations: `$HOME`.
#[must_use]
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_adapter_is_found_by_its_name_and_unknown_kinds_are_not() {
        for harness in all() {
            assert_eq!(adapter(harness.name()).unwrap().name(), harness.name());
        }
        assert!(adapter("agy").is_none());
        assert!(adapter_for(None).is_none());
    }

    #[test]
    fn the_cross_harness_tables_keep_the_order_and_spellings_they_had() {
        // The lists the readers used before the adapters owned them (#574).
        assert_eq!(
            queue_hints(),
            [
                "Press up to edit queued messages",
                "edit last queued message",
                "Queued follow-up inputs",
            ]
        );
        let mut approval = approval_patterns().to_vec();
        approval.sort_unstable();
        let mut expected = vec![
            "would you like to run",
            "do you want to proceed",
            "approval required",
            "requires approval",
            "allow command?",
            "approve this",
            "yes, i trust this folder",
            "trust this workspace",
            "do you trust",
            "workspace trust",
        ];
        expected.sort_unstable();
        assert_eq!(approval, expected);
        let mut bullets = reply_bullets().to_vec();
        bullets.sort_unstable();
        let mut expected = vec!['●', '⏺', '◆', '•'];
        expected.sort_unstable();
        assert_eq!(bullets, expected);
        let mut glyphs = user_turn_glyphs().to_vec();
        glyphs.sort_unstable();
        let mut expected = vec!['❯', '›', '>'];
        expected.sort_unstable();
        assert_eq!(glyphs, expected);
        let mut glyphs = tool_output_glyphs().to_vec();
        glyphs.sort_unstable();
        let mut expected = vec!['⎿', '└'];
        expected.sort_unstable();
        assert_eq!(glyphs, expected);
        let mut glyphs = running_glyphs().to_vec();
        glyphs.sort_unstable();
        let mut expected = vec!['•', '◦', '◆', '◇', '◈', '✻', '✽', '✶', '✳', '✢', '*'];
        expected.sort_unstable();
        assert_eq!(glyphs, expected);
        let mut placeholders = composer_placeholders().to_vec();
        placeholders.sort_unstable();
        let mut expected = vec![
            "Ask Codex to do anything",
            "Explain this codebase",
            "Summarize recent commits",
            "Implement {feature}",
            "Find and fix a bug in @filename",
            "Write tests for @filename",
            "Improve documentation in @filename",
            "Run /review on my current changes",
            "Use /skills to list available skills",
            "Press up to edit queued messages",
            "Type your message",
        ];
        expected.sort_unstable();
        assert_eq!(placeholders, expected);
    }
}
