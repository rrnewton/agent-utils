#!/usr/bin/env python3
"""Render standalone package documentation from shared templates.

Each tool keeps language-neutral prose in ``common/docs/<tool>/*.template.md``
and distribution-specific prose in ``common/docs/<tool>/fragments/<language>/``.
The rendered README and user guide live inside each distributable package so
they survive installation.  Generated files are committed artifacts; edit the
templates or fragments, then run this script.

Usage:
  python3 scripts/embed_userguides.py
  python3 scripts/embed_userguides.py --check
"""

from __future__ import annotations

import argparse
import re
import sys
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
PLACEHOLDER = "{{DISTRIBUTION}}"
UNEXPANDED_TEMPLATE = re.compile(r"\{\{|\}\}")

TOOLS: tuple[tuple[str, str, str], ...] = (
    ("safe-ci-dag-runner", "safe_ci_dag_runner", "safe-ci-dag-runner"),
    ("tick-hub", "tick_hub", "tick-hub"),
    ("pr-landing-planner", "pr_landing_planner", "pr-landing-planner"),
)


@dataclass(frozen=True)
class Render:
    """One template/fragment pair and its rendered destination."""

    tool: str
    document: str
    language: str
    destination: str

    @property
    def template(self) -> str:
        return f"common/docs/{self.tool}/{self.document}.template.md"

    @property
    def fragment(self) -> str:
        return f"common/docs/{self.tool}/fragments/{self.language}/{self.document}.md"


def _renders() -> tuple[Render, ...]:
    rendered: list[Render] = []
    for tool, py_package, rust_crate in TOOLS:
        rendered.extend(
            (
                Render(tool, "README", "python", f"py/{py_package}/README.md"),
                Render(tool, "USER_GUIDE", "python", f"py/{py_package}/USER_GUIDE.md"),
                Render(tool, "README", "rust", f"rs/{rust_crate}/README.md"),
                Render(
                    tool,
                    "USER_GUIDE",
                    "rust",
                    f"rs/{rust_crate}/src/embedded_userguide.md",
                ),
            )
        )
    return tuple(rendered)


RENDERS = _renders()

# These checks apply to installed-package documentation, not the suite README.
# Platform dependencies such as Git, GitHub, Graphviz, and systemd are valid to
# document; names of this source suite and unrelated products are not.
COMMON_FORBIDDEN: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("suite name", re.compile(r"agent-utils", re.IGNORECASE)),
    ("unrelated project", re.compile(r"\b(?:DeepScry|Hermit)\b", re.IGNORECASE)),
    ("source-tree docs path", re.compile(r"common/docs/", re.IGNORECASE)),
    ("source-tree script", re.compile(r"scripts/embed_userguides\.py", re.IGNORECASE)),
    ("internal incident identifier", re.compile(r"\bds-[a-z0-9]+\b", re.IGNORECASE)),
)

LANGUAGE_FORBIDDEN: dict[str, tuple[tuple[str, re.Pattern[str]], ...]] = {
    "python": (
        ("other implementation language", re.compile(r"\bRust\b", re.IGNORECASE)),
        (
            "other toolchain",
            re.compile(r"\b(?:Cargo|rustc|rustup)\b|crates\.io", re.IGNORECASE),
        ),
        ("other source-tree path", re.compile(r"(?:^|[ (`])rs/", re.MULTILINE)),
    ),
    "rust": (
        (
            "other implementation language",
            re.compile(r"\bpython(?:[0-9]+(?:\.[0-9]+)*)?\b", re.IGNORECASE),
        ),
        ("other package manager", re.compile(r"\bpip\b|PyPI", re.IGNORECASE)),
        ("other source-tree path", re.compile(r"(?:^|[ (`])py/", re.MULTILINE)),
    ),
}


@dataclass(frozen=True)
class DirectCopy:
    """One prototype guide copied verbatim outside the paired-doc contract."""

    source: str
    destination: str


# agent-team-timeline is being developed concurrently and is deliberately not
# yet part of the Python/Rust template and standalone-package contract. Keep its
# existing single-source guide synchronized without pretending it is paired.
DIRECT_COPIES: tuple[DirectCopy, ...] = (
    DirectCopy(
        source="common/docs/agent-team-timeline/USER_GUIDE.md",
        destination="py/agent_team_timeline/USER_GUIDE.md",
    ),
)


def _read(relative: str) -> str:
    return (REPO_ROOT / relative).read_text(encoding="utf-8")


def _render(item: Render) -> str:
    template_path = REPO_ROOT / item.template
    fragment_path = REPO_ROOT / item.fragment
    if not template_path.is_file():
        raise FileNotFoundError(f"documentation template missing: {item.template}")
    if not fragment_path.is_file():
        raise FileNotFoundError(f"documentation fragment missing: {item.fragment}")

    template = _read(item.template)
    count = template.count(PLACEHOLDER)
    if count != 1:
        raise ValueError(f"{item.template}: expected exactly one {PLACEHOLDER}, found {count}")
    fragment = _read(item.fragment).strip()
    return template.replace(PLACEHOLDER, fragment).rstrip() + "\n"


def _lint(item: Render, text: str) -> list[str]:
    errors: list[str] = []
    template_match = UNEXPANDED_TEMPLATE.search(text)
    if template_match is not None:
        line = text.count("\n", 0, template_match.start()) + 1
        errors.append(f"unexpanded template syntax at line {line}")
    for description, pattern in COMMON_FORBIDDEN + LANGUAGE_FORBIDDEN[item.language]:
        match = pattern.search(text)
        if match is not None:
            line = text.count("\n", 0, match.start()) + 1
            errors.append(f"{description} at line {line}: {match.group(0)!r}")
    for tool, py_package, rust_crate in TOOLS:
        if tool == item.tool:
            continue
        sibling = re.compile(
            rf"(?:{re.escape(tool)}|{re.escape(py_package)}|{re.escape(rust_crate)})",
            re.IGNORECASE,
        )
        match = sibling.search(text)
        if match is not None:
            line = text.count("\n", 0, match.start()) + 1
            errors.append(f"sibling package at line {line}: {match.group(0)!r}")
    return errors


def _expected() -> tuple[tuple[Render, str], ...]:
    expected: list[tuple[Render, str]] = []
    for item in RENDERS:
        text = _render(item)
        errors = _lint(item, text)
        if errors:
            details = "\n".join(f"  - {error}" for error in errors)
            raise ValueError(f"{item.template} + {item.fragment} is not standalone:\n{details}")
        expected.append((item, text))
    return tuple(expected)


def _direct_expected() -> tuple[tuple[DirectCopy, str], ...]:
    expected: list[tuple[DirectCopy, str]] = []
    for item in DIRECT_COPIES:
        source = REPO_ROOT / item.source
        if not source.is_file():
            raise FileNotFoundError(f"documentation source missing: {item.source}")
        expected.append((item, source.read_text(encoding="utf-8")))
    return tuple(expected)


def generate() -> list[str]:
    """Render every package document and return the written paths."""

    rendered = _expected()
    copied = _direct_expected()
    written: list[str] = []
    for item, text in rendered:
        destination = REPO_ROOT / item.destination
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(text, encoding="utf-8")
        written.append(item.destination)
    for copy, text in copied:
        destination = REPO_ROOT / copy.destination
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(text, encoding="utf-8")
        written.append(copy.destination)
    return written


def check() -> tuple[list[str], list[str]]:
    """Return stale destinations and standalone-lint failures."""

    stale: list[str] = []
    lint_errors: list[str] = []
    for item, wanted in _expected():
        destination = REPO_ROOT / item.destination
        if not destination.is_file():
            stale.append(item.destination)
            continue
        actual = destination.read_text(encoding="utf-8")
        if actual != wanted:
            stale.append(item.destination)
        for error in _lint(item, actual):
            lint_errors.append(f"{item.destination}: {error}")
    for copy, wanted in _direct_expected():
        destination = REPO_ROOT / copy.destination
        if not destination.is_file() or destination.read_text(encoding="utf-8") != wanted:
            stale.append(copy.destination)
    return stale, lint_errors


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Render standalone package READMEs and user guides from shared templates."
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify every generated document and standalone-language rule; do not write",
    )
    args = parser.parse_args(list(argv) if argv is not None else None)

    try:
        if bool(args.check):
            stale, lint_errors = check()
            if stale or lint_errors:
                if stale:
                    print("embed_userguides: stale or missing generated document(s):", file=sys.stderr)
                    for path in stale:
                        print(f"  {path}", file=sys.stderr)
                if lint_errors:
                    print("embed_userguides: standalone-document violation(s):", file=sys.stderr)
                    for error in lint_errors:
                        print(f"  {error}", file=sys.stderr)
                return 1
            print(
                f"embed_userguides: {len(RENDERS)} paired package documents are current and "
                f"standalone; {len(DIRECT_COPIES)} prototype guide is current"
            )
            return 0

        written = generate()
    except (FileNotFoundError, OSError, ValueError) as error:
        print(f"embed_userguides: {error}", file=sys.stderr)
        return 1

    print(f"embed_userguides: rendered {len(written)} document(s):")
    for path in written:
        print(f"  {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
