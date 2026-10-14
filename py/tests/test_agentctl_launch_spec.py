"""Launch specs: written at start, read from the pinned process at adopt and anchor, replayed by revive."""
from __future__ import annotations

import json
from dataclasses import replace
from pathlib import Path
from typing import Literal, cast

import pytest

from agentctl import launch_spec, profiles
from agentctl.client import AgentPaneInfo, CustomProcessIdentity, HerdrClient
from agentctl.errors import AgentDeliveryError, HerdrUnavailable
from agentctl.sessions import Sessions
from agentctl.subagents import AgentRecord, wrkslots_executable
from .test_herdr_subagents import FakeManagedClient, setup

CODEX = "/usr/local/bin/codex_cli/codex"
CLAUDE = "/usr/local/bin/claude_code/claude"


def foreign(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, *, harness: str = "codex",
    reported: str | None = None, command_line: tuple[str, ...] | None = None,
) -> tuple[Sessions, FakeManagedClient, str]:
    """One hand-started harness in a one-pane tab, with its pinned process's command line."""
    monkeypatch.delenv("HERDR_WORKSPACE_ID", raising=False)
    fake = FakeManagedClient()
    fake.workspace = "w1"
    fake.create_tab(workspace_id="w1", label="foreign", cwd=str(tmp_path))
    pane = fake.presentations[-1].pane_id
    fake.infos[pane] = AgentPaneInfo(
        pane, "w1", str(tmp_path), harness, "idle",
        harness if reported else None, reported, terminal_id="term-1", tab_id=fake.presentations[-1].tab_id,
    )
    fake.harness_pids[pane] = 500
    if command_line is not None:
        fake.command_lines[500] = command_line
    return Sessions(cast(HerdrClient, fake), tmp_path / "registry"), fake, pane


def adopt(sessions: Sessions, pane: str, cwd: Path, harness: str = "codex") -> dict[str, object]:
    return sessions.adopt("lead", pane_id=pane, expected_workspace="subagents",
                          expected_cwd=str(cwd), harness=harness)


def kill(fake: FakeManagedClient, monkeypatch: pytest.MonkeyPatch, record: AgentRecord) -> None:
    """The harness exits and its pane returns to an idle shell, as after a reboot."""
    process = record.harness_anchor
    assert process is not None and record.pane_id is not None

    def liveness(expected: CustomProcessIdentity) -> Literal["alive", "dead", "unknown"]:
        return "dead" if expected == process else "alive"

    def agent_pane(name: str) -> str:
        matches = [entry[2] for entry in fake.launched if entry[0] == name]
        if not matches:
            raise HerdrUnavailable('agent get: {"error":{"code":"agent_not_found"}}')
        return matches[-1]

    monkeypatch.setattr(fake, "process_liveness", liveness, raising=False)
    monkeypatch.setattr(fake, "agent_pane", agent_pane)
    fake.infos[record.pane_id] = replace(fake.infos[record.pane_id], agent=None, status="unknown",
                                         session_agent=None, session_value=None)


def test_start_records_the_exact_launch_before_herdr_reports_back(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    fake.fail_start = True
    with pytest.raises(AgentDeliveryError):
        manager.start("worker", cwd=str(tmp_path), harness="claude", model="chosen",
                      harness_args=("--dangerously-skip-permissions",))
    record = manager.get("worker")
    spec = record.launch_spec
    assert record.lifecycle == "launch_failed" and spec is not None
    assert spec["source"] == "start" and spec["executable"] is None
    assert spec["argv"] == record.arguments == [
        "--session-id", cast(dict[str, str], record.native_session)["value"],
        "--model", "chosen", "--dangerously-skip-permissions",
    ]
    assert spec["native_session"] == {
        "agent": "claude", "value": cast(dict[str, str], record.native_session)["value"], "source": "asserted",
    }
    assert (spec["cwd"], spec["profile"], spec["workspace_label"]) == (str(tmp_path), None, None)


@pytest.mark.parametrize("reported", [None, "herdr-conversation"])
def test_adopt_reads_the_pinned_command_line_and_its_conversation(
    reported: str | None, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, _fake, pane = foreign(tmp_path, monkeypatch, reported=reported, command_line=(
        CODEX, "resume", "argv-conversation", "--no-alt-screen", "--dangerously-bypass-approvals-and-sandbox",
    ))
    result = adopt(sessions, pane, tmp_path)
    spec = cast(dict[str, object], result["launch_spec"])
    assert spec["source"] == "adopt" and spec["executable"] == CODEX
    assert spec["argv"] == ["resume", "argv-conversation", "--no-alt-screen",
                            "--dangerously-bypass-approvals-and-sandbox"]
    assert spec["workspace_label"] == "subagents" and spec["environment_names"] == []
    assert spec["native_session"] == (
        {"agent": "codex", "value": "argv-conversation", "source": "argv"} if reported is None
        else {"agent": "codex", "value": reported, "source": "observed"}
    )
    assert "revive" in cast(list[str], result["capabilities"])


def test_adopt_without_a_readable_command_line_records_no_spec_and_cannot_revive(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, pane = foreign(tmp_path, monkeypatch, harness="claude")
    result = adopt(sessions, pane, tmp_path, harness="claude")
    assert "launch_spec" not in result and "revive" not in cast(list[str], result["capabilities"])
    record = sessions.get("lead")
    kill(fake, monkeypatch, record)
    plan = sessions.revive("lead", dry_run=True)
    assert plan["action"] == "blocked"
    assert "agentctl anchor lead --replace" in cast(str, plan["reason"])
    with pytest.raises(AgentDeliveryError, match="no launch spec"):
        sessions.revive("lead")
    assert fake.launched == []


def test_anchor_records_a_caller_conversation_and_refuses_a_contradiction(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, pane = foreign(tmp_path, monkeypatch, harness="claude")
    adopt(sessions, pane, tmp_path, harness="claude")
    fake.command_lines[500] = (CLAUDE, "--dangerously-skip-permissions", "--resume", "launched-as")
    result = sessions.anchor("lead", session="after-clear")
    assert result["launch_spec_source"] == "anchor"
    spec = sessions.get("lead").launch_spec
    assert spec is not None and spec["native_session"] == {
        "agent": "claude", "value": "after-clear", "source": "caller",
    }
    other_root = tmp_path / "other"
    other_root.mkdir()
    other, _other_fake, other_pane = foreign(other_root, monkeypatch, harness="claude", reported="reported",
                                             command_line=(CLAUDE, "--resume", "reported"))
    adopt(other, other_pane, other_root, harness="claude")
    before = other.get("lead").launch_spec
    with pytest.raises(AgentDeliveryError, match="contradicts the session Herdr reports"):
        other.anchor("lead", session="different")
    assert other.get("lead").launch_spec == before


def test_anchor_drops_a_stale_spec_when_the_replaced_harness_is_unreadable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, pane = foreign(tmp_path, monkeypatch, command_line=(CODEX, "resume", "one"))
    adopt(sessions, pane, tmp_path)
    assert sessions.get("lead").launch_spec is not None
    fake.harness_pids[pane] = 501
    result = sessions.anchor("lead", replace=True)
    assert result["replaced"] is True and result["launch_spec_source"] is None
    assert sessions.get("lead").launch_spec is None
    with pytest.raises(AgentDeliveryError, match="--session-id cannot be recorded"):
        sessions.anchor("lead", session="named")


def test_revive_replays_an_adopted_spec_as_an_owned_agent_and_leaves_the_old_pane(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, pane = foreign(tmp_path, monkeypatch, command_line=(
        CODEX, "resume", "lead-conversation", "--no-alt-screen", "--model", "gpt",
        "--dangerously-bypass-approvals-and-sandbox",
    ))
    adopt(sessions, pane, tmp_path)
    old = sessions.get("lead")
    kill(fake, monkeypatch, old)
    plan = sessions.revive("lead", dry_run=True)
    assert plan["action"] == "revive", plan
    result = sessions.revive("lead")
    expected = ("resume", "lead-conversation", "--no-alt-screen", "--model", "gpt",
                "--dangerously-bypass-approvals-and-sandbox")
    assert fake.launched[-1][0:2] == ("lead", "codex") and fake.launched[-1][3] == expected
    new = sessions.get("lead")
    assert new.adapter == "herdr" and new.arguments == list(expected) and new.resume == "lead-conversation"
    assert new.foreign_shell_identity is None and new.harness_anchor is not None
    spec = new.launch_spec
    assert spec is not None and spec["source"] == "revive" and spec["argv"] == list(expected)
    assert spec["workspace_label"] == "subagents"
    assert result["pane_closed"] is False and result["tab_closed"] is False and fake.closed == []
    archived = json.loads((Path(str(result["archive"])) / "agent.json").read_text())
    assert archived["adapter"] == "herdr-foreign" and archived["lifecycle"] == "stopped"


def test_revive_refuses_a_spec_whose_program_is_not_the_harness(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, pane = foreign(tmp_path, monkeypatch, harness="claude",
                                   command_line=("/usr/bin/node", "/opt/cli.js", "--resume", "x"))
    adopt(sessions, pane, tmp_path, harness="claude")
    kill(fake, monkeypatch, sessions.get("lead"))
    assert "is not the claude harness" in cast(str, sessions.revive("lead", dry_run=True)["reason"])
    assert fake.launched == []


def test_record_with_a_malformed_spec_is_refused(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="codex")
    path = manager._directory("worker") / "agent.json"
    document = json.loads(path.read_text())
    document["launch_spec"]["argv"] = ["", "x"]
    path.write_text(json.dumps(document))
    with pytest.raises(AgentDeliveryError, match=r"invalid launch spec \(argv\)"):
        manager.get("worker")


def test_wrkslots_comes_from_the_environment_then_project_configuration_then_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    from .test_agentctl_sessions import _write_profiles
    configured = tmp_path / "configured-wrkslots"
    _write_profiles(tmp_path, {})
    path = tmp_path / ".agentctl" / "profiles.json"
    document = json.loads(path.read_text())
    document["wrkslots"] = str(configured)
    path.write_text(json.dumps(document))
    monkeypatch.delenv("AGENTCTL_WRKSLOTS_BIN", raising=False)
    monkeypatch.setenv("PATH", str(tmp_path / "empty"))
    registry = tmp_path / ".agentctl"
    assert profiles.wrkslots_for_registry(registry) == str(configured)
    assert wrkslots_executable(registry) == str(configured)
    assert wrkslots_executable(None) is None
    monkeypatch.setenv("AGENTCTL_WRKSLOTS_BIN", "/explicit/wrkslots")
    assert wrkslots_executable(registry) == "/explicit/wrkslots"
    document["wrkslots"] = "relative/wrkslots"
    path.write_text(json.dumps(document))
    with pytest.raises(AgentDeliveryError, match="absolute path"):
        profiles.wrkslots_for_registry(registry)


@pytest.mark.parametrize(("harness", "argv", "replayed"), [
    ("claude", ["--session-id", "old", "--fork-session", "--effort", "max"], ["--resume", "new", "--effort", "max"]),
    ("claude", ["--resume=old", "--continue"], ["--resume", "new"]),
    ("codex", ["resume", "old", "--no-alt-screen"], ["resume", "new", "--no-alt-screen"]),
    ("codex", ["--no-alt-screen"], ["resume", "new", "--no-alt-screen"]),
    ("muse", ["--model", "m", "resume", "old"], ["--model", "m", "resume", "new"]),
])
def test_replay_replaces_every_conversation_selector_with_one_resume(
    harness: str, argv: list[str], replayed: list[str],
) -> None:
    assert list(launch_spec.replay_arguments(harness, argv, "new")) == replayed


@pytest.mark.parametrize(("harness", "argv"), [
    ("claude", ["--resume", "--model", "x"]),
    ("claude", ["-cfoo"]),
    ("codex", ["resume", "--last"]),
    ("codex", ["--model", "m", "resume", "x"]),
    ("muse", ["resume", "x", "--model", "m"]),
    ("bash", []),
])
def test_replay_refuses_selectors_it_cannot_reproduce(harness: str, argv: list[str]) -> None:
    with pytest.raises(AgentDeliveryError):
        launch_spec.replay_arguments(harness, argv, "new")


def test_argv_conversation_refuses_two_different_conversations() -> None:
    assert launch_spec.argv_conversation("claude", ["-r", "same", "--resume=same"]) == "same"
    with pytest.raises(AgentDeliveryError, match="more than one conversation"):
        launch_spec.argv_conversation("claude", ["--resume=one", "-rtwo"])


def failed_codex_launch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> tuple[Sessions, FakeManagedClient, AgentRecord]:
    """A resumed Codex launch whose readiness wait failed while the harness came up anyway."""
    monkeypatch.delenv("HERDR_WORKSPACE_ID", raising=False)
    fake = FakeManagedClient()
    sessions = Sessions(cast(HerdrClient, fake), tmp_path / "registry")
    fake.fail_start = True
    with pytest.raises(AgentDeliveryError):
        sessions.start("liteinst", cwd=str(tmp_path), harness="codex", resume="goal-conversation")
    record = sessions.get("liteinst")
    assert record.lifecycle == "launch_failed" and record.pane_id is not None
    fake.infos[record.pane_id] = replace(fake.infos[record.pane_id], agent="codex", status="working",
                                         session_agent="codex", session_value="goal-conversation")
    return sessions, fake, record


def test_promote_pins_a_live_harness_behind_a_failed_launch_without_touching_its_tab(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, record = failed_codex_launch(tmp_path, monkeypatch)
    result = sessions.promote("liteinst", expected_token=record.token)
    promoted = sessions.get("liteinst")
    assert result["promoted"] is True and promoted.lifecycle == "running" and promoted.error is None
    assert promoted.harness_anchor is not None and promoted.terminal_id is not None
    assert promoted.session_value == "goal-conversation" and promoted.token == record.token
    assert fake.closed == [] and len(fake.launched) == 1 and fake.submitted == []
    with pytest.raises(AgentDeliveryError, match="applies to a launch_failed record"):
        sessions.promote("liteinst")


@pytest.mark.parametrize("failure", ["absent", "other-harness", "other-session", "renamed", "token"])
def test_promote_refuses_unless_the_launched_harness_is_provably_there(
    failure: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, record = failed_codex_launch(tmp_path, monkeypatch)
    pane = cast(str, record.pane_id)
    if failure == "absent":
        fake.infos[pane] = replace(fake.infos[pane], agent=None, status="unknown",
                                   session_agent=None, session_value=None)
    elif failure == "other-harness":
        fake.infos[pane] = replace(fake.infos[pane], agent="claude", session_agent=None, session_value=None)
    elif failure == "other-session":
        fake.infos[pane] = replace(fake.infos[pane], session_value="another-conversation")
    elif failure == "renamed":
        fake.launched = [("someone", *entry[1:]) for entry in fake.launched]
    with pytest.raises((AgentDeliveryError, HerdrUnavailable)):
        sessions.promote("liteinst", expected_token="stale" if failure == "token" else None)
    assert sessions.get("liteinst").lifecycle == "launch_failed"
