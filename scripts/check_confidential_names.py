#!/usr/bin/env python3
"""Refuse confidential names in the public tree without naming them.

One confidential model codename reached this public tree in a prompt
glossary, two code comments and two issue files (#2068
public-tree-confidential-name). Removing it once is not enough: nothing
stopped it being typed, and the guard that stops it cannot list it either,
because this file is public too.

So the names live here only as salted SHA-256 digests of their lowercased
tokens. The scan splits every tracked file (plus untracked, unignored
files, for the same reason `check_client_names.py` does) into tokens on
non-alphanumerics and hashes each token the same way, so a bare use, a
capitalised use, and a hyphen-joined use (`muse-<name>-x`) all match,
while an ordinary word that merely contains the letters as part of a
longer token does not. Commit messages in the landing range are scanned
the same way when a base ref resolves: a message is not a file, and a
clean tree with a naming message would still publish the name.

An optional private names file outside the repository (one name per
line, `#` comments) adds digests in memory for local runs:
`--extra-names FILE` or `$AGENT_UTILS_CONFIDENTIAL_NAMES`. The file is
never printed and never committed.

Honest limit: a short name's digest is brute-forceable, salt or not. This
is a tripwire against accidents -- the way the codename arrived -- not
secrecy against a determined reader. Adding a name means computing its
digest locally (`digest_of` below) and pasting only the hex.

Usage:
    python3 scripts/check_confidential_names.py [--base REF]
    python3 scripts/check_confidential_names.py --self-test
"""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

#: Salt for the digests. Not a secret: it keeps the committed hex from
#: being a bare hash of a name that any precomputed table already holds.
SALT = "agent-utils-confidential-names-v1"

#: Salted SHA-256 digests of lowercased confidential tokens. The names
#: themselves never appear in this repository. One digest per name; a
#: hyphen-joined or capitalised use needs no extra entry, because tokens
#: are lowercased and split before hashing.
DIGESTS: frozenset[str] = frozenset(
    {
        "80db231bf6552a7786b1ccc395e1ef969dd11f40487637013325f27e9932baaf",
    }
)

TOKEN_SPLIT = re.compile(r"[^A-Za-z0-9]+")
BASE_CANDIDATES = ("github/main", "origin/main")
ENV_EXTRA_NAMES = "AGENT_UTILS_CONFIDENTIAL_NAMES"


def digest_of(token: str) -> str:
    """The committed form of one confidential token."""
    return hashlib.sha256(f"{SALT}:{token.lower()}".encode()).hexdigest()


def tokens_of(text: str) -> list[str]:
    """Lowercased alphanumeric tokens of `text`, in order."""
    return [token.lower() for token in TOKEN_SPLIT.split(text) if token]


def matched_digests(text: str, digests: frozenset[str]) -> set[str]:
    """Which confidential digests `text` contains, token by token."""
    return {digest_of(token) for token in set(tokens_of(text))} & digests


def file_hits(names: list[str], root: Path, digests: frozenset[str]) -> list[str]:
    """`path:line: matches confidential digest <prefix>` lines, naming nothing."""
    hits: list[str] = []
    for name in names:
        try:
            text = (root / name).read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            continue  # binary or unreadable: no token can hide in it as text
        for number, line in enumerate(text.splitlines(), start=1):
            for digest in sorted(matched_digests(line, digests)):
                hits.append(f"{name}:{number}: matches confidential digest {digest[:12]}")
    return hits


def message_hits(messages: list[tuple[str, str]], digests: frozenset[str]) -> list[str]:
    """`commit <sha>: message matches ...` lines, naming nothing."""
    hits: list[str] = []
    for sha, message in messages:
        for digest in sorted(matched_digests(message, digests)):
            hits.append(f"commit {sha[:12]}: message matches confidential digest {digest[:12]}")
    return hits


def extra_digests(path: Path | None) -> frozenset[str]:
    """Digests from a private names file (never printed), or none."""
    if path is None:
        return frozenset()
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise SystemExit(f"check-confidential-names: cannot read extra-names file: {error}")
    found = {
        digest_of(line.strip())
        for line in lines
        if line.strip() and not line.strip().startswith("#")
    }
    return frozenset(found)


def repo_files() -> list[str]:
    """Every tracked file, PLUS untracked files git is not ignoring.

    Untracked matters: this runs before a commit, and a brand-new file is
    the most likely place for a confidential name to arrive.
    """
    names: set[str] = set()
    for argv in (
        ("git", "ls-files", "-z"),
        ("git", "ls-files", "--others", "--exclude-standard", "-z"),
    ):
        done = subprocess.run(argv, cwd=REPO_ROOT, capture_output=True, text=True, check=False)
        if done.returncode != 0:
            raise SystemExit(
                f"check-confidential-names: `{' '.join(argv)}` failed:\n{done.stderr.strip()}"
            )
        names.update(name for name in done.stdout.split("\0") if name)
    return sorted(names)


def _git(*argv: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ("git", *argv), cwd=REPO_ROOT, capture_output=True, text=True, check=False
    )


def resolve_base(explicit: str | None) -> str | None:
    """The landing-range base, or None when no ref resolves (scan skipped)."""
    if explicit:
        return explicit if _git("rev-parse", "--verify", explicit).returncode == 0 else None
    for candidate in BASE_CANDIDATES:
        if _git("rev-parse", "--verify", candidate).returncode == 0:
            return candidate
    return None


def commit_messages(base: str) -> list[tuple[str, str]] | None:
    """`(sha, message)` for `merge-base(base, HEAD)..HEAD`, or None if unknowable."""
    merge_base = _git("merge-base", base, "HEAD")
    if merge_base.returncode != 0:
        return None
    start = merge_base.stdout.strip() or base
    listed = _git("rev-list", f"{start}..HEAD")
    if listed.returncode != 0:
        return None
    messages: list[tuple[str, str]] = []
    for sha in listed.stdout.splitlines():
        shown = _git("log", "-1", "--format=%B", sha)
        if shown.returncode != 0:
            return None
        messages.append((sha, shown.stdout))
    return messages


def self_test() -> int:
    """Check the guard itself with a SYNTHETIC name, never a real one."""
    import tempfile

    failures: list[str] = []

    def expect(condition: bool, why: str) -> None:
        if not condition:
            failures.append(why)

    synthetic = "syntheticname"
    digests = frozenset({digest_of(synthetic)})
    expect(bool(matched_digests(f"try {synthetic} here", digests)), "must catch a bare use")
    expect(bool(matched_digests(f"muse-{synthetic}-summarizer", digests)), "must catch a hyphen-joined use")
    expect(bool(matched_digests(synthetic.capitalize(), digests)), "must catch a capitalised use")
    expect(bool(matched_digests(synthetic.upper(), digests)), "must catch an upper-case use")
    expect(not matched_digests(f"{synthetic}x", digests), "must pass a longer token containing the letters")
    expect(not matched_digests("synthetic name", digests), "must pass the split words on their own")
    expect(not matched_digests("nothing to see", digests), "must pass clean text")
    expect(
        bool(message_hits([("a" * 40, f"all about {synthetic}")], digests)),
        "must catch the name in a commit message",
    )
    expect(
        not message_hits([("a" * 40, "a clean message")], digests),
        "must pass a clean commit message",
    )

    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        (root / "clean.md").write_text("nothing to see\n", encoding="utf-8")
        (root / "dirty.md").write_text(f"use muse-{synthetic}-1.0\n", encoding="utf-8")
        hits = file_hits(["clean.md", "dirty.md"], root, digests)
        expect(
            len(hits) == 1 and hits[0].startswith("dirty.md:1:"),
            f"one dirty file must be reported, got {hits}",
        )
        expect(synthetic not in " ".join(hits), "a hit must never print the name itself")
        extra = root / "private-names.txt"
        extra.write_text(f"# local only\n{synthetic}\n", encoding="utf-8")
        expect(extra_digests(extra) == digests, "a private names file must add its digests")
    expect(extra_digests(None) == frozenset(), "no private file means no extra digests")

    for failure in failures:
        print(f"FAIL  {failure}", file=sys.stderr)
    if failures:
        print(f"\n{len(failures)} self-test failure(s)", file=sys.stderr)
        return 1
    print("check-confidential-names --self-test: PASSED (11 controls)")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--self-test", action="store_true", help="check the guard offline, then exit")
    parser.add_argument(
        "--base",
        default=None,
        help="landing-range base ref for the commit-message scan "
        "(default: first of github/main, origin/main that resolves)",
    )
    parser.add_argument(
        "--extra-names",
        default=None,
        help="private file outside the repository with one extra confidential "
        f"name per line (default: ${ENV_EXTRA_NAMES} when set)",
    )
    args = parser.parse_args(argv)

    if args.self_test:
        return self_test()

    extra_path = args.extra_names or os.environ.get(ENV_EXTRA_NAMES)
    digests = DIGESTS | extra_digests(Path(extra_path) if extra_path else None)

    names = repo_files()
    hits = file_hits(names, REPO_ROOT, digests)
    scanned = "messages not scanned (no base ref resolves)"
    base = resolve_base(args.base)
    if base is not None:
        messages = commit_messages(base)
        if messages is not None:
            hits += message_hits(messages, digests)
            scanned = f"{len(messages)} commit message(s) since {base}"

    if hits:
        print("check-confidential-names: FAIL", file=sys.stderr)
        for hit in hits:
            print(f"  {hit}", file=sys.stderr)
        print(
            "\n  A confidential name (see #2068 public-tree-confidential-name) must not\n"
            "  appear in this public tree, in any file or commit message. Refer to it\n"
            "  only as \"the model codename\"; the guard knows it by digest alone.",
            file=sys.stderr,
        )
        return 1

    print(f"check-confidential-names: ok - {len(names)} files and {scanned} name no confidential name")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
