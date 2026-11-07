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
import io
import json
import os
import re
import subprocess
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


def test_classify_journals_empty_matches_python(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    rc, out = _python_classify(oracle_root, fmt="human")
    assert rc == 0
    result = _rust_classify(rust_root, fmt="human")
    assert result.returncode == 0, result.stderr
    assert _norm(result.stdout, rust_root) == _norm(out, oracle_root)
    assert "journals=0" in result.stdout
