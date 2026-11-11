#!/usr/bin/env python3
"""List every direct scroll write outside the loop's scroll owner, in report mode by default.

`#1251 scroll-single-owner` (part of `#1210 ui-stability-no-async-shift`). Today several page
writers set the scroll position on their own, and the browser's native scroll anchoring corrects
as well, so one frame can carry competing corrections and the reader sees the row move. The
target is one owner of scroll decisions: the loop owner in `web/src/view/channel/scrollkeep.ts`.
This check makes that adoption auditable path by path: it scans the page sources for direct
`scrollTop` assignments and `scrollTo` / `scrollBy` / `scrollIntoView` calls outside the owner,
groups them by adoption path, and prints them.

Modes:

  report (default): always exits 0 and prints the full inventory. It fails nothing yet.
  enforce: `python3 tests/scroll_writer_guard.py --enforce P1 P9` exits 1 when a writer in an
  adopted path remains outside the owner and the allow-list. Each D4 path flips its own group to
  enforcing as it moves into the loop; unadopted paths stay report-only.

The owner file itself and the scroll-trace instrumentation (which wraps the prototypes to
observe writes, and never originates one) are excluded. Reads of `scrollTop` / `scrollHeight`
for measurement are not writes and are never listed. A stored record field that happens to be
named `scrollTop` (the documents reading chain) is listed with its reason and allow-listed, so
enforcing the documents path does not chase a plain object.

Usage:
    python3 vibe-talk/tests/scroll_writer_guard.py
    python3 vibe-talk/tests/scroll_writer_guard.py --enforce P1
    python3 vibe-talk/tests/scroll_writer_guard.py --self-test
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path

WEB_ROOT = Path(__file__).resolve().parents[1] / "web"
"""The page sources this guard scans: `web/` in this checkout."""

OWNER_FILES = frozenset({"web/src/view/channel/scrollkeep.ts"})
"""The loop's scroll owner: the one place a direct scroll write is the design, not a violation."""

INSTRUMENTATION_FILES = frozenset({"web/src/view/channel/scrolltrace.ts"})
"""Observation only: wraps `scrollTop` / `scrollTo` / `scrollBy` / `scrollIntoView` to record writes."""

# Adoption paths in landing order (see #1251). Files not listed here are reported as
# unclassified, which enforcing mode treats as a violation in every adopted run: a new writer
# cannot silently opt out.
PATH_BY_FILE: dict[str, str] = {
    "web/voice.js": "P8",
    "web/src/view/channel/listkeepers.ts": "P1",
    "web/src/view/channel/footroom.ts": "P1",
    "web/src/view/channel/cardplace.ts": "P2",
    "web/src/view/channel/filterplace.ts": "P2",
    "web/src/view/channel/threadentry.ts": "P2",
    "web/src/view/channel/channelviews.ts": "P2",
    "web/src/view/channel/gathering.ts": "P2",
    "web/src/view/channel/litcards.ts": "P2",
    "web/src/view/channel/threadcardhead.ts": "P2",
    "web/src/view/channel/replyarrows.ts": "P2",
    "web/src/view/channel/replysession.ts": "P2",
    "web/src/view/channel/readcore.ts": "P2",
    "web/src/view/shell/navigation.ts": "P2",
    "web/src/view/channel/folding.ts": "P3",
    "web/src/view/shell/foldkeys.ts": "P3",
    "web/src/view/documents/list.ts": "P4",
    "web/src/view/documents/index.ts": "P4",
    "web/src/view/documents/reader.ts": "P4",
    "web/src/view/shell/keyselection.ts": "P5",
    "web/src/view/shell/selectionkeys.ts": "P5",
    "web/src/view/shell/scrollkeys.ts": "P5",
    "web/src/view/shell/pointerselection.ts": "P5",
    "web/src/view/settings/help.ts": "P6",
    "web/src/view/settings/folds.ts": "P6",
    "web/src/view/settings/dismissed.ts": "P6",
    "web/src/view/dom/shell.ts": "P6",
    "web/src/view/voice/readingjump.ts": "P6",
    "web/src/view/voice/transcript.ts": "P6",
    "web/src/view/channel/maincards.ts": "P6",
    "web/src/view/channel/filterchips.ts": "P6",
    "web/src/view/channel/bookmark.ts": "P6",
    "web/src/view/composer/grow.ts": "P7",
    "web/src/view/composer/keyboard.ts": "P7",
    "web/src/view/channel/olderwalks.ts": "P7",
    "web/src/view/channel/channelrows.ts": "P9",
}

PATH_TITLES: dict[str, str] = {
    "P0": "owner (scrollkeep) — the one legal writer",
    "P1": "keepers / foot-follow (background)",
    "P2": "placement: cards, filter, thread entry",
    "P3": "collapse / fold (per-act anchor choice)",
    "P4": "documents list",
    "P5": "shell selection / keys (user-initiated, must route through the owner)",
    "P6": "voice transcript / misc user jumps (incl. the #1224 mention-jump shape)",
    "P7": "composer / keyboard geometry",
    "P8": "legacy script (voice.js, adopted last)",
    "P9": "reaction-overlay decide-before-paint (no direct writer; height via the owner)",
    "unclassified": "unclassified — new writer, must be grouped before enforcing",
}

# Writers that look like scroll writes but are not scroller writes, keyed by
# (file, line-text needle) with the reason. Enforcing mode permits exactly these.
ALLOW_LIST: tuple[tuple[str, str, str], ...] = (
    (
        "web/src/view/documents/index.ts",
        "current.scrollTop = scrollTopNow()",
        "a stored reading-chain record field, not a scroller write",
    ),
)

SCROLLTOP_WRITE = re.compile(r"\.scrollTop\s*(?:\+=|-=|=(?!=))")
SCROLL_CALLS: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("scrollTo", re.compile(r"\.scrollTo\s*\(")),
    ("scrollBy", re.compile(r"\.scrollBy\s*\(")),
    ("scrollIntoView", re.compile(r"\.scrollIntoView\s*\(")),
)


@dataclass(frozen=True)
class Writer:
    """One direct scroll write outside the owner: where it is and what it writes."""

    file: str
    line: int
    kind: str
    text: str

    @property
    def path(self) -> str:
        return PATH_BY_FILE.get(self.file, "unclassified")

    @property
    def allowed(self) -> str | None:
        for allowed_file, needle, reason in ALLOW_LIST:
            if self.file == allowed_file and needle in self.text:
                return reason
        return None


def _is_comment_only(line: str) -> bool:
    stripped = line.lstrip()
    return stripped.startswith("//") or stripped.startswith("*") or stripped.startswith("/*")


def scan_file(path: Path, rel: str) -> list[Writer]:
    """Every direct scroll write in one page source file, in line order."""
    writers: list[Writer] = []
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        if _is_comment_only(line):
            continue
        if SCROLLTOP_WRITE.search(line):
            writers.append(Writer(rel, number, "scrollTop-write", line.strip()))
            continue
        for kind, pattern in SCROLL_CALLS:
            if pattern.search(line):
                writers.append(Writer(rel, number, kind, line.strip()))
                break
    return writers


def scan() -> list[Writer]:
    """Every direct scroll write outside the owner and its instrumentation, sorted by file."""
    candidates = [WEB_ROOT / "voice.js", *sorted((WEB_ROOT / "src").rglob("*.ts"))]
    writers: list[Writer] = []
    for path in candidates:
        rel = path.relative_to(WEB_ROOT.parent).as_posix()
        if rel in OWNER_FILES or rel in INSTRUMENTATION_FILES:
            continue
        writers.extend(scan_file(path, rel))
    return writers


def report(writers: list[Writer]) -> str:
    """The full inventory, grouped by adoption path, as the report-mode output."""
    lines = [
        "scroll-writer guard (#1251 scroll-single-owner): writer inventory outside the loop owner.",
        f"owner: {', '.join(sorted(OWNER_FILES))} (excluded); "
        f"instrumentation excluded: {', '.join(sorted(INSTRUMENTATION_FILES))}.",
        f"allow-list ({len(ALLOW_LIST)}): "
        + "; ".join(f"{file}: {reason}" for file, _needle, reason in ALLOW_LIST),
        "",
    ]
    by_path: dict[str, list[Writer]] = {}
    for writer in writers:
        by_path.setdefault(writer.path, []).append(writer)
    for path in ["P1", "P2", "P3", "P4", "P5", "P6", "P7", "P8", "P9", "unclassified"]:
        group = by_path.get(path, [])
        lines.append(f"{path} {PATH_TITLES[path]}: {len(group)} writer(s)")
        for writer in group:
            allowed = f" [allow-listed: {writer.allowed}]" if writer.allowed is not None else ""
            lines.append(f"  {writer.file}:{writer.line} [{writer.kind}] {writer.text}{allowed}")
    lines.append(f"total: {len(writers)} direct writer(s) outside the owner.")
    return "\n".join(lines)


def violations(writers: list[Writer], adopted: list[str]) -> list[Writer]:
    """Writers that enforcing the `adopted` paths forbids: in an adopted path, not allow-listed."""
    return [w for w in writers if w.path in adopted and w.allowed is None]


def self_test() -> int:
    """The pattern contract: writes match, reads and comparisons never do."""
    cases: list[tuple[str, bool]] = [
        ("    area.scrollTop = area.scrollHeight;", True),
        ("    area.scrollTop += moved;", True),
        ("    area.scrollTop -= step;", True),
        ("    el(\"scroll-area\").scrollTop = top;", True),
        ("    area.scrollTo({ top: to });", True),
        ("    area.scrollBy({ top: px });", True),
        ("    row.scrollIntoView({ block: \"nearest\" });", True),
        ("    const top = area.scrollTop;", False),
        ("    if (area.scrollTop === 0) return;", False),
        ("    if (area.scrollTop == 0) return;", False),
        ("    if (area.scrollTop <= 1) return;", False),
        ("    if (area.scrollTop >= end - 1) return;", False),
        ("    return scroller.scrollTop + scroller.clientHeight;", False),
        ("// `scrollTop === 0` at touchstart was a state", False),
    ]
    failed = 0
    for text, expected in cases:
        found = bool(SCROLLTOP_WRITE.search(text)) or any(p.search(text) for _k, p in SCROLL_CALLS)
        # Comment-only lines are skipped by the scanner before patterns run.
        if _is_comment_only(text):
            found = False
        if found != expected:
            print(f"self-test FAIL: {text!r} matched={found}, expected {expected}")
            failed += 1
    if failed:
        print(f"scroll-writer guard self-test: {failed} case(s) failed")
        return 1
    print(f"scroll-writer guard self-test: {len(cases)} cases passed")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--enforce",
        nargs="*",
        metavar="PATH",
        help="adoption paths to enforce (e.g. P1 P9); without it the guard only reports",
    )
    parser.add_argument("--self-test", action="store_true", help="run the pattern contract and exit")
    args = parser.parse_args(argv)
    if args.self_test:
        return self_test()
    writers = scan()
    print(report(writers))
    adopted: list[str] = list(args.enforce or [])
    if not adopted:
        return 0
    bad = violations(writers, adopted)
    if bad:
        print(f"\nENFORCING {', '.join(adopted)}: {len(bad)} writer(s) must move into the owner:")
        for writer in bad:
            print(f"  {writer.file}:{writer.line} [{writer.kind}] {writer.text}")
        return 1
    print(f"\nENFORCING {', '.join(adopted)}: no writers outside the owner. OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
