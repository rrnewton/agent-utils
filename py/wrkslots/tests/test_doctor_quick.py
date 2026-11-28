"""Fast bounded doctor and per-phase timings (#1706).

`doctor --quick` is the periodic-gate mode: it reads only the
registry snapshot (under the shared lock), never spawns a subprocess,
and names every check it skips.  The full doctor reports per-phase
timings and its subprocess count in --json, and its slow checks run
outside the lock, so another command proceeds meanwhile.  Waits are
on conditions; timeouts only detect a hang.
"""

from __future__ import annotations

import dataclasses
import json
import threading
import time
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import command, create, make_project


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")


def test_quick_never_runs_the_slow_checks(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0

    def forbidden(*args: object, **kwargs: object) -> object:
        raise AssertionError("quick mode ran a slow check")

    monkeypatch.setattr(cli, "_shared_submodule_doctor_findings", forbidden)
    monkeypatch.setattr(cli, "_worktree_config_doctor_findings", forbidden)
    monkeypatch.setattr(cli, "_registry_storage_inconsistencies", forbidden)
    monkeypatch.setattr(cli, "_owner_liveness_states", forbidden)

    code = cli.main(
        ["--project-root", str(project), "doctor", "--quick",
         "--format", "json"]
    )
    payload = json.loads(capsys.readouterr().out)
    assert code == 0
    assert payload["quick"] is True
    assert payload["subprocesses"] == 0
    assert payload["counts_by_kind"] == {}
    assert payload["interrupted_journals"] == []
    assert payload["slowest_check"] in payload["timings"]
    skipped = " ".join(payload["skipped"])
    assert "shared-submodule" in skipped
    assert "liveness probes" in skipped
    assert "storage census" in skipped
    (row,) = payload["active"]
    assert row["slot"] == "slot01"


def test_quick_human_names_what_it_skips(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    code = cli.main(["--project-root", str(project), "doctor", "--quick"])
    out = capsys.readouterr().out
    assert code == 0
    assert "quick=true" in out
    assert out.startswith("project=")
    assert "SKIPPED:" in out
    assert "subprocesses=0" in out


def test_full_doctor_reports_phase_timings(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    code = cli.main(
        ["--project-root", str(project), "doctor", "--format", "json"]
    )
    payload = json.loads(capsys.readouterr().out)
    assert code == 0
    timings = payload["timings"]
    for phase in (
        "snapshot",
        "storage_census",
        "owner_liveness",
        "shared_submodule",
        "worktree_config",
        "total",
    ):
        assert phase in timings, phase
        assert timings[phase] >= 0
    assert payload["subprocesses"] > 0
    assert payload["counts_by_kind"] == {}


def test_full_doctor_slow_checks_hold_no_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    assert (
        create(
            project, slot="slot02", agent="codex-2", branch="codex/task2"
        ).returncode
        == 0
    )
    entered = threading.Event()
    release = threading.Event()
    real = cli._shared_submodule_doctor_findings

    def blocking(
        config: cli.Config, states: object
    ) -> list[dict[str, object]]:
        entered.set()
        assert release.wait(timeout=60)
        return real(config, states)  # type: ignore[arg-type]

    monkeypatch.setattr(cli, "_shared_submodule_doctor_findings", blocking)
    outcome: dict[str, object] = {}

    def run_doctor() -> None:
        outcome["returncode"] = cli.main(
            ["--project-root", str(project), "doctor"]
        )

    worker = threading.Thread(target=run_doctor, daemon=True)
    worker.start()
    try:
        assert entered.wait(timeout=60)
        # Doctor is inside its slow checks; the lock is free, so an
        # unrelated mutation proceeds.
        code = cli.main(
            [
                "--project-root", str(project), "hold", "slot02",
                "--reason", "keep",
            ]
        )
        assert code == 0
    finally:
        release.set()
    worker.join(timeout=60)
    assert not worker.is_alive()
    assert outcome["returncode"] == 0


def test_quick_json_matches_rust(tmp_path: Path) -> None:
    """Rust doctor --quick carries the same fields (#1706 parity)."""
    import subprocess

    from wrkslots.tests.rust_binary import rust_wrkslotsd
    from wrkslots.tests.test_doctor_state_rust import _fixture, _root

    root = _root(tmp_path)
    _fixture(root)
    env_python = [
        "python3", "-m", "wrkslots", "--project-root", str(root),
        "doctor", "--quick", "--format", "json",
    ]
    import os

    env = dict(os.environ)
    env["PYTHONPATH"] = str(Path(__file__).resolve().parents[2])
    python_result = subprocess.run(
        env_python, capture_output=True, text=True, check=False, env=env,
        timeout=900,
    )
    rust_result = subprocess.run(
        [str(rust_wrkslotsd()), "doctor-text", "--quick", "--root",
         str(root), "--format", "json"],
        capture_output=True, text=True, check=False, timeout=900,
    )
    assert python_result.returncode == 0, python_result.stderr
    assert rust_result.returncode == 0, rust_result.stderr
    python_doc = json.loads(python_result.stdout)
    rust_doc = json.loads(rust_result.stdout)
    for doc in (python_doc, rust_doc):
        doc.pop("timings", None)
        doc.pop("subprocesses", None)
        for record in doc["active"]:
            record.pop("heartbeat_age_seconds", None)
    assert rust_doc == python_doc


def test_declared_submodules_memo_reads_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Both doctor walks share one declared-submodule read (#1706c)."""
    calls: list[Path] = []
    real = cli._declared_submodules

    def counting(vcs: cli._GitVcs, repository: Path) -> list[tuple[str, str]]:
        calls.append(repository)
        return real(vcs, repository)

    monkeypatch.setattr(cli, "_declared_submodules", counting)
    token = cli._DECLARED_SUBMODULES_MEMO.set({})
    try:
        vcs = cli._GitVcs()
        repo = tmp_path / "repo"
        repo.mkdir()
        (repo / ".gitmodules").write_text(
            '[submodule "sub"]\n\tpath = sub\n\turl = ../sub.git\n',
            encoding="utf-8",
        )
        first = cli._declared_submodules_cached(vcs, repo)
        second = cli._declared_submodules_cached(vcs, repo)
    finally:
        cli._DECLARED_SUBMODULES_MEMO.reset(token)
    assert first == second == [("sub", "sub")]
    assert calls == [repo]


def test_registered_liveness_answers_cover_every_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Concurrent probes return one answer per record (#1706c)."""
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    assert (
        create(
            project, slot="slot02", agent="codex-2", branch="codex/task2"
        ).returncode
        == 0
    )
    config = cli._load_config(str(project), "testhost")
    records = cli._load_active(config).slots
    answers = cli._registered_liveness_states(config, records)
    assert {
        (record.machine, record.slot, record.generation) for record in records
    } == set(answers)
    assert all(state in {"dead", "alive", "unverifiable"} for state, _ in answers.values())


# --- #2468: doctor CPU and lock scope -------------------------------------
#
# Costs are counts (Git processes, event parses), never wall time; every cost
# test also pins that the answer is the answer of the slower path.


def _doctor_json(project: Path, capsys: pytest.CaptureFixture[str]) -> dict[str, object]:
    code = cli.main(["--project-root", str(project), "doctor", "--format", "json"])
    payload = json.loads(capsys.readouterr().out)
    assert code == 0, payload
    assert isinstance(payload, dict)
    return payload


def _comparable(payload: dict[str, object]) -> str:
    """The doctor document without the fields that vary between two runs."""
    document = dict(payload)
    document.pop("timings", None)
    document.pop("subprocesses", None)
    rows = document["active"]
    assert isinstance(rows, list)
    active = [dict(row) for row in rows]
    for row in active:
        row.pop("heartbeat_age_seconds", None)
    document["active"] = active
    return json.dumps(document, sort_keys=True)


def _submodule_project(tmp_path: Path, slots: int) -> Path:
    """Slots whose post-provision hook populates recursive submodules privately."""
    from wrkslots.tests.test_lifecycle import add_recursive_submodules

    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    for index in range(1, slots + 1):
        result = create(
            project,
            slot=f"slot{index:02d}",
            agent=f"codex-{index}",
            branch=f"codex/task{index}",
        )
        assert result.returncode == 0, result.stderr
    return project


def _per_key_inspect(
    vcs: cli._GitVcs, probe: Path, common: Path
) -> cli._WorktreeConfigState:
    """_inspect_worktree_config as it was before #2468: one Git process per key."""

    def get(file: Path, key: str) -> str | None:
        result = vcs._run(probe, ["config", "--file", str(file), "--get", key], check=False)
        return result.stdout.strip() if result.returncode == 0 else None

    held: str | None = None
    worktree_config = common / "config.worktree"
    if worktree_config.is_file() and not worktree_config.is_symlink():
        held = get(worktree_config, "core.worktree")
    bare = get(common / "config", "core.bare")
    return cli._WorktreeConfigState(
        common=common,
        extension_enabled=get(common / "config", "extensions.worktreeConfig") == "true",
        shared_core_worktree=get(common / "config", "core.worktree"),
        held_core_worktree=held,
        core_bare_true=bare is not None and bare.lower() == "true",
    )


@pytest.mark.parametrize(
    ("shared", "held"),
    [
        ("[core]\n\tbare\n\tworktree = /a\n", None),
        ("[core]\n\tWorkTree = /first\n[Core]\n\tworktree = /last\n", "[core]\n\tworktree = /held\n"),
        ("[extensions]\n\tworktreeConfig = true\n[core]\n\tbare = TRUE\n", "[core]\n"),
        ("[extensions]\n\tWorktreeConfig\n[sub \"X\"]\n\tworktree = /no\n", None),
        ("[core\n", "[core]\n\tworktree = /held\n"),
        (None, "not a config [\n"),
        ("[core]\n\tworktree = \"  /spaced  \"\n", None),
    ],
)
def test_inspect_worktree_config_reads_each_file_once_with_per_key_answers(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    shared: str | None,
    held: str | None,
) -> None:
    common = tmp_path / "common"
    common.mkdir()
    if shared is not None:
        (common / "config").write_text(shared, encoding="utf-8")
    if held is not None:
        (common / "config.worktree").write_text(held, encoding="utf-8")
    vcs = cli._GitVcs()
    expected = _per_key_inspect(vcs, tmp_path, common)
    calls: list[list[str]] = []
    real = cli._GitVcs._run

    def counting(repository: Path, args: list[str], **kwargs: object) -> object:
        calls.append(list(args))
        return real(repository, args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(cli._GitVcs, "_run", staticmethod(counting))
    assert cli._inspect_worktree_config(vcs, tmp_path, common) == expected
    assert len(calls) == (2 if held is not None else 1)


def test_doctor_asks_git_each_question_once_and_the_primary_once_per_run(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project = _submodule_project(tmp_path, 3)
    primary = (project / "repo").resolve()
    real = cli._GitVcs._run
    calls: list[tuple[str, tuple[str, ...]]] = []

    def counting(repository: Path, args: list[str], **kwargs: object) -> object:
        calls.append((str(repository), tuple(args)))
        return real(repository, args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(cli._GitVcs, "_run", staticmethod(counting))

    def walk_calls(rows: int) -> list[tuple[str, tuple[str, ...]]]:
        """Git processes of the two submodule walks over the first ``rows`` rows."""
        calls.clear()
        real_shared = cli._shared_submodule_doctor_findings
        real_config = cli._worktree_config_doctor_findings
        recorded: list[tuple[str, tuple[str, ...]]] = []

        def first_rows(states: list[cli.ActiveState]) -> list[cli.ActiveState]:
            return [
                dataclasses.replace(state, slots=tuple(state.slots[:rows]))
                for state in states
            ]

        def shared(config: cli.Config, states: list[cli.ActiveState]) -> object:
            before = len(calls)
            try:
                return real_shared(config, first_rows(states))
            finally:
                recorded.extend(calls[before:])

        def worktree(config: cli.Config, states: list[cli.ActiveState]) -> object:
            before = len(calls)
            try:
                return real_config(config, first_rows(states))
            finally:
                recorded.extend(calls[before:])

        monkeypatch.setattr(cli, "_shared_submodule_doctor_findings", shared)
        monkeypatch.setattr(cli, "_worktree_config_doctor_findings", worktree)
        try:
            _doctor_json(project, capsys)
        finally:
            monkeypatch.setattr(cli, "_shared_submodule_doctor_findings", real_shared)
            monkeypatch.setattr(cli, "_worktree_config_doctor_findings", real_config)
        return recorded

    one = walk_calls(1)
    two = walk_calls(2)
    three = walk_calls(3)
    # The memo: no Git question is asked twice in one run.
    assert len(three) == len(set(three)), sorted(set(c for c in three if three.count(c) > 1))
    # The primary repository and its submodules are asked about once per
    # run, however many rows share them.
    def in_primary(call: tuple[str, tuple[str, ...]]) -> bool:
        return Path(call[0]).resolve().is_relative_to(primary)

    assert sorted(c for c in one if in_primary(c)) == sorted(c for c in three if in_primary(c))
    assert any(in_primary(c) for c in one)
    # Each further row adds only its own checkouts' questions: the same
    # number for every row (the first row also asks once about the
    # primary's common directory from its own checkout).
    assert len(three) - len(two) == len(two) - len(one)
    assert 0 < len(two) - len(one) <= len([c for c in one if not in_primary(c)])


def test_doctor_memo_answers_are_the_unmemoized_answers(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project = _submodule_project(tmp_path, 2)
    # One broken worktree-config repository, so the walk reports something.
    leaf_common = Path(
        cli._GitVcs._run(
            project / "worktrees" / "slot01" / "product" / "component",
            ["rev-parse", "--path-format=absolute", "--git-common-dir"],
        ).stdout.strip()
    )
    cli._GitVcs._run(
        leaf_common, ["config", "--file", str(leaf_common / "config"), "extensions.worktreeConfig", "true"]
    )
    fast = _comparable(_doctor_json(project, capsys))
    monkeypatch.setattr(cli, "_doctor_vcs", cli._GitVcs)
    monkeypatch.setattr(cli, "_inspect_worktree_config", _per_key_inspect)
    monkeypatch.setattr(
        cli._PhasedReadCache,
        "shared_submodule_findings",
        lambda self, config, states: cli._shared_submodule_doctor_findings(config, states),
    )
    monkeypatch.setattr(
        cli._PhasedReadCache,
        "worktree_config_findings",
        lambda self, config, states: cli._worktree_config_doctor_findings(config, states),
    )
    slow = _comparable(_doctor_json(project, capsys))
    assert fast == slow
    assert "shared-submodule-private" in fast
    assert "worktree-config-core-worktree-in-common" in fast


def _settle_and_cap_events(monkeypatch: pytest.MonkeyPatch) -> None:
    """Make a small log stand in for one far over the read-only retention caps."""
    later = cli._EVENT_MEMO_STABLE_NS * 10
    monkeypatch.setattr(cli, "_event_memo_clock_ns", lambda: time.time_ns() + later)
    monkeypatch.setattr(cli, "_CHAIN_MEMO_EVENTS_LIMIT", 1)
    monkeypatch.setattr(cli, "_EVENT_MEMO_BYTES_LIMIT", 1)


def test_doctor_parses_the_log_once_and_only_new_events_under_the_lock(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    for index in (1, 2, 3):
        assert create(
            project, slot=f"slot{index:02d}", agent=f"codex-{index}", branch=f"codex/t{index}"
        ).returncode == 0
    config = cli._load_config(str(project), "testhost")
    cold = _comparable(_doctor_json(project, capsys))
    events = len(cli._load_events(config))

    _settle_and_cap_events(monkeypatch)
    parsed: dict[str, list[int]] = {"locked": [], "unlocked": []}
    real_parse = cli._event_from_path

    def counted(*args: object, **kwargs: object) -> dict[str, object]:
        locked = cli._holds_lock(config.control / "ACTIVE")
        sequence = args[2]
        assert isinstance(sequence, int)
        parsed["locked" if locked else "unlocked"].append(sequence)
        return real_parse(*args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(cli, "_event_from_path", counted)
    real_prewarm = cli._prewarm_registry_read
    appended: list[int] = []
    chains: list[int] = []

    def prewarm_then_append(read: object) -> None:
        real_prewarm(read)  # type: ignore[arg-type]
        memo = cli._ACTIVE_EVENT_MEMO[-1]
        assert memo.bytes_limit is not None and memo.chain_limit is not None
        chains.extend(len(chain.events) for chain in memo.chains.values())
        # A mutation lands between the prewarm and the locked snapshot.
        result = command(project, "hold", "slot02", "--reason", "mid-run")
        assert result.returncode == 0, result.stderr
        appended.append(1)

    monkeypatch.setattr(cli, "_prewarm_registry_read", prewarm_then_append)
    warm = _doctor_json(project, capsys)
    assert appended == [1]
    # 2a: the prewarm kept no chain over the cap.
    assert all(length <= 1 for length in chains)
    # One pass over the log for every fold: no settled event is parsed twice.
    settled = [sequence for sequence in parsed["unlocked"] if sequence <= events]
    assert sorted(settled) == list(range(1, events + 1))
    # Under the lock only the appended event is read (once per fold that
    # resumes on its own), never the settled log.
    assert parsed["locked"] and all(sequence == events + 1 for sequence in parsed["locked"])
    # The answer is the cold answer plus the hold.
    warm_rows = warm["active"]
    assert isinstance(warm_rows, list)
    hold_rows = [row for row in warm_rows if row["held"]]
    assert [row["slot"] for row in hold_rows] == ["slot02"]
    monkeypatch.undo()
    assert _comparable(_doctor_json(project, capsys)) == _comparable(warm)
    assert cold != _comparable(warm)


def test_doctor_retake_measures_outside_the_lock_and_a_mutation_proceeds(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    assert create(project, slot="slot02", agent="codex-2", branch="codex/task2").returncode == 0
    config = cli._load_config(str(project), "testhost")
    measures: list[int] = []
    walks: list[int] = []
    real_proof = cli._proof_mirror_doctor_findings
    real_walk = cli._shared_submodule_doctor_findings

    def proof_with_mutation(config_arg: cli.Config) -> list[dict[str, object]]:
        # Inside every measurement: no registry lock is held, and an
        # exclusive mutation from another process proceeds (and changes the
        # registry, so doctor retakes its snapshot).
        assert not cli._holds_lock(config.control / "ACTIVE")
        verb = ("hold", "slot02", "--reason", "retake") if len(measures) % 2 == 0 else ("unhold", "slot02")
        result = command(project, "--wait-lock", "30", *verb)
        assert result.returncode == 0, result.stderr
        measures.append(1)
        return real_proof(config_arg)

    def counted_walk(config_arg: cli.Config, states: object) -> list[dict[str, object]]:
        walks.append(1)
        return real_walk(config_arg, states)  # type: ignore[arg-type]

    monkeypatch.setattr(cli, "_proof_mirror_doctor_findings", proof_with_mutation)
    monkeypatch.setattr(cli, "_shared_submodule_doctor_findings", counted_walk)
    _doctor_json(project, capsys)
    # The first measurement plus every retake (_PHASED_READ_RETRIES).
    assert len(measures) == cli._PHASED_READ_RETRIES + 1
    # 1b: a retake with the same checkouts does not walk the submodules again.
    assert len(walks) == 1
