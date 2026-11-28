//! The same pane captures as Python, plus guarded, at-most-once answer effects.
use std::cell::{Cell, RefCell};
use std::time::Duration;

use agentctl::agent::AgentRuntime;
use agentctl::harness_tui::{harness_tui, State};
use agentctl::submission::GuardedInput;

const OPTIONS: &str = include_str!("fixtures/tui/codex-options.txt");
const FREEFORM: &str = include_str!("fixtures/tui/codex-freeform.txt");
const ASYNC_OPTIONS: &str = include_str!("fixtures/tui/codex-async-options.txt");
const ASYNC_FREEFORM: &str = include_str!("fixtures/tui/codex-async-freeform.txt");
const RATE_LIMIT: &str = include_str!("fixtures/tui/codex-rate-limit.txt");
const WORKING: &str = "• Working (2s • esc to interrupt)\n»\n  GPT default · /work/project\n";

#[test]
fn rate_limit_reminder_requires_complete_unchanged_visible_menu() {
    let adapter = harness_tui(Some("codex")).unwrap();
    let q = adapter.classify(RATE_LIMIT).question.unwrap();
    assert_eq!(q.selected, Some(1));
    assert_eq!(q.options[1].text, "Keep current model");
    assert_eq!(
        adapter.answer_plan(&q, Some(2), None).unwrap().keys,
        ["Down"]
    );
    for broken in [
        RATE_LIMIT.replace("  2. Keep current model\n", ""),
        RATE_LIMIT.replace("Keep current model\n", "Change current model\n"),
        RATE_LIMIT.replace("  2.", "› 2."),
        RATE_LIMIT.replace("gpt-6-luna for lower", "a different model for lower"),
        RATE_LIMIT.replace("enter select · esc back", "enter confirm"),
        format!("{RATE_LIMIT}\n»\n  GPT default · /work/project\n"),
    ] {
        assert_ne!(adapter.classify(&broken).state, State::BlockingQuestion);
    }
}

#[test]
fn rate_limit_guarded_answer_selects_current_model_once() {
    for drop_submit in [false, true] {
        let mut terminal = Terminal::new(RATE_LIMIT);
        terminal.drop_submit = drop_submit;
        let result = agentctl::tui_prompts::answer(
            &terminal,
            "pane",
            harness_tui(Some("codex")).unwrap(),
            (Some(2), None),
            false,
            &|_| Ok(()),
            &Runtime::default(),
        );
        if drop_submit {
            assert_eq!(result.unwrap_err().exit_code(), 76);
        } else {
            assert_eq!(result.unwrap()["answered"], true);
        }
        assert_eq!(*terminal.keys.borrow(), ["Down", "Enter"]);
        assert!(terminal.text.borrow().is_empty());
    }
}

#[test]
fn shared_state_corpus() {
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/tui/states.json")).unwrap();
    for row in corpus.as_array().unwrap() {
        let adapter = harness_tui(row["harness"].as_str()).unwrap();
        assert_eq!(
            serde_json::to_value(adapter.classify(row["screen"].as_str().unwrap()).state).unwrap(),
            row["state"]
        );
    }
    assert!(harness_tui(Some("unknown")).is_none());
}

#[test]
fn claude_permission_proceed_is_an_answerable_question() {
    // #2011: a captured Claude Bash permission prompt (`Do you want to
    // proceed? ❯ 1. Yes  2. No`) classified as a menu, so `answer`
    // refused it while `prompts` showed it. The dangerous-operation
    // variant of the same form answers too; a hook confirmation stays
    // owner-only.
    let adapter = harness_tui(Some("claude")).unwrap();
    let screen = include_str!("fixtures/tui/claude-permission-proceed.txt");
    let view = adapter.classify(screen);
    assert_eq!(view.state, State::BlockingQuestion);
    let q = view.question.unwrap();
    assert_eq!(q.selected, Some(1));
    assert_eq!(q.options.len(), 2);
    assert!(adapter.answer_plan(&q, Some(2), None).is_ok());
    assert!(adapter.answer_plan(&q, Some(1), None).is_ok());
    let dangerous = screen.replace(
        "Permission rule Bash requires confirmation for this command.",
        "Dangerous rm operation: this command may delete files",
    );
    let view = adapter.classify(&dangerous);
    assert_eq!(view.state, State::BlockingQuestion);
    assert!(view
        .question
        .unwrap()
        .text
        .contains("Dangerous rm operation"));
    let hook = "──────\nHook PreToolUse:Bash requires confirmation\nRunning this command may delete files: rm -rf build\nDo you want to proceed?\n❯ 1. Yes\n  2. No\nEsc to cancel · Tab to amend\n";
    let view = adapter.classify(hook);
    assert_eq!(view.state, State::BlockingQuestion);
    assert!(adapter
        .answer_plan(&view.question.unwrap(), Some(1), None)
        .is_err());
}

#[test]
fn dangerous_permission_answer_requires_confirm_dangerous() {
    // #2046: without --confirm-dangerous the answer refuses, names the
    // warning line, and types nothing; with it, the answer goes through
    // and its report names the same line. A hook confirmation refuses
    // even with the flag.
    let screen = include_str!("fixtures/tui/claude-permission-proceed.txt").replace(
        "Permission rule Bash requires confirmation for this command.",
        "Dangerous rm operation: this command may delete files",
    );
    let mut terminal = Terminal::new(&screen);
    let error = agentctl::tui_prompts::answer(
        &terminal,
        "pane",
        harness_tui(Some("claude")).unwrap(),
        (Some(1), None),
        false,
        &|_| Ok(()),
        &Runtime::default(),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Dangerous rm operation: this command may delete files"),
        "{error}"
    );
    assert!(error.to_string().contains("--confirm-dangerous"), "{error}");
    assert!(terminal.keys.borrow().is_empty(), "no key may be sent");
    let result = agentctl::tui_prompts::answer(
        &terminal,
        "pane",
        harness_tui(Some("claude")).unwrap(),
        (Some(1), None),
        true,
        &|_| Ok(()),
        &Runtime::default(),
    )
    .unwrap();
    assert_eq!(result["answered"], true);
    assert!(
        result["question"]
            .as_str()
            .unwrap()
            .contains("Dangerous rm operation"),
        "{result}"
    );
    assert_eq!(*terminal.keys.borrow(), ["Enter"]);
    terminal = Terminal::new(
        "──────\nHook PreToolUse:Bash requires confirmation\nRunning this command may delete files: rm -rf build\nDo you want to proceed?\n❯ 1. Yes\n  2. No\nEsc to cancel · Tab to amend\n",
    );
    let error = agentctl::tui_prompts::answer(
        &terminal,
        "pane",
        harness_tui(Some("claude")).unwrap(),
        (Some(1), None),
        true,
        &|_| Ok(()),
        &Runtime::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("owner-only"), "{error}");
    assert!(terminal.keys.borrow().is_empty());
}

#[test]
fn upstream_question_snapshots_and_unsupported_layouts() {
    let adapter = harness_tui(Some("codex")).unwrap();
    let q = adapter.classify(OPTIONS).question.unwrap();
    assert_eq!(q.selected, Some(1));
    assert_eq!(q.options.len(), 4);
    assert_eq!(
        adapter.answer_plan(&q, Some(2), None).unwrap().keys,
        ["Down"]
    );
    assert_eq!(
        adapter
            .answer_plan(&q, None, Some("Alternative"))
            .unwrap()
            .keys,
        ["Down", "Down", "Down", "Tab"]
    );
    for screen in [
        include_str!("fixtures/tui/codex-multi_question_first.txt"),
        include_str!("fixtures/tui/codex-hidden_options_footer.txt"),
        include_str!("fixtures/tui/codex-options_notes_visible.txt"),
    ] {
        let view = adapter.classify(screen);
        assert_eq!(view.state, State::BlockingQuestion);
        assert!(adapter
            .answer_plan(&view.question.unwrap(), Some(2), None)
            .is_err());
    }
    assert!(adapter.answer_plan(&q, Some(99), None).is_err());
    assert!(adapter.answer_plan(&q, Some(4), None).is_err());
    assert!(adapter.answer_plan(&q, None, Some("\u{1b}bad")).is_err());
    let stale = format!("{OPTIONS}\n»\n  GPT default · /work/project\n");
    assert_ne!(adapter.classify(&stale).state, State::BlockingQuestion);
}

struct Terminal {
    screen: RefCell<String>,
    keys: RefCell<Vec<String>>,
    text: RefCell<Vec<String>>,
    collapsed: Cell<bool>,
    drop_submit: bool,
}
impl Terminal {
    fn new(screen: &str) -> Self {
        Self {
            screen: RefCell::new(screen.to_owned()),
            keys: RefCell::new(vec![]),
            text: RefCell::new(vec![]),
            collapsed: Cell::new(false),
            drop_submit: false,
        }
    }
}
impl GuardedInput for Terminal {
    fn read_screen(&self, _pane: &str) -> agentctl::error::Result<String> {
        Ok(if self.collapsed.get() {
            "• Queued follow-up inputs\n  ? 1 question\n  shift+← to answer".to_owned()
        } else {
            self.screen.borrow().clone()
        })
    }
    fn send_keys(&self, _pane: &str, key: &str) -> agentctl::error::Result<()> {
        self.keys.borrow_mut().push(key.to_owned());
        if key == "shift+left" {
            self.collapsed.set(false);
        } else if key == "Down" || key == "Up" {
            let adapter = harness_tui(Some("codex")).unwrap();
            let selected = adapter
                .classify(&self.screen.borrow())
                .question
                .unwrap()
                .selected
                .unwrap();
            let next = if key == "Down" {
                selected + 1
            } else {
                selected - 1
            };
            let screen = self
                .screen
                .borrow()
                .replace("› ", "  ")
                .replace(&format!("  {next}. "), &format!("› {next}. "));
            *self.screen.borrow_mut() = screen;
        } else if key == "Tab" {
            let screen = self
                .screen
                .borrow()
                .replace("tab to add notes", "tab or esc to clear notes")
                .replace("  tab or esc", "  › Add notes\n\n  tab or esc");
            *self.screen.borrow_mut() = screen;
        } else if key == "Enter" && !self.drop_submit {
            if self.screen.borrow().contains("Do you want to proceed?") {
                // A Claude permission prompt dismisses to its idle composer.
                *self.screen.borrow_mut() =
                    "────────────────────────────────────────\n❯\n────────────────────────────────────────\n"
                        .to_owned();
            } else {
                *self.screen.borrow_mut() = WORKING.to_owned();
            }
        }
        Ok(())
    }
    fn send_text(&self, _pane: &str, text: &str) -> agentctl::error::Result<()> {
        self.text.borrow_mut().push(text.to_owned());
        let value = text
            .strip_prefix("\u{1b}[200~")
            .unwrap()
            .strip_suffix("\u{1b}[201~")
            .unwrap();
        let screen = self
            .screen
            .borrow()
            .replace("› Type your answer (optional)", &format!("› {value}"))
            .replace("› Add notes", &format!("› {value}"))
            .replace("  Type your answer\n", &format!("  {value}\n"));
        *self.screen.borrow_mut() = screen;
        Ok(())
    }
    fn native_prompt(&self, _pane: &str, _text: &str) -> agentctl::error::Result<()> {
        panic!("answer must not use ordinary prompt injection")
    }
}
#[derive(Default)]
struct Runtime(Cell<Duration>);
impl AgentRuntime for Runtime {
    fn monotonic(&self) -> Duration {
        self.0.get()
    }
    fn sleep(&self, delta: Duration) {
        self.0.set(self.0.get() + delta);
    }
}

#[test]
fn option_answer_expands_selects_and_confirms() {
    let terminal = Terminal::new(OPTIONS);
    terminal.collapsed.set(true);
    let result = agentctl::tui_prompts::answer(
        &terminal,
        "pane",
        harness_tui(Some("codex")).unwrap(),
        (Some(2), None),
        false,
        &|_| Ok(()),
        &Runtime::default(),
    )
    .unwrap();
    assert_eq!(result["answered"], true);
    assert_eq!(*terminal.keys.borrow(), ["shift+left", "Down", "Enter"]);
}

#[test]
fn free_text_and_other_option_use_literal_paste() {
    for screen in [FREEFORM, ASYNC_FREEFORM, OPTIONS] {
        let terminal = Terminal::new(screen);
        let result = agentctl::tui_prompts::answer(
            &terminal,
            "pane",
            harness_tui(Some("codex")).unwrap(),
            (None, Some("A literal answer")),
            false,
            &|_| Ok(()),
            &Runtime::default(),
        )
        .unwrap();
        assert_eq!(result["answered"], true);
        assert_eq!(
            terminal
                .keys
                .borrow()
                .iter()
                .filter(|k| k.as_str() == "Enter")
                .count(),
            1
        );
        assert_eq!(
            *terminal.text.borrow(),
            ["\u{1b}[200~A literal answer\u{1b}[201~"]
        );
    }
}

#[test]
fn async_question_refuses_stale_capture_and_existing_draft() {
    let adapter = harness_tui(Some("codex")).unwrap();
    assert_ne!(
        adapter
            .classify(&format!("{ASYNC_FREEFORM}\nshell$\n"))
            .state,
        State::BlockingQuestion
    );
    let question = adapter
        .classify(&ASYNC_FREEFORM.replace("Type your answer", "Existing draft"))
        .question
        .unwrap();
    assert!(adapter
        .answer_plan(&question, None, Some("Replacement"))
        .is_err());
    assert_eq!(question.draft, "Existing draft");
}

#[test]
fn dropped_submission_is_uncertain_and_not_repeated() {
    let mut terminal = Terminal::new(OPTIONS);
    terminal.drop_submit = true;
    let error = agentctl::tui_prompts::answer(
        &terminal,
        "pane",
        harness_tui(Some("codex")).unwrap(),
        (Some(1), None),
        false,
        &|_| Ok(()),
        &Runtime::default(),
    )
    .unwrap_err();
    assert_eq!(error.exit_code(), 76);
    assert_eq!(*terminal.keys.borrow(), ["Enter"]);
}

#[test]
fn bad_selection_and_recipient_refusal_send_nothing() {
    let terminal = Terminal::new(OPTIONS);
    let error = agentctl::tui_prompts::answer(
        &terminal,
        "pane",
        harness_tui(Some("codex")).unwrap(),
        (Some(99), None),
        false,
        &|_| Ok(()),
        &Runtime::default(),
    )
    .unwrap_err();
    assert_eq!(error.exit_code(), 75);
    assert!(terminal.keys.borrow().is_empty());
    let error = agentctl::tui_prompts::answer(
        &terminal,
        "pane",
        harness_tui(Some("codex")).unwrap(),
        (Some(1), None),
        false,
        &|_| Err(agentctl::error::AdapterError::recipient_changed("replaced")),
        &Runtime::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("replaced"));
    assert!(terminal.keys.borrow().is_empty());
}

#[test]
fn cli_question_commands_are_discoverable_and_selection_is_exclusive() {
    for command in ["prompts", "answer"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_agentctl"))
            .args([command, "--help"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8(output.stdout).unwrap().contains("NAME"));
    }
    for args in [
        vec!["answer", "worker"],
        vec!["answer", "worker", "--option", "1", "--text", "x"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_agentctl"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
}

#[test]
fn inspection_reveals_only_and_never_selects_or_submits() {
    let terminal = Terminal::new(OPTIONS);
    terminal.collapsed.set(true);
    let adapter = harness_tui(Some("codex")).unwrap();
    let view = agentctl::tui_prompts::inspect_question(
        &terminal,
        "pane",
        adapter,
        &|_| Ok(()),
        &Runtime::default(),
    )
    .unwrap();
    assert_eq!(view.question.unwrap().selected, Some(1));
    assert_eq!(*terminal.keys.borrow(), ["shift+left"]);
    assert!(terminal.text.borrow().is_empty());
    let _ = agentctl::tui_prompts::inspect_question(
        &terminal,
        "pane",
        adapter,
        &|_| Ok(()),
        &Runtime::default(),
    )
    .unwrap();
    assert_eq!(*terminal.keys.borrow(), ["shift+left"]);
}

#[test]
fn inspection_stops_after_reveal_on_an_unknown_form() {
    let terminal = Terminal::new("Choose model?\n› 1. Fast\nenter to select | esc to cancel");
    terminal.collapsed.set(true);
    let error = agentctl::tui_prompts::inspect_question(
        &terminal,
        "pane",
        harness_tui(Some("codex")).unwrap(),
        &|_| Ok(()),
        &Runtime::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        agentctl::agent::AgentError::PossiblySubmitted(_)
    ));
    assert_eq!(*terminal.keys.borrow(), ["shift+left"]);
    assert!(terminal.text.borrow().is_empty());
}

#[test]
fn async_options_navigate_and_submit_at_most_once() {
    for drop_submit in [false, true] {
        let mut terminal = Terminal::new(ASYNC_OPTIONS);
        terminal.drop_submit = drop_submit;
        let result = agentctl::tui_prompts::answer(
            &terminal,
            "pane",
            harness_tui(Some("codex")).unwrap(),
            (Some(2), None),
            false,
            &|_| Ok(()),
            &Runtime::default(),
        );
        if drop_submit {
            assert_eq!(result.unwrap_err().exit_code(), 76);
        } else {
            assert_eq!(result.unwrap()["answered"], true);
        }
        assert_eq!(*terminal.keys.borrow(), ["Down", "Enter"]);
        assert!(terminal.text.borrow().is_empty());
    }
}

#[test]
fn async_options_refuse_incomplete_or_ambiguous_forms() {
    for (old, new) in [
        ("› 1.", "  1."),
        ("    2.", "  › 2."),
        ("    2.", "    4."),
        ("    3. Other", "    … more options"),
        ("    3. Other", "    3. Other\n    › Existing draft"),
        ("shift+→ main prompt", "shift+→ main prompt\nshell$"),
    ] {
        let terminal = Terminal::new(&ASYNC_OPTIONS.replace(old, new));
        assert!(agentctl::tui_prompts::answer(
            &terminal,
            "pane",
            harness_tui(Some("codex")).unwrap(),
            (Some(2), None),
            false,
            &|_| Ok(()),
            &Runtime::default(),
        )
        .is_err());
        assert!(terminal.keys.borrow().is_empty());
        assert!(terminal.text.borrow().is_empty());
    }
}

#[test]
fn matching_wrapped_draft_is_confirmed_without_retyping() {
    for drop_submit in [false, true] {
        let mut terminal = Terminal::new(&ASYNC_FREEFORM.replace(
            "Type your answer",
            "Keep the paid lifecycle and\n  prepare separate boundary companions.",
        ));
        terminal.drop_submit = drop_submit;
        let result = agentctl::tui_prompts::answer(
            &terminal,
            "pane",
            harness_tui(Some("codex")).unwrap(),
            (
                None,
                Some("Keep the paid lifecycle and prepare separate boundary companions."),
            ),
            false,
            &|_| Ok(()),
            &Runtime::default(),
        );
        if drop_submit {
            assert_eq!(result.unwrap_err().exit_code(), 76);
        } else {
            assert_eq!(result.unwrap()["answered"], true);
        }
        assert_eq!(*terminal.keys.borrow(), ["Enter"]);
        assert!(terminal.text.borrow().is_empty());
    }
    let terminal = Terminal::new(
        &ASYNC_FREEFORM.replace("Type your answer", "Someone else's\n  existing draft"),
    );
    assert!(agentctl::tui_prompts::answer(
        &terminal,
        "pane",
        harness_tui(Some("codex")).unwrap(),
        (None, Some("Replacement")),
        false,
        &|_| Ok(()),
        &Runtime::default(),
    )
    .is_err());
    assert!(terminal.keys.borrow().is_empty());
    assert!(terminal.text.borrow().is_empty());
}

// #2421 agentctl-asks-never-block: refusing permission prompts. A menu
// fake moves its cursor between drawn option rows like the harnesses do
// and records which option the submit key chose, so every test can check
// that nothing but a refusal is ever submitted.

const CLAUDE_PERMISSION: &str = include_str!("fixtures/tui/claude-permission-proceed.txt");
const CLAUDE_HOOK: &str = "──────\nHook PreToolUse:Bash requires confirmation\nRunning this command may delete files: rm -rf build\nDo you want to proceed?\n❯ 1. Yes\n  2. No\nEsc to cancel · Tab to amend\n";
const CLAUDE_DANGEROUS: &str = "────────────────────────────────────────\n Bash command\n rm -rf \"$OUT/\"*\n Dangerous rm operation on possibly-empty variable path\n Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don't ask again for rm commands in /work/project\n   3. No, and tell Claude what to do differently (esc)\n";
const CLAUDE_TRUST: &str = "────────────────────────────────────────\n Do you trust the files in this folder?\n /work/project\n ❯ 1. Yes, proceed\n   2. No, exit\n Enter to confirm · Esc to exit\n";
const CLAUDE_QUESTION: &str = "────────────────────────────────────────\n Which branch should I use?\n ❯ 1. main\n   2. No preference\n Enter to select · Tab/Arrow keys to navigate · Esc to cancel\n";
const MUSE_APPROVAL: &str = include_str!("../testdata/muse-busy/muse-approval.txt");
const MUSE_PEER: &str = include_str!("../testdata/muse-busy/muse-peer-approval.txt");
const MUSE_PEER_FOCUSED: &str =
    include_str!("../testdata/muse-busy/muse-peer-approval-focused.txt");
const MUSE_IDLE: &str = include_str!("../testdata/muse-busy/muse-idle.txt");
const CLAUDE_IDLE: &str =
    "────────────────────────────────────────\n❯\n────────────────────────────────────────\n";

/// How the menu fake misbehaves.
#[derive(Clone, Copy, PartialEq)]
enum Fault {
    None,
    /// Arrow keys do nothing.
    StuckCursor,
    /// The first arrow key replaces the prompt with a different one.
    PromptChanges,
    /// The submit key is swallowed: the prompt stays up.
    SubmitIgnored,
    /// An arrow key moves the cursor two options, not one.
    CursorSkips,
}

struct Menu {
    screen: RefCell<String>,
    marker: char,
    idle: String,
    fault: Fault,
    keys: RefCell<Vec<String>>,
    submitted: RefCell<Vec<String>>,
}

impl Menu {
    fn new(screen: &str, harness: &str, fault: Fault) -> Self {
        let (marker, idle) = if harness == "muse" {
            ('⟩', MUSE_IDLE.to_owned())
        } else {
            ('❯', CLAUDE_IDLE.to_owned())
        };
        Self {
            screen: RefCell::new(screen.to_owned()),
            marker,
            idle,
            fault,
            keys: RefCell::new(vec![]),
            submitted: RefCell::new(vec![]),
        }
    }

    /// Indices of the trailing run of option rows (`N. label`, cursor or
    /// not): the dialog's menu, not a numbered list above it.
    fn option_rows(&self, rows: &[String]) -> Vec<usize> {
        let found: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                let body = row.trim().trim_start_matches(self.marker).trim_start();
                body.split_once(". ")
                    .is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            })
            .map(|(index, _)| index)
            .collect();
        let Some(mut start) = found.len().checked_sub(1) else {
            return found;
        };
        while start > 0 && found[start - 1] + 1 == found[start] {
            start -= 1;
        }
        found[start..].to_vec()
    }

    fn selected(&self, rows: &[String], options: &[usize]) -> Option<usize> {
        options
            .iter()
            .position(|&index| rows[index].trim_start().starts_with(self.marker))
    }

    fn draw(&self, rows: &mut [String], options: &[usize], cursor: usize) {
        for (position, &index) in options.iter().enumerate() {
            let body = rows[index]
                .trim()
                .trim_start_matches(self.marker)
                .trim_start()
                .to_owned();
            rows[index] = if position == cursor {
                format!(" {} {body}", self.marker)
            } else {
                format!("   {body}")
            };
        }
    }

    fn start_at(&self, cursor: usize) {
        let mut rows: Vec<String> = self.screen.borrow().lines().map(str::to_owned).collect();
        let options = self.option_rows(&rows);
        self.draw(&mut rows, &options, cursor);
        *self.screen.borrow_mut() = rows.join("\n") + "\n";
    }
}

impl GuardedInput for Menu {
    fn read_screen(&self, _pane: &str) -> agentctl::error::Result<String> {
        Ok(self.screen.borrow().clone())
    }
    fn send_keys(&self, _pane: &str, key: &str) -> agentctl::error::Result<()> {
        self.keys.borrow_mut().push(key.to_owned());
        let mut rows: Vec<String> = self.screen.borrow().lines().map(str::to_owned).collect();
        let options = self.option_rows(&rows);
        match key {
            "Down" | "Up" => {
                if self.fault == Fault::PromptChanges {
                    *self.screen.borrow_mut() =
                        CLAUDE_HOOK.replace("rm -rf build", "git push --force");
                    return Ok(());
                }
                if self.fault == Fault::StuckCursor {
                    return Ok(());
                }
                let step = if self.fault == Fault::CursorSkips {
                    2
                } else {
                    1
                };
                let cursor = match self.selected(&rows, &options) {
                    // An unfocused menu takes focus on its first option.
                    None => 0,
                    Some(at) if key == "Down" => (at + step).min(options.len() - 1),
                    Some(at) => at.saturating_sub(step),
                };
                self.draw(&mut rows, &options, cursor);
                *self.screen.borrow_mut() = rows.join("\n") + "\n";
            }
            "Enter" => {
                let chosen = self.selected(&rows, &options).map(|at| {
                    let body = rows[options[at]]
                        .trim()
                        .trim_start_matches(self.marker)
                        .trim_start()
                        .to_owned();
                    body.split_once(". ")
                        .map_or(body.clone(), |(_, label)| label.to_owned())
                });
                self.submitted
                    .borrow_mut()
                    .push(chosen.unwrap_or_else(|| "<no cursor>".to_owned()));
                if self.fault != Fault::SubmitIgnored {
                    *self.screen.borrow_mut() = self.idle.clone();
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn send_text(&self, _pane: &str, text: &str) -> agentctl::error::Result<()> {
        panic!("a refusal never types text, got {text:?}");
    }
    fn native_prompt(&self, _pane: &str, text: &str) -> agentctl::error::Result<()> {
        panic!("a refusal never submits a prompt, got {text:?}");
    }
}

fn deny_on(menu: &Menu, harness: &str) -> agentctl::agent::AgentResult<Option<serde_json::Value>> {
    agentctl::tui_prompts::deny(
        menu,
        "pane",
        harness,
        None,
        &|_| Ok(()),
        &Runtime::default(),
    )
}

/// Every recognised prompt: harness, screen, kind and the refusing option's label.
fn recognised() -> Vec<(&'static str, String, &'static str, &'static str)> {
    vec![
        ("claude", CLAUDE_PERMISSION.to_owned(), "permission", "No"),
        ("claude", CLAUDE_HOOK.to_owned(), "hook-confirmation", "No"),
        (
            "claude",
            CLAUDE_DANGEROUS.to_owned(),
            "permission",
            "No, and tell Claude what to do differently (esc)",
        ),
        (
            "muse",
            MUSE_APPROVAL.to_owned(),
            "command-approval",
            "Abort the entire command (esc)",
        ),
        ("muse", MUSE_PEER.to_owned(), "peer-approval", "Reject once"),
        (
            "muse",
            MUSE_PEER_FOCUSED.to_owned(),
            "peer-approval",
            "Reject once",
        ),
    ]
}

#[test]
fn deny_labels_refuse_and_approving_labels_never_pass() {
    use agentctl::harness_tui::deny_label;
    for label in [
        "No",
        "no",
        "No, and tell Claude what to do differently (esc)",
        "Reject once",
        "Abort the entire command (esc)",
    ] {
        assert!(deny_label(label), "{label}");
    }
    for label in [
        "Yes",
        "Yes, proceed",
        "Yes, and don't ask again for rm commands in /work/project",
        "Allow once",
        "Allow for this session",
        "Allow this stage once (y)",
        "Trust all and continue",
        "Continue without trusting",
        "Block",
        "None of the above",
        "Not now",
        "main",
        "",
    ] {
        assert!(!deny_label(label), "{label}");
    }
}

#[test]
fn each_recognised_prompt_names_its_refusal_and_what_it_asked() {
    use agentctl::harness_tui::permission_denial;
    for (harness, screen, kind, label) in recognised() {
        let denial = permission_denial(Some(harness), &screen)
            .unwrap_or_else(|| panic!("{kind} on {harness} not recognised"));
        assert_eq!(denial.kind, kind);
        let chosen = denial
            .options
            .iter()
            .find(|option| option.number == denial.deny)
            .unwrap();
        assert_eq!(chosen.text, label, "{kind}");
    }
    let permission = permission_denial(Some("claude"), CLAUDE_PERMISSION).unwrap();
    assert!(
        permission.prompt.contains("rm -rf /work/project/victim"),
        "{}",
        permission.prompt
    );
    assert!(
        permission.prompt.contains("requires confirmation"),
        "{}",
        permission.prompt
    );
    let hook = permission_denial(Some("claude"), CLAUDE_HOOK).unwrap();
    assert!(
        hook.prompt
            .contains("Hook PreToolUse:Bash requires confirmation"),
        "{}",
        hook.prompt
    );
    let command = permission_denial(Some("muse"), MUSE_APPROVAL).unwrap();
    assert!(
        command.prompt.contains("$ date > stamp.txt"),
        "{}",
        command.prompt
    );
    let peer = permission_denial(Some("muse"), MUSE_PEER).unwrap();
    assert_eq!(peer.selected, None);
    assert_eq!(peer.focus_keys, ["Down"]);
    assert!(
        peer.prompt.contains("Message from coordinator"),
        "{}",
        peer.prompt
    );
}

#[test]
fn prompts_that_are_not_permission_asks_are_never_recognised() {
    use agentctl::harness_tui::permission_denial;
    // A folder trust dialog ("No, exit" would quit the harness), an
    // ordinary question, idle composers, a harness with no recogniser, and
    // the wrong harness for a screen are all left to the report path.
    assert!(permission_denial(Some("claude"), CLAUDE_TRUST).is_none());
    assert!(permission_denial(Some("claude"), CLAUDE_QUESTION).is_none());
    assert!(permission_denial(Some("claude"), CLAUDE_IDLE).is_none());
    assert!(permission_denial(Some("muse"), MUSE_IDLE).is_none());
    assert!(permission_denial(Some("codex"), CLAUDE_PERMISSION).is_none());
    assert!(permission_denial(None, CLAUDE_PERMISSION).is_none());
    assert!(permission_denial(Some("muse"), CLAUDE_PERMISSION).is_none());
    assert!(permission_denial(Some("claude"), MUSE_APPROVAL).is_none());
    // A form with no refusal, or two refusals of which none is plain, is ambiguous.
    let no_refusal = CLAUDE_HOOK.replace("  2. No", "  2. Yes, always");
    assert!(permission_denial(Some("claude"), &no_refusal).is_none());
    let two = CLAUDE_DANGEROUS.replace(
        "   2. Yes, and don't ask again for rm commands in /work/project",
        "   2. No, exit",
    );
    assert!(permission_denial(Some("claude"), &two).is_none());
    // Without the proceed line it is not the exact form.
    let no_proceed = CLAUDE_HOOK.replace("Do you want to proceed?\n", "");
    assert!(permission_denial(Some("claude"), &no_proceed).is_none());
}

#[test]
fn a_refusal_submits_only_the_refusing_option_from_every_cursor_position() {
    // The property #2421 promises: whatever option the cursor starts on,
    // the submit key is sent once, and only with a refusal selected.
    use agentctl::harness_tui::{deny_label, permission_denial};
    for (harness, screen, kind, label) in recognised() {
        let denial = permission_denial(Some(harness), &screen).unwrap();
        for start in 0..denial.options.len() {
            let menu = Menu::new(&screen, harness, Fault::None);
            if denial.selected.is_some() {
                menu.start_at(start);
            }
            let result = deny_on(&menu, harness).unwrap().unwrap();
            assert_eq!(result["denied"], true);
            assert_eq!(result["kind"], kind);
            assert_eq!(result["option"], label);
            let submitted = menu.submitted.borrow();
            assert_eq!(submitted.len(), 1, "{kind} from {start}: {submitted:?}");
            assert!(
                deny_label(&submitted[0]),
                "{kind} from {start}: {submitted:?}"
            );
            assert_eq!(submitted[0], label);
            let keys = menu.keys.borrow();
            assert_eq!(keys.last().map(String::as_str), Some("Enter"));
            assert_eq!(keys.iter().filter(|key| *key == "Enter").count(), 1);
        }
    }
}

#[test]
fn a_refusal_moves_up_or_down_and_focuses_an_unfocused_menu_first() {
    let menu = Menu::new(CLAUDE_PERMISSION, "claude", Fault::None);
    deny_on(&menu, "claude").unwrap().unwrap();
    assert_eq!(*menu.keys.borrow(), ["Down", "Enter"]);
    let menu = Menu::new(CLAUDE_DANGEROUS, "claude", Fault::None);
    menu.start_at(2);
    deny_on(&menu, "claude").unwrap().unwrap();
    assert_eq!(*menu.keys.borrow(), ["Enter"]);
    let menu = Menu::new(CLAUDE_PERMISSION, "claude", Fault::None);
    menu.start_at(1);
    deny_on(&menu, "claude").unwrap().unwrap();
    assert_eq!(*menu.keys.borrow(), ["Enter"]);
    let menu = Menu::new(MUSE_PEER_FOCUSED, "muse", Fault::None);
    menu.start_at(3);
    deny_on(&menu, "muse").unwrap().unwrap();
    assert_eq!(*menu.keys.borrow(), ["Up", "Up", "Enter"]);
    let menu = Menu::new(MUSE_PEER, "muse", Fault::None);
    deny_on(&menu, "muse").unwrap().unwrap();
    assert_eq!(*menu.keys.borrow(), ["Down", "Down", "Enter"]);
    assert_eq!(*menu.submitted.borrow(), ["Reject once"]);
}

#[test]
fn an_unrecognised_prompt_or_a_different_prompt_gets_no_key() {
    for (harness, screen) in [
        ("claude", CLAUDE_TRUST),
        ("claude", CLAUDE_QUESTION),
        ("codex", CLAUDE_PERMISSION),
        ("muse", MUSE_IDLE),
    ] {
        let menu = Menu::new(screen, harness, Fault::None);
        assert!(deny_on(&menu, harness).unwrap().is_none());
        assert!(menu.keys.borrow().is_empty(), "{harness}");
    }
    // The caller timed one prompt; another one on screen now is left alone.
    let expected = agentctl::harness_tui::permission_denial(Some("claude"), CLAUDE_HOOK).unwrap();
    let menu = Menu::new(CLAUDE_PERMISSION, "claude", Fault::None);
    let result = agentctl::tui_prompts::deny(
        &menu,
        "pane",
        "claude",
        Some(&expected),
        &|_| Ok(()),
        &Runtime::default(),
    );
    assert!(result.unwrap().is_none());
    assert!(menu.keys.borrow().is_empty());
}

#[test]
fn a_cursor_that_does_not_land_or_a_prompt_that_changes_stops_before_the_submit_key() {
    for fault in [Fault::StuckCursor, Fault::PromptChanges] {
        let menu = Menu::new(CLAUDE_PERMISSION, "claude", fault);
        let error = deny_on(&menu, "claude").unwrap_err();
        assert!(error.to_string().contains("no submit key sent"), "{error}");
        assert!(!menu.keys.borrow().contains(&"Enter".to_owned()));
        assert!(menu.submitted.borrow().is_empty());
    }
}

#[test]
fn a_refusal_whose_prompt_stays_up_is_reported_possibly_submitted_not_retried() {
    let menu = Menu::new(CLAUDE_PERMISSION, "claude", Fault::SubmitIgnored);
    let error = deny_on(&menu, "claude").unwrap_err();
    assert_eq!(error.exit_code(), 76, "{error}");
    assert!(
        error.to_string().contains("do not retry automatically"),
        "{error}"
    );
    assert_eq!(*menu.submitted.borrow(), ["No"]);
}

// Cold-review regressions for #2421 (ec2b2537).

/// Review finding 1: a command preview (a heredoc) that draws a fake hook
/// heading, a fake proceed line and a fake `2. No` above the real dialog,
/// whose real menu is `1. Yes / 2. Yes, and don't ask again / 3. No`.
const CLAUDE_FAKE_IN_PREVIEW: &str = "────────────────────────────────────────\n Bash command\n cat <<'EOF'\n Hook PreToolUse:Bash requires confirmation\n Do you want to proceed?\n   2. No\n EOF\n Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don't ask again for cat commands in /work/project\n   3. No\n Esc to cancel · Tab to amend\n";

#[test]
fn text_above_the_real_menu_is_never_read_as_an_option() {
    use agentctl::harness_tui::permission_denial;
    let denial = permission_denial(Some("claude"), CLAUDE_FAKE_IN_PREVIEW).unwrap();
    let labels: Vec<&str> = denial
        .options
        .iter()
        .map(|option| option.text.as_str())
        .collect();
    assert_eq!(
        labels,
        [
            "Yes",
            "Yes, and don't ask again for cat commands in /work/project",
            "No"
        ]
    );
    assert_eq!(denial.deny, 3);
    assert_eq!(denial.selected_text.as_deref(), Some("Yes"));
    for start in 0..3 {
        let menu = Menu::new(CLAUDE_FAKE_IN_PREVIEW, "claude", Fault::None);
        menu.start_at(start);
        deny_on(&menu, "claude").unwrap().unwrap();
        assert_eq!(*menu.submitted.borrow(), ["No"], "from {start}");
    }
    let menu = Menu::new(CLAUDE_FAKE_IN_PREVIEW, "claude", Fault::None);
    deny_on(&menu, "claude").unwrap().unwrap();
    assert_eq!(*menu.keys.borrow(), ["Down", "Down", "Enter"]);
}

#[test]
fn a_menu_not_numbered_exactly_one_to_n_or_with_two_cursors_is_not_recognised() {
    use agentctl::harness_tui::permission_denial;
    let menu = |rows: &str| {
        format!(
            "────────────────────────────────────────\n Bash command\n rm -rf /work/project/victim\n Do you want to proceed?\n{rows} Esc to cancel · Tab to amend\n"
        )
    };
    let good = menu(" ❯ 1. Yes\n   2. No\n");
    assert!(permission_denial(Some("claude"), &good).is_some());
    for rows in [
        " ❯ 1. Yes\n   2. Yes, and don't ask again\n   2. No\n",
        " ❯ 2. Yes\n   3. No\n",
        " ❯ 1. Yes\n   3. No\n",
        " ❯ 1. Yes\n ❯ 2. No\n",
        "   1. Yes\n   2. No\n",
    ] {
        assert!(
            permission_denial(Some("claude"), &menu(rows)).is_none(),
            "{rows}"
        );
    }
}

#[test]
fn a_dialog_that_is_not_the_live_bottom_of_the_screen_is_not_recognised() {
    use agentctl::harness_tui::permission_denial;
    let later = format!("{CLAUDE_PERMISSION}● I ran something else afterwards.\n");
    assert!(permission_denial(Some("claude"), &later).is_none());
}

#[test]
fn context_words_outside_the_dialog_box_or_a_question_form_get_no_refusal() {
    // Review finding 3: an AskUserQuestion-style proceed question, or one
    // whose permission words sit only in the transcript above the box, or
    // a box too tall to be a permission dialog, is never refused.
    use agentctl::harness_tui::permission_denial;
    let question = "────────────────────────────────────────\n Should I run the dangerous bash command cleanup?\n Do you want to proceed?\n ❯ 1. Yes\n   2. No\n Enter to select · ↑/↓ to navigate · Esc to cancel\n";
    assert!(permission_denial(Some("claude"), question).is_none());
    let outside = "● I am about to run a dangerous bash command that requires confirmation.\n────────────────────────────────────────\n Ready?\n Do you want to proceed?\n ❯ 1. Yes\n   2. No\n Esc to cancel · Tab to amend\n";
    assert!(permission_denial(Some("claude"), outside).is_none());
    let tall = format!(
        "────────────────────────────────────────\n Bash command\n{} Do you want to proceed?\n ❯ 1. Yes\n   2. No\n Esc to cancel · Tab to amend\n",
        " line\n".repeat(25)
    );
    assert!(permission_denial(Some("claude"), &tall).is_none());
    for screen in [question, outside, tall.as_str()] {
        let menu = Menu::new(screen, "claude", Fault::None);
        assert!(deny_on(&menu, "claude").unwrap().is_none());
        assert!(menu.keys.borrow().is_empty());
    }
}

#[test]
fn a_cursor_that_lands_on_the_wrong_option_gets_no_submit_key() {
    let menu = Menu::new(CLAUDE_FAKE_IN_PREVIEW, "claude", Fault::CursorSkips);
    menu.start_at(0);
    // Down from 1 is expected to land on 2; it lands on 3: stop.
    let error = deny_on(&menu, "claude").unwrap_err();
    assert!(error.to_string().contains("no submit key sent"), "{error}");
    assert!(menu.submitted.borrow().is_empty());
}

#[test]
fn a_peer_message_that_quotes_option_rows_never_moves_the_refusal() {
    use agentctl::harness_tui::permission_denial;
    // Quoted rows separated from the real menu are text, not options.
    let quoted = MUSE_PEER_FOCUSED.replace(
        "  \"The gate queue moved; may I re-run your shard on the new base?\"\n",
        "  \"Pick one:\n  1. Allow once\n  2. Allow for this session\"\n\n",
    );
    assert_ne!(quoted, MUSE_PEER_FOCUSED);
    let denial = permission_denial(Some("muse"), &quoted).unwrap();
    assert_eq!(denial.deny, 2);
    assert_eq!(denial.options.len(), 4);
    let menu = Menu::new(&quoted, "muse", Fault::None);
    deny_on(&menu, "muse").unwrap().unwrap();
    assert_eq!(*menu.submitted.borrow(), ["Reject once"]);
    // Rows run into the menu: numbering breaks, nothing is recognised.
    let adjacent = MUSE_PEER_FOCUSED.replace(
        "⟩ 1. Allow once",
        "  1. Allow once\n  2. Allow once\n⟩ 1. Allow once",
    );
    assert_ne!(adjacent, MUSE_PEER_FOCUSED);
    assert!(permission_denial(Some("muse"), &adjacent).is_none());
}
