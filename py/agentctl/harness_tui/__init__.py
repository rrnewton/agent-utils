"""Harness-specific terminal forms behind one conservative, read-only classifier."""
from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import Protocol


class State(str, Enum):
    """What the visible terminal, rather than its scrollback, currently accepts."""

    IDLE = "idle_composer"
    WORKING = "working"
    QUESTION = "blocking_question"
    MENU = "menu_or_picker"
    UNKNOWN = "unknown"


@dataclass(frozen=True)
class Option:
    """One displayed option, with its one-based screen number."""

    number: int
    text: str


@dataclass(frozen=True)
class Question:
    """Only facts actually visible in a supported question form."""

    text: str
    options: tuple[Option, ...] = ()
    selected: int | None = None
    total: int = 1
    collapsed: bool = False
    notes: bool = False
    draft: str = ""


@dataclass(frozen=True)
class View:
    """Classification and optional extracted question."""

    state: State
    question: Question | None = None


@dataclass(frozen=True)
class AnswerPlan:
    """Navigation before staging an optional literal answer and submitting once."""

    keys: tuple[str, ...]
    submit_key: str
    text: str | None = None
    selected: int | None = None


class HarnessTui(Protocol):
    """All harness-specific screen matching and question key policy lives here."""

    working_followups: bool
    working_submit_key: str
    open_question_keys: tuple[str, ...]

    def classify(self, screen: str) -> View:
        """Classify visible text and extract any safely recognizable question."""

    def answer_plan(self, question: Question, option: int | None, text: str | None) -> AnswerPlan:
        """Refuse unsupported forms; return keys for the exact displayed question."""


def harness_tui(harness: str | None) -> HarnessTui | None:
    """Select an adapter without duplicating its screen rules in callers."""
    from .claude import ClaudeTui
    from .codex import CodexTui

    adapters: dict[str, HarnessTui] = {"codex": CodexTui(), "claude": ClaudeTui()}
    return adapters.get(harness or "")


def plain(screen: str) -> str:
    """Use the same escape stripping as verified composer submission."""
    from agentctl.submission import render_screen

    return "\n".join(row for row, _solid in render_screen(screen))


def navigation(question: Question, number: int) -> tuple[str, ...]:
    """Navigate only a fully visible, contiguous list with a known selection."""
    if question.total != 1 or question.collapsed or question.notes or question.draft:
        raise ValueError("only one fully visible, unanswered question with no existing draft is supported")
    numbers = [item.number for item in question.options]
    if numbers != list(range(1, len(numbers) + 1)) or number not in numbers or question.selected not in numbers:
        raise ValueError("option is out of range, hidden, or the current selection is not visible")
    assert question.selected is not None
    delta = number - question.selected
    return ("Down" if delta > 0 else "Up",) * abs(delta)
