"""Differential: wrkslotsd classify-create-journals against Python.

#562 rust-primary-wrkslots: the soak `classify-create-journals`
reader. The Python oracle is `_cmd_classify_create_journals` on a
copy of the fixture registry holding one interrupted create (a
failing post-provision hook left its journal); the Rust reader runs
on the original. Stdout must match byte for byte apart from fixture
roots and commit ids. Temporary fixture registries only.
"""

from __future__ import annotations

import argparse
import contextlib
import fcntl
import io
import json
import os
import re
import subprocess
from collections.abc import Iterator
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.rust_binary import rust_wrkslotsd
from wrkslots.tests.test_create_text_rust import _fixture, _repo
from wrkslots.tests.test_transfer_owner_write_rust import (
    _dead_identity,
    _fixture as _transfer_fixture,
    _live_identity,
)


AU_ROOT = Path(__file__).resolve().parents[3]
HEX40 = re.compile(r"\b[0-9a-f]{40}\b")


def _norm(text: str, root: Path) -> str:
    text = text.replace(str(root), "ROOT").replace(str(root.parent), "BASE")
    return HEX40.sub("HEAD", text)


def _interrupted(root: Path) -> None:
    config = root / ".wrkslots.yml"
    config.write_text(
        config.read_text() + "post_provision_hooks:\n  - 'echo hook-broken >&2; exit 3'\n"
    )
    args = argparse.Namespace(
        project_root=str(root), machine="node-a", slot="slot-b", slot_type="agent",
        coordinator_authorized=True, agent="agent-new", task="task-1",
        purpose="testing", owner_pid=os.getpid(), coordinator_pid=os.getpid(),
        task_scope_unit=None, task_scope_invocation_id=None,
        repo=["repo=../upstream-repo"], remote=[], remote_url=[],
        branch=["repo=suite/new"], start=[], override_disk_floor=False,
        representation="config", format="human", wait_lock=0.0,
    )
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        with pytest.raises(Exception):
            wrkslots_cli._cmd_create(args)
    assert list((root / "worktrees").glob("CREATE.*.journal"))


def _python_classify(root: Path, *, fmt: str) -> tuple[int, str]:
    args = argparse.Namespace(
        project_root=str(root), machine="node-a", slot="slot-c",
        slot_type="agent", agent="agent-other", format=fmt,
    )
    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer):
        rc = wrkslots_cli._cmd_classify_create_journals(args)
    return rc, buffer.getvalue()


def _rust_classify(root: Path, *, fmt: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            str(rust_wrkslotsd()), "classify-create-journals", "slot-c",
            "--agent", "agent-other", "--slot-type", "agent",
            "--machine", "node-a", "--root", str(root), "--format", fmt,
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
        env=dict(os.environ),
    )


def test_classify_journals_human_matches_python(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    _interrupted(rust_root)
    _interrupted(oracle_root)
    rc, out = _python_classify(oracle_root, fmt="human")
    assert rc == 0
    result = _rust_classify(rust_root, fmt="human")
    assert result.returncode == 0, result.stderr
    assert _norm(result.stdout, rust_root) == _norm(out, oracle_root)
    assert "live-incomplete-create=1" in result.stdout


def test_classify_journals_json_matches_python(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    _interrupted(rust_root)
    _interrupted(oracle_root)
    rc, out = _python_classify(oracle_root, fmt="json")
    assert rc == 0
    result = _rust_classify(rust_root, fmt="json")
    assert result.returncode == 0, result.stderr
    # Journal identity (device/inode/size/sha) differs per fixture file.
    def scrub(text: str) -> str:
        return re.sub(r'"journal_identity": \{[^}]*\}', '"journal_identity": {}', text)
    assert scrub(_norm(result.stdout, rust_root)) == scrub(_norm(out, oracle_root))


def _write_import_journal(root: Path, owner: dict[str, object]) -> Path:
    active = root / "worktrees" / "ACTIVE.node-a.json"
    document = json.loads(active.read_text(encoding="utf-8"))
    record = document["slots"][0]
    record["owner"] = owner
    record["coordinator_lease"] = owner
    journal = root / "worktrees" / "ACTIVE.node-a.journal"
    journal.write_text(
        json.dumps(
            {
                "schema": 2,
                "kind": "import-existing",
                "machine": "node-a",
                "slot": "slot-a",
                "record": record,
            }
        ),
        encoding="utf-8",
    )
    return journal


def test_live_non_create_journal_is_busy_in_both_editions(tmp_path: Path) -> None:
    rust_root = _transfer_fixture(tmp_path / "rust", None)
    oracle_root = _transfer_fixture(tmp_path / "oracle", None)
    _write_import_journal(rust_root, _live_identity(os.getpid()))
    _write_import_journal(oracle_root, _live_identity(os.getpid()))
    with pytest.raises(wrkslots_cli._JournalBusy) as caught:
        _python_classify(oracle_root, fmt="json")
    assert caught.value.kind == "import-existing"
    assert caught.value.pid == os.getpid()
    assert "busy" in str(caught.value) and "interrupted" not in str(caught.value)
    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer), contextlib.redirect_stderr(io.StringIO()):
        main_rc = wrkslots_cli.main(
            [
                "--project-root", str(oracle_root), "--machine", "node-a",
                "classify-create-journals", "slot-c", "--agent", "agent-other",
                "--slot-type", "agent", "--format", "json",
            ]
        )
    assert main_rc == 75
    assert json.loads(buffer.getvalue())["status"] == "busy"
    result = _rust_classify(rust_root, fmt="json")
    assert result.returncode == 75, result.stderr
    assert "busy" in result.stderr and "import-existing" in result.stderr
    assert str(os.getpid()) in result.stderr
    payload = json.loads(result.stdout)
    assert payload["status"] == "busy"
    assert payload["busy"]["kind"] == "import-existing"
    assert payload["busy"]["pid"] == os.getpid()


def _write_scoped_import_journal(root: Path) -> Path:
    active = root / "worktrees" / "ACTIVE.node-a.json"
    document = json.loads(active.read_text(encoding="utf-8"))
    record = document["slots"][0]
    owner = _live_identity(os.getpid())
    scope_cgroup = "/user.slice/user-1000.slice/user@1000.service/app.slice/wrkslots-recovery.scope"
    owner["cgroup_path"] = scope_cgroup
    record["owner"] = owner
    record["task_scope"] = {
        "unit": "wrkslots-recovery.scope",
        "invocation_id": "a" * 32,
        "cgroup_path": scope_cgroup,
        "boot_id": owner["boot_id"],
        "leader_pid": owner["pid"],
        "leader_start_ticks": owner["start_ticks"],
        "verification": "systemd-runtime-invocation-symlink-v1",
    }
    journal = root / "worktrees" / "ACTIVE.node-a.journal"
    journal.write_text(
        json.dumps(
            {
                "schema": 2,
                "kind": "import-existing",
                "machine": "node-a",
                "slot": "slot-a",
                "record": record,
            }
        ),
        encoding="utf-8",
    )
    return journal


def test_live_pid_with_ended_task_scope_refuses_not_busy(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Unit proof: with the process generation itself live, only the
    # ended task scope stands between busy and the hard refusal.
    owner = _live_identity(os.getpid())
    scope_cgroup = "/user.slice/user-1000.slice/user@1000.service/app.slice/wrkslots-recovery.scope"
    owner["cgroup_path"] = scope_cgroup
    raw: dict[str, object] = {
        "kind": "import-existing",
        "owner": owner,
        "task_scope": {
            "unit": "wrkslots-recovery.scope",
            "invocation_id": "a" * 32,
            "cgroup_path": scope_cgroup,
            "boot_id": owner["boot_id"],
            "leader_pid": owner["pid"],
            "leader_start_ticks": owner["start_ticks"],
            "verification": "systemd-runtime-invocation-symlink-v1",
        },
    }
    monkeypatch.setattr(
        wrkslots_cli, "_process_state", lambda identity: ("live", "live")
    )
    monkeypatch.setattr(
        wrkslots_cli,
        "_verify_task_scope_invocation",
        lambda scope: (_ for _ in ()).throw(
            wrkslots_cli.Refusal("task scope invocation does not match")
        ),
    )
    assert wrkslots_cli._live_journal_owner(raw) is None
    monkeypatch.setattr(
        wrkslots_cli, "_verify_task_scope_invocation", lambda scope: None
    )
    live = wrkslots_cli._live_journal_owner(raw)
    assert live is not None and live.pid == os.getpid()
    monkeypatch.undo()

    rust_root = _transfer_fixture(tmp_path / "rust", None)
    oracle_root = _transfer_fixture(tmp_path / "oracle", None)
    _write_scoped_import_journal(rust_root)
    _write_scoped_import_journal(oracle_root)
    with pytest.raises(wrkslots_cli.Refusal) as caught:
        _python_classify(oracle_root, fmt="json")
    assert not isinstance(caught.value, wrkslots_cli._JournalBusy)
    assert "interrupted" in str(caught.value)
    result = _rust_classify(rust_root, fmt="json")
    assert result.returncode == 1, result.stderr
    assert "interrupted" in result.stderr


def test_dead_non_create_journal_still_refuses_in_both_editions(
    tmp_path: Path,
) -> None:
    rust_root = _transfer_fixture(tmp_path / "rust", None)
    oracle_root = _transfer_fixture(tmp_path / "oracle", None)
    _write_import_journal(rust_root, _dead_identity())
    _write_import_journal(oracle_root, _dead_identity())
    with pytest.raises(wrkslots_cli.Refusal) as caught:
        _python_classify(oracle_root, fmt="json")
    assert not isinstance(caught.value, wrkslots_cli._JournalBusy)
    assert "interrupted" in str(caught.value)
    result = _rust_classify(rust_root, fmt="json")
    assert result.returncode == 1, result.stderr
    assert "interrupted" in result.stderr


def _write_seal(
    root: Path,
    actor: dict[str, object],
    *,
    kind: str = "validate-batch-seal",
    with_target: bool = True,
) -> Path:
    control = root / "worktrees"
    targets: list[dict[str, object]] = []
    if with_target:
        slot_dir = control / "validate" / "target"
        slot_dir.mkdir(parents=True, exist_ok=True)
        info = slot_dir.stat()
        targets = [
            {
                "slot": "target",
                "generation": 1,
                "path": "worktrees/validate/target",
                "original_mode": 0o755,
                "identity": [info.st_dev, info.st_ino, 0o40755],
            }
        ]
    seal = control / "VALIDATE-BATCH-SEAL.node-a.journal"
    seal.write_text(
        json.dumps(
            {
                "schema": 1,
                "kind": kind,
                "machine": "node-a",
                "created_at": "2026-11-07T00:00:00+00:00",
                "actor": actor,
                "targets": targets,
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    return seal


def _python_main_classify(root: Path) -> tuple[int, str, str]:
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        rc = wrkslots_cli.main(
            [
                "--project-root", str(root), "--machine", "node-a",
                "classify-create-journals", "slot-c", "--agent", "agent-other",
                "--slot-type", "agent", "--format", "json",
            ]
        )
    return rc, out.getvalue(), err.getvalue()


def test_classify_live_seal_structured_blocking_reason_matches_python(
    tmp_path: Path,
) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    _write_seal(rust_root, _live_identity(os.getpid()))
    _write_seal(oracle_root, _live_identity(os.getpid()))
    rc, out, err = _python_main_classify(oracle_root)
    assert rc == 3, err
    assert "interrupted validation-batch seal recorded" in err
    result = _rust_classify(rust_root, fmt="json")
    assert result.returncode == 3, result.stderr
    assert "interrupted validation-batch seal recorded" in result.stderr
    assert _norm(result.stdout, rust_root) == _norm(out, oracle_root)
    reasons = json.loads(result.stdout)["blocking_reasons"]
    assert reasons[0]["kind"] == "validate-batch-seal"
    assert reasons[0]["operation"] == "remove-validate-batch"
    assert reasons[0]["actor_state"] == "live"
    assert reasons[0]["targets"] == ["target"]


def test_classify_dead_seal_is_not_blocking_in_both_editions(
    tmp_path: Path,
) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    _write_seal(rust_root, _dead_identity())
    _write_seal(oracle_root, _dead_identity())
    rc, out, err = _python_main_classify(oracle_root)
    assert rc == 0, err
    result = _rust_classify(rust_root, fmt="json")
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["blocking"] is False
    assert "blocking_reasons" not in json.loads(result.stdout)
    assert json.loads(out)["blocking"] is False


def test_classify_unknown_and_other_kind_seals_match_python(
    tmp_path: Path,
) -> None:
    for label, actor, kind, want_kind, want_state in [
        ("unknown", {**_live_identity(os.getpid()), "host_id": "0" * 32},
         "validate-batch-seal", "validate-batch-seal", "unknown"),
        ("other", _live_identity(os.getpid()), "some-other-seal", "other", "unknown"),
    ]:
        rust_root = _fixture(tmp_path / f"rust-{label}")
        oracle_root = _fixture(tmp_path / f"oracle-{label}")
        _write_seal(rust_root, actor, kind=kind)
        _write_seal(oracle_root, actor, kind=kind)
        rc, out, err = _python_main_classify(oracle_root)
        assert rc == 3, (label, err)
        result = _rust_classify(rust_root, fmt="json")
        assert result.returncode == 3, (label, result.stderr)
        assert _norm(result.stdout, rust_root) == _norm(out, oracle_root)
        reason = json.loads(result.stdout)["blocking_reasons"][0]
        assert reason["kind"] == want_kind, label
        assert reason["actor_state"] == want_state, label
def _write_mismatched_import_journal(
    root: Path, owner: dict[str, object]
) -> Path:
    """A standalone journal whose append-only evidence says otherwise.

    The pending operation in the event log carries the same journal with
    one field altered, so the physical journal provably differs from the
    durable evidence: provenance's standalone mismatch (#1033).
    """
    journal = _write_import_journal(root, owner)
    physical = json.loads(journal.read_text(encoding="utf-8"))
    pending = json.loads(json.dumps(physical))
    pending["record"]["purpose"] = "altered after append-only publication"
    config = wrkslots_cli._load_config(str(root), "node-a")
    wrkslots_cli._ensure_event_log(config, "node-a")
    wrkslots_cli._write_event_file(
        config,
        "node-a",
        "operation-progress-recorded",
        {
            "slot": "slot-a",
            "operation": "import-existing",
            "journal": pending,
        },
    )
    return journal


def _hold_writer_lock(root: Path) -> contextlib.AbstractContextManager[None]:
    """Hold a registry's exclusive writer lock as an in-flight operation."""

    @contextlib.contextmanager
    def hold() -> Iterator[None]:
        path = root / "worktrees" / "ACTIVE.global.lock"
        descriptor = os.open(path, os.O_RDWR | os.O_CREAT, 0o600)
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            yield
        finally:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
            os.close(descriptor)

    return hold()


def test_mismatched_standalone_journal_with_live_owner_is_busy_in_both_editions(
    tmp_path: Path,
) -> None:
    # #1033: the reclaim race. A standalone journal that differs from
    # append-only evidence while its writer lock is held is retryable
    # busy, never the hard provenance refusal. A merely live recorded
    # owner would prove nothing: owners routinely outlive operations.
    rust_root = _transfer_fixture(tmp_path / "rust", None)
    oracle_root = _transfer_fixture(tmp_path / "oracle", None)
    _write_mismatched_import_journal(rust_root, _live_identity(os.getpid()))
    _write_mismatched_import_journal(oracle_root, _live_identity(os.getpid()))
    with _hold_writer_lock(oracle_root):
        with pytest.raises(wrkslots_cli._JournalBusy) as caught:
            _python_classify(oracle_root, fmt="json")
    assert caught.value.kind == "import-existing"
    assert caught.value.pid == os.getpid()
    with _hold_writer_lock(rust_root):
        result = _rust_classify(rust_root, fmt="json")
    assert result.returncode == 75, result.stderr
    assert "busy" in result.stderr and "import-existing" in result.stderr
    payload = json.loads(result.stdout)
    assert payload["status"] == "busy"
    assert payload["busy"]["kind"] == "import-existing"
    assert payload["busy"]["pid"] == os.getpid()


def test_mismatched_standalone_journal_with_no_live_operation_still_refuses(
    tmp_path: Path,
) -> None:
    # The same mismatch with nothing running is real corruption.
    rust_root = _transfer_fixture(tmp_path / "rust", None)
    oracle_root = _transfer_fixture(tmp_path / "oracle", None)
    _write_mismatched_import_journal(rust_root, _dead_identity())
    _write_mismatched_import_journal(oracle_root, _dead_identity())
    with pytest.raises(
        wrkslots_cli.StateError, match="differs from append-only"
    ):
        _python_classify(oracle_root, fmt="json")
    result = _rust_classify(rust_root, fmt="json")
    assert result.returncode == 1, result.stderr
    assert "standalone recovery journal differs" in result.stderr
    assert "busy" not in result.stdout


def test_classify_journals_empty_matches_python(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    rc, out = _python_classify(oracle_root, fmt="human")
    assert rc == 0
    result = _rust_classify(rust_root, fmt="human")
    assert result.returncode == 0, result.stderr
    assert _norm(result.stdout, rust_root) == _norm(out, oracle_root)
    assert "journals=0" in result.stdout


def _orphan_archive_claim(root: Path) -> Path:
    """An interrupted create for slot-b plus an archive record for it.

    #1086: the interrupted create alone classifies non-blocking; the
    archive claim on the same slot is the second condition that makes
    classification refuse for every unrelated slot on the host.
    """
    _interrupted(root)
    journal = next((root / "worktrees").glob("CREATE.*.journal"))
    raw = json.loads(journal.read_text(encoding="utf-8"))
    config = wrkslots_cli._load_config(str(root), "node-a")
    finished = "2026-10-26T02:00:00Z"
    entry: dict[str, object] = {
        "archive_id": f"node-a:slot-b:1:{finished}",
        "slot": "slot-b",
        "agent": "agent-new",
        "task": "task-1",
        "purpose": "testing",
        "machine": "node-a",
        "generation": 1,
        "created_at": "2026-10-26T00:00:00Z",
        "finished_at": finished,
        "mode": "remove",
        "actor": "coordinator",
        "physical_storage": "removed",
        "validation": ["archived before the interrupted create"],
        "limitations": [],
        "continuation": "none needed",
        "checkouts": raw["created"],
    }
    archive = wrkslots_cli._load_archive(config)
    wrkslots_cli._write_archive_state(
        config,
        wrkslots_cli.ArchiveState(
            archive.machine, archive.revision + 1, (*archive.records, entry)
        ),
        action="slot-archived",
        slot="slot-b",
        evidence={"archive_id": entry["archive_id"]},
    )
    return journal


def test_orphan_archive_claim_refusal_names_journal_in_both_editions(
    tmp_path: Path,
) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    rust_journal = _orphan_archive_claim(rust_root)
    oracle_journal = _orphan_archive_claim(oracle_root)
    rc, _out, err = _python_main_classify(oracle_root)
    assert rc == 3, err
    for needle in (
        "host fault",
        str(oracle_journal),
        "slot-b",
        "agent-new",
        "conflicts with an archive",
        "wrkslots recover",
        "classify-create-journals",
    ):
        assert needle in err, needle
    assert re.search(r"age=\d+s", err), err
    result = _rust_classify(rust_root, fmt="json")
    assert result.returncode == 1, result.stderr
    for needle in (
        "host fault",
        str(rust_journal),
        "slot-b",
        "agent-new",
        "conflicts with an archive",
        "wrkslots recover",
        "classify-create-journals",
    ):
        assert needle in result.stderr, needle
    assert re.search(r"age=\d+s", result.stderr), result.stderr

    def scrub(text: str, root: Path) -> str:
        text = text.split("host fault:", 1)[-1]
        text = _norm(text.strip(), root).replace('"', "'")
        return re.sub(r"age=\d+s", "age=Ns", text)

    assert scrub(result.stderr, rust_root) == scrub(err.splitlines()[0], oracle_root)
