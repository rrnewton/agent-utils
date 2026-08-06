"""Idempotent bring-up and session-cache tests.

"Idempotent" is asserted by counting the CREATE calls across repeated runs, not by checking that
the second run returns the same ids — identical ids with a second workspace quietly created behind
them would pass the weaker check.
"""

from __future__ import annotations

import json
import os
import threading
from typing import cast

import pytest

import herdr_run.state as state
from herdr_run.client import HerdrClient
from herdr_run.config import Config
from herdr_run.errors import ConfigError, HerdrUnavailable
from herdr_run.session import cache_path, resolve_target, tab_label_for
from tests.herdr_fake import FakeHerdrClient


@pytest.fixture(autouse=True)
def _isolated_account_state(tmp_path: object, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(state, "_account_home", lambda: os.path.join(str(tmp_path), "account-home"))


def _config(tmp_path: str, **kwargs: object) -> Config:
    base: dict[str, object] = {"project_root": tmp_path, "spool_dir": "spool"}
    base.update(kwargs)
    return Config(**base)  # type: ignore[arg-type]


def _client(fake: FakeHerdrClient) -> HerdrClient:
    # The fake implements the methods the session layer uses; the cast keeps the production
    # signature honest without a runtime dependency on a live server.
    return cast(HerdrClient, fake)


def test_first_run_creates_workspace_and_renames_the_default_tab(tmp_path: object) -> None:
    root = str(tmp_path)
    fake = FakeHerdrClient()
    target = resolve_target(_client(fake), _config(root), "hermit-coord")

    assert target.created == ("workspace", "tab")
    assert target.tab_label == "hermit-coord"
    # The default tab is RENAMED, not supplemented: a fresh workspace must end with exactly one tab.
    workspace = fake.workspaces[target.workspace_id]
    assert len(workspace.tabs) == 1
    assert workspace.tabs[target.tab_id].label == "hermit-coord"


def test_second_run_creates_nothing(tmp_path: object) -> None:
    root = str(tmp_path)
    fake = FakeHerdrClient()
    config = _config(root)
    first = resolve_target(_client(fake), config, "hermit-coord")
    creates_after_first = [call for call in fake.calls if call.startswith("create_")]

    second = resolve_target(_client(fake), config, "hermit-coord")

    assert second.pane_id == first.pane_id
    assert second.created == ()
    assert second.from_cache is True
    assert [call for call in fake.calls if call.startswith("create_")] == creates_after_first

    path = cache_path(config)
    assert os.stat(os.path.dirname(path)).st_mode & 0o777 == 0o700
    assert os.stat(path).st_mode & 0o777 == 0o600
    assert os.stat(f"{path}.lock").st_mode & 0o777 == 0o600


def test_second_agent_reuses_the_workspace_and_adds_only_a_tab(tmp_path: object) -> None:
    root = str(tmp_path)
    fake = FakeHerdrClient()
    config = _config(root)
    first = resolve_target(_client(fake), config, "hermit-coord")
    second = resolve_target(_client(fake), config, "hermit-lander")

    assert second.workspace_id == first.workspace_id
    assert second.created == ("tab",)
    assert second.pane_id != first.pane_id
    assert len(fake.workspaces) == 1


def test_partial_state_is_completed_not_recreated(tmp_path: object) -> None:
    """Workspace exists, tab does not: only the tab is created."""
    root = str(tmp_path)
    fake = FakeHerdrClient()
    fake.create_workspace(label="agent-cmds", cwd=root)
    target = resolve_target(_client(fake), _config(root), "hermit-dbi")
    assert target.created == ("tab",)
    assert len(fake.workspaces) == 1


def test_starts_the_server_when_absent(tmp_path: object) -> None:
    fake = FakeHerdrClient(server_running=False)
    target = resolve_target(_client(fake), _config(str(tmp_path)), "agent")
    assert fake.started_server is True
    assert "server" in target.created


def test_concurrent_first_resolution_across_projects_creates_session_once(
    tmp_path: object,
) -> None:
    """The account-global resolution lock closes the lookup-then-create race."""

    class BlockingFirstCreate(FakeHerdrClient):
        def __init__(self) -> None:
            super().__init__()
            self.entered = threading.Event()
            self.release = threading.Event()
            self._count_lock = threading.Lock()
            self.create_count = 0

        def create_workspace(self, *, label: str, cwd: str) -> tuple[str, str, str]:
            with self._count_lock:
                self.create_count += 1
                ordinal = self.create_count
            if ordinal == 1:
                self.entered.set()
                if not self.release.wait(timeout=5.0):
                    raise AssertionError("test did not release the first workspace creation")
            return super().create_workspace(label=label, cwd=cwd)

    root = str(tmp_path)
    project_one = os.path.join(root, "project-one")
    project_two = os.path.join(root, "project-two")
    os.makedirs(project_one)
    os.makedirs(project_two)
    configs = (_config(project_one), _config(project_two))
    fake = BlockingFirstCreate()
    targets: list[object] = []
    failures: list[BaseException] = []

    def resolve(config: Config) -> None:
        try:
            targets.append(resolve_target(_client(fake), config, "agent"))
        except BaseException as exc:
            failures.append(exc)

    first = threading.Thread(target=resolve, args=(configs[0],), daemon=True)
    first.start()
    assert fake.entered.wait(timeout=5.0), "first resolver never reached workspace creation"
    second = threading.Thread(target=resolve, args=(configs[1],), daemon=True)
    second.start()
    try:
        second.join(timeout=0.1)
        assert second.is_alive(), "second resolver bypassed the account-global session lock"
    finally:
        fake.release.set()
        first.join(timeout=5.0)
        second.join(timeout=5.0)

    assert failures == []
    assert not first.is_alive() and not second.is_alive()
    assert fake.create_count == 1
    assert len(fake.workspaces) == 1
    assert len(targets) == 2


# --- the cache is an optimisation, never an authority --------------------------------------------


def test_cache_is_rejected_when_the_pane_is_gone(tmp_path: object) -> None:
    root = str(tmp_path)
    fake = FakeHerdrClient()
    config = _config(root)
    first = resolve_target(_client(fake), config, "agent")

    # Simulate the human closing the tab out from under us.
    workspace = fake.workspaces[first.workspace_id]
    workspace.tabs.clear()

    second = resolve_target(_client(fake), config, "agent")
    assert second.from_cache is False
    assert second.created == ("tab",)


def test_cache_is_rejected_when_the_tab_label_moved(tmp_path: object) -> None:
    """A cached pane whose tab was renamed must not be typed into: it is somebody else's tab now."""
    root = str(tmp_path)
    fake = FakeHerdrClient()
    config = _config(root)
    first = resolve_target(_client(fake), config, "agent")
    fake.rename_tab(first.tab_id, "someone-elses-tab")

    second = resolve_target(_client(fake), config, "agent")
    assert second.from_cache is False
    assert second.tab_id != first.tab_id


def test_cache_is_rejected_when_the_workspace_label_moved(tmp_path: object) -> None:
    """A live pane id is not authority when its workspace has become somebody else's."""
    root = str(tmp_path)
    fake = FakeHerdrClient()
    config = _config(root)
    first = resolve_target(_client(fake), config, "agent")
    fake.workspaces[first.workspace_id].label = "someone-elses-workspace"

    second = resolve_target(_client(fake), config, "agent")

    assert second.from_cache is False
    assert second.workspace_id != first.workspace_id
    assert fake.workspaces[second.workspace_id].label == config.workspace
    assert fake.workspaces[first.workspace_id].label == "someone-elses-workspace"


def test_corrupt_cache_is_ignored_rather_than_fatal(tmp_path: object) -> None:
    root = str(tmp_path)
    config = _config(root)
    path = cache_path(config)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write("{not json at all")

    target = resolve_target(_client(FakeHerdrClient()), config, "agent")
    assert target.from_cache is False
    # And it is rewritten as valid JSON.
    with open(path, encoding="utf-8") as handle:
        assert isinstance(json.load(handle), dict)


def test_invalid_utf8_cache_is_ignored_rather_than_escaping(tmp_path: object) -> None:
    root = str(tmp_path)
    config = _config(root)
    path = cache_path(config)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(b"\xff\xfe")

    target = resolve_target(_client(FakeHerdrClient()), config, "agent")

    assert target.from_cache is False
    with open(path, encoding="utf-8") as handle:
        assert isinstance(json.load(handle), dict)


def test_no_cache_forces_re_resolution(tmp_path: object) -> None:
    root = str(tmp_path)
    fake = FakeHerdrClient()
    config = _config(root)
    resolve_target(_client(fake), config, "agent")
    second = resolve_target(_client(fake), config, "agent", use_cache=False)
    assert second.from_cache is False
    assert second.created == ()  # re-resolved from labels, nothing new created


# --- ambiguity is refused, not guessed ------------------------------------------------------------


def test_split_tab_is_refused(tmp_path: object) -> None:
    root = str(tmp_path)
    fake = FakeHerdrClient()
    config = _config(root)
    target = resolve_target(_client(fake), config, "agent")
    fake.workspaces[target.workspace_id].tabs[target.tab_id].pane_ids.append("extra:p9")

    with pytest.raises(HerdrUnavailable, match="unsplit tab"):
        resolve_target(_client(fake), config, "agent", use_cache=False)


# --- tab naming ------------------------------------------------------------------------------------


def test_tab_label_default_is_the_agent_name() -> None:
    assert tab_label_for(Config(project_root="/tmp/dev-hermit"), "hermit-coord") == "hermit-coord"


def test_tab_label_schema_supports_project_placeholder() -> None:
    config = Config(tab_name="{project}-{agent}", project_root="/tmp/dev-hermit")
    assert tab_label_for(config, "kvm") == "dev-hermit-kvm"


def test_unknown_placeholder_is_a_clear_error() -> None:
    config = Config(tab_name="{nope}")
    with pytest.raises(ConfigError, match=r"plain \{agent\} and \{project\}"):
        tab_label_for(config, "agent")
