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


def test_retire_name_at_other_live_pane_refuses(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    # F1: the Herdr name pointing at a different live pane is not stale.
    manager, fake, old = prepare_revive(tmp_path, monkeypatch, harness="claude")
    assert old.pane_id is not None and old.harness_anchor is not None
    object.__setattr__(old.harness_anchor, "boot_id", OLD_BOOT)
    manager._save(old)
    fake.presentations = [item for item in fake.presentations if item.pane_id != old.pane_id]
    from agentctl.client import Pane
    fake.presentations.append(Pane("w9:other", old.tab_id or "w1:t9", "w1"))
    monkeypatch.setattr(fake, "agent_pane", lambda _name: "w9:other")
    with pytest.raises(AgentDeliveryError, match="w9:other"):
        _retire(manager)
    assert (manager._directory("worker") / "agent.json").exists()


def test_revive_name_at_other_live_pane_refuses(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, old = prepare_revive(tmp_path, monkeypatch, harness="claude")
    assert old.pane_id is not None and old.harness_anchor is not None
    object.__setattr__(old.harness_anchor, "boot_id", OLD_BOOT)
    manager._save(old)
    fake.presentations = [item for item in fake.presentations if item.pane_id != old.pane_id]
    from agentctl.client import Pane
    fake.presentations.append(Pane("w9:other", old.tab_id or "w1:t9", "w1"))
    monkeypatch.setattr(fake, "agent_pane", lambda _name: "w9:other")
    with pytest.raises(AgentDeliveryError, match="w9:other"):
        manager.revive("worker", expected_token=old.token)


def test_retire_herdr_unavailable_is_unproven(
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

    def down(**_kwargs):  # type: ignore[no-untyped-def]
        raise HerdrUnavailable("server unavailable")

    monkeypatch.setattr(fake, "panes", down)
    with pytest.raises(AgentDeliveryError):
        _retire(manager)
    assert (manager._directory("worker") / "agent.json").exists()


def test_retire_missing_identity_is_unproven(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    record = manager.get("worker")
    assert record.pane_id is not None
    record.harness_identity = None
    record.custom_process_identity = None
    manager._save(record)
    _make_pane_gone(fake, record.pane_id, monkeypatch)
    with pytest.raises(AgentDeliveryError):
        _retire(manager)
    assert (manager._directory("worker") / "agent.json").exists()


def test_revive_live_session_holder_refuses(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    # F2: a live process carrying the recorded session in its environment.
    import subprocess
    manager, fake, old = prepare_revive(tmp_path, monkeypatch, harness="claude")
    assert old.pane_id is not None and old.harness_anchor is not None
    object.__setattr__(old.harness_anchor, "boot_id", OLD_BOOT)
    token = "panedead-holder-session-1944"
    old.session_value = token
    if old.native_session is not None:
        old.native_session = {**old.native_session, "value": token}
    manager._save(old)
    fake.presentations = [item for item in fake.presentations if item.pane_id != old.pane_id]
    holder = subprocess.Popen(["sleep", "30"], env={"HOLDER_SESSION": token, "PATH": "/usr/bin:/bin"})
    try:
        with pytest.raises(AgentDeliveryError, match="live process"):
            manager.revive("worker", expected_token=old.token)
    finally:
        holder.kill()
        holder.wait()


def test_retire_pane_reporting_recorded_session_refuses(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Item 2: a live pane reporting the recorded session is alive, not stale.
    manager, fake, old = prepare_revive(tmp_path, monkeypatch, harness="claude")
    assert old.pane_id is not None and old.harness_anchor is not None
    object.__setattr__(old.harness_anchor, "boot_id", OLD_BOOT)
    manager._save(old)
    fake.presentations = [item for item in fake.presentations if item.pane_id != old.pane_id]
    from dataclasses import replace as _replace
    from agentctl.client import Pane
    fake.presentations.append(Pane("w9:other", old.tab_id or "w1:t9", "w1"))
    session = old.native_session["value"] if old.native_session else "claude-session"
    fake.infos["w9:other"] = _replace(
        fake.infos[old.pane_id], pane_id="w9:other", agent="claude",
        session_agent="claude", session_value=session,
    )
    monkeypatch.setattr(fake, "agent_pane", lambda _name: "w9:other")
    with pytest.raises(AgentDeliveryError, match="w9:other"):
        _retire(manager)
    assert (manager._directory("worker") / "agent.json").exists()


def test_revive_candidate_holding_session_after_launch_is_not_refused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Item 1: the fake launch itself starts a real --resume holder process.
    import subprocess
    manager, fake, old = prepare_revive(tmp_path, monkeypatch, harness="claude")
    assert old.pane_id is not None and old.harness_anchor is not None
    object.__setattr__(old.harness_anchor, "boot_id", OLD_BOOT)
    manager._save(old)
    fake.presentations = [item for item in fake.presentations if item.pane_id != old.pane_id]
    session = old.native_session["value"] if old.native_session else "claude-session"
    holders: list[subprocess.Popen[bytes]] = []
    original_start = fake.start_agent

    def start_with_holder(*args: object, **kwargs: object) -> None:
        original_start(*args, **kwargs)  # type: ignore[arg-type]
        holders.append(subprocess.Popen(
            ["python3", "-c", "import time; time.sleep(30)", "--resume", session],
        ))

    monkeypatch.setattr(fake, "start_agent", start_with_holder)
    try:
        result = manager.revive("worker", expected_token=old.token)
        assert result["revived"] is True
    finally:
        for holder in holders:
            holder.kill()
            holder.wait()


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
