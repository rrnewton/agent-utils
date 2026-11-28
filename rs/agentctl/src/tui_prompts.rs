//! Verified question answering. A possibly submitted answer is never retried.
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

use crate::agent::{AgentError, AgentResult, AgentRuntime, UndeliveredMessage};
use crate::harness_tui::{HarnessTui, Question, State, View};
use crate::submission::GuardedInput;

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

struct Answering<'a> {
    terminal: &'a dyn GuardedInput,
    pane: &'a str,
    adapter: &'a dyn HarnessTui,
    verify: &'a dyn Fn(&str) -> crate::error::Result<()>,
    runtime: &'a dyn AgentRuntime,
    confirm_dangerous: bool,
    attempted: bool,
}
impl Answering<'_> {
    fn observe(&self) -> AgentResult<View> {
        (self.verify)(self.pane)?;
        let screen = self.terminal.read_screen(self.pane)?;
        (self.verify)(self.pane)?;
        Ok(self.adapter.classify(&screen))
    }
    fn key(&mut self, key: &str) -> AgentResult<()> {
        self.attempted = true;
        self.terminal.send_keys(self.pane, key)?;
        Ok(())
    }
    fn same(&self, expected: &Question, selected: Option<usize>, notes: bool) -> AgentResult<()> {
        let deadline = self.runtime.monotonic() + Duration::from_secs(2);
        loop {
            let view = self.observe()?;
            if view.question.as_ref().is_some_and(|q| {
                q.text != expected.text
                    || q.options != expected.options
                    || q.total != expected.total
                    || q.draft != expected.draft
            }) {
                return Err(fail("screen changed or question has a draft; inspect agentctl prompts before answering"));
            }
            if view.state == State::BlockingQuestion
                && view.question.as_ref().is_some_and(|q| {
                    !q.collapsed
                        && selected.is_none_or(|n| q.selected == Some(n))
                        && q.notes == notes
                })
            {
                return Ok(());
            }
            if self.runtime.monotonic() >= deadline {
                return Err(fail(
                    "question selection redraw was not verified; no further key sent",
                ));
            }
            self.runtime.sleep(Duration::from_millis(100));
        }
    }
    fn inspect(&mut self) -> AgentResult<View> {
        let mut view = self.observe()?;
        if view.question.as_ref().is_some_and(|q| q.collapsed) {
            for key in self.adapter.open_question_keys() {
                self.key(key)?;
            }
            let deadline = self.runtime.monotonic() + Duration::from_secs(2);
            loop {
                view = self.observe()?;
                if view.question.as_ref().is_some_and(|q| !q.collapsed) {
                    break;
                }
                if self.runtime.monotonic() >= deadline {
                    return Err(fail("question did not open into a supported form"));
                }
                self.runtime.sleep(Duration::from_millis(100));
            }
        }
        Ok(view)
    }
    fn run(&mut self, option: Option<usize>, text: Option<&str>) -> AgentResult<Value> {
        if option.is_some() == text.is_some() {
            return Err(fail("choose exactly one of --option or --text"));
        }
        let view = self.inspect()?;
        if view.state != State::BlockingQuestion {
            return Err(fail(
                "no supported blocking question is visible; nothing was typed",
            ));
        }
        let q = view
            .question
            .ok_or_else(|| fail("question did not open into a supported form"))?;
        // A dangerous-operation permission prompt needs its own explicit
        // confirmation (#2046 agentctl-answer-dangerous-confirm): the
        // refusal names the matched warning line, and so does the
        // confirmed answer's report (the `question` field below). Hook
        // confirmations are refused separately in `answer_plan`, with or
        // without this flag.
        if !self.confirm_dangerous && q.text.to_lowercase().contains("dangerous") {
            return Err(fail(format!(
                "refusing to answer a dangerous-operation prompt without --confirm-dangerous: {}; rerun with --confirm-dangerous only when a human has confirmed this exact warning",
                q.text
            )));
        }
        let plan = self.adapter.answer_plan(&q, option, text).map_err(fail)?;
        let mut selected = q.selected;
        for key in &plan.keys {
            self.same(&q, selected, false)?;
            self.key(key)?;
            if let Some(n) = selected.as_mut() {
                match *key {
                    "Down" => *n += 1,
                    "Up" => *n = n.saturating_sub(1),
                    _ => {}
                }
            }
        }
        self.same(
            &q,
            plan.selected,
            plan.text.is_some() && !q.options.is_empty(),
        )?;
        if let Some(text) = &plan.text {
            self.attempted = true;
            self.terminal
                .send_text(self.pane, &format!("\u{1b}[200~{text}\u{1b}[201~"))?;
            let deadline = self.runtime.monotonic() + Duration::from_secs(2);
            loop {
                if self.observe()?.question.is_some_and(|staged| {
                    staged.text == q.text
                        && staged.options == q.options
                        && staged.total == q.total
                        && staged.selected == plan.selected
                        && staged.draft == *text
                }) {
                    break;
                }
                if self.runtime.monotonic() >= deadline {
                    return Err(fail(
                        "answer paste was not verified; no submission key sent",
                    ));
                }
                self.runtime.sleep(Duration::from_millis(100));
            }
        }
        self.key(plan.submit_key)?;
        let deadline = self.runtime.monotonic() + Duration::from_secs(3);
        loop {
            if matches!(self.observe()?.state, State::IdleComposer | State::Working) {
                return Ok(json!({"answered": true, "pane_id": self.pane, "question": q.text}));
            }
            if self.runtime.monotonic() >= deadline {
                return Err(fail("question dismissal was not verified; answer may have been submitted; do not retry automatically"));
            }
            self.runtime.sleep(Duration::from_millis(100));
        }
    }
}

/// Answer with recipient checks around every observation and guarded input effect.
pub fn answer(
    terminal: &dyn GuardedInput,
    pane: &str,
    adapter: &dyn HarnessTui,
    selection: (Option<usize>, Option<&str>),
    confirm_dangerous: bool,
    verify: &dyn Fn(&str) -> crate::error::Result<()>,
    runtime: &dyn AgentRuntime,
) -> AgentResult<Value> {
    // Same per-pane admission point as prompt delivery: locked and paced.
    let _admission =
        crate::pane_admission::PaneAdmission::acquire(pane).map_err(|e| fail(e.to_string()))?;
    let mut answering = Answering {
        terminal,
        pane,
        adapter,
        verify,
        runtime,
        confirm_dangerous,
        attempted: false,
    };
    answering.run(selection.0, selection.1).map_err(|error| {
        if answering.attempted {
            AgentError::PossiblySubmitted(UndeliveredMessage {
                message: error.to_string(),
                message_id: "tui-answer".to_owned(),
                artifact: PathBuf::from("visible pane; inspect with agentctl prompts NAME"),
            })
        } else {
            error
        }
    })
}

/// Reveal a collapsed form for inspection, without selecting, pasting, or submitting.
pub fn inspect_question(
    terminal: &dyn GuardedInput,
    pane: &str,
    adapter: &dyn HarnessTui,
    verify: &dyn Fn(&str) -> crate::error::Result<()>,
    runtime: &dyn AgentRuntime,
) -> AgentResult<View> {
    let mut inspection = Answering {
        terminal,
        pane,
        adapter,
        verify,
        runtime,
        confirm_dangerous: false,
        attempted: false,
    };
    inspection.inspect().map_err(|error| {
        if inspection.attempted {
            AgentError::PossiblySubmitted(UndeliveredMessage {
                message: error.to_string(),
                message_id: "tui-reveal".to_owned(),
                artifact: PathBuf::from(
                    "visible pane; inspect interactively; no answer submission attempted",
                ),
            })
        } else {
            error
        }
    })
}

/// Refuse a recognised permission or approval prompt (#2421
/// agentctl-asks-never-block): the only answer agentctl gives on its own,
/// and never an approval. The pane is re-read and the prompt re-recognised
/// ([`crate::harness_tui::permission_denial`]) before every key; the cursor
/// is moved one step at a time onto the one refusing option and each move
/// is seen to land; the submit key is sent only while the selected option
/// reads as a refusal ([`crate::harness_tui::deny_label`]); and the prompt
/// must then be seen to leave. A prompt that changes, or a cursor that
/// moves anywhere else, stops the attempt with no submit key.
///
/// `expected` is the prompt the caller saw and timed; a different prompt
/// on screen now is left alone. `Ok(None)` means no recognised prompt is
/// showing (it left, or was never one agentctl may refuse) and nothing was
/// typed.
pub fn deny(
    terminal: &dyn GuardedInput,
    pane: &str,
    harness: &str,
    expected: Option<&crate::harness_tui::Denial>,
    verify: &dyn Fn(&str) -> crate::error::Result<()>,
    runtime: &dyn AgentRuntime,
) -> AgentResult<Option<Value>> {
    let _admission =
        crate::pane_admission::PaneAdmission::acquire(pane).map_err(|e| fail(e.to_string()))?;
    let mut denying = Denying {
        terminal,
        pane,
        harness,
        verify,
        runtime,
        attempted: false,
    };
    denying.run(expected).map_err(|error| {
        if denying.attempted {
            AgentError::PossiblySubmitted(UndeliveredMessage {
                message: error.to_string(),
                message_id: "tui-deny".to_owned(),
                artifact: PathBuf::from("visible pane; inspect with agentctl prompts NAME"),
            })
        } else {
            error
        }
    })
}

struct Denying<'a> {
    terminal: &'a dyn GuardedInput,
    pane: &'a str,
    harness: &'a str,
    verify: &'a dyn Fn(&str) -> crate::error::Result<()>,
    runtime: &'a dyn AgentRuntime,
    attempted: bool,
}

/// Whether two recognised prompts are the same prompt: same kind, text and
/// options (the cursor may differ).
fn same_prompt(a: &crate::harness_tui::Denial, b: &crate::harness_tui::Denial) -> bool {
    a.kind == b.kind && a.prompt == b.prompt && a.options == b.options && a.deny == b.deny
}

impl Denying<'_> {
    fn observe(&self) -> AgentResult<Option<crate::harness_tui::Denial>> {
        (self.verify)(self.pane)?;
        let screen = self.terminal.read_screen(self.pane)?;
        (self.verify)(self.pane)?;
        Ok(crate::harness_tui::permission_denial(
            Some(self.harness),
            &screen,
        ))
    }

    fn key(&mut self, key: &str) -> AgentResult<()> {
        self.attempted = true;
        self.terminal.send_keys(self.pane, key)?;
        Ok(())
    }

    /// Wait for the same prompt with the cursor on `want` (any cursor when
    /// `None` would do is not allowed: a cursor must be drawn).
    fn landed(
        &self,
        prompt: &crate::harness_tui::Denial,
        want: Option<usize>,
    ) -> AgentResult<crate::harness_tui::Denial> {
        let deadline = self.runtime.monotonic() + Duration::from_secs(2);
        loop {
            match self.observe()? {
                Some(now) if same_prompt(&now, prompt) => {
                    if now.selected.is_some() && (want.is_none() || now.selected == want) {
                        return Ok(now);
                    }
                }
                _ => {
                    return Err(fail(
                        "the permission prompt changed or left while it was being refused; no submit key sent",
                    ))
                }
            }
            if self.runtime.monotonic() >= deadline {
                return Err(fail(
                    "the cursor did not land where expected on the permission prompt; no submit key sent",
                ));
            }
            self.runtime.sleep(Duration::from_millis(100));
        }
    }

    fn run(&mut self, expected: Option<&crate::harness_tui::Denial>) -> AgentResult<Option<Value>> {
        let Some(mut prompt) = self.observe()? else {
            return Ok(None);
        };
        if expected.is_some_and(|expected| !same_prompt(expected, &prompt)) {
            return Ok(None);
        }
        if prompt.selected.is_none() {
            for key in prompt.focus_keys {
                self.key(key)?;
            }
            prompt = self.landed(&prompt, None)?;
        }
        let deny = prompt.deny;
        loop {
            let selected = prompt.selected.ok_or_else(|| {
                fail("no cursor is drawn on the permission prompt; no submit key sent")
            })?;
            if selected == deny {
                break;
            }
            let (key, next) = if deny > selected {
                ("Down", selected + 1)
            } else {
                ("Up", selected - 1)
            };
            self.key(key)?;
            prompt = self.landed(&prompt, Some(next))?;
        }
        // The last look before the submit key: still this prompt, the cursor
        // on its refusing option, and that option reads as a refusal.
        let last = self.landed(&prompt, Some(deny))?;
        let chosen = last
            .options
            .iter()
            .find(|option| option.number == deny && Some(option.number) == last.selected)
            .filter(|option| crate::harness_tui::deny_label(&option.text))
            .ok_or_else(|| fail("the selected option is not a refusal; no submit key sent"))?
            .clone();
        self.key("Enter")?;
        let deadline = self.runtime.monotonic() + Duration::from_secs(3);
        loop {
            if self.observe()?.is_none_or(|now| !same_prompt(&now, &last)) {
                return Ok(Some(json!({
                    "denied": true,
                    "pane_id": self.pane,
                    "kind": last.kind,
                    "prompt": last.prompt,
                    "option": chosen.text,
                })));
            }
            if self.runtime.monotonic() >= deadline {
                return Err(fail(
                    "the permission prompt is still showing after its refusal was submitted; do not retry automatically",
                ));
            }
            self.runtime.sleep(Duration::from_millis(100));
        }
    }
}
