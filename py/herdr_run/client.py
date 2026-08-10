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
import pwd
import signal
import stat
import subprocess
import time
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass

from herdr_run.errors import HerdrUnavailable
from herdr_run.jsonx import as_mapping, as_sequence, get_int, get_str, opt_str

__all__ = [
    "HerdrClient",
    "Pane",
    "ProcessInfo",
    "Runner",
    "SERVER_UNIT",
    "CONTROL_TIMEOUT_SECONDS",
    "AgentPaneInfo",
]

#: A ``subprocess.run``-shaped callable, injected so tests never spawn a real process.
Runner = Callable[[Sequence[str]], "subprocess.CompletedProcess[str]"]

#: Transient user unit that owns an auto-started server. Named so a stray one is identifiable, and
#: so repeated starts collide on the unit name instead of racing up several servers.
SERVER_UNIT = "herdr-run-server"

#: Maximum/avoidance bound for one Herdr CLI or systemd control call. Command execution itself has
#: its separate configurable timeout in the pane runner.
CONTROL_TIMEOUT_SECONDS = 30.0

# Linux exposes process IDs through the positive range of its signed ``pid_t``.  Keep this bound
# explicit so Python's arbitrary-precision integers cannot accept protocol values that the Rust
# implementation (or a Linux process API) cannot represent.
_MAX_PROCESS_ID = 2_147_483_647


def _get_process_id(mapping: dict[str, object], key: str, what: str) -> int:
    """Require one positive Linux ``pid_t``-compatible protocol value."""
    value = get_int(mapping, key, what)
    if not 1 <= value <= _MAX_PROCESS_ID:
        raise TypeError(
            f"{what}: field {key!r} is outside the positive Linux pid_t range"
        )
    return value


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


@dataclass(frozen=True)
class AgentPaneInfo:
    """Identity and readiness fields for one interactive-agent pane."""

    pane_id: str
    workspace_id: str
    cwd: str
    agent: str | None
    status: str
    session_agent: str | None
    session_value: str | None


def _bounded_control_command(
    command: Sequence[str],
    *,
    environ: Mapping[str, str] | None = None,
    timeout: float = CONTROL_TIMEOUT_SECONDS,
) -> "subprocess.CompletedProcess[str]":
    """Capture one control command and kill its whole process group on timeout.

    A Herdr or ``systemd-run`` wrapper may itself start helpers which inherit the captured pipes.
    Giving every control call a fresh session lets the timeout path terminate those descendants as
    well as the immediate child, then reap the child before returning.  Otherwise a timed-out call
    could leave an unbounded helper behind or block forever waiting for its inherited pipe ends.
    """
    argv = list(command)
    process = subprocess.Popen(
        argv,
        text=True,
        encoding="utf-8",
        errors="replace",
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=None if environ is None else dict(environ),
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        # start_new_session=True makes the child's PID its process-group ID.  Kill the group even
        # when the immediate child happened to exit at the deadline: descendants may still own the
        # captured pipe ends and are precisely what this cleanup is intended to catch.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        finally:
            # Reap the immediate child and drain/close both pipes before exposing the timeout.
            process.kill()
            process.communicate()
        raise
    return subprocess.CompletedProcess(argv, process.returncode, stdout, stderr)


def default_runner(command: Sequence[str]) -> "subprocess.CompletedProcess[str]":
    """Run a command and capture its output. The real runner, replaced by tests."""
    return _bounded_control_command(command)


_PRODUCTION_RUNNER = default_runner


def _validated_executable(candidate: str, name: str) -> str:
    """Canonicalize and validate one fixed executable candidate."""
    resolved = os.path.realpath(os.path.abspath(candidate))
    try:
        metadata = os.stat(resolved)
    except OSError as exc:
        raise HerdrUnavailable(
            f"cannot inspect {name} executable {resolved}: {exc}"
        ) from exc
    if not stat.S_ISREG(metadata.st_mode) or not os.access(resolved, os.X_OK):
        raise HerdrUnavailable(
            f"{name} executable is not an executable regular file: {resolved}"
        )
    if metadata.st_mode & (stat.S_IWGRP | stat.S_IWOTH):
        raise HerdrUnavailable(
            f"refusing group/world-writable {name} executable outside the jail: {resolved}"
        )
    return resolved


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
        # Injected runners are protocol fakes and intentionally receive the literal configured
        # command.  Production calls pin the executable to one canonical absolute path before
        # asking systemd to execute it outside the jail.
        self._resolve_executable = run is None
        selected_runner = default_runner if run is None else run
        # Keeping the production runner as an identity sentinel lets tests replace the module's
        # default runner while still exercising absolute-path resolution.  The real default is
        # the only path that may invoke subprocess directly with the scrubbed environment below.
        self._production_runner = selected_runner is _PRODUCTION_RUNNER
        self._resolved_bin: str | None = None
        self._resolved_systemd_run: str | None = None
        self._broker = broker
        self._run: Runner = selected_runner
        self._environ = dict(os.environ if environ is None else environ)
        self._sleep = sleep

    # ---- plumbing ---------------------------------------------------------------------------

    def _executable(self) -> str:
        """Return one canonical executable without trusting the caller's PATH ordering.

        This removes the easy ``PATH=$PWD:$PATH`` escalation into ``systemd-run``. It is a safety
        rail, not a same-UID trust boundary: normal per-user installations are owner-writable, as
        documented in the user guide.
        """
        if not self._resolve_executable:
            return self._bin
        if self._resolved_bin is not None:
            return self._resolved_bin
        candidate = self._bin
        if os.path.sep not in candidate:
            home = self._account_home()
            fixed_candidates = (
                os.path.join("/usr/local/bin", candidate),
                os.path.join("/usr/bin", candidate),
                os.path.join(home, ".local", "bin", candidate),
                os.path.join(home, "bin", candidate),
                os.path.join(home, ".cargo", "bin", candidate),
            )
            candidate = next(
                (
                    path
                    for path in fixed_candidates
                    if os.path.isfile(path) and os.access(path, os.X_OK)
                ),
                "",
            )
            if not candidate:
                searched = ", ".join(fixed_candidates)
                raise HerdrUnavailable(
                    f"Herdr executable not found in fixed install locations: {searched}"
                )
        resolved = _validated_executable(candidate, "Herdr")
        self._resolved_bin = resolved
        return resolved

    @staticmethod
    def _account_home() -> str:
        """Read HOME from the account database, not caller-controlled environment text."""
        try:
            home = pwd.getpwuid(os.getuid()).pw_dir
        except (KeyError, OSError) as exc:
            raise HerdrUnavailable(
                f"cannot resolve the current account's home directory: {exc}"
            ) from exc
        if not home:
            raise HerdrUnavailable("the current account has no home directory")
        return home

    def _systemd_executable(self) -> str:
        if not self._resolve_executable:
            return "systemd-run"
        if self._resolved_systemd_run is not None:
            return self._resolved_systemd_run
        for candidate in ("/usr/bin/systemd-run", "/bin/systemd-run"):
            resolved = os.path.realpath(candidate)
            if os.path.isfile(resolved) and os.access(resolved, os.X_OK):
                validated = _validated_executable(resolved, "systemd-run")
                self._resolved_systemd_run = validated
                return validated
        raise HerdrUnavailable(
            "systemd-run was not found at /usr/bin/systemd-run or /bin/systemd-run"
        )

    def _execute(
        self,
        command: Sequence[str],
        *,
        timeout: float = CONTROL_TIMEOUT_SECONDS,
    ) -> "subprocess.CompletedProcess[str]":
        """Invoke one control command; production calls discard caller HOME/PATH."""
        try:
            if not self._production_runner:
                return self._run(command)
            environ = dict(self._environ)
            environ.pop("PATH", None)
            environ["HOME"] = self._account_home()
            return _bounded_control_command(command, environ=environ, timeout=timeout)
        except subprocess.TimeoutExpired as exc:
            raise HerdrUnavailable(
                f"Herdr control command timed out after {timeout:g} seconds"
            ) from exc
        except (OSError, ValueError) as exc:
            raise HerdrUnavailable(f"cannot invoke Herdr: {exc}") from exc

    def _outside_jail(self, command: Sequence[str]) -> list[str]:
        """Wrap ``command`` in a transient user unit, escaping the agent's confinement.

        The command is already an absolute canonical path. HOME comes from the account database;
        forwarding caller-controlled PATH/HOME would turn a planted executable into an unconfined
        process.
        """
        return [
            self._systemd_executable(),
            "--user",
            "--wait",
            "--pipe",
            "--collect",
            "--quiet",
            "--setenv",
            f"HOME={self._account_home()}",
            *command,
        ]

    def _invoke(
        self,
        args: Sequence[str],
        *,
        timeout: float = CONTROL_TIMEOUT_SECONDS,
    ) -> "subprocess.CompletedProcess[str]":
        command = [self._executable(), *args]
        if self._broker == "systemd-run":
            command = self._outside_jail(command)
        try:
            return self._execute(command, timeout=timeout)
        except (
            OSError
        ) as exc:  # pragma: no cover - _execute already narrows production failures
            raise HerdrUnavailable(f"cannot invoke Herdr: {exc}") from exc

    def _call(self, args: Sequence[str], purpose: str) -> dict[str, object]:
        """Invoke a socket-API subcommand and return its ``result`` object."""
        completed = self._invoke(args)
        if completed.returncode != 0:
            detail = (
                completed.stderr or completed.stdout or ""
            ).strip() or f"exit {completed.returncode}"
            raise HerdrUnavailable(f"{purpose}: {detail}")
        try:
            document: object = json.loads(completed.stdout)
        except json.JSONDecodeError as exc:
            preview = completed.stdout.strip()[:200]
            raise HerdrUnavailable(
                f"{purpose}: herdr returned non-JSON output: {preview!r}"
            ) from exc
        try:
            envelope = as_mapping(document, purpose)
            result = envelope.get("result")
            if not isinstance(result, dict):
                raise HerdrUnavailable(
                    f"{purpose}: herdr response has no result object"
                )
            return as_mapping(result, purpose)
        except TypeError as exc:
            raise HerdrUnavailable(f"{purpose}: invalid Herdr response: {exc}") from exc

    # ---- server -----------------------------------------------------------------------------

    def server_running(self) -> bool:
        """Is a compatible Herdr server currently up? Never raises; absence is a normal answer."""
        try:
            completed = self._invoke(["status", "--json"])
        except HerdrUnavailable:
            return False
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

        try:
            launch = self._execute(
                [
                    self._systemd_executable(),
                    "--user",
                    "--collect",
                    "--unit",
                    SERVER_UNIT,
                    "--description",
                    "herdr-run Herdr server (outside the agent sandbox)",
                    "--setenv",
                    f"HOME={self._account_home()}",
                    self._executable(),
                    "server",
                ]
            )
        except OSError as exc:
            raise HerdrUnavailable(f"cannot start the Herdr server: {exc}") from exc
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
        try:
            matches: list[str] = []
            for entry in as_sequence(result.get("workspaces"), "workspace list"):
                workspace = as_mapping(entry, "workspace list entry")
                if opt_str(workspace, "label") == label:
                    matches.append(
                        get_str(workspace, "workspace_id", "workspace list entry")
                    )
        except TypeError as exc:
            raise HerdrUnavailable(
                f"workspace list: invalid Herdr response: {exc}"
            ) from exc
        if len(matches) > 1:
            raise HerdrUnavailable(
                f"workspace label {label!r} is ambiguous across ids: {', '.join(matches)}"
            )
        return matches[0] if matches else None

    def create_workspace(self, *, label: str, cwd: str) -> tuple[str, str, str]:
        """Create a workspace. Returns ``(workspace_id, root_tab_id, root_pane_id)``.

        Herdr gives a new workspace one default tab (labelled ``"1"``); the caller renames it rather
        than creating a second tab, so a freshly created workspace has exactly one tab.
        """
        result = self._call(
            ["workspace", "create", "--label", label, "--cwd", cwd, "--no-focus"],
            f"workspace create {label!r}",
        )
        try:
            workspace = as_mapping(result.get("workspace"), "workspace create")
            tab = as_mapping(result.get("tab"), "workspace create")
            pane = as_mapping(result.get("root_pane"), "workspace create")
            return (
                get_str(workspace, "workspace_id", "workspace create"),
                get_str(tab, "tab_id", "workspace create"),
                get_str(pane, "pane_id", "workspace create"),
            )
        except TypeError as exc:
            raise HerdrUnavailable(
                f"workspace create: invalid Herdr response: {exc}"
            ) from exc

    def tab_id_for_label(self, workspace_id: str, label: str) -> str | None:
        """Resolve a tab LABEL within one workspace to its id, or ``None`` when absent."""
        result = self._call(["tab", "list", "--workspace", workspace_id], "tab list")
        try:
            matches: list[str] = []
            for entry in as_sequence(result.get("tabs"), "tab list"):
                tab = as_mapping(entry, "tab list entry")
                if opt_str(tab, "label") == label:
                    matches.append(get_str(tab, "tab_id", "tab list entry"))
        except TypeError as exc:
            raise HerdrUnavailable(f"tab list: invalid Herdr response: {exc}") from exc
        if len(matches) > 1:
            raise HerdrUnavailable(
                f"tab label {label!r} is ambiguous in workspace {workspace_id}: "
                f"{', '.join(matches)}"
            )
        return matches[0] if matches else None

    def create_tab(self, *, workspace_id: str, label: str, cwd: str) -> str:
        """Create a labelled tab in an existing workspace and return its id. Never steals focus."""
        result = self._call(
            [
                "tab",
                "create",
                "--workspace",
                workspace_id,
                "--label",
                label,
                "--cwd",
                cwd,
                "--no-focus",
            ],
            f"tab create {label!r}",
        )
        try:
            tab = as_mapping(result.get("tab", result), "tab create")
            return get_str(tab, "tab_id", "tab create")
        except TypeError as exc:
            raise HerdrUnavailable(
                f"tab create: invalid Herdr response: {exc}"
            ) from exc

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
        try:
            for entry in as_sequence(result.get("panes"), "pane list"):
                pane = as_mapping(entry, "pane list entry")
                parsed = Pane(
                    pane_id=get_str(pane, "pane_id", "pane list entry"),
                    tab_id=get_str(pane, "tab_id", "pane list entry"),
                    workspace_id=get_str(pane, "workspace_id", "pane list entry"),
                )
                if workspace_id is not None and parsed.workspace_id != workspace_id:
                    raise HerdrUnavailable(
                        f"pane list: returned pane {parsed.pane_id!r} from workspace "
                        f"{parsed.workspace_id!r}, expected {workspace_id!r}"
                    )
                out.append(parsed)
        except TypeError as exc:
            raise HerdrUnavailable(f"pane list: invalid Herdr response: {exc}") from exc
        return tuple(out)

    def pane_exists(self, pane_id: str) -> bool:
        """Is this pane id still live? Used to invalidate a cached id rather than trust it."""
        try:
            self._call(["pane", "get", pane_id], f"pane get {pane_id}")
        except HerdrUnavailable:
            return False
        return True

    def pane_info(self, pane_id: str) -> AgentPaneInfo:
        """Return validated identity/readiness data for an interactive pane."""
        result = self._call(["pane", "get", pane_id], f"pane get {pane_id}")
        try:
            pane = as_mapping(result.get("pane"), "pane get")
            returned = get_str(pane, "pane_id", "pane get")
            if returned != pane_id:
                raise HerdrUnavailable(
                    f"pane get: returned pane {returned!r}, expected {pane_id!r}"
                )
            session_raw = pane.get("agent_session")
            session_agent: str | None = None
            session_value: str | None = None
            if session_raw is not None:
                session = as_mapping(session_raw, "pane agent_session")
                session_agent = opt_str(session, "agent")
                session_value = opt_str(session, "value")
            return AgentPaneInfo(
                pane_id=returned,
                workspace_id=get_str(pane, "workspace_id", "pane get"),
                cwd=get_str(pane, "cwd", "pane get"),
                agent=opt_str(pane, "agent"),
                status=opt_str(pane, "agent_status") or "unknown",
                session_agent=session_agent,
                session_value=session_value,
            )
        except TypeError as exc:
            raise HerdrUnavailable(f"pane get: invalid Herdr response: {exc}") from exc

    def workspace_label(self, workspace_id: str) -> str:
        """Return the label for one exact workspace id."""
        result = self._call(["workspace", "get", workspace_id], "workspace get")
        try:
            workspace = as_mapping(result.get("workspace"), "workspace get")
            returned = get_str(workspace, "workspace_id", "workspace get")
            if returned != workspace_id:
                raise HerdrUnavailable(
                    f"workspace get: returned workspace {returned!r}, expected {workspace_id!r}"
                )
            return get_str(workspace, "label", "workspace get")
        except TypeError as exc:
            raise HerdrUnavailable(f"workspace get: invalid Herdr response: {exc}") from exc

    def wait_agent_status(self, pane_id: str, status: str, timeout_ms: int) -> None:
        """Wait for a native Herdr agent-state transition."""
        purpose = f"wait for pane {pane_id} status {status}"
        completed = self._invoke(
            ["wait", "agent-status", pane_id, "--status", status, "--timeout", str(timeout_ms)],
            timeout=max(CONTROL_TIMEOUT_SECONDS, timeout_ms / 1000.0 + 5.0),
        )
        if completed.returncode != 0:
            detail = (completed.stderr or completed.stdout or "").strip() or f"exit {completed.returncode}"
            raise HerdrUnavailable(f"{purpose}: {detail}")
        try:
            envelope = as_mapping(json.loads(completed.stdout), purpose)
            data = as_mapping(envelope.get("data"), purpose)
            returned_pane = get_str(data, "pane_id", purpose)
            returned_status = get_str(data, "agent_status", purpose)
        except (json.JSONDecodeError, TypeError) as exc:
            raise HerdrUnavailable(f"{purpose}: invalid Herdr event response: {exc}") from exc
        if returned_pane != pane_id or returned_status != status:
            raise HerdrUnavailable(
                f"{purpose}: event reported pane={returned_pane!r} status={returned_status!r}"
            )

    def process_info(self, pane_id: str) -> ProcessInfo:
        """The pane's live shell pid and foreground process group — the readiness signal."""
        result = self._call(
            ["pane", "process-info", "--pane", pane_id], f"pane process-info {pane_id}"
        )
        try:
            info = as_mapping(result.get("process_info"), "pane process-info")
            foreground: list[tuple[int, str, str]] = []
            for entry in as_sequence(
                info.get("foreground_processes"), "foreground_processes"
            ):
                process = as_mapping(entry, "foreground process")
                foreground.append(
                    (
                        _get_process_id(process, "pid", "foreground process"),
                        opt_str(process, "name") or "",
                        opt_str(process, "cmdline") or "",
                    )
                )
            parsed = ProcessInfo(
                pane_id=get_str(info, "pane_id", "pane process-info"),
                shell_pid=_get_process_id(info, "shell_pid", "pane process-info"),
                foreground_pgid=_get_process_id(
                    info, "foreground_process_group_id", "pane process-info"
                ),
                foreground=tuple(foreground),
            )
            if parsed.pane_id != pane_id:
                raise HerdrUnavailable(
                    f"pane process-info: returned pane {parsed.pane_id!r}, expected {pane_id!r}"
                )
            return parsed
        except TypeError as exc:
            raise HerdrUnavailable(
                f"pane process-info: invalid Herdr response: {exc}"
            ) from exc

    def read(
        self,
        pane_id: str,
        *,
        source: str = "recent-unwrapped",
        lines: int | None = None,
    ) -> str:
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
            detail = (
                completed.stderr or completed.stdout or ""
            ).strip() or f"exit {completed.returncode}"
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
