"""`audit --cache-census-only` scopes cache planning without scoping any proof."""

from __future__ import annotations

import json
from collections.abc import Mapping, Sequence
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import (
    checkout,
    command,
    commit_task,
    create,
    empty_process_path_census,
    finish,
    make_project,
    mark_owner_dead,
    set_liveness,
    slots_directory,
)


UNINSPECTED = (
    "cache inspection not performed: slot is outside the --cache-census-only selection"
)


@pytest.fixture(autouse=True)
def isolated_machine_and_cache_home(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")
    # The unscoped default state lives below XDG_CACHE_HOME; keep it in the test.
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "xdg"))


# --- selector parsing -------------------------------------------------------


def test_parse_without_selection_is_empty() -> None:
    assert cli._parse_audit_cache_selection(None) == ()
    assert cli._parse_audit_cache_selection([]) == ()


def test_parse_accepts_exact_identities_up_to_the_bound() -> None:
    raw = [f"host-{index}:validate.slot_{index}={index + 1}" for index in range(8)]
    assert cli._AUDIT_CACHE_SELECTION_LIMIT == 8
    assert cli._parse_audit_cache_selection(raw) == tuple(
        (f"host-{index}", f"validate.slot_{index}", index + 1) for index in range(8)
    )
    assert cli._parse_audit_cache_selection(["h:s=123456789012345678"]) == (
        ("h", "s", 123456789012345678),
    )


def test_parse_refuses_more_than_the_bound() -> None:
    raw = [f"host:slot{index}=1" for index in range(9)]
    with pytest.raises(cli.Refusal, match="at most 8 identities; got 9"):
        cli._parse_audit_cache_selection(raw)


@pytest.mark.parametrize(
    "raw",
    [
        "",
        "slot=1",  # no machine
        "host:slot",  # no generation
        "host:slot=",
        ":slot=1",
        "host:=1",
        "host:slot=0",
        "host:slot=-1",
        "host:slot=+1",
        "host:slot=01",
        "host:slot=1.0",
        "host:slot= 1",
        "host:slot=1 ",
        "host:slot=1_0",
        "host:slot=1\n",
        "host:slot=1234567890123456789",  # beyond 18 digits
        "host:a:b=1",  # a colon cannot belong to a slot name
        "host:slot==1",
        "host:-slot=1",
        "-host:slot=1",
        "host:sl/ot=1",
        "host:sl ot=1",
        "host\n:slot=1",
        f"{'h' * 65}:slot=1",
        f"host:{'s' * 65}=1",
    ],
)
def test_parse_refuses_malformed_selector(raw: str) -> None:
    with pytest.raises(cli.Refusal, match="invalid --cache-census-only"):
        cli._parse_audit_cache_selection([raw])


@pytest.mark.parametrize(
    "raw",
    [
        ["host:slot=1", "host:slot=1"],
        ["host:slot=1", "host:slot=2"],
    ],
    ids=["exact", "other-generation"],
)
def test_parse_refuses_duplicate_identity(raw: list[str]) -> None:
    with pytest.raises(cli.Refusal, match="duplicate --cache-census-only identity"):
        cli._parse_audit_cache_selection(raw)


def test_audit_defaults_and_bounds_are_unchanged() -> None:
    parsed = cli._build_parser().parse_args(["audit"])
    assert parsed.cache_census_only is None
    assert parsed.cache_census_state is None
    assert parsed.cache_work_limit == 100_000
    assert parsed.cache_wall_seconds == 5.0
    assert parsed.gate is False
    assert cli._AUDIT_CACHE_STATE_BYTES_LIMIT == 64 * 1024 * 1024


# --- real lifecycle fixture -------------------------------------------------


def _fleet(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    files: Mapping[str, int],
) -> tuple[Path, dict[str, Path]]:
    """Finished, dead-owner, expired slots whose only cache is `target`."""

    project, repository, _remote = make_project(tmp_path, cache_globs=("target",))
    caches: dict[str, Path] = {}
    for slot, count in files.items():
        made = create(project, slot=slot, agent=f"agent-{slot}", branch=f"test/{slot}")
        assert made.returncode == 0, made.stderr
        tree = checkout(project, slot)
        commit_task(repository, tree, f"test/{slot}")
        finished = finish(project, slot=slot, agent=f"agent-{slot}")
        assert finished.returncode == 0, finished.stderr
        cache = tree / "target"
        cache.mkdir()
        for index in range(count):
            (cache / f"artifact-{index:04d}").write_bytes(b"x" * 4096)
        caches[slot] = cache
    for slot in files:
        mark_owner_dead(project, slot=slot)
    set_liveness(project, "dead")
    monkeypatch.setattr(
        cli, "_capture_process_path_census", empty_process_path_census
    )
    return project, caches


def _audit(
    capsys: pytest.CaptureFixture[str], project: Path, *extra: str
) -> tuple[int, str, str]:
    code = cli.main(["--project-root", str(project), "audit", "--format", "json", *extra])
    captured = capsys.readouterr()
    return code, captured.out, captured.err


def _rows(output: str) -> dict[str, dict[str, object]]:
    return {str(row["slot"]): row for row in json.loads(output)["slots"]}


def _selected(*identities: str) -> list[str]:
    return [item for identity in identities for item in ("--cache-census-only", identity)]


def _expected_bytes(cache: Path) -> int:
    # Independent enumeration, not the census.
    return sum(path.lstat().st_blocks * 512 for path in (cache, *cache.rglob("*")))


def _state_roots(state_path: Path) -> dict[str, Mapping[str, object]]:
    state = json.loads(state_path.read_text(encoding="utf-8"))
    return {key: value for key, value in state["roots"].items()}


def test_unscoped_audit_is_unchanged_by_the_option(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, caches = _fleet(tmp_path, monkeypatch, {"alpha": 2, "bravo": 3})

    code, out, err = _audit(capsys, project)

    assert code == 0, err
    rows = _rows(out)
    for slot, cache in caches.items():
        assert rows[slot]["verdict"] == "DELETABLE"
        assert rows[slot]["reasons"] == []
        assert rows[slot]["cache_status"] == "complete"
        assert rows[slot]["cache_bytes"] == _expected_bytes(cache)
    assert "uninspected" not in out
    default_state = cli._audit_cache_default_state_candidate(
        cli._load_config(str(project), "testhost")
    )
    assert default_state is not None and default_state.is_file()
    assert str(tmp_path / "xdg") in str(default_state)


def test_unselected_rows_fail_closed_and_are_never_planned(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, caches = _fleet(
        tmp_path, monkeypatch, {"alpha": 2, "bravo": 3, "charlie": 1}
    )
    held = command(project, "hold", "charlie", "--reason", "keep for inspection")
    assert held.returncode == 0, held.stderr
    stray_cache = slots_directory(project) / "stray" / "product" / "target"
    stray_cache.mkdir(parents=True)
    (stray_cache / "artifact").write_bytes(b"stray")
    storage = tmp_path / "scoped"
    storage.mkdir()
    state_path = storage / "census.json"

    code, out, err = _audit(
        capsys,
        project,
        "--cache-census-state",
        str(state_path),
        *_selected("testhost:alpha=1"),
    )

    assert code == 0, err
    payload = json.loads(out)
    rows = _rows(out)
    assert rows["alpha"]["verdict"] == "DELETABLE"
    assert rows["alpha"]["cache_status"] == "complete"
    assert rows["alpha"]["cache_bytes"] == _expected_bytes(caches["alpha"])
    # bravo would be DELETABLE in an unscoped audit (see the test above).
    assert rows["bravo"]["verdict"] == "BLOCKED"
    assert rows["bravo"]["reasons"] == [UNINSPECTED]
    assert rows["bravo"]["cache_status"] == "uninspected"
    assert rows["bravo"]["cache_bytes"] is None
    assert rows["bravo"]["cache_error"] is None
    assert rows["charlie"]["verdict"] == "HELD"
    assert rows["charlie"]["cache_status"] == "uninspected"
    assert rows["charlie"]["cache_bytes"] is None
    assert rows["stray"]["verdict"] == "BLOCKED"
    stray_reasons = rows["stray"]["reasons"]
    assert isinstance(stray_reasons, list)
    assert stray_reasons[-1] == UNINSPECTED
    assert rows["stray"]["cache_status"] == "uninspected"
    assert rows["stray"]["cache_bytes"] is None
    assert not any(
        row["verdict"] == "DELETABLE" for slot, row in rows.items() if slot != "alpha"
    )
    assert "bravo" in payload["unknown_slots"]
    # Only the selected row's cache root was planned and persisted.
    roots = _state_roots(state_path)
    assert {root["path"] for root in roots.values()} == {str(caches["alpha"])}
    cache_phase = next(
        phase for phase in payload["metrics"]["phases"] if phase["name"] == "cache-census"
    )
    assert cache_phase["work"]["subjects"] == 1
    assert cache_phase["work"]["cache_roots"] == 1


@pytest.mark.parametrize(
    ("selectors", "message"),
    [
        (("testhost:nosuch=1",), "is not an active registry row"),
        (("otherhost:alpha=1",), "is not an active registry row"),
        (("testhost:alpha=2",), "does not match the active generation 1"),
        (("testhost:alpha=1", "testhost:bravo=7"), "does not match the active generation 1"),
    ],
    ids=["unknown-slot", "wrong-machine", "wrong-generation", "one-of-two-mismatched"],
)
def test_selector_must_bind_an_exact_active_identity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    selectors: Sequence[str],
    message: str,
) -> None:
    project, _caches = _fleet(tmp_path, monkeypatch, {"alpha": 1, "bravo": 1})
    storage = tmp_path / "scoped"
    storage.mkdir()

    code, out, err = _audit(
        capsys,
        project,
        "--cache-census-state",
        str(storage / "census.json"),
        *_selected(*selectors),
    )

    assert code == 3
    assert out == ""
    assert err.startswith("REFUSED: --cache-census-only ")
    assert message in err
    assert list(storage.iterdir()) == []


def test_selector_requires_explicit_non_default_state_outside_the_project(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _caches = _fleet(tmp_path, monkeypatch, {"alpha": 1})
    selector = _selected("testhost:alpha=1")

    code, out, err = _audit(capsys, project, *selector)
    assert (code, out) == (3, "")
    assert "requires an explicit --cache-census-state" in err

    storage = tmp_path / "scoped"
    storage.mkdir()
    code, out, err = _audit(
        capsys, project, "--gate", "--cache-census-state", str(storage / "s.json"), *selector
    )
    assert (code, out) == (3, "")
    assert "not a fleet gate" in err

    code, out, err = _audit(
        capsys, project, "--cache-census-state", str(project / "inside.json"), *selector
    )
    assert (code, out) == (3, "")
    assert "outside the managed project tree" in err

    config = cli._load_config(str(project), "testhost")
    default_state = cli._audit_cache_default_state_candidate(config)
    assert default_state is not None
    default_state.parent.mkdir(parents=True)
    code, out, err = _audit(
        capsys, project, "--cache-census-state", str(default_state), *selector
    )
    assert (code, out) == (3, "")
    assert "must not use the default audit cache census state path" in err
    assert list(default_state.parent.iterdir()) == []
    assert list(storage.iterdir()) == []

    # A malformed selector is refused before configuration or registry reads.
    code, out, err = _audit(
        capsys, project, "--cache-census-state", str(storage / "s.json"),
        *_selected("testhost:alpha"),
    )
    assert (code, out) == (3, "")
    assert "invalid --cache-census-only" in err


def test_two_selected_subjects_resume_to_a_joint_deletable_report(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, caches = _fleet(
        tmp_path, monkeypatch, {"alpha": 20, "bravo": 20, "charlie": 20}
    )
    storage = tmp_path / "scoped"
    storage.mkdir()
    state_path = storage / "census.json"
    arguments = (
        "--cache-census-state",
        str(state_path),
        "--cache-work-limit",
        "64",
        *_selected("testhost:alpha=1", "testhost:bravo=1"),
    )

    code, out, err = _audit(capsys, project, *arguments)
    assert code == 0, err
    rows = _rows(out)
    first_work = next(
        phase["work"]
        for phase in json.loads(out)["metrics"]["phases"]
        if phase["name"] == "cache-census"
    )
    # Alpha spends 22 measure + 22 verify units (20 entries plus '.' and
    # '..' each). Its 21-unit finalization does not fit the remaining 20, so
    # the call reserves alpha's next turn and bravo consumes nothing.
    assert first_work["work_consumed"] == 44
    assert first_work["subjects"] == 2
    # One 20-entry root costs 65 units from scratch, more than the 64-unit
    # budget, so completion is only reachable through persisted progress.
    assert rows["alpha"]["cache_status"] == "partial"
    assert rows["bravo"]["cache_status"] == "partial"
    assert rows["alpha"]["verdict"] == rows["bravo"]["verdict"] == "BLOCKED"
    first_state = json.loads(state_path.read_text(encoding="utf-8"))
    assert first_state["next_subject"] == "record:testhost:alpha:1"
    first_roots = list(first_state["roots"].values())
    assert [root["path"] for root in first_roots] == [str(caches["alpha"])]
    assert first_roots[0]["phase"] == "finalize"
    assert first_roots[0]["status"] == "partial"

    for _ in range(12):
        code, out, err = _audit(capsys, project, *arguments)
        assert code == 0, err
        rows = _rows(out)
        for slot in ("alpha", "bravo"):
            assert rows[slot]["cache_status"] in ("partial", "complete"), rows[slot]
        assert rows["charlie"]["verdict"] == "BLOCKED"
        assert rows["charlie"]["reasons"] == [UNINSPECTED]
        if all(rows[slot]["verdict"] == "DELETABLE" for slot in ("alpha", "bravo")):
            break
    else:
        pytest.fail("two selected subjects never reached one joint DELETABLE report")
    for slot in ("alpha", "bravo"):
        assert rows[slot]["reasons"] == []
        assert rows[slot]["cache_bytes"] == _expected_bytes(caches[slot])
    assert {root["path"] for root in _state_roots(state_path).values()} == {
        str(caches["alpha"]), str(caches["bravo"])
    }


def test_registry_revision_change_discards_scoped_progress(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _caches = _fleet(
        tmp_path, monkeypatch, {"alpha": 20, "bravo": 20, "charlie": 1}
    )
    storage = tmp_path / "scoped"
    storage.mkdir()
    state_path = storage / "census.json"
    arguments = (
        "--cache-census-state",
        str(state_path),
        "--cache-work-limit",
        "30",
        *_selected("testhost:alpha=1", "testhost:bravo=1"),
    )

    assert _audit(capsys, project, *arguments)[0] == 0
    first = json.loads(state_path.read_text(encoding="utf-8"))
    assert _audit(capsys, project, *arguments)[0] == 0
    advanced = json.loads(state_path.read_text(encoding="utf-8"))
    assert advanced["registry_revision"] == first["registry_revision"]
    assert advanced["roots"] != first["roots"]

    # An unselected row's registry write changes the fleet revision.
    mark_owner_dead(project, slot="charlie")
    code, out, err = _audit(capsys, project, *arguments)

    assert code == 0, err
    restarted = json.loads(state_path.read_text(encoding="utf-8"))
    assert restarted["registry_revision"] != first["registry_revision"]
    # A fresh start at the same budget reproduces the first call exactly.
    assert restarted["roots"] == first["roots"]
    assert restarted["next_subject"] == first["next_subject"]
    rows = _rows(out)
    assert rows["alpha"]["cache_status"] == rows["bravo"]["cache_status"] == "partial"


def test_selected_nested_git_is_still_refused(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, caches = _fleet(tmp_path, monkeypatch, {"alpha": 2, "bravo": 2})
    nested = caches["alpha"] / "vendored" / ".git"
    nested.mkdir(parents=True)
    (nested / "HEAD").write_text("ref: refs/heads/main\n", encoding="utf-8")
    storage = tmp_path / "scoped"
    storage.mkdir()

    code, out, err = _audit(
        capsys,
        project,
        "--cache-census-state",
        str(storage / "census.json"),
        *_selected("testhost:alpha=1", "testhost:bravo=1"),
    )

    assert code == 0, err
    rows = _rows(out)
    assert rows["alpha"]["verdict"] == "BLOCKED"
    assert rows["alpha"]["cache_status"] == "error"
    assert rows["alpha"]["cache_bytes"] is None
    assert "nested Git metadata" in str(rows["alpha"]["cache_error"])
    alpha_reasons = rows["alpha"]["reasons"]
    assert isinstance(alpha_reasons, list)
    assert any(
        str(reason).startswith("cache inspection failed:")
        and "nested Git metadata" in str(reason)
        for reason in alpha_reasons
    )
    assert nested.is_dir()
    assert rows["bravo"]["verdict"] == "DELETABLE"


def test_selected_state_over_the_byte_bound_is_refused(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _caches = _fleet(tmp_path, monkeypatch, {"alpha": 2, "bravo": 2})
    monkeypatch.setattr(cli, "_AUDIT_CACHE_STATE_BYTES_LIMIT", 128)
    storage = tmp_path / "scoped"
    storage.mkdir()

    code, out, err = _audit(
        capsys,
        project,
        "--cache-census-state",
        str(storage / "census.json"),
        *_selected("testhost:alpha=1", "testhost:bravo=1"),
    )

    assert code == 0, err
    rows = _rows(out)
    for slot in ("alpha", "bravo"):
        assert rows[slot]["verdict"] == "BLOCKED"
        assert rows[slot]["cache_status"] == "error"
        assert rows[slot]["cache_bytes"] is None
        assert "128-byte safety bound" in str(rows[slot]["cache_error"])
    assert sorted(path.name for path in storage.iterdir()) == ["census.json.key"]
