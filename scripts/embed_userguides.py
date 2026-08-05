#!/usr/bin/env python3
"""Render and link standalone package documentation from shared sources.

Each tool keeps language-neutral prose in ``common/docs/<tool>/*.template.md``
and distribution-specific prose in ``common/docs/<tool>/fragments/<language>/``.
Rendered README and user-guide files live under ``common/docs``.  Package trees
link to those committed artifacts; package builders dereference the links so
installed wheels and crates remain self-contained.  Edit templates, fragments,
or a single-language source document, then run this script.

Usage:
  python3 scripts/embed_userguides.py
  python3 scripts/embed_userguides.py --check
"""

from __future__ import annotations

import argparse
import os
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
                Render(
                    tool,
                    "README",
                    "python",
                    f"common/docs/{tool}/rendered/python/README.md",
                ),
                Render(
                    tool,
                    "USER_GUIDE",
                    "python",
                    f"common/docs/{tool}/rendered/python/USER_GUIDE.md",
                ),
                Render(
                    tool,
                    "README",
                    "rust",
                    f"common/docs/{tool}/rendered/rust/README.md",
                ),
                Render(
                    tool,
                    "USER_GUIDE",
                    "rust",
                    f"common/docs/{tool}/rendered/rust/USER_GUIDE.md",
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
    (
        "development-history language",
        re.compile(
            r"\b(?:prototype|roadmap|formerly|previously|planned|not yet|legacy|historical|"
            r"predates?|follow-on|stub)\b",
            re.IGNORECASE,
        ),
    ),
    (
        "implementation-provenance language",
        re.compile(
            r"\b(?:ported|parity|cross-language|both builds?)\b|"
            r"\b(?:direct|generic|typed)?\s*port of\b",
            re.IGNORECASE,
        ),
    ),
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
class StandaloneDocument:
    """One authoritative single-language document that must remain standalone."""

    tool: str
    document: str
    language: str
    source: str


@dataclass(frozen=True)
class PackageLink:
    """One repository symlink from a package tree to an authoritative file."""

    destination: str
    target: str

    @property
    def relative_target(self) -> str:
        return os.path.relpath(self.target, start=str(Path(self.destination).parent))


STANDALONE_DOCUMENTS: tuple[StandaloneDocument, ...] = (
    StandaloneDocument(
        tool="agent-team-timeline",
        document="README",
        language="python",
        source="common/docs/agent-team-timeline/README.md",
    ),
    StandaloneDocument(
        tool="agent-team-timeline",
        document="USER_GUIDE",
        language="python",
        source="common/docs/agent-team-timeline/USER_GUIDE.md",
    ),
)


def _package_links() -> tuple[PackageLink, ...]:
    links: list[PackageLink] = []
    for tool, py_package, rust_crate in TOOLS:
        links.extend(
            (
                PackageLink(
                    f"py/{py_package}/README.md",
                    f"common/docs/{tool}/rendered/python/README.md",
                ),
                PackageLink(
                    f"py/{py_package}/USER_GUIDE.md",
                    f"common/docs/{tool}/rendered/python/USER_GUIDE.md",
                ),
                PackageLink(
                    f"rs/{rust_crate}/README.md",
                    f"common/docs/{tool}/rendered/rust/README.md",
                ),
                PackageLink(
                    f"rs/{rust_crate}/src/embedded_userguide.md",
                    f"common/docs/{tool}/rendered/rust/USER_GUIDE.md",
                ),
                PackageLink(f"py/{py_package}/LICENSE", "LICENSE"),
                PackageLink(f"rs/{rust_crate}/LICENSE", "LICENSE"),
            )
        )
    links.extend(
        (
            PackageLink(
                "py/agent_team_timeline/README.md",
                "common/docs/agent-team-timeline/README.md",
            ),
            PackageLink(
                "py/agent_team_timeline/USER_GUIDE.md",
                "common/docs/agent-team-timeline/USER_GUIDE.md",
            ),
            PackageLink("py/agent_team_timeline/LICENSE", "LICENSE"),
        )
    )
    return tuple(links)


PACKAGE_LINKS = _package_links()


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


def _lint(item: Render | StandaloneDocument, text: str) -> list[str]:
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


def _standalone_expected() -> tuple[tuple[StandaloneDocument, str], ...]:
    expected: list[tuple[StandaloneDocument, str]] = []
    for item in STANDALONE_DOCUMENTS:
        source = REPO_ROOT / item.source
        if not source.is_file():
            raise FileNotFoundError(f"documentation source missing: {item.source}")
        text = source.read_text(encoding="utf-8")
        errors = _lint(item, text)
        if errors:
            details = "\n".join(f"  - {error}" for error in errors)
            raise ValueError(f"{item.source} is not standalone:\n{details}")
        expected.append((item, text))
    return tuple(expected)


def _replace_with_link(item: PackageLink) -> None:
    destination = REPO_ROOT / item.destination
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.is_symlink() and os.readlink(destination) == item.relative_target:
        return
    if destination.exists() or destination.is_symlink():
        if destination.is_dir() and not destination.is_symlink():
            raise IsADirectoryError(f"package link destination is a directory: {item.destination}")
        destination.unlink()
    destination.symlink_to(item.relative_target)


def _link_is_current(item: PackageLink) -> bool:
    destination = REPO_ROOT / item.destination
    target = REPO_ROOT / item.target
    return (
        destination.is_symlink()
        and os.readlink(destination) == item.relative_target
        and target.is_file()
        and destination.resolve() == target.resolve()
    )


def generate() -> list[str]:
    """Render every package document and return the written paths."""

    rendered = _expected()
    _standalone_expected()
    written: list[str] = []
    for render, text in rendered:
        destination = REPO_ROOT / render.destination
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(text, encoding="utf-8")
        written.append(render.destination)
    for link in PACKAGE_LINKS:
        _replace_with_link(link)
        written.append(link.destination)
    return written


def check() -> tuple[list[str], list[str]]:
    """Return stale destinations and standalone-lint failures."""

    stale: list[str] = []
    lint_errors: list[str] = []
    for render, wanted in _expected():
        destination = REPO_ROOT / render.destination
        if not destination.is_file() or destination.is_symlink():
            stale.append(render.destination)
            continue
        actual = destination.read_text(encoding="utf-8")
        if actual != wanted:
            stale.append(render.destination)
        for error in _lint(render, actual):
            lint_errors.append(f"{render.destination}: {error}")
    _standalone_expected()
    for link in PACKAGE_LINKS:
        if not _link_is_current(link):
            stale.append(link.destination)
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
                f"embed_userguides: {len(RENDERS)} paired documents and "
                f"{len(STANDALONE_DOCUMENTS)} single-language documents are current, standalone, "
                f"and linked through {len(PACKAGE_LINKS)} package paths"
            )
            return 0

        written = generate()
    except (FileNotFoundError, OSError, ValueError) as error:
        print(f"embed_userguides: {error}", file=sys.stderr)
        return 1

    print(f"embed_userguides: refreshed {len(written)} rendered files and package links:")
    for path in written:
        print(f"  {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
