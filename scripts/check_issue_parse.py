#!/usr/bin/env python3
"""Refuse a change that leaves an issue file the pinned `mb` cannot parse.

`mb` reads every file under `.minibeads/issues/` as one repository, and one
file with a top-level heading outside Description / Design / Acceptance
Criteria / Notes makes the whole repository unreadable. That has happened
three times (#898's top-level `# Resolution` blocked mb repo-wide for 16 h,
then #905/#917, then #1000/#1001/#932/#954), each time unnoticed until a human
ran `mb` by hand. This check runs mb's own reader over every issue file in
the gate: the fast path is one `mb list` on the repository, and when that
fails each file is parsed alone in a scratch directory so every bad file is
named with mb's own error line.

The reader is the repository's pinned minibeads (`ext/minibeads`, built with
cargo), never a `mb` found on PATH: the gate must test the parser the
repository pins, not whatever a host happens to have installed.

Usage:
    python3 scripts/check_issue_parse.py
    python3 scripts/check_issue_parse.py --self-test
"""

from __future__ import annotations

import argparse
import fcntl
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
PINNED = Path("ext/minibeads")
ISSUES = Path(".minibeads/issues")


def _run(argv: list[str], cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(argv, cwd=cwd, capture_output=True, text=True, check=False)


def _local_minibeads(root: Path, commit: str) -> Path | None:
    """A minibeads clone already on this host that holds the pinned commit.

    Every linked worktree of this repository keeps its own submodule clone
    under the common git directory, so a fresh worktree can check the
    submodule out from a sibling's clone with no network.
    """

    done = _run(["git", "rev-parse", "--path-format=absolute", "--git-common-dir"], cwd=root)
    if done.returncode != 0:
        return None
    common = Path(done.stdout.strip())
    candidates = [common / "modules" / PINNED]
    candidates += sorted((common / "worktrees").glob(f"*/modules/{PINNED}"))
    for candidate in candidates:
        has = _run(["git", "--git-dir", str(candidate), "cat-file", "-e", f"{commit}^{{commit}}"], cwd=root)
        if has.returncode == 0:
            return candidate
    return None


def _checkout_pinned(root: Path, source: Path) -> None:
    """Check out `ext/minibeads` without the network, or name the one fix."""

    pinned = _run(["git", "rev-parse", f"HEAD:{PINNED}"], cwd=root)
    commit = pinned.stdout.strip()
    local = _local_minibeads(root, commit) if pinned.returncode == 0 else None
    if local is not None:
        # Clone from a sibling worktree's submodule: a gate never needs the network.
        done = _run(
            ["git", "-c", "protocol.file.allow=always",
             "-c", f"submodule.{PINNED}.url={local}",
             "submodule", "update", "--init", str(PINNED)],
            cwd=root,
        )
        if done.returncode == 0 and (source / "Cargo.toml").is_file():
            return
        detail = done.stderr.strip()
    else:
        detail = f"no clone of minibeads on this host holds the pinned commit {commit or '?'}"
    raise SystemExit(
        f"check-issue-parse: cannot check out the pinned minibeads at {PINNED} "
        f"without the network ({detail}).\n"
        "  Initialize it once, outside the gate: "
        f"herdr-run run --cwd {root} \"git submodule update --init {PINNED}\""
    )


def ensure_mb(root: Path) -> Path:
    """The pinned `mb` binary, built from `ext/minibeads` (never PATH).

    The check and its self-test run as two gate steps at once, so the checkout
    and the build happen under one lock in the worktree's git directory.
    """

    source = root / PINNED
    git_dir = _run(["git", "rev-parse", "--path-format=absolute", "--git-dir"], cwd=root)
    lock_path = Path(git_dir.stdout.strip() or root) / "check-issue-parse.lock"
    with lock_path.open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if not (source / "Cargo.toml").is_file():
            _checkout_pinned(root, source)
        done = _run(["cargo", "build", "--release", "--bin", "mb"], cwd=source)
    binary = source / "target" / "release" / "mb"
    if done.returncode != 0 or not binary.is_file():
        raise SystemExit(
            "check-issue-parse: cannot build the pinned mb from "
            f"{PINNED}:\n{done.stderr.strip()}"
        )
    return binary


def _error_line(done: subprocess.CompletedProcess[str], name: str) -> str:
    lines = (done.stderr + done.stdout).splitlines()
    for line in lines:
        if name in line and line.strip():
            return line.strip()
    for line in lines:
        if line.strip().startswith("Error:"):
            return line.strip()
    return f"mb exited {done.returncode} without an error line"


def parse_failures(root: Path, mb: Path) -> list[str]:
    """One `file: mb error` line per issue file the pinned mb refuses."""

    if _run([str(mb), "list"], cwd=root).returncode == 0:
        return []
    issues = root / ISSUES
    if not issues.is_dir():
        return [f"{ISSUES}: mb refuses the repository but no issues directory exists"]
    failures: list[str] = []
    for path in sorted(issues.glob("*.md")):
        with tempfile.TemporaryDirectory() as raw:
            scratch = Path(raw) / ISSUES
            scratch.mkdir(parents=True)
            shutil.copyfile(path, scratch / path.name)
            done = _run([str(mb), "list"], cwd=Path(raw))
        if done.returncode != 0:
            failures.append(f"{ISSUES}/{path.name}: {_error_line(done, path.name)}")
    if not failures:
        failures.append(
            f"{ISSUES}: mb refuses the repository as a whole but every file "
            "parses alone; run `mb list` for its error"
        )
    return failures


def self_test() -> int:
    """Prove the checker can fail: a good file passes, a bad heading is named."""

    failures: list[str] = []

    def expect(condition: bool, why: str) -> None:
        if not condition:
            failures.append(why)

    mb = ensure_mb(REPO_ROOT)
    good = (
        "---\ntitle: 'good-slug: a fine issue'\nstatus: open\npriority: 2\n"
        "issue_type: task\ncreated_at: 2026-11-07T00:00:00+00:00\n"
        "updated_at: 2026-11-07T00:00:00+00:00\n---\n\n"
        "# Description\n\nFine.\n\n# Notes\n\nAlso fine.\n"
    )
    bad = good.replace("good-slug", "bad-slug").replace(
        "# Notes\n\nAlso fine.", "# Bogus heading\n\nNot a section mb knows."
    )
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        issues = root / ISSUES
        issues.mkdir(parents=True)
        (issues / "agent-utils-9001.md").write_text(good, encoding="utf-8")
        expect(
            parse_failures(root, mb) == [],
            "a parseable issue file passes",
        )
        (issues / "agent-utils-9002.md").write_text(bad, encoding="utf-8")
        hits = parse_failures(root, mb)
        expect(
            len(hits) == 1
            and "agent-utils-9002.md" in hits[0]
            and "Unknown Markdown section" in hits[0],
            f"a refused file is named with mb's error line, got {hits}",
        )

    for failure in failures:
        print(f"FAIL  {failure}", file=sys.stderr)
    if failures:
        print(f"\n{len(failures)} self-test failure(s)", file=sys.stderr)
        return 1
    print("check-issue-parse --self-test: PASSED")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--self-test", action="store_true", help="check the guard offline, then exit"
    )
    args = parser.parse_args(argv)

    if args.self_test:
        return self_test()

    hits = parse_failures(REPO_ROOT, ensure_mb(REPO_ROOT))
    if hits:
        print("check-issue-parse: FAIL", file=sys.stderr)
        for hit in hits:
            print(f"  {hit}", file=sys.stderr)
        print(
            "\n  The pinned mb cannot read every issue file, so `mb` is broken "
            "repo-wide.\n  Keep top-level headings to Description, Design, "
            "Acceptance Criteria and Notes;\n  use ## for a heading inside a "
            "section. See #951 mb-id-collision-guard.",
            file=sys.stderr,
        )
        return 1
    print("check-issue-parse: ok — the pinned mb parses every issue file")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
