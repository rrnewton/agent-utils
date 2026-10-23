"""Read and answer recognized forms without replaying an uncertain submission."""
from __future__ import annotations

import time
from dataclasses import replace
from collections.abc import Callable

from agentctl.client import GuardedInput
from agentctl.errors import AgentDeliveryError, AgentPossiblySubmitted
from agentctl.harness_tui import HarnessTui, Question, State, View


def blocked_hint(view: View) -> str:
    """A useful pre-injection refusal, including only text actually seen."""
    question = view.question.text if view.question else "Question text is not recognizable on this screen."
    return (f"{question} Use agentctl prompts NAME to inspect; "
            "agentctl answer NAME --option N or --text TEXT to answer a supported form.")


def inspect_question(
    terminal: GuardedInput, pane: str, adapter: HarnessTui, *,
    verify: Callable[[str], None], sleep: Callable[[float], None] = time.sleep,
    monotonic: Callable[[], float] = time.monotonic,
) -> View:
    """Reveal a collapsed question only; never select, paste, or submit an answer."""
    attempted = False
    try:
        def observe() -> View:
            verify(pane)
            view = adapter.classify(terminal.read_screen(pane))
            verify(pane)
            return view

        view = observe()
        if view.question is None or not view.question.collapsed:
            return view
        for key in adapter.open_question_keys:
            attempted = True
            terminal.send_keys(pane, key)
        deadline = monotonic() + 2
        while True:
            view = observe()
            if view.state == State.QUESTION and view.question is not None and not view.question.collapsed:
                return view
            if monotonic() >= deadline:
                raise AgentDeliveryError("question did not open into a supported form; no answer selected or submitted")
            sleep(0.1)
    except Exception as exc:
        if attempted:
            raise AgentPossiblySubmitted(str(exc), message_id="tui-reveal",
                artifact="visible pane; inspect interactively; no answer submission attempted") from exc
        raise


def answer(
    terminal: GuardedInput, pane: str, adapter: HarnessTui, *,
    option: int | None = None, text: str | None = None, replace_draft: bool = False,
    verify: Callable[[str], None], sleep: Callable[[float], None] = time.sleep,
    monotonic: Callable[[], float] = time.monotonic,
) -> dict[str, object]:
    """Never repeat Enter: after an input attempt every uncertain result exits 76."""
    if (option is None) == (text is None):
        raise AgentDeliveryError("choose exactly one of --option or --text")
    if replace_draft and text is None:
        raise AgentDeliveryError("--replace-draft requires --text")
    attempted = False

    def observe() -> View:
        verify(pane)
        view = adapter.classify(terminal.read_screen(pane))
        verify(pane)
        return view

    def key(value: str) -> None:
        nonlocal attempted
        attempted = True
        terminal.send_keys(pane, value)

    def same_question(expected: Question, *, selected: int | None = None, notes: bool | None = None) -> Question:
        deadline = monotonic() + 2
        while True:
            view = observe()
            q = view.question
            if q is not None and (q.text != expected.text or q.options != expected.options
                                  or q.total != expected.total or q.draft != expected.draft):
                raise AgentDeliveryError("screen changed or question has a draft; inspect agentctl prompts before answering")
            if (view.state == State.QUESTION and q is not None and not q.collapsed
                    and (selected is None or q.selected == selected)
                    and (notes is None or q.notes == notes)):
                return q
            if monotonic() >= deadline:
                raise AgentDeliveryError("question selection redraw was not verified; no further key sent")
            sleep(0.1)

    try:
        view = observe()
        if view.state != State.QUESTION or view.question is None:
            raise AgentDeliveryError("no supported blocking question is visible; nothing was typed")
        if view.question.collapsed:
            for value in adapter.open_question_keys:
                key(value)
            deadline = monotonic() + 2
            while True:
                view = observe()
                if view.question is not None and not view.question.collapsed:
                    break
                if monotonic() >= deadline:
                    raise AgentDeliveryError("question did not open into a supported form")
                sleep(0.1)
        q = view.question
        if q is None:
            raise AgentDeliveryError("question did not open into a supported form")
        if replace_draft:
            if (adapter.free_text_clear_key is None or q.options or q.total != 1
                    or q.collapsed or q.notes):
                raise AgentDeliveryError("draft replacement requires a supported single free-text form")
            # Validate the replacement before clearing any caller-inspected draft.
            adapter.answer_plan(replace(q, draft=""), option, text)
            if q.draft:
                same_question(q, selected=q.selected, notes=False)
                key(adapter.free_text_clear_key)
                deadline = monotonic() + 2
                while True:
                    cleared = observe()
                    current = cleared.question
                    if (cleared.state != State.QUESTION or current is None
                            or replace(current, draft="") != replace(q, draft="")):
                        raise AgentDeliveryError("question changed while clearing draft; no answer pasted or submitted")
                    if not current.draft:
                        q = current
                        break
                    if current.draft != q.draft or monotonic() >= deadline:
                        raise AgentDeliveryError("empty draft was not verified; no answer pasted or submitted")
                    sleep(0.1)
        plan = adapter.answer_plan(q, option, text)
        selected = q.selected
        for value in plan.keys:
            same_question(q, selected=selected, notes=False)
            key(value)
            if value == "Down" and selected is not None:
                selected += 1
            elif value == "Up" and selected is not None:
                selected -= 1
        same_question(q, selected=plan.selected, notes=bool(plan.text and q.options))
        if plan.text is not None:
            attempted = True
            terminal.send_text(pane, "\x1b[200~" + plan.text + "\x1b[201~")
            # No submission key until the exact literal answer is visible in this same form.
            deadline = monotonic() + 2
            while True:
                staged = observe().question
                if (staged is not None and staged.text == q.text and staged.options == q.options
                        and staged.total == q.total and staged.selected == plan.selected
                        and staged.draft == plan.text):
                    break
                if monotonic() >= deadline:
                    raise AgentDeliveryError("answer paste was not verified; no submission key sent")
                sleep(0.1)
        key(plan.submit_key)
        deadline = monotonic() + 3
        while True:
            after = observe()
            if after.state in (State.IDLE, State.WORKING):
                return {"answered": True, "pane_id": pane, "question": q.text}
            if monotonic() >= deadline:
                raise AgentDeliveryError("question dismissal was not verified; answer may have been submitted; do not retry automatically")
            sleep(0.1)
    except Exception as exc:
        if attempted:
            raise AgentPossiblySubmitted(str(exc), message_id="tui-answer", artifact="visible pane; inspect with agentctl prompts NAME") from exc
        if isinstance(exc, ValueError):
            raise AgentDeliveryError(str(exc)) from exc
        raise
