"""The agentctl-backed running check reports dead only after agentctl retired the agent."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from collections.abc import Iterator
from pathlib import Path

import pytest

PROBE = Path(__file__).resolve().parents[1] / "examples" / "agentctl_liveness_probe.py"


def boot_id() -> str:
    return Path("/proc/sys/kernel/random/boot_id").read_text(encoding="ascii").strip()


def identity(pid: int) -> dict[str, object]:
    stat = Path(f"/proc/{pid}/stat").read_text(encoding="ascii")
    ticks = int(stat[stat.rindex(")") + 2 :].split()[19])
    return {"pid": pid, "starttime_ticks": ticks, "boot_id": boot_id(), "version": 1}


def write_record(path: Path, name: str, **fields: object) -> None:
    path.mkdir(parents=True)
    (path / "agent.json").write_text(json.dumps({"name": name, **fields}), encoding="utf-8")


def run(project: Path, agent: str) -> tuple[int, str]:
    env = {key: value for key, value in os.environ.items() if key != "AGENTCTL_REGISTRY"}
    env["WRKSLOTS_PROJECT_ROOT"] = str(project)
    done = subprocess.run(
        [sys.executable, str(PROBE), agent], env=env, capture_output=True, text=True, check=False
    )
    return done.returncode, done.stdout


@pytest.fixture
def exited_identity() -> dict[str, object]:
    child = subprocess.Popen([sys.executable, "-c", "pass"])
    recorded = identity(child.pid)
    child.wait()
    return recorded


@pytest.fixture
def pane_holder() -> Iterator[str]:
    pane = f"wTEST:p{os.getpid()}"
    env = dict(os.environ, HERDR_PANE_ID=pane)
    child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"], env=env)
    try:
        yield pane
    finally:
        child.kill()
        child.wait()


def test_archived_record_with_exited_process_is_dead(
    tmp_path: Path, exited_identity: dict[str, object]
) -> None:
    write_record(
        tmp_path / ".agentctl/archive/w1-abc",
        "w1",
        custom_process_identity=exited_identity,
        pane_id="wTEST:p-gone",
    )
    code, out = run(tmp_path, "w1")
    assert (code, out.split()[:3]) == (0, ["dead", "agent=w1", "rc=0"])


def test_active_record_with_live_process_is_alive(tmp_path: Path) -> None:
    write_record(tmp_path / ".agentctl/w1", "w1", custom_process_identity=identity(os.getpid()))
    code, out = run(tmp_path, "w1")
    assert (code, out.split()[:3]) == (1, ["alive", "agent=w1", "rc=1"])
    assert len(out.splitlines()) == 1


def test_active_record_without_live_process_is_unverifiable(
    tmp_path: Path, exited_identity: dict[str, object]
) -> None:
    write_record(tmp_path / ".agentctl/w1", "w1", custom_process_identity=exited_identity)
    assert run(tmp_path, "w1")[0] == 2


def test_archived_record_whose_process_lives_is_alive(tmp_path: Path) -> None:
    write_record(
        tmp_path / ".agentctl/archive/w1-abc", "w1", custom_process_identity=identity(os.getpid())
    )
    assert run(tmp_path, "w1")[0] == 1


def test_archived_record_whose_pane_is_still_occupied_is_unverifiable(
    tmp_path: Path, exited_identity: dict[str, object], pane_holder: str
) -> None:
    write_record(
        tmp_path / ".agentctl/archive/w1-abc",
        "w1",
        custom_process_identity=exited_identity,
        pane_id=pane_holder,
    )
    assert run(tmp_path, "w1")[0] == 2


def test_unknown_agent_and_prefix_collisions_are_unverifiable(tmp_path: Path) -> None:
    write_record(tmp_path / ".agentctl/archive/w1-extra-abc", "w1-extra")
    assert run(tmp_path, "w1")[0] == 2
    assert run(tmp_path, "nobody")[0] == 2


def test_identity_without_boot_id_is_unverifiable(
    tmp_path: Path, exited_identity: dict[str, object]
) -> None:
    broken = {key: value for key, value in exited_identity.items() if key != "boot_id"}
    write_record(tmp_path / ".agentctl/archive/w1-abc", "w1", custom_process_identity=broken)
    assert run(tmp_path, "w1")[0] == 2


def test_malformed_name_and_record_are_unverifiable(tmp_path: Path) -> None:
    assert run(tmp_path, "Bad Name")[0] == 2
    (tmp_path / ".agentctl/archive/w1-abc").mkdir(parents=True)
    (tmp_path / ".agentctl/archive/w1-abc/agent.json").write_text("{", encoding="utf-8")
    assert run(tmp_path, "w1")[0] == 2


def history(*names: str) -> list[dict[str, object]]:
    return [{"name": name, "renamed_at": 1.0, "journal_id": f"{index:032x}"}
            for index, name in enumerate(names)]


def test_renamed_live_agent_is_alive_under_its_old_name(tmp_path: Path) -> None:
    registry = tmp_path / ".agentctl"
    write_record(registry / "new", "new", name_history=history("old"),
                 harness_identity=identity(os.getpid()))
    assert run(tmp_path, "old") == (1, "alive agent=old rc=1 reason=active-record-process-alive\n")


def test_earlier_stopped_archive_does_not_make_a_renamed_live_agent_dead(
    tmp_path: Path, exited_identity: dict[str, object],
) -> None:
    # The false-dead case: OLD was stopped once, a later OLD was renamed NEW and still runs.
    registry = tmp_path / ".agentctl"
    write_record(registry / "archive" / "old-aaaa", "old", custom_process_identity=exited_identity)
    write_record(registry / "new", "new", name_history=history("old"),
                 harness_identity=identity(os.getpid()))
    assert run(tmp_path, "old")[0] == 1


def test_rename_chain_is_followed(tmp_path: Path) -> None:
    registry = tmp_path / ".agentctl"
    write_record(registry / "third", "third", name_history=history("first", "second"),
                 harness_identity=identity(os.getpid()))
    assert run(tmp_path, "first")[0] == 1
    assert run(tmp_path, "second")[0] == 1


def test_renamed_then_stopped_agent_is_dead_under_its_old_name(
    tmp_path: Path, exited_identity: dict[str, object],
) -> None:
    registry = tmp_path / ".agentctl"
    write_record(registry / "archive" / "new-bbbb", "new", name_history=history("old"),
                 harness_identity=exited_identity)
    assert run(tmp_path, "old") == (0, "dead agent=old rc=0 reason=stopped-by-agentctl\n")


def test_incomplete_rename_is_unverifiable(tmp_path: Path) -> None:
    registry = tmp_path / ".agentctl"
    (registry / ".renames").mkdir(parents=True)
    (registry / ".renames" / "tok.json").write_text(
        json.dumps({"old": "old", "new": "new"}), encoding="utf-8")
    assert run(tmp_path, "old") == (2, "unverifiable agent=old rc=2 reason=rename-incomplete\n")
    assert run(tmp_path, "new")[0] == 2


def test_adopted_archive_keeps_blocking_while_its_runtime_lives(tmp_path: Path) -> None:
    # Stopping an adopted agent unregisters it without ending its runtime.
    registry = tmp_path / ".agentctl"
    write_record(registry / "archive" / "renamed-cccc", "renamed", adapter="herdr-foreign",
                 name_history=history("adopted"), foreign_shell_identity=identity(os.getpid()))
    assert run(tmp_path, "adopted")[0] == 1


def test_unreadable_record_makes_any_name_unverifiable(tmp_path: Path) -> None:
    registry = tmp_path / ".agentctl"
    (registry / "archive" / "broken-dddd").mkdir(parents=True)
    (registry / "archive" / "broken-dddd" / "agent.json").write_text("{", encoding="utf-8")
    code, line = run(tmp_path, "other")
    assert code == 2 and "reason=error:" in line


def test_a_corrupt_active_record_is_unverifiable_not_dead(
    tmp_path: Path, exited_identity: dict[str, object],
) -> None:
    registry = tmp_path / ".agentctl"
    (registry / "old").mkdir(parents=True)
    (registry / "old" / "agent.json").write_text("{}", encoding="utf-8")
    write_record(registry / "archive" / "old-eeee", "old", custom_process_identity=exited_identity)
    code, line = run(tmp_path, "old")
    assert code == 2 and "has_no_name" in line


def test_a_former_name_beyond_the_history_cap_still_finds_the_live_lifetime(
    tmp_path: Path, exited_identity: dict[str, object],
) -> None:
    registry = tmp_path / ".agentctl"
    write_record(registry / "archive" / "old-ffff", "old", custom_process_identity=exited_identity)
    write_record(registry / "new", "new", name_history=history("recent"), former_names=["old"],
                 harness_identity=identity(os.getpid()))
    assert run(tmp_path, "old")[0] == 1


def test_a_name_no_record_carries_is_the_one_bound_no_record_line(tmp_path: Path) -> None:
    """A readable registry with no record of the name says exactly that, on one line.

    `wrkslots remove --attest-agent-stopped` accepts a coordinator's attestation
    only for this answer, so its shape is part of the protocol.
    """

    (tmp_path / ".agentctl").mkdir()
    assert run(tmp_path, "hold-x") == (
        2, "unverifiable agent=hold-x rc=2 reason=no-agentctl-record\n"
    )
    write_record(tmp_path / ".agentctl/archive/other-abc", "other")
    assert run(tmp_path, "hold-x") == (
        2, "unverifiable agent=hold-x rc=2 reason=no-agentctl-record\n"
    )


def test_a_missing_registry_is_not_a_no_record_answer(tmp_path: Path) -> None:
    """A registry that is not there says nothing about the agent.

    A misconfigured registry location must not make every agent look
    unregistered, or a coordinator's attestation would be accepted for an agent
    whose real registry still records it.
    """

    assert run(tmp_path, "hold-x") == (
        2, "unverifiable agent=hold-x rc=2 reason=no-agentctl-registry\n"
    )
    (tmp_path / ".agentctl").write_text("not a directory\n", encoding="utf-8")
    assert run(tmp_path, "hold-x") == (
        2, "unverifiable agent=hold-x rc=2 reason=no-agentctl-registry\n"
    )


TRIPWIRE_TEXT = (
    "agentctl registry layout 2: each agent lives in agents/<name>/ and its locks in locks/. "
    "This file is not an agent; it makes agentctl builds older than layout 2 refuse this "
    "registry instead of misreading it. Upgrade agentctl."
)


def layout_two(project: Path) -> Path:
    """An agentctl registry in layout 2 (#593 registry-agents-subdir), with its tripwire."""
    agents = project / ".agentctl/agents"
    agents.mkdir(parents=True)
    (project / ".agentctl/locks").mkdir()
    (agents / "agent.json").write_text(json.dumps(TRIPWIRE_TEXT) + "\n", encoding="utf-8")
    return agents


def test_a_layout_two_live_agent_is_alive_and_a_stopped_one_is_dead(
    tmp_path: Path, exited_identity: dict[str, object]
) -> None:
    agents = layout_two(tmp_path)
    write_record(agents / "w1", "w1", custom_process_identity=identity(os.getpid()))
    write_record(
        tmp_path / ".agentctl/archive/w2-abc", "w2",
        custom_process_identity=exited_identity, pane_id="wTEST:p-gone",
    )
    assert run(tmp_path, "w1")[0] == 1
    assert run(tmp_path, "w2")[0] == 0
    # The tripwire and the layout's own directories are not agents.
    code, out = run(tmp_path, "agents")
    assert (code, out.split()[3]) == (2, "reason=no-agentctl-record")


def test_a_live_agent_moved_to_layout_two_never_looks_dead(
    tmp_path: Path, exited_identity: dict[str, object]
) -> None:
    # An earlier agent of the same name was stopped; the live one now lives in agents/.
    write_record(
        tmp_path / ".agentctl/archive/w1-old", "w1",
        custom_process_identity=exited_identity, pane_id="wTEST:p-gone",
    )
    agents = layout_two(tmp_path)
    write_record(agents / "w1", "w1", custom_process_identity=identity(os.getpid()))
    code, out = run(tmp_path, "w1")
    assert (code, out.split()[:3]) == (1, ["alive", "agent=w1", "rc=1"])


def test_both_layouts_are_read_while_a_registry_is_mixed(tmp_path: Path) -> None:
    agents = layout_two(tmp_path)
    write_record(tmp_path / ".agentctl/w1", "w1", custom_process_identity=identity(os.getpid()))
    write_record(agents / "w2", "w2", custom_process_identity=identity(os.getpid()))
    assert run(tmp_path, "w1")[0] == 1
    assert run(tmp_path, "w2")[0] == 1


def test_a_probe_older_than_layout_two_fails_closed_on_the_tripwire(
    tmp_path: Path, exited_identity: dict[str, object]
) -> None:
    """The pre-layout-2 probe read only REGISTRY/*/agent.json, so it would miss a moved live agent
    and, with a stopped namesake archived, call it dead. The tripwire makes it refuse instead."""
    old_probe = PROBE.read_text(encoding="utf-8")
    start = old_probe.index("    # The top level first, then agents/")
    end = old_probe.index("    active = matching(active_paths, agent, skip_foreign=True)")
    legacy = old_probe[:start] + (
        "    active_paths = sorted(\n"
        "        path / \"agent.json\" for path in registry.iterdir()\n"
        "        if NAME.fullmatch(path.name) and path.name != \"archive\" and (path / \"agent.json\").exists()\n"
        "    )\n"
    ) + old_probe[end:]
    legacy = legacy.replace(
        "matching(active_paths, agent, skip_foreign=True)", "matching(active_paths, agent)"
    )
    legacy_probe = tmp_path / "legacy_probe.py"
    legacy_probe.write_text(legacy, encoding="utf-8")
    project = tmp_path / "project"
    write_record(
        project / ".agentctl/archive/w1-old", "w1",
        custom_process_identity=exited_identity, pane_id="wTEST:p-gone",
    )
    agents = layout_two(project)
    write_record(agents / "w1", "w1", custom_process_identity=identity(os.getpid()))
    env = {key: value for key, value in os.environ.items() if key != "AGENTCTL_REGISTRY"}
    env["WRKSLOTS_PROJECT_ROOT"] = str(project)
    done = subprocess.run(
        [sys.executable, str(legacy_probe), "w1"], env=env, capture_output=True, text=True,
        check=False,
    )
    assert done.returncode == 2, done.stdout
    assert done.stdout.startswith("unverifiable agent=w1 rc=2"), done.stdout


def test_a_foreign_nonobject_active_record_is_skipped_not_fatal(
    tmp_path: Path, exited_identity: dict[str, object]
) -> None:
    """One foreign agent's malformed record must not block every verdict (#295)."""
    registry = tmp_path / ".agentctl"
    write_record(registry / "archive/w1-abc", "w1", custom_process_identity=exited_identity)
    foreign = registry / "agents" / "foreign"
    foreign.mkdir(parents=True)
    (foreign / "agent.json").write_text('"not a record"\n', encoding="utf-8")
    code, out = run(tmp_path, "w1")
    assert code == 0, out


def test_the_queried_agents_nonobject_record_stays_fatal(tmp_path: Path) -> None:
    registry = tmp_path / ".agentctl"
    own = registry / "agents" / "w1"
    own.mkdir(parents=True)
    (own / "agent.json").write_text('"not a record"\n', encoding="utf-8")
    code, out = run(tmp_path, "w1")
    assert code == 2 and "reason=error:" in out
