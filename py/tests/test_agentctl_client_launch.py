"""Exact Herdr allocation commands used by first-class agentctl launches."""
from __future__ import annotations

import json
import subprocess
from collections.abc import Sequence

from agentctl.client import HerdrClient


class Runner:
    def __init__(self, response: dict[str, object]) -> None:
        self.response = response
        self.calls: list[list[str]] = []

    def __call__(self, command: Sequence[str]) -> subprocess.CompletedProcess[str]:
        self.calls.append(list(command))
        return subprocess.CompletedProcess(
            command, 0, json.dumps({"result": self.response}), ""
        )


_ENVIRONMENT = (
    "META_CODEX_AI_GATEWAY=azure-codex-cyber:openai",
    "LITERAL=a b=$(unexpanded)=tail",
)


def test_workspace_creation_passes_literal_environment_before_launch() -> None:
    runner = Runner({
        "workspace": {"workspace_id": "w1"},
        "tab": {"tab_id": "t1"},
        "root_pane": {"pane_id": "p1"},
    })
    client = HerdrClient(herdr_bin="fixture-herdr", run=runner)
    assert client.create_workspace(
        label="subagents", cwd="/work/project", environment=_ENVIRONMENT
    ) == ("w1", "t1", "p1")
    assert runner.calls == [[
        "fixture-herdr", "workspace", "create", "--label", "subagents",
        "--cwd", "/work/project", "--env", _ENVIRONMENT[0],
        "--env", _ENVIRONMENT[1], "--no-focus",
    ]]


def test_tab_creation_passes_literal_environment_before_launch() -> None:
    runner = Runner({
        "tab": {"tab_id": "t1"},
        "root_pane": {"pane_id": "p1", "tab_id": "t1", "workspace_id": "w1"},
    })
    client = HerdrClient(herdr_bin="fixture-herdr", run=runner)
    assert client.create_tab_with_pane(
        workspace_id="w1", label="worker", cwd="/work/project",
        environment=_ENVIRONMENT,
    ) == ("t1", "p1")
    assert runner.calls == [[
        "fixture-herdr", "tab", "create", "--workspace", "w1",
        "--label", "worker", "--cwd", "/work/project",
        "--env", _ENVIRONMENT[0], "--env", _ENVIRONMENT[1], "--no-focus",
    ]]
