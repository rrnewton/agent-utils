"""Tests for stable, evidence-backed timeline glossary records."""

from __future__ import annotations

from agent_team_timeline.terminology import (
    GlossaryTerm,
    TermSource,
    glossary_catalog_markdown,
    glossary_term_id,
    plain_language_context_text,
    scan_terminology,
)


def test_term_ids_are_stable_url_safe_and_collision_resistant() -> None:
    first = glossary_term_id("exact-head")

    assert first == glossary_term_id("exact-head")
    assert first.startswith("term-exact-head-")
    assert first != glossary_term_id("exact head")
    assert first != glossary_term_id("EXACT-HEAD")
    assert all(character.islower() or character.isdigit() or character == "-" for character in first)


def test_scanned_terms_and_catalog_carry_the_same_stable_ids() -> None:
    terms = scan_terminology(
        (
            TermSource(1_775_000_000_000, "Use `exact-head` validation before release."),
            TermSource(1_775_000_001_000, "The exact-head check protects the release."),
        ),
        "America/New_York",
    )

    exact = next(term for term in terms if term.term == "exact-head")
    catalog = glossary_catalog_markdown("codex-test", terms)
    assert exact.term_id == glossary_term_id(exact.term)
    assert exact.term_id in catalog
    assert "# codex-test project glossary" in catalog
    assert "## Project overview" in catalog
    assert "### Definition" in catalog
    assert "### First-use evidence" in catalog


def test_plain_language_context_excludes_unsupported_definitions() -> None:
    supported = GlossaryTerm(
        term="exact-head",
        introduced_at_ms=1,
        occurrences=2,
        context="The exact-head check binds a release to one revision.",
        week="2026-W31",
        term_id=glossary_term_id("exact-head"),
        definition="A release check that requires the tested and landed revision to match.",
        definition_status="supported",
    )
    unsupported = GlossaryTerm(
        term="DBI",
        introduced_at_ms=2,
        occurrences=2,
        context="DBI remained blocked.",
        week="2026-W31",
        term_id=glossary_term_id("DBI"),
        definition="Insufficient evidence: the source never expands DBI.",
        definition_status="insufficient-evidence",
    )

    context = plain_language_context_text(
        "Hermit runs guest software deterministically.", (supported, unsupported)
    )

    assert "Hermit runs guest software deterministically" in context
    assert "exact-head" in context
    assert "DBI" not in context
