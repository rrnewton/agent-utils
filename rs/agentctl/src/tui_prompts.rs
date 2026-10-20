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
                    || !q.draft.is_empty()
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
    verify: &dyn Fn(&str) -> crate::error::Result<()>,
    runtime: &dyn AgentRuntime,
) -> AgentResult<Value> {
    let mut answering = Answering {
        terminal,
        pane,
        adapter,
        verify,
        runtime,
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
