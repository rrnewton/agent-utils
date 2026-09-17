"""The registry audit and the process census must not re-ask settled questions.

Every test here pins a COST property and an EQUIVALENCE property together,
because a cheaper census is only worth having if it answers what the expensive
one answered. The cost assertions are counts of work -- Git processes, parses --
never wall time, so they mean the same thing on a loaded box as on an idle one.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

from wrkslots import cli


def _git(repository: Path, *arguments: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repository), *arguments],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


@pytest.fixture
def repository(tmp_path: Path) -> Path:
    root = tmp_path / "repository"
    root.mkdir()
    _git(root, "init", "--initial-branch=main")
    _git(root, "config", "user.email", "test@example.invalid")
    _git(root, "config", "user.name", "Test")
    (root / "file.txt").write_text("contents\n", encoding="utf-8")
    _git(root, "add", "file.txt")
    _git(root, "commit", "-m", "initial")
    return root


def test_worktree_identity_matches_the_three_separate_questions(
    repository: Path,
) -> None:
    """The merged call must return exactly what the separate spellings return."""

    vcs = cli._GitVcs()

    root, common, head = vcs.worktree_identity(repository)

    assert root == vcs.repository_root(repository)
    assert common == vcs.common_directory(repository)
    assert head == vcs.head(repository)


def test_worktree_identity_matches_on_a_linked_worktree(
    repository: Path, tmp_path: Path
) -> None:
    """A linked worktree is the shape the registry audit actually asks about.

    Its toplevel and its common directory DIFFER, so this is the case that would
    catch a merged call returning the two values in the wrong order -- which the
    single-repository case above cannot see.
    """

    linked = tmp_path / "linked"
    _git(repository, "worktree", "add", "--detach", str(linked), "HEAD")
    vcs = cli._GitVcs()

    root, common, head = vcs.worktree_identity(linked)

    assert root == linked.absolute()
    assert common == (repository / ".git").absolute()
    assert root != common
    assert head == vcs.head(repository)


def test_worktree_identity_uses_one_git_process_for_all_three(
    repository: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The whole point: one process, not three.

    This is the control. Restore the three separate calls and this fails, which
    the equivalence tests above would not.
    """

    invocations: list[list[str]] = []
    original = cli._GitVcs._run

    def counting(
        target: Path,
        arguments: list[str],
        **keywords: object,
    ) -> subprocess.CompletedProcess[str]:
        invocations.append(list(arguments))
        return original(target, arguments, **keywords)  # type: ignore[arg-type]

    monkeypatch.setattr(cli._GitVcs, "_run", staticmethod(counting))

    cli._GitVcs().worktree_identity(repository)

    assert len(invocations) == 1, invocations


def test_worktree_identity_refuses_an_unborn_head_like_the_separate_call(
    tmp_path: Path,
) -> None:
    """A refusal the merged call must keep: HEAD that resolves to no commit."""

    empty = tmp_path / "empty"
    empty.mkdir()
    _git(empty, "init", "--initial-branch=main")
    vcs = cli._GitVcs()

    with pytest.raises(cli.Refusal):
        vcs.head(empty)
    with pytest.raises(cli.Refusal):
        vcs.worktree_identity(empty)


def test_worktree_identity_refuses_a_directory_that_is_not_a_repository(
    tmp_path: Path,
) -> None:
    plain = tmp_path / "plain"
    plain.mkdir()

    with pytest.raises(cli.Refusal):
        cli._GitVcs().worktree_identity(plain)


def test_target_match_cache_agrees_with_the_uncached_walk() -> None:
    """Same answers, hits and misses alike."""

    slot = Path("/slots/validate-fresh-one")
    targets = {slot: "validate-fresh-one"}
    probes = [
        slot,
        slot / "nested" / "deep",
        Path("/"),
        Path("/slots"),
        Path("/slots/validate-fresh-one-but-longer"),
        Path("/unrelated/path"),
        Path("relative/not/absolute"),
    ]
    cache: dict[Path, tuple[Path, str] | None] = {}

    for probe in probes:
        uncached = cli._matching_absent_validate_target(probe, targets)
        cached = cli._matching_absent_validate_target(probe, targets, cache)
        assert cached == uncached, probe
        # Second ask comes from the memo and must not differ.
        assert cli._matching_absent_validate_target(probe, targets, cache) == uncached


def test_target_match_cache_remembers_a_negative_answer() -> None:
    """A stored `None` is a real answer.

    Written because `.get` cannot tell "cached: matches nothing" from "not yet
    asked", and the census is DOMINATED by references that match nothing -- so a
    `.get`-based memo would silently re-walk the common case and buy nothing.
    """

    lookups: list[Path] = []

    class CountingTargets(dict[Path, str]):
        def get(  # type: ignore[override]
            self, key: Path, default: str | None = None
        ) -> str | None:
            lookups.append(key)
            return super().get(key, default)

    targets = CountingTargets({Path("/slots/one"): "one"})
    cache: dict[Path, tuple[Path, str] | None] = {}
    probe = Path("/elsewhere/entirely")

    assert cli._matching_absent_validate_target(probe, targets, cache) is None
    walked = len(lookups)
    assert walked > 0
    assert cli._matching_absent_validate_target(probe, targets, cache) is None

    # The second ask must have come from the memo: no further target lookups.
    assert len(lookups) == walked, lookups
    assert cache[probe] is None


def test_mountinfo_parse_cache_agrees_with_the_uncached_parse() -> None:
    table = (
        "24 30 0:22 / /proc rw,nosuid - proc proc rw\n"
        "25 30 0:5 / /dev rw,nosuid - devtmpfs devtmpfs rw\n"
        "26 30 259:2 /slots/validate-fresh-one /mnt/x rw - btrfs /dev/one rw\n"
    )
    cache: dict[str, tuple[tuple[Path, str], ...]] = {}

    uncached = cli._parse_mountinfo_paths(table, "probe")
    cached = cli._parse_mountinfo_paths(table, "probe", cache)

    assert cached == uncached
    assert cli._parse_mountinfo_paths(table, "probe", cache) == uncached


def test_mountinfo_parse_cache_reuses_one_parse_across_identical_tables() -> None:
    """The control for the memo that matters most on a fleet host.

    Nearly every process there has its own mount namespace listing the same
    table, so the parse must happen once per DISTINCT table rather than once per
    process. Remove the cache and this fails.
    """

    table = "24 30 0:22 / /proc rw,nosuid - proc proc rw\n"
    other = "24 30 0:22 / /sys rw,nosuid - sysfs sysfs rw\n"
    cache: dict[str, tuple[tuple[Path, str], ...]] = {}

    for pid in range(50):
        cli._parse_mountinfo_paths(table, f"mount evidence for PID {pid}", cache)
    cli._parse_mountinfo_paths(other, "mount evidence for PID 99", cache)

    assert set(cache) == {table, other}


def test_mountinfo_parse_cache_never_stores_a_refusal(
) -> None:
    """A malformed table must refuse once PER PROCESS, naming that process.

    Caching the failure would report the first PID's name for every later one,
    which is exactly the kind of misattributed evidence this census exists to
    avoid.
    """

    malformed = "this line has no field separator\n"
    cache: dict[str, tuple[tuple[Path, str], ...]] = {}

    with pytest.raises(cli.Refusal, match="PID 11"):
        cli._parse_mountinfo_paths(malformed, "mount evidence for PID 11", cache)
    with pytest.raises(cli.Refusal, match="PID 22"):
        cli._parse_mountinfo_paths(malformed, "mount evidence for PID 22", cache)

    assert cache == {}
