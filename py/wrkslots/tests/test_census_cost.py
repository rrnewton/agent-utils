"""The registry audit and the process census must not re-ask settled questions.

Every test here pins a COST property and an EQUIVALENCE property together,
because a cheaper census is only worth having if it answers what the expensive
one answered. The cost assertions are counts of work -- Git processes, parses --
never wall time, so they mean the same thing on a loaded box as on an idle one.
"""

from __future__ import annotations

import json
import shutil
import subprocess
from dataclasses import replace
from pathlib import Path

import pytest

from wrkslots import cli


def _config(tmp_path: Path, *, batch: bool = False) -> cli.Config:
    root = tmp_path / "project"
    worktrees = root / "worktrees"
    root.mkdir(exist_ok=True)
    worktrees.mkdir(exist_ok=True)
    liveness = root / "liveness.py"
    liveness.write_text("raise SystemExit(2)\n", encoding="utf-8")
    batch_path = root / "liveness-batch.py"
    if batch:
        batch_path.write_text("raise SystemExit(2)\n", encoding="utf-8")
    return cli.Config(
        root=root,
        config_path=root / ".wrkslots.yml",
        worktrees=worktrees,
        control=worktrees,
        machine="testhost",
        default_remote="origin",
        default_landed_ref="refs/remotes/origin/main",
        heartbeat_ttl_seconds=3600,
        liveness_command=liveness,
        liveness_batch_command=batch_path if batch else None,
    )


def _records(count: int) -> tuple[cli.ActiveRecord, ...]:
    coordinator = cli.ProcessIdentity(1, 1, "boot", "host", "/fixture")
    return tuple(
        cli.ActiveRecord(
            slot=f"slot-{index:03d}",
            agent=f"agent-{index:03d}",
            task=f"task-{index:03d}",
            purpose="batch equivalence fixture",
            slot_type="agent",
            machine="testhost",
            generation=1,
            created_at="2026-01-01T00:00:00+00:00",
            heartbeat_at="2026-01-01T00:00:00+00:00",
            heartbeat_ttl_seconds=3600,
            owner=None,
            coordinator_lease=coordinator,
            coordinator_recovery_note=None,
            handoff=None,
            checkouts=(),
        )
        for index in range(count)
    )


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


@pytest.mark.parametrize("row_count", (64, 128, 256))
def test_batch_liveness_matches_per_subject_results_with_one_invocation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    row_count: int,
) -> None:
    config = _config(tmp_path, batch=True)
    records = _records(row_count)
    calls = 0

    def run(
        _command: list[str],
        *,
        input_data: bytes,
        **_kwargs: object,
    ) -> tuple[int, bytes, bytes]:
        nonlocal calls
        calls += 1
        request = json.loads(input_data)
        results = []
        for index, subject in enumerate(request["subjects"]):
            state = ("dead", "alive", "unverifiable")[index % 3]
            results.append(
                {
                    "subject_id": subject["subject_id"],
                    "agent": subject["agent"],
                    "state": state,
                    "detail": f"fixture {state}",
                }
            )
        response = {
            "schema": cli._LIVENESS_BATCH_RESPONSE_SCHEMA,
            "request_sha256": request["request_sha256"],
            "results": results,
        }
        return 0, json.dumps(response).encode("utf-8"), b""

    monkeypatch.setattr(cli, "_run_bounded_read_only_command", run)

    def run_legacy(
        command: list[str],
        **_kwargs: object,
    ) -> subprocess.CompletedProcess[str]:
        index = int(command[-1].rsplit("-", 1)[1])
        state = ("dead", "alive", "unverifiable")[index % 3]
        return subprocess.CompletedProcess(
            command,
            {"dead": 0, "alive": 1, "unverifiable": 2}[state],
            f"fixture {state}\n",
            "",
        )

    monkeypatch.setattr(subprocess, "run", run_legacy)

    observed = cli._registered_liveness_states(config, records)
    legacy = cli._registered_liveness_states(
        replace(config, liveness_batch_command=None), records
    )

    assert calls == 1
    assert observed == legacy
    assert len(observed) == row_count


@pytest.mark.parametrize("mutation", ("missing", "duplicate", "misbound", "wrong-request"))
def test_batch_liveness_goalpost_mutations_fail_closed(
    tmp_path: Path,
    mutation: str,
) -> None:
    config = _config(tmp_path, batch=True)
    request = cli._liveness_batch_request(config, _records(2))
    subjects = request["subjects"]
    assert isinstance(subjects, list)
    results = [
        {
            "subject_id": subject["subject_id"],
            "agent": subject["agent"],
            "state": "dead",
            "detail": "fixture dead",
        }
        for subject in subjects
    ]
    response = {
        "schema": cli._LIVENESS_BATCH_RESPONSE_SCHEMA,
        "request_sha256": request["request_sha256"],
        "results": results,
    }
    if mutation == "missing":
        results.pop()
    elif mutation == "duplicate":
        results.append(dict(results[0]))
    elif mutation == "misbound":
        results[0]["agent"] = "another-agent"
    else:
        response["request_sha256"] = "0" * 64

    with pytest.raises(cli.Refusal):
        cli._parse_liveness_batch_response(json.dumps(response), request)


def test_batch_liveness_malformed_response_makes_every_subject_unverifiable(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    config = _config(tmp_path, batch=True)
    records = _records(4)
    monkeypatch.setattr(
        cli,
        "_run_bounded_read_only_command",
        lambda _command, **_kwargs: (0, b"not-json", b""),
    )

    observed = cli._registered_liveness_states(config, records)

    assert set(state for state, _detail in observed.values()) == {"unverifiable"}
    assert all("not JSON" in detail for _state, detail in observed.values())


def test_cache_census_is_partial_then_resumes_exactly(
    tmp_path: Path,
) -> None:
    config = _config(tmp_path)
    checkout = config.root / "checkout"
    cache = checkout / "target"
    cache.mkdir(parents=True)
    for index in range(9):
        (cache / f"file-{index}").write_bytes(bytes([index]) * (index + 1))
    checkout_identity = cli._open_directory_identity(checkout, "checkout")
    planned = {
        "subject": (
            cli.CacheDirectory(
                path=cache,
                checkout_root=checkout,
                checkout_device=checkout_identity[0],
                checkout_inode=checkout_identity[1],
                checkout_mount_id=checkout_identity[2],
            ),
        )
    }
    states = (cli.ActiveState("testhost", 7, ()),)
    state_path = tmp_path / "cache-state.json"
    statuses: list[str] = []
    work: list[int] = []

    for _ in range(20):
        measured, counters = cli._audit_cache_census(
            config,
            states,
            planned,
            {},
            state_path=state_path,
            work_limit=2,
            wall_seconds=5,
        )
        statuses.append(measured["subject"].status)
        work.append(counters["work_consumed"])
        if measured["subject"].status == "complete":
            break

    if measured["subject"].status != "complete":
        measured, final_counters = cli._audit_cache_census(
            config,
            states,
            planned,
            {},
            state_path=state_path,
            work_limit=100,
            wall_seconds=5,
        )
        assert final_counters["work_consumed"] <= 100
        statuses.append(measured["subject"].status)

    expected = sum(path.stat().st_blocks * 512 for path in (cache, *cache.iterdir()))
    assert statuses[0] == "partial"
    assert statuses[-1] == "complete"
    assert measured["subject"].bytes == expected
    assert all(value <= 2 for value in work)
    assert state_path.is_file()


def test_cache_census_identity_change_never_reuses_completed_bytes(
    tmp_path: Path,
) -> None:
    config = _config(tmp_path)
    checkout = config.root / "checkout"
    cache = checkout / "target"
    cache.mkdir(parents=True)
    (cache / "old").write_bytes(b"old")
    checkout_identity = cli._open_directory_identity(checkout, "checkout")

    def planned() -> dict[str, tuple[cli.CacheDirectory, ...]]:
        return {
            "subject": (
                cli.CacheDirectory(
                    path=cache,
                    checkout_root=checkout,
                    checkout_device=checkout_identity[0],
                    checkout_inode=checkout_identity[1],
                    checkout_mount_id=checkout_identity[2],
                ),
            )
        }

    states = (cli.ActiveState("testhost", 9, ()),)
    state_path = tmp_path / "cache-state.json"
    first, _work = cli._audit_cache_census(
        config,
        states,
        planned(),
        {},
        state_path=state_path,
        work_limit=100,
        wall_seconds=5,
    )
    assert first["subject"].status == "complete"
    old_bytes = first["subject"].bytes

    shutil.rmtree(cache)
    cache.mkdir()
    for index in range(5):
        (cache / f"new-{index}").write_bytes(b"new contents")
    second, _work = cli._audit_cache_census(
        config,
        states,
        planned(),
        {},
        state_path=state_path,
        work_limit=1,
        wall_seconds=5,
    )

    assert second["subject"].status == "partial"
    assert second["subject"].bytes is None
    assert second["subject"].bytes != old_bytes
    persisted = json.loads(state_path.read_text(encoding="utf-8"))
    assert len(persisted["roots"]) == 1


def test_cache_census_refuses_mutated_resume_path_outside_root(
    tmp_path: Path,
) -> None:
    config = _config(tmp_path)
    checkout = config.root / "checkout"
    cache = checkout / "target"
    cache.mkdir(parents=True)
    (cache / "inside").write_bytes(b"inside")
    checkout_identity = cli._open_directory_identity(checkout, "checkout")
    planned = {
        "subject": (
            cli.CacheDirectory(
                path=cache,
                checkout_root=checkout,
                checkout_device=checkout_identity[0],
                checkout_inode=checkout_identity[1],
                checkout_mount_id=checkout_identity[2],
            ),
        )
    }
    states = (cli.ActiveState("testhost", 11, ()),)
    state_path = tmp_path / "cache-state.json"
    first, _work = cli._audit_cache_census(
        config,
        states,
        planned,
        {},
        state_path=state_path,
        work_limit=1,
        wall_seconds=5,
    )
    assert first["subject"].status == "partial"
    state = json.loads(state_path.read_text(encoding="utf-8"))
    root = next(iter(state["roots"].values()))
    root["pending"] = [".."]
    root["current"] = None
    root["status"] = "partial"
    state_path.write_text(json.dumps(state), encoding="utf-8")

    observed, _work = cli._audit_cache_census(
        config,
        states,
        planned,
        {},
        state_path=state_path,
        work_limit=10,
        wall_seconds=5,
    )

    assert observed["subject"].status == "complete"
    assert observed["subject"].bytes == cli._allocated_cache_bytes(
        config, planned["subject"][0]
    )


def test_cache_census_single_large_directory_makes_bounded_progress(
    tmp_path: Path,
) -> None:
    config = _config(tmp_path)
    checkout = config.root / "checkout"
    cache = checkout / "target"
    cache.mkdir(parents=True)
    for index in range(1024):
        (cache / f"artifact-{index:04d}").write_bytes(b"x")
    checkout_identity = cli._open_directory_identity(checkout, "checkout")
    planned = {
        "subject": (
            cli.CacheDirectory(
                path=cache,
                checkout_root=checkout,
                checkout_device=checkout_identity[0],
                checkout_inode=checkout_identity[1],
                checkout_mount_id=checkout_identity[2],
            ),
        )
    }
    states = (cli.ActiveState("testhost", 12, ()),)
    state_path = tmp_path / "large-cache-state.json"
    consumed: list[int] = []

    for _ in range(34):
        measured, counters = cli._audit_cache_census(
            config,
            states,
            planned,
            {},
            state_path=state_path,
            work_limit=64,
            wall_seconds=5,
        )
        consumed.append(counters["work_consumed"])
        if measured["subject"].status == "complete":
            break

    if measured["subject"].status != "complete":
        measured, final_counters = cli._audit_cache_census(
            config,
            states,
            planned,
            {},
            state_path=state_path,
            work_limit=2048,
            wall_seconds=5,
        )
        assert final_counters["work_consumed"] <= 2048

    assert measured["subject"].status == "complete"
    assert all(0 < work <= 64 for work in consumed)
    assert len(consumed) <= 34


def test_cache_census_preserves_nested_git_metadata_refusal(
    tmp_path: Path,
) -> None:
    config = _config(tmp_path)
    checkout = config.root / "checkout"
    cache = checkout / "target"
    nested = cache / "nested"
    nested.mkdir(parents=True)
    (nested / ".git").mkdir()
    checkout_identity = cli._open_directory_identity(checkout, "checkout")
    directory = cli.CacheDirectory(
        path=cache,
        checkout_root=checkout,
        checkout_device=checkout_identity[0],
        checkout_inode=checkout_identity[1],
        checkout_mount_id=checkout_identity[2],
    )

    with pytest.raises(cli.Refusal, match="nested Git metadata"):
        cli._allocated_cache_bytes(config, directory)
    measured, _work = cli._audit_cache_census(
        config,
        (cli.ActiveState("testhost", 13, ()),),
        {"subject": (directory,)},
        {},
        state_path=tmp_path / "nested-git-state.json",
        work_limit=100,
        wall_seconds=5,
    )

    assert measured["subject"].status == "error"
    assert measured["subject"].bytes is None
    assert measured["subject"].error is not None
    assert "nested Git metadata" in measured["subject"].error


def test_completed_cache_census_revalidates_nested_directories(
    tmp_path: Path,
) -> None:
    config = _config(tmp_path)
    checkout = config.root / "checkout"
    cache = checkout / "target"
    nested = cache / "nested"
    nested.mkdir(parents=True)
    (nested / "artifact").write_bytes(b"artifact")
    checkout_identity = cli._open_directory_identity(checkout, "checkout")
    directory = cli.CacheDirectory(
        path=cache,
        checkout_root=checkout,
        checkout_device=checkout_identity[0],
        checkout_inode=checkout_identity[1],
        checkout_mount_id=checkout_identity[2],
    )
    states = (cli.ActiveState("testhost", 14, ()),)
    planned = {"subject": (directory,)}
    state_path = tmp_path / "nested-mutation-state.json"
    complete, _work = cli._audit_cache_census(
        config,
        states,
        planned,
        {},
        state_path=state_path,
        work_limit=100,
        wall_seconds=5,
    )
    assert complete["subject"].status == "complete"

    (nested / ".git").mkdir()
    changed, _work = cli._audit_cache_census(
        config,
        states,
        planned,
        {},
        state_path=state_path,
        work_limit=100,
        wall_seconds=5,
    )

    assert changed["subject"].status == "error"
    assert changed["subject"].bytes is None
    assert changed["subject"].error is not None
    assert "nested Git metadata" in changed["subject"].error


@pytest.mark.parametrize("forged_status", ("complete", "partial"))
def test_forged_cache_state_cannot_bypass_fresh_recursive_verification(
    tmp_path: Path,
    forged_status: str,
) -> None:
    config = _config(tmp_path)
    checkout = config.root / "checkout"
    cache = checkout / "target"
    cache.mkdir(parents=True)
    (cache / ".git").mkdir()
    checkout_identity = cli._open_directory_identity(checkout, "checkout")
    directory = cli.CacheDirectory(
        path=cache,
        checkout_root=checkout,
        checkout_device=checkout_identity[0],
        checkout_inode=checkout_identity[1],
        checkout_mount_id=checkout_identity[2],
    )
    states = (cli.ActiveState("testhost", 15, ()),)
    identity = cli._audit_cache_root_identity(config, directory)
    assert identity is not None
    key = cli._audit_cache_root_key(directory, identity)
    forged = cli._new_audit_cache_root(directory, identity)
    metadata = cache.stat()
    forged.update(
        {
            "phase": "finalize" if forged_status == "complete" else "verify",
            "bytes": 0,
            "verify_bytes": 0,
            "pending": [],
            "current": None,
            "status": forged_status,
            "directories_visited": 1,
            "directories_verified": 1,
            "verified_directories": [
                {
                    "path": ".",
                    "identity": [
                        metadata.st_dev,
                        metadata.st_ino,
                        metadata.st_mtime_ns,
                        metadata.st_ctime_ns,
                    ],
                }
            ],
            "finalize_index": 1 if forged_status == "complete" else 0,
            "directories_finalized": 1 if forged_status == "complete" else 0,
        }
    )
    state_path = tmp_path / "forged-complete.json"
    state_path.write_text(
        json.dumps(
            {
                "schema": cli._AUDIT_CACHE_CENSUS_SCHEMA,
                "registry_revision": cli._audit_registry_revision(states),
                "roots": {key: forged},
            }
        ),
        encoding="utf-8",
    )

    measured, _work = cli._audit_cache_census(
        config,
        states,
        {"subject": (directory,)},
        {},
        state_path=state_path,
        work_limit=100,
        wall_seconds=5,
    )

    assert measured["subject"].status == "error"
    assert measured["subject"].bytes is None
    assert measured["subject"].error is not None
    assert "nested Git metadata" in measured["subject"].error
    key_path = state_path.with_name(f"{state_path.name}.key")
    assert key_path.stat().st_mode & 0o777 == 0o600


def test_cache_census_finalization_rechecks_earlier_verification_chunks(
    tmp_path: Path,
) -> None:
    config = _config(tmp_path)
    checkout = config.root / "checkout"
    cache = checkout / "target"
    for name in ("first", "second"):
        directory = cache / name
        directory.mkdir(parents=True)
        (directory / "artifact").write_bytes(name.encode("ascii"))
    checkout_identity = cli._open_directory_identity(checkout, "checkout")
    cache_directory = cli.CacheDirectory(
        path=cache,
        checkout_root=checkout,
        checkout_device=checkout_identity[0],
        checkout_inode=checkout_identity[1],
        checkout_mount_id=checkout_identity[2],
    )
    states = (cli.ActiveState("testhost", 18, ()),)
    planned = {"subject": (cache_directory,)}
    state_path = tmp_path / "cross-chunk-mutation.json"
    changed: Path | None = None

    for _ in range(40):
        measured, _work = cli._audit_cache_census(
            config,
            states,
            planned,
            {},
            state_path=state_path,
            work_limit=1,
            wall_seconds=5,
        )
        state = json.loads(state_path.read_text(encoding="utf-8"))
        root = next(iter(state["roots"].values()))
        current = root.get("current")
        current_path = current.get("path") if isinstance(current, dict) else None
        completed_nested = [
            item["path"]
            for item in root["verified_directories"]
            if item["path"] != "." and item["path"] != current_path
        ]
        if root["phase"] == "verify" and completed_nested:
            changed = cache / completed_nested[0]
            (changed / ".git").mkdir()
            break
    assert changed is not None
    assert measured["subject"].status == "partial"

    observed, _work = cli._audit_cache_census(
        config,
        states,
        planned,
        {},
        state_path=state_path,
        work_limit=100,
        wall_seconds=5,
    )

    assert observed["subject"].status == "error"
    assert observed["subject"].bytes is None
    assert observed["subject"].error is not None
    assert "changed before final publication" in observed["subject"].error


def test_completed_cache_state_rechecks_file_allocation(
    tmp_path: Path,
) -> None:
    config = _config(tmp_path)
    checkout = config.root / "checkout"
    cache = checkout / "target"
    cache.mkdir(parents=True)
    artifact = cache / "artifact"
    artifact.write_bytes(b"x")
    checkout_identity = cli._open_directory_identity(checkout, "checkout")
    directory = cli.CacheDirectory(
        path=cache,
        checkout_root=checkout,
        checkout_device=checkout_identity[0],
        checkout_inode=checkout_identity[1],
        checkout_mount_id=checkout_identity[2],
    )
    states = (cli.ActiveState("testhost", 16, ()),)
    planned = {"subject": (directory,)}
    state_path = tmp_path / "file-growth-state.json"
    first, _work = cli._audit_cache_census(
        config,
        states,
        planned,
        {},
        state_path=state_path,
        work_limit=100,
        wall_seconds=5,
    )
    assert first["subject"].status == "complete"

    artifact.write_bytes(b"x" * (2 * 1024 * 1024))
    historical = cli._allocated_cache_bytes(config, directory)
    assert historical != first["subject"].bytes
    second, _work = cli._audit_cache_census(
        config,
        states,
        planned,
        {},
        state_path=state_path,
        work_limit=100,
        wall_seconds=5,
    )

    assert second["subject"].status == "complete"
    assert second["subject"].bytes == historical


def test_cache_census_counts_symlink_without_following_like_historical_scan(
    tmp_path: Path,
) -> None:
    config = _config(tmp_path)
    checkout = config.root / "checkout"
    cache = checkout / "target"
    cache.mkdir(parents=True)
    outside = checkout / "outside"
    outside.write_bytes(b"outside")
    (cache / "link").symlink_to(outside)
    checkout_identity = cli._open_directory_identity(checkout, "checkout")
    directory = cli.CacheDirectory(
        path=cache,
        checkout_root=checkout,
        checkout_device=checkout_identity[0],
        checkout_inode=checkout_identity[1],
        checkout_mount_id=checkout_identity[2],
    )
    measured, _work = cli._audit_cache_census(
        config,
        (cli.ActiveState("testhost", 17, ()),),
        {"subject": (directory,)},
        {},
        state_path=tmp_path / "symlink-state.json",
        work_limit=100,
        wall_seconds=5,
    )

    expected = cli._allocated_cache_bytes(config, directory)
    assert measured["subject"].status == "complete"
    assert measured["subject"].bytes == expected


def test_cache_census_state_path_refuses_external_symlink_into_project(
    tmp_path: Path,
) -> None:
    config = _config(tmp_path)
    alias = tmp_path / "external-alias"
    alias.symlink_to(config.root, target_is_directory=True)

    with pytest.raises(cli.Refusal, match="symlink-free"):
        cli._audit_cache_state_path(config, str(alias / "census.json"))

    assert not (config.root / "census.json").exists()
    assert not (config.root / "census.json.key").exists()


def test_cache_census_refuses_nonprivate_existing_key(tmp_path: Path) -> None:
    config = _config(tmp_path)
    checkout = config.root / "checkout"
    cache = checkout / "target"
    cache.mkdir(parents=True)
    (cache / "artifact").write_bytes(b"artifact")
    checkout_identity = cli._open_directory_identity(checkout, "checkout")
    directory = cli.CacheDirectory(
        path=cache,
        checkout_root=checkout,
        checkout_device=checkout_identity[0],
        checkout_inode=checkout_identity[1],
        checkout_mount_id=checkout_identity[2],
    )
    states = (cli.ActiveState("testhost", 19, ()),)
    state_path = tmp_path / "key-mode-state.json"
    first, _work = cli._audit_cache_census(
        config,
        states,
        {"subject": (directory,)},
        {},
        state_path=state_path,
        work_limit=100,
        wall_seconds=5,
    )
    assert first["subject"].status == "complete"
    key_path = state_path.with_name(f"{state_path.name}.key")
    key_path.chmod(0o644)

    second, _work = cli._audit_cache_census(
        config,
        states,
        {"subject": (directory,)},
        {},
        state_path=state_path,
        work_limit=100,
        wall_seconds=5,
    )

    assert second["subject"].status == "error"
    assert second["subject"].bytes is None
    assert second["subject"].error is not None
    assert "owner-bound 0600" in second["subject"].error


def test_cache_census_finalization_rechecks_earlier_file_allocation(
    tmp_path: Path,
) -> None:
    config = _config(tmp_path)
    checkout = config.root / "checkout"
    cache = checkout / "target"
    cache.mkdir(parents=True)
    for index in range(4):
        (cache / f"artifact-{index}").write_bytes(b"x")
    checkout_identity = cli._open_directory_identity(checkout, "checkout")
    directory = cli.CacheDirectory(
        path=cache,
        checkout_root=checkout,
        checkout_device=checkout_identity[0],
        checkout_inode=checkout_identity[1],
        checkout_mount_id=checkout_identity[2],
    )
    states = (cli.ActiveState("testhost", 20, ()),)
    planned = {"subject": (directory,)}
    state_path = tmp_path / "cross-chunk-file-growth.json"
    changed: Path | None = None

    for _ in range(30):
        measured, _work = cli._audit_cache_census(
            config,
            states,
            planned,
            {},
            state_path=state_path,
            work_limit=1,
            wall_seconds=5,
        )
        state = json.loads(state_path.read_text(encoding="utf-8"))
        root = next(iter(state["roots"].values()))
        if root["phase"] == "verify" and root["verified_entries"]:
            changed = cache / root["verified_entries"][0]["path"]
            changed.write_bytes(b"x" * (2 * 1024 * 1024))
            break
    assert changed is not None
    assert measured["subject"].status == "partial"

    observed, _work = cli._audit_cache_census(
        config,
        states,
        planned,
        {},
        state_path=state_path,
        work_limit=100,
        wall_seconds=5,
    )

    assert observed["subject"].status == "error"
    assert observed["subject"].bytes is None
    assert observed["subject"].error is not None
    assert "entry changed before final publication" in observed["subject"].error
