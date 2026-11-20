"""#1944: previous-boot proof for retire/revive with a stale Herdr agent object."""
from __future__ import annotations

import hashlib
from pathlib import Path

import pytest

from agentctl.errors import AgentDeliveryError, HerdrUnavailable
from agentctl.subagents import ManagedAgents
from .test_agentctl_lifecycle import prepare_revive
from .test_herdr_subagents import FakeManagedClient, setup

OLD_BOOT = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"


def _current_boot() -> str:
    return Path("/proc/sys/kernel/random/boot_id").read_text(encoding="ascii").strip()


def _make_pane_gone(fake: FakeManagedClient, pane: str, monkeypatch: pytest.MonkeyPatch) -> None:
    fake.presentations = [item for item in fake.presentations if item.pane_id != pane]

    def info(pane_id: str):  # type: ignore[no-untyped-def]
        if pane_id == pane:
            raise HerdrUnavailable('pane get: {"error":{"code":"pane_not_found"}}')
        return FakeManagedClient.pane_info(fake, pane_id)

    monkeypatch.setattr(fake, "pane_info", info)


def _retire(manager: ManagedAgents, name: str = "worker"):  # type: ignore[no-untyped-def]
    path = manager._directory(name) / "agent.json"
    record = manager.get(name)
    return manager.stop(
        name, expected_token=record.token, retire_dead_native=True,
        expected_record_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
    )


def test_retire_previous_boot_missing_pane_stale_agent_archives(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    record = manager.get("worker")
    assert record.pane_id is not None
    mutated = manager.get("worker")
    assert mutated.harness_anchor is not None
    object.__setattr__(mutated.harness_anchor, "boot_id", OLD_BOOT)
    manager._save(mutated)
    _make_pane_gone(fake, record.pane_id, monkeypatch)
    # agent_pane still resolves: the stale Herdr agent object remains.
    assert fake.agent_pane("worker") == record.pane_id
    result = _retire(manager)
    assert result["retired_dead_native"] is True
    assert not (manager._directory("worker") / "agent.json").exists()


def test_retire_same_boot_missing_pane_refuses_with_remedy(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    record = manager.get("worker")
    assert record.pane_id is not None and record.harness_anchor is not None
    object.__setattr__(record.harness_anchor, "boot_id", _current_boot())
    manager._save(record)
    _make_pane_gone(fake, record.pane_id, monkeypatch)
    with pytest.raises(AgentDeliveryError, match="remedy"):
        _retire(manager)
    assert (manager._directory("worker") / "agent.json").exists()


def test_revive_previous_boot_missing_pane_revives(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, old = prepare_revive(tmp_path, monkeypatch, harness="claude")
    assert old.pane_id is not None and old.harness_anchor is not None
    object.__setattr__(old.harness_anchor, "boot_id", OLD_BOOT)
    manager._save(old)
    fake.presentations = [item for item in fake.presentations if item.pane_id != old.pane_id]
    result = manager.revive("worker", expected_token=old.token)
    assert result["revived"] is True


def test_revive_same_boot_missing_pane_refuses_with_remedy(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, old = prepare_revive(tmp_path, monkeypatch, harness="claude")
    assert old.pane_id is not None and old.harness_anchor is not None
    object.__setattr__(old.harness_anchor, "boot_id", _current_boot())
    manager._save(old)
    fake.presentations = [item for item in fake.presentations if item.pane_id != old.pane_id]
    # Same boot and the harness process is still alive: pane loss alone proves nothing.
    monkeypatch.setattr(fake, "process_liveness", lambda _expected: "alive", raising=False)
    with pytest.raises(AgentDeliveryError, match="remedy"):
        manager.revive("worker", expected_token=old.token)
