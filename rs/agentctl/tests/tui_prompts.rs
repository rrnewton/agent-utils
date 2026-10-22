//! The same pane captures as Python, plus guarded, at-most-once answer effects.
use std::cell::{Cell, RefCell};
use std::time::Duration;

use agentctl::agent::AgentRuntime;
use agentctl::harness_tui::{harness_tui, State};
use agentctl::submission::GuardedInput;

const OPTIONS: &str = include_str!("fixtures/tui/codex-options.txt");
const FREEFORM: &str = include_str!("fixtures/tui/codex-freeform.txt");
const ASYNC_FREEFORM: &str = include_str!("fixtures/tui/codex-async-freeform.txt");
const WORKING: &str = "• Working (2s • esc to interrupt)\n»\n  GPT default · /work/project\n";

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
            *self.screen.borrow_mut() = WORKING.to_owned();
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
