"""Thin typed wrapper over the ``herdr`` CLI's socket-API subcommands.

Why the CLI and not the socket directly: ``herdr`` already speaks its own protocol version
(``protocol: 17`` at time of writing) and ships compatibility checks. Re-implementing the wire
format here would create a second, silently-drifting client.

**The one thing that must not be got wrong**: the Herdr SERVER is what determines whether panes are
inside or outside the agent sandbox, because panes are its children. If a confined agent starts the
server itself, every pane it creates inherits that confinement and the whole tool becomes an
elaborate way to get the same 403 — silently, because nothing about the pane would look different.
So :meth:`HerdrClient.ensure_server` always launches through ``systemd-run --user``, which reparents
the server onto the user manager outside the jail. Control CALLS do not need that (the server's unix
socket is reachable from inside; measured on devbig014 2026-08-06), and default to going direct.
"""

from __future__ import annotations

import json
import os
import subprocess
import time
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass

from herdr_run.errors import HerdrUnavailable
from herdr_run.jsonx import as_mapping, as_sequence, get_int, get_str, opt_str

__all__ = ["HerdrClient", "Pane", "ProcessInfo", "Runner", "SERVER_UNIT"]

#: A ``subprocess.run``-shaped callable, injected so tests never spawn a real process.
Runner = Callable[[Sequence[str]], "subprocess.CompletedProcess[str]"]

#: Transient user unit that owns an auto-started server. Named so a stray one is identifiable, and
#: so repeated starts collide on the unit name instead of racing up several servers.
SERVER_UNIT = "herdr-run-server"


@dataclass(frozen=True)
class Pane:
    """One terminal pane and the tab/workspace it belongs to."""

    pane_id: str
    tab_id: str
    workspace_id: str


@dataclass(frozen=True)
class ProcessInfo:
    """A pane's live foreground-process state, as reported by ``herdr pane process-info``."""

    pane_id: str
    shell_pid: int
    foreground_pgid: int
    #: ``(pid, name, cmdline)`` for each process in the pane's foreground process group.
    foreground: tuple[tuple[int, str, str], ...]


def default_runner(command: Sequence[str]) -> "subprocess.CompletedProcess[str]":
    """Run a command and capture its output. The real runner, replaced by tests."""
    return subprocess.run(list(command), text=True, capture_output=True, check=False)


class HerdrClient:
    """Command-level access to a Herdr session, with everything narrowed to concrete types."""

    def __init__(
        self,
        *,
        herdr_bin: str = "herdr",
        broker: str = "direct",
        run: Runner | None = None,
        environ: Mapping[str, str] | None = None,
        sleep: Callable[[float], None] = time.sleep,
    ) -> None:
        self._bin = herdr_bin
        self._broker = broker
        self._run: Runner = run if run is not None else default_runner
        self._environ = dict(os.environ if environ is None else environ)
        self._sleep = sleep

    # ---- plumbing ---------------------------------------------------------------------------

    def _outside_jail(self, command: Sequence[str]) -> list[str]:
        """Wrap ``command`` in a transient user unit, escaping the agent's confinement.

        HOME and PATH are forwarded explicitly: a systemd user unit starts from the manager's
        environment, so without them ``herdr`` cannot find its socket under ``~/.config/herdr``.
        """
        home = self._environ.get("HOME", "")
        path = self._environ.get("PATH", "")
        if not home or not path:
            raise HerdrUnavailable("HOME and PATH are required to broker a call outside the jail")
        return [
            "systemd-run",
            "--user",
            "--wait",
            "--pipe",
            "--collect",
            "--quiet",
            "--setenv",
            f"HOME={home}",
            "--setenv",
            f"PATH={path}",
            *command,
        ]

    def _invoke(self, args: Sequence[str]) -> "subprocess.CompletedProcess[str]":
        command = [self._bin, *args]
        if self._broker == "systemd-run":
            command = self._outside_jail(command)
        return self._run(command)

    def _call(self, args: Sequence[str], purpose: str) -> dict[str, object]:
        """Invoke a socket-API subcommand and return its ``result`` object."""
        completed = self._invoke(args)
        if completed.returncode != 0:
            detail = (completed.stderr or completed.stdout or "").strip() or f"exit {completed.returncode}"
            raise HerdrUnavailable(f"{purpose}: {detail}")
        try:
            document: object = json.loads(completed.stdout)
        except json.JSONDecodeError as exc:
            preview = completed.stdout.strip()[:200]
            raise HerdrUnavailable(f"{purpose}: herdr returned non-JSON output: {preview!r}") from exc
        envelope = as_mapping(document, purpose)
        result = envelope.get("result")
        if not isinstance(result, dict):
            raise HerdrUnavailable(f"{purpose}: herdr response has no result object")
        return as_mapping(result, purpose)

    # ---- server -----------------------------------------------------------------------------

    def server_running(self) -> bool:
        """Is a compatible Herdr server currently up? Never raises; absence is a normal answer."""
        completed = self._invoke(["status", "--json"])
        if completed.returncode != 0:
            return False
        try:
            document: object = json.loads(completed.stdout)
        except json.JSONDecodeError:
            return False
        if not isinstance(document, dict):
            return False
        server = document.get("server")
        return isinstance(server, dict) and server.get("running") is True

    def ensure_server(self, *, attempts: int = 30, delay: float = 0.2) -> bool:
        """Start the Herdr server if it is not already running. Returns True if it had to start one.

        Always launched via ``systemd-run --user`` regardless of the configured broker — see this
        module's docstring for why that is a correctness requirement, not a convenience.
        """
        if self.server_running():
            return False

        home = self._environ.get("HOME", "")
        path = self._environ.get("PATH", "")
        if not home or not path:
            raise HerdrUnavailable("HOME and PATH are required to start the Herdr server")
        launch = self._run(
            [
                "systemd-run",
                "--user",
                "--collect",
                "--unit",
                SERVER_UNIT,
                "--description",
                "herdr-run Herdr server (outside the agent sandbox)",
                "--setenv",
                f"HOME={home}",
                "--setenv",
                f"PATH={path}",
                self._bin,
                "server",
            ]
        )
        combined = (launch.stdout or "") + (launch.stderr or "")
        if launch.returncode != 0 and "already exists" not in combined:
            detail = combined.strip() or f"exit {launch.returncode}"
            raise HerdrUnavailable(f"cannot start the Herdr server: {detail}")

        for _ in range(attempts):
            self._sleep(delay)
            if self.server_running():
                return True
        raise HerdrUnavailable(
            f"the Herdr server did not become ready after {attempts} attempts; "
            f"check 'systemctl --user status {SERVER_UNIT}'"
        )

    # ---- workspaces / tabs / panes ------------------------------------------------------------

    def workspace_id_for_label(self, label: str) -> str | None:
        """Resolve a workspace LABEL to its id, or ``None`` when no workspace carries that label."""
        result = self._call(["workspace", "list"], "workspace list")
        for entry in as_sequence(result.get("workspaces", []), "workspace list"):
            workspace = as_mapping(entry, "workspace list entry")
            if opt_str(workspace, "label") == label:
                return get_str(workspace, "workspace_id", "workspace list entry")
        return None

    def create_workspace(self, *, label: str, cwd: str) -> tuple[str, str, str]:
        """Create a workspace. Returns ``(workspace_id, root_tab_id, root_pane_id)``.

        Herdr gives a new workspace one default tab (labelled ``"1"``); the caller renames it rather
        than creating a second tab, so a freshly created workspace has exactly one tab.
        """
        result = self._call(
            ["workspace", "create", "--label", label, "--cwd", cwd, "--no-focus"],
            f"workspace create {label!r}",
        )
        workspace = as_mapping(result.get("workspace"), "workspace create")
        tab = as_mapping(result.get("tab"), "workspace create")
        pane = as_mapping(result.get("root_pane"), "workspace create")
        return (
            get_str(workspace, "workspace_id", "workspace create"),
            get_str(tab, "tab_id", "workspace create"),
            get_str(pane, "pane_id", "workspace create"),
        )

    def tab_id_for_label(self, workspace_id: str, label: str) -> str | None:
        """Resolve a tab LABEL within one workspace to its id, or ``None`` when absent."""
        result = self._call(["tab", "list", "--workspace", workspace_id], "tab list")
        for entry in as_sequence(result.get("tabs", []), "tab list"):
            tab = as_mapping(entry, "tab list entry")
            if opt_str(tab, "label") == label:
                return get_str(tab, "tab_id", "tab list entry")
        return None

    def create_tab(self, *, workspace_id: str, label: str, cwd: str) -> str:
        """Create a labelled tab in an existing workspace and return its id. Never steals focus."""
        result = self._call(
            ["tab", "create", "--workspace", workspace_id, "--label", label, "--cwd", cwd, "--no-focus"],
            f"tab create {label!r}",
        )
        tab = as_mapping(result.get("tab", result), "tab create")
        return get_str(tab, "tab_id", "tab create")

    def rename_tab(self, tab_id: str, label: str) -> None:
        """Relabel an existing tab."""
        self._call(["tab", "rename", tab_id, label], f"tab rename {tab_id}")

    def panes(self, workspace_id: str | None = None) -> tuple[Pane, ...]:
        """Every pane, optionally restricted to one workspace."""
        args = ["pane", "list"]
        if workspace_id is not None:
            args += ["--workspace", workspace_id]
        result = self._call(args, "pane list")
        out: list[Pane] = []
        for entry in as_sequence(result.get("panes", []), "pane list"):
            pane = as_mapping(entry, "pane list entry")
            out.append(
                Pane(
                    pane_id=get_str(pane, "pane_id", "pane list entry"),
                    tab_id=get_str(pane, "tab_id", "pane list entry"),
                    workspace_id=get_str(pane, "workspace_id", "pane list entry"),
                )
            )
        return tuple(out)

    def pane_exists(self, pane_id: str) -> bool:
        """Is this pane id still live? Used to invalidate a cached id rather than trust it."""
        try:
            self._call(["pane", "get", pane_id], f"pane get {pane_id}")
        except HerdrUnavailable:
            return False
        return True

    def process_info(self, pane_id: str) -> ProcessInfo:
        """The pane's live shell pid and foreground process group — the readiness signal."""
        result = self._call(["pane", "process-info", "--pane", pane_id], f"pane process-info {pane_id}")
        info = as_mapping(result.get("process_info"), "pane process-info")
        foreground: list[tuple[int, str, str]] = []
        for entry in as_sequence(info.get("foreground_processes", []), "foreground_processes"):
            process = as_mapping(entry, "foreground process")
            foreground.append(
                (
                    get_int(process, "pid", "foreground process"),
                    opt_str(process, "name") or "",
                    opt_str(process, "cmdline") or "",
                )
            )
        return ProcessInfo(
            pane_id=get_str(info, "pane_id", "pane process-info"),
            shell_pid=get_int(info, "shell_pid", "pane process-info"),
            foreground_pgid=get_int(info, "foreground_process_group_id", "pane process-info"),
            foreground=tuple(foreground),
        )

    def read(self, pane_id: str, *, source: str = "recent-unwrapped", lines: int | None = None) -> str:
        """Read the pane's rendered text. ANSI is stripped (no ``--raw``), trailing spaces included.

        Note this returns PLAIN TEXT, not JSON: ``herdr pane read`` writes the terminal contents to
        stdout directly, which is why it does not go through :meth:`_call`.
        """
        args = ["pane", "read", pane_id, "--source", source]
        if lines is not None:
            args += ["--lines", str(lines)]
        completed = self._invoke(args)
        if completed.returncode != 0:
            detail = (completed.stderr or "").strip() or f"exit {completed.returncode}"
            raise HerdrUnavailable(f"pane read {pane_id}: {detail}")
        return completed.stdout

    def _call_ok(self, args: Sequence[str], purpose: str) -> None:
        """Invoke a subcommand that reports success only through its exit status.

        ``pane run`` and ``pane send-keys`` write NOTHING on success — they are input-injection
        calls, not queries — so requiring a JSON envelope here would fail every successful call.
        """
        completed = self._invoke(args)
        if completed.returncode != 0:
            detail = (completed.stderr or completed.stdout or "").strip() or f"exit {completed.returncode}"
            raise HerdrUnavailable(f"{purpose}: {detail}")

    def run(self, pane_id: str, command: str) -> None:
        """Type ``command`` into the pane's shell and submit it.

        Fire-and-forget by design: ``herdr pane run`` returns as soon as the line is submitted, so
        the caller collects the result from the spool files rather than from this call.
        """
        self._call_ok(["pane", "run", pane_id, command], f"pane run {pane_id}")

    def send_keys(self, pane_id: str, keys: str) -> None:
        """Send named key presses (for example ``ctrl+u``) to a pane."""
        self._call_ok(["pane", "send-keys", pane_id, keys], f"pane send-keys {pane_id}")
