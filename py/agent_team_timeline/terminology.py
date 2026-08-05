"""Chronological terminology extraction used before transcript summarization."""

from __future__ import annotations

import re
from collections import defaultdict
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import datetime, timezone
from zoneinfo import ZoneInfo


@dataclass(frozen=True)
class TermSource:
    """Timestamped text considered for terminology extraction."""

    at_ms: int
    text: str


@dataclass(frozen=True)
class GlossaryTerm:
    """A detected term with its first context and occurrence metadata."""

    term: str
    introduced_at_ms: int
    occurrences: int
    context: str
    week: str


_BACKTICK = re.compile(r"`([^`\n]{2,80})`")
_SLUG = re.compile(r"(?<![\w/])[a-z][a-z0-9]+(?:-[a-z0-9]+){1,5}(?![\w/])")
_ACRONYM = re.compile(r"(?<!\w)[A-Z][A-Z0-9]{1,8}(?!\w)")
_SPACE = re.compile(r"\s+")
_SENTENCE = re.compile(r"(?<=[.!?])\s+")
_STOP = frozenset(
    {
        "AGENTS",
        "API",
        "CLI",
        "EDT",
        "EST",
        "JSON",
        "JSONL",
        "MVP",
        "PDT",
        "PST",
        "TODO",
        "UTC",
        "URL",
        "YAML",
        "github",
        "origin-main",
    }
)


def _week(at_ms: int, display_timezone: str) -> str:
    local = datetime.fromtimestamp(at_ms / 1000, tz=timezone.utc).astimezone(
        ZoneInfo(display_timezone)
    )
    year, number, _ = local.isocalendar()
    return f"{year}-W{number:02d}"


def _context(text: str, term: str) -> str:
    clean = _SPACE.sub(" ", text).strip()
    for sentence in _SENTENCE.split(clean):
        if term in sentence:
            return sentence[:320].strip()
    pos = clean.find(term)
    if pos < 0:
        return clean[:320]
    return clean[max(0, pos - 100) : pos + len(term) + 180].strip()


def _candidates(text: str) -> set[str]:
    found = {match.group(1).strip() for match in _BACKTICK.finditer(text)}
    found.update(match.group(0) for match in _SLUG.finditer(text))
    found.update(match.group(0) for match in _ACRONYM.finditer(text))
    return {
        term
        for term in found
        if term not in _STOP
        and 2 <= len(term) <= 80
        and " " not in term.strip("`/")[:1]
        and not term.isdigit()
    }


def scan_terminology(
    sources: Sequence[TermSource], display_timezone: str, *, limit: int = 120
) -> tuple[GlossaryTerm, ...]:
    """Extract stable terms in introduction order.

    This first pass is intentionally deterministic. It gives every summary batch the same project
    vocabulary even when an LLM cache is cold; the original sentence is retained as evidence rather
    than inventing a definition.
    """

    occurrences: dict[str, int] = defaultdict(int)
    first: dict[str, tuple[int, str]] = {}
    explicit: set[str] = set()
    for source in sorted(sources, key=lambda item: (item.at_ms, item.text)):
        backticked = {match.group(1).strip() for match in _BACKTICK.finditer(source.text)}
        explicit.update(backticked)
        for term in _candidates(source.text):
            occurrences[term] += 1
            if term not in first:
                first[term] = (source.at_ms, _context(source.text, term))

    eligible = [
        term
        for term, count in occurrences.items()
        if count >= 2 or term in explicit or "-" in term
    ]
    eligible.sort(key=lambda term: (first[term][0], term.casefold()))
    return tuple(
        GlossaryTerm(
            term=term,
            introduced_at_ms=first[term][0],
            occurrences=occurrences[term],
            context=first[term][1],
            week=_week(first[term][0], display_timezone),
        )
        for term in eligible[:limit]
    )


def glossary_prompt_text(terms: Sequence[GlossaryTerm]) -> str:
    """Render concise chronological context to prepend to summary prompts."""

    lines = ["Project terminology (prefer these exact names):"]
    for term in terms:
        lines.append(f"- {term.term}: {term.context}")
    return "\n".join(lines)


def glossary_markdown(team_slug: str, week: str, terms: Sequence[GlossaryTerm]) -> str:
    """Render one raw, version-controllable weekly glossary file."""

    lines = [
        f"# {week} {team_slug} terminology",
        "",
        "Terms are ordered by first appearance. The quoted context is source evidence used to keep",
        "later summaries aligned with the user's and coordinator's vocabulary.",
        "",
    ]
    matching = [term for term in terms if term.week == week]
    if not matching:
        lines.append("_No new stable terms were introduced in this week._")
    for term in matching:
        instant = datetime.fromtimestamp(term.introduced_at_ms / 1000, tz=timezone.utc)
        lines.extend(
            [
                f"## {term.term}",
                "",
                f"Introduced {instant.isoformat().replace('+00:00', 'Z')}; "
                f"seen {term.occurrences} time(s).",
                "",
                f"> {term.context}",
                "",
            ]
        )
    return "\n".join(lines).rstrip() + "\n"
