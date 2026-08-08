"""Brackets for ``common/bin/rebase-delta-guard``.

The guard exists because a conflict-free ``git rebase`` can silently rewrite a
file: an untracked ``$GIT_DIR/info/attributes`` binding sends the file through a
re-serializing merge driver, the rebase reports success, ``git status`` stays
clean, and the delta is no longer the one you wrote.

Every case here is **planted**. A check that has only been seen not to misbehave
is not evidence, and a guard that fails on everything looks vigilant while being
worthless -- so the positive cases are asserted just as hard as the negative
ones. Both were live risks: the first bracket run of the Hermit predecessor
reported the *positive* case failing, because the test recovered a pre-rebase
SHA by searching the log instead of recording it.

Every SHA below is captured at the moment the commit is created, never
recovered afterwards by searching. That is the habit the tool encodes.
"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path
from typing import NamedTuple

import pytest

GUARD = Path(__file__).resolve().parents[2] / "common" / "bin" / "rebase-delta-guard"

# A merge driver representative of the class: it performs a real, correct 3-way
# merge and then rewrites the whole file. Faithful to the observed incident,
# where the intended change survived intact and rode along with a whole-file
# re-serialization. Deliberately dependency-free so the bracket cannot be
# skipped by a missing library.
RESERIALIZING_DRIVER = '''\
import json, subprocess, sys
base, ours, theirs = sys.argv[1], sys.argv[2], sys.argv[3]
rc = subprocess.call(["git", "merge-file", ours, base, theirs])
text = open(ours).read()
try:
    obj = json.loads(text)
except Exception:
    # Not JSON: double every line's leading indentation. Semantically inert for
    # the test's purposes, and it rewrites nearly every line.
    out = []
    for line in text.splitlines(True):
        stripped = line.lstrip(" ")
        out.append(" " * (2 * (len(line) - len(stripped))) + stripped)
    text = "".join(out)
else:
    text = json.dumps(obj, indent=4, sort_keys=True) + "\\n"
open(ours, "w").write(text)
sys.exit(rc)
'''


def git(repo: Path, *args: str, check: bool = True) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        capture_output=True,
        text=True,
        check=False,
    )
    if check and result.returncode != 0:
        raise AssertionError(f"git {' '.join(args)} failed: {result.stderr}")
    return result.stdout.strip()


def guard(repo: Path, *args: str) -> subprocess.CompletedProcess[str]:
    # Captured rather than piped. Piping this into `head` under `pipefail`
    # yields rc=141 from SIGPIPE, which reads as a crash in the tool.
    return subprocess.run(
        [str(GUARD), "--repo", str(repo), *args],
        capture_output=True,
        text=True,
        check=False,
    )


class Fixture(NamedTuple):
    """The three commits a rebase bracket needs, captured when each is created.

    Typed rather than a dict so a caller cannot quietly pass the wrong field, and
    so the SHAs travel as the strings they are.
    """

    repo: Path
    old_base: str
    old_head: str
    new_base: str


@pytest.fixture()
def fixture_repo(tmp_path: Path) -> Fixture:
    """A repository whose feature branch and main both touch one JSON file.

    Both sides touching the same file is what forces git to run a per-file
    3-way merge during the rebase, which is what gives a merge driver its
    opening. Without that, the driver never fires and the bracket would be
    inert.
    """
    repo = tmp_path / "repo"
    repo.mkdir()
    git(repo, "init", "-q", "-b", "main")
    git(repo, "config", "user.email", "guard@test")
    git(repo, "config", "user.name", "guard test")

    payload = {f"key_{i:03d}": {"nested": [i, i * 2], "name": f"item {i}"} for i in range(60)}
    target = repo / "registry.json"
    target.write_text(json.dumps(payload, indent=2) + "\n")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "base")
    old_base = git(repo, "rev-parse", "HEAD")  # recorded, not searched

    git(repo, "checkout", "-q", "-b", "feature")
    text = target.read_text().replace('"name": "item 7"', '"name": "item 7 PLANTED"', 1)
    target.write_text(text)
    git(repo, "commit", "-q", "-am", "feature: one field")
    old_head = git(repo, "rev-parse", "HEAD")  # recorded, not searched

    git(repo, "checkout", "-q", "main")
    text = target.read_text().replace('"name": "item 41"', '"name": "item 41 UNRELATED"', 1)
    target.write_text(text)
    git(repo, "commit", "-q", "-am", "main: an unrelated field")
    new_base = git(repo, "rev-parse", "HEAD")  # recorded, not searched

    return Fixture(repo=repo, old_base=old_base, old_head=old_head, new_base=new_base)


def install_driver(repo: Path, tmp_path: Path, pattern: str = "registry.json") -> None:
    driver = tmp_path / "reserialize.py"
    driver.write_text(RESERIALIZING_DRIVER)
    git_dir = Path(git(repo, "rev-parse", "--absolute-git-dir"))
    (git_dir / "info").mkdir(exist_ok=True)
    (git_dir / "info" / "attributes").write_text(f"{pattern} merge=reserialize\n")
    git(repo, "config", "merge.reserialize.driver", f"python3 {driver} %O %A %B")


def rebase(repo: Path, onto: str, *flags: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", "-C", str(repo), "rebase", *flags, onto],
        capture_output=True,
        text=True,
        check=False,
    )


def test_positive_clean_rebase_passes(fixture_repo: Fixture, tmp_path: Path) -> None:
    """The guard must PASS an honest rebase. A guard that never passes is worthless."""
    repo = fixture_repo.repo
    git(repo, "checkout", "-q", "feature")

    recorded = guard(repo, "record", "--base", fixture_repo.old_base)
    assert recorded.returncode == 0, recorded.stderr

    assert rebase(repo, fixture_repo.new_base).returncode == 0

    verified = guard(repo, "verify")
    assert verified.returncode == 0, f"{verified.stdout}\n{verified.stderr}"
    assert "OK" in verified.stdout


def test_negative_reserializing_driver_is_caught(
    fixture_repo: Fixture, tmp_path: Path
) -> None:
    """The planted driver rewrites the file; the guard must refuse, loudly."""
    repo = fixture_repo.repo
    git(repo, "checkout", "-q", "feature")
    install_driver(repo, tmp_path)

    recorded = guard(repo, "record", "--base", fixture_repo.old_base)
    assert recorded.returncode == 0, recorded.stderr

    result = rebase(repo, fixture_repo.new_base)
    # The whole point: the rebase itself reports success and leaves a clean tree.
    assert result.returncode == 0, result.stderr
    assert git(repo, "status", "--porcelain") == ""

    verified = guard(repo, "verify")
    assert verified.returncode == 1, f"guard did not catch the rewrite: {verified.stdout}"
    assert "FAIL" in verified.stdout


def test_remedy_apply_backend_preserves_bytes(
    fixture_repo: Fixture, tmp_path: Path
) -> None:
    """With the driver STILL bound, ``rebase --apply`` must preserve the delta.

    This is the documented workaround, so it is bracketed rather than asserted
    in prose.
    """
    repo = fixture_repo.repo
    git(repo, "checkout", "-q", "feature")
    install_driver(repo, tmp_path)

    assert guard(repo, "record", "--base", fixture_repo.old_base).returncode == 0
    assert rebase(repo, fixture_repo.new_base, "--apply").returncode == 0
    assert git(repo, "check-attr", "merge", "--", "registry.json").endswith("reserialize")

    verified = guard(repo, "verify")
    assert verified.returncode == 0, f"{verified.stdout}\n{verified.stderr}"


def test_missing_recording_is_indeterminate_not_pass(fixture_repo: Fixture) -> None:
    """No recording must not read as success. Fail-closed on absence of evidence."""
    repo = fixture_repo.repo
    git(repo, "checkout", "-q", "feature")
    verified = guard(repo, "verify")
    assert verified.returncode == 3
    assert "INDETERMINATE" in verified.stderr


def test_dropped_commit_is_indeterminate_even_when_the_count_still_matches(
    fixture_repo: Fixture,
) -> None:
    """A count is not an identity, and this is the case that proves it.

    Record a two-commit series, drop one, then rebase onto a base that is itself
    one commit further along. ``HEAD~2`` still resolves and the arithmetic still
    balances -- so a count-only derivation names a commit that is not the base at
    all and reports a difference that is an artifact of the wrong base rather
    than a rewritten file. The recorded subjects are what catch it.

    This bracket found that defect in the tool's own derivation. It is kept in
    exactly this shape so a regression to counting alone cannot pass.
    """
    repo = fixture_repo.repo
    git(repo, "checkout", "-q", "feature")
    (repo / "extra.txt").write_text("extra\n")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "feature: second commit")
    assert guard(repo, "record", "--base", fixture_repo.old_base).returncode == 0

    git(repo, "reset", "-q", "--hard", "HEAD~1")
    assert rebase(repo, fixture_repo.new_base).returncode == 0

    verified = guard(repo, "verify")
    assert verified.returncode == 3, "a wrong base must not be reported as a delta change"
    assert "not the recorded series" in verified.stderr
    # The trap, asserted so the reason survives: the count agreed.
    assert "The count matched" in verified.stderr


def test_shortened_history_is_indeterminate(fixture_repo: Fixture) -> None:
    """The plainer case: HEAD has fewer ancestors than the recorded series."""
    repo = fixture_repo.repo
    git(repo, "checkout", "-q", "feature")
    assert guard(repo, "record", "--base", fixture_repo.old_base).returncode == 0
    git(repo, "checkout", "-q", "--detach", fixture_repo.old_base)

    verified = guard(repo, "verify")
    assert verified.returncode == 3
    assert "INDETERMINATE" in verified.stderr


def test_state_is_per_worktree_not_shared(fixture_repo: Fixture, tmp_path: Path) -> None:
    """Two worktrees of one repository must not overwrite each other's recording.

    Linked worktrees share ``$GIT_COMMON_DIR``; putting the state there would
    reintroduce the shared-mutable-state class this tool responds to.
    """
    repo = fixture_repo.repo
    git(repo, "checkout", "-q", "feature")
    linked = tmp_path / "linked"
    git(repo, "worktree", "add", "-q", "-b", "other", str(linked), fixture_repo.new_base)

    assert guard(repo, "record", "--base", fixture_repo.old_base).returncode == 0
    # The linked worktree has its own state, so it sees no recording at all.
    assert guard(linked, "status").returncode == 3
    # ... and the original recording is untouched.
    assert guard(repo, "status").returncode == 0


def test_check_subcommand_is_usable_without_recording(fixture_repo: Fixture) -> None:
    """The stateless form still works, for a rebase somebody else already did."""
    repo = fixture_repo.repo
    git(repo, "checkout", "-q", "feature")
    assert rebase(repo, fixture_repo.new_base).returncode == 0
    new_head = git(repo, "rev-parse", "HEAD")

    result = guard(
        repo,
        "check",
        fixture_repo.old_base,
        fixture_repo.old_head,
        fixture_repo.new_base,
        new_head,
    )
    assert result.returncode == 0, f"{result.stdout}\n{result.stderr}"


def test_guard_is_executable_and_reports_usage() -> None:
    assert os.access(GUARD, os.X_OK), "the tool must be committed executable"
    result = subprocess.run([str(GUARD)], capture_output=True, text=True, check=False)
    assert result.returncode == 2
