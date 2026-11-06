"""Panes whose Herdr server reports process IDs from another PID namespace."""
from __future__ import annotations

import json
import os
import socket
import subprocess
from collections.abc import Mapping, Sequence
from pathlib import Path

import pytest

from agentctl.client import HerdrClient
from agentctl.errors import HerdrUnavailable
from agentctl.pidns import (
    LocalProcess, Translation, PidNamespaceView, TranslationRefused, container_exec_target,
    local_container_id, parse_stat, server_pid_visible, translate,
)

CONTAINER = "addd45b11cd563b0e53ddde2e37ab2313d5cd30e246e7540403a54f15f6f37eb"
OTHER = "f" * 64
SESSION = "01a174d9-7b3f-7bc3-97b5-ecc4f65c68fe"
ROLLOUT = f"/home/u/.codex/sessions/2026/10/25/rollout-2026-10-25T16-56-12-{SESSION}.jsonl"
EXEC = ("podman", "exec", "-it", CONTAINER[:12], "bash")


class Table:
    """A fake local process table: init, one exec session running codex, one bystander."""

    def __init__(self, processes: Sequence[LocalProcess],
                 files: Mapping[int, tuple[str, ...]] | None = None) -> None:
        self.rows = {process.pid: process for process in processes}
        self.files = dict(files or {})

    def processes(self) -> Mapping[int, LocalProcess]:
        return self.rows

    def open_files(self, pid: int) -> tuple[str, ...]:
        return self.files.get(pid, ())

    def cwd(self, pid: int) -> str:
        return f"/work/{pid}"


def process(pid: int, ppid: int, pgrp: int, tpgid: int, name: str) -> LocalProcess:
    return LocalProcess(pid, ppid, pgrp, tpgid, name, (f"/usr/bin/{name}", "--flag"),
                        f"/opt/{name}/bin/{name}")


def standard_table(**files: tuple[str, ...]) -> Table:
    rows = [
        process(1, 0, 1, 1, "bash"),
        process(20, 0, 20, 30, "bash"),
        process(30, 20, 30, 30, "codex"),
        process(31, 30, 31, 30, "codex-code-mode-host"),
        process(40, 1, 40, 1, "codex"),
    ]
    return Table(rows, {30: (ROLLOUT, "/dev/pts/1"), **{int(k[1:]): v for k, v in files.items()}})


def run(table: Table, *, foreground: Sequence[Sequence[str]] = (EXEC,),
        container: str | None = CONTAINER, agent: str | None = "codex",
        session: str | None = SESSION) -> Translation:
    return translate(reported_foreground=foreground, session_agent=agent,
                     session_value=session, container_id=container, table=table)


def test_exec_session_maps_to_the_local_shell_and_foreground_group() -> None:
    translation = run(standard_table())
    assert translation.shell_pid == 20
    assert translation.foreground_pgid == 30
    assert translation.harness_pid == 30
    assert [entry[0] for entry in translation.foreground] == [30]
    assert translation.foreground[0][3] == "/usr/bin/codex"
    assert translation.cwd == "/work/30"


@pytest.mark.parametrize(
    ("kwargs", "message"),
    [
        ({"foreground": (EXEC, ("make", "root"))}, "not exactly one"),
        ({"foreground": (("bash",),)}, "not podman or docker exec"),
        ({"foreground": (("podman", "run", "-it", "image"),)}, "not podman or docker exec"),
        ({"container": OTHER}, "another container"),
        ({"container": None}, "cannot be determined"),
        ({"foreground": (("podman", "exec", "-it", CONTAINER[:6], "bash"),)}, "another container"),
        ({"session": None}, "no usable native session"),
        ({"agent": None}, "no usable native session"),
        ({"session": "other-session-id"}, "found 0"),
        ({"agent": "claude"}, "found 0"),
    ],
)
def test_untranslatable_panes_are_refused(kwargs: dict[str, object], message: str) -> None:
    with pytest.raises(TranslationRefused, match=message):
        run(standard_table(), **kwargs)  # type: ignore[arg-type]


def test_two_holders_of_one_session_are_ambiguous() -> None:
    with pytest.raises(TranslationRefused, match="found 2"):
        run(standard_table(p40=(ROLLOUT,)))


def test_a_harness_under_the_container_init_is_not_an_exec_session() -> None:
    table = standard_table()
    table.files = {40: (ROLLOUT,)}
    with pytest.raises(TranslationRefused, match="init process"):
        run(table)


def test_a_foreground_member_outside_the_exec_session_is_refused() -> None:
    table = standard_table()
    table.rows[41] = process(41, 1, 30, 1, "intruder")
    with pytest.raises(TranslationRefused, match="outside the exec session"):
        run(table)


def test_broken_ancestry_is_refused() -> None:
    table = standard_table()
    del table.rows[20]
    with pytest.raises(TranslationRefused, match="ancestry is incomplete"):
        run(table)


def test_session_file_name_must_end_with_the_session() -> None:
    table = standard_table()
    table.files = {30: (ROLLOUT + ".bak", f"/x/{SESSION}-other.jsonl")}
    with pytest.raises(TranslationRefused, match="found 0"):
        run(table)
    table.files = {30: (f"/x/{SESSION}.jsonl",)}
    assert run(table).harness_pid == 30


@pytest.mark.parametrize(
    ("argv", "target"),
    [
        (("podman", "exec", "-it", "abc", "bash"), "abc"),
        (("/usr/bin/docker", "exec", "-e", "A=B", "-u", "root", "abc", "sh"), "abc"),
        (("podman", "exec", "--env=A=B", "--workdir", "/w", "abc"), "abc"),
        (("podman", "exec", "--", "abc", "bash"), "abc"),
        (("podman", "exec", "-it"), None),
        (("podman", "run", "abc"), None),
        (("nerdctl", "exec", "abc"), None),
    ],
)
def test_container_exec_target(argv: tuple[str, ...], target: str | None) -> None:
    assert container_exec_target(argv) == target


def test_local_container_id_reads_the_hostname_bind_mount() -> None:
    podman = (f"1 2 0:3 /containers/overlay-containers/{CONTAINER}/userdata/hostname "
              "/etc/hostname rw - xfs /dev/x rw")
    docker = f"1 2 0:3 /var/lib/docker/containers/{OTHER}/hostname /etc/hostname rw - ext4 /dev/y rw"
    hosts = f"1 2 0:3 /containers/overlay-containers/{OTHER}/userdata/hosts /etc/hosts rw - xfs /dev/x rw"
    assert local_container_id(podman + "\n" + hosts) == CONTAINER
    assert local_container_id(docker) == OTHER
    assert local_container_id(hosts) is None
    assert local_container_id(podman + "\n" + docker) is None


def test_parse_stat_reads_the_terminal_foreground_group() -> None:
    raw = b"2532 (ba sh) S 0 2532 2532 34817 3477 4194560 1 2 3 4"
    assert parse_stat(raw) == (2532, "ba sh", 0, 2532, 3477)
    assert parse_stat(b"garbage") is None


def test_parse_stat_matches_this_process() -> None:
    with open("/proc/self/stat", "rb") as stream:
        parsed = parse_stat(stream.read())
    assert parsed is not None and parsed[0] == os.getpid()
    assert parsed[2] == os.getppid() and parsed[3] == os.getpgrp()


def test_a_local_server_is_visible(tmp_path: Path) -> None:
    path = str(tmp_path / "s.sock")
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(path)
    server.listen(1)
    try:
        assert server_pid_visible(path) is True
    finally:
        server.close()
    assert server_pid_visible(str(tmp_path / "missing.sock")) is None


class HerdrResponses:
    def __init__(self, pane: dict[str, object], process_info: dict[str, object]) -> None:
        self.pane = pane
        self.process_info = process_info
        self.calls: list[list[str]] = []

    def __call__(self, command: Sequence[str]) -> subprocess.CompletedProcess[str]:
        self.calls.append(list(command))
        if list(command[1:3]) == ["pane", "get"]:
            result: dict[str, object] = {"pane": self.pane}
        elif list(command[1:3]) == ["pane", "process-info"]:
            result = {"process_info": self.process_info}
        else:
            raise AssertionError(command)
        return subprocess.CompletedProcess(command, 0, json.dumps({"result": result}), "")


def host_pane(*, agent: str | None = None) -> HerdrResponses:
    pane: dict[str, object] = {
        "pane_id": "w2:p1", "workspace_id": "w2", "tab_id": "w2:t1",
        "cwd": "/host/launcher", "agent_status": "unknown", "terminal_id": "term_1",
        "agent_session": {"agent": "codex", "kind": "id", "value": SESSION},
    }
    if agent is not None:
        pane["agent"] = agent
    return HerdrResponses(pane, {
        "pane_id": "w2:p1", "shell_pid": 3563113, "foreground_process_group_id": 3879230,
        "foreground_processes": [{"pid": 3879230, "name": "podman",
                                  "cmdline": " ".join(EXEC), "argv": list(EXEC)}],
    })


def client(responses: HerdrResponses, *, foreign: bool, table: Table | None = None) -> HerdrClient:
    view = PidNamespaceView(foreign=foreign, table=table or standard_table(), container_id=CONTAINER)
    return HerdrClient(herdr_bin="fixture-herdr", run=responses, pid_namespace=view)


def test_foreign_pane_reports_local_processes_cwd_and_harness() -> None:
    herdr = client(host_pane(), foreign=True)
    info = herdr.process_info("w2:p1")
    assert (info.shell_pid, info.foreground_pgid) == (20, 30)
    assert [entry[0] for entry in info.foreground] == [30]
    pane = herdr.pane_info("w2:p1")
    assert pane.cwd == "/work/30"
    assert pane.agent == "codex"
    assert (pane.session_agent, pane.session_value) == ("codex", SESSION)


def test_harness_that_is_not_the_foreground_is_not_reported_as_the_agent() -> None:
    table = standard_table()
    table.rows[20] = process(20, 0, 20, 20, "bash")
    pane = client(host_pane(), foreign=True, table=table).pane_info("w2:p1")
    assert pane.agent is None
    assert pane.cwd == "/work/20"


def test_untranslatable_foreign_pane_refuses_process_proofs_and_keeps_reported_identity() -> None:
    table = standard_table()
    table.files = {}
    herdr = client(host_pane(), foreign=True, table=table)
    with pytest.raises(HerdrUnavailable, match="outside this PID namespace"):
        herdr.process_info("w2:p1")
    pane = herdr.pane_info("w2:p1")
    assert (pane.cwd, pane.agent) == ("/host/launcher", None)


def test_local_server_numbers_pass_through_unchanged() -> None:
    herdr = client(host_pane(agent="codex"), foreign=False)
    info = herdr.process_info("w2:p1")
    assert (info.shell_pid, info.foreground_pgid) == (3563113, 3879230)
    assert herdr.pane_info("w2:p1").cwd == "/host/launcher"


def test_injected_protocol_fakes_never_probe_a_socket() -> None:
    responses = host_pane()
    herdr = HerdrClient(herdr_bin="fixture-herdr", run=responses)
    assert herdr.pid_namespace().foreign is False
    herdr.process_info("w2:p1")
    assert all(call[1:3] != ["status", "server"] for call in responses.calls)
