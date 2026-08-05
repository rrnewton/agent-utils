"""Chronological terminology extraction used before transcript summarization."""

from __future__ import annotations

import hashlib
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
    """A detected term, stable identity, and optional evidence-bounded definition."""

    term: str
    introduced_at_ms: int
    occurrences: int
    context: str
    week: str
    term_id: str
    definition: str = ""
    definition_status: str = "unavailable"
    available_at_ms: int | None = None

    @property
    def summary_available_at_ms(self) -> int:
        """Return when this term may first affect cached chronological summaries."""

        return (
            self.introduced_at_ms
            if self.available_at_ms is None
            else self.available_at_ms
        )


_BACKTICK = re.compile(r"`([^`\n]{2,80})`")
_SLUG = re.compile(r"(?<![\w/])[a-z][a-z0-9]+(?:-[a-z0-9]+){1,5}(?![\w/])")
_ACRONYM = re.compile(r"(?<!\w)[A-Z][A-Z0-9]{1,8}(?!\w)")
_SPACE = re.compile(r"\s+")
_SENTENCE = re.compile(r"(?<=[.!?])\s+")
_ID_SEPARATOR = re.compile(r"[^a-z0-9]+")
_STOP = frozenset(
    {
        "AGENTS",
        "API",
        "CLI",
        "EDT",
        "EST",
        "IF",
        "JSON",
        "JSONL",
        "MVP",
        "NOT",
        "OR",
        "PARENT",
        "PDT",
        "PST",
        "SAME",
        "THEN",
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


def glossary_term_id(term: str) -> str:
    """Return a stable URL-safe ID for an exact glossary name.

    The readable prefix is cosmetic; the digest makes punctuation/case collisions deterministic
    and keeps links stable even when terms normalize to the same ASCII slug.
    """

    clean = term.strip()
    if not clean:
        raise ValueError("glossary term must not be empty")
    slug = _ID_SEPARATOR.sub("-", clean.casefold()).strip("-")[:48].rstrip("-")
    if not slug:
        slug = "term"
    digest = hashlib.sha256(clean.encode("utf-8")).hexdigest()[:12]
    return f"term-{slug}-{digest}"


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
    eligible_at: dict[str, int] = {}
    for source in sorted(sources, key=lambda item: (item.at_ms, item.text)):
        backticked = {match.group(1).strip() for match in _BACKTICK.finditer(source.text)}
        for term in _candidates(source.text):
            occurrences[term] += 1
            if term not in first:
                first[term] = (source.at_ms, _context(source.text, term))
            if (
                term in backticked
                or "-" in term
                or occurrences[term] >= 2
            ):
                eligible_at.setdefault(term, source.at_ms)

    eligible = list(eligible_at)
    eligible.sort(key=lambda term: (first[term][0], term.casefold()))
    return tuple(
        GlossaryTerm(
            term=term,
            introduced_at_ms=first[term][0],
            occurrences=occurrences[term],
            context=first[term][1],
            week=_week(first[term][0], display_timezone),
            term_id=glossary_term_id(term),
            available_at_ms=eligible_at[term],
        )
        for term in eligible[:limit]
    )


def glossary_prompt_text(terms: Sequence[GlossaryTerm]) -> str:
    """Render concise chronological context to prepend to summary prompts."""

    lines = ["Project terminology (prefer these exact names):"]
    for term in terms:
        lines.append(f"- {term.term}: {term.context}")
    return "\n".join(lines)


def plain_language_context_text(
    project_overview: str, terms: Sequence[GlossaryTerm]
) -> str:
    """Render overview and only supported definitions for newcomer rollup prompts."""

    lines = ["Durable project overview (source-bounded):", project_overview.strip()]
    supported = [term for term in terms if term.definition_status == "supported"]
    lines.extend(["", "Evidence-backed project terminology definitions:"])
    if supported:
        lines.extend(f"- {term.term}: {term.definition}" for term in supported)
    else:
        lines.append("- No model-backed glossary definitions are supported by current evidence.")
    return "\n".join(lines)


def _definition_text(term: GlossaryTerm) -> str:
    if term.definition:
        return term.definition
    return "No model-backed definition is available; consult the first-use evidence below."


def glossary_markdown(team_slug: str, week: str, terms: Sequence[GlossaryTerm]) -> str:
    """Render one raw, version-controllable weekly glossary file."""

    lines = [
        f"# {week} {team_slug} terminology",
        "",
        "Terms are ordered by first appearance. Definitions are generated only from retained",
        "source occurrences; first-use evidence is shown separately.",
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
                f"Stable glossary ID: `{term.term_id}`  ",
                f"Introduced {instant.isoformat().replace('+00:00', 'Z')}; "
                f"seen {term.occurrences} time(s).",
                "",
                "### Definition",
                "",
                _definition_text(term),
                "",
                "### First-use evidence",
                "",
                f"> {term.context}",
                "",
            ]
        )
    return "\n".join(lines).rstrip() + "\n"


def glossary_catalog_markdown(
    team_slug: str, terms: Sequence[GlossaryTerm], project_overview: str = ""
) -> str:
    """Render a discoverable all-time glossary catalog for one team."""

    lines = [
        f"# {team_slug} project glossary",
        "",
        "## Project overview",
        "",
        project_overview.strip()
        or "No model-backed project overview is available for this archive.",
        "",
        "## Project terms",
        "",
        "These source-bounded terms are ordered by first appearance. In the website, recognized",
        "uses of an exact term link to its stable `#glossary/<term-id>` entry. Definitions and",
        "their first-use evidence are deliberately separate.",
        "",
    ]
    if not terms:
        lines.append("_No stable project terms have been detected yet._")
    for term in terms:
        instant = datetime.fromtimestamp(term.introduced_at_ms / 1000, tz=timezone.utc)
        lines.extend(
            [
                f"## {term.term}",
                "",
                f"Stable glossary ID: `{term.term_id}`  ",
                f"Introduced {instant.isoformat().replace('+00:00', 'Z')} in {term.week}; "
                f"seen {term.occurrences} time(s).",
                "",
                "### Definition",
                "",
                _definition_text(term),
                "",
                "### First-use evidence",
                "",
                f"> {term.context}",
                "",
            ]
        )
    return "\n".join(lines).rstrip() + "\n"


__all__ = [
    "GlossaryTerm",
    "TermSource",
    "glossary_catalog_markdown",
    "glossary_markdown",
    "glossary_prompt_text",
    "glossary_term_id",
    "plain_language_context_text",
    "scan_terminology",
]
