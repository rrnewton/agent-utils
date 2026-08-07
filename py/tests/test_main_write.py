"""Both sides of the agent-utils single-writer publication guard.

A guard is only worth having if BOTH brackets fire. The negatives here plant a
genuine violation and require a refusal; the positives plant a legitimate
publication -- and a legitimate ordinary feature-branch push -- and require that
they go through. A guard that blocked those would be worse than the gap.

The fixture builds a real bare "origin" plus real clones on disk, so the
assertions are about git's actual pre-push evaluation rather than a mock of it.
"""

from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
from collections.abc import Iterator
from pathlib import Path
from types import ModuleType

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]


def _load_main_write() -> ModuleType:
    path = REPO_ROOT / "scripts" / "main_write.py"
    spec = importlib.util.spec_from_file_location("_agent_utils_main_write", path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


main_write = _load_main_write()


def _git(root: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(root), *args],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def _commit(root: Path, name: str, text: str) -> str:
    (root / name).write_text(text)
    _git(root, "add", "--", name)
    _git(root, "commit", "-q", "-m", f"add {name}")
    return _git(root, "rev-parse", "HEAD")


def _init_identity(root: Path) -> None:
    _git(root, "config", "user.email", "guard-test@example.invalid")
    _git(root, "config", "user.name", "Guard Test")
    _git(root, "config", "commit.gpgsign", "false")


@pytest.fixture()
def world(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Iterator[dict[str, Path]]:
    """A bare origin with `main`, plus a hooked clone, fully offline."""
    monkeypatch.setenv(main_write.NO_PROXY_ENV, "1")
    monkeypatch.setenv(main_write.LOCK_ENV, str(tmp_path / "writer.lock"))
    monkeypatch.delenv(main_write.TOKEN_ENV, raising=False)
    monkeypatch.delenv(main_write.RECEIPT_ENV, raising=False)

    origin = tmp_path / "origin.git"
    subprocess.run(
        ["git", "init", "-q", "--bare", "--initial-branch=main", str(origin)], check=True
    )

    # The seed carries the tool and the hook as TRACKED files, exactly as
    # agent-utils does, so clones inherit them and `status` sees a clean tree.
    seed = tmp_path / "seed"
    subprocess.run(["git", "init", "-q", "--initial-branch=main", str(seed)], check=True)
    _init_identity(seed)
    (seed / "scripts").mkdir()
    (seed / "scripts" / "main_write.py").write_text(
        (REPO_ROOT / "scripts" / "main_write.py").read_text()
    )
    (seed / ".githooks").mkdir()
    (seed / ".githooks" / "pre-push").write_text(main_write._HOOK_BODY)
    (seed / ".githooks" / "pre-push").chmod(0o755)
    (seed / "base.txt").write_text("base\n")
    _git(seed, "add", "--", "scripts/main_write.py", ".githooks/pre-push", "base.txt")
    _git(seed, "commit", "-q", "-m", "seed")
    _git(seed, "remote", "add", "origin", str(origin))
    _git(seed, "push", "-q", "origin", "HEAD:refs/heads/main")

    work = tmp_path / "work"
    subprocess.run(["git", "clone", "-q", str(origin), str(work)], check=True)
    _init_identity(work)
    _run_tool(work, "install-hooks")

    yield {"origin": origin, "seed": seed, "work": work, "lock": tmp_path / "writer.lock"}


def _run_tool(root: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(REPO_ROOT / "scripts" / "main_write.py"), "--root", str(root), *args],
        capture_output=True,
        text=True,
        env=os.environ.copy(),
    )


def _remote_main(origin: Path) -> str:
    return _git(origin, "rev-parse", "refs/heads/main")


# --------------------------------------------------------------------------- #
# POSITIVE bracket -- the guard must not be inert in the blocking direction     #
# --------------------------------------------------------------------------- #


def test_publish_performs_a_legitimate_fast_forward_publication(
    world: dict[str, Path],
) -> None:
    work, origin = world["work"], world["origin"]
    head = _commit(work, "feature.txt", "feature\n")

    result = _run_tool(work, "publish")

    assert result.returncode == 0, result.stderr
    assert f"published={head}" in result.stdout
    assert "ancestry=1/1" in result.stdout
    assert _remote_main(origin) == head


def test_ordinary_feature_branch_push_is_not_blocked(world: dict[str, Path]) -> None:
    """The guard is scoped to refs/heads/main. Everything else is ordinary work."""
    work = world["work"]
    _git(work, "checkout", "-q", "-b", "topic")
    _commit(work, "topic.txt", "topic\n")

    pushed = subprocess.run(
        ["git", "-C", str(work), "push", "origin", "HEAD:refs/heads/topic"],
        capture_output=True,
        text=True,
    )

    assert pushed.returncode == 0, pushed.stderr
    assert _git(world["origin"], "rev-parse", "refs/heads/topic")


def test_publish_is_idempotent_when_main_already_contains_the_revision(
    world: dict[str, Path],
) -> None:
    work = world["work"]
    _commit(work, "feature.txt", "feature\n")
    assert _run_tool(work, "publish").returncode == 0

    again = _run_tool(work, "publish")

    assert again.returncode == 0, again.stderr
    assert "already-published=" in again.stdout


# --------------------------------------------------------------------------- #
# NEGATIVE bracket -- plant the violation, require the refusal                  #
# --------------------------------------------------------------------------- #


def test_bare_unserialized_push_to_main_is_refused(world: dict[str, Path]) -> None:
    work, origin = world["work"], world["origin"]
    before = _remote_main(origin)
    _commit(work, "sneaky.txt", "sneaky\n")

    pushed = subprocess.run(
        ["git", "-C", str(work), "push", "origin", "HEAD:refs/heads/main"],
        capture_output=True,
        text=True,
    )

    assert pushed.returncode != 0
    assert "no serialized-writer receipt" in pushed.stderr
    assert _remote_main(origin) == before, "refused push must not move main"


def test_copied_receipt_without_a_live_holder_is_refused(
    world: dict[str, Path], monkeypatch: pytest.MonkeyPatch
) -> None:
    """A token is a label. Presenting one without holding the lock must fail."""
    work, origin, lock = world["work"], world["origin"], world["lock"]
    before = _remote_main(origin)
    receipt = main_write.receipt_path(lock)
    receipt.write_text(
        main_write.Receipt(
            token="forged", pid=os.getpid(), expect=before, started=0.0
        ).to_json()
    )
    _commit(work, "forged.txt", "forged\n")

    env = os.environ.copy()
    env[main_write.TOKEN_ENV] = "forged"
    env[main_write.RECEIPT_ENV] = str(receipt)
    pushed = subprocess.run(
        ["git", "-C", str(work), "push", "origin", "HEAD:refs/heads/main"],
        capture_output=True,
        text=True,
        env=env,
    )

    assert pushed.returncode != 0
    assert "writer lock is not held" in pushed.stderr
    assert _remote_main(origin) == before


def test_stale_fetch_receipt_fails_compare_and_swap(world: dict[str, Path]) -> None:
    """A receipt bound to an older origin/main must not authorize a push."""
    lock = world["lock"]
    receipt = main_write.receipt_path(lock)
    # A genuine ancestor, so the CAS check is the one under test rather than
    # the ancestry check short-circuiting ahead of it.
    receipt.write_text(
        main_write.Receipt(
            token="tok", pid=os.getppid(), expect="a" * 40, started=0.0
        ).to_json()
    )
    os.environ[main_write.TOKEN_ENV] = "tok"
    os.environ[main_write.RECEIPT_ENV] = str(receipt)
    try:
        with main_write.WriterLock(lock):
            with pytest.raises(main_write.WriteRefused, match="remote main moved after the fetch"):
                main_write.verify_receipt(
                    lock, expected_remote="b" * 40, hook_pid=os.getpid()
                )
    finally:
        os.environ.pop(main_write.TOKEN_ENV, None)
        os.environ.pop(main_write.RECEIPT_ENV, None)


def test_receipt_from_an_unrelated_process_is_refused(world: dict[str, Path]) -> None:
    """The holder must be an ANCESTOR, not merely another live process."""
    lock = world["lock"]
    receipt = main_write.receipt_path(lock)
    # PID 1 is live but is not an ancestor of this test in the relevant sense
    # of "this push runs inside that serialized operation".
    receipt.write_text(
        main_write.Receipt(
            token="tok", pid=os.getpid(), expect="c" * 40, started=0.0
        ).to_json()
    )
    os.environ[main_write.TOKEN_ENV] = "tok"
    os.environ[main_write.RECEIPT_ENV] = str(receipt)
    try:
        with main_write.WriterLock(lock):
            # A sibling pid that is live but not an ancestor of the caller.
            assert not main_write._is_live_ancestor(os.getpid(), os.getpid())
            with pytest.raises(main_write.WriteRefused, match="not a live ancestor"):
                main_write.verify_receipt(lock, expected_remote="c" * 40, hook_pid=os.getpid())
    finally:
        os.environ.pop(main_write.TOKEN_ENV, None)
        os.environ.pop(main_write.RECEIPT_ENV, None)


def test_non_fast_forward_publication_is_refused(world: dict[str, Path]) -> None:
    work, origin, seed = world["work"], world["origin"], world["seed"]
    _commit(seed, "remote-only.txt", "remote\n")
    _git(seed, "push", "-q", "origin", "HEAD:refs/heads/main")
    advanced = _remote_main(origin)
    _commit(work, "diverged.txt", "diverged\n")

    result = _run_tool(work, "publish")

    assert result.returncode == 1, result.stdout + result.stderr
    assert "not based on freshly fetched origin/main" in result.stderr
    assert _remote_main(origin) == advanced


def test_second_concurrent_writer_is_refused_while_the_lock_is_held(
    world: dict[str, Path],
) -> None:
    work, lock = world["work"], world["lock"]
    _commit(work, "contended.txt", "contended\n")

    with main_write.WriterLock(lock):
        result = _run_tool(work, "publish")

    assert result.returncode == 1
    assert "another agent-utils main writer owns" in result.stderr


def test_main_deletion_is_refused(world: dict[str, Path]) -> None:
    work, origin = world["work"], world["origin"]
    before = _remote_main(origin)

    pushed = subprocess.run(
        ["git", "-C", str(work), "push", "origin", "--delete", "main"],
        capture_output=True,
        text=True,
    )

    assert pushed.returncode != 0
    assert "refusing to delete main" in pushed.stderr
    assert _remote_main(origin) == before


# --------------------------------------------------------------------------- #
# status + PR exceptions                                                        #
# --------------------------------------------------------------------------- #


def test_status_reports_the_queue_predicate_and_its_inputs(world: dict[str, Path]) -> None:
    work = world["work"]

    clear = _run_tool(work, "status", "--json")
    assert clear.returncode == 0, clear.stderr
    assert '"queue_clear": true' in clear.stdout

    _commit(work, "unlanded.txt", "unlanded\n")
    busy = _run_tool(work, "status", "--json")
    assert busy.returncode == 1
    assert '"unlanded_commits": 1' in busy.stdout


def test_recorded_exception_reason_requires_an_exact_allowed_slug() -> None:
    assert (
        main_write.recorded_exception_reason("Exception-Reason: high-risk-preland-review")
        == "high-risk-preland-review"
    )
    assert (
        main_write.recorded_exception_reason(
            "context\nException-Reason: atomic-consumer-change\nmore"
        )
        == "atomic-consumer-change"
    )
    # Prose that merely mentions a reason is not a recorded reason.
    assert main_write.recorded_exception_reason("this is high risk, honest") is None
    assert main_write.recorded_exception_reason("Exception-Reason: because-i-said-so") is None
    assert main_write.recorded_exception_reason("") is None


def test_pr_exceptions_flags_both_violations_and_passes_a_clean_state(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    import argparse

    def fake_pulls(_repository: str) -> list[dict[str, object]]:
        return [
            {"number": 1, "body": "Exception-Reason: high-risk-preland-review"},
            {"number": 2, "body": "no reason at all"},
        ]

    monkeypatch.setattr(main_write, "_open_pull_requests", fake_pulls)
    args = argparse.Namespace(repository="rrnewton/agent-utils", json=True)
    assert main_write.cmd_pr_exceptions(args) == 1
    reported = capsys.readouterr().out
    assert '"satisfied": false' in reported
    assert "records no" in reported
    assert "at most one exceptional PR" in reported

    monkeypatch.setattr(
        main_write,
        "_open_pull_requests",
        lambda _repository: [{"number": 3, "body": "Exception-Reason: atomic-consumer-change"}],
    )
    assert main_write.cmd_pr_exceptions(args) == 0
    assert '"satisfied": true' in capsys.readouterr().out
