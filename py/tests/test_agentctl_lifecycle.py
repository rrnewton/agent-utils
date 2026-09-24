"""Regression cases for queue transitions and generation-owned terminal lifecycle."""
from __future__ import annotations

import errno
import fcntl
import json
import os
import stat
import threading
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace
from typing import cast

import pytest

from agentctl import agent
import agentctl.subagents as subagents_module
from agentctl.client import AgentPaneInfo, HerdrClient, Pane, PaneShellProof
from agentctl.errors import AgentDeliveryError, HerdrUnavailable
from agentctl.subagents import ManagedAgents
from .test_agentctl_adopt import prepare_legacy_dead, setup_foreign
from .test_herdr_subagents import FakeManagedClient, setup


def test_start_holds_its_generation_lock_through_brief_and_returned_status(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    original_info = fake.pane_info

    def info(pane: str) -> AgentPaneInfo:
        descriptor = os.open(tmp_path / "registry/.worker.lock", os.O_RDONLY)
        try:
            with pytest.raises(BlockingIOError):
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        finally:
            os.close(descriptor)
        return original_info(pane)

    monkeypatch.setattr(fake, "pane_info", info)
    result = manager.start("worker", cwd=str(tmp_path), brief="this generation's task")
    assert result["lifecycle"] == "running"
    assert fake.submitted == ["this generation's task"]


def test_start_holds_registry_identity_lock_through_native_session_commit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    original_start = fake.start_agent

    def start_agent(
        name: str, kind: str, pane_id: str, arguments: tuple[str, ...], *, timeout: float,
    ) -> None:
        descriptor = os.open(tmp_path / "registry/.identity.lock", os.O_RDONLY)
        try:
            with pytest.raises(BlockingIOError):
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        finally:
            os.close(descriptor)
        original_start(name, kind, pane_id, arguments, timeout=timeout)

    monkeypatch.setattr(fake, "start_agent", start_agent)
    assert manager.start("worker", cwd=str(tmp_path))["lifecycle"] == "running"


def test_start_session_change_leaves_launch_failed_record_stoppable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    original_info = fake.pane_info
    record_path = tmp_path / "registry/worker/agent.json"

    def info(pane: str) -> AgentPaneInfo:
        result = original_info(pane)
        if record_path.exists() and json.loads(record_path.read_text())["lifecycle"] == "running":
            return replace(result, session_value="replacement-session")
        return result

    monkeypatch.setattr(fake, "pane_info", info)
    with pytest.raises(AgentDeliveryError):
        manager.start("worker", cwd=str(tmp_path))
    failed = manager.get("worker")
    assert failed.lifecycle == "launch_failed"
    assert failed.session_agent is None and failed.session_value is None
    assert manager.stop("worker")["pane_closed"] is True


def test_start_refuses_unregistered_same_session_in_another_workspace_and_is_stoppable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    fake.presentations.append(Pane("w2:p1", "w2:t1", "w2"))
    fake.infos["w2:p1"] = AgentPaneInfo(
        "w2:p1", "w2", str(tmp_path), "codex", "idle", "codex", "session-1",
    )
    with pytest.raises(AgentDeliveryError, match="not globally unique"):
        manager.start("worker", cwd=str(tmp_path))
    failed = manager.get("worker")
    assert failed.lifecycle == "launch_failed"
    assert failed.session_value is None
    assert manager.stop("worker")["pane_closed"] is True
    assert fake.presentations == [Pane("w2:p1", "w2:t1", "w2")]


def test_allocation_captures_pane_before_later_launch_failure(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    fake.workspace = "w1"
    fake.fail_start = True
    with pytest.raises(AgentDeliveryError, match="failed"):
        manager.start("worker", cwd=str(tmp_path))
    record = manager.get("worker")
    assert record.tab_id == "w1:t1" and record.pane_id == "w1:p1"
    assert manager.stop("worker")["pane_closed"] is True


@pytest.mark.parametrize("claimed", [False, True])
def test_old_partial_allocation_is_recovered_only_for_the_original_unclaimed_shell(claimed: bool, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    fake.fail_start = True
    with pytest.raises(AgentDeliveryError):
        manager.start("worker", cwd=str(tmp_path))
    manager._save(replace(manager.get("worker"), pane_id=None))
    if claimed:
        fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], agent="claude")
        with pytest.raises(AgentDeliveryError, match="ownership changed"):
            manager.stop("worker")
        assert fake.closed == []
    else:
        result = manager.stop("worker")
        assert result["pane_closed"] is True
        archived = json.loads((Path(str(result["archive"])) / "agent.json").read_text())
        assert archived["pane_id"] == "w1:p1"


def test_stop_preserves_a_sibling_pane_added_during_output_capture(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    original_read = fake.read
    inserted = False

    def read(pane_id: str, *, source: str, lines: int) -> str:
        nonlocal inserted
        if not inserted:
            inserted = True
            fake.presentations.append(Pane("w1:human", "w1:t1", "w1"))
            fake.infos["w1:human"] = replace(fake.infos[pane_id], pane_id="w1:human", agent=None,
                session_agent=None, session_value=None)
        return original_read(pane_id, source=source, lines=lines)

    monkeypatch.setattr(fake, "read", read)
    result = manager.stop("worker")
    assert result["pane_closed"] is True and result["tab_closed"] is False
    assert fake.presentations == [Pane("w1:human", "w1:t1", "w1")]


def make_managed_dead(
    manager: ManagedAgents, fake: FakeManagedClient, tmp_path: Path,
) -> tuple[str, str]:
    started = manager.start("worker", cwd=str(tmp_path))
    pane = str(started["pane_id"])
    fake.infos[pane] = replace(
        fake.infos[pane], agent=None, status="unknown",
        session_agent=None, session_value=None,
    )
    return pane, str(started["token"])


def test_managed_dead_stop_requires_token_then_closes_exact_pane_and_archives(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    pane, token = make_managed_dead(manager, fake, tmp_path)

    with pytest.raises(AgentDeliveryError, match="requires --expected-token"):
        manager.stop("worker")
    result = manager.stop("worker", expected_token=token)

    archive = Path(str(result["archive"]))
    assert result["managed_dead"] is True
    assert result["pane_closed"] is True and result["tab_closed"] is True
    assert fake.closed == ["w1:t1"]
    assert fake.presentations == []
    assert json.loads((archive / "agent.json").read_text())["lifecycle"] == "stopped"
    assert json.loads((archive / "output.json").read_text())["pane_id"] == pane


def test_managed_dead_stop_refuses_expanded_stopped_record_before_close(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    _pane, token = make_managed_dead(manager, fake, tmp_path)
    active = manager.registry / "worker"
    record_path = active / "agent.json"
    document = json.loads(record_path.read_text())
    document["future_padding"] = ["x"] * 150_000
    raw = (json.dumps(document, separators=(",", ":")) + "\n").encode()
    stopped = dict(document)
    stopped["lifecycle"] = "stopped"
    assert len(raw) < subagents_module._MAX_AGENT_RECORD_BYTES
    assert len(agent._json_text(stopped).encode()) > subagents_module._MAX_AGENT_RECORD_BYTES
    record_path.write_bytes(raw)
    record_path.chmod(0o600)

    with pytest.raises(AgentDeliveryError, match="stopped agent record larger"):
        manager.stop("worker", expected_token=token)

    assert fake.closed == []
    assert record_path.read_bytes() == raw
    assert not (active / "output.json").exists()
    assert not (manager.registry / "archive" / f"worker-{token}").exists()


@pytest.mark.parametrize(
    "change",
    ["replacement", "session", "moved", "sibling", "shell", "record", "directory", "capture"],
)
def test_managed_dead_stop_refuses_identity_changes_without_closing_or_archiving(
    change: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    pane, token = make_managed_dead(manager, fake, tmp_path)
    original_read = fake.read
    if change == "session":
        fake.infos[pane] = replace(
            fake.infos[pane], session_agent="codex", session_value="replacement"
        )
    elif change == "moved":
        fake.presentations[0] = replace(fake.presentations[0], tab_id="w1:moved")
    elif change == "sibling":
        fake.presentations.append(Pane("w1:human", "w1:t1", "w1"))
    elif change == "capture":
        def fail_read(_pane_id: str, *, source: str, lines: int) -> str:
            del source, lines
            raise HerdrUnavailable("capture failed")
        monkeypatch.setattr(fake, "read", fail_read)
    else:
        def changing_read(pane_id: str, *, source: str, lines: int) -> str:
            if change == "replacement":
                fake.infos[pane_id] = replace(
                    fake.infos[pane_id], agent="codex", status="idle",
                    session_agent="codex", session_value="new-session",
                )
            elif change == "shell":
                fake.foreign_shell_identity = replace(
                    fake.foreign_shell_identity,
                    starttime_ticks=fake.foreign_shell_identity.starttime_ticks + 1,
                )
            elif change == "directory":
                active = manager.registry / "worker"
                displaced = manager.registry / ".worker-displaced"
                if not displaced.exists():
                    active.rename(displaced)
                    active.mkdir(mode=0o700)
                    (active / "agent.json").write_bytes((displaced / "agent.json").read_bytes())
                    (active / "agent.json").chmod(0o600)
            else:
                path = manager.registry / "worker" / "agent.json"
                document = json.loads(path.read_text())
                document["token"] = "replacement-generation"
                agent._atomic_json(str(path), document)
            return original_read(pane_id, source=source, lines=lines)
        monkeypatch.setattr(fake, "read", changing_read)

    with pytest.raises(AgentDeliveryError):
        manager.stop("worker", expected_token=token)

    assert fake.closed == []
    assert (manager.registry / "worker").is_dir()
    assert not list((manager.registry / "archive").glob("worker-*")) if (
        manager.registry / "archive"
    ).exists() else True


def test_managed_dead_stop_reproves_runtime_after_artifact_preparation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    _pane, token = make_managed_dead(manager, fake, tmp_path)
    record_path = manager.registry / "worker" / "agent.json"
    original_record = record_path.read_bytes()
    original_atomic = ManagedAgents._atomic_snapshot_bytes
    changed = False

    def change_after_output_preparation(
        pinned: subagents_module._PinnedAgentDirectory,
        content: bytes,
        *,
        name: str = "output.json",
    ) -> subagents_module._InstalledArtifact:
        nonlocal changed
        installed = original_atomic(pinned, content, name=name)
        if name == "output.json" and not changed:
            changed = True
            fake.foreign_shell_identity = replace(
                fake.foreign_shell_identity,
                starttime_ticks=fake.foreign_shell_identity.starttime_ticks + 1,
            )
        return installed

    monkeypatch.setattr(
        ManagedAgents,
        "_atomic_snapshot_bytes",
        staticmethod(change_after_output_preparation),
    )
    with pytest.raises(AgentDeliveryError, match="immediately before close"):
        manager.stop("worker", expected_token=token)

    assert changed is True
    assert fake.closed == []
    assert record_path.read_bytes() == original_record
    assert not (manager.registry / "worker" / "output.json").exists()
    assert not list((manager.registry / "archive").glob("worker-*"))


@pytest.mark.parametrize("change", ["record", "directory", "output"])
def test_managed_dead_stop_rechecks_registry_after_artifact_preparation(
    change: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    _pane, token = make_managed_dead(manager, fake, tmp_path)
    record_path = manager.registry / "worker" / "agent.json"
    original_record = record_path.read_bytes()
    original_atomic = ManagedAgents._atomic_snapshot_bytes
    changed = False

    def change_after_output_preparation(
        pinned: subagents_module._PinnedAgentDirectory,
        content: bytes,
        *,
        name: str = "output.json",
    ) -> subagents_module._InstalledArtifact:
        nonlocal changed
        installed = original_atomic(pinned, content, name=name)
        if name != "output.json" or changed:
            return installed
        changed = True
        if change == "record":
            document = json.loads(record_path.read_text())
            document["goal"] = "replacement record"
            agent._atomic_json(str(record_path), document)
        elif change == "directory":
            displaced = manager.registry / ".worker-original"
            (manager.registry / "worker").rename(displaced)
            (manager.registry / "worker").mkdir(mode=0o700)
            replacement = manager.registry / "worker" / "agent.json"
            replacement.write_bytes((displaced / "agent.json").read_bytes())
            replacement.chmod(0o600)
        else:
            output = manager.registry / "worker" / "output.json"
            output.rename(manager.registry / "worker" / ".prepared-output")
            output.write_bytes(b"replacement output")
            output.chmod(0o600)
        return installed

    monkeypatch.setattr(
        ManagedAgents,
        "_atomic_snapshot_bytes",
        staticmethod(change_after_output_preparation),
    )
    with pytest.raises(AgentDeliveryError):
        manager.stop("worker", expected_token=token)

    assert changed is True
    assert fake.closed == []
    assert not list((manager.registry / "archive").glob("worker-*"))
    if change == "record":
        assert json.loads(record_path.read_text())["goal"] == "replacement record"
        assert record_path.read_bytes() != original_record
        assert not (manager.registry / "worker" / "output.json").exists()
    elif change == "directory":
        displaced = manager.registry / ".worker-original"
        assert (displaced / "agent.json").read_bytes() == original_record
        assert not (displaced / "output.json").exists()
        assert (manager.registry / "worker").is_dir()
    else:
        assert record_path.read_bytes() == original_record
        assert (manager.registry / "worker" / "output.json").read_bytes() == (
            b"replacement output"
        )


@pytest.mark.parametrize("change", ["record", "directory"])
def test_managed_dead_stop_rechecks_registry_during_final_record_proof(
    change: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    _pane, token = make_managed_dead(manager, fake, tmp_path)
    active = manager.registry / "worker"
    record_path = active / "agent.json"
    original_record = record_path.read_bytes()
    original_record_bytes = manager._record_bytes
    changed = False

    def record_bytes(
        pinned: subagents_module._PinnedAgentDirectory,
        *, require_active_name: bool = True,
    ) -> bytes:
        nonlocal changed
        if not changed and (active / "output.json").exists():
            changed = True
            if change == "record":
                document = json.loads(record_path.read_text())
                document["goal"] = "replacement after proof"
                agent._atomic_json(str(record_path), document)
            else:
                displaced = manager.registry / ".worker-original"
                active.rename(displaced)
                active.mkdir(mode=0o700)
                replacement = active / "agent.json"
                replacement.write_bytes((displaced / "agent.json").read_bytes())
                replacement.chmod(0o600)
        return original_record_bytes(
            pinned, require_active_name=require_active_name,
        )

    monkeypatch.setattr(manager, "_record_bytes", record_bytes)
    with pytest.raises(AgentDeliveryError):
        manager.stop("worker", expected_token=token)

    assert changed is True
    assert fake.closed == []
    if change == "record":
        assert json.loads(record_path.read_text())["goal"] == "replacement after proof"
        assert not (active / "output.json").exists()
    else:
        displaced = manager.registry / ".worker-original"
        assert (displaced / "agent.json").read_bytes() == original_record
        assert not (displaced / "output.json").exists()
        assert active.is_dir()


@pytest.mark.parametrize(
    ("change", "expected_error"),
    [
        ("sibling", "recorded tab is not the exact one-pane tab"),
        ("agent", "pane still reports agent 'codex'"),
    ],
)
def test_managed_dead_stop_rechecks_pane_after_final_record_proof(
    change: str, expected_error: str,
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    pane, token = make_managed_dead(manager, fake, tmp_path)
    active = manager.registry / "worker"
    original_record_bytes = manager._record_bytes
    changed = False

    def record_bytes(
        pinned: subagents_module._PinnedAgentDirectory,
        *, require_active_name: bool = True,
    ) -> bytes:
        nonlocal changed
        result = original_record_bytes(
            pinned, require_active_name=require_active_name,
        )
        if not changed and (active / "output.json").exists():
            changed = True
            if change == "sibling":
                presentation = next(
                    item for item in fake.presentations if item.pane_id == pane
                )
                sibling = Pane("w1:human", presentation.tab_id, presentation.workspace_id)
                fake.presentations.append(sibling)
                fake.infos[sibling.pane_id] = replace(
                    fake.infos[pane], pane_id=sibling.pane_id,
                )
            else:
                fake.infos[pane] = replace(fake.infos[pane], agent="codex")
        return result

    monkeypatch.setattr(manager, "_record_bytes", record_bytes)
    with pytest.raises(AgentDeliveryError, match=expected_error):
        manager.stop("worker", expected_token=token)

    assert changed is True
    assert fake.closed == []
    assert active.is_dir()
    assert not (active / "output.json").exists()
    assert not list((manager.registry / "archive").glob("worker-*"))


@pytest.mark.parametrize(
    ("change", "expected_error"),
    [
        ("sibling", "recorded tab is not the exact one-pane tab"),
        ("agent", "pane still reports agent 'codex'"),
    ],
)
def test_managed_dead_stop_rechecks_pane_after_final_shell_proof(
    change: str, expected_error: str,
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    pane, token = make_managed_dead(manager, fake, tmp_path)
    active = manager.registry / "worker"
    original_shell_proof = fake.pane_idle_shell_identity
    changed = False

    def shell_proof(pane_id: str) -> PaneShellProof | None:
        nonlocal changed
        result = original_shell_proof(pane_id)
        if not changed and (active / "output.json").exists():
            changed = True
            if change == "sibling":
                presentation = next(
                    item for item in fake.presentations if item.pane_id == pane
                )
                sibling = Pane("w1:human", presentation.tab_id, presentation.workspace_id)
                fake.presentations.append(sibling)
                fake.infos[sibling.pane_id] = replace(
                    fake.infos[pane], pane_id=sibling.pane_id,
                )
            else:
                fake.infos[pane] = replace(fake.infos[pane], agent="codex")
        return result

    monkeypatch.setattr(fake, "pane_idle_shell_identity", shell_proof)
    with pytest.raises(AgentDeliveryError, match=expected_error):
        manager.stop("worker", expected_token=token)

    assert changed is True
    assert fake.closed == []
    assert active.is_dir()
    assert not (active / "output.json").exists()
    assert not list((manager.registry / "archive").glob("worker-*"))


def test_managed_dead_stop_retries_after_close_result_is_uncertain(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    _pane, token = make_managed_dead(manager, fake, tmp_path)
    original_close = fake.close_pane
    failed = False

    def close_then_fail(pane_id: str) -> None:
        nonlocal failed
        original_close(pane_id)
        if not failed:
            failed = True
            raise HerdrUnavailable("injected result loss after close")

    monkeypatch.setattr(fake, "close_pane", close_then_fail)
    with pytest.raises(HerdrUnavailable, match="result loss after close"):
        manager.stop("worker", expected_token=token)

    active = manager.registry / "worker"
    assert json.loads((active / "agent.json").read_text())["lifecycle"] == "running"
    assert not fake.presentations
    monkeypatch.setattr(fake, "close_pane", original_close)
    result = manager.stop("worker", expected_token=token)
    archive = Path(str(result["archive"]))
    assert not active.exists()
    assert json.loads((archive / "agent.json").read_text())["lifecycle"] == "stopped"


def test_managed_dead_stop_preflights_archive_and_snapshot_failures_before_close(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    for failure in ("collision", "snapshot"):
        root = tmp_path / failure
        root.mkdir()
        manager, fake = setup(root, monkeypatch)
        _pane, token = make_managed_dead(manager, fake, root)
        if failure == "collision":
            destination = manager.registry / "archive" / f"worker-{token}"
            destination.mkdir(parents=True)
        else:
            original_atomic = ManagedAgents._atomic_snapshot_bytes
            failed = False

            def fail_output(
                pinned: subagents_module._PinnedAgentDirectory,
                content: bytes, *, name: str = "output.json",
            ) -> subagents_module._InstalledArtifact:
                nonlocal failed
                if name == "output.json" and not failed:
                    failed = True
                    raise AgentDeliveryError("injected snapshot failure")
                return original_atomic(pinned, content, name=name)

            monkeypatch.setattr(
                ManagedAgents, "_atomic_snapshot_bytes", staticmethod(fail_output)
            )
        with pytest.raises(AgentDeliveryError):
            manager.stop("worker", expected_token=token)
        assert fake.closed == []
        assert (manager.registry / "worker").is_dir()


def test_managed_dead_stop_refuses_stopping_record_without_prior_proof(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    _pane, token = make_managed_dead(manager, fake, tmp_path)
    path = manager.registry / "worker" / "agent.json"
    document = json.loads(path.read_text())
    document["lifecycle"] = "stopping"
    agent._atomic_json(str(path), document)

    with pytest.raises(AgentDeliveryError, match="requires a running herdr record"):
        manager.stop("worker", expected_token=token)
    assert fake.closed == []
    assert path.is_file()


@pytest.mark.parametrize(("field", "value"), [("mode", "headless"), ("backend", "tmux")])
def test_managed_dead_stop_refuses_crossed_mode_or_backend(
    field: str, value: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    _pane, token = make_managed_dead(manager, fake, tmp_path)
    path = manager.registry / "worker" / "agent.json"
    document = json.loads(path.read_text())
    document[field] = value
    agent._atomic_json(str(path), document)

    with pytest.raises(AgentDeliveryError):
        manager.stop("worker", expected_token=token)
    assert fake.closed == []
    assert path.is_file()


def test_managed_dead_stop_rename_race_preserves_truthful_stopped_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    _pane, token = make_managed_dead(manager, fake, tmp_path)
    real_rename = subagents_module._rename_directory_noreplace_at

    def collide(
        source_parent: int, source_name: str,
        destination_parent: int, destination_name: str,
    ) -> None:
        if destination_name.startswith("worker-"):
            destination = manager.registry / "archive" / destination_name
            destination.symlink_to(tmp_path / "missing-archive-target", target_is_directory=True)
        real_rename(source_parent, source_name, destination_parent, destination_name)

    monkeypatch.setattr(subagents_module, "_rename_directory_noreplace_at", collide)
    with pytest.raises(AgentDeliveryError, match="existing agent archive"):
        manager.stop("worker", expected_token=token)

    active = manager.registry / "worker"
    assert fake.closed == ["w1:t1"]
    assert json.loads((active / "agent.json").read_text())["lifecycle"] == "stopped"
    assert (active / "output.json").is_file()
    assert os.path.lexists(manager.registry / "archive" / f"worker-{token}")


@pytest.mark.parametrize(
    "failing_label", ["published agent archive", "published agent registry"],
)
def test_managed_dead_stop_fsync_failure_attempts_both_and_leaves_one_complete_archive(
    failing_label: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    _pane, token = make_managed_dead(manager, fake, tmp_path)
    real_fsync = subagents_module._fsync_pinned_directory
    calls: list[str] = []

    def fail_one(descriptor: int, label: str) -> None:
        calls.append(label)
        if label == failing_label:
            raise OSError("injected publication fsync failure")
        real_fsync(descriptor, label)

    monkeypatch.setattr(subagents_module, "_fsync_pinned_directory", fail_one)
    with pytest.raises(AgentDeliveryError, match="published.*durability is uncertain"):
        manager.stop("worker", expected_token=token)
    destination = manager.registry / "archive" / f"worker-{token}"
    assert fake.closed == ["w1:t1"]
    assert not (manager.registry / "worker").exists()
    assert json.loads((destination / "agent.json").read_text())["lifecycle"] == "stopped"
    assert (destination / "output.json").is_file()
    assert calls[-2:] == ["published agent archive", "published agent registry"]


@pytest.mark.parametrize("target_kind", ["agent", "parent"])
@pytest.mark.parametrize("replacement", ["private", "public", "missing"])
def test_directory_pin_rejects_validate_to_open_replacement(
    target_kind: str, replacement: str,
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    prepare_legacy_dead(manager, fake, pane)
    if target_kind == "agent":
        target = manager.registry / "foreign"
    else:
        target = manager.registry / "archive"
        target.mkdir(mode=0o700)
    displaced = target.with_name(f".{target.name}-original")
    real_open = os.open
    swapped = False

    def open_after_swap(path: os.PathLike[str] | str, flags: int, *args: object,
                        **kwargs: object) -> int:
        nonlocal swapped
        if not swapped and Path(path) == target and kwargs.get("dir_fd") is None:
            swapped = True
            target.rename(displaced)
            if replacement != "missing":
                target.mkdir(mode=0o700)
                target.chmod(0o700 if replacement == "private" else 0o755)
        return real_open(path, flags, *args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr("agentctl.subagents.os.open", open_after_swap)
    with pytest.raises(AgentDeliveryError):
        if target_kind == "agent":
            with manager._pinned_agent_directory("foreign"):
                pytest.fail("replacement directory was pinned")
        else:
            with manager._pinned_parent_directory(target, label="agent archive"):
                pytest.fail("replacement directory was pinned")

    assert swapped is True
    assert displaced.is_dir()
    if replacement == "missing":
        assert not target.exists()
    else:
        assert target.is_dir()


@pytest.mark.parametrize("target_kind", ["agent", "parent"])
def test_pinned_directory_recheck_refuses_postopen_permission_change(
    target_kind: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    prepare_legacy_dead(manager, fake, pane)
    if target_kind == "agent":
        target = manager.registry / "foreign"
        with manager._pinned_agent_directory("foreign") as pinned:
            target.chmod(0o755)
            with pytest.raises(AgentDeliveryError):
                manager._verify_pinned_agent_directory(pinned)
    else:
        target = manager.registry / "archive"
        target.mkdir(mode=0o700)
        with manager._pinned_parent_directory(
            target, label="agent archive",
        ) as pinned:
            target.chmod(0o755)
            with pytest.raises(AgentDeliveryError):
                manager._verify_pinned_parent_directory(
                    pinned, label="agent archive",
                )


def test_atomic_snapshot_close_failure_is_delivery_and_retains_installed_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    prepare_legacy_dead(manager, fake, pane)
    active = manager.registry / "foreign"
    real_close = os.close
    injected = False

    with manager._pinned_agent_directory("foreign") as pinned:
        def fail_staging_close(descriptor: int) -> None:
            nonlocal injected
            if not injected and descriptor != pinned.descriptor:
                injected = True
                real_close(descriptor)
                raise OSError(errno.EIO, "injected staging close failure")
            real_close(descriptor)

        monkeypatch.setattr("agentctl.subagents.os.close", fail_staging_close)

        with pytest.raises(AgentDeliveryError) as raised:
            manager._atomic_snapshot_bytes(pinned, b'{"new":true}\n')

    assert raised.value.exit_code == 75
    assert "registry artifact cleanup failed" in str(raised.value)
    assert injected is True
    assert (active / "output.json").read_bytes() == b'{"new":true}\n'


def test_atomic_snapshot_never_unlinks_reused_temp_name_after_install(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    prepare_legacy_dead(manager, fake, pane)
    active = manager.registry / "foreign"
    real_replace = os.replace
    reused: Path | None = None

    def install_then_reuse(
        source: os.PathLike[str] | str,
        destination: os.PathLike[str] | str,
        *args: object,
        **kwargs: object,
    ) -> None:
        nonlocal reused
        real_replace(source, destination, *args, **kwargs)  # type: ignore[arg-type]
        if str(source).startswith(".output.json-recovery-"):
            reused = active / str(source)
            reused.write_bytes(b"replacement owner")
            reused.chmod(0o600)

    monkeypatch.setattr("agentctl.subagents.os.replace", install_then_reuse)
    with manager._pinned_agent_directory("foreign") as pinned:
        manager._atomic_snapshot_bytes(pinned, b'{"new":true}\n')

    assert (active / "output.json").read_bytes() == b'{"new":true}\n'
    assert reused is not None and reused.read_bytes() == b"replacement owner"


def test_atomic_snapshot_reports_primary_and_cleanup_failures(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    prepare_legacy_dead(manager, fake, pane)
    real_unlink = os.unlink

    def fail_write(_descriptor: int, _content: bytes) -> int:
        raise OSError(errno.ENOSPC, "injected write failure")

    def fail_cleanup_unlink(
        path: os.PathLike[str] | str, *args: object, **kwargs: object,
    ) -> None:
        if str(path).startswith(".output.json-recovery-"):
            raise OSError(errno.EIO, "injected cleanup failure")
        real_unlink(path, *args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr("agentctl.subagents.os.write", fail_write)
    monkeypatch.setattr("agentctl.subagents.os.unlink", fail_cleanup_unlink)
    with manager._pinned_agent_directory("foreign") as pinned:
        with pytest.raises(AgentDeliveryError) as raised:
            manager._atomic_snapshot_bytes(pinned, b'{"new":true}\n')

    message = str(raised.value)
    assert raised.value.exit_code == 75
    assert "cannot write pinned registry artifact" in message
    assert "registry artifact cleanup failed" in message
    assert not (manager.registry / "foreign" / "output.json").exists()


@pytest.mark.parametrize("collision", ["file", "symlink"])
def test_atomic_snapshot_never_unlinks_unowned_staging_collision(
    collision: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    prepare_legacy_dead(manager, fake, pane)
    nonce = "0" * 32
    temporary = (
        manager.registry / "foreign"
        / f".output.json-recovery-{os.getpid()}-{nonce}"
    )
    if collision == "file":
        temporary.write_bytes(b"sentinel")
        temporary.chmod(0o600)
    else:
        victim = tmp_path / "sentinel-target"
        victim.write_bytes(b"sentinel")
        temporary.symlink_to(victim)
    monkeypatch.setattr(
        "agentctl.subagents.uuid.uuid4", lambda: SimpleNamespace(hex=nonce),
    )

    with manager._pinned_agent_directory("foreign") as pinned:
        with pytest.raises(AgentDeliveryError, match="cannot write pinned"):
            manager._atomic_snapshot_bytes(pinned, b'{"new":true}\n')

    assert os.path.lexists(temporary)
    if collision == "file":
        assert temporary.read_bytes() == b"sentinel"
    else:
        assert temporary.is_symlink()


def test_atomic_snapshot_refuses_replaced_owned_staging_name_and_preserves_replacement(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    prepare_legacy_dead(manager, fake, pane)
    active = manager.registry / "foreign"
    held = active / ".held-original-staging"
    content = b'{"new":true}\n'
    real_write = os.write
    injected = False

    def swap_after_write(descriptor: int, data: bytes) -> int:
        nonlocal injected
        written = real_write(descriptor, data)
        if not injected:
            injected = True
            temporary = next(active.glob(".output.json-recovery-*"))
            temporary.rename(held)
            temporary.write_bytes(b"x" * len(content))
            temporary.chmod(0o600)
        return written

    monkeypatch.setattr("agentctl.subagents.os.write", swap_after_write)
    with manager._pinned_agent_directory("foreign") as pinned:
        with pytest.raises(
            AgentDeliveryError, match="staging generation changed",
        ) as raised:
            manager._atomic_snapshot_bytes(pinned, content)

    assert "replacement was preserved" in str(raised.value)
    assert injected is True
    assert held.read_bytes() == content
    replacement = next(active.glob(".output.json-recovery-*"))
    assert replacement.read_bytes() == b"x" * len(content)
    assert not (active / "output.json").exists()


def test_atomic_snapshot_reproves_installed_generation_and_preserves_new_temp_name(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    prepare_legacy_dead(manager, fake, pane)
    active = manager.registry / "foreign"
    held = active / ".held-installed-output"
    content = b'{"new":true}\n'
    real_replace = os.replace
    stale_temp: Path | None = None

    def replace_then_swap_installed(
        source: os.PathLike[str] | str,
        destination: os.PathLike[str] | str,
        *args: object,
        **kwargs: object,
    ) -> None:
        nonlocal stale_temp
        real_replace(source, destination, *args, **kwargs)  # type: ignore[arg-type]
        if str(source).startswith(".output.json-recovery-"):
            (active / str(destination)).rename(held)
            (active / str(destination)).write_bytes(b"replacement")
            (active / str(destination)).chmod(0o600)
            stale_temp = active / str(source)
            stale_temp.write_bytes(b"new-temp-owner")
            stale_temp.chmod(0o600)

    monkeypatch.setattr("agentctl.subagents.os.replace", replace_then_swap_installed)
    with manager._pinned_agent_directory("foreign") as pinned:
        with pytest.raises(
            AgentDeliveryError, match="was not the staged generation",
        ):
            manager._atomic_snapshot_bytes(pinned, content)

    assert held.read_bytes() == content
    assert (active / "output.json").read_bytes() == b"replacement"
    assert stale_temp is not None and stale_temp.read_bytes() == b"new-temp-owner"


@pytest.mark.parametrize("phase", ["before-rename", "after-rename"])
def test_legacy_recovery_never_restores_over_replaced_artifact_generations(
    phase: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    token, digest, raw = prepare_legacy_dead(manager, fake, pane)
    active = manager.registry / "foreign"
    prior = b'{"prior":true}\n'
    (active / "output.json").write_bytes(prior)
    (active / "output.json").chmod(0o600)
    held = active / f".held-recovery-{phase}"
    injected = False
    if phase == "before-rename":
        real_write = os.write

        def swap_staging_after_write(descriptor: int, data: bytes) -> int:
            nonlocal injected
            written = real_write(descriptor, data)
            if not injected:
                injected = True
                temporary = next(active.glob(".output.json-recovery-*"))
                temporary.rename(held)
                temporary.write_bytes(b"replacement owner")
                temporary.chmod(0o600)
            return written

        monkeypatch.setattr("agentctl.subagents.os.write", swap_staging_after_write)
    else:
        real_replace = os.replace

        def swap_installed_after_replace(
            source: os.PathLike[str] | str,
            destination: os.PathLike[str] | str,
            *args: object,
            **kwargs: object,
        ) -> None:
            nonlocal injected
            real_replace(source, destination, *args, **kwargs)  # type: ignore[arg-type]
            if not injected and str(source).startswith(".output.json-recovery-"):
                injected = True
                (active / str(destination)).rename(held)
                (active / str(destination)).write_bytes(b"replacement owner")
                (active / str(destination)).chmod(0o600)

        monkeypatch.setattr("agentctl.subagents.os.replace", swap_installed_after_replace)

    with pytest.raises(AgentDeliveryError, match="generation|replacement"):
        manager.stop(
            "foreign",
            expected_token=token,
            recover_legacy_adoption=True,
            expected_record_sha256=digest,
        )

    assert injected is True
    assert fake.closed == []
    assert (active / "agent.json").read_bytes() == raw
    assert not (manager.registry / "archive" / f"foreign-{token}").exists()
    if phase == "before-rename":
        assert (active / "output.json").read_bytes() == prior
    else:
        assert (active / "output.json").read_bytes() == b"replacement owner"
        assert held.is_file()


@pytest.mark.parametrize("phase", ["before-rename", "after-rename"])
def test_managed_dead_preparation_never_restores_over_replaced_artifact_generations(
    phase: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    _pane, token = make_managed_dead(manager, fake, tmp_path)
    active = manager.registry / "worker"
    prior = b'{"prior":true}\n'
    (active / "output.json").write_bytes(prior)
    (active / "output.json").chmod(0o600)
    held = active / f".held-managed-{phase}"
    injected = False
    if phase == "before-rename":
        real_write = os.write

        def swap_staging_after_write(descriptor: int, data: bytes) -> int:
            nonlocal injected
            written = real_write(descriptor, data)
            if not injected:
                injected = True
                temporary = next(active.glob(".output.json-recovery-*"))
                temporary.rename(held)
                temporary.write_bytes(b"replacement owner")
                temporary.chmod(0o600)
            return written

        monkeypatch.setattr("agentctl.subagents.os.write", swap_staging_after_write)
    else:
        real_replace = os.replace

        def swap_installed_after_replace(
            source: os.PathLike[str] | str,
            destination: os.PathLike[str] | str,
            *args: object,
            **kwargs: object,
        ) -> None:
            nonlocal injected
            real_replace(source, destination, *args, **kwargs)  # type: ignore[arg-type]
            if not injected and str(source).startswith(".output.json-recovery-"):
                injected = True
                (active / str(destination)).rename(held)
                (active / str(destination)).write_bytes(b"replacement owner")
                (active / str(destination)).chmod(0o600)

        monkeypatch.setattr("agentctl.subagents.os.replace", swap_installed_after_replace)

    with pytest.raises(AgentDeliveryError, match="generation|replacement"):
        manager.stop("worker", expected_token=token)

    assert injected is True
    assert fake.closed == []
    assert (active / "agent.json").is_file()
    assert not (manager.registry / "archive" / f"worker-{token}").exists()
    if phase == "before-rename":
        assert (active / "output.json").read_bytes() == prior
    else:
        assert (active / "output.json").read_bytes() == b"replacement owner"
        assert held.is_file()


@pytest.mark.parametrize("record_read_fails", [False, True])
def test_publication_reconciles_successful_rename_reported_as_error(
    record_read_fails: bool, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    token, _digest, raw = prepare_legacy_dead(manager, fake, pane)
    active = manager.registry / "foreign"
    (active / "output.json").write_bytes(b'{"prior":true}\n')
    (active / "output.json").chmod(0o600)
    archive = manager.registry / "archive"
    archive.mkdir(mode=0o700)
    destination = archive / f"foreign-{token}"
    real_rename = subagents_module._rename_directory_noreplace_at

    def rename_then_report_error(
        source_parent: int, source_name: str,
        destination_parent: int, destination_name: str,
    ) -> None:
        real_rename(source_parent, source_name, destination_parent, destination_name)
        if record_read_fails:
            (destination / "agent.json").unlink()
            (destination / "agent.json").symlink_to(tmp_path / "missing-record")
        raise AgentDeliveryError("injected lost successful rename result")

    monkeypatch.setattr(
        subagents_module, "_rename_directory_noreplace_at", rename_then_report_error,
    )
    with manager._pinned_agent_directory("foreign") as pinned:
        with pytest.raises(AgentDeliveryError) as raised:
            manager._publish_archive(
                pinned,
                destination,
                {"text": "new retained output"},
                expected_record=raw,
                before_publish=lambda: None,
            )

    assert "reported failure" in str(raised.value)
    if record_read_fails:
        assert "record could not be proved" in str(raised.value)
    else:
        assert "proved generation was published" in str(raised.value)
    assert not active.exists()
    assert json.loads((destination / "output.json").read_text())["text"] == (
        "new retained output"
    )


def test_publication_reconciles_late_destination_collision_and_restores_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    token, _digest, raw = prepare_legacy_dead(manager, fake, pane)
    active = manager.registry / "foreign"
    prior = b'{"prior":true}\n'
    (active / "output.json").write_bytes(prior)
    (active / "output.json").chmod(0o600)
    archive = manager.registry / "archive"
    archive.mkdir(mode=0o700)
    destination = archive / f"foreign-{token}"

    def create_late_collision() -> None:
        destination.mkdir(mode=0o700)
        (destination / "sentinel").write_bytes(b"other owner")

    with manager._pinned_agent_directory("foreign") as pinned:
        with pytest.raises(AgentDeliveryError, match="existing agent archive"):
            manager._publish_archive(
                pinned,
                destination,
                {"text": "new retained output"},
                expected_record=raw,
                before_publish=create_late_collision,
            )

    assert active.is_dir()
    assert (active / "output.json").read_bytes() == prior
    assert (destination / "sentinel").read_bytes() == b"other owner"


@pytest.mark.parametrize("already_uncertain", [False, True])
def test_publication_parent_close_failure_retains_published_output(
    already_uncertain: bool, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    token, _digest, raw = prepare_legacy_dead(manager, fake, pane)
    active = manager.registry / "foreign"
    (active / "output.json").write_bytes(b'{"prior":true}\n')
    (active / "output.json").chmod(0o600)
    archive = manager.registry / "archive"
    archive.mkdir(mode=0o700)
    destination = archive / f"foreign-{token}"
    archive_identity = (archive.stat().st_dev, archive.stat().st_ino)
    real_close = os.close
    injected = False

    def fail_archive_parent_close(descriptor: int) -> None:
        nonlocal injected
        try:
            identity = (os.fstat(descriptor).st_dev, os.fstat(descriptor).st_ino)
        except OSError:
            identity = (-1, -1)
        real_close(descriptor)
        if not injected and identity == archive_identity:
            injected = True
            raise OSError(errno.EIO, "injected archive-parent close failure")

    monkeypatch.setattr("agentctl.subagents.os.close", fail_archive_parent_close)
    if already_uncertain:
        monkeypatch.setattr(
            subagents_module,
            "_fsync_pinned_directory",
            lambda _descriptor, _label: (_ for _ in ()).throw(
                OSError(errno.EIO, "injected publication fsync failure")
            ),
        )
    with manager._pinned_agent_directory("foreign") as pinned:
        with pytest.raises(AgentDeliveryError, match="published|publication state"):
            manager._publish_archive(
                pinned,
                destination,
                {"text": "new retained output"},
                expected_record=raw,
                before_publish=lambda: None,
            )

    assert injected is True
    assert not active.exists()
    assert json.loads((destination / "output.json").read_text())["text"] == (
        "new retained output"
    )


def test_publication_refuses_reappeared_source_without_mutating_published_archive(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    token, _digest, raw = prepare_legacy_dead(manager, fake, pane)
    active = manager.registry / "foreign"
    archive = manager.registry / "archive"
    archive.mkdir(mode=0o700)
    destination = archive / f"foreign-{token}"
    real_rename = subagents_module._rename_directory_noreplace_at

    def reappear_after_publish(
        source_parent: int, source_name: str,
        destination_parent: int, destination_name: str,
    ) -> None:
        real_rename(source_parent, source_name, destination_parent, destination_name)
        if destination_name == destination.name:
            active.mkdir(mode=0o700)
            (active / "agent.json").write_bytes(raw)
            (active / "agent.json").chmod(0o600)

    monkeypatch.setattr(
        subagents_module, "_rename_directory_noreplace_at", reappear_after_publish,
    )
    with manager._pinned_agent_directory("foreign") as pinned:
        with pytest.raises(AgentDeliveryError, match="rollback was not attempted"):
            manager._publish_archive(
                pinned,
                destination,
                {"text": "new retained output"},
                expected_record=raw,
                before_publish=lambda: None,
            )

    assert active.is_dir()
    assert destination.is_dir()
    assert json.loads((destination / "output.json").read_text())["text"] == (
        "new retained output"
    )


def test_publication_refuses_postrename_permission_change(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    token, _digest, raw = prepare_legacy_dead(manager, fake, pane)
    archive = manager.registry / "archive"
    archive.mkdir(mode=0o700)
    destination = archive / f"foreign-{token}"
    real_rename = subagents_module._rename_directory_noreplace_at

    def make_published_generation_public(
        source_parent: int, source_name: str,
        destination_parent: int, destination_name: str,
    ) -> None:
        real_rename(source_parent, source_name, destination_parent, destination_name)
        if destination_name == destination.name:
            destination.chmod(0o755)

    monkeypatch.setattr(
        subagents_module,
        "_rename_directory_noreplace_at",
        make_published_generation_public,
    )
    with manager._pinned_agent_directory("foreign") as pinned:
        with pytest.raises(
            AgentDeliveryError, match="became unsafe.*rollback was not attempted",
        ):
            manager._publish_archive(
                pinned,
                destination,
                {"text": "new retained output"},
                expected_record=raw,
                before_publish=lambda: None,
            )

    assert not (manager.registry / "foreign").exists()
    assert stat.S_IMODE(destination.stat().st_mode) == 0o755
    assert json.loads((destination / "output.json").read_text())["text"] == (
        "new retained output"
    )


def test_publication_maps_postrename_stat_failure_to_delivery_and_rolls_back(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    token, _digest, raw = prepare_legacy_dead(manager, fake, pane)
    archive = manager.registry / "archive"
    archive.mkdir(mode=0o700)
    destination = archive / f"foreign-{token}"
    real_stat = os.stat
    injected = False

    def fail_published_stat_once(
        path: os.PathLike[str] | str | int, *args: object, **kwargs: object,
    ) -> os.stat_result:
        nonlocal injected
        if (not injected and path == destination.name
                and kwargs.get("dir_fd") is not None):
            injected = True
            raise OSError(errno.EIO, "injected post-publication stat failure")
        return real_stat(path, *args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr("agentctl.subagents.os.stat", fail_published_stat_once)
    with manager._pinned_agent_directory("foreign") as pinned:
        with pytest.raises(
            AgentDeliveryError, match="cannot inspect published agent directory",
        ):
            manager._publish_archive(
                pinned,
                destination,
                {"text": "new output"},
                expected_record=raw,
                before_publish=lambda: None,
            )

    assert injected is True
    assert (manager.registry / "foreign").is_dir()
    assert not destination.exists()
    assert not (manager.registry / "foreign" / "output.json").exists()


def test_publication_never_promotes_replaced_archive_entry_during_rollback(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    token, _digest, raw = prepare_legacy_dead(manager, fake, pane)
    archive = manager.registry / "archive"
    archive.mkdir(mode=0o700)
    active = manager.registry / "foreign"
    destination = archive / f"foreign-{token}"
    displaced = archive / ".held-original"
    real_rename = subagents_module._rename_directory_noreplace_at

    def replace_destination_after_publish(
        source_parent: int, source_name: str,
        destination_parent: int, destination_name: str,
    ) -> None:
        real_rename(source_parent, source_name, destination_parent, destination_name)
        if destination_name == destination.name:
            destination.rename(displaced)
            destination.mkdir(mode=0o700)
            (destination / "attacker-marker").write_text("replacement")

    monkeypatch.setattr(
        subagents_module,
        "_rename_directory_noreplace_at",
        replace_destination_after_publish,
    )
    with manager._pinned_agent_directory("foreign") as pinned:
        with pytest.raises(AgentDeliveryError, match="destination was replaced"):
            manager._publish_archive(
                pinned,
                destination,
                {"text": "new retained output"},
                expected_record=raw,
                before_publish=lambda: None,
            )

    assert not active.exists()
    assert (destination / "attacker-marker").read_text() == "replacement"
    assert json.loads((displaced / "output.json").read_text())["text"] == (
        "new retained output"
    )


def test_publication_reproves_generation_after_inverse_rollback(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    token, _digest, raw = prepare_legacy_dead(manager, fake, pane)
    archive = manager.registry / "archive"
    archive.mkdir(mode=0o700)
    active = manager.registry / "foreign"
    destination = archive / f"foreign-{token}"
    displaced = manager.registry / ".foreign-after-rollback"
    real_rename = subagents_module._rename_directory_noreplace_at

    def move_generation_after_inverse_rename(
        source_parent: int, source_name: str,
        destination_parent: int, destination_name: str,
    ) -> None:
        real_rename(source_parent, source_name, destination_parent, destination_name)
        if source_name == destination.name and destination_name == "foreign":
            active.rename(displaced)

    monkeypatch.setattr(
        subagents_module,
        "_rename_directory_noreplace_at",
        move_generation_after_inverse_rename,
    )
    with manager._pinned_agent_directory("foreign") as pinned:
        with pytest.raises(
            AgentDeliveryError, match="rollback state could not be proved",
        ):
            manager._publish_archive(
                pinned,
                destination,
                {"text": "new retained output"},
                expected_record=raw + b"mismatch",
                before_publish=lambda: None,
            )

    assert not active.exists()
    assert not destination.exists()
    assert json.loads((displaced / "output.json").read_text())["text"] == (
        "new retained output"
    )


def test_publication_refuses_replaced_archive_parent_and_rolls_back(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    token, _digest, raw = prepare_legacy_dead(manager, fake, pane)
    active = manager.registry / "foreign"
    archive = manager.registry / "archive"
    archive.mkdir(mode=0o700)
    displaced_archive = manager.registry / ".archive-original"
    destination = archive / f"foreign-{token}"
    real_rename = subagents_module._rename_directory_noreplace_at

    def swap_archive_parent_after_publish(
        source_parent: int, source_name: str,
        destination_parent: int, destination_name: str,
    ) -> None:
        real_rename(source_parent, source_name, destination_parent, destination_name)
        if destination_name == destination.name:
            archive.rename(displaced_archive)
            archive.mkdir(mode=0o700)

    monkeypatch.setattr(
        subagents_module,
        "_rename_directory_noreplace_at",
        swap_archive_parent_after_publish,
    )
    with manager._pinned_agent_directory("foreign") as pinned:
        with pytest.raises(AgentDeliveryError, match="agent archive directory changed"):
            manager._publish_pinned_directory(
                pinned, destination, expected_record=raw,
            )

    assert active.is_dir()
    assert not destination.exists()
    assert not (displaced_archive / destination.name).exists()


@pytest.mark.parametrize(
    "failing_label", ["agent archive rollback", "agent registry rollback"],
)
def test_publication_reports_each_rollback_fsync_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, failing_label: str,
) -> None:
    manager, fake, pane = setup_foreign(tmp_path, monkeypatch)
    token, _digest, raw = prepare_legacy_dead(manager, fake, pane)
    archive = manager.registry / "archive"
    archive.mkdir(mode=0o700)
    destination = archive / f"foreign-{token}"
    real_fsync = subagents_module._fsync_pinned_directory
    real_record_bytes = manager._record_bytes
    injected = False
    calls: list[str] = []

    def fail_published_record_proof_once(
        pinned: subagents_module._PinnedAgentDirectory,
        *, require_active_name: bool = True,
    ) -> bytes:
        nonlocal injected
        if not require_active_name and not injected:
            injected = True
            raise AgentDeliveryError("injected transient publication proof failure")
        return real_record_bytes(pinned, require_active_name=require_active_name)

    def fail_one_parent(descriptor: int, label: str) -> None:
        calls.append(label)
        if label == failing_label:
            raise OSError("injected rollback fsync failure")
        real_fsync(descriptor, label)

    monkeypatch.setattr(manager, "_record_bytes", fail_published_record_proof_once)
    monkeypatch.setattr(subagents_module, "_fsync_pinned_directory", fail_one_parent)
    with manager._pinned_agent_directory("foreign") as pinned:
        with pytest.raises(
            AgentDeliveryError,
            match=rf"rollback completed.*durability is uncertain.*{failing_label}",
        ):
            manager._publish_archive(
                pinned,
                destination,
                {"text": "new retained output"},
                expected_record=raw,
                before_publish=lambda: None,
            )

    assert injected is True
    assert calls[-2:] == ["agent archive rollback", "agent registry rollback"]
    assert (manager.registry / "foreign").is_dir()
    assert not destination.exists()
    assert json.loads(
        (manager.registry / "foreign" / "output.json").read_text()
    )["text"] == "new retained output"


def test_managed_dead_publication_refuses_postproof_directory_swap(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    _pane, token = make_managed_dead(manager, fake, tmp_path)
    real_rename = subagents_module._rename_directory_noreplace_at
    swapped = False

    def swap_before_publish(
        source_parent: int, source_name: str,
        destination_parent: int, destination_name: str,
    ) -> None:
        nonlocal swapped
        if not swapped and destination_name.startswith("worker-"):
            swapped = True
            displaced = manager.registry / ".worker-original"
            active = manager.registry / source_name
            active.rename(displaced)
            active.mkdir(mode=0o700)
            (active / "agent.json").write_bytes((displaced / "agent.json").read_bytes())
            (active / "agent.json").chmod(0o600)
        real_rename(source_parent, source_name, destination_parent, destination_name)

    monkeypatch.setattr(subagents_module, "_rename_directory_noreplace_at", swap_before_publish)
    with pytest.raises(AgentDeliveryError, match="published directory was not"):
        manager.stop("worker", expected_token=token)
    destination = manager.registry / "archive" / f"worker-{token}"
    displaced = manager.registry / ".worker-original"
    assert fake.closed == ["w1:t1"]
    assert not (manager.registry / "worker").exists()
    assert (destination / "agent.json").is_file()
    assert json.loads((destination / "agent.json").read_text())["lifecycle"] == "stopped"
    assert (displaced / "agent.json").is_file()
    assert not (destination / "output.json").exists()
    assert (displaced / "output.json").is_file()


def test_postclose_probe_failure_does_not_prevent_archival(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    original_close = fake.close_pane

    def close(pane: str) -> None:
        original_close(pane)
        fake.offline = True

    monkeypatch.setattr(fake, "close_pane", close)
    result = manager.stop("worker")
    assert result["pane_closed"] is True and result["tab_closed"] is None
    assert Path(str(result["archive"])).is_dir()


def test_explicit_send_serializes_id_reservation_against_delivery_transitions(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    fake = FakeManagedClient()
    manager = ManagedAgents(cast(HerdrClient, fake), tmp_path / "registry")
    monkeypatch.delenv("HERDR_WORKSPACE_ID", raising=False)
    manager.start("worker", cwd=str(tmp_path))
    queue = str(tmp_path / "queue")
    target = manager.get("worker").target()
    agent._bind_queue(queue, target)
    lock = agent._open_private_lock(str(Path(queue) / ".delivery.lock"), "test transaction")
    entered, wrote = threading.Event(), threading.Event()
    original_enqueue, original_create = agent._enqueue, agent._atomic_json_create

    def enqueue(*args: object, **kwargs: object) -> str:
        entered.set()
        return original_enqueue(*args, **kwargs)  # type: ignore[arg-type]

    def create(path: str, document: dict[str, object]) -> None:
        original_create(path, document)
        if path.endswith("inbox/reused-id.json"):
            wrote.set()

    monkeypatch.setattr(agent, "_enqueue", enqueue)
    monkeypatch.setattr(agent, "_atomic_json_create", create)
    errors: list[BaseException] = []

    def sender() -> None:
        try:
            agent.send(cast(HerdrClient, fake), target, queue, "new task", message_id="reused-id")
        except BaseException as exc:
            errors.append(exc)

    fcntl.flock(lock, fcntl.LOCK_EX)
    thread = threading.Thread(target=sender)
    thread.start()
    try:
        assert entered.wait(2)
        assert not wrote.wait(0.1), "explicit ID escaped the queue transition lock"
        agent._atomic_json(str(Path(queue) / "processed/reused-id.json"),
            {"id": "reused-id", "text": "previously submitted", "queued_at": 0, "delivery_attempts": 1})
    finally:
        os.close(lock)
        thread.join(2)
    assert not thread.is_alive()
    assert len(errors) == 1 and "already exists" in str(errors[0])
    assert fake.submitted == []


def test_unknown_metadata_survives_ownership_and_session_updates_without_a_wrapper(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, _ = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    path = tmp_path / "registry/worker/agent.json"
    record = json.loads(path.read_text())
    record["future_metadata"] = {"nested": [1, "preserve"]}
    agent._atomic_json(str(path), record)
    manager.pause("worker")
    manager.bind_session("worker", "session-1")
    saved = json.loads(path.read_text())
    assert saved["future_metadata"] == record["future_metadata"]
    assert "_unknown" not in saved
    assert manager.status("worker")["future_metadata"] == record["future_metadata"]
    invalid = manager.get("worker")
    invalid._unknown["paused"] = False
    with pytest.raises(AgentDeliveryError, match="conflicts"):
        manager._save(invalid)


def test_stop_refuses_native_identity_replacement_during_output_capture(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    original_read = fake.read

    def read(pane_id: str, *, source: str, lines: int) -> str:
        monkeypatch.setattr(fake, "agent_pane", lambda _: "w1:replacement")
        return original_read(pane_id, source=source, lines=lines)

    monkeypatch.setattr(fake, "read", read)
    with pytest.raises(HerdrUnavailable, match="no longer owns"):
        manager.stop("worker")
    assert not fake.closed
    assert manager.get("worker").lifecycle == "running"
