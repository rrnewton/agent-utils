"""Codex default-keymap questions; unknown layouts never receive answer input."""
from __future__ import annotations

import re

from . import AnswerPlan, Option, Question, State, View, navigation, plain


class CodexTui:
    """Codex accepts ordinary follow-ups on Enter while a turn is working."""

    working_followups = True
    working_submit_key = "Enter"
    open_question_keys: tuple[str, ...] = ("shift+left",)

    def classify(self, screen: str) -> View:
        """Read default-keymap Codex forms, refusing missing or contradictory layout cues."""
        from agentctl.submission import codex_idle_composer, composer_view

        clean = plain(screen)
        rows = clean.splitlines()
        tail = "\n".join(rows[-16:]).lower()
        footer = next((row.strip().lower() for row in reversed(rows) if row.strip()), "")
        if (footer.startswith("enter submit") and "^] skip" in footer
                and ("shift+→ main prompt" in footer or "shift+right main prompt" in footer)):
            # Async questions have a dedicated editor but no numbered Question header.
            queued = [i for i, row in enumerate(rows)
                      if row.strip() in ("• Queued follow-up inputs", "Queued follow-up inputs")]
            if queued:
                last = max(i for i, row in enumerate(rows) if row.strip())
                parts = re.split(r"\n\s*\n", "\n".join(rows[queued[-1] + 1:last]).strip())
                if len(parts) >= 2 and "\n" not in parts[-1]:
                    editor = parts[-1].strip().removeprefix("›").strip()
                    title = "\n\n".join("\n".join(line.strip() for line in part.splitlines())
                                        for part in parts[:-1]).strip()
                    if title and not re.search(r"(?m)^\s*Question \d+/\d+", title):
                        draft = "" if editor == "Type your answer" else editor
                        return View(State.QUESTION, Question(title, draft=draft))
            return View(State.MENU)
        headers = list(re.finditer(r"(?m)^\s*Question (\d+)/(\d+) \(\d+ unanswered\)\s*$", clean))
        if headers and ("enter to submit answer" in footer or "enter to submit all" in footer):
            header = headers[-1]
            body = clean[header.end():].splitlines()
            options: list[Option] = []
            selected = None
            title: list[str] = []
            draft = ""
            notes = "tab or esc to clear notes" in tail
            editor = False
            for row in body:
                line = row.strip()
                if "enter to submit" in line:
                    break
                match = re.match(r"^\s*(›\s*)?(\d+)\.\s+(.+)$", row)
                if match:
                    number = int(match[2])
                    options.append(Option(number, match[3].strip()))
                    if match[1]:
                        selected = number
                elif line.startswith("›"):
                    editor = True
                    content = line[1:].strip()
                    if content not in ("Type your answer (optional)", "Add notes"):
                        draft = content
                elif line:
                    if options:
                        last = options[-1]
                        options[-1] = Option(last.number, last.text + " " + line)
                    else:
                        title.append(line)
            if not title or not options and not editor:
                return View(State.MENU)
            return View(State.QUESTION, Question("\n".join(title), tuple(options), selected,
                        int(header[2]), notes=notes, draft=draft))
        if "shift+← to answer" in tail or "shift+left to answer" in tail:
            return View(State.QUESTION, Question("Question collapsed; text and options are not visible.", collapsed=True))
        view = composer_view("codex", screen)
        modal_region = footer if view is not None else tail
        if any(label in modal_region for label in ("esc to cancel", "enter to confirm", "trust this folder",
                                                  "allow command?", "enter to submit", "select model")):
            return View(State.MENU)
        if view is not None and ("esc to interrupt" in tail or "tab to queue message" in tail):
            return View(State.WORKING)
        if codex_idle_composer(screen):
            return View(State.IDLE)
        return View(State.UNKNOWN)

    def answer_plan(self, question: Question, option: int | None, text: str | None) -> AnswerPlan:
        """Plan one visible option or empty text answer without guessing hidden state."""
        if option is not None:
            keys = navigation(question, option)
            item = next(item for item in question.options if item.number == option)
            if item.text.startswith("None of the above"):
                raise ValueError("the free-text option needs --text, not --option")
            return AnswerPlan(keys, "Enter", selected=option)
        if text is None or not text.strip() or any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in text):
            raise ValueError("--text must be nonempty single-line text without terminal controls")
        if not question.options:
            if question.total != 1 or question.collapsed or question.draft or question.notes:
                raise ValueError("free-text question is not empty or has an unsupported layout")
            return AnswerPlan((), "Enter", text=text)
        other = next((item.number for item in question.options if item.text.startswith("None of the above")), None)
        if other is None:
            raise ValueError("this question has no visible free-text option; choose --option")
        return AnswerPlan(navigation(question, other) + ("Tab",), "Enter", text=text, selected=other)
