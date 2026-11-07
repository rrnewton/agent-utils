#!/usr/bin/env python3
"""Refuse a change that files an issue under a number main already owns.

Lanes file issues from stale views of main, and each takes the next free
number it can see. Twice in one day that gave two different issues the same
number: the second landing then either failed late or silently replaced the
first issue's file. This check compares the issue files a change touches
against the target main (the first of `github/main`, `origin/main` that
resolves, or `--base`): filing issue N under a slug main does not already
have for N is refused, naming both slugs; editing issue N under its existing
slug passes. A change that fields two files for the same new number is
refused the same way.

`mb create` cannot reserve numbers atomically today (it takes the highest
number in the local checkout); that belongs to the minibeads tool and is
tracked as a follow-up (#951's notes). This landing guard is the half that
needs no shared service: it runs in every gate, cheaply, before anything
lands.

Usage:
    python3 scripts/check_issue_numbers.py [--base REF]
    python3 scripts/check_issue_numbers.py --self-test
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
ISSUES = ".minibeads/issues"

#: `.minibeads/issues/<prefix>-<N>.md` with a purely numeric N. The hex-suffixed
#: legacy files (agent-utils-f0a72c.md) are not in the numeric space.
ISSUE_FILE = re.compile(r"^\.minibeads/issues/.+-(\d+)\.md$")
SLUG = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
TITLE = re.compile(r"^title:\s*'(.*)'\s*$|^title:\s*\"(.*)\"\s*$|^title:\s*(\S.*)$")
BASE_CANDIDATES = ("github/main", "origin/main")


def slug_of(text: str) -> str | None:
    """The issue's slug: the `<slug>:` prefix of its title, or None."""

    for line in text.splitlines()[:20]:
        match = TITLE.match(line.strip()) if line.startswith("title:") else None
        if match is None:
            continue
        title = next(group for group in match.groups() if group is not None)
        candidate, colon, _rest = title.partition(":")
        if colon and SLUG.match(candidate):
            return candidate
        return None
    return None


def violations(
    changed: dict[str, str | None],
    base_files: dict[int, tuple[str, str | None]],
) -> list[str]:
    """Collision lines for one change.

    `changed` maps each touched issue path to its new slug (None when the file
    is gone or its slug does not parse); `base_files` maps every issue number
    on the target main to `(path, slug)`.
    """

    hits: list[str] = []
    by_number: dict[int, list[tuple[str, str | None]]] = {}
    for path, slug in sorted(changed.items()):
        match = ISSUE_FILE.match(path)
        if match is None:
            continue
        by_number.setdefault(int(match.group(1)), []).append((path, slug))
    for number, present in sorted(by_number.items()):
        base = base_files.get(number)
        if base is not None:
            base_path, base_slug = base
            for path, slug in present:
                if slug != base_slug:
                    hits.append(
                        f"{path}: files issue #{number} as {slug!r}, but the target "
                        f"main already files #{number} as {base_slug!r} in {base_path}; "
                        "renumber this issue, or edit the existing one under its slug"
                    )
        slugs = {slug for _path, slug in present}
        if len(present) > 1 and len(slugs) > 1:
            named = ", ".join(f"{path} as {slug!r}" for path, slug in present)
            hits.append(f"one change files issue #{number} twice: {named}")
    return hits


def _git(*argv: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    done = subprocess.run(
        ("git", *argv), cwd=REPO_ROOT, capture_output=True, text=True, check=False
    )
    if check and done.returncode != 0:
        raise SystemExit(
            f"check-issue-numbers: `git {' '.join(argv)}` failed:\n{done.stderr.strip()}"
        )
    return done


def resolve_base(explicit: str | None) -> str:
    if explicit:
        if _git("rev-parse", "--verify", explicit, check=False).returncode == 0:
            return explicit
        raise SystemExit(f"check-issue-numbers: base ref {explicit!r} does not resolve")
    for candidate in BASE_CANDIDATES:
        if _git("rev-parse", "--verify", candidate, check=False).returncode == 0:
            return candidate
    raise SystemExit(
        "check-issue-numbers: no base ref resolves (tried "
        + ", ".join(BASE_CANDIDATES)
        + "); pass --base REF so collisions are judged against a real target main"
    )


def base_issue_files(
    base: str, numbers: set[int] | None = None
) -> dict[int, tuple[str, str | None]]:
    """Issue files on `base`, optionally only the `numbers` the change touches.

    One `git show` per file cost ~830 forks and the gate's inner memory cap:
    read every wanted blob through one `git cat-file --batch` process.
    """

    listing = _git("ls-tree", "-r", "--name-only", base, "--", ISSUES).stdout
    wanted: list[tuple[int, str]] = []
    for path in listing.splitlines():
        match = ISSUE_FILE.match(path)
        if match is None:
            continue
        number = int(match.group(1))
        if numbers is not None and number not in numbers:
            continue
        wanted.append((number, path))
    if not wanted:
        return {}
    proc = subprocess.Popen(
        ("git", "cat-file", "--batch"),
        cwd=REPO_ROOT,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
    )
    assert proc.stdin is not None and proc.stdout is not None
    out: dict[int, tuple[str, str | None]] = {}
    for number, path in wanted:
        proc.stdin.write(f"{base}:{path}\n".encode())
        proc.stdin.flush()
        header = proc.stdout.readline().decode("utf-8", "replace").split()
        if len(header) < 3 or header[1] != "blob":
            raise SystemExit(f"check-issue-numbers: cannot read {base}:{path}")
        size = int(header[2])
        text = proc.stdout.read(size).decode("utf-8", "replace")
        proc.stdout.read(1)  # the newline cat-file appends after each object
        out[number] = (path, slug_of(text))
    proc.stdin.close()
    proc.wait()
    return out


def changed_issue_files(base: str) -> dict[str, str | None]:
    merge_base = _git("merge-base", base, "HEAD").stdout.strip() or base
    changed: dict[str, str | None] = {}
    for argv in (
        ("diff", "--no-renames", "--name-only", f"{merge_base}...HEAD"),
        ("diff", "--no-renames", "--name-only", "HEAD"),
    ):
        for path in _git(*argv).stdout.splitlines():
            if ISSUE_FILE.match(path):
                changed.setdefault(path, "")
    for path in _git("ls-files", "--others", "--exclude-standard").stdout.splitlines():
        if ISSUE_FILE.match(path):
            changed.setdefault(path, "")
    out: dict[str, str | None] = {}
    for path in changed:
        local = REPO_ROOT / path
        # A deletion (or the old side of an in-change rename) claims no number:
        # it leaves the number free, it does not file under it.
        if local.is_file():
            out[path] = slug_of(local.read_text(encoding="utf-8"))
    return out


def self_test() -> int:
    """Check the guard itself: pure slug/violation cases plus one temp git repo."""

    failures: list[str] = []

    def expect(condition: bool, why: str) -> None:
        if not condition:
            failures.append(why)

    expect(slug_of("title: 'my-slug: a thing'\n") == "my-slug", "slug parses from a quoted title")
    expect(slug_of("title: 'two words: a thing'\n") is None, "a non-slug title has no slug")
    expect(slug_of("no title here\n") is None, "a missing title has no slug")

    base: dict[int, tuple[str, str | None]] = {
        951: (".minibeads/issues/agent-utils-951.md", "slug-b")
    }
    added: dict[str, str | None] = {".minibeads/issues/agent-utils-951.md": "slug-a"}
    hits = violations(added, base)
    expect(len(hits) == 1 and "'slug-a'" in hits[0] and "'slug-b'" in hits[0],
           f"adding N under a new slug is refused naming both slugs, got {hits}")
    expect(violations({".minibeads/issues/agent-utils-951.md": "slug-b"}, base) == [],
           "editing N under its existing slug passes")
    expect(violations({".minibeads/issues/agent-utils-952.md": "slug-a"}, base) == [],
           "a fresh number passes")
    both: dict[str, str | None] = {
        ".minibeads/issues/agent-utils-960.md": "one-slug",
        ".minibeads/issues/other-960.md": "two-slugs",
    }
    expect(len(violations(both, base)) == 1, "one change filing N twice under two slugs is refused")
    expect(len(violations({".minibeads/issues/agent-utils-951.md": None}, base)) == 1,
           "an unparseable slug for an owned number is refused")

    import tempfile

    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        env_note = "temp repo"
        del env_note

        def git(*argv: str) -> subprocess.CompletedProcess[str]:
            return subprocess.run(
                ("git", *argv), cwd=root, capture_output=True, text=True, check=True
            )

        git("init", "-q", "-b", "main")
        git("config", "user.email", "check@example.invalid")
        git("config", "user.name", "check")
        issues = root / ".minibeads" / "issues"
        issues.mkdir(parents=True)
        (issues / "agent-utils-951.md").write_text(
            "---\ntitle: 'slug-b: the first filing'\n---\n", encoding="utf-8"
        )
        git("add", ".")
        git("commit", "-q", "-m", "base")
        git("switch", "-q", "-c", "lane")
        (issues / "agent-utils-951.md").write_text(
            "---\ntitle: 'slug-a: a different issue'\n---\n", encoding="utf-8"
        )
        git("add", ".")
        git("commit", "-q", "-m", "colliding lane")

        global REPO_ROOT
        saved = REPO_ROOT
        try:
            REPO_ROOT = root
            files = base_issue_files("main")
            expect(files.get(951, (None, None))[1] == "slug-b", f"base slug read, got {files}")
            changed = changed_issue_files("main")
            expect(changed.get(".minibeads/issues/agent-utils-951.md") == "slug-a",
                   f"changed slug read, got {changed}")
            expect(len(violations(changed, files)) == 1, "temp-repo collision is refused")
            (issues / "agent-utils-951.md").write_text(
                "---\ntitle: 'slug-b: edited under its slug'\n---\n", encoding="utf-8"
            )
            changed = changed_issue_files("main")
            expect(violations(changed, files) == [], "temp-repo same-slug edit passes")
        finally:
            REPO_ROOT = saved

    for failure in failures:
        print(f"FAIL  {failure}", file=sys.stderr)
    if failures:
        print(f"\n{len(failures)} self-test failure(s)", file=sys.stderr)
        return 1
    print("check-issue-numbers --self-test: PASSED")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--base", default=None,
                        help="target main ref (default: first of github/main, origin/main)")
    parser.add_argument("--self-test", action="store_true", help="check the guard offline, then exit")
    args = parser.parse_args(argv)

    if args.self_test:
        return self_test()

    base = resolve_base(args.base)
    changed = changed_issue_files(base)
    numbers = {
        int(match.group(1))
        for path in changed
        if (match := ISSUE_FILE.match(path)) is not None
    }
    hits = violations(changed, base_issue_files(base, numbers))
    if hits:
        print("check-issue-numbers: FAIL", file=sys.stderr)
        for hit in hits:
            print(f"  {hit}", file=sys.stderr)
        print(
            "\n  Two lanes took the same issue number from stale views of main. Renumber the\n"
            "  new issue (and its slug references), or edit the existing issue under the slug\n"
            "  main already has for that number. See #951 mb-id-collision-guard.",
            file=sys.stderr,
        )
        return 1
    print(f"check-issue-numbers: ok — no issue number collides with {base}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
