#!/usr/bin/env python3
"""Embed each tool's user guide INTO its package/crate so it survives distribution.

Single source of truth: ``common/docs/<tool>/USER_GUIDE.md``. Neither ``pip install`` nor
``cargo install`` (nor a crates.io / PyPI publish) ships ``common/docs/``, so a symlink into the
source tree does NOT survive packaging. This script COPIES each guide into a location INSIDE the
distributable unit:

* Python: ``py/<pkg>/USER_GUIDE.md`` (declared as ``package-data`` in ``pyproject.toml`` and read at
  runtime via ``importlib.resources`` — a real package resource, present in the wheel/sdist).
* Rust:   ``rs/<crate>/src/embedded_userguide.md`` (baked in with ``include_str!``; kept UNDER
  ``src/`` so the ``include_str!`` target is packaged by ``cargo package`` / crates.io).

The embedded copies are DERIVED artifacts. They are committed to the repo (so a fresh checkout and
CI — which build the crate / import the package directly, without running ``./setup`` first — have
them present), but ``common/docs/<tool>/USER_GUIDE.md`` remains the ONE editable source. ``./setup``
regenerates them on every build; ``--check`` verifies they are in sync (used by CI to fail loudly if
someone edited a source guide without regenerating).

Usage:
  scripts/embed_userguides.py            # (re)generate every embedded copy from its source
  scripts/embed_userguides.py --check    # verify copies match their source; exit 1 if stale
"""

from __future__ import annotations

import argparse
import os
import sys
from collections.abc import Sequence
from dataclasses import dataclass

#: Repo root = the parent of this script's ``scripts/`` directory (works from any CWD).
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


@dataclass(frozen=True)
class Embed:
    """One tool's guide: its single SOURCE path and every embedded DESTINATION (all repo-relative)."""

    tool: str
    source: str
    destinations: tuple[str, ...]


#: The embed map. tick-hub has no Rust port yet, so it lists only the Python destination.
EMBEDS: tuple[Embed, ...] = (
    Embed(
        tool="safe-ci-dag-runner",
        source="common/docs/safe-ci-dag-runner/USER_GUIDE.md",
        destinations=(
            "py/safe_ci_dag_runner/USER_GUIDE.md",
            "rs/safe-ci-dag-runner/src/embedded_userguide.md",
        ),
    ),
    Embed(
        tool="tick-hub",
        source="common/docs/tick-hub/USER_GUIDE.md",
        destinations=("py/tick_hub/USER_GUIDE.md",),
    ),
)

_GENERATED_BANNER = (
    "<!-- GENERATED FILE — DO NOT EDIT. Copied by scripts/embed_userguides.py from {source}. "
    "Edit that single source, then run ./setup (or scripts/embed_userguides.py) to regenerate. -->\n"
)


def _read(path: str) -> str:
    with open(path, encoding="utf-8") as handle:
        return handle.read()


def _embedded_text(source_rel: str) -> str:
    """The exact bytes to embed: the source guide, VERBATIM.

    The text is embedded verbatim (no banner injected) so the ``--userguide`` output is
    byte-identical to the source guide AND byte-identical across the Python and Rust builds (both
    embed the same source). The 'do not edit' warning lives in this script + the README, not in the
    payload, to keep the emitted guide clean.
    """
    return _read(os.path.join(REPO_ROOT, source_rel))


def generate() -> list[str]:
    """(Re)write every embedded copy from its source. Returns the list of destinations written."""
    written: list[str] = []
    for embed in EMBEDS:
        source_abs = os.path.join(REPO_ROOT, embed.source)
        if not os.path.isfile(source_abs):
            raise FileNotFoundError(f"embed source missing: {embed.source}")
        text = _embedded_text(embed.source)
        for dest_rel in embed.destinations:
            dest_abs = os.path.join(REPO_ROOT, dest_rel)
            os.makedirs(os.path.dirname(dest_abs), exist_ok=True)
            with open(dest_abs, "w", encoding="utf-8") as handle:
                handle.write(text)
            written.append(dest_rel)
    return written


def check() -> list[str]:
    """Return the list of embedded copies that are MISSING or STALE relative to their source."""
    stale: list[str] = []
    for embed in EMBEDS:
        source_abs = os.path.join(REPO_ROOT, embed.source)
        if not os.path.isfile(source_abs):
            raise FileNotFoundError(f"embed source missing: {embed.source}")
        want = _embedded_text(embed.source)
        for dest_rel in embed.destinations:
            dest_abs = os.path.join(REPO_ROOT, dest_rel)
            if not os.path.isfile(dest_abs) or _read(dest_abs) != want:
                stale.append(dest_rel)
    return stale


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Embed each tool's USER_GUIDE.md into its package/crate.")
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify embedded copies match their source (exit 1 if any is stale); do not write",
    )
    ns = parser.parse_args(list(argv) if argv is not None else None)

    if bool(ns.check):
        stale = check()
        if stale:
            print("embed_userguides: STALE embedded guide(s) — run ./setup to regenerate:", file=sys.stderr)
            for path in stale:
                print(f"  {path}", file=sys.stderr)
            return 1
        print("embed_userguides: all embedded guides are in sync with their source")
        return 0

    written = generate()
    print(f"embed_userguides: wrote {len(written)} embedded guide(s):")
    for path in written:
        print(f"  {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
