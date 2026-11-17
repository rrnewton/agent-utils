"""Claude Code single-question selection forms, distinct from approval menus."""
from __future__ import annotations

import re

from . import AnswerPlan, Option, Question, State, View, navigation, plain


class ClaudeTui:
    """Working Claude follow-ups retain the existing explicit opt-in policy."""

    working_followups = True
    working_submit_key = "Enter"
    open_question_keys: tuple[str, ...] = ()
    free_text_clear_key: str | None = None

    def classify(self, screen: str) -> View:
        """Recognize a single selection form without treating approval menus as questions."""
        from agentctl.submission import composer_view

        clean = plain(screen)
        rows = clean.splitlines()
        # A Claude hook confirmation (``Hook PreToolUse:Bash requires
        # confirmation … Do you want to proceed? 1. Yes 2. No``) is an
        # owner-only decision (#1784): recognize it as a blocking question
        # naming the hook and the first line of its reason, so the watcher
        # can page a person. ``answer_plan`` refuses it: nothing answers
        # it automatically.
        hook = next(
            (i for i, row in enumerate(rows) if "requires confirmation" in row.lower()),
            None,
        )
        if hook is not None and any("do you want to proceed" in row.lower() for row in rows):
            hook_options: list[Option] = []
            hook_selected = None
            for row in rows:
                match = re.match(r"^\s*(❯\s*)?(\d+)\.\s+(.+)$", row)
                if match:
                    number = int(match[2])
                    hook_options.append(Option(number, match[3].strip()))
                    if match[1]:
                        hook_selected = number
            if len(hook_options) >= 2 and hook_selected is not None:
                reason = next(
                    (
                        row.strip()
                        for row in rows[hook + 1:]
                        if row.strip()
                        and "do you want to proceed" not in row.lower()
                        and not re.match(r"^\s*(❯\s*)?\d+\.\s+", row)
                    ),
                    "",
                )
                heading = rows[hook].strip()
                text = f"{heading}: {reason}" if reason else heading
                return View(
                    State.QUESTION,
                    Question(text, tuple(hook_options), hook_selected, total=1),
                )
        # Transcript questions above the current form cannot supply its title/options.
        border = max((i for i, row in enumerate(rows) if len(row.strip()) >= 3
                      and set(row.strip()) <= set("─━═")), default=-1)
        rows = rows[border + 1:]
        tail = "\n".join(rows[-16:]).lower()
        footer = next((row.strip().lower() for row in reversed(rows) if row.strip()), "")
        if "enter to select" in footer and "esc to cancel" in footer:
            # Approval/trust and command pickers must never masquerade as questions.
            if any(word in "\n".join(rows).lower() for word in ("do you want to proceed", "trust", "select model", "permission", "bash command")):
                return View(State.MENU)
            options: list[Option] = []
            selected = None
            question = ""
            for row in rows:
                match = re.match(r"^\s*(❯\s*)?(\d+)\.\s+(.+)$", row)
                if match:
                    number = int(match[2])
                    options.append(Option(number, match[3].strip()))
                    if match[1]:
                        selected = number
                elif row.strip().endswith("?"):
                    question = row.strip()
            if question and options and selected is not None:
                count = "\n".join(rows).count("☐") + "\n".join(rows).count("☑")
                return View(State.QUESTION, Question(question, tuple(options), selected, total=max(1, count)))
            return View(State.MENU)
        if "esc to cancel" in tail or "enter to confirm" in tail:
            return View(State.MENU)
        view = composer_view("claude", screen)
        if view is not None:
            return View(State.WORKING if "esc to interrupt" in tail else State.IDLE)
        return View(State.UNKNOWN)

    def answer_plan(self, question: Question, option: int | None, text: str | None) -> AnswerPlan:
        """Plan only a recognized numbered selection; refuse unmodeled text editors."""
        # A hook confirmation is owner-only (#1784): never answered through
        # agentctl, automatically or otherwise; a person answers at the pane.
        if "requires confirmation" in question.text.lower():
            raise ValueError(
                "this is an owner-only hook confirmation; answer it at the pane "
                "in person — agentctl never answers it"
            )
        if text is not None or option is None:
            raise ValueError("this Claude form supports --option only; free-text forms are not recognized safely")
        keys = navigation(question, option)
        item = next(item for item in question.options if item.number == option)
        if item.text.lower().startswith(("other", "type something")):
            raise ValueError("this option opens another form; answer it interactively")
        return AnswerPlan(keys, "Enter", selected=option)
