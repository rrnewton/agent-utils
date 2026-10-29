//! Muse's ruled editor during a running turn, for follow-up delivery only.
//!
//! Muse 1.4.14 keeps its editor drawn and typeable while a turn runs. Enter there steers the
//! running turn: the prompt is printed at once as a user row above the editor. Tab queues it for
//! the next turn instead, which waits for the whole turn to end; Tab does nothing on an idle
//! editor, and Esc interrupts the turn. agentctl therefore steers with Enter, which also
//! submits an ordinary prompt if the turn ended in between (#505 agentctl-muse-followup).
//!
//! Muse forms (approvals, panels, questions) are never answered here, so this adapter is offered
//! only by [`super::followup_tui`] and not by [`super::harness_tui`], which `agentctl prompts`
//! and `agentctl answer` use.
use super::{plain, AnswerPlan, HarnessTui, Question, State, View};
use crate::client::{
    muse_editor_rows_empty, muse_verified_process_editor_rows,
    muse_verified_process_turn_in_progress,
};

/// Rows, ending at the last row with text, searched for a Muse form or panel.
const FORM_ROWS: usize = 24;
/// Lowercase phrases of the Muse 1.4.14 forms and panels that take the editor's or the footer's
/// place: a command approval, its options, and the shortcuts panel `/help` opens.
const FORM_MARKERS: [&str; 4] = [
    "would you like to run the following command?",
    "allow this stage once",
    "abort the entire command",
    "esc to close",
];

pub struct MuseTui;

/// Lowercase phrases of Muse 1.4's command approval form, which takes the editor's place.
const APPROVAL_MARKERS: [&str; 2] = [
    "would you like to run the following command?",
    "allow this stage once",
];

impl MuseTui {
    /// Whether the editor is empty: the screen's editor rows hold only the prompt marker.
    pub(crate) fn editor_empty(screen: &str) -> bool {
        muse_verified_process_editor_rows(&plain(screen))
            .is_some_and(|rows| muse_editor_rows_empty(&rows))
    }

    /// Whether a command approval form stands where the editor would be, so the pane waits for
    /// a person: `agentctl status` reports it `blocked` rather than `working` (#678
    /// muse-bridge-latency).
    pub(crate) fn approval_form(screen: &str) -> bool {
        let clean = plain(screen);
        if muse_verified_process_editor_rows(&clean).is_some() {
            return false;
        }
        let rows: Vec<_> = clean.lines().collect();
        let Some(last) = rows.iter().rposition(|row| !row.trim().is_empty()) else {
            return false;
        };
        let tail = rows[(last + 1).saturating_sub(FORM_ROWS)..=last]
            .join("\n")
            .to_lowercase();
        APPROVAL_MARKERS.iter().any(|marker| tail.contains(marker))
    }
}

impl HarnessTui for MuseTui {
    fn working_followups(&self) -> bool {
        true
    }
    fn open_question_keys(&self) -> &'static [&'static str] {
        &[]
    }
    fn classify(&self, screen: &str) -> View {
        let clean = plain(screen);
        if muse_verified_process_editor_rows(&clean).is_some() {
            return View::state(if muse_verified_process_turn_in_progress(&clean) {
                State::Working
            } else {
                State::IdleComposer
            });
        }
        let rows: Vec<_> = clean.lines().collect();
        let last = rows.iter().rposition(|row| !row.trim().is_empty());
        let tail = last.map_or(String::new(), |last| {
            rows[(last + 1).saturating_sub(FORM_ROWS)..=last]
                .join("\n")
                .to_lowercase()
        });
        if FORM_MARKERS.iter().any(|marker| tail.contains(marker)) {
            return View::state(State::MenuOrPicker);
        }
        View::state(State::Unknown)
    }
    fn answer_plan(
        &self,
        _question: &Question,
        _option: Option<usize>,
        _text: Option<&str>,
    ) -> Result<AnswerPlan, String> {
        Err("Muse forms are not answered by agentctl; answer them in the pane".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{
        muse_verified_process_paste_in_composer, muse_verified_process_paste_is_exact_composer,
        muse_verified_process_prompt_transcript_count, muse_verified_process_turn_running,
    };

    // Screens captured from live Muse 1.4.14 sessions on 2026-10-25; see
    // testdata/muse-busy/README.md.
    const IDLE: &str = include_str!("../../testdata/muse-busy/muse-idle.txt");
    const THINKING: &str = include_str!("../../testdata/muse-busy/muse-working-thinking.txt");
    const COMMAND: &str = include_str!("../../testdata/muse-busy/muse-working-command.txt");
    const DRAFT: &str = include_str!("../../testdata/muse-busy/muse-working-draft.txt");
    const STEER_NOTICE: &str = include_str!("../../testdata/muse-busy/muse-steer-notice.txt");
    const STEERED: &str = include_str!("../../testdata/muse-busy/muse-steered.txt");
    const QUEUED_NOTICE: &str = include_str!("../../testdata/muse-busy/muse-queued-notice.txt");
    const QUEUED: &str = include_str!("../../testdata/muse-busy/muse-working-queued.txt");
    const MULTILINE: &str = include_str!("../../testdata/muse-busy/muse-steered-multiline.txt");
    const HELP: &str = include_str!("../../testdata/muse-busy/muse-help-panel.txt");
    const APPROVAL: &str = include_str!("../../testdata/muse-busy/muse-approval.txt");
    const DOUBLE_CHECKING: &str =
        include_str!("../../testdata/muse-busy/muse-double-checking-queued.txt");
    const INTERRUPTING: &str = include_str!("../../testdata/muse-busy/muse-interrupting.txt");
    const WRAPPED: &str = include_str!("../../testdata/muse-busy/muse-wrapped-draft.txt");
    const OVERLONG: &str = include_str!("../../testdata/muse-busy/muse-overlong-token-draft.txt");
    // Muse 1.4.15, 2026-10-29, inside a /goal turn (#678 muse-bridge-latency).
    const FOREGROUND_HINT: &str = include_str!("../../testdata/muse-busy/muse-foreground-hint.txt");
    const STEER_FAILED: &str = include_str!("../../testdata/muse-busy/muse-steer-failed.txt");
    const SUBAGENT: &str = include_str!("../../testdata/muse-busy/muse-subagent-panel.txt");

    const ALPHA: &str = "FOLLOWUP-ALPHA: when you reply, also include the word ALPHA.";
    const BETA: &str = "FOLLOWUP-BETA: also include the word BETA.";
    const GAMMA: &str =
        "FOLLOWUP-GAMMA line one\nline two of gamma\nline three: include GAMMA in reply";

    /// The same frame after Muse has scrolled its versioned header away.
    fn headerless(screen: &str) -> String {
        screen.replace("  Muse Code 1.4.14\n", "")
    }

    fn state(screen: &str) -> State {
        MuseTui.classify(screen).state
    }

    #[test]
    fn captured_frames_classify_as_idle_working_or_a_form() {
        for screen in [IDLE.to_owned(), headerless(IDLE)] {
            assert_eq!(state(&screen), State::IdleComposer);
            assert!(MuseTui::editor_empty(&screen));
        }
        for (name, screen) in [
            ("thinking", THINKING),
            ("command output below the progress row", COMMAND),
            ("steered", STEERED),
            ("queued input below the progress row", QUEUED),
            ("multi-line steer", MULTILINE),
            ("double checking with queued input", DOUBLE_CHECKING),
        ] {
            for screen in [screen.to_owned(), headerless(screen)] {
                assert_eq!(state(&screen), State::Working, "{name}");
                assert!(MuseTui::editor_empty(&screen), "{name}");
            }
        }
        // An operator draft during a turn is a working editor that is not empty.
        assert_eq!(state(DRAFT), State::Working);
        assert!(!MuseTui::editor_empty(DRAFT));
        // An approval form takes the editor's place, and the shortcuts panel the footer's.
        assert_eq!(state(APPROVAL), State::MenuOrPicker);
        assert!(!MuseTui::editor_empty(APPROVAL));
        assert_eq!(state(HELP), State::MenuOrPicker);
        assert!(!MuseTui::editor_empty(HELP));
        // For a few seconds after Enter, Tab or Esc Muse right-aligns a notice on the footer.
        // The footer is read by its structure, so the notice does not hide the editor: the turn
        // still reads as running, an empty editor as empty, and a draft as a draft.
        for screen in [STEER_NOTICE, QUEUED_NOTICE] {
            assert_eq!(state(screen), State::Working);
            assert!(MuseTui::editor_empty(screen));
        }
        assert_eq!(state(INTERRUPTING), State::Working);
        assert!(!MuseTui::editor_empty(INTERRUPTING));
        assert!(MuseTui
            .answer_plan(
                &Question {
                    text: String::new(),
                    options: vec![],
                    selected: None,
                    total: 1,
                    collapsed: false,
                    notes: false,
                    draft: String::new(),
                },
                Some(1),
                None
            )
            .is_err());
    }

    #[test]
    fn a_paste_muse_wraps_inside_a_word_or_a_path_is_still_the_exact_editor() {
        // Muse 1.4.14 broke `odd/places` after the slash and a 210-character token mid-word, so
        // a proof that joined rows only with a space never saw these pastes staged; the drain
        // then left them in the editor, unsubmitted, as possibly submitted.
        let long = "wrap-probe: please keep working on the current task, and afterwards \
                    summarise in one paragraph what you did, including the exact command lines \
                    you ran, their exit statuses, whether anything printed to stderr, and \
                    supercalifragilisticexpialidocious-hyphenated-compound-words-that/might/\
                    break/at/odd/places plus a URL https://example.com/a/very/long/path/that/\
                    keeps/going/and/going/without/any/spaces/at/all/whatsoever/to/force/a/hard/\
                    character/wrap END";
        assert!(WRAPPED.contains("at/odd/\n  places"));
        assert!(muse_verified_process_paste_is_exact_composer(WRAPPED, long));
        // The screen cannot say whether a space sat at the break, but the words must match.
        assert!(!muse_verified_process_paste_is_exact_composer(
            WRAPPED,
            &long.replace("odd/places", "odd/placez")
        ));
        let token = format!("token-{}_x.y,z;w:q END2", "abcdefghij".repeat(20));
        assert!(OVERLONG.contains("abcde\n  fghij"));
        assert!(muse_verified_process_paste_is_exact_composer(
            OVERLONG, &token
        ));
        assert_eq!(state(WRAPPED), State::Working);
        assert!(!MuseTui::editor_empty(WRAPPED));
    }

    #[test]
    fn the_running_turn_row_is_found_below_output_and_queued_input() {
        // `agentctl status` and the follow-up reader share one rule, which finds the row when
        // command output or a queued-input block follows it, and in its `◆` phase (#678
        // muse-bridge-latency).
        for screen in [THINKING, COMMAND, QUEUED, DOUBLE_CHECKING] {
            let screen = headerless(screen);
            assert!(muse_verified_process_turn_running(&screen));
            assert!(muse_verified_process_turn_in_progress(&screen));
        }
        assert!(!muse_verified_process_turn_running(&headerless(IDLE)));
        assert!(!muse_verified_process_turn_in_progress(&headerless(IDLE)));
    }

    #[test]
    fn a_steered_prompt_is_proved_by_a_new_user_turn_and_a_queued_one_is_not() {
        let staged = headerless(DRAFT);
        assert!(muse_verified_process_paste_is_exact_composer(
            &staged, ALPHA
        ));
        assert_eq!(
            muse_verified_process_prompt_transcript_count(&staged, ALPHA),
            0
        );
        let steered = headerless(STEERED);
        assert_eq!(
            muse_verified_process_prompt_transcript_count(&steered, ALPHA),
            1
        );
        assert!(!muse_verified_process_paste_in_composer(&steered, ALPHA));
        let multiline = headerless(MULTILINE);
        assert_eq!(
            muse_verified_process_prompt_transcript_count(&multiline, GAMMA),
            1
        );
        assert_eq!(
            muse_verified_process_prompt_transcript_count(&multiline, ALPHA),
            1
        );
        // A prompt Muse queued for the next turn is drawn in its `◆ Queued input` block, not as
        // a user turn, so it never passes for a steered one.
        assert!(QUEUED.contains(&format!("└ {BETA}")));
        assert_eq!(
            muse_verified_process_prompt_transcript_count(&headerless(QUEUED), BETA),
            0
        );
    }

    #[test]
    fn a_running_turn_reads_working_whatever_herdr_reports_and_whatever_its_glyph() {
        use crate::client::muse_verified_pane_status;
        // Herdr reports a Muse pane done once the turn's answer is drawn, while Muse is still in
        // its `◆ Double checking` phase, and its progress glyph cycles through `◆ ◇ ◈`.
        for (name, screen) in [
            ("◆ thinking", THINKING),
            ("◆ double checking", DOUBLE_CHECKING),
            ("foreground command hint", FOREGROUND_HINT),
            ("steer refused mid command", STEER_FAILED),
        ] {
            for screen in [screen.to_owned(), headerless(screen)] {
                for reported in ["idle", "done", "working"] {
                    assert_eq!(
                        muse_verified_pane_status(reported, &screen),
                        "working",
                        "{name} reported {reported}"
                    );
                }
            }
            for glyph in ["◆", "◇", "◈"] {
                let screen = screen
                    .replace("◆ Thinking (", &format!("{glyph} Thinking ("))
                    .replace("◇ Running command", &format!("{glyph} Running command"))
                    .replace("◆ Double checking", &format!("{glyph} Double checking"));
                assert_eq!(
                    muse_verified_pane_status("done", &screen),
                    "working",
                    "{name} {glyph}"
                );
            }
        }
        assert_eq!(muse_verified_pane_status("done", &headerless(IDLE)), "idle");
    }

    #[test]
    fn a_refused_steer_is_the_whole_editor_again() {
        // Muse puts a steer it refuses back in its editor as the paste's placeholder, so the
        // steer path sees the prompt still staged, presses Enter again, and at its deadline takes
        // it back out instead of leaving a draft that refuses every later prompt.
        let prompt = "x".repeat(1635);
        assert!(STEER_FAILED.contains("steer failed"));
        assert!(muse_verified_process_paste_is_exact_composer(
            STEER_FAILED,
            &prompt
        ));
        assert!(!MuseTui::editor_empty(STEER_FAILED));
        assert_eq!(state(STEER_FAILED), State::Working);
    }

    #[test]
    fn an_approval_form_blocks_and_a_panel_or_an_editor_does_not() {
        assert!(MuseTui::approval_form(APPROVAL));
        assert!(!MuseTui::approval_form(HELP));
        for screen in [IDLE, THINKING, COMMAND, STEER_FAILED] {
            assert!(!MuseTui::approval_form(screen));
        }
    }

    #[test]
    fn a_quoted_or_indented_progress_row_is_not_a_running_turn() {
        use crate::client::muse_verified_pane_status;
        // A capture the agent printed or was sent, tool output under `└`, or an answer that
        // mentions the phrase sits above an idle editor without making it a running turn.
        for quoted in [
            "└ ◇ Calling tools (30s · esc to interrupt)\n",
            "  ◇ Calling tools (30s · esc to interrupt)\n",
            "◆ The row reads `◇ Thinking (3s · esc to interrupt)` while it works.\n",
        ] {
            let screen = IDLE.replacen("────", &format!("{quoted}────"), 1);
            assert_ne!(screen, IDLE);
            for screen in [screen.clone(), headerless(&screen)] {
                assert!(!muse_verified_process_turn_running(&screen), "{quoted}");
                assert_eq!(
                    muse_verified_pane_status("done", &screen),
                    "idle",
                    "{quoted}"
                );
            }
        }
    }

    #[test]
    fn a_running_sub_agent_in_the_terminals_panel_does_not_hide_the_editor() {
        use crate::client::muse_verified_pane_status;
        // The panel's tree row for a sub-agent has `·` fields, like a footer; read as one, it
        // made two footers, no editor, and a pane that took no message while the sub-agent ran.
        assert!(SUBAGENT.contains("└ ◇ counter ·3  shared · Run the shell command"));
        assert_eq!(state(SUBAGENT), State::Working);
        assert!(MuseTui::editor_empty(SUBAGENT));
        assert_eq!(muse_verified_pane_status("done", SUBAGENT), "working");
        let finished = SUBAGENT.replace("◈ Thinking (3s · esc to interrupt)", "◆ Worked for 40s");
        assert_eq!(muse_verified_pane_status("done", &finished), "idle");
    }

    #[test]
    fn a_prompt_in_the_input_queue_is_counted_by_its_text_or_its_placeholder() {
        use crate::client::muse_verified_process_prompt_queued_count;
        let queued = headerless(QUEUED);
        assert_eq!(muse_verified_process_prompt_queued_count(&queued, BETA), 1);
        assert_eq!(muse_verified_process_prompt_queued_count(&queued, ALPHA), 0);
        let long = "y".repeat(1637);
        let placeholder = queued.replace(&format!("└ {BETA}"), "└ [Pasted Content 1637 chars]");
        assert_eq!(
            muse_verified_process_prompt_queued_count(&placeholder, &long),
            1
        );
        assert_eq!(
            muse_verified_process_prompt_queued_count(&placeholder, &"y".repeat(1636)),
            0
        );
        // No queue block, no count; a user turn is not a queued item.
        assert_eq!(
            muse_verified_process_prompt_queued_count(&headerless(STEERED), ALPHA),
            0
        );
    }
}
