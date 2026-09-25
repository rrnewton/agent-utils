"""Named, long-lived interactive agents sharing a Herdr workspace.

Herdr owns the terminal and harness process; this module owns durable names,
launch intent, queue routing, output snapshots, and conservative tab teardown.
It never closes a workspace, silently restarts a conversation, or changes the
harness's permission settings. Coordinators and humans see the same terminal.
"""
from __future__ import annotations

import fcntl
import hashlib
import json
import math
import os
import re
import stat
import sys
import time
import uuid
from collections.abc import Callable, Iterator, Sequence
from contextlib import AbstractContextManager, contextmanager, nullcontext
from dataclasses import asdict, dataclass, field, replace
from enum import Enum
from pathlib import Path
from typing import cast

from agentctl import agent
from agentctl.client import (
    AgentPaneInfo,
    CustomProcessIdentity,
    HerdrClient,
    Pane,
    PaneShellProof,
    claude_active_screen,
    claude_prompt_is_exact_composer,
    claude_prompt_transcript_count,
    claude_staged_composer,
    muse_idle_composer,
    muse_verified_process_composer,
    muse_verified_process_idle_composer,
    muse_verified_process_prompt_in_composer,
    muse_verified_process_prompt_is_exact_composer,
    muse_verified_process_prompt_transcript_count,
    muse_startup_metadata,
    muse_trust_prompt,
)
from agentctl.errors import (
    AgentDeliveryError,
    HerdrRunError,
    HerdrUnavailable,
    RuntimeIdentityMismatch,
)
from agentctl.profiles import (
    reasoning_arguments,
    validate_structured_harness_argument_conflicts,
)

_NAME = re.compile(r"[a-z][a-z0-9-]{0,31}\Z")
_KIND = re.compile(r"[a-z][a-z0-9-]{0,63}\Z")
_ENVIRONMENT_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z")
_BRACKETED_PASTE_START = "\x1b[200~"
_BRACKETED_PASTE_END = "\x1b[201~"
_MAX_U64 = (1 << 64) - 1
_MAX_PROCESS_ID = (1 << 31) - 1
_MAX_AGENT_RECORD_BYTES = 1 << 20
_MAX_SNAPSHOT_BYTES = 16 << 20
_MAX_TERMINAL_RETIREMENT_BYTES = _MAX_AGENT_RECORD_BYTES
_MAX_QUEUE_ARTIFACT_BYTES = 16 << 20
_SHA256 = re.compile(r"[0-9a-f]{64}\Z")
_HEALTH_SCHEMA = "agentctl-health/v1"
_HEALTH_PROBE_SECONDS = 60.0
_SESSION_STORAGE_SCHEMA = "agentctl-session/v4"
_PREVIOUS_SESSION_STORAGE_SCHEMA = "agentctl-session/v3"
_LEGACY_SESSION_STORAGE_SCHEMA = "agentctl-session/v2"
_LAUNCH_SPEC_SCHEMA = "agentctl-launch/v2"
_LEGACY_LAUNCH_SPEC_SCHEMA = "agentctl-launch/v1"
_GOAL_STATE_SCHEMA = "agentctl-goal/v1"
_GOAL_TRANSACTION_SCHEMA = "agentctl-goal-transaction/v1"
_GOAL_TRANSACTION_FILE = "goal-transaction.json"
_TERMINAL_STATE_SCHEMA = "agentctl-terminal-state/v1"
_TERMINAL_RETIREMENT_SCHEMA = "agentctl-terminal-retirement/v2"
_LEGACY_TERMINAL_RETIREMENT_SCHEMA = "agentctl-terminal-retirement/v1"
_TERMINAL_RETIREMENT_FILE = "terminal-retirement.json"
_MANAGED_DEAD_RETIREMENT_SCHEMA = "agentctl-managed-dead-retirement/v1"
_MANAGED_DEAD_RETIREMENT_FILE = "managed-dead-retirement.json"
_NATIVE_SESSION_SCHEMA = "agentctl-native-session/v1"
_RELOCATION_SCHEMA = "agentctl-relocation/v1"
_SESSION_COMPATIBILITY_FIELDS = frozenset({
    "arguments", "harness", "cwd", "adapter", "mode", "backend", "model",
    "resume", "launch_profile", "launch_argv", "launch_environment_names",
    "runtime_home", "runtime_ownership", "launch_executable",
    "launch_executable_device", "launch_executable_inode",
    "launch_permission_mode", "runner_pid", "runner_started_at",
    "session_agent", "session_value", "session_source", "goal_delivery",
    "goal_session_id", "goal_command", "goal_messages", "goal_message_id",
    "terminal",
})

_TERMINAL_OUTCOMES = frozenset({
    "managed-dead-preserved", "custom-runtime-absent", "turn-runner-stopped",
    "managed-dead-closed", "owned-pane-closed", "owned-runtime-absent",
    "foreign-unregistered",
})


_rename_directory_noreplace_at = agent._rename_directory_noreplace_at


def _fsync_pinned_directory(descriptor: int, label: str) -> None:
    """Durably order one already-pinned directory; ``label`` is diagnostic only."""
    del label
    os.fsync(descriptor)


def _token(value: str, field_name: str = "token") -> str:
    """Validate one generation token before it becomes a path component."""
    if re.fullmatch(r"[a-z0-9-]{1,80}", value) is None:
        raise AgentDeliveryError(f"{field_name} has an invalid shape")
    return value


def _snapshot_json_bytes(document: dict[str, object]) -> bytes:
    """Encode retained terminal text as UTF-8 without ASCII escape expansion."""
    return (
        json.dumps(
            document,
            indent=2,
            sort_keys=True,
            allow_nan=False,
            ensure_ascii=False,
        )
        + "\n"
    ).encode("utf-8")


def _close_descriptor(
    descriptor: int, label: str, *, primary: BaseException | None,
) -> None:
    """Close one recovery fd without leaking a raw OS error or hiding its cause."""
    try:
        os.close(descriptor)
    except OSError as exc:
        if primary is not None:
            error_type = (
                _ArchivePublicationUncertainError
                if isinstance(primary, _ArchivePublicationUncertainError)
                else AgentDeliveryError
            )
            raise error_type(
                f"{primary}; additionally could not close {label}: {exc}"
            ) from primary
        raise AgentDeliveryError(f"cannot close {label}: {exc}") from exc


def _name(value: str) -> str:
    if not _NAME.fullmatch(value) or value == "archive":
        raise AgentDeliveryError("agent name must start with a lowercase letter and contain 1-32 lowercase letters, digits or hyphens; 'archive' is reserved")
    return value


def harness_arguments(
    harness: str, *, model: str | None = None, resume: str | None = None,
    extra: Sequence[str] = (),
) -> tuple[str, ...]:
    """Build harness-specific arguments; explicit extras retain user configuration."""
    if not _KIND.fullmatch(harness):
        raise AgentDeliveryError("harness must be a Herdr agent kind")
    if any(not value or "\0" in value for value in extra):
        raise AgentDeliveryError("harness arguments must be nonempty and contain no NUL")
    args: list[str] = []
    if harness == "codex":
        if resume:
            args.extend(("resume", resume))
        args.append("--no-alt-screen")
        if model:
            args.extend(("--model", model))
    elif harness == "claude":
        if resume:
            args.extend(("--resume", resume))
        if model:
            args.extend(("--model", model))
    elif harness == "muse":
        if resume is not None:
            raise AgentDeliveryError("interactive Muse resume is not supported; use literal owner-configured argv")
        if model:
            args.extend(("--model", model))
    elif model is not None or resume is not None:
        raise AgentDeliveryError("model and resume presets support codex/claude/muse; use harness arguments for other kinds")
    if any("\0" in value for value in args):
        raise AgentDeliveryError("harness arguments must contain no NUL")
    return tuple((*args, *extra))


def environment_entries(values: Sequence[str]) -> tuple[str, ...]:
    """Validate literal ``KEY=VALUE`` entries for a newly created terminal."""
    entries: list[str] = []
    for entry in values:
        name, separator, value = entry.partition("=")
        if not separator:
            raise AgentDeliveryError("environment entry must use KEY=VALUE")
        if "\0" in name or _ENVIRONMENT_NAME.fullmatch(name) is None:
            raise AgentDeliveryError(
                "environment variable name must match [A-Za-z_][A-Za-z0-9_]*"
            )
        if "\0" in value:
            raise AgentDeliveryError("environment variable value must contain no NUL")
        entries.append(entry)
    return tuple(entries)


@dataclass(frozen=True)
class LaunchSpec:
    """Canonical durable launch intent; argv is the sole argument authority."""

    harness: str
    cwd: str
    adapter: str
    mode: str
    backend: str
    model: str | None
    resume: str | None
    profile: str | None
    argv: tuple[str, ...]
    environment_names: tuple[str, ...]
    runtime_home: str | None
    runtime_ownership: str
    executable: tuple[str, int, int] | None
    permission_mode: str | None = None

    @property
    def arguments(self) -> list[str]:
        """Derive harness arguments from the one canonical argv vector."""
        return list(self.argv[1:]) if self.argv else []

    def to_document(self) -> dict[str, object]:
        """Return the one tagged durable representation of this launch intent."""
        executable: dict[str, object] | None = None
        if self.executable is not None:
            path, device, inode = self.executable
            executable = {"path": path, "device": device, "inode": inode}
        document: dict[str, object] = {
            "schema": _LAUNCH_SPEC_SCHEMA,
            "harness": self.harness,
            "cwd": self.cwd,
            "adapter": self.adapter,
            "mode": self.mode,
            "backend": self.backend,
            "model": self.model,
            "resume": self.resume,
            "profile": self.profile,
            "argv": list(self.argv),
            "environment_names": list(self.environment_names),
            "runtime_home": self.runtime_home,
            "runtime_ownership": self.runtime_ownership,
            "executable": executable,
            "permission_mode": self.permission_mode,
        }
        return document


@dataclass(frozen=True)
class TerminalState:
    """Canonical result of the one terminal transition for a generation."""

    outcome: str
    evidence: dict[str, object]

    @classmethod
    def create(
        cls, outcome: str, evidence: dict[str, object] | None = None,
    ) -> "TerminalState":
        if outcome not in _TERMINAL_OUTCOMES:
            raise AgentDeliveryError("unsupported terminal retirement outcome")
        return cls(outcome=outcome, evidence=dict(evidence or {}))

    @classmethod
    def from_document(cls, value: object) -> "TerminalState":
        if (not isinstance(value, dict)
                or set(value) != {"schema", "outcome", "evidence"}
                or value.get("schema") != _TERMINAL_STATE_SCHEMA
                or not isinstance(value.get("evidence"), dict)):
            raise AgentDeliveryError("invalid terminal state")
        outcome = value.get("outcome")
        if not isinstance(outcome, str):
            raise AgentDeliveryError("invalid terminal state outcome")
        return cls.create(
            outcome, cast(dict[str, object], value["evidence"]),
        )

    def to_document(self) -> dict[str, object]:
        return {
            "schema": _TERMINAL_STATE_SCHEMA,
            "outcome": self.outcome,
            "evidence": dict(self.evidence),
        }


@dataclass
class AgentRecord:
    """Canonical session state with one immutable launch-intent authority."""
    name: str
    token: str
    launch: LaunchSpec
    created_at: float
    lifecycle: str = "starting"
    workspace_id: str | None = None
    tab_id: str | None = None
    pane_id: str | None = None
    session_agent: str | None = None
    session_value: str | None = None
    session_source: str | None = None
    startup_warning: str | None = None
    effective_reasoning_effort: str | None = None
    error: str | None = None
    goal: str | None = None
    goal_command: list[str] | None = None
    goal_message_id: str | None = None
    paused: bool = False
    pane_reported_by_agentctl: bool = False
    custom_process_identity: CustomProcessIdentity | None = None
    foreign_shell_identity: CustomProcessIdentity | None = None
    runner_identity: CustomProcessIdentity | None = None
    terminal: TerminalState | None = None
    _legacy_runner_pid: int | None = field(default=None, repr=False)
    _legacy_runner_started_at: str | None = field(default=None, repr=False)
    _legacy_goal_delivery: str | None = field(default=None, repr=False)
    _legacy_goal_messages: dict[str, str] = field(default_factory=dict, repr=False)
    _legacy_goal_pointer: bool = field(default=False, repr=False)
    _legacy_terminal_authority: bool = field(default=False, repr=False)
    _unknown: dict[str, object] = field(default_factory=dict, init=False, repr=False)

    @property
    def arguments(self) -> list[str]:
        """Compatibility view derived from the canonical launch vector."""
        return self.launch.arguments

    @property
    def goal_delivery(self) -> str | None:
        """Decode-edge fallback for rows predating queue-derived delivery state."""
        return self._legacy_goal_delivery

    @property
    def goal_messages(self) -> dict[str, str]:
        """Decode-edge map migrated into tagged queue artifacts on the next write."""
        return self._legacy_goal_messages

    @property
    def goal_session_id(self) -> str | None:
        """Compatibility projection of the one native-session authority."""
        return self.session_value

    @property
    def runner_pid(self) -> int | None:
        """Compatibility projection of the boot-bound runner identity."""
        if self.runner_identity is not None:
            return self.runner_identity.pid
        return self._legacy_runner_pid

    @property
    def runner_started_at(self) -> str | None:
        """Compatibility projection of the boot-bound runner identity."""
        if self.runner_identity is not None:
            return str(self.runner_identity.starttime_ticks)
        return self._legacy_runner_started_at

    def set_runner_identity(self, identity: CustomProcessIdentity | None) -> None:
        """Replace the runtime identity and retire any decode-only PID fallback."""
        self.runner_identity = identity
        self._legacy_runner_pid = None
        self._legacy_runner_started_at = None

    def to_document(self) -> dict[str, object]:
        """Return the stable public compatibility view."""
        document = asdict(self)
        document.pop("_unknown")
        document.pop("_legacy_runner_pid")
        document.pop("_legacy_runner_started_at")
        document.pop("_legacy_goal_delivery")
        document.pop("_legacy_goal_messages")
        document.pop("_legacy_goal_pointer")
        document.pop("_legacy_terminal_authority")
        document.pop("launch")
        document.pop("terminal")
        if self.terminal is not None:
            document["terminal"] = self.terminal.to_document()
        launch = self.launch
        executable = launch.executable
        document.update({
            "schema": 1,
            "harness": launch.harness,
            "cwd": launch.cwd,
            "adapter": launch.adapter,
            "mode": launch.mode,
            "backend": launch.backend,
            "model": launch.model,
            "resume": launch.resume,
            "launch_profile": launch.profile,
            "launch_executable": executable[0] if executable is not None else None,
            "launch_executable_device": executable[1] if executable is not None else None,
            "launch_executable_inode": executable[2] if executable is not None else None,
            "launch_argv": list(launch.argv),
            "launch_environment_names": list(launch.environment_names),
            "runtime_home": launch.runtime_home,
            "runtime_ownership": launch.runtime_ownership,
            "launch_permission_mode": launch.permission_mode,
            "session_source": self.session_source,
            # Compatibility view. Current storage has one native-session
            # authority and does not retain a separate goal-session value.
            "goal_session_id": self.session_value,
            "goal_delivery": self._legacy_goal_delivery,
            "goal_messages": {},
            # Legacy consumers may still display these scalar fields, but
            # current rows derive them from the one boot/image-bound identity.
            "runner_pid": self.runner_pid,
            "runner_started_at": self.runner_started_at,
        })
        document["arguments"] = self.arguments
        if self._unknown.keys() & document.keys():
            raise AgentDeliveryError("unknown agent metadata conflicts with a known schema field")
        document.update(self._unknown)
        return document

    def to_storage_document(self) -> dict[str, object]:
        """Return the strict current schema with canonical launch and goal state."""
        # Reuse the public-view collision check before placing extensions into
        # their dedicated namespace.
        self.to_document()
        launch = self.launch
        if self.runner_identity is None and (
            self._legacy_runner_pid is not None
            or self._legacy_runner_started_at is not None
        ):
            raise AgentDeliveryError(
                "cannot migrate a PID/start-only runner without boot-bound identity"
            )
        if ((self.session_value is None) != (self.session_agent is None)
                or (self.session_value is None) != (self.session_source is None)
                or self.session_source not in (None, "observed", "asserted")
                or (self.session_agent is not None
                    and self.session_agent != launch.harness)):
            raise AgentDeliveryError("incomplete native session identity")
        self._validate_runtime_shape(launch)
        if self.lifecycle == "stopped" and self.terminal is None:
            raise AgentDeliveryError(
                "current stopped agent record has no canonical terminal state"
            )
        if self.lifecycle != "stopped" and self.terminal is not None:
            raise AgentDeliveryError(
                "nonterminal agent record cannot contain terminal state"
            )
        native_session: dict[str, object] | None = None
        if self.session_value is not None:
            native_session = {
                "schema": _NATIVE_SESSION_SCHEMA,
                "agent": self.session_agent or launch.harness,
                "value": self.session_value,
                "source": self.session_source or "observed",
            }
        document: dict[str, object] = {
            "schema": _SESSION_STORAGE_SCHEMA,
            "name": self.name,
            "token": self.token,
            "created_at": self.created_at,
            "lifecycle": self.lifecycle,
            "launch": launch.to_document(),
            "workspace_id": self.workspace_id,
            "tab_id": self.tab_id,
            "pane_id": self.pane_id,
            "native_session": native_session,
            "startup_warning": self.startup_warning,
            "effective_reasoning_effort": self.effective_reasoning_effort,
            "error": self.error,
            "goal": {
                "schema": _GOAL_STATE_SCHEMA,
                "objective": self.goal,
                "message_id": self.goal_message_id,
                "native_command": self.goal_command,
            },
            "paused": self.paused,
            "pane_reported_by_agentctl": self.pane_reported_by_agentctl,
            "custom_process_identity": (
                asdict(self.custom_process_identity)
                if self.custom_process_identity is not None else None
            ),
            "foreign_shell_identity": (
                asdict(self.foreign_shell_identity)
                if self.foreign_shell_identity is not None else None
            ),
            "runner_identity": (
                asdict(self.runner_identity) if self.runner_identity is not None else None
            ),
            "terminal": (
                None if self.terminal is None else self.terminal.to_document()
            ),
            "extensions": dict(self._unknown),
        }
        # Validate the serialized form itself so in-memory callers cannot
        # publish a state that only a later reader would reject.
        self._from_value(document, Path("<in-memory>"), self.name)
        return document

    def _validate_runtime_shape(self, launch: LaunchSpec) -> None:
        if self.lifecycle not in {
            "starting", "running", "stopping", "stopped", "launch_failed", "adopt_failed",
        }:
            raise AgentDeliveryError(f"invalid lifecycle {self.lifecycle!r}")
        expected_program = (
            launch.executable[0] if launch.executable is not None else launch.harness
        )
        launch_shape = (
            _KIND.fullmatch(launch.harness) is not None
            and Path(launch.cwd).is_absolute()
            and all(
                value is None or (bool(value) and "\0" not in value)
                for value in (launch.model, launch.resume, launch.profile)
            )
            and all(value and "\0" not in value for value in launch.argv)
            and all(_ENVIRONMENT_NAME.fullmatch(value) is not None
                    for value in launch.environment_names)
            and (
                (launch.adapter == "herdr-foreign" and not launch.argv)
                or (bool(launch.argv) and launch.argv[0] == expected_program)
            )
        )
        # Interactive adapters launch the harness directly, so the structured
        # model/resume arguments must be reflected in argv.  The turn-runner
        # protocol carries those fields separately and argv contains only its
        # harness-specific extra arguments; requiring the interactive prefix
        # there would reject every canonical headless record we write.
        if launch.adapter in ("herdr", "herdr-pane"):
            structured = harness_arguments(
                launch.harness, model=launch.model, resume=launch.resume,
            )
            launch_shape = launch_shape and tuple(
                launch.argv[1:1 + len(structured)]
            ) == structured
        if launch.adapter == "turn-runner":
            valid = (
                launch.mode == "headless"
                and launch.backend in ("herdr", "tmux")
                and launch.runtime_ownership == "owned"
                and launch.runtime_home is not None
                and Path(launch.runtime_home).is_absolute()
                and self.custom_process_identity is None
                and self.foreign_shell_identity is None
                and launch.permission_mode in (None, "native", "bypass")
                and (launch.permission_mode != "bypass" or launch.harness == "codex")
            )
        else:
            valid = (
                launch.adapter in ("herdr", "herdr-pane", "herdr-foreign")
                and launch.mode == "interactive"
                and launch.backend == "herdr"
                and launch.runtime_home is None
                and self.runner_identity is None
                and self._legacy_runner_pid is None
                and self._legacy_runner_started_at is None
                and launch.permission_mode is None
            )
            if launch.adapter == "herdr":
                valid = valid and launch.runtime_ownership == "owned"
                valid = valid and self.custom_process_identity is None
                valid = valid and self.foreign_shell_identity is None
            elif launch.adapter == "herdr-pane":
                valid = valid and launch.runtime_ownership == "owned" and launch.harness == "muse"
                valid = valid and self.foreign_shell_identity is None
            else:
                valid = valid and launch.runtime_ownership == "foreign"
                valid = valid and self.custom_process_identity is None
        if not launch_shape or not valid:
            raise AgentDeliveryError(
                "adapter, mode, backend, ownership, and runtime identity are inconsistent"
            )

    @classmethod
    def load(cls, path: Path, name: str) -> AgentRecord:
        """Reject malformed or non-private state before using any recorded identity."""
        value = agent._read_queue_json(
            str(path), "agent record", require_private=True,
            max_artifact_bytes=_MAX_AGENT_RECORD_BYTES,
        )
        return cls._from_value(value, path, name)

    @classmethod
    def _from_value(cls, value: object, path: Path, name: str) -> AgentRecord:
        """Validate an already identity-bound record value."""
        if not isinstance(value, dict):
            raise AgentDeliveryError(f"invalid agent record: {path}")
        source_schema = value.get("schema")
        legacy_goal_pointer = (
            (source_schema == 1 and not isinstance(source_schema, bool))
            or source_schema == _LEGACY_SESSION_STORAGE_SCHEMA
        )
        document = cls._normalize_storage_value(cast(dict[str, object], value), path)
        for key in ("name", "token", "harness", "cwd", "lifecycle"):
            if not isinstance(document.get(key), str) or not document[key]:
                raise AgentDeliveryError(f"invalid agent record field {key}: {path}")
        for key in ("workspace_id", "tab_id", "pane_id", "session_agent", "session_value", "session_source", "model", "resume", "startup_warning", "effective_reasoning_effort", "error", "goal", "goal_delivery", "goal_session_id", "goal_message_id", "launch_profile", "launch_executable", "runtime_ownership", "runner_started_at", "launch_permission_mode"):
            if document.get(key) is not None and not isinstance(document[key], str):
                raise AgentDeliveryError(f"invalid agent record field {key}: {path}")
        created = document.get("created_at")
        args = document.get("arguments")
        goal_command = document.get("goal_command")
        launch_argv = document.get("launch_argv", [])
        launch_environment_names = document.get("launch_environment_names", [])
        if (document.get("schema") != 1 or isinstance(document.get("schema"), bool)
            or document["name"] != name or not isinstance(created, (int, float))
            or isinstance(created, bool) or not math.isfinite(created)
            or re.fullmatch(r"[a-z0-9-]{1,80}", str(document["token"])) is None
            or not isinstance(args, list) or any(not isinstance(item, str) for item in args)
            or not isinstance(launch_argv, list)
            or any(not isinstance(item, str) or not item or "\0" in item for item in launch_argv)
            or not isinstance(launch_environment_names, list)
            or any(not isinstance(item, str) or _ENVIRONMENT_NAME.fullmatch(item) is None
                   for item in launch_environment_names)):
            raise AgentDeliveryError(f"invalid agent record: {path}")
        for key in ("launch_executable_device", "launch_executable_inode"):
            value = document.get(key)
            if (value is not None
                    and (not isinstance(value, int) or isinstance(value, bool)
                         or not 1 <= value <= _MAX_U64)):
                raise AgentDeliveryError(f"invalid agent record field {key}: {path}")
        if document.get("runtime_ownership") not in (None, "owned", "foreign"):
            raise AgentDeliveryError(f"invalid runtime ownership in {path}")
        if document.get("session_source") not in (None, "observed", "asserted"):
            raise AgentDeliveryError(f"invalid native session source in {path}")
        if ((document.get("session_value") is None)
                != (document.get("session_source") is None)):
            raise AgentDeliveryError(f"incomplete native session identity in {path}")
        if ((document.get("session_value") is None)
                != (document.get("session_agent") is None)):
            raise AgentDeliveryError(f"incomplete native session identity in {path}")
        if (document.get("session_agent") is not None
                and document.get("session_agent") != document.get("harness")):
            raise AgentDeliveryError(f"native session harness mismatch in {path}")
        runner_pid = document.get("runner_pid")
        runner_started_at = document.get("runner_started_at")
        if (runner_pid is not None
                and (not isinstance(runner_pid, int) or isinstance(runner_pid, bool)
                     or not 1 <= runner_pid <= _MAX_PROCESS_ID)):
            raise AgentDeliveryError(f"invalid runner pid in {path}")
        if (runner_started_at is not None
                and (not isinstance(runner_started_at, str)
                     or not runner_started_at.isascii() or not runner_started_at.isdigit()
                     or not any(character != "0" for character in runner_started_at))):
            raise AgentDeliveryError(f"invalid runner start time in {path}")
        if (runner_pid is None) != (runner_started_at is None):
            raise AgentDeliveryError(f"incomplete runner identity in {path}")
        if ((document.get("launch_executable_device") is None)
                != (document.get("launch_executable_inode") is None)):
            raise AgentDeliveryError(f"incomplete launch executable identity in {path}")
        launch_executable = document.get("launch_executable")
        if ((launch_executable is None)
                != (document.get("launch_executable_device") is None)):
            raise AgentDeliveryError(f"incomplete launch executable intent in {path}")
        if (launch_executable is not None
                and (not isinstance(launch_executable, str)
                     or not os.path.isabs(launch_executable)
                     or not launch_argv or launch_argv[0] != launch_executable
                     or document.get("launch_executable_device") is None)):
            raise AgentDeliveryError(f"invalid launch executable intent in {path}")
        if goal_command is not None and (not isinstance(goal_command, list) or not goal_command
            or any(not isinstance(item, str) or not item or "\0" in item for item in goal_command)):
            raise AgentDeliveryError(f"invalid goal command in agent record: {path}")
        goals = document.get("goal_messages", {})
        if not isinstance(goals, dict) or any(not isinstance(key, str) or agent._MESSAGE_ID.fullmatch(key) is None
            or not isinstance(value, str) or not value for key, value in goals.items()):
            raise AgentDeliveryError(f"invalid goal messages in agent record: {path}")
        goal_message_id = document.get("goal_message_id")
        if goal_message_id is not None and (not isinstance(goal_message_id, str) or agent._MESSAGE_ID.fullmatch(goal_message_id) is None):
            raise AgentDeliveryError(f"invalid goal message id in agent record: {path}")
        if goal_message_id is not None and document.get("goal") is None:
            raise AgentDeliveryError(f"goal message id has no objective in agent record: {path}")
        if (isinstance(goal_message_id, str) and goal_message_id in goals
                and goals[goal_message_id] != document.get("goal")):
            raise AgentDeliveryError(
                f"legacy goal pointer disagrees with its duplicate objective map in {path}"
            )
        if document.get("goal_delivery") not in (
            None, "pending", "possibly_submitted", "delivered",
        ):
            raise AgentDeliveryError(f"invalid goal delivery in agent record: {path}")
        warning = document.get("startup_warning")
        effective_effort = document.get("effective_reasoning_effort")
        if (isinstance(warning, str)
                and (len(warning) > 256 or not warning.isascii()
                     or any(character in warning for character in "\r\n\0"))):
            raise AgentDeliveryError(f"invalid startup warning in agent record: {path}")
        if (isinstance(effective_effort, str)
                and effective_effort not in (
                    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"
                )):
            raise AgentDeliveryError(f"invalid effective reasoning effort in agent record: {path}")
        session_fields = {
            key for key, field_info in cls.__dataclass_fields__.items()
            if field_info.init and key != "launch"
        }
        fields = {key: document[key] for key in session_fields if key in document}
        fields["_legacy_goal_delivery"] = document.get("goal_delivery")
        fields["_legacy_goal_messages"] = cast(dict[str, str], goals)
        fields["_legacy_goal_pointer"] = legacy_goal_pointer
        raw_terminal = document.get("terminal")
        terminal = (
            None if raw_terminal is None else TerminalState.from_document(raw_terminal)
        )
        lifecycle = cast(str, document["lifecycle"])
        if terminal is not None and lifecycle != "stopped":
            raise AgentDeliveryError(
                f"nonterminal agent record contains terminal state: {path}"
            )
        if source_schema == _SESSION_STORAGE_SCHEMA and (
            (lifecycle == "stopped") != (terminal is not None)
        ):
            raise AgentDeliveryError(
                f"current agent record has inconsistent terminal state: {path}"
            )
        fields["terminal"] = terminal
        fields["_legacy_terminal_authority"] = (
            source_schema in {
                1, _PREVIOUS_SESSION_STORAGE_SCHEMA,
                _LEGACY_SESSION_STORAGE_SCHEMA,
            }
            and lifecycle == "stopped"
            and terminal is None
        )
        if document.get("adapter", "herdr") not in ("herdr", "herdr-pane", "herdr-foreign", "turn-runner"):
            raise AgentDeliveryError(f"unsupported runtime adapter in {path}")
        if document.get("mode", "interactive") not in ("interactive", "headless"):
            raise AgentDeliveryError(f"invalid execution mode in {path}")
        if document.get("backend", "herdr") not in ("herdr", "tmux"):
            raise AgentDeliveryError(f"invalid terminal backend in {path}")
        if not isinstance(document.get("paused", False), bool):
            raise AgentDeliveryError(f"invalid pause state in {path}")
        if not isinstance(document.get("pane_reported_by_agentctl", False), bool):
            raise AgentDeliveryError(f"invalid pane report ownership in {path}")
        def process_identity(key: str) -> CustomProcessIdentity | None:
            raw_identity = document.get(key)
            if raw_identity is None:
                return None
            if (not isinstance(raw_identity, dict)
                    or set(raw_identity) != {
                        "version", "boot_id", "pid", "starttime_ticks",
                        "executable_device", "executable_inode",
                    }):
                raise AgentDeliveryError(f"invalid {key.replace('_', ' ')} in {path}")
            identity = cast(dict[str, object], raw_identity)
            integers = [
                identity.get("version"), identity.get("pid"),
                identity.get("starttime_ticks"), identity.get("executable_device"),
                identity.get("executable_inode"),
            ]
            boot_id = identity.get("boot_id")
            if (any(not isinstance(value, int) or isinstance(value, bool) for value in integers)
                    or identity.get("version") != 1
                    or not isinstance(boot_id, str)
                    or re.fullmatch(
                        r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
                        boot_id,
                    ) is None
                    or not 1 <= cast(int, identity["pid"]) <= 2_147_483_647
                    or not 1 <= cast(int, identity["starttime_ticks"]) <= _MAX_U64
                    or not 1 <= cast(int, identity["executable_device"]) <= _MAX_U64
                    or not 1 <= cast(int, identity["executable_inode"]) <= _MAX_U64):
                raise AgentDeliveryError(f"invalid {key.replace('_', ' ')} in {path}")
            return CustomProcessIdentity(
                version=1, boot_id=boot_id, pid=cast(int, identity["pid"]),
                starttime_ticks=cast(int, identity["starttime_ticks"]),
                executable_device=cast(int, identity["executable_device"]),
                executable_inode=cast(int, identity["executable_inode"]),
            )
        custom_identity = process_identity("custom_process_identity")
        foreign_shell_identity = process_identity("foreign_shell_identity")
        runner_identity = process_identity("runner_identity")
        if custom_identity is not None:
            fields["custom_process_identity"] = custom_identity
        if foreign_shell_identity is not None:
            fields["foreign_shell_identity"] = foreign_shell_identity
        if runner_identity is not None:
            fields["runner_identity"] = runner_identity
            if (runner_pid is not None and runner_pid != runner_identity.pid) or (
                runner_started_at is not None
                and runner_started_at != str(runner_identity.starttime_ticks)
            ):
                raise AgentDeliveryError(f"contradictory runner identity in {path}")
        elif runner_pid is not None:
            fields["_legacy_runner_pid"] = runner_pid
            fields["_legacy_runner_started_at"] = runner_started_at
        if (custom_identity is not None
                and (document.get("adapter", "herdr") != "herdr-pane"
                     or document.get("harness") != "muse"
                     or not isinstance(document.get("pane_id"), str)
                     or not document["pane_id"])):
            raise AgentDeliveryError(
                f"custom process identity requires a Muse herdr-pane in {path}"
            )
        if (foreign_shell_identity is not None
                and (document.get("adapter", "herdr") != "herdr-foreign"
                     or not isinstance(document.get("pane_id"), str)
                     or not document["pane_id"])):
            raise AgentDeliveryError(
                f"foreign shell identity requires a herdr-foreign pane in {path}"
            )
        home = document.get("runtime_home")
        if home is not None and (not isinstance(home, str) or not Path(home).is_absolute()):
            raise AgentDeliveryError(f"invalid runtime directory in {path}")
        adapter = str(document.get("adapter", "herdr"))
        harness = cast(str, document["harness"])
        argv = tuple(cast(list[str], launch_argv))
        if not argv and adapter != "herdr-foreign":
            argv = (harness,)
        executable = (
            None if launch_executable is None else (
                launch_executable,
                cast(int, document["launch_executable_device"]),
                cast(int, document["launch_executable_inode"]),
            )
        )
        fields["launch"] = LaunchSpec(
            harness=harness,
            cwd=cast(str, document["cwd"]),
            adapter=adapter,
            mode=str(document.get("mode", "interactive")),
            backend=str(document.get("backend", "herdr")),
            model=cast(str | None, document.get("model")),
            resume=cast(str | None, document.get("resume")),
            profile=cast(str | None, document.get("launch_profile")),
            argv=argv,
            environment_names=tuple(cast(list[str], launch_environment_names)),
            runtime_home=home,
            runtime_ownership=str(document.get("runtime_ownership", "owned")),
            executable=executable,
            permission_mode=cast(str | None, document.get("launch_permission_mode")),
        )
        record = cls(**fields)  # type: ignore[arg-type]
        compatibility_fields = {"schema", *_SESSION_COMPATIBILITY_FIELDS}
        record._unknown = {
            key: value for key, value in document.items()
            if key not in session_fields and key not in compatibility_fields
        }
        record._validate_runtime_shape(record.launch)
        return record

    @classmethod
    def _normalize_storage_value(
        cls, document: dict[str, object], path: Path,
    ) -> dict[str, object]:
        """Translate current/v2 storage or one supported flat row at the decode edge."""
        storage_schema = document.get("schema")
        if storage_schema in (
            _SESSION_STORAGE_SCHEMA,
            _PREVIOUS_SESSION_STORAGE_SCHEMA,
            _LEGACY_SESSION_STORAGE_SCHEMA,
        ):
            common_fields = {
                "schema", "name", "token", "created_at", "lifecycle", "launch",
                "workspace_id", "tab_id", "pane_id",
                "startup_warning", "effective_reasoning_effort", "error",
                "paused", "pane_reported_by_agentctl",
                "custom_process_identity", "foreign_shell_identity", "runner_identity",
                "extensions",
            }
            if storage_schema == _SESSION_STORAGE_SCHEMA:
                top_fields = common_fields | {"native_session", "goal", "terminal"}
            elif storage_schema == _PREVIOUS_SESSION_STORAGE_SCHEMA:
                top_fields = common_fields | {"native_session", "goal"}
            else:
                top_fields = common_fields | {
                    "session_agent", "session_value",
                    "goal", "goal_delivery", "goal_session_id", "goal_command",
                    "goal_messages", "goal_message_id",
                }
            if set(document) != top_fields:
                raise AgentDeliveryError(
                    f"invalid agent record {storage_schema} fields: {path}"
                )
            launch_value = document.get("launch")
            if not isinstance(launch_value, dict):
                raise AgentDeliveryError(f"invalid agent record launch specification: {path}")
            launch = cast(dict[str, object], launch_value)
            launch_fields = {
                "schema", "harness", "cwd", "adapter", "mode", "backend", "model",
                "resume", "profile", "argv", "environment_names", "runtime_home",
                "runtime_ownership", "executable",
            }
            launch_schema = launch.get("schema")
            if launch_schema == _LAUNCH_SPEC_SCHEMA:
                launch_fields.add("permission_mode")
            elif launch_schema != _LEGACY_LAUNCH_SPEC_SCHEMA:
                raise AgentDeliveryError(f"invalid agent record launch schema: {path}")
            if set(launch) != launch_fields:
                raise AgentDeliveryError(f"invalid agent record launch fields: {path}")
            extensions = document.get("extensions")
            canonical_extension_fields = (
                top_fields
                | launch_fields
                | {
                    key for key, field_info in cls.__dataclass_fields__.items()
                    if field_info.init
                }
                | _SESSION_COMPATIBILITY_FIELDS
            )
            if not isinstance(extensions, dict) or any(
                not isinstance(key, str) or key in canonical_extension_fields
                for key in extensions
            ):
                raise AgentDeliveryError(f"invalid agent record extensions: {path}")
            executable_value = launch.get("executable")
            if executable_value is None:
                executable: dict[str, object] = {}
            elif (isinstance(executable_value, dict)
                    and set(executable_value) == {"path", "device", "inode"}):
                executable = cast(dict[str, object], executable_value)
            else:
                raise AgentDeliveryError(f"invalid agent record launch executable identity: {path}")
            argv = launch.get("argv")
            if not isinstance(argv, list):
                raise AgentDeliveryError(f"invalid agent record launch argv: {path}")
            normalized = {
                key: value for key, value in document.items()
                if key not in {"schema", "launch", "extensions", "native_session"}
            }
            if storage_schema in (
                _SESSION_STORAGE_SCHEMA, _PREVIOUS_SESSION_STORAGE_SCHEMA,
            ):
                goal_value = document.get("goal")
                if (not isinstance(goal_value, dict)
                        or set(goal_value) != {
                            "schema", "objective", "message_id",
                            "native_command",
                        }
                        or goal_value.get("schema") != _GOAL_STATE_SCHEMA):
                    raise AgentDeliveryError(
                        f"invalid agent record goal state: {path}"
                    )
                normalized.update({
                    "goal": goal_value.get("objective"),
                    "goal_delivery": None,
                    "goal_command": goal_value.get("native_command"),
                    "goal_messages": {},
                    "goal_message_id": goal_value.get("message_id"),
                })
                native_value = document.get("native_session")
                if native_value is None:
                    normalized.update({
                        "session_agent": None,
                        "session_value": None,
                        "session_source": None,
                        "goal_session_id": None,
                    })
                elif (isinstance(native_value, dict)
                        and set(native_value) == {
                            "schema", "agent", "value", "source",
                        }
                        and native_value.get("schema") == _NATIVE_SESSION_SCHEMA):
                    normalized.update({
                        "session_agent": native_value.get("agent"),
                        "session_value": native_value.get("value"),
                        "session_source": native_value.get("source"),
                        "goal_session_id": native_value.get("value"),
                    })
                else:
                    raise AgentDeliveryError(
                        f"invalid agent record native session: {path}"
                    )
                if storage_schema == _PREVIOUS_SESSION_STORAGE_SCHEMA:
                    normalized["terminal"] = None
            else:
                observed = normalized.get("session_value")
                asserted = normalized.get("goal_session_id")
                if (observed is not None and asserted is not None
                        and observed != asserted):
                    raise AgentDeliveryError(
                        f"contradictory native session identities in {path}"
                    )
                value = observed if observed is not None else asserted
                normalized["session_value"] = value
                if value is not None and normalized.get("session_agent") is None:
                    normalized["session_agent"] = launch.get("harness")
                normalized["session_source"] = (
                    "observed" if observed is not None else
                    "asserted" if asserted is not None else None
                )
                normalized["goal_session_id"] = value
            normalized.update({
                "schema": 1,
                "harness": launch.get("harness"),
                "cwd": launch.get("cwd"),
                "adapter": launch.get("adapter"),
                "mode": launch.get("mode"),
                "backend": launch.get("backend"),
                "model": launch.get("model"),
                "resume": launch.get("resume"),
                "launch_profile": launch.get("profile"),
                "launch_argv": argv,
                "arguments": list(argv[1:]),
                "launch_environment_names": launch.get("environment_names"),
                "runtime_home": launch.get("runtime_home"),
                "runtime_ownership": launch.get("runtime_ownership"),
                "launch_executable": executable.get("path"),
                "launch_executable_device": executable.get("device"),
                "launch_executable_inode": executable.get("inode"),
                "launch_permission_mode": launch.get("permission_mode"),
            })
            normalized.update(cast(dict[str, object], extensions))
            return normalized

        if document.get("schema") != 1 or isinstance(document.get("schema"), bool):
            raise AgentDeliveryError(f"invalid agent record schema: {path}")
        if any(key in document for key in (
            "launch", "native_session", "extensions", "terminal",
        )):
            raise AgentDeliveryError(
                f"legacy agent record contains a reserved current-schema field: {path}"
            )
        normalized = dict(document)
        args = normalized.get("arguments")
        argv = normalized.get("launch_argv", [])
        if not isinstance(args, list) or not isinstance(argv, list):
            return normalized
        if argv:
            if args and args != argv[1:]:
                raise AgentDeliveryError(f"contradictory launch arguments in {path}")
            normalized["arguments"] = list(argv[1:])
        elif args:
            program = normalized.get("launch_executable") or normalized.get("harness")
            if isinstance(program, str) and program:
                normalized["launch_argv"] = [program, *args]
        if normalized.get("runtime_ownership") is None:
            normalized["runtime_ownership"] = (
                "foreign" if normalized.get("adapter", "herdr") == "herdr-foreign"
                else "owned"
            )
        observed = normalized.get("session_value")
        asserted = normalized.get("goal_session_id")
        if observed is not None and asserted is not None and observed != asserted:
            raise AgentDeliveryError(
                f"contradictory native session identities in {path}"
            )
        value = observed if observed is not None else asserted
        normalized["session_value"] = value
        if value is not None and normalized.get("session_agent") is None:
            normalized["session_agent"] = normalized.get("harness")
        normalized["session_source"] = (
            "observed" if observed is not None else
            "asserted" if asserted is not None else None
        )
        normalized["goal_session_id"] = value
        return normalized

    def target(self) -> agent.Target:
        """Pin the exact pane and, when available at launch, its durable session."""
        if not self.pane_id:
            raise AgentDeliveryError(f"agent {self.name!r} has no confirmed pane; inspect its launch error")
        observed_session = self.session_source == "observed"
        return agent.Target(
            pane_id=self.pane_id,
            session_agent=self.session_agent if observed_session else None,
            session_value=self.session_value if observed_session else None,
            expected_agent=self.launch.harness,
            expected_cwd=self.launch.cwd,
        )


@dataclass(frozen=True)
class _DeadPaneProof:
    """One exact absent-agent, one-pane-tab, idle-shell observation."""

    info: AgentPaneInfo
    presentation: Pane
    shell: PaneShellProof


class AdoptedRuntimeState(str, Enum):
    """Exhaustive result of one side-effect-free adopted-runtime probe."""

    LIVE_EXACT = "live-exact"
    IDLE_SHELL_EXACT = "idle-shell-exact"
    MISSING = "missing"
    AMBIGUOUS = "ambiguous"
    IDENTITY_MISMATCH = "identity-mismatch"
    UNKNOWN = "unknown"


@dataclass(frozen=True)
class AdoptedRuntimeEvidence:
    """One deadline-bound snapshot used by every adopted-runtime operation."""

    state: AdoptedRuntimeState
    reason_code: str
    reason: str
    info: AgentPaneInfo | None = None
    presentation: Pane | None = None

    def public_document(self) -> dict[str, str]:
        """Return the stable, presentation-safe evidence fields."""
        return {
            "state": self.state.value,
            "reason_code": self.reason_code,
            "reason": self.reason,
        }

    def require_live(self) -> AgentPaneInfo:
        """Return the exact live pane or fail closed with this probe's reason."""
        if self.state is not AdoptedRuntimeState.LIVE_EXACT or self.info is None:
            raise HerdrUnavailable(self.reason)
        return self.info


@dataclass(frozen=True)
class _LegacyRecordSnapshot:
    """Exact record bytes, parsed meaning, and containing directory generation."""

    record: AgentRecord
    content: bytes
    digest: str
    directory_device: int
    directory_inode: int


@dataclass(frozen=True)
class _ManagedRecordSnapshot:
    """Parsed managed record and its containing directory generation."""

    record: AgentRecord
    content: bytes
    directory_device: int
    directory_inode: int


@dataclass(frozen=True)
class _PinnedAgentDirectory:
    """Open directory generation held from first proof through publication."""

    name: str
    path: Path
    descriptor: int
    device: int
    inode: int


@dataclass(frozen=True)
class _PaneRoute:
    workspace_id: str
    tab_id: str
    pane_id: str

    def to_document(self) -> dict[str, str]:
        return asdict(self)

    @classmethod
    def from_document(cls, raw: object, field_name: str) -> _PaneRoute:
        if not isinstance(raw, dict) or set(raw) != {
            "workspace_id", "tab_id", "pane_id",
        }:
            raise AgentDeliveryError(f"invalid relocation {field_name} route")
        values = [raw[key] for key in ("workspace_id", "tab_id", "pane_id")]
        if any(not isinstance(value, str) or not value or "\0" in value
               for value in values):
            raise AgentDeliveryError(f"invalid relocation {field_name} route")
        return cls(*cast(list[str], values))


@dataclass(frozen=True)
class _RelocationJournal:
    token: str
    terminal_id: str
    old: _PaneRoute
    target_workspace_id: str

    def to_document(self) -> dict[str, object]:
        return {
            "schema": _RELOCATION_SCHEMA,
            "token": self.token,
            "terminal_id": self.terminal_id,
            "old": self.old.to_document(),
            "target_workspace_id": self.target_workspace_id,
            "new_tab": True,
        }

    @classmethod
    def from_document(cls, raw: object) -> _RelocationJournal:
        expected = {
            "schema", "token", "terminal_id", "old",
            "target_workspace_id", "new_tab",
        }
        if (not isinstance(raw, dict) or set(raw) != expected
                or raw.get("schema") != _RELOCATION_SCHEMA
                or raw.get("new_tab") is not True):
            raise AgentDeliveryError("invalid relocation journal")
        token = raw.get("token")
        terminal_id = raw.get("terminal_id")
        target = raw.get("target_workspace_id")
        if any(not isinstance(value, str) or not value or "\0" in value
               for value in (token, terminal_id, target)):
            raise AgentDeliveryError("invalid relocation journal identity")
        return cls(
            cast(str, token), cast(str, terminal_id),
            _PaneRoute.from_document(raw.get("old"), "old"), cast(str, target),
        )


@dataclass(frozen=True)
class _PinnedParentDirectory:
    """Open parent-directory generation used by one namespace transaction."""

    path: Path
    descriptor: int
    device: int
    inode: int


@dataclass(frozen=True)
class _InstalledArtifact:
    """Exact private file generation installed under a pinned directory."""

    name: str
    device: int
    inode: int
    size: int
    digest: str


class _ArchivePublicationUncertainError(AgentDeliveryError):
    """The held directory cannot safely be restored through the active name."""


class _ArtifactNotInstalledError(AgentDeliveryError):
    """An artifact write failed while the prior named generation stayed intact."""


class _ArtifactInstalledError(AgentDeliveryError):
    """An exact artifact was installed before a later bounded operation failed."""

    def __init__(self, message: str, installed: _InstalledArtifact) -> None:
        super().__init__(message)
        self.installed = installed


class _ArtifactPublicationUncertainError(_ArchivePublicationUncertainError):
    """An artifact name no longer has a safely reversible generation."""


def _goal_replacement_selected(screen: str, objective: str) -> bool:
    normalized = " ".join(screen.split())
    wanted = " ".join(objective.split())
    return ("Replace goal?" in normalized
            and any(f"New objective: {wanted} {marker} 1. Replace current goal Set the new objective and start it now" in normalized
                    for marker in ("›", "❯"))
            and "2. Cancel Keep the current goal" in normalized
            and "Press enter to confirm or esc to go back" in normalized)


def _goal_prompt(harness: str, objective: str) -> str:
    """Return the sole durable wire representation of one requested goal."""
    if harness == "codex":
        return f"/goal {objective}"
    return (
        f"Your ongoing goal: {objective}\n"
        "Work toward this goal and report completion or blockers."
    )


class _WorkspaceClient:
    """Add exact workspace checks to every queue readiness probe, after enqueue."""

    def __init__(
        self, client: HerdrClient, record: AgentRecord, *,
        queue: str | None = None, check_prompt: bool = True,
        deadline: float | None = None,
        submission_guard: Callable[[], AbstractContextManager[None]] | None = None,
    ) -> None:
        self.client, self.record = client, record
        self.goal_objective: str | None = None
        self.queue = queue
        self.check_prompt = check_prompt
        # harness, exact text, prior submitted-turn count, prior active UI.
        self.custom_submission: tuple[str, str, int, bool] | None = None
        self.deadline = deadline
        self.submission_guard = submission_guard
        self.adopted_evidence_observation: AdoptedRuntimeEvidence | None = None

    def _timeout(self, purpose: str) -> float:
        assert self.deadline is not None
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            raise HerdrUnavailable(f"{purpose} deadline expired")
        return remaining

    def adopted_evidence(self, *, refresh: bool = False) -> AdoptedRuntimeEvidence:
        """Probe one adopted runtime once, classifying uncertainty fail-closed."""
        if self.record.launch.adapter != "herdr-foreign":
            raise AgentDeliveryError("adopted-runtime evidence requires herdr-foreign")
        if not refresh and self.adopted_evidence_observation is not None:
            return self.adopted_evidence_observation

        def observed(
            state: AdoptedRuntimeState, code: str, reason: str,
            info: AgentPaneInfo | None = None, presentation: Pane | None = None,
        ) -> AdoptedRuntimeEvidence:
            result = AdoptedRuntimeEvidence(state, code, reason, info, presentation)
            self.adopted_evidence_observation = result
            return result

        pane_id = self.record.pane_id
        shell_identity = self.record.foreign_shell_identity
        if pane_id is None or shell_identity is None:
            return observed(
                AdoptedRuntimeState.UNKNOWN,
                "runtime-probe-failed",
                f"legacy record has no identity-bound pane shell for adopted "
                f"agent {self.record.name!r}",
            )
        try:
            panes = (
                self.client.panes(timeout=self._timeout("adopted pane list"))
                if self.deadline is not None and isinstance(self.client, HerdrClient)
                else self.client.panes()
            )
        except HerdrRunError as exc:
            return observed(
                AdoptedRuntimeState.UNKNOWN, "runtime-probe-failed", str(exc),
            )
        matches = [pane for pane in panes if pane.pane_id == pane_id]
        if not matches:
            return observed(
                AdoptedRuntimeState.MISSING, "pane-missing",
                "expected one recorded pane, found 0",
            )
        if len(matches) != 1:
            return observed(
                AdoptedRuntimeState.AMBIGUOUS, "pane-identity-ambiguous",
                f"expected one recorded pane, found {len(matches)}",
            )
        presentation = matches[0]
        if presentation.workspace_id != self.record.workspace_id:
            return observed(
                AdoptedRuntimeState.IDENTITY_MISMATCH, "runtime-identity-mismatch",
                f"adopted pane {pane_id!r} workspace identity changed",
                presentation=presentation,
            )
        try:
            info = (
                self.client.pane_info(
                    pane_id, timeout=self._timeout("adopted pane identity probe"),
                )
                if self.deadline is not None and isinstance(self.client, HerdrClient)
                else self.client.pane_info(pane_id)
            )
        except HerdrRunError as exc:
            return observed(
                AdoptedRuntimeState.UNKNOWN, "runtime-probe-failed", str(exc),
                presentation=presentation,
            )
        if (info.pane_id != pane_id or info.workspace_id != self.record.workspace_id
                or os.path.realpath(info.cwd) != os.path.realpath(self.record.launch.cwd)):
            return observed(
                AdoptedRuntimeState.IDENTITY_MISMATCH, "runtime-identity-mismatch",
                f"adopted pane {pane_id!r} route or cwd identity changed",
                info, presentation,
            )
        try:
            if self.deadline is not None and isinstance(self.client, HerdrClient):
                self.client.verify_pane_shell_identity(
                    pane_id, shell_identity,
                    timeout=self._timeout("adopted shell identity probe"),
                )
            else:
                self.client.verify_pane_shell_identity(pane_id, shell_identity)
        except RuntimeIdentityMismatch as exc:
            return observed(
                AdoptedRuntimeState.IDENTITY_MISMATCH,
                "runtime-identity-mismatch", str(exc), info, presentation,
            )
        except HerdrRunError as exc:
            return observed(
                AdoptedRuntimeState.UNKNOWN,
                "runtime-probe-failed", str(exc), info, presentation,
            )
        if info.agent == self.record.launch.harness:
            if (self.record.session_source == "observed"
                    and ((self.record.session_agent is not None
                          and info.session_agent != self.record.session_agent)
                         or (self.record.session_value is not None
                             and info.session_value != self.record.session_value))):
                return observed(
                    AdoptedRuntimeState.IDENTITY_MISMATCH,
                    "runtime-identity-mismatch",
                    f"adopted pane {pane_id!r} is not exactly one live pane "
                    "with the recorded native session identity",
                    info, presentation,
                )
            return observed(
                AdoptedRuntimeState.LIVE_EXACT, "ok",
                "adopted harness and shell generation are live and exact",
                info, presentation,
            )
        if info.agent is not None:
            return observed(
                AdoptedRuntimeState.IDENTITY_MISMATCH,
                "expected-harness-missing",
                f"pane {pane_id!r} reports agent {info.agent!r}, "
                f"expected {self.record.launch.harness!r}",
                info, presentation,
            )
        if info.session_agent is not None or info.session_value is not None:
            return observed(
                AdoptedRuntimeState.IDENTITY_MISMATCH,
                "runtime-identity-mismatch",
                f"absent adopted agent in pane {pane_id!r} retains native session identity",
                info, presentation,
            )
        try:
            idle = (
                self.client.pane_is_same_idle_shell(
                    pane_id, shell_identity,
                    timeout=self._timeout("adopted idle-shell proof"),
                )
                if self.deadline is not None and isinstance(self.client, HerdrClient)
                else self.client.pane_is_same_idle_shell(pane_id, shell_identity)
            )
        except HerdrRunError as exc:
            return observed(
                AdoptedRuntimeState.UNKNOWN, "runtime-probe-failed", str(exc),
                info, presentation,
            )
        if idle and presentation.tab_id != self.record.tab_id:
            return observed(
                AdoptedRuntimeState.IDENTITY_MISMATCH,
                "runtime-identity-mismatch",
                f"recorded tab changed while adopted agent {self.record.name!r} was absent",
                info, presentation,
            )
        if idle:
            return observed(
                AdoptedRuntimeState.IDLE_SHELL_EXACT, "expected-harness-missing",
                f"adopted pane {pane_id!r} returned to its exact recorded idle shell",
                info, presentation,
            )
        return observed(
            AdoptedRuntimeState.UNKNOWN, "agent-report-missing",
            f"pane {pane_id!r} is not at the recorded identity-bound idle shell process group",
            info, presentation,
        )

    def pane_info(self, pane_id: str) -> AgentPaneInfo:
        # Sessions started by this manager have a second lifecycle identity in
        # Herdr's named-agent registry.  Adopted sessions deliberately do not:
        # assigning or replacing a native name would mutate a runtime this
        # registry does not own.  Their exact pane/session/workspace/cwd/harness
        # assertions remain the authority instead.
        if self.record.launch.adapter == "herdr-foreign" and pane_id == self.record.pane_id:
            return self.adopted_evidence().require_live()
        owned_pane: str | None
        if self.record.launch.adapter == "herdr":
            owned_pane = (
                self.client.agent_pane(
                    self.record.name, timeout=self._timeout("agent identity probe")
                )
                if self.deadline is not None and isinstance(self.client, HerdrClient)
                else self.client.agent_pane(self.record.name)
            )
        else:
            owned_pane = self.record.pane_id
        if owned_pane != self.record.pane_id:
            raise HerdrUnavailable(f"agent {self.record.name!r} no longer owns its recorded pane")
        info = (
            self.client.pane_info(
                pane_id, timeout=self._timeout("pane identity probe")
            )
            if self.deadline is not None and isinstance(self.client, HerdrClient)
            else self.client.pane_info(pane_id)
        )
        if info.workspace_id != self.record.workspace_id:
            raise HerdrUnavailable(f"agent {self.record.name!r} workspace identity changed")
        if pane_id == self.record.pane_id:
            if (self.record.session_source == "observed"
                    and self.record.session_value is not None
                    and info.session_value is not None
                    and info.session_value != self.record.session_value):
                raise HerdrUnavailable(f"agent {self.record.name!r} native session identity changed")
            if info.status in ("idle", "done") and info.agent == "claude":
                try:
                    screen = (
                        self.client.read(
                            pane_id, source="visible", lines=200,
                            timeout=self._timeout("pane readiness probe"),
                        )
                        if self.deadline is not None and isinstance(self.client, HerdrClient)
                        else self.client.read(pane_id, source="visible", lines=200)
                    )
                except HerdrRunError:
                    if self.check_prompt:
                        raise
                else:
                    if (self.check_prompt
                            and "Quick safety check: Is this a project you created or one you trust?" in screen
                            and "No, exit" in screen and "Yes, I trust this folder" in screen):
                        raise HerdrUnavailable("Claude workspace trust prompt requires human attention; no input was submitted")
                    if claude_staged_composer(screen):
                        info = AgentPaneInfo(
                            pane_id=info.pane_id,
                            workspace_id=info.workspace_id,
                            cwd=info.cwd,
                            agent=info.agent,
                            status="staged",
                            session_agent=info.session_agent,
                            session_value=info.session_value,
                        )
                    elif claude_active_screen(screen):
                        info = AgentPaneInfo(
                            pane_id=info.pane_id,
                            workspace_id=info.workspace_id,
                            cwd=info.cwd,
                            agent=info.agent,
                            status="working",
                            session_agent=info.session_agent,
                            session_value=info.session_value,
                        )
            if self.record.launch.adapter == "herdr-pane":
                reported_status = info.status
                if self.deadline is not None and isinstance(self.client, HerdrClient):
                    self.client.verify_custom_harness(
                        pane_id, self.record.launch.harness,
                        self.record.custom_process_identity,
                        timeout=self._timeout("custom harness probe"),
                    )
                    screen = self.client.read(
                        pane_id, source="visible", lines=200,
                        timeout=self._timeout("custom harness readiness probe"),
                    )
                else:
                    self.client.verify_custom_harness(
                        pane_id, self.record.launch.harness, self.record.custom_process_identity,
                    )
                    screen = self.client.read(pane_id, source="visible", lines=200)
                if muse_trust_prompt(screen):
                    raise HerdrUnavailable(
                        "Muse workspace trust prompt requires human attention; no input was submitted"
                    )
                info = AgentPaneInfo(
                    pane_id=info.pane_id,
                    workspace_id=info.workspace_id,
                    cwd=info.cwd,
                    # Herdr's native agent label is advisory for custom
                    # harnesses. Exact executable/process identity above is
                    # the ownership proof and can safely bridge a delayed or
                    # lost report-agent update.
                    agent=self.record.launch.harness,
                    status=(
                        "idle"
                        if (
                            muse_idle_composer(screen)
                            or (
                                reported_status in ("idle", "done")
                                and muse_verified_process_idle_composer(screen)
                            )
                        )
                        else "staged"
                        if (reported_status in ("idle", "done")
                            and muse_verified_process_composer(screen))
                        else "working"
                    ),
                    session_agent=info.session_agent,
                    session_value=info.session_value,
                )
        return info

    def panes(self, workspace_id: str | None = None) -> tuple[Pane, ...]:
        del workspace_id
        if self.record.launch.adapter == "herdr-foreign":
            evidence = self.adopted_evidence()
            return (() if evidence.presentation is None else (evidence.presentation,))
        if self.deadline is not None and isinstance(self.client, HerdrClient):
            return self.client.panes(
                self.record.workspace_id, timeout=self._timeout("pane list probe")
            )
        return self.client.panes(self.record.workspace_id)

    def workspace_label(self, workspace_id: str) -> str:
        if self.deadline is not None and isinstance(self.client, HerdrClient):
            return self.client.workspace_label(
                workspace_id, timeout=self._timeout("workspace identity probe")
            )
        return self.client.workspace_label(workspace_id)

    def prompt_agent(self, pane_id: str, command: str) -> None:
        guard = (
            nullcontext()
            if self.submission_guard is None
            else self.submission_guard()
        )
        with guard:
            self._prompt_agent_under_guard(pane_id, command)

    def _prompt_agent_under_guard(self, pane_id: str, command: str) -> None:
        """Cross the injection boundary while the registry generation is pinned."""
        self.goal_objective = None
        if self.queue is not None and command.startswith("/goal "):
            inflight = Path(self.queue) / "inflight"
            for path in sorted(inflight.glob("*.json")):
                document = agent._read_queue_json(
                    str(path), "goal message", require_private=True,
                )
                if (isinstance(document, dict)
                        and document.get("kind") == "goal"
                        and document.get("text") == command):
                    self.goal_objective = command[len("/goal "):]
                    break
        if self.record.launch.adapter != "herdr-pane":
            if self.record.launch.harness != "claude":
                self.client.prompt_agent(pane_id, command)
                return
            before = self.read(pane_id, source="visible", lines=200)
            prior_count = claude_prompt_transcript_count(before, command)
            prior_active = claude_active_screen(before)
            already_staged = claude_prompt_is_exact_composer(before, command)
            if not already_staged:
                if claude_staged_composer(before):
                    raise HerdrUnavailable(
                        "Claude editor contains different buffered input; no input was sent"
                    )
                self.client.prompt_agent(pane_id, command)
            staged = self.read(pane_id, source="visible", lines=200)
            if not claude_prompt_is_exact_composer(staged, command):
                # An unrecognized screen retains Herdr's native submission and
                # working-transition contract. It cannot authorize extra input.
                return
            # Herdr 0.8 can report agent_prompted after only staging text in
            # Claude's editor. The UI itself names this exact submit chord.
            verified = self.pane_info(pane_id)
            if verified.status != "staged":
                raise HerdrUnavailable(
                    "Claude staged prompt changed before its submit chord"
                )
            self.client.send_keys(pane_id, "ctrl+x ctrl+s")
            self.custom_submission = (
                "claude", command, prior_count, prior_active,
            )
            return
        if "\0" in command or "\x1b" in command:
            raise HerdrUnavailable(
                "Muse pane prompts cannot contain NUL or terminal escape characters"
            )
        info = self.pane_info(pane_id)
        if info.status not in ("idle", "staged"):
            raise HerdrUnavailable(
                f"custom pane {pane_id} is not at a verified idle or staged Muse composer"
            )
        before = self.client.read(pane_id, source="recent-unwrapped", lines=200)
        if not muse_verified_process_composer(before):
            raise HerdrUnavailable(
                "current Muse editor could not be verified before delivery; no input was sent"
            )
        staged = before
        if not muse_verified_process_prompt_is_exact_composer(before, command):
            if not muse_verified_process_idle_composer(before):
                raise HerdrUnavailable(
                    "Muse editor contains different buffered input; no input or Enter was sent"
                )
            self.client.send_text(
                pane_id, f"{_BRACKETED_PASTE_START}{command}{_BRACKETED_PASTE_END}"
            )
            deadline = time.monotonic() + 2.0
            while time.monotonic() < deadline:
                self.client.verify_custom_harness(
                    pane_id, self.record.launch.harness,
                    self.record.custom_process_identity,
                )
                staged = self.client.read(
                    pane_id, source="recent-unwrapped", lines=200,
                )
                if (staged != before
                        and muse_verified_process_prompt_is_exact_composer(
                            staged, command,
                        )):
                    break
                time.sleep(min(0.05, max(0.0, deadline - time.monotonic())))
            else:
                raise HerdrUnavailable(
                    "literal text insertion did not produce exact visible Muse editor evidence; Enter was not sent"
                )
        self.client.verify_custom_harness(
            pane_id, self.record.launch.harness, self.record.custom_process_identity
        )
        confirmed = self.client.read(
            pane_id, source="recent-unwrapped", lines=200,
        )
        if not muse_verified_process_prompt_is_exact_composer(confirmed, command):
            raise HerdrUnavailable(
                "Muse editor changed before submission; Enter was not sent"
            )
        self.client.verify_custom_harness(
            pane_id, self.record.launch.harness, self.record.custom_process_identity
        )
        self.client.send_keys(pane_id, "Enter")
        self.custom_submission = (
            "muse", command,
            muse_verified_process_prompt_transcript_count(confirmed, command),
            False,
        )

    def wait_agent_status(self, pane_id: str, status: str, timeout_ms: int) -> None:
        operation_deadline = time.monotonic() + timeout_ms / 1000.0
        saved_deadline = self.deadline
        self.deadline = (
            operation_deadline if saved_deadline is None
            else min(saved_deadline, operation_deadline)
        )
        try:
            self._wait_agent_status_bounded(pane_id, status, timeout_ms)
        finally:
            self.deadline = saved_deadline

    def _wait_agent_status_bounded(
        self, pane_id: str, status: str, timeout_ms: int,
    ) -> None:
        """Reconcile one transition without exceeding its absolute deadline."""
        if self.custom_submission is not None and status == "working":
            harness, command, prior_count, prior_active = self.custom_submission
            deadline = time.monotonic() + timeout_ms / 1000
            retry_after = time.monotonic() + min(0.25, timeout_ms / 2000)
            retried_enter = False
            while time.monotonic() < deadline:
                if harness == "muse":
                    if isinstance(cast(object, self.client), HerdrClient):
                        self.client.verify_custom_harness(
                            pane_id, self.record.launch.harness,
                            self.record.custom_process_identity,
                            timeout=self._timeout("custom submission identity probe"),
                        )
                    else:
                        self.client.verify_custom_harness(
                            pane_id, self.record.launch.harness,
                            self.record.custom_process_identity,
                        )
                    screen = self.read(
                        pane_id, source="recent-unwrapped", lines=200,
                    )
                    transcript_count = muse_verified_process_prompt_transcript_count(
                        screen, command,
                    )
                    in_composer = muse_verified_process_prompt_in_composer(
                        screen, command,
                    )
                else:
                    self.pane_info(pane_id)
                    # Herdr's native status can lag a healthy Claude process.
                    # Bounded terminal history preserves long submitted turns
                    # after they leave the visible viewport.
                    screen = self.read(
                        pane_id, source="recent-unwrapped", lines=5000,
                    )
                    transcript_count = claude_prompt_transcript_count(screen, command)
                    in_composer = claude_prompt_is_exact_composer(screen, command)
                if transcript_count > prior_count and not in_composer:
                    self.custom_submission = None
                    return
                if (harness == "claude" and not prior_active
                        and not in_composer and claude_active_screen(screen)):
                    # Exact text and one documented submit chord were already
                    # verified. A newly active UI is a sufficient transition
                    # receipt when the full prompt is outside retained history.
                    self.custom_submission = None
                    return
                if (harness == "muse" and not retried_enter
                        and time.monotonic() >= retry_after
                        and transcript_count == prior_count
                        and muse_verified_process_prompt_is_exact_composer(
                            screen, command,
                        )):
                    if isinstance(cast(object, self.client), HerdrClient):
                        self.client.verify_custom_harness(
                            pane_id, self.record.launch.harness,
                            self.record.custom_process_identity,
                            timeout=self._timeout("custom submission retry identity probe"),
                        )
                        self.client.send_keys(
                            pane_id, "Enter",
                            timeout=self._timeout("custom submission retry"),
                        )
                    else:
                        self.client.verify_custom_harness(
                            pane_id, self.record.launch.harness,
                            self.record.custom_process_identity,
                        )
                        self.client.send_keys(pane_id, "Enter")
                    retried_enter = True
                time.sleep(min(0.05, max(0.0, deadline - time.monotonic())))
            raise HerdrUnavailable(
                "agent did not show a verified post-submission screen transition"
            )
        assert self.deadline is not None
        initial_error: HerdrUnavailable | None = None
        try:
            initial_ms = min(
                1000,
                max(1, int(self._timeout("initial agent status wait") * 1000)),
            )
            self.client.wait_agent_status(pane_id, status, initial_ms)
            return
        except HerdrUnavailable as exc:
            # Herdr's wait subscription may be installed after the transition
            # it is waiting for.  Preserve the event failure, but reconcile it
            # against the exact currently owned pane before declaring delivery
            # ambiguous.  This never re-runs the prompt operation.
            initial_error = exc
            info = self.pane_info(pane_id)
            if info.status == status:
                return
        if self.goal_objective is not None and status == "working":
            screen = self.read(pane_id, source="visible", lines=200)
            if _goal_replacement_selected(screen, self.goal_objective):
                if isinstance(cast(object, self.client), HerdrClient):
                    self.client.send_keys(
                        pane_id, "Enter",
                        timeout=self._timeout("goal replacement confirmation"),
                    )
                else:
                    self.client.send_keys(pane_id, "Enter")
        remaining = int(self._timeout("final agent status wait") * 1000)
        if remaining <= 0:
            assert initial_error is not None
            raise initial_error
        try:
            self.client.wait_agent_status(pane_id, status, remaining)
            return
        except HerdrUnavailable as final_error:
            if self.pane_info(pane_id).status == status:
                return
            raise final_error from initial_error

    def read(self, pane_id: str, *, source: str, lines: int) -> str:
        if self.deadline is not None and isinstance(self.client, HerdrClient):
            return self.client.read(
                pane_id, source=source, lines=lines,
                timeout=self._timeout("pane read probe"),
            )
        return self.client.read(pane_id, source=source, lines=lines)


class ManagedAgents:
    """Registry-backed API for a coordinator's visible foreign-harness workers."""

    def __init__(self, client: HerdrClient, registry: str | Path = ".herdr-agents") -> None:
        self.client = client
        self.registry = Path(os.path.abspath(registry))

    def _prepare(self) -> None:
        self.registry.mkdir(mode=0o700, parents=True, exist_ok=True)
        agent._validate_private_directory(str(self.registry), "agent registry", tighten=True)

    @contextmanager
    def _lock(self, name: str) -> Iterator[None]:
        self._prepare()
        # Locks live outside agent directories, so stop/reuse cannot replace a held inode.
        path = self.registry / f".{_name(name)}.lock"
        descriptor = agent._open_private_lock(str(path), "agent lifecycle lock")
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            yield
        finally:
            os.close(descriptor)

    @staticmethod
    def _delivery_identity(record: AgentRecord) -> tuple[object, ...]:
        """Return the immutable generation and runtime route used for submission."""
        return (
            record.token,
            record.launch,
            record.workspace_id,
            record.tab_id,
            record.pane_id,
            record.session_agent,
            record.session_value,
            record.session_source,
            record.custom_process_identity,
            record.foreign_shell_identity,
            record.runner_identity,
        )

    @contextmanager
    def _submission_guard(self, expected: AgentRecord) -> Iterator[None]:
        """Pin one live generation only across its irreversible prompt injection."""
        with self._lock(expected.name):
            current = self._load_expected(expected.name, expected.token)
            if current.lifecycle != "running":
                raise AgentDeliveryError(
                    f"agent {expected.name!r} is no longer running"
                )
            if self._delivery_identity(current) != self._delivery_identity(expected):
                raise AgentDeliveryError(
                    f"agent {expected.name!r} runtime identity changed before submission"
                )
            yield

    def _delivery_client(self, record: AgentRecord) -> HerdrClient:
        """Build the queue client whose injection boundary revalidates the record."""
        return cast(
            HerdrClient,
            _WorkspaceClient(
                self.client,
                record,
                queue=self._queue(record.name),
                submission_guard=lambda: self._submission_guard(record),
            ),
        )

    @contextmanager
    def _try_lock(self, name: str, deadline: float) -> Iterator[bool]:
        """Briefly try one lock without letting a busy row block health scans."""
        self._prepare()
        path = self.registry / f".{_name(name)}.lock"
        descriptor = agent._open_private_lock(str(path), "agent lifecycle lock")
        acquired = False
        lock_deadline = min(deadline, time.monotonic() + 0.05)
        try:
            while True:
                try:
                    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    acquired = True
                    break
                except BlockingIOError:
                    remaining = lock_deadline - time.monotonic()
                    if remaining <= 0:
                        break
                    time.sleep(min(0.005, remaining))
            yield acquired
        finally:
            os.close(descriptor)

    @contextmanager
    def _identity_transaction(self) -> Iterator[None]:
        """Serialize live native-session claims across start and adoption."""
        self._prepare()
        descriptor = agent._open_private_lock(
            str(self.registry / ".identity.lock"), "agent identity lock"
        )
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            yield
        finally:
            os.close(descriptor)

    def _directory(self, name: str) -> Path:
        return self.registry / _name(name)

    def _load(self, name: str) -> AgentRecord:
        directory = self._directory(name)
        agent._validate_private_directory(str(self.registry), "agent registry")
        if not directory.exists():
            raise AgentDeliveryError(f"unknown agent {name!r}; use list to inspect the registry")
        agent._validate_private_directory(str(directory), "agent directory")
        return AgentRecord.load(directory / "agent.json", name)

    def _load_expected(self, name: str, expected_token: str | None = None) -> AgentRecord:
        record = self._load(name)
        if expected_token is not None and record.token != expected_token:
            raise AgentDeliveryError(f"agent {name!r} was replaced before this operation")
        return record

    @contextmanager
    def _pinned_agent_directory(
        self, name: str, *, path: Path | None = None,
        label: str = "agent directory",
    ) -> Iterator[_PinnedAgentDirectory]:
        """Hold one private agent-directory inode across proof and publication."""
        path = self._directory(name) if path is None else path
        agent._validate_private_directory(str(path), label)
        try:
            before = os.stat(path, follow_symlinks=False)
            if (not stat.S_ISDIR(before.st_mode) or before.st_uid != os.getuid()
                    or stat.S_IMODE(before.st_mode) & 0o077):
                raise AgentDeliveryError("agent directory is not a private directory")
            descriptor = os.open(
                path,
                os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
                | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0),
            )
        except AgentDeliveryError:
            raise
        except OSError as exc:
            raise AgentDeliveryError(f"cannot pin agent directory: {exc}") from exc
        try:
            try:
                opened = os.fstat(descriptor)
            except OSError as exc:
                raise AgentDeliveryError(
                    f"cannot inspect pinned agent directory: {exc}"
                ) from exc
            if (not stat.S_ISDIR(opened.st_mode) or opened.st_uid != os.getuid()
                    or stat.S_IMODE(opened.st_mode) & 0o077
                    or (opened.st_dev, opened.st_ino)
                    != (before.st_dev, before.st_ino)):
                raise AgentDeliveryError(
                    "agent directory changed or is not private while being pinned"
                )
            pinned = _PinnedAgentDirectory(
                name=name,
                path=path,
                descriptor=descriptor,
                device=before.st_dev,
                inode=before.st_ino,
            )
            self._verify_pinned_agent_directory(pinned)
            yield pinned
        finally:
            _close_descriptor(
                descriptor, "pinned agent directory", primary=sys.exc_info()[1],
            )

    @staticmethod
    def _verify_pinned_agent_directory(pinned: _PinnedAgentDirectory) -> None:
        """Require the registry pathname to name the held directory generation."""
        try:
            path = os.stat(pinned.path, follow_symlinks=False)
            opened = os.fstat(pinned.descriptor)
        except OSError as exc:
            raise AgentDeliveryError(
                f"agent {pinned.name!r} registry directory changed: {exc}"
            ) from exc
        if (not stat.S_ISDIR(path.st_mode) or path.st_uid != os.getuid()
                or stat.S_IMODE(path.st_mode) & 0o077
                or not stat.S_ISDIR(opened.st_mode) or opened.st_uid != os.getuid()
                or stat.S_IMODE(opened.st_mode) & 0o077
                or (path.st_dev, path.st_ino) != (pinned.device, pinned.inode)
                or (opened.st_dev, opened.st_ino) != (pinned.device, pinned.inode)):
            raise AgentDeliveryError(
                f"agent {pinned.name!r} registry directory changed"
            )

    @staticmethod
    @contextmanager
    def _pinned_parent_directory(
        path: Path, *, label: str,
    ) -> Iterator[_PinnedParentDirectory]:
        agent._validate_private_directory(str(path), label)
        try:
            before = os.stat(path, follow_symlinks=False)
            if (not stat.S_ISDIR(before.st_mode) or before.st_uid != os.getuid()
                    or stat.S_IMODE(before.st_mode) & 0o077):
                raise AgentDeliveryError(f"{label} is not a private directory")
            descriptor = os.open(
                path,
                os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
                | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0),
            )
        except AgentDeliveryError:
            raise
        except OSError as exc:
            raise AgentDeliveryError(f"cannot pin {label}: {exc}") from exc
        try:
            try:
                opened = os.fstat(descriptor)
            except OSError as exc:
                raise AgentDeliveryError(f"cannot inspect pinned {label}: {exc}") from exc
            if (not stat.S_ISDIR(opened.st_mode) or opened.st_uid != os.getuid()
                    or stat.S_IMODE(opened.st_mode) & 0o077
                    or (opened.st_dev, opened.st_ino)
                    != (before.st_dev, before.st_ino)):
                raise AgentDeliveryError(
                    f"{label} changed or is not private while being pinned"
                )
            pinned = _PinnedParentDirectory(
                path=path,
                descriptor=descriptor,
                device=before.st_dev,
                inode=before.st_ino,
            )
            ManagedAgents._verify_pinned_parent_directory(pinned, label=label)
            yield pinned
        finally:
            _close_descriptor(
                descriptor, f"pinned {label}", primary=sys.exc_info()[1],
            )

    @staticmethod
    def _verify_pinned_parent_directory(
        pinned: _PinnedParentDirectory, *, label: str,
    ) -> None:
        try:
            path = os.stat(pinned.path, follow_symlinks=False)
            opened = os.fstat(pinned.descriptor)
        except OSError as exc:
            raise AgentDeliveryError(f"{label} directory changed: {exc}") from exc
        if (not stat.S_ISDIR(path.st_mode) or path.st_uid != os.getuid()
                or stat.S_IMODE(path.st_mode) & 0o077
                or not stat.S_ISDIR(opened.st_mode) or opened.st_uid != os.getuid()
                or stat.S_IMODE(opened.st_mode) & 0o077
                or (path.st_dev, path.st_ino) != (pinned.device, pinned.inode)
                or (opened.st_dev, opened.st_ino) != (pinned.device, pinned.inode)):
            raise AgentDeliveryError(f"{label} directory changed")

    def _pinned_artifact_bytes(
        self, pinned: _PinnedAgentDirectory, *, name: str, limit: int,
        purpose: str, require_named_directory: bool = True,
    ) -> bytes:
        """Read one bounded private file relative to a pinned directory."""
        if "/" in name or name in ("", ".", ".."):
            raise AgentDeliveryError("invalid pinned artifact name")
        path = pinned.path / name
        if require_named_directory:
            self._verify_pinned_agent_directory(pinned)
        flags = (
            os.O_RDONLY
            | getattr(os, "O_CLOEXEC", 0)
            | getattr(os, "O_NOFOLLOW", 0)
            | getattr(os, "O_NONBLOCK", 0)
        )
        descriptor = -1
        try:
            descriptor = os.open(name, flags, dir_fd=pinned.descriptor)
            before = os.fstat(descriptor)
            if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid()
                    or stat.S_IMODE(before.st_mode) & 0o077 or before.st_nlink != 1
                    or before.st_size > limit):
                raise AgentDeliveryError(f"unsafe {purpose}: {path}")
            content = bytearray()
            while True:
                remaining = limit + 1 - len(content)
                if remaining <= 0:
                    raise AgentDeliveryError(
                        f"{purpose} exceeds {limit} bytes: {path}"
                    )
                block = os.read(descriptor, min(64 << 10, remaining))
                if not block:
                    break
                content.extend(block)
            after = os.fstat(descriptor)
            if (before.st_size != len(content)
                    or (after.st_dev, after.st_ino, after.st_mode, after.st_uid,
                        after.st_nlink, after.st_size, after.st_mtime_ns,
                        after.st_ctime_ns) != (
                            before.st_dev, before.st_ino, before.st_mode, before.st_uid,
                            before.st_nlink, before.st_size, before.st_mtime_ns,
                            before.st_ctime_ns,
                        )):
                raise AgentDeliveryError(f"{purpose} changed while reading: {path}")
            if require_named_directory:
                self._verify_pinned_agent_directory(pinned)
            return bytes(content)
        except AgentDeliveryError:
            raise
        except OSError as exc:
            raise AgentDeliveryError(f"cannot read {purpose} {path}: {exc}") from exc
        finally:
            if descriptor >= 0:
                _close_descriptor(
                    descriptor, purpose, primary=sys.exc_info()[1],
                )

    def _record_bytes(
        self, pinned: _PinnedAgentDirectory, *, require_active_name: bool = True,
    ) -> bytes:
        """Read one bounded record relative to the held directory generation."""
        return self._pinned_artifact_bytes(
            pinned, name="agent.json", limit=_MAX_AGENT_RECORD_BYTES,
            purpose="agent record", require_named_directory=require_active_name,
        )

    def _legacy_record_snapshot(
        self, pinned: _PinnedAgentDirectory, *, expected_token: str,
        expected_digest: str,
    ) -> _LegacyRecordSnapshot:
        """Bind exact recovery bytes, meaning, digest, and directory generation."""
        name = pinned.name
        path = self._directory(name) / "agent.json"
        if _SHA256.fullmatch(expected_digest) is None:
            raise AgentDeliveryError(
                "expected-record-sha256 must be exactly 64 lowercase hexadecimal characters"
            )
        content = self._record_bytes(pinned)
        digest = hashlib.sha256(content).hexdigest()
        document = agent._decode_json_bytes(
            content, "legacy agent record", path,
        )
        if not isinstance(document, dict):
            raise AgentDeliveryError(f"invalid legacy agent record: {path}")
        if document.get("schema") != 1 or "foreign_shell_identity" in document:
            raise AgentDeliveryError(
                "--recover-legacy-adoption requires foreign_shell_identity to be absent, "
                "not null or populated, in a v1 record"
            )
        record = AgentRecord._from_value(document, path, name)
        if record.token != expected_token or digest != expected_digest:
            raise AgentDeliveryError(
                f"agent {name!r} record changed before adoption recovery"
            )
        return _LegacyRecordSnapshot(
            record=record,
            content=content,
            digest=digest,
            directory_device=pinned.device,
            directory_inode=pinned.inode,
        )

    def _managed_record_snapshot(
        self, pinned: _PinnedAgentDirectory, *, expected_token: str,
    ) -> _ManagedRecordSnapshot:
        """Bind a managed record read to one containing directory generation."""
        content = self._record_bytes(pinned)
        path = pinned.path / "agent.json"
        document = agent._decode_json_bytes(content, "agent record", path)
        record = AgentRecord._from_value(document, path, pinned.name)
        if record.token != expected_token:
            raise AgentDeliveryError(
                f"agent {pinned.name!r} was replaced before this operation"
            )
        return _ManagedRecordSnapshot(
            record=record,
            content=content,
            directory_device=pinned.device,
            directory_inode=pinned.inode,
        )

    @contextmanager
    def _pane_lock(self, pane_id: str) -> Iterator[None]:
        """Serialize cooperative control across registries for one exact pane."""
        descriptor = agent._open_private_lock(
            agent._target_lock_path(pane_id), "host-wide target lock"
        )
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            yield
        finally:
            os.close(descriptor)

    def _save(self, record: AgentRecord) -> None:
        document = record.to_storage_document()
        # Validate the encoded size before legacy queue migration so an
        # unwritable session cannot partially advance its compatibility state.
        agent._serialized_json(document, _MAX_AGENT_RECORD_BYTES, allow_nan=False)
        self._migrate_legacy_goal_messages(record)
        if record.goal_message_id is not None:
            self._goal_artifact_state(record, allow_prepared=True)
        agent._atomic_json(
            str(self._directory(record.name) / "agent.json"),
            document,
            max_artifact_bytes=_MAX_AGENT_RECORD_BYTES,
        )

    def _migrate_legacy_goal_messages(self, record: AgentRecord) -> None:
        """Tag exact v1/v2 goal artifacts before retiring decode-only state."""
        legacy_goals = dict(record.goal_messages)
        if record._legacy_goal_pointer and record.goal_message_id is not None:
            if record.goal is None:
                raise AgentDeliveryError(
                    "legacy goal pointer has no session objective"
                )
            mapped = legacy_goals.get(record.goal_message_id)
            if mapped is not None and mapped != record.goal:
                raise AgentDeliveryError(
                    "legacy goal pointer disagrees with its duplicate objective map"
                )
            legacy_goals[record.goal_message_id] = record.goal
        if not legacy_goals:
            return
        queue = self._queue(record.name)
        if not os.path.lexists(queue):
            raise AgentDeliveryError(
                "legacy goal migration requires its durable queue"
            )
        agent._validate_existing_queue(queue)
        lock = agent._open_private_lock(
            str(Path(queue) / ".delivery.lock"), "queue delivery lock",
        )
        try:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as exc:
                raise AgentDeliveryError(
                    "legacy goal migration is busy with queue delivery; retry"
                ) from exc
            for identifier, objective in sorted(legacy_goals.items()):
                state = agent.message_state(queue, identifier)
                if state is None:
                    raise AgentDeliveryError(
                        f"legacy goal artifact {identifier!r} is missing; "
                        "refusing to discard its confirmation authority"
                    )
                folder = {
                    "pending": "inbox", "inflight": "inflight",
                    "processed": "processed", "failed": "failed",
                }[state]
                path = Path(queue) / folder / f"{identifier}.json"
                document = agent._read_queue_json(
                    str(path), "legacy goal message", require_private=True,
                )
                if (not isinstance(document, dict)
                        or document.get("id") != identifier
                        or document.get("text") != _goal_prompt(
                            record.launch.harness, objective,
                        )
                        or document.get("kind") not in (None, "goal")):
                    raise AgentDeliveryError(
                        f"legacy goal artifact {identifier!r} disagrees with its session record"
                    )
                if document.get("kind") is None:
                    migrated = dict(document)
                    migrated["kind"] = "goal"
                    agent._atomic_json(str(path), migrated)
            record._legacy_goal_messages.clear()
            record._legacy_goal_pointer = False
        finally:
            os.close(lock)

    def _goal_transaction_path(self, record: AgentRecord) -> Path:
        return self._directory(record.name) / _GOAL_TRANSACTION_FILE

    def _read_goal_transaction(
        self, record: AgentRecord,
    ) -> tuple[str, str] | None:
        path = self._goal_transaction_path(record)
        try:
            path.lstat()
        except FileNotFoundError:
            return None
        except OSError as exc:
            raise AgentDeliveryError(f"cannot inspect goal transaction: {exc}") from exc
        document = agent._read_queue_json(
            str(path), "goal transaction", require_private=True,
            max_artifact_bytes=_MAX_AGENT_RECORD_BYTES,
        )
        if (not isinstance(document, dict)
                or set(document) != {
                    "schema", "token", "message_id", "objective",
                }
                or document.get("schema") != _GOAL_TRANSACTION_SCHEMA
                or document.get("token") != record.token
                or not isinstance(document.get("message_id"), str)
                or agent._MESSAGE_ID.fullmatch(
                    cast(str, document["message_id"])
                ) is None
                or not isinstance(document.get("objective"), str)
                or not cast(str, document["objective"]).strip()
                or "\n" in cast(str, document["objective"])
                or "\r" in cast(str, document["objective"])):
            raise AgentDeliveryError("goal transaction is invalid or belongs to another generation")
        return cast(str, document["message_id"]), cast(str, document["objective"])

    def _write_goal_transaction(
        self, record: AgentRecord, identifier: str, objective: str,
    ) -> None:
        path = self._goal_transaction_path(record)
        if os.path.lexists(path):
            raise AgentDeliveryError("unfinished goal transaction must be reconciled first")
        try:
            agent._atomic_json_create(
                str(path), {
                    "schema": _GOAL_TRANSACTION_SCHEMA,
                    "token": record.token,
                    "message_id": identifier,
                    "objective": objective,
                }, max_artifact_bytes=_MAX_AGENT_RECORD_BYTES,
            )
        except FileExistsError as exc:
            raise AgentDeliveryError(
                "unfinished goal transaction appeared concurrently"
            ) from exc

    def _remove_goal_transaction(self, record: AgentRecord) -> None:
        path = self._goal_transaction_path(record)
        try:
            path.unlink()
        except FileNotFoundError:
            raise AgentDeliveryError("goal transaction disappeared before commit")
        except OSError as exc:
            raise AgentDeliveryError(f"cannot retire goal transaction: {exc}") from exc
        agent._fsync_dir(str(path.parent))

    def _goal_artifact_state(
        self, record: AgentRecord, *, allow_prepared: bool,
    ) -> str:
        identifier = record.goal_message_id
        objective = record.goal
        if identifier is None or objective is None:
            raise AgentDeliveryError("goal message pointer has no session objective")
        queue = self._queue(record.name)
        state = agent.message_state(queue, identifier)
        if state is None:
            transaction = self._read_goal_transaction(record)
            if (allow_prepared
                    and transaction == (identifier, objective)):
                return "prepared"
            raise AgentDeliveryError(
                f"goal message {identifier!r} has no durable queue artifact"
            )
        folder = {
            "pending": "inbox", "inflight": "inflight",
            "processed": "processed", "failed": "failed",
        }[state]
        path = Path(queue) / folder / f"{identifier}.json"
        document = agent._read_queue_json(
            str(path), "goal message", require_private=True,
            max_artifact_bytes=_MAX_QUEUE_ARTIFACT_BYTES,
        )
        legacy_match = record._legacy_goal_pointer
        if (not isinstance(document, dict)
                or document.get("id") != identifier
                or document.get("text") != _goal_prompt(
                    record.launch.harness, objective,
                )
                or (document.get("kind") != "goal"
                    and not (document.get("kind") is None and legacy_match))):
            raise AgentDeliveryError(
                f"goal message {identifier!r} disagrees with its session record"
            )
        return state

    def _reconcile_goal_transaction(self, record: AgentRecord) -> None:
        transaction = self._read_goal_transaction(record)
        if transaction is None:
            return
        identifier, objective = transaction
        pointer_matches = record.goal_message_id == identifier
        state = agent.message_state(self._queue(record.name), identifier)
        if not pointer_matches:
            if state is not None:
                raise AgentDeliveryError(
                    "uncommitted goal transaction already has a queue artifact"
                )
            self._remove_goal_transaction(record)
            return
        if record.goal != objective:
            raise AgentDeliveryError(
                "goal transaction disagrees with the session objective"
            )
        if state is None:
            agent.enqueue_goal(
                self._queue(record.name),
                _goal_prompt(record.launch.harness, objective),
                message_id=identifier,
                max_artifact_bytes=_MAX_QUEUE_ARTIFACT_BYTES,
            )
        self._goal_artifact_state(record, allow_prepared=False)
        self._remove_goal_transaction(record)

    def _queue(self, name: str) -> str:
        return str(self._directory(name) / "queue")

    def _refuse_inflight_delivery(self, record: AgentRecord) -> None:
        """Keep teardown from overtaking an injection already at its barrier."""
        root = self._queue(record.name)
        if not os.path.lexists(root):
            return
        agent._validate_existing_queue(root)
        agent._validate_existing_binding(root, record.target())
        inflight = Path(root) / "inflight"
        if not inflight.is_dir():
            raise AgentDeliveryError("delivery queue is incomplete")
        identifiers = sorted(
            path.name[:-5] for path in inflight.iterdir()
            if path.name.endswith(".json")
        )
        if identifiers:
            raise AgentDeliveryError(
                "refusing stop while prompt submission is in flight: "
                + ", ".join(identifiers)
            )

    def get(self, name: str) -> AgentRecord:
        """Read durable metadata without requiring Herdr to be reachable."""
        return self._load(name)

    def _identity_owner(
        self, session_agent: str, session_value: str, *, exclude: str | None = None,
    ) -> AgentRecord | None:
        """Return an active record holding this exact provider-local session."""
        if not self.registry.exists():
            return None
        agent._validate_private_directory(str(self.registry), "agent registry")
        for path in self.registry.iterdir():
            if (not _NAME.fullmatch(path.name) or path.name == "archive"
                    or path.name == exclude):
                continue
            other = self._load(path.name)
            if ((other.session_agent or other.launch.harness) == session_agent
                    and session_value == other.session_value):
                return other
        return None

    def start(
        self, name: str, *, cwd: str, workspace_id: str | None = None,
        workspace_label: str | None = None,
        harness: str = "codex", model: str | None = None, resume: str | None = None,
        reasoning_effort: str | None = None,
        harness_args: Sequence[str] = (), environment: Sequence[str] = (),
        brief: str | None = None,
        startup_timeout: float = 30.0, ready_timeout: float = 900.0,
        working_timeout: float = 30.0, max_attempts: int = 3,
        launch_profile: str | None = None,
    ) -> dict[str, object]:
        """Create one new tab and start its interactive harness without stealing focus.

        Failed launches retain their record and terminal for diagnosis. Stop the
        named agent after inspecting it to archive its artifacts and release its name.
        """
        _name(name)
        root = str(Path(cwd).expanduser().resolve())
        if not Path(root).is_dir():
            raise AgentDeliveryError(f"cwd is not a directory: {root}")
        if workspace_id is not None and workspace_label is not None:
            raise AgentDeliveryError("workspace id and label are mutually exclusive")
        for value, field in (
            (workspace_id, "workspace id"), (workspace_label, "workspace label"),
        ):
            if value is not None and (not value or "\0" in value):
                raise AgentDeliveryError(f"{field} must be nonempty and contain no NUL")
        if not math.isfinite(startup_timeout) or not 0 < startup_timeout <= 300:
            raise AgentDeliveryError("startup timeout must be between 0 and 300 seconds")
        if isinstance(harness_args, (str, bytes)) or any(
            not isinstance(item, str) or not item or "\0" in item
            for item in harness_args
        ):
            raise AgentDeliveryError(
                "harness arguments must be a sequence of nonempty NUL-free strings"
            )
        if harness in ("codex", "claude", "muse"):
            validate_structured_harness_argument_conflicts(
                harness, list(harness_args), label=f"{harness} launch",
                structured_model=model is not None,
                structured_effort=reasoning_effort is not None,
                structured_resume=resume is not None,
            )
        structured_effort = reasoning_arguments(harness, reasoning_effort)
        arguments = harness_arguments(
            harness, model=model, resume=resume,
            extra=(*structured_effort, *harness_args),
        )
        environment = environment_entries(environment)
        if brief is not None and not brief:
            raise AgentDeliveryError("brief must not be empty")
        with self._lock(name):
            with self._identity_transaction():
                if resume is not None:
                    owner = self._identity_owner(harness, resume, exclude=name)
                    if owner is not None:
                        raise AgentDeliveryError(
                            f"native session is already registered as {owner.name!r}"
                        )
                directory = self._directory(name)
                if os.path.lexists(directory):
                    raise AgentDeliveryError(f"agent {name!r} already registered; stop it before reusing the name")
                directory.mkdir(mode=0o700)
                agent._fsync_dir(str(self.registry))
                record = AgentRecord(
                    name, uuid.uuid4().hex,
                    LaunchSpec(
                        harness=harness, cwd=root,
                        adapter="herdr-pane" if harness == "muse" else "herdr",
                        mode="interactive", backend="herdr", model=model,
                        resume=resume, profile=launch_profile,
                        argv=(harness, *arguments),
                        environment_names=tuple(
                            entry.partition("=")[0] for entry in environment
                        ),
                        runtime_home=None, runtime_ownership="owned",
                        executable=None,
                    ),
                    time.time(),
                )
                self._save(record)
                try:
                    self._create_presentation(
                        record, workspace_id, workspace_label, environment,
                    )
                    assert record.pane_id is not None
                    if record.launch.adapter == "herdr-pane":
                        def persist_launch_intent(
                            executable: str, device: int, inode: int,
                            argv: tuple[str, ...],
                        ) -> None:
                            record.launch = replace(
                                record.launch,
                                executable=(executable, device, inode),
                                argv=argv,
                            )
                            self._save(record)

                        def persist_identity(identity: CustomProcessIdentity) -> None:
                            record.custom_process_identity = identity
                            self._save(record)

                        self.client.start_pane_agent(
                            name, harness, record.pane_id, arguments,
                            timeout=startup_timeout,
                            on_launch_intent=persist_launch_intent,
                            on_observed=persist_identity,
                        )
                        record.pane_reported_by_agentctl = True
                        self._save(record)
                        screen = self.client.read(
                            record.pane_id, source="visible", lines=200
                        )
                        (record.startup_warning,
                         record.effective_reasoning_effort) = muse_startup_metadata(screen)
                    else:
                        self.client.start_agent(name, harness, record.pane_id, arguments, timeout=startup_timeout)
                    info = self._checked(record, ready=True)
                    if info.workspace_id != record.workspace_id:
                        raise AgentDeliveryError("started agent moved to another workspace")
                    record.session_agent = info.session_agent
                    record.session_value = info.session_value
                    record.session_source = (
                        "observed" if info.session_value is not None else None
                    )
                    if info.session_agent is not None and info.session_value is not None:
                        owner = self._identity_owner(
                            info.session_agent, info.session_value, exclude=name
                        )
                        if owner is not None:
                            try:
                                self.client.close_pane(record.pane_id)
                            except HerdrRunError as close_error:
                                record.session_agent = record.session_value = None
                                record.session_source = None
                                raise AgentDeliveryError(
                                    f"native session is already registered as {owner.name!r}; "
                                    f"could not close the conflicting new pane: {close_error}"
                                ) from close_error
                            record.session_agent = record.session_value = None
                            record.session_source = None
                            raise AgentDeliveryError(
                                f"native session is already registered as {owner.name!r}; "
                                "closed the conflicting new pane"
                            )
                        try:
                            agent.resolve_target(self.client, record.target())
                        except HerdrRunError as identity_error:
                            record.session_agent = record.session_value = None
                            record.session_source = None
                            raise AgentDeliveryError(
                                "started native session is not globally unique; "
                                "the failed owned pane remains available for stop"
                            ) from identity_error
                    record.lifecycle = "running"
                    self._save(record)
                    try:
                        agent.resolve_target(self.client, record.target())
                        final_info = self._checked(record, ready=True)
                    except HerdrRunError:
                        record.session_agent = record.session_value = None
                        record.session_source = None
                        raise
                    if (final_info.session_agent != record.session_agent
                            or final_info.session_value != record.session_value):
                        record.session_agent = record.session_value = None
                        record.session_source = None
                        raise AgentDeliveryError(
                            "started agent native session changed during identity commit"
                        )
                except (HerdrRunError, ValueError, OSError) as exc:
                    record.lifecycle = "launch_failed"
                    record.error = (
                        "launch failed with caller-supplied environment; "
                        "details omitted from status"
                        if environment else str(exc)
                    )
                    self._save(record)
                    raise AgentDeliveryError(
                        f"launch of {name!r} failed: {exc}; record and any created "
                        f"tab retained at {directory}"
                    ) from exc
            if brief is not None:
                client = cast(HerdrClient, _WorkspaceClient(self.client, record, queue=self._queue(name)))
                agent.send(client, record.target(), self._queue(name), brief,
                           ready_timeout=ready_timeout, working_timeout=working_timeout,
                           max_attempts=max_attempts)
            # Keep the generation lock until its initial instruction and result
            # are captured; a replacement must never receive this launch's brief.
            return self._status_record(record)

    def adopt(
        self, name: str, *, pane_id: str, expected_workspace: str,
        expected_cwd: str, harness: str, session: str | None = None,
    ) -> dict[str, object]:
        """Register an existing interactive Herdr agent without owning its runtime.

        Adoption is intentionally narrower than startup: every live identity
        assertion is explicit, no pane/name/process is changed, and the saved
        adapter marks the runtime as foreign so retirement cannot close it.
        """
        _name(name)
        if not pane_id or "\0" in pane_id:
            raise AgentDeliveryError("adopt needs a nonempty pane id without NUL")
        if not expected_workspace or "\0" in expected_workspace:
            raise AgentDeliveryError("adopt needs a nonempty expected workspace label without NUL")
        if not _KIND.fullmatch(harness):
            raise AgentDeliveryError("harness must be a Herdr agent kind")
        if harness == "muse":
            raise AgentDeliveryError(
                "adopting Muse is unsupported because agentctl cannot yet pin the existing "
                "foreground process identity; start an owned Muse session instead"
            )
        root = str(Path(expected_cwd).expanduser().resolve())
        if not Path(root).is_dir():
            raise AgentDeliveryError(f"cwd is not a directory: {root}")
        if session is not None and (not session or "\0" in session):
            raise AgentDeliveryError("native session id must be nonempty and contain no NUL")
        target = agent.Target(
            pane_id=pane_id,
            session_agent=harness if session is not None else None,
            session_value=session,
            expected_agent=harness,
            expected_workspace=expected_workspace,
            expected_cwd=root,
        )
        with self._lock(name):
            directory = self._directory(name)
            if os.path.lexists(directory):
                raise AgentDeliveryError(
                    f"agent {name!r} already registered; stop it before reusing the name"
                )
            # Names have independent lifecycle locks.  Serialize adoption as a
            # registry-wide identity transaction as well, so two callers cannot
            # register different panes that report the same native session.
            with self._identity_transaction():
                target_lock, _locked_pane, info = agent._lock_resolved_target(self.client, target)
                try:
                    return self._adopt_locked(
                        name, directory, root, harness, pane_id, info
                    )
                finally:
                    os.close(target_lock)

    def recover_start(
        self, name: str, *, expected_token: str, expected_pid: int,
    ) -> dict[str, object]:
        """Recover or reconcile one exactly identified live Muse process."""
        with self._lock(name):
            record = self._load_expected(name, expected_token)
            if (record.lifecycle not in {"starting", "launch_failed"}
                    or record.launch.adapter != "herdr-pane"
                    or record.launch.harness != "muse" or record.launch.mode != "interactive"
                    or record.launch.backend != "herdr" or record.launch.runtime_ownership != "owned"
                    or record.pane_id is None
                    or record.tab_id is None or record.workspace_id is None
                    or not record.launch.argv
                    or record.session_agent is not None or record.session_value is not None):
                raise AgentDeliveryError(
                    "start recovery requires a starting or launch_failed Muse interactive Herdr "
                    "record with complete launch intent and pane ownership"
                )
            launch_device = (
                record.launch.executable[1]
                if record.launch.executable is not None else None
            )
            launch_inode = (
                record.launch.executable[2]
                if record.launch.executable is not None else None
            )
            if record.custom_process_identity is not None:
                recorded_image = (
                    record.custom_process_identity.executable_device,
                    record.custom_process_identity.executable_inode,
                )
                if (launch_device is not None and launch_inode is not None
                        and (launch_device, launch_inode) != recorded_image):
                    raise AgentDeliveryError(
                        "refusing start recovery: launch and process executable identities disagree"
                    )
                launch_device, launch_inode = recorded_image
            if launch_device is None or launch_inode is None:
                raise AgentDeliveryError(
                    "start recovery requires a saved launch or process executable identity"
                )
            pane_id = record.pane_id
            assert pane_id is not None
            presentations = [
                pane for pane in self.client.panes()
                if pane.pane_id == pane_id
            ]
            if (len(presentations) != 1
                    or presentations[0].tab_id != record.tab_id
                    or presentations[0].workspace_id != record.workspace_id):
                raise AgentDeliveryError(
                    "refusing start recovery: recorded pane, tab, or workspace ownership changed"
                )
            info = self.client.pane_info(pane_id)
            if (info.pane_id != pane_id
                    or info.workspace_id != record.workspace_id
                    or os.path.realpath(info.cwd) != os.path.realpath(record.launch.cwd)
                    or info.agent not in (None, record.launch.harness)):
                raise AgentDeliveryError(
                    "refusing start recovery: live pane identity or agent report changed"
                )
            identity = self.client.recover_pane_agent(
                pane_id, record.launch.argv,
                launch_device, launch_inode,
                expected_pid,
            )
            if (record.custom_process_identity is not None
                    and record.custom_process_identity != identity):
                raise AgentDeliveryError(
                    "refusing start recovery: persisted custom process identity changed"
                )
            record.custom_process_identity = identity
            self._save(record)
            try:
                def commit_running() -> None:
                    screen = self.client.read(
                        pane_id, source="visible", lines=200,
                    )
                    if not muse_idle_composer(screen):
                        raise HerdrUnavailable(
                            "start recovery found the exact Muse process but no verified idle composer"
                        )
                    # The exact process identity, not Herdr's advisory native
                    # label, is the durable authority for custom harnesses.
                    record.lifecycle = "running"
                    record.error = None
                    self._save(record)

                self.client.commit_recovered_pane_agent(
                    pane_id, record.launch.harness, identity, commit_running,
                )
            except HerdrRunError as exc:
                record.lifecycle = "launch_failed"
                record.error = f"start recovery failed after identity persistence: {exc}"
                self._save(record)
                raise AgentDeliveryError(record.error) from exc
            return self._status_record(record)

    def _adopt_locked(
        self, name: str, directory: Path, root: str, harness: str,
        pane_id: str, info: AgentPaneInfo,
    ) -> dict[str, object]:
        """Commit one adoption while its registry and live pane are locked."""
        if info.agent is None:
            raise AgentDeliveryError(f"refusing pane {pane_id}: no live agent is detected")
        if (info.session_agent is None) != (info.session_value is None):
            raise AgentDeliveryError(
                f"refusing pane {pane_id}: native session identity is incomplete"
            )
        if (info.session_agent is not None
                and (not info.session_agent or not info.session_value
                     or "\0" in info.session_agent or "\0" in info.session_value)):
            raise AgentDeliveryError(
                f"refusing pane {pane_id}: native session identity is invalid"
            )
        if info.session_agent is not None and info.session_agent != harness:
            raise AgentDeliveryError(
                f"refusing pane {pane_id}: native session agent is "
                f"{info.session_agent!r}, expected {harness!r}"
            )
        if info.session_value is not None:
            # A reported native session becomes the durable queue authority.
            # Prove now that it resolves uniquely back to this exact pane.
            agent.resolve_target(self.client, agent.Target(
                pane_id=info.pane_id, session_agent=info.session_agent,
                session_value=info.session_value, expected_agent=harness,
                expected_cwd=root,
            ))
        presentations = [pane for pane in self.client.panes()
                         if pane.pane_id == info.pane_id]
        if len(presentations) != 1:
            raise AgentDeliveryError(
                f"refusing pane {pane_id}: expected one live presentation, "
                f"found {len(presentations)}"
            )
        presentation = presentations[0]
        if presentation.workspace_id != info.workspace_id:
            raise AgentDeliveryError(
                f"refusing pane {pane_id}: presentation workspace identity changed"
            )
        shell_identity = self.client.pane_shell_identity(info.pane_id)
        if self.registry.exists():
            agent._validate_private_directory(str(self.registry), "agent registry")
            for path in self.registry.iterdir():
                if not _NAME.fullmatch(path.name) or path.name == "archive":
                    continue
                other = self._load(path.name)
                same_session = (info.session_value is not None
                                and (other.session_agent or other.launch.harness)
                                    == info.session_agent
                                and info.session_value == other.session_value)
                if other.pane_id == info.pane_id or same_session:
                    raise AgentDeliveryError(
                        f"pane {pane_id!r} is already registered as {other.name!r}"
                    )
        confirmed = agent.resolve_target(self.client, agent.Target(
            pane_id=info.pane_id, session_agent=info.session_agent,
            session_value=info.session_value, expected_agent=harness,
            expected_cwd=root,
        ))
        if (confirmed.workspace_id != info.workspace_id
                or confirmed.session_agent != info.session_agent
                or confirmed.session_value != info.session_value):
            raise AgentDeliveryError(
                f"refusing pane {pane_id}: live identity changed before adoption"
            )
        final_presentations = [pane for pane in self.client.panes()
                               if pane.pane_id == confirmed.pane_id]
        if (len(final_presentations) != 1
                or final_presentations[0].tab_id != presentation.tab_id
                or final_presentations[0].workspace_id != presentation.workspace_id):
            raise AgentDeliveryError(
                f"refusing pane {pane_id}: live presentation changed before adoption"
            )
        if self.client.pane_shell_identity(confirmed.pane_id) != shell_identity:
            raise AgentDeliveryError(
                f"refusing pane {pane_id}: pane shell process changed before adoption"
            )
        directory.mkdir(mode=0o700)
        agent._fsync_dir(str(self.registry))
        record = AgentRecord(
            name, uuid.uuid4().hex,
            LaunchSpec(
                harness=harness, cwd=root, adapter="herdr-foreign",
                mode="interactive", backend="herdr", model=None, resume=None,
                profile=None, argv=(), environment_names=(), runtime_home=None,
                runtime_ownership="foreign", executable=None,
            ),
            time.time(),
            lifecycle="running", workspace_id=info.workspace_id,
            tab_id=presentation.tab_id, pane_id=info.pane_id,
            session_agent=info.session_agent,
            session_value=info.session_value,
            session_source="observed" if info.session_value is not None else None,
            foreign_shell_identity=shell_identity,
        )
        self._save(record)
        try:
            result = self._status_record(record)
            if result.get("probe_error") is not None:
                raise AgentDeliveryError(
                    f"final live-identity verification failed: {result['probe_error']}"
                )
            final_info = self._checked(record)
            if (final_info.session_agent != record.session_agent
                    or final_info.session_value != record.session_value):
                raise AgentDeliveryError(
                    "final live native-session identity changed"
                )
            live = [pane for pane in self.client.panes()
                    if pane.pane_id == record.pane_id]
            if (len(live) != 1 or live[0].tab_id != record.tab_id
                    or live[0].workspace_id != record.workspace_id):
                raise AgentDeliveryError("final live-presentation verification failed")
            if self.client.pane_shell_identity(confirmed.pane_id) != shell_identity:
                raise AgentDeliveryError("final pane shell process identity changed")
            return result
        except (HerdrRunError, OSError) as exc:
            try:
                failed = self._archive_failed_adoption(record, str(exc))
            except (HerdrRunError, OSError) as cleanup:
                raise AgentDeliveryError(
                    f"adoption failed final verification ({exc}); could not finish "
                    f"failed-record archival from {directory}: {cleanup}"
                ) from exc
            raise AgentDeliveryError(
                f"adoption failed final verification and was not registered; "
                f"diagnostic record archived at {failed}: {exc}"
            ) from exc

    def _archive_failed_adoption(self, record: AgentRecord, error: str) -> Path:
        """Atomically remove a failed generation from the active namespace."""
        archive = self.registry / "archive"
        archive.mkdir(mode=0o700, exist_ok=True)
        agent._validate_private_directory(str(archive), "agent archive")
        destination = archive / f"{record.name}-{record.token}-adopt-failed"
        os.rename(self._directory(record.name), destination)
        agent._fsync_dir(str(archive))
        agent._fsync_dir(str(self.registry))
        record.lifecycle = "adopt_failed"
        record.error = error
        agent._atomic_json(str(destination / "agent.json"), record.to_storage_document())
        return destination

    def _create_presentation(
        self, record: AgentRecord, workspace_id: str | None,
        workspace_label: str | None,
        environment: Sequence[str],
    ) -> None:
        # Independent registries can share the default workspace. Serialize label
        # resolution and creation host-wide, releasing before any harness startup.
        may_create_shared_default = (
            workspace_id is None and workspace_label is None
            and not os.environ.get("HERDR_WORKSPACE_ID")
        )
        selected_id = workspace_id
        selected_label = workspace_label
        if selected_id is None and selected_label is None:
            selected_id = os.environ.get("HERDR_WORKSPACE_ID")
        if selected_label is None and selected_id is None:
            selected_label = "subagents"
        lock_key = selected_id or selected_label
        assert lock_key is not None
        lock = agent._open_private_lock(
            agent._target_lock_path(f"managed-workspace:{lock_key}"),
            "workspace allocation lock",
        )
        try:
            fcntl.flock(lock, fcntl.LOCK_EX)
            selected = selected_id
            if selected is not None:
                self.client.workspace_label(selected)
            else:
                assert selected_label is not None
                selected = self.client.workspace_id_for_label(selected_label)
            if selected is None:
                assert selected_label is not None
                if not may_create_shared_default:
                    raise AgentDeliveryError(
                        f"workspace label {selected_label!r} does not exist"
                    )
                selected, tab, pane = self.client.create_workspace(
                    label=selected_label, cwd=record.launch.cwd, environment=environment
                )
                record.workspace_id, record.tab_id, record.pane_id = selected, tab, pane
                self._save(record)
                self.client.rename_tab(tab, record.name)
            else:
                record.workspace_id = selected
                record.tab_id, record.pane_id = self.client.create_tab_with_pane(
                    workspace_id=selected, label=record.name, cwd=record.launch.cwd,
                    environment=environment,
                )
                self._save(record)
        finally:
            os.close(lock)

    def _checked(self, record: AgentRecord, *, ready: bool = False) -> AgentPaneInfo:
        if record.launch.adapter not in ("herdr", "herdr-pane", "herdr-foreign"):
            raise AgentDeliveryError("this operation requires the interactive Herdr adapter")
        client = cast(HerdrClient, _WorkspaceClient(self.client, record, check_prompt=ready))
        info = agent.resolve_target(client, record.target())
        if info.workspace_id != record.workspace_id:
            raise AgentDeliveryError(f"agent {record.name!r} workspace identity changed")
        return info

    def _checked_or_failed_pane_report(self, record: AgentRecord, pane_id: str) -> None:
        """Prove a failed custom launch is still ours or has returned to its shell."""
        if (record.lifecycle in ("starting", "launch_failed")
                and record.launch.adapter == "herdr-pane"
                and record.custom_process_identity is not None):
            info = self.client.pane_info(pane_id)
            if (info.pane_id == pane_id and info.workspace_id == record.workspace_id
                    and os.path.realpath(info.cwd) == os.path.realpath(record.launch.cwd)):
                try:
                    self.client.verify_custom_harness(
                        pane_id, record.launch.harness, record.custom_process_identity
                    )
                    return
                except HerdrUnavailable:
                    pass
        # Preserve the original failed-launch fallback exactly. A stale report plus
        # return to the original shell is sufficient only after agentctl itself
        # reported that pane; an unreported pre-readiness pane remains unowned.
        if (record.lifecycle == "launch_failed" and record.launch.adapter == "herdr-pane"
                and record.pane_reported_by_agentctl):
            info = self.client.pane_info(pane_id)
            if (info.pane_id == pane_id and info.workspace_id == record.workspace_id
                    and os.path.realpath(info.cwd) == os.path.realpath(record.launch.cwd)
                    and info.agent == record.launch.harness
                    and self.client.pane_is_idle_shell(pane_id)):
                return
        if (record.lifecycle == "starting" and record.launch.adapter == "herdr-pane"
                and record.custom_process_identity is None):
            raise HerdrUnavailable(
                f"cannot prove starting custom harness ownership in pane {pane_id}"
            )
        self._checked(record)

    def status(self, name: str) -> dict[str, object]:
        """Report live state or a visible probe error, preserving every durable record."""
        return self._status_record(self._load(name))

    @staticmethod
    def _remaining_timeout(deadline: float, purpose: str) -> float:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise HerdrUnavailable(f"{purpose} deadline expired")
        return remaining

    def _health_panes(self, deadline: float | None) -> tuple[Pane, ...]:
        if deadline is not None and isinstance(self.client, HerdrClient):
            return self.client.panes(
                timeout=self._remaining_timeout(deadline, "health pane list")
            )
        return self.client.panes()

    def _health_pane_info(
        self, pane_id: str, deadline: float | None,
    ) -> AgentPaneInfo:
        if deadline is not None and isinstance(self.client, HerdrClient):
            return self.client.pane_info(
                pane_id,
                timeout=self._remaining_timeout(deadline, "health pane identity"),
            )
        return self.client.pane_info(pane_id)

    def _health_verify_custom(
        self, record: AgentRecord, deadline: float | None,
    ) -> None:
        assert record.pane_id is not None
        if deadline is not None and isinstance(self.client, HerdrClient):
            self.client.verify_custom_harness(
                record.pane_id, record.launch.harness, record.custom_process_identity,
                timeout=self._remaining_timeout(
                    deadline, "health custom harness verification"
                ),
            )
            return
        self.client.verify_custom_harness(
            record.pane_id, record.launch.harness, record.custom_process_identity,
        )

    def _health_idle_shell(
        self, pane_id: str, deadline: float | None,
    ) -> PaneShellProof | None:
        if deadline is not None and isinstance(self.client, HerdrClient):
            return self.client.pane_idle_shell_identity(
                pane_id,
                timeout=self._remaining_timeout(deadline, "health idle-shell proof"),
            )
        return self.client.pane_idle_shell_identity(pane_id)

    def _health_same_idle_shell(
        self, pane_id: str, identity: CustomProcessIdentity,
        deadline: float | None,
    ) -> bool:
        if deadline is not None and isinstance(self.client, HerdrClient):
            return self.client.pane_is_same_idle_shell(
                pane_id, identity,
                timeout=self._remaining_timeout(
                    deadline, "health idle-shell identity proof"
                ),
            )
        return self.client.pane_is_same_idle_shell(pane_id, identity)

    def _status_record(
        self, record: AgentRecord, *, deadline: float | None = None,
    ) -> dict[str, object]:
        """Probe one pinned record without resolving its name a second time."""
        name = record.name
        result: dict[str, object] = record.to_document()
        result["queue"] = self._queue(name)
        result["output"] = str(self._directory(name) / "output.json")
        result["goal_source"] = "requested" if record.goal is not None else None
        result["goal_delivery"] = self._goal_delivery(record)
        client = _WorkspaceClient(
            self.client, record, check_prompt=False, deadline=deadline,
        )
        try:
            result.update(agent.status(
                cast(HerdrClient, client), record.target(), self._queue(name),
            ))
            result["probe_error"] = None
        except HerdrRunError as exc:
            result["agent_status"], result["probe_error"] = "unknown", str(exc)
        if record.launch.adapter == "herdr-foreign":
            evidence = client.adopted_evidence_observation
            if evidence is None:
                evidence = client.adopted_evidence()
            result["runtime_evidence"] = evidence.public_document()
        return result

    def _classify_health(
        self, record: AgentRecord, status: dict[str, object], *,
        deadline: float | None = None,
    ) -> tuple[str, str, str]:
        """Classify one status probe without turning control loss into proof of death."""
        if record.lifecycle != "running":
            return (
                "unhealthy",
                "lifecycle-not-running",
                f"saved lifecycle is {record.lifecycle!r}, expected 'running'",
            )
        if record.launch.adapter == "herdr-foreign":
            raw = status.get("runtime_evidence")
            if not isinstance(raw, dict):
                return (
                    "unknown", "runtime-probe-failed",
                    "adopted-runtime probe produced no typed evidence",
                )
            state = raw.get("state")
            code = raw.get("reason_code")
            detail = raw.get("reason")
            if not all(isinstance(value, str) for value in (state, code, detail)):
                return (
                    "unknown", "runtime-probe-failed",
                    "adopted-runtime probe produced malformed typed evidence",
                )
            if state == AdoptedRuntimeState.LIVE_EXACT.value:
                if status.get("probe_error") is None:
                    return "healthy", "ok", cast(str, detail)
                return "unknown", "runtime-probe-failed", str(status["probe_error"])
            if state == AdoptedRuntimeState.UNKNOWN.value:
                return "unknown", cast(str, code), cast(str, detail)
            return "unhealthy", cast(str, code), cast(str, detail)
        probe_error = status.get("probe_error")
        if probe_error is None:
            return "healthy", "ok", "expected harness and runtime identity are live"
        reason = str(probe_error)
        if record.pane_id is None:
            return "unhealthy", "pane-identity-missing", reason
        try:
            presentations = [
                pane for pane in self._health_panes(deadline)
                if pane.pane_id == record.pane_id
            ]
        except HerdrRunError:
            return "unknown", "runtime-probe-failed", reason
        if not presentations:
            return "unhealthy", "pane-missing", reason
        if len(presentations) != 1:
            return "unhealthy", "pane-identity-ambiguous", reason
        try:
            info = self._health_pane_info(record.pane_id, deadline)
        except HerdrRunError:
            return "unknown", "runtime-probe-failed", reason
        presentation = presentations[0]
        session_agent_matches = (
            record.session_agent is None
            or info.session_agent == record.session_agent
            or (info.agent is None and info.session_agent is None)
        )
        session_value_matches = (
            record.session_value is None
            or info.session_value == record.session_value
            or (info.agent is None and info.session_value is None)
        )
        if (presentation.workspace_id != record.workspace_id
                or presentation.tab_id != record.tab_id
                or info.pane_id != record.pane_id
                or info.workspace_id != record.workspace_id
                or os.path.realpath(info.cwd) != os.path.realpath(record.launch.cwd)
                or not session_agent_matches or not session_value_matches):
            return "unhealthy", "runtime-identity-mismatch", reason
        if record.launch.adapter == "herdr-pane":
            if record.custom_process_identity is not None:
                try:
                    self._health_verify_custom(record, deadline)
                except HerdrRunError:
                    pass
                else:
                    return "unknown", "custom-harness-verification-unconfirmed", reason
            try:
                custom_shell_proof = self._health_idle_shell(record.pane_id, deadline)
            except HerdrRunError:
                return "unknown", "runtime-probe-failed", reason
            if custom_shell_proof is not None:
                return (
                    "unhealthy",
                    "expected-harness-missing",
                    f"pane {record.pane_id!r} has no live exact {record.launch.harness!r} "
                    f"process and is at a stable idle shell (reported agent "
                    f"{info.agent!r}); status probe: {reason}",
                )
            return "unknown", "custom-harness-verification-unconfirmed", reason
        if info.agent != record.launch.harness:
            if info.agent is None:
                try:
                    shell_fallback = (
                        self._health_idle_shell(record.pane_id, deadline) is not None
                    )
                except HerdrRunError:
                    return "unknown", "runtime-probe-failed", reason
                if not shell_fallback:
                    return "unknown", "agent-report-missing", reason
            return (
                "unhealthy",
                "expected-harness-missing",
                f"pane {record.pane_id!r} reports agent {info.agent!r}, "
                f"expected {record.launch.harness!r}; status probe: {reason}",
            )
        return "unknown", "runtime-probe-failed", reason

    def _persist_health(
        self, record: AgentRecord, health: str, reason_code: str,
        reason: str, checked_at: float,
    ) -> dict[str, object]:
        """Durably retain the current condition and the last confirmed failure."""
        path = self._directory(record.name) / "health.json"
        previous: dict[str, object] = {}
        if path.exists():
            try:
                value = agent._read_queue_json(
                    str(path), "agent health record", require_private=True,
                    max_artifact_bytes=64 << 10,
                )
                if isinstance(value, dict) and value.get("schema") == _HEALTH_SCHEMA:
                    previous = cast(dict[str, object], value)
            except HerdrRunError:
                # This file is derived health state, not session authority. Replace a
                # malformed prior observation with the new verified observation.
                previous = {}
        same_condition = (
            previous.get("token") == record.token
            and previous.get("health") == health
            and previous.get("reason_code") == reason_code
            and previous.get("reason") == reason
        )
        first_detected_at = previous.get("first_detected_at") if same_condition else checked_at
        if not isinstance(first_detected_at, (int, float)) or isinstance(first_detected_at, bool):
            first_detected_at = checked_at
        runtime_state = (
            "live" if health == "healthy" else
            "dead" if reason_code in {
                "expected-harness-missing", "pane-missing",
                "runner-not-live",
            } else
            "inactive" if reason_code == "lifecycle-not-running" else
            "unknown"
        )
        document: dict[str, object] = {
            "schema": _HEALTH_SCHEMA,
            "name": record.name,
            "token": record.token,
            "health": health,
            "runtime_state": runtime_state,
            "reason_code": reason_code,
            "reason": reason,
            "first_detected_at": first_detected_at,
            "last_checked_at": checked_at,
        }
        for key in (
            "last_unhealthy_at", "last_unhealthy_reason_code", "last_unhealthy_reason",
            "last_unknown_at", "last_unknown_reason_code", "last_unknown_reason",
        ):
            if key in previous:
                document[key] = previous[key]
        if health == "unhealthy":
            document.update(
                last_unhealthy_at=checked_at,
                last_unhealthy_reason_code=reason_code,
                last_unhealthy_reason=reason,
            )
        elif health == "unknown":
            document.update(
                last_unknown_at=checked_at,
                last_unknown_reason_code=reason_code,
                last_unknown_reason=reason,
            )
        agent._atomic_json(str(path), document, max_artifact_bytes=64 << 10)
        return {**document, "recorded": True, "record_path": str(path)}

    def _health_one_result(
        self, name: str, checked_at: float, deadline: float,
    ) -> tuple[dict[str, object], dict[str, object] | None]:
        """Check and record one name, propagating registry/probe setup errors."""
        if time.monotonic() >= deadline:
            return self._unrecorded_health(
                name, checked_at, "probe-deadline-exceeded",
                "health probe deadline expired before the record was read",
            ), None
        initial = self._load(name)
        if time.monotonic() >= deadline:
            return self._unrecorded_health(
                name, checked_at, "probe-deadline-exceeded",
                "health probe deadline expired before this session was checked",
                record=initial,
            ), None
        with self._try_lock(name, deadline) as acquired:
            if not acquired:
                return self._unrecorded_health(
                    name, checked_at, "lifecycle-lock-contended",
                    f"agent {name!r} lifecycle lock is held by another operation",
                    record=initial,
                ), None
            record = self._load_expected(name, initial.token)
            if time.monotonic() >= deadline:
                return self._unrecorded_health(
                    name, checked_at, "probe-deadline-exceeded",
                    "health probe deadline expired before runtime inspection",
                    record=record,
                ), None
            status = self._status_record(record, deadline=deadline)
            if time.monotonic() >= deadline:
                observation = self._unrecorded_health(
                    name, checked_at, "probe-deadline-exceeded",
                    "health probe deadline expired during runtime inspection",
                    record=record,
                )
                observation["agent_status"] = status.get("agent_status")
                observation["probe_error"] = status.get("probe_error")
                return observation, status
            health, reason_code, reason = self._classify_health(
                record, status, deadline=deadline,
            )
            if time.monotonic() >= deadline:
                observation = self._unrecorded_health(
                    name, checked_at, "probe-deadline-exceeded",
                    "health probe deadline expired during liveness classification",
                    record=record,
                )
                observation["agent_status"] = status.get("agent_status")
                observation["probe_error"] = status.get("probe_error")
                return observation, status
            observation = self._persist_health(
                record, health, reason_code, reason, checked_at,
            )
            observation["lifecycle"] = record.lifecycle
            observation["agent_status"] = status.get("agent_status")
            observation["probe_error"] = status.get("probe_error")
            return observation, status

    @staticmethod
    def _unrecorded_health(
        name: str, checked_at: float, reason_code: str, reason: str, *,
        record: AgentRecord | None = None,
    ) -> dict[str, object]:
        return {
            "schema": _HEALTH_SCHEMA,
            "name": name,
            "token": record.token if record is not None else None,
            "health": "unknown",
            "runtime_state": "unknown",
            "reason_code": reason_code,
            "reason": reason,
            "first_detected_at": checked_at,
            "last_checked_at": checked_at,
            "recorded": False,
            "record_path": None,
            "lifecycle": record.lifecycle if record is not None else None,
            "agent_status": None,
            "probe_error": reason,
        }

    def _health_one(
        self, name: str, checked_at: float, deadline: float,
    ) -> tuple[dict[str, object], dict[str, object] | None]:
        """Check and record one name while isolating it from every other name."""
        try:
            return self._health_one_result(name, checked_at, deadline)
        except (HerdrRunError, OSError, ValueError, TypeError) as exc:
            code = (
                "probe-deadline-exceeded"
                if time.monotonic() >= deadline else "registry-or-health-error"
            )
            return self._unrecorded_health(
                name, checked_at, code, str(exc),
            ), None

    def status_health_snapshot(
        self, name: str, *, checked_at: float | None = None,
        deadline: float | None = None,
    ) -> tuple[dict[str, object], dict[str, object] | None]:
        """Return one status and its health verdict from one bounded probe."""
        observation, status = self._health_one_result(
            name, time.time() if checked_at is None else checked_at,
            time.monotonic() + _HEALTH_PROBE_SECONDS if deadline is None else deadline,
        )
        return observation, status

    def health_snapshot(
        self, names: Sequence[str] = (), *, checked_at: float | None = None,
        deadline: float | None = None,
    ) -> tuple[dict[str, object], list[dict[str, object] | None]]:
        """Return one aggregate and the exact status snapshot behind each verdict."""
        now = time.time() if checked_at is None else checked_at
        probe_deadline = (
            time.monotonic() + _HEALTH_PROBE_SECONDS if deadline is None else deadline
        )
        requested = sorted(dict.fromkeys(names))
        registry_error: str | None = None
        if not requested:
            if self.registry.exists():
                try:
                    agent._validate_private_directory(str(self.registry), "agent registry")
                    requested = sorted(
                        path.name for path in self.registry.iterdir()
                        if _NAME.fullmatch(path.name) and path.name != "archive"
                    )
                except (HerdrRunError, OSError) as exc:
                    registry_error = str(exc)
        checked = [
            self._health_one(name, now, probe_deadline) for name in requested
        ]
        sessions = [observation for observation, _status in checked]
        statuses = [status for _observation, status in checked]
        healthy = registry_error is None and all(
            item.get("health") == "healthy" and item.get("recorded") is True
            for item in sessions
        )
        return {
            "schema": _HEALTH_SCHEMA,
            "checked_at": now,
            "healthy": healthy,
            "registry_error": registry_error,
            "sessions": sessions,
        }, statuses

    def health(
        self, names: Sequence[str] = (), *, checked_at: float | None = None,
        deadline: float | None = None,
    ) -> dict[str, object]:
        """Check several names independently and return one machine-readable verdict."""
        return self.health_snapshot(
            names, checked_at=checked_at, deadline=deadline,
        )[0]

    @staticmethod
    def _status_with_health(
        observation: dict[str, object], status: dict[str, object] | None,
    ) -> dict[str, object]:
        """Render status and health from one probe without a second lookup."""
        if status is None:
            status = {
                "name": observation.get("name"),
                "token": observation.get("token"),
                "lifecycle": observation.get("lifecycle"),
                "agent_status": observation.get("agent_status"),
                "probe_error": observation.get("probe_error"),
            }
        else:
            status = dict(status)
        status.update({
            "health": observation.get("health"),
            "runtime_state": observation.get("runtime_state"),
            "health_reason_code": observation.get("reason_code"),
            "health_reason": observation.get("reason"),
            "health_first_detected_at": observation.get("first_detected_at"),
            "health_last_checked_at": observation.get("last_checked_at"),
            "health_recorded": observation.get("recorded"),
            "health_record_path": observation.get("record_path"),
        })
        return status

    def status_with_health(self, name: str) -> tuple[dict[str, object], bool]:
        """Return the canonical status payload and exit verdict from one probe."""
        observation, status = self.status_health_snapshot(name)
        return self._status_with_health(observation, status), observation.get("health") == "healthy"

    def list_with_health(self) -> tuple[list[dict[str, object]], bool]:
        """Return the canonical list payload and aggregate exit verdict."""
        aggregate, statuses = self.health_snapshot(())
        observations = aggregate.get("sessions")
        if not isinstance(observations, list):
            raise AgentDeliveryError("health snapshot has no session list")
        rows = [
            self._status_with_health(observation, status)
            for observation, status in zip(observations, statuses, strict=True)
            if isinstance(observation, dict)
        ]
        return rows, aggregate.get("healthy") is True

    def list(self) -> list[dict[str, object]]:
        """List every registered agent; unavailable Herdr is not evidence of death."""
        if not self.registry.exists():
            return []
        agent._validate_private_directory(str(self.registry), "agent registry")
        return [self.status(path.name) for path in sorted(self.registry.iterdir())
                if _NAME.fullmatch(path.name) and path.name != "archive"]

    def send(self, name: str, text: str, *, message_id: str | None = None, expected_token: str | None = None, **options: object) -> agent.QueueResult:
        """Enqueue under the lifecycle lock, then wait without holding it."""
        if "require_existing" in options:
            raise AgentDeliveryError("require_existing is managed internally")
        max_artifact_bytes = options.get("max_artifact_bytes")
        if max_artifact_bytes is not None and not isinstance(max_artifact_bytes, int):
            raise AgentDeliveryError("max_artifact_bytes must be a positive integer or None")
        atomic_policy = options.get("atomic_policy")
        with self._lock(name):
            record = self._load_expected(name, expected_token)
            self._reconcile_goal_transaction(record)
            self._require_automation(record)
            if record.goal_messages:
                self._save(record)
            identifier = agent.enqueue_bound(
                self._queue(name), record.target(), text,
                message_id=message_id,
                max_artifact_bytes=cast(int | None, max_artifact_bytes),
                atomic_policy=cast(agent.AtomicWritePolicy | None, atomic_policy),
            )
            client = self._delivery_client(record)
        drained = agent.drain(
            client, record.target(), self._queue(name),
            require_existing=True, **options,
        )
        return agent.finish_identified_delivery(
            self._queue(name), identifier, drained,
            max_artifact_bytes=cast(int | None, max_artifact_bytes),
        )

    def drain(self, name: str, **options: object) -> agent.QueueResult:
        """Retry pending messages without retaining the lifecycle lock while busy."""
        if "require_existing" in options:
            raise AgentDeliveryError("require_existing is managed internally")
        max_artifact_bytes = options.get("max_artifact_bytes")
        if max_artifact_bytes is not None and not isinstance(max_artifact_bytes, int):
            raise AgentDeliveryError("max_artifact_bytes must be a positive integer or None")
        with self._lock(name):
            record = self._load(name)
            self._reconcile_goal_transaction(record)
            self._require_automation(record)
            if record.goal_messages:
                self._save(record)
            agent._bind_queue(
                self._queue(name), record.target(),
                max_artifact_bytes=cast(int | None, max_artifact_bytes),
            )
            client = self._delivery_client(record)
        return agent.drain(
            client, record.target(), self._queue(name),
            require_existing=True, **options,
        )  # type: ignore[arg-type]

    def reconcile_delivery(
        self, name: str, message_id: str, expected_sha256: str,
    ) -> agent.QueueResult:
        """Mark one ambiguous Muse prompt delivered from exact transcript evidence."""
        with self._lock(name):
            record = self._load(name)
            self._checked(record)
            if record.launch.adapter != "herdr-pane" or record.launch.harness != "muse":
                raise AgentDeliveryError(
                    "delivery reconciliation currently requires an owned interactive Muse pane"
                )
            client = cast(
                HerdrClient, _WorkspaceClient(self.client, record, queue=self._queue(name)),
            )
            if not os.path.lexists(Path(self._queue(name)) / "target.json"):
                raise AgentDeliveryError(
                    "delivery reconciliation requires an existing exact queue binding"
                )
            agent._validate_existing_binding(self._queue(name), record.target())

            def exact_user_turn(text: str) -> bool:
                target_lock, pane_id, before = agent._lock_resolved_target(
                    client, record.target(),
                )
                try:
                    screen = client.read(
                        pane_id, source="recent-unwrapped", lines=5000,
                    )
                    after = agent.resolve_target(client, record.target())
                    return (
                        before == after
                        and muse_verified_process_prompt_transcript_count(
                            screen, text,
                        ) > 0
                        and not muse_verified_process_prompt_in_composer(
                            screen, text,
                        )
                    )
                finally:
                    os.close(target_lock)

            return agent.reconcile_delivery(
                self._queue(name), message_id, expected_sha256, exact_user_turn,
            )

    def read(self, name: str, *, lines: int = 500) -> str:
        """Read human and coordinator turns together; persist the latest bounded snapshot."""
        with self._lock(name):
            record = self._load(name)
            self._checked(record)
            text = agent.read(self.client, record.target(), lines=lines)
            agent._atomic_json(str(self._directory(name) / "output.json"),
                               {"text": text, "captured_at": time.time(), "pane_id": record.pane_id})
            return text

    def wait(
        self, name: str, *, timeout: float = 900.0,
        sleep: Callable[[float], None] = time.sleep,
        monotonic: Callable[[], float] = time.monotonic,
        expected_token: str | None = None,
    ) -> dict[str, object]:
        """Wait for idle/done, fail visibly on blocked/unknown identity or a deadline.

        This is readiness, not task completion: an active native goal may continue
        after a turn. Callers should inspect the conversation and goal separately.
        """
        if not math.isfinite(timeout) or not 0 <= timeout <= 31_536_000:
            raise AgentDeliveryError("wait timeout must be finite and between 0 and 31536000 seconds")
        token = self._load_expected(name, expected_token).token
        deadline = monotonic() + timeout
        while True:
            with self._lock(name):
                record = self._load_expected(name, token)
                info = self._checked(record)
                if info.status in ("idle", "done", "staged"):
                    return self._status_record(record)
                if info.status not in ("working", "starting", "unknown"):
                    raise AgentDeliveryError(f"agent {name!r} requires attention (state {info.status!r}); read its pane")
            remaining = deadline - monotonic()
            if remaining <= 0:
                raise AgentDeliveryError(f"timed out waiting for agent {name!r} (state {info.status!r})")
            sleep(min(0.25, remaining))

    def bind_session(self, name: str, session_id: str, *, goal_command: Sequence[str] | None = None) -> dict[str, object]:
        """Bind an explicitly known native session; never guess by cwd or change a queue binding."""
        if not session_id or "\0" in session_id:
            raise AgentDeliveryError("native session id must be nonempty and contain no NUL")
        if goal_command is not None and (not goal_command or any(not item or "\0" in item for item in goal_command)):
            raise AgentDeliveryError("goal command must be a nonempty argument vector")
        with self._lock(name):
            with self._identity_transaction():
                record = self._load(name)
                self._reconcile_goal_transaction(record)
                info = self._checked(record)
                for existing in (record.session_value, info.session_value):
                    if existing is not None and existing != session_id:
                        raise AgentDeliveryError("refusing to replace an already bound native session")
                owner = self._identity_owner(record.launch.harness, session_id, exclude=name)
                if owner is not None:
                    raise AgentDeliveryError(
                        f"native session is already registered as {owner.name!r}"
                    )
                reported: list[str] = []
                for pane in self.client.panes():
                    live = self.client.pane_info(pane.pane_id)
                    if (live.session_agent == record.launch.harness
                            and live.session_value == session_id):
                        reported.append(pane.pane_id)
                if reported and (len(reported) != 1
                                 or reported[0] != record.pane_id):
                    raise AgentDeliveryError(
                        "native session is reported by another or ambiguous live pane"
                    )
                # Some Herdr detection sources do not expose session metadata. The
                # caller explicitly asserts the native session; the independently
                # verified live name and pane remain the lifecycle authority.
                record.session_agent = record.launch.harness
                record.session_value = session_id
                record.session_source = (
                    "observed"
                    if (info.session_agent == record.launch.harness
                        and info.session_value == session_id)
                    else "asserted"
                )
                if goal_command is not None:
                    record.goal_command = list(goal_command)
                self._save(record)
                return {"name": name, "session_id": session_id, "source": "explicit"}

    def _goal_delivery(self, record: AgentRecord) -> str | None:
        if record.goal_message_id is not None:
            state = self._goal_artifact_state(record, allow_prepared=True)
            return {
                "prepared": "pending",
                "processed": "delivered",
                "failed": "possibly_submitted",
                "inflight": "possibly_submitted",
                "pending": "pending",
            }[state]
        # Decode-edge compatibility for rows written before session/v3. New
        # writes derive delivery exclusively from the queue artifact location.
        return record.goal_delivery

    def _goal_result(self, record: AgentRecord, command: Sequence[str] | None) -> dict[str, object]:
        result: dict[str, object] = {"name": record.name, "goal": record.goal,
            "delivery": self._goal_delivery(record), "source": "requested", "native_status": "unverified"}
        if record.launch.harness != "codex":
            return result
        session = record.session_value
        if not session:
            result["native_error"] = "native session unknown; bind-session with the session id reported by this agent"
            return result
        from agentctl.codex_goal import CodexGoalError, get_goal
        try:
            native = get_goal(session, command or record.goal_command)
        except CodexGoalError as exc:
            result["native_error"] = str(exc)
        else:
            result["native"] = native
            result["source"] = "native"
            result["native_status"] = "absent" if native is None else native["status"]
            result["goal"] = None if native is None else native["objective"]
        return result

    def _dead_pane_proof(
        self, record: AgentRecord, *, operation: str,
    ) -> _DeadPaneProof:
        """Prove one exact absent-agent, one-pane tab and idle shell generation."""
        pane_id = record.pane_id
        tab_id = record.tab_id
        workspace_id = record.workspace_id
        if not pane_id or not tab_id or not workspace_id:
            raise AgentDeliveryError(
                f"refusing to {operation} {record.name!r}: record lacks pane, tab, or workspace identity"
            )
        def snapshot() -> tuple[Pane, AgentPaneInfo]:
            panes = self.client.panes()
            presentations = [pane for pane in panes if pane.pane_id == pane_id]
            if len(presentations) != 1:
                raise AgentDeliveryError(
                    f"refusing to {operation} {record.name!r}: expected one recorded pane, "
                    f"found {len(presentations)}"
                )
            presentation = presentations[0]
            if (presentation.tab_id != tab_id
                    or presentation.workspace_id != workspace_id):
                raise AgentDeliveryError(
                    f"refusing to {operation} {record.name!r}: recorded pane, tab, or workspace changed"
                )
            tab_panes = [pane for pane in panes if pane.tab_id == tab_id]
            if len(tab_panes) != 1 or tab_panes[0] != presentation:
                raise AgentDeliveryError(
                    f"refusing to {operation} {record.name!r}: recorded tab is not the exact one-pane tab"
                )
            info = self.client.pane_info(pane_id)
            if (info.pane_id != pane_id or info.workspace_id != workspace_id
                    or os.path.realpath(info.cwd) != os.path.realpath(record.launch.cwd)):
                raise AgentDeliveryError(
                    f"refusing to {operation} {record.name!r}: recorded pane, workspace, or cwd changed"
                )
            stale_custom_label = (
                record.launch.adapter == "herdr-pane"
                and record.custom_process_identity is not None
                and info.agent == record.launch.harness
            )
            if info.agent is not None and not stale_custom_label:
                raise AgentDeliveryError(
                    f"refusing to {operation} {record.name!r}: pane still reports agent {info.agent!r}"
                )
            if info.session_agent is not None or info.session_value is not None:
                raise AgentDeliveryError(
                    f"refusing to {operation} {record.name!r}: absent agent has native session identity"
                )
            return presentation, info

        presentation, info = snapshot()
        if record.launch.adapter == "herdr-pane":
            identity = record.custom_process_identity
            if identity is None:
                raise AgentDeliveryError(
                    f"refusing to {operation} {record.name!r}: record lacks custom process identity"
                )
            try:
                absent = self.client.process_generation_absent(identity)
            except HerdrRunError as exc:
                raise AgentDeliveryError(
                    f"refusing to {operation} {record.name!r}: cannot prove recorded "
                    f"custom process generation absent: {exc}"
                ) from exc
            if not absent:
                raise AgentDeliveryError(
                    f"refusing to {operation} {record.name!r}: recorded custom process "
                    "generation is still live"
                )
        try:
            shell = self.client.pane_idle_shell_identity(pane_id)
        except HerdrRunError as exc:
            raise AgentDeliveryError(
                f"refusing to {operation} {record.name!r}: cannot prove supported "
                f"idle shell generation: {exc}"
            ) from exc
        if shell is None:
            raise AgentDeliveryError(
                f"refusing to {operation} {record.name!r}: pane is not a supported idle shell "
                "generation without descendants"
            )
        final_presentation, final_info = snapshot()
        if final_presentation != presentation or final_info != info:
            raise AgentDeliveryError(
                f"refusing to {operation} {record.name!r}: pane membership or agent state "
                "changed during shell proof"
            )
        return _DeadPaneProof(
            info=final_info, presentation=final_presentation, shell=shell,
        )

    def _bounded_terminal_text(self, pane_id: str, *, operation: str) -> str:
        try:
            output = self.client.read(
                pane_id, source="recent-unwrapped", lines=5000
            )
            if not output:
                output = self.client.read(pane_id, source="recent", lines=5000)
            return output
        except HerdrRunError as exc:
            raise AgentDeliveryError(
                f"cannot preserve terminal output before {operation}: {exc}"
            ) from exc

    def _archive_destination(self, record: AgentRecord) -> tuple[Path, Path]:
        _name(record.name)
        _token(record.token)
        archive = self.registry / "archive"
        try:
            archive.mkdir(mode=0o700, exist_ok=True)
        except OSError as exc:
            raise AgentDeliveryError(f"cannot prepare agent archive: {exc}") from exc
        agent._validate_private_directory(str(archive), "agent archive")
        destination = archive / f"{record.name}-{record.token}"
        if os.path.lexists(destination):
            raise AgentDeliveryError(
                f"refusing to overwrite existing agent archive {destination}"
            )
        return archive, destination

    @staticmethod
    def _optional_snapshot_bytes(pinned: _PinnedAgentDirectory) -> bytes | None:
        """Read an existing private snapshot exactly so publication can roll back."""
        path = pinned.path / "output.json"
        flags = (
            os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
            | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
        )
        try:
            descriptor = os.open("output.json", flags, dir_fd=pinned.descriptor)
        except FileNotFoundError:
            return None
        except OSError as exc:
            raise AgentDeliveryError(
                f"cannot open existing output snapshot {path}: {exc}"
            ) from exc
        try:
            before = os.fstat(descriptor)
            if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid()
                    or stat.S_IMODE(before.st_mode) & 0o077 or before.st_nlink != 1
                    or before.st_size > _MAX_SNAPSHOT_BYTES):
                raise AgentDeliveryError(f"unsafe existing output snapshot: {path}")
            content = bytearray()
            while len(content) <= _MAX_SNAPSHOT_BYTES:
                block = os.read(descriptor, min(64 << 10, _MAX_SNAPSHOT_BYTES + 1 - len(content)))
                if not block:
                    break
                content.extend(block)
            after = os.fstat(descriptor)
            if (len(content) > _MAX_SNAPSHOT_BYTES or before.st_size != len(content)
                    or (after.st_dev, after.st_ino, after.st_mode, after.st_uid,
                        after.st_nlink, after.st_size, after.st_mtime_ns,
                        after.st_ctime_ns) != (
                            before.st_dev, before.st_ino, before.st_mode, before.st_uid,
                            before.st_nlink, before.st_size, before.st_mtime_ns,
                            before.st_ctime_ns,
                        )):
                raise AgentDeliveryError(f"existing output snapshot changed while reading: {path}")
            return bytes(content)
        except AgentDeliveryError:
            raise
        except OSError as exc:
            raise AgentDeliveryError(
                f"cannot inspect existing output snapshot {path}: {exc}"
            ) from exc
        finally:
            _close_descriptor(
                descriptor, "existing output snapshot", primary=sys.exc_info()[1],
            )

    @staticmethod
    def _atomic_snapshot_bytes(
        pinned: _PinnedAgentDirectory, content: bytes, *, name: str = "output.json",
    ) -> _InstalledArtifact:
        """Replace one private snapshot with exact bytes and durable directory metadata."""
        if name not in {
            "agent.json", "output.json", _TERMINAL_RETIREMENT_FILE,
        }:
            raise AgentDeliveryError("unsupported pinned registry artifact name")
        if name == "agent.json":
            limit = _MAX_AGENT_RECORD_BYTES
        elif name == _TERMINAL_RETIREMENT_FILE:
            limit = _MAX_TERMINAL_RETIREMENT_BYTES
        else:
            limit = _MAX_SNAPSHOT_BYTES
        if len(content) > limit:
            raise AgentDeliveryError(
                f"refusing {name} larger than {limit} bytes"
            )
        temporary = f".{name}-recovery-{os.getpid()}-{uuid.uuid4().hex}"
        descriptor = -1
        temporary_owned = False
        created: os.stat_result | None = None
        rename_attempted = False
        installed_verified = False
        failure: BaseException | None = None
        try:
            descriptor = os.open(
                temporary,
                os.O_RDWR | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0)
                | getattr(os, "O_NOFOLLOW", 0),
                0o600,
                dir_fd=pinned.descriptor,
            )
            temporary_owned = True
            created = os.fstat(descriptor)
            if (not stat.S_ISREG(created.st_mode)
                    or created.st_uid != os.getuid()
                    or stat.S_IMODE(created.st_mode) & 0o077
                    or created.st_nlink != 1):
                raise AgentDeliveryError(
                    f"unsafe pinned registry artifact staging file for {name}"
                )
            offset = 0
            while offset < len(content):
                written = os.write(descriptor, content[offset:])
                if written <= 0:
                    raise OSError("output snapshot restoration made no progress")
                offset += written
            os.fsync(descriptor)
            held = os.fstat(descriptor)
            held_content = os.pread(descriptor, len(content) + 1, 0)
            staged = os.stat(
                temporary, dir_fd=pinned.descriptor, follow_symlinks=False,
            )
            if (not stat.S_ISREG(held.st_mode)
                    or not stat.S_ISREG(staged.st_mode)
                    or held.st_uid != os.getuid() or staged.st_uid != os.getuid()
                    or stat.S_IMODE(held.st_mode) & 0o077
                    or stat.S_IMODE(staged.st_mode) & 0o077
                    or held.st_nlink != 1 or staged.st_nlink != 1
                    or held.st_size != len(content) or staged.st_size != len(content)
                    or held_content != content
                    or (held.st_dev, held.st_ino) != (created.st_dev, created.st_ino)
                    or (staged.st_dev, staged.st_ino) != (
                        created.st_dev, created.st_ino,
                    )):
                raise AgentDeliveryError(
                    f"registry artifact staging generation changed before installing {name}"
                )
            rename_attempted = True
            try:
                os.replace(
                    temporary, name,
                    src_dir_fd=pinned.descriptor, dst_dir_fd=pinned.descriptor,
                )
            except OSError as exc:
                raise AgentDeliveryError(
                    f"cannot install pinned registry artifact {name}: {exc}"
                ) from exc
            # The staging name no longer belongs to this operation.  Never
            # remove a new entry that appears there after the rename.
            temporary_owned = False
            installed = os.stat(
                name, dir_fd=pinned.descriptor, follow_symlinks=False,
            )
            held = os.fstat(descriptor)
            held_content = os.pread(descriptor, len(content) + 1, 0)
            if (not stat.S_ISREG(installed.st_mode)
                    or installed.st_uid != os.getuid()
                    or stat.S_IMODE(installed.st_mode) & 0o077
                    or installed.st_nlink != 1
                    or installed.st_size != len(content)
                    or held_content != content
                    or (installed.st_dev, installed.st_ino) != (
                        created.st_dev, created.st_ino,
                    )
                    or (held.st_dev, held.st_ino, held.st_size) != (
                        created.st_dev, created.st_ino, len(content),
                    )):
                raise AgentDeliveryError(
                    f"installed registry artifact {name} was not the staged generation"
                )
            installed_verified = True
            os.fsync(pinned.descriptor)
        except AgentDeliveryError as exc:
            failure = exc
        except OSError as exc:
            failure = AgentDeliveryError(
                f"cannot write pinned registry artifact {name}: {exc}"
            )

        # If replace(2) committed but its wrapper reported an error, classify
        # the namespace before any cleanup. A known installed generation may
        # be rolled back by a caller; a replacement or missing generation may
        # not be overwritten.
        if (failure is not None and rename_attempted and not installed_verified
                and created is not None):
            try:
                held = os.fstat(descriptor)
                held_content = os.pread(descriptor, len(content) + 1, 0)
                reconciled_staging: os.stat_result | None
                try:
                    reconciled_staging = os.stat(
                        temporary,
                        dir_fd=pinned.descriptor,
                        follow_symlinks=False,
                    )
                except FileNotFoundError:
                    reconciled_staging = None
                reconciled_install: os.stat_result | None
                try:
                    reconciled_install = os.stat(
                        name,
                        dir_fd=pinned.descriptor,
                        follow_symlinks=False,
                    )
                except FileNotFoundError:
                    reconciled_install = None
                held_exact = (
                    stat.S_ISREG(held.st_mode)
                    and held.st_uid == os.getuid()
                    and not stat.S_IMODE(held.st_mode) & 0o077
                    and held.st_nlink == 1
                    and held.st_size == len(content)
                    and held_content == content
                    and (held.st_dev, held.st_ino) == (created.st_dev, created.st_ino)
                )
                staged_exact = (
                    reconciled_staging is not None
                    and stat.S_ISREG(reconciled_staging.st_mode)
                    and reconciled_staging.st_uid == os.getuid()
                    and not stat.S_IMODE(reconciled_staging.st_mode) & 0o077
                    and reconciled_staging.st_nlink == 1
                    and (reconciled_staging.st_dev, reconciled_staging.st_ino) == (
                        created.st_dev, created.st_ino,
                    )
                )
                installed_exact = (
                    reconciled_install is not None
                    and stat.S_ISREG(reconciled_install.st_mode)
                    and reconciled_install.st_uid == os.getuid()
                    and not stat.S_IMODE(reconciled_install.st_mode) & 0o077
                    and reconciled_install.st_nlink == 1
                    and reconciled_install.st_size == len(content)
                    and (reconciled_install.st_dev, reconciled_install.st_ino) == (
                        created.st_dev, created.st_ino,
                    )
                )
                if held_exact and installed_exact and not staged_exact:
                    temporary_owned = False
                    installed_verified = True
                elif not (held_exact and staged_exact):
                    temporary_owned = False
            except OSError:
                temporary_owned = False

        cleanup_failures: list[tuple[str, BaseException]] = []
        if descriptor >= 0:
            try:
                os.close(descriptor)
            except OSError as exc:
                cleanup_failures.append(("close staging file", exc))
        if temporary_owned:
            try:
                staged = os.stat(
                    temporary,
                    dir_fd=pinned.descriptor,
                    follow_symlinks=False,
                )
                if (created is None
                        or (staged.st_dev, staged.st_ino) != (
                            created.st_dev, created.st_ino,
                        )
                        or not stat.S_ISREG(staged.st_mode)
                        or staged.st_uid != os.getuid()
                        or stat.S_IMODE(staged.st_mode) & 0o077
                        or staged.st_nlink != 1):
                    raise AgentDeliveryError(
                        "registry staging name no longer denotes the owned file; "
                        "replacement was preserved"
                    )
                # A same-uid process ignoring the cooperative registry lock can
                # still race this final identity check and unlink.
                os.unlink(temporary, dir_fd=pinned.descriptor)
            except FileNotFoundError:
                pass
            except (OSError, AgentDeliveryError) as exc:
                cleanup_failures.append(("remove staging file", exc))
        if cleanup_failures:
            detail = "; ".join(
                f"{label}: {error}" for label, error in cleanup_failures
            )
            cleanup_error = AgentDeliveryError(
                f"registry artifact cleanup failed: {detail}"
            )
            if failure is None:
                failure = cleanup_error
            else:
                failure = AgentDeliveryError(f"{failure}; {cleanup_error}")

        installed_artifact = (
            _InstalledArtifact(
                name,
                created.st_dev,
                created.st_ino,
                len(content),
                hashlib.sha256(content).hexdigest(),
            )
            if created is not None else None
        )
        if failure is not None:
            if installed_verified and installed_artifact is not None:
                raise _ArtifactInstalledError(
                    str(failure), installed_artifact,
                ) from failure
            if rename_attempted and not temporary_owned:
                raise _ArtifactPublicationUncertainError(str(failure)) from failure
            raise _ArtifactNotInstalledError(str(failure)) from failure
        if installed_artifact is None or not installed_verified:
            raise _ArtifactPublicationUncertainError(
                f"cannot prove installed registry artifact {name}"
            )
        return installed_artifact

    def _publish_pinned_directory(
        self,
        pinned: _PinnedAgentDirectory,
        destination: Path,
        *,
        expected_record: bytes,
    ) -> None:
        """Atomically publish the exact held generation and its exact record."""
        archive_path = destination.parent
        if archive_path != self.registry / "archive" or not destination.name:
            raise AgentDeliveryError("invalid agent archive destination")
        publication_state = False
        try:
            with self._pinned_parent_directory(
                self.registry, label="agent registry",
            ) as registry_parent, self._pinned_parent_directory(
                archive_path, label="agent archive",
            ) as archive_parent:
                self._verify_pinned_agent_directory(pinned)
                try:
                    source = os.stat(
                        pinned.name,
                        dir_fd=registry_parent.descriptor,
                        follow_symlinks=False,
                    )
                except OSError as exc:
                    raise AgentDeliveryError(
                        f"cannot inspect active agent directory: {exc}"
                    ) from exc
                if (source.st_dev, source.st_ino) != (pinned.device, pinned.inode):
                    raise AgentDeliveryError(
                        f"agent {pinned.name!r} registry directory changed before publication"
                    )
                try:
                    _rename_directory_noreplace_at(
                        registry_parent.descriptor,
                        pinned.name,
                        archive_parent.descriptor,
                        destination.name,
                    )
                except Exception as rename_error:
                    # A successful rename may be reported as an error by the
                    # filesystem or wrapper.  Reconcile both names before the
                    # caller decides whether restoring output is safe.
                    try:
                        try:
                            active = os.stat(
                                pinned.name,
                                dir_fd=registry_parent.descriptor,
                                follow_symlinks=False,
                            )
                        except FileNotFoundError:
                            active = None
                        try:
                            archived = os.stat(
                                destination.name,
                                dir_fd=archive_parent.descriptor,
                                follow_symlinks=False,
                            )
                        except FileNotFoundError:
                            archived = None
                        opened = os.fstat(pinned.descriptor)
                    except OSError as proof_error:
                        publication_state = True
                        raise _ArchivePublicationUncertainError(
                            f"archive rename reported failure ({rename_error}) and its "
                            f"namespace outcome could not be proved: {proof_error}"
                        ) from proof_error
                    active_exact = (
                        active is not None
                        and stat.S_ISDIR(active.st_mode)
                        and active.st_uid == os.getuid()
                        and not stat.S_IMODE(active.st_mode) & 0o077
                        and (active.st_dev, active.st_ino)
                        == (pinned.device, pinned.inode)
                    )
                    archived_exact = (
                        archived is not None
                        and stat.S_ISDIR(archived.st_mode)
                        and archived.st_uid == os.getuid()
                        and not stat.S_IMODE(archived.st_mode) & 0o077
                        and (archived.st_dev, archived.st_ino)
                        == (pinned.device, pinned.inode)
                    )
                    opened_exact = (
                        stat.S_ISDIR(opened.st_mode)
                        and opened.st_uid == os.getuid()
                        and not stat.S_IMODE(opened.st_mode) & 0o077
                        and (opened.st_dev, opened.st_ino)
                        == (pinned.device, pinned.inode)
                    )
                    if active_exact and opened_exact and not archived_exact:
                        raise
                    publication_state = True
                    if active is None and archived_exact and opened_exact:
                        try:
                            published_record = self._record_bytes(
                                pinned, require_active_name=False,
                            )
                        except Exception as proof_error:
                            raise _ArchivePublicationUncertainError(
                                f"archive rename reported failure ({rename_error}); the "
                                "generation was published but its record could not be "
                                f"proved: {proof_error}"
                            ) from proof_error
                        if published_record == expected_record:
                            raise _ArchivePublicationUncertainError(
                                f"archive rename reported failure ({rename_error}) after the "
                                "proved generation was published"
                            ) from rename_error
                        raise _ArchivePublicationUncertainError(
                            f"archive rename reported failure ({rename_error}); the "
                            "generation was published but its record changed"
                        ) from rename_error
                    raise _ArchivePublicationUncertainError(
                        f"archive rename reported failure ({rename_error}) with an "
                        "ambiguous namespace outcome"
                    ) from rename_error
                publication_state = True
                try:
                    self._verify_pinned_parent_directory(
                        registry_parent, label="agent registry",
                    )
                    self._verify_pinned_parent_directory(
                        archive_parent, label="agent archive",
                    )
                    try:
                        os.stat(
                            pinned.name,
                            dir_fd=registry_parent.descriptor,
                            follow_symlinks=False,
                        )
                    except FileNotFoundError:
                        pass
                    except OSError as exc:
                        raise AgentDeliveryError(
                            f"cannot inspect active agent directory after publication: {exc}"
                        ) from exc
                    else:
                        raise AgentDeliveryError(
                            f"agent {pinned.name!r} active name reappeared during publication"
                        )
                    try:
                        published = os.stat(
                            destination.name,
                            dir_fd=archive_parent.descriptor,
                            follow_symlinks=False,
                        )
                        opened = os.fstat(pinned.descriptor)
                    except OSError as exc:
                        raise AgentDeliveryError(
                            f"cannot inspect published agent directory: {exc}"
                        ) from exc
                    if (not stat.S_ISDIR(published.st_mode)
                            or published.st_uid != os.getuid()
                            or stat.S_IMODE(published.st_mode) & 0o077
                            or not stat.S_ISDIR(opened.st_mode)
                            or opened.st_uid != os.getuid()
                            or stat.S_IMODE(opened.st_mode) & 0o077
                            or (published.st_dev, published.st_ino)
                            != (pinned.device, pinned.inode)
                            or (opened.st_dev, opened.st_ino)
                            != (pinned.device, pinned.inode)):
                        raise AgentDeliveryError(
                            f"agent {pinned.name!r} published directory was not the proved generation"
                        )
                    if self._record_bytes(
                        pinned, require_active_name=False,
                    ) != expected_record:
                        raise AgentDeliveryError(
                            f"agent {pinned.name!r} record changed during archival publication"
                        )
                except Exception as cause:
                    # All agentctl participants hold the name lock.  Reprove both
                    # namespace entries immediately before the inverse rename so a
                    # replacement at either name is never promoted to active state.
                    # renameat2 cannot compare an inode atomically, so an actor that
                    # ignores the cooperative lock can still race this last check;
                    # the no-replace rename at least refuses an occupied active name.
                    try:
                        os.stat(
                            pinned.name,
                            dir_fd=registry_parent.descriptor,
                            follow_symlinks=False,
                        )
                    except FileNotFoundError:
                        pass
                    except OSError as proof_error:
                        raise _ArchivePublicationUncertainError(
                            f"archive identity check failed ({cause}); cannot prove the "
                            f"active name absent before rollback: {proof_error}"
                        ) from proof_error
                    else:
                        raise _ArchivePublicationUncertainError(
                            f"archive identity check failed ({cause}); active name reappeared, "
                            "so rollback was not attempted"
                        ) from cause
                    try:
                        rollback_source = os.stat(
                            destination.name,
                            dir_fd=archive_parent.descriptor,
                            follow_symlinks=False,
                        )
                    except OSError as proof_error:
                        raise _ArchivePublicationUncertainError(
                            f"archive identity check failed ({cause}); cannot reprove the "
                            f"published generation before rollback: {proof_error}"
                        ) from proof_error
                    if (not stat.S_ISDIR(rollback_source.st_mode)
                            or rollback_source.st_uid != os.getuid()
                            or stat.S_IMODE(rollback_source.st_mode) & 0o077
                            or (rollback_source.st_dev, rollback_source.st_ino) != (
                                pinned.device, pinned.inode,
                            )):
                        raise _ArchivePublicationUncertainError(
                            f"archive identity check failed ({cause}); archive destination "
                            "was replaced, or became unsafe, so rollback was not attempted"
                        ) from cause
                    try:
                        _rename_directory_noreplace_at(
                            archive_parent.descriptor,
                            destination.name,
                            registry_parent.descriptor,
                            pinned.name,
                        )
                    except Exception as rollback:
                        raise _ArchivePublicationUncertainError(
                            f"archive identity check failed ({cause}) and rollback was incomplete: "
                            f"{rollback}"
                        ) from rollback
                    try:
                        self._verify_pinned_parent_directory(
                            registry_parent, label="agent registry",
                        )
                        self._verify_pinned_parent_directory(
                            archive_parent, label="agent archive",
                        )
                        self._verify_pinned_agent_directory(pinned)
                        restored = os.stat(
                            pinned.name,
                            dir_fd=registry_parent.descriptor,
                            follow_symlinks=False,
                        )
                        if (restored.st_dev, restored.st_ino) != (
                            pinned.device, pinned.inode,
                        ):
                            raise AgentDeliveryError(
                                "restored active name is not the proved generation"
                            )
                        try:
                            os.stat(
                                destination.name,
                                dir_fd=archive_parent.descriptor,
                                follow_symlinks=False,
                            )
                        except FileNotFoundError:
                            pass
                        else:
                            raise AgentDeliveryError(
                                "archive destination remained after rollback"
                            )
                        if self._record_bytes(pinned) != expected_record:
                            raise AgentDeliveryError(
                                "restored agent record is not the proved content"
                            )
                    except Exception as rollback_proof:
                        raise _ArchivePublicationUncertainError(
                            f"archive identity check failed ({cause}); inverse rename completed "
                            f"but rollback state could not be proved: {rollback_proof}"
                        ) from rollback_proof
                    sync_failures: list[str] = []
                    for parent, label in (
                        (archive_parent, "agent archive rollback"),
                        (registry_parent, "agent registry rollback"),
                    ):
                        try:
                            _fsync_pinned_directory(parent.descriptor, label)
                        except OSError as exc:
                            sync_failures.append(f"{label}: {exc}")
                    if sync_failures:
                        raise _ArchivePublicationUncertainError(
                            f"archive identity check failed ({cause}); rollback completed but "
                            f"directory durability is uncertain: {'; '.join(sync_failures)}"
                        ) from cause
                    publication_state = False
                    raise
                sync_failures = []
                for parent, label in (
                    (archive_parent, "published agent archive"),
                    (registry_parent, "published agent registry"),
                ):
                    try:
                        _fsync_pinned_directory(parent.descriptor, label)
                    except OSError as exc:
                        sync_failures.append(f"{label}: {exc}")
                if sync_failures:
                    raise _ArchivePublicationUncertainError(
                        "agent archive was published but directory durability is uncertain: "
                        f"{'; '.join(sync_failures)}"
                    )
        except _ArchivePublicationUncertainError:
            raise
        except Exception as error:
            if publication_state:
                raise _ArchivePublicationUncertainError(
                    f"agent archive publication state is uncertain after publication: {error}"
                ) from error
            raise

    @staticmethod
    def _restore_output_snapshot(
        pinned: _PinnedAgentDirectory, previous: bytes | None,
    ) -> None:
        if previous is None:
            try:
                os.unlink("output.json", dir_fd=pinned.descriptor)
            except FileNotFoundError:
                pass
            os.fsync(pinned.descriptor)
        else:
            ManagedAgents._atomic_snapshot_bytes(pinned, previous)

    @staticmethod
    def _verify_installed_output_snapshot(
        pinned: _PinnedAgentDirectory,
        installed: _InstalledArtifact,
    ) -> None:
        """Require output to retain the exact generation and bytes we installed."""
        descriptor = -1
        try:
            descriptor = os.open(
                installed.name,
                os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
                | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0),
                dir_fd=pinned.descriptor,
            )
            before = os.fstat(descriptor)
            content = bytearray()
            while len(content) <= installed.size:
                block = os.read(
                    descriptor,
                    min(64 << 10, installed.size + 1 - len(content)),
                )
                if not block:
                    break
                content.extend(block)
            after = os.fstat(descriptor)
        except OSError as exc:
            raise _ArchivePublicationUncertainError(
                f"cannot reprove installed output before rollback: {exc}"
            ) from exc
        finally:
            if descriptor >= 0:
                _close_descriptor(
                    descriptor,
                    "installed output rollback proof",
                    primary=sys.exc_info()[1],
                )
        if (not stat.S_ISREG(before.st_mode)
                or before.st_uid != os.getuid()
                or stat.S_IMODE(before.st_mode) & 0o077
                or before.st_nlink != 1
                or before.st_size != installed.size
                or len(content) != installed.size
                or hashlib.sha256(content).hexdigest() != installed.digest
                or (before.st_dev, before.st_ino) != (
                    installed.device, installed.inode,
                )
                or (after.st_dev, after.st_ino, after.st_mode, after.st_uid,
                    after.st_nlink, after.st_size, after.st_mtime_ns,
                    after.st_ctime_ns) != (
                        before.st_dev, before.st_ino, before.st_mode, before.st_uid,
                        before.st_nlink, before.st_size, before.st_mtime_ns,
                        before.st_ctime_ns,
                    )):
            raise _ArchivePublicationUncertainError(
                "installed output generation changed before use; replacement was preserved"
            )

    @staticmethod
    def _restore_installed_output_snapshot(
        pinned: _PinnedAgentDirectory,
        previous: bytes | None,
        installed: _InstalledArtifact,
    ) -> None:
        """Restore only while output still names the generation we installed."""
        ManagedAgents._verify_installed_output_snapshot(pinned, installed)
        # Registry participants hold the per-name lock; a same-uid process that
        # ignores it can still race this final check and replacement operation.
        ManagedAgents._restore_output_snapshot(pinned, previous)

    def _publish_archive(
        self,
        pinned: _PinnedAgentDirectory,
        destination: Path,
        snapshot: dict[str, object],
        *,
        expected_record: bytes,
        before_publish: Callable[[], None],
    ) -> None:
        """Prepare output, reprove state, then publish in one namespace move."""
        previous = self._optional_snapshot_bytes(pinned)
        installed: _InstalledArtifact | None = None
        try:
            installed = self._atomic_snapshot_bytes(
                pinned, _snapshot_json_bytes(snapshot)
            )
            self._verify_installed_output_snapshot(pinned, installed)
            before_publish()
            self._publish_pinned_directory(
                pinned, destination, expected_record=expected_record,
            )
        except _ArchivePublicationUncertainError:
            # The held directory was moved and could not be safely returned to
            # the active name.  Restoring through its fd would silently mutate a
            # retained or displaced generation rather than roll back publication.
            raise
        except _ArtifactNotInstalledError:
            # The previous named output was never replaced.
            raise
        except _ArtifactInstalledError as error:
            try:
                self._restore_installed_output_snapshot(
                    pinned, previous, error.installed,
                )
            except Exception as rollback:
                raise _ArchivePublicationUncertainError(
                    f"archive preparation failed ({error}) and exact output rollback "
                    f"was unsafe or incomplete: {rollback}"
                ) from rollback
            raise
        except Exception:
            if installed is None:
                raise
            try:
                self._restore_installed_output_snapshot(pinned, previous, installed)
            except Exception as rollback:
                raise _ArchivePublicationUncertainError(
                    "archive publication failed and exact output rollback was unsafe or "
                    f"incomplete: {rollback}"
                ) from rollback
            raise

    def _recover_legacy_adoption(
        self, record: AgentRecord, *, expected_token: str | None,
        expected_record_sha256: str | None,
        expected_token_explicit: bool,
    ) -> dict[str, object]:
        """Explicitly retire an identity-less adopted record without touching its pane."""
        if not record.pane_id:
            raise AgentDeliveryError(
                f"refusing to recover legacy adoption {record.name!r}: record lacks pane identity"
            )
        with self._pane_lock(record.pane_id):
            with self._pinned_agent_directory(record.name) as pinned:
                return self._recover_legacy_adoption_locked(
                    record, pinned=pinned, expected_token=expected_token,
                    expected_record_sha256=expected_record_sha256,
                    expected_token_explicit=expected_token_explicit,
                )

    def _recover_legacy_adoption_locked(
        self, record: AgentRecord, *, pinned: _PinnedAgentDirectory,
        expected_token: str | None,
        expected_record_sha256: str | None,
        expected_token_explicit: bool,
    ) -> dict[str, object]:
        if (not expected_token_explicit or expected_token is None
                or expected_record_sha256 is None):
            raise AgentDeliveryError(
                "legacy adoption recovery requires --expected-token and "
                "--expected-record-sha256"
            )
        initial = self._legacy_record_snapshot(
            pinned,
            expected_token=expected_token,
            expected_digest=expected_record_sha256,
        )
        if initial.record.to_document() != record.to_document():
            raise AgentDeliveryError(
                f"refusing to recover adoption {record.name!r}: registry record changed"
            )
        record = initial.record
        if (record.launch.adapter != "herdr-foreign"
                or record.lifecycle != "running"
                or record.launch.mode != "interactive"
                or record.launch.backend != "herdr"
                or record.foreign_shell_identity is not None):
            raise AgentDeliveryError(
                "--recover-legacy-adoption applies only to a running herdr-foreign "
                "record missing foreign_shell_identity"
            )
        before = self._dead_pane_proof(record, operation="recover legacy adoption")
        text = self._bounded_terminal_text(
            before.info.pane_id, operation="legacy adoption recovery"
        )
        current_snapshot = self._legacy_record_snapshot(
            pinned,
            expected_token=expected_token,
            expected_digest=expected_record_sha256,
        )
        if current_snapshot != initial:
            raise AgentDeliveryError(
                f"refusing to recover legacy adoption {record.name!r}: registry record changed"
            )
        current = current_snapshot.record
        after = self._dead_pane_proof(current, operation="recover legacy adoption")
        if after != before:
            raise AgentDeliveryError(
                f"refusing to recover legacy adoption {record.name!r}: "
                "runtime identity changed during output capture"
            )
        _archive, destination = self._archive_destination(current)

        def reprove_before_publish() -> None:
            final_snapshot = self._legacy_record_snapshot(
                pinned,
                expected_token=expected_token,
                expected_digest=expected_record_sha256,
            )
            if final_snapshot != initial:
                raise AgentDeliveryError(
                    f"refusing to recover adoption {record.name!r}: registry record changed"
                )
            if self._dead_pane_proof(
                final_snapshot.record, operation="recover legacy adoption"
            ) != before:
                raise AgentDeliveryError(
                    f"refusing to recover legacy adoption {record.name!r}: "
                    "runtime identity changed before archival"
                )

        self._publish_archive(
            pinned,
            destination,
            {"text": text, "captured_at": time.time(), "pane_id": record.pane_id,
             "recovery_shell_identity": {
                 **asdict(before.shell.identity),
                 "executable_path": before.shell.executable_path,
             }},
            expected_record=initial.content,
            before_publish=reprove_before_publish,
        )
        return {
            "name": record.name,
            "archive": str(destination),
            "pane_closed": False,
            "tab_closed": False,
            "runtime_preserved": True,
            "recovered_legacy_adoption": True,
            "record_sha256": expected_record_sha256,
            "recovery_shell_identity": {
                **asdict(before.shell.identity),
                "executable_path": before.shell.executable_path,
            },
        }

    def _retire_managed_dead(
        self, record: AgentRecord, *, expected_token: str | None,
        expected_token_explicit: bool,
    ) -> dict[str, object]:
        """Retire one owned pane only after proving a stable returned shell."""
        if not record.pane_id:
            raise AgentDeliveryError(
                f"refusing to retire dead managed agent {record.name!r}: record lacks pane identity"
            )
        with self._pane_lock(record.pane_id):
            with self._pinned_agent_directory(record.name) as pinned:
                return self._retire_managed_dead_locked(
                    record, pinned=pinned, expected_token=expected_token,
                    expected_token_explicit=expected_token_explicit,
                )

    @staticmethod
    def _terminal_retirement_document(
        record_bytes: bytes,
    ) -> dict[str, object]:
        """Commit publication of a record that already owns its terminal result."""
        return {
            "schema": _TERMINAL_RETIREMENT_SCHEMA,
            "record_sha256": hashlib.sha256(record_bytes).hexdigest(),
        }

    @staticmethod
    def _legacy_managed_dead_retirement_document(
        record: AgentRecord, record_bytes: bytes,
    ) -> dict[str, object]:
        """Decode-only shape written by pre-terminal-receipt checkpoints."""
        return {
            "schema": _MANAGED_DEAD_RETIREMENT_SCHEMA,
            "name": record.name,
            "token": record.token,
            "record_sha256": hashlib.sha256(record_bytes).hexdigest(),
            "pane_closed": False,
            "runtime_preserved": True,
        }

    @staticmethod
    def _retirement_artifact_exists(
        pinned: _PinnedAgentDirectory, name: str, *, purpose: str,
    ) -> bool:
        try:
            metadata = os.stat(
                name, dir_fd=pinned.descriptor, follow_symlinks=False,
            )
        except FileNotFoundError:
            return False
        except OSError as exc:
            raise AgentDeliveryError(f"cannot inspect {purpose}: {exc}") from exc
        if (not stat.S_ISREG(metadata.st_mode)
                or metadata.st_uid != os.getuid()
                or stat.S_IMODE(metadata.st_mode) & 0o077
                or metadata.st_nlink != 1):
            raise AgentDeliveryError(f"unsafe {purpose}")
        return True

    def _terminal_retirement_exists(self, record: AgentRecord) -> bool:
        """Detect a durable current receipt, or its decode-only predecessor."""
        with self._pinned_agent_directory(record.name) as pinned:
            present = self._retirement_artifact_exists(
                pinned, _TERMINAL_RETIREMENT_FILE,
                purpose="terminal retirement receipt",
            ) or self._retirement_artifact_exists(
                pinned, _MANAGED_DEAD_RETIREMENT_FILE,
                purpose="legacy managed-dead retirement receipt",
            )
            self._verify_pinned_agent_directory(pinned)
            return present

    def _read_terminal_retirement(
        self, pinned: _PinnedAgentDirectory, record: AgentRecord,
        record_bytes: bytes,
    ) -> TerminalState | None:
        if self._retirement_artifact_exists(
            pinned, _TERMINAL_RETIREMENT_FILE,
            purpose="terminal retirement receipt",
        ):
            path = pinned.path / _TERMINAL_RETIREMENT_FILE
            content = self._pinned_artifact_bytes(
                pinned, name=_TERMINAL_RETIREMENT_FILE,
                limit=_MAX_TERMINAL_RETIREMENT_BYTES,
                purpose="terminal retirement receipt",
            )
            document = agent._decode_json_bytes(
                content, "terminal retirement receipt", path,
            )
            if not isinstance(document, dict):
                raise AgentDeliveryError(
                    "terminal retirement receipt disagrees with its stopped record"
                )
            digest = hashlib.sha256(record_bytes).hexdigest()
            if document.get("schema") == _TERMINAL_RETIREMENT_SCHEMA:
                if (set(document) != {"schema", "record_sha256"}
                        or document.get("record_sha256") != digest
                        or record.terminal is None):
                    raise AgentDeliveryError(
                        "terminal retirement receipt disagrees with its stopped record"
                    )
                return record.terminal
            if document.get("schema") == _LEGACY_TERMINAL_RETIREMENT_SCHEMA:
                if (set(document) != {
                        "schema", "record_sha256", "outcome", "evidence",
                    }
                        or document.get("record_sha256") != digest
                        or not isinstance(document.get("outcome"), str)
                        or not isinstance(document.get("evidence"), dict)):
                    raise AgentDeliveryError(
                        "legacy terminal receipt disagrees with its stopped record"
                    )
                terminal = TerminalState.create(
                    cast(str, document["outcome"]),
                    cast(dict[str, object], document["evidence"]),
                )
                if record.terminal is not None and record.terminal != terminal:
                    raise AgentDeliveryError(
                        "legacy terminal receipt conflicts with canonical terminal state"
                    )
                return terminal
            raise AgentDeliveryError(
                "terminal retirement receipt has an unsupported schema"
            )
        if not self._retirement_artifact_exists(
            pinned, _MANAGED_DEAD_RETIREMENT_FILE,
            purpose="legacy managed-dead retirement receipt",
        ):
            return None
        path = pinned.path / _MANAGED_DEAD_RETIREMENT_FILE
        content = self._pinned_artifact_bytes(
            pinned, name=_MANAGED_DEAD_RETIREMENT_FILE,
            limit=_MAX_AGENT_RECORD_BYTES,
            purpose="legacy managed-dead retirement receipt",
        )
        document = agent._decode_json_bytes(
            content, "legacy managed-dead retirement receipt", path,
        )
        expected = self._legacy_managed_dead_retirement_document(record, record_bytes)
        if document != expected:
            raise AgentDeliveryError(
                "legacy managed-dead retirement receipt disagrees with its stopped record"
            )
        terminal = TerminalState.create("managed-dead-preserved")
        if record.terminal is not None and record.terminal != terminal:
            raise AgentDeliveryError(
                "legacy managed-dead receipt conflicts with canonical terminal state"
            )
        return terminal

    def _terminal_retirement_result(
        self, record: AgentRecord, destination: Path, terminal: TerminalState,
    ) -> dict[str, object]:
        if record.terminal is not None and record.terminal != terminal:
            raise AgentDeliveryError(
                "terminal publication state disagrees with its stopped record"
            )
        outcome = terminal.outcome
        evidence = terminal.evidence
        if outcome == "turn-runner-stopped":
            if (record.launch.adapter != "turn-runner"
                    or set(evidence) != {"killed_window"}
                    or not isinstance(evidence.get("killed_window"), bool)):
                raise AgentDeliveryError("terminal receipt has inconsistent runtime outcome")
            expected_runtime_home = self._directory(record.name) / "runtime"
            expected_inner_archive = (
                destination / "runtime/state/_archive"
                / f"{record.name}-{record.token}"
            )
            if record.launch.runtime_home != str(expected_runtime_home):
                raise AgentDeliveryError(
                    "terminal receipt runtime archive is not the exact derived path"
                )
            runtime = {
                "record": None,
                "result": {
                    "name": record.name,
                    "killed_window": evidence["killed_window"],
                    "archived_to": str(expected_inner_archive),
                    "state_path": None,
                    "was_registered": True,
                    "forced": False,
                    "unverified_presentation": None,
                },
            }
            return {
                "name": record.name, "archive": str(destination),
                "runtime": runtime,
            }
        if outcome == "foreign-unregistered":
            if (record.launch.adapter != "herdr-foreign"
                    or record.launch.mode != "interactive"
                    or record.launch.backend != "herdr"
                    or record.launch.runtime_ownership != "foreign"
                    or evidence):
                raise AgentDeliveryError("terminal receipt has inconsistent foreign outcome")
            return {
                "name": record.name, "archive": str(destination),
                "pane_closed": False, "tab_closed": False,
                "runtime_preserved": True,
            }
        if outcome in {
            "managed-dead-closed", "owned-pane-closed", "owned-runtime-absent",
        }:
            compatible_adapter = (
                record.launch.adapter == "herdr"
                if outcome == "managed-dead-closed"
                else record.launch.adapter in {"herdr", "herdr-pane"}
            )
            if (not compatible_adapter
                    or record.launch.mode != "interactive"
                    or record.launch.backend != "herdr"
                    or record.launch.runtime_ownership != "owned"):
                raise AgentDeliveryError("terminal receipt has inconsistent owned outcome")
            if outcome in {"managed-dead-closed", "owned-pane-closed"}:
                if (set(evidence) != {"tab_closed"}
                        or (evidence["tab_closed"] is not None
                            and not isinstance(evidence["tab_closed"], bool))):
                    raise AgentDeliveryError("terminal receipt has invalid pane-close evidence")
                result = {
                    "name": record.name, "archive": str(destination),
                    "pane_closed": True, "tab_closed": evidence["tab_closed"],
                }
                if outcome == "managed-dead-closed":
                    result.update({
                        "managed_dead": True,
                        "runtime_preserved": False,
                        "continuation": None,
                    })
                return result
            if evidence:
                raise AgentDeliveryError("terminal receipt has invalid absent-runtime evidence")
            return {
                "name": record.name, "archive": str(destination),
                "pane_closed": False, "tab_closed": False,
            }
        if (record.launch.adapter != "herdr-pane"
                or record.launch.harness != "muse"
                or record.launch.mode != "interactive"
                or record.launch.backend != "herdr"
                or record.launch.runtime_ownership != "owned"
                or record.custom_process_identity is None
                or not record.workspace_id or not record.tab_id or not record.pane_id
                or evidence):
            raise AgentDeliveryError("terminal receipt has inconsistent custom outcome")
        if outcome == "managed-dead-preserved":
            return {
                "name": record.name,
                "archive": str(destination),
                "pane_closed": False,
                "tab_closed": False,
                "managed_dead": True,
                "runtime_preserved": True,
                "continuation": (
                    "The dead custom runtime was archived; its shell pane was preserved "
                    "because Herdr does not provide a terminal-generation-conditional close."
                ),
            }
        if outcome == "custom-runtime-absent":
            return {
                "name": record.name,
                "archive": str(destination),
                "pane_closed": False,
                "tab_closed": None,
                "ordinary_stop_recovered": True,
                "runtime_preserved": False,
            }
        raise AgentDeliveryError("terminal receipt has unsupported outcome")

    def _turn_runner_retirement_evidence(
        self, record: AgentRecord, destination: Path, runtime: object,
    ) -> dict[str, object]:
        """Validate a worker stop reply and retain only its irreducible outcome."""
        if (not isinstance(runtime, dict)
                or set(runtime) != {"result", "record"}
                or runtime.get("record") is not None
                or not isinstance(runtime.get("result"), dict)):
            raise AgentDeliveryError("terminal receipt has invalid runtime evidence")
        result = cast(dict[str, object], runtime["result"])
        if (set(result) != {
                "name", "killed_window", "archived_to", "state_path",
                "was_registered", "forced", "unverified_presentation",
            }
                or result.get("name") != record.name
                or any(not isinstance(result.get(field), bool) for field in (
                    "killed_window", "was_registered", "forced",
                ))
                or not isinstance(result.get("archived_to"), str)
                or (result.get("state_path") is not None
                    and not isinstance(result.get("state_path"), str))
                or result.get("was_registered") is not True
                or result.get("forced") is not False
                or result.get("unverified_presentation") is not None
                or result.get("state_path") is not None):
            raise AgentDeliveryError("terminal receipt has invalid runtime stop result")
        expected_inner_archive = (
            destination / "runtime/state/_archive" / f"{record.name}-{record.token}"
        )
        if result.get("archived_to") != str(expected_inner_archive):
            raise AgentDeliveryError(
                "terminal receipt runtime archive is not the exact derived path"
            )
        return {"killed_window": cast(bool, result["killed_window"])}

    def _terminal_archive_receipt(
        self, name: str, expected_token: str | None, *,
        after_record_read: Callable[[], None] = lambda: None,
        after_receipt_read: Callable[[], None] = lambda: None,
    ) -> dict[str, object] | None:
        if expected_token is None or os.path.lexists(self._directory(name)):
            return None
        _name(name)
        _token(expected_token, "expected token")
        destination = self.registry / "archive" / f"{name}-{expected_token}"
        if not os.path.lexists(destination):
            return None
        with self._pinned_agent_directory(
            name, path=destination, label="terminal agent archive",
        ) as pinned:
            snapshot = self._managed_record_snapshot(
                pinned, expected_token=expected_token,
            )
            record = snapshot.record
            if record.lifecycle != "stopped":
                raise AgentDeliveryError(
                    "terminal archive does not contain a stopped generation"
                )
            after_record_read()
            receipt = self._read_terminal_retirement(
                pinned, record, snapshot.content,
            )
            if receipt is None:
                return None
            after_receipt_read()
            self._verify_pinned_agent_directory(pinned)
        return self._terminal_retirement_result(record, destination, receipt)

    def _complete_terminal_publication(
        self, record: AgentRecord, *, pinned: _PinnedAgentDirectory,
        expected_token: str,
    ) -> dict[str, object]:
        snapshot = self._managed_record_snapshot(
            pinned, expected_token=expected_token,
        )
        final = snapshot.record
        if final.lifecycle != "stopped":
            raise AgentDeliveryError(
                "terminal publication requires an exact stopped record"
            )
        receipt = self._read_terminal_retirement(pinned, final, snapshot.content)
        if receipt is None:
            raise AgentDeliveryError(
                "terminal publication has no matching retirement receipt"
            )
        _archive, destination = self._archive_destination(final)
        # Validate outcome/record coherence before making the publication
        # visible.  This is also the one result derivation path used after a
        # lost response.
        self._terminal_retirement_result(final, destination, receipt)
        self._publish_pinned_directory(
            pinned, destination, expected_record=snapshot.content,
        )
        return self._terminal_archive_receipt(
            final.name, expected_token,
        ) or self._terminal_retirement_result(final, destination, receipt)

    def _complete_ordinary_stopped_custom_publication(
        self, record: AgentRecord, *, expected_token: str | None,
        expected_token_explicit: bool,
    ) -> dict[str, object]:
        """Archive a post-close stop prefix without ever touching a new pane."""
        if not expected_token_explicit or expected_token is None:
            raise AgentDeliveryError(
                f"recovering stopped custom agent {record.name!r} requires --expected-token"
            )
        with self._pinned_agent_directory(record.name) as pinned:
            snapshot = self._managed_record_snapshot(
                pinned, expected_token=expected_token,
            )
            current = snapshot.record
            if (current.lifecycle != "stopped"
                    or current.launch.adapter != "herdr-pane"
                    or current.launch.harness != "muse"
                    or current.launch.mode != "interactive"
                    or current.launch.backend != "herdr"
                    or current.launch.runtime_ownership != "owned"
                    or current.pane_id is None):
                raise AgentDeliveryError(
                    "ordinary stopped custom publication has inconsistent state"
                )
            # This invocation never closes a pane. A matching pane present at
            # proof time is ambiguous; one appearing afterward is preserved.
            if any(
                pane.pane_id == current.pane_id for pane in self.client.panes()
            ):
                raise AgentDeliveryError(
                    "stopped custom runtime still has a pane but no "
                    "managed-dead retirement receipt; pane was preserved"
                )
            _archive, destination = self._archive_destination(current)
            current.terminal = TerminalState.create("custom-runtime-absent")
            current._legacy_terminal_authority = False
            stopped_bytes = agent._json_text(
                current.to_storage_document()
            ).encode("utf-8")
            self._atomic_snapshot_bytes(
                pinned, stopped_bytes, name="agent.json",
            )
            receipt = self._terminal_retirement_document(stopped_bytes)
            self._atomic_snapshot_bytes(
                pinned, _snapshot_json_bytes(receipt),
                name=_TERMINAL_RETIREMENT_FILE,
            )
            self._publish_pinned_directory(
                pinned, destination, expected_record=stopped_bytes,
            )
        return self._terminal_archive_receipt(
            current.name, expected_token,
        ) or self._terminal_retirement_result(
            current, destination, cast(TerminalState, current.terminal),
        )

    def _retire_managed_dead_locked(
        self, record: AgentRecord, *, pinned: _PinnedAgentDirectory,
        expected_token: str | None,
        expected_token_explicit: bool,
    ) -> dict[str, object]:
        if not expected_token_explicit or expected_token is None:
            raise AgentDeliveryError(
                f"retiring dead managed agent {record.name!r} requires --expected-token"
            )
        initial = self._managed_record_snapshot(
            pinned, expected_token=expected_token,
        )
        if initial.record.to_document() != record.to_document():
            raise AgentDeliveryError(
                f"refusing to retire dead managed agent {record.name!r}: registry record changed"
            )
        record = initial.record
        if (record.lifecycle == "stopped"
                and record.launch.adapter == "herdr-pane"):
            return self._complete_terminal_publication(
                record, pinned=pinned, expected_token=expected_token,
            )
        if (record.launch.adapter not in ("herdr", "herdr-pane")
                or record.lifecycle != "running"
                or record.launch.mode != "interactive" or record.launch.backend != "herdr"):
            raise AgentDeliveryError(
                "managed-dead retirement requires a running herdr record with "
                "owned interactive routing"
            )
        before = self._dead_pane_proof(record, operation="retire dead managed agent")
        text = self._bounded_terminal_text(
            before.info.pane_id, operation="managed-dead retirement"
        )
        current_snapshot = self._managed_record_snapshot(
            pinned, expected_token=expected_token,
        )
        if current_snapshot != initial:
            raise AgentDeliveryError(
                f"refusing to retire dead managed agent {record.name!r}: registry record changed"
            )
        current = current_snapshot.record
        after = self._dead_pane_proof(current, operation="retire dead managed agent")
        if after != before:
            raise AgentDeliveryError(
                f"refusing to retire dead managed agent {record.name!r}: "
                "runtime identity changed during output capture"
            )
        _archive, destination = self._archive_destination(current)
        final_snapshot = self._managed_record_snapshot(
            pinned, expected_token=expected_token,
        )
        if final_snapshot != initial:
            raise AgentDeliveryError(
                f"refusing to retire dead managed agent {record.name!r}: registry record changed"
            )
        final = final_snapshot.record
        if self._dead_pane_proof(final, operation="retire dead managed agent") != before:
            raise AgentDeliveryError(
                f"refusing to retire dead managed agent {record.name!r}: "
                "runtime identity changed before close"
            )
        final.lifecycle = "stopped"
        close_pane = final.launch.adapter == "herdr"
        final.terminal = TerminalState.create(
            "managed-dead-closed" if close_pane else "managed-dead-preserved",
            {"tab_closed": False} if close_pane else {},
        )
        self._migrate_legacy_goal_messages(final)
        bounded_stopped_bytes = agent._json_text(
            final.to_storage_document()
        ).encode("utf-8")
        if len(bounded_stopped_bytes) > _MAX_AGENT_RECORD_BYTES:
            raise AgentDeliveryError(
                f"refusing stopped agent record larger than {_MAX_AGENT_RECORD_BYTES} bytes"
            )
        previous_output = self._optional_snapshot_bytes(pinned)
        installed: _InstalledArtifact | None = None
        try:
            installed = self._atomic_snapshot_bytes(
                pinned,
                _snapshot_json_bytes({
                    "text": text, "captured_at": time.time(), "pane_id": record.pane_id,
                    "retirement_shell_identity": {
                        **asdict(before.shell.identity),
                        "executable_path": before.shell.executable_path,
                    },
                }),
            )
        except _ArtifactNotInstalledError:
            raise
        except _ArtifactPublicationUncertainError:
            raise
        except _ArtifactInstalledError as error:
            try:
                self._restore_installed_output_snapshot(
                    pinned, previous_output, error.installed,
                )
            except Exception as rollback:
                raise _ArchivePublicationUncertainError(
                    f"managed retirement preparation failed ({error}) and exact output "
                    f"rollback was unsafe or incomplete: {rollback}"
                ) from rollback
            raise
        except Exception:
            if installed is None:
                raise
            try:
                self._restore_installed_output_snapshot(
                    pinned, previous_output, installed,
                )
            except Exception as rollback:
                raise _ArchivePublicationUncertainError(
                    "managed retirement preparation failed and exact output rollback was "
                    f"unsafe or incomplete: {rollback}"
                ) from rollback
            raise
        try:
            assert installed is not None
            self._verify_installed_output_snapshot(pinned, installed)
            if self._record_bytes(pinned) != initial.content:
                raise AgentDeliveryError(
                    f"refusing to retire dead managed agent {record.name!r}: "
                    "registry record changed immediately before close"
                )
            if self._dead_pane_proof(
                final, operation="retire dead managed agent"
            ) != before:
                raise AgentDeliveryError(
                    f"refusing to retire dead managed agent {record.name!r}: "
                    "runtime identity changed immediately before close"
                )
        except Exception:
            try:
                assert installed is not None
                self._restore_installed_output_snapshot(
                    pinned, previous_output, installed,
                )
            except Exception as rollback:
                raise _ArchivePublicationUncertainError(
                    "managed retirement final runtime proof failed and rollback "
                    f"was unsafe or incomplete: {rollback}"
                ) from rollback
            raise
        assert final.pane_id is not None
        if close_pane:
            self.client.close_pane(final.pane_id)
            try:
                tab_closed: bool | None = not any(
                    pane.tab_id == final.tab_id for pane in self.client.panes()
                )
            except HerdrRunError:
                tab_closed = None
            final.terminal = TerminalState.create(
                "managed-dead-closed", {"tab_closed": tab_closed},
            )
        else:
            final.terminal = TerminalState.create("managed-dead-preserved")
        final._legacy_terminal_authority = False
        stopped_bytes = agent._json_text(final.to_storage_document()).encode("utf-8")
        self._atomic_snapshot_bytes(pinned, stopped_bytes, name="agent.json")
        retirement = self._terminal_retirement_document(stopped_bytes)
        self._atomic_snapshot_bytes(
            pinned, _snapshot_json_bytes(retirement),
            name=_TERMINAL_RETIREMENT_FILE,
        )
        self._publish_pinned_directory(
            pinned, destination, expected_record=stopped_bytes,
        )
        return self._terminal_retirement_result(
            final, destination, cast(TerminalState, final.terminal),
        )

    def goal(self, name: str, text: str | None = None, *, goal_command: Sequence[str] | None = None, **options: object) -> dict[str, object]:
        """Read native goal state when bound, or submit a goal to the visible conversation.

        Codex receives its native /goal command. Other harnesses receive a plain
        goal instruction. Delivery proves submission, not native goal completion.
        """
        if text is None:
            record = self._load(name)
            self._checked(record)
            return self._goal_result(record, goal_command)
        if not text.strip() or "\n" in text or "\r" in text:
            raise AgentDeliveryError("goal must be a nonempty single line")
        if "require_existing" in options:
            raise AgentDeliveryError("require_existing is managed internally")
        max_artifact_bytes = options.get("max_artifact_bytes")
        if max_artifact_bytes is not None and not isinstance(max_artifact_bytes, int):
            raise AgentDeliveryError("max_artifact_bytes must be a positive integer or None")
        atomic_policy = options.get("atomic_policy")
        with self._lock(name):
            record = self._load(name)
            self._reconcile_goal_transaction(record)
            self._require_automation(record)
            record.goal = text
            identifier = f"{time.time_ns():020d}-{os.getpid()}"
            self._write_goal_transaction(record, identifier, text)
            record.goal_message_id = identifier
            self._save(record)
            prompt = _goal_prompt(record.launch.harness, text)
            agent.enqueue_bound(
                self._queue(name), record.target(), prompt,
                message_id=identifier, kind="goal",
                max_artifact_bytes=_MAX_QUEUE_ARTIFACT_BYTES,
                atomic_policy=cast(agent.AtomicWritePolicy | None, atomic_policy),
            )
            self._remove_goal_transaction(record)
            client = self._delivery_client(record)
        drained = agent.drain(
            client, record.target(), self._queue(name),
            require_existing=True, **options,
        )
        agent.finish_identified_delivery(
            self._queue(name), identifier, drained,
            max_artifact_bytes=_MAX_QUEUE_ARTIFACT_BYTES,
        )
        return self._goal_result(record, goal_command)

    def stop(
        self, name: str, *, expected_token: str | None = None,
        recover_legacy_adoption: bool = False,
        expected_record_sha256: str | None = None,
    ) -> dict[str, object]:
        """Retire one exact registered generation or recover an adopted runtime."""
        return self._stop(
            name,
            expected_token=expected_token,
            recover_legacy_adoption=recover_legacy_adoption,
            expected_record_sha256=expected_record_sha256,
            expected_token_explicit=expected_token is not None,
        )

    def _stop(
        self, name: str, *, expected_token: str | None,
        recover_legacy_adoption: bool,
        expected_record_sha256: str | None,
        expected_token_explicit: bool,
    ) -> dict[str, object]:
        """Close only the owned single-pane tab and archive the complete record/queue.

        A confirmed missing pane permits archival. A probe failure, changed agent
        identity, or extra human-created panes refuses teardown and keeps state.
        """
        with self._lock(name):
            if (expected_token_explicit and not recover_legacy_adoption
                    and expected_record_sha256 is None):
                receipt = self._terminal_archive_receipt(name, expected_token)
                if receipt is not None:
                    return receipt
            record = self._load_expected(name, expected_token)
            self._reconcile_goal_transaction(record)
            confirmed_record = self._load_expected(name, record.token)
            if confirmed_record.to_document() != record.to_document():
                raise AgentDeliveryError(f"agent {name!r} record changed before stop")
            record = confirmed_record
            relocation = self._read_relocation_journal(record)
            if relocation is not None:
                raise AgentDeliveryError(
                    "unfinished relocation must be reconciled with agentctl relocate "
                    "before stop"
                )
            self._refuse_inflight_delivery(record)
            if recover_legacy_adoption:
                return self._recover_legacy_adoption(
                    record, expected_token=expected_token,
                    expected_record_sha256=expected_record_sha256,
                    expected_token_explicit=expected_token_explicit,
                )
            if expected_record_sha256 is not None:
                raise AgentDeliveryError(
                    "--expected-record-sha256 requires --recover-legacy-adoption"
                )
            if record.lifecycle == "stopped" and self._terminal_retirement_exists(record):
                if not expected_token_explicit or expected_token is None:
                    raise AgentDeliveryError(
                        f"recovering stopped agent {record.name!r} requires --expected-token"
                    )
                with self._pinned_agent_directory(record.name) as pinned:
                    return self._complete_terminal_publication(
                        record, pinned=pinned, expected_token=expected_token,
                    )
            if (record.launch.adapter == "herdr-pane"
                    and record.lifecycle == "stopped"):
                return self._complete_ordinary_stopped_custom_publication(
                    record, expected_token=expected_token,
                    expected_token_explicit=expected_token_explicit,
                )
            if record.launch.adapter == "herdr-foreign":
                # This registry owns only delivery state.  Revalidate and retain
                # one final snapshot, but never close, rename, signal, or
                # otherwise mutate the adopted runtime.
                evidence_client = _WorkspaceClient(
                    self.client, record, check_prompt=False,
                )

                def inspect_foreign() -> AdoptedRuntimeEvidence:
                    evidence = evidence_client.adopted_evidence(refresh=True)
                    if evidence.state not in {
                        AdoptedRuntimeState.LIVE_EXACT,
                        AdoptedRuntimeState.IDLE_SHELL_EXACT,
                    }:
                        raise AgentDeliveryError(
                            f"refusing to unregister adopted agent {name!r}: "
                            f"{evidence.reason}"
                        )
                    assert evidence.info is not None
                    assert evidence.presentation is not None
                    return evidence

                evidence = inspect_foreign()
                assert evidence.info is not None
                info = evidence.info
                output = self._bounded_terminal_text(
                    info.pane_id, operation="unregistering"
                )
                try:
                    final_evidence = inspect_foreign()
                except (AgentDeliveryError, HerdrRunError) as exc:
                    raise AgentDeliveryError(
                        f"refusing to unregister adopted agent {name!r}: "
                        "runtime identity could not be reverified after output capture"
                    ) from exc
                if final_evidence != evidence:
                    raise AgentDeliveryError(
                        f"refusing to unregister adopted agent {name!r}: "
                        "runtime identity changed during output capture"
                    )
                _archive, destination = self._archive_destination(record)
                agent._atomic_json(str(self._directory(name) / "output.json"),
                                   {"text": output, "captured_at": time.time(),
                                    "pane_id": record.pane_id})
                try:
                    persisted_evidence = inspect_foreign()
                except (AgentDeliveryError, HerdrRunError) as exc:
                    raise AgentDeliveryError(
                        f"refusing to unregister adopted agent {name!r}: "
                        "runtime identity could not be reverified before archival"
                    ) from exc
                if persisted_evidence != evidence:
                    raise AgentDeliveryError(
                        f"refusing to unregister adopted agent {name!r}: "
                        "runtime identity changed before archival"
                    )
                record.lifecycle = "stopped"
                record.terminal = TerminalState.create("foreign-unregistered")
                record._legacy_terminal_authority = False
                self._save(record)
                with self._pinned_agent_directory(name) as pinned:
                    snapshot = self._managed_record_snapshot(
                        pinned, expected_token=record.token,
                    )
                    receipt = self._terminal_retirement_document(snapshot.content)
                    self._atomic_snapshot_bytes(
                        pinned, _snapshot_json_bytes(receipt),
                        name=_TERMINAL_RETIREMENT_FILE,
                    )
                    self._publish_pinned_directory(
                        pinned, destination, expected_record=snapshot.content,
                    )
                return self._terminal_retirement_result(
                    snapshot.record, destination,
                    cast(TerminalState, snapshot.record.terminal),
                )
            panes = self.client.panes()
            if (record.launch.adapter in ("herdr", "herdr-pane")
                    and record.lifecycle in ("running", "stopping")
                    and record.pane_id is not None):
                recorded = [pane for pane in panes if pane.pane_id == record.pane_id]
                if len(recorded) == 1:
                    observed = self.client.pane_info(record.pane_id)
                    dead = record.launch.adapter == "herdr" and observed.agent is None
                    if (record.launch.adapter == "herdr-pane"
                            and record.custom_process_identity is not None):
                        dead = self.client.process_generation_absent(
                            record.custom_process_identity
                        )
                    if dead:
                        if record.lifecycle != "running":
                            raise AgentDeliveryError(
                                "managed-dead retirement requires a running herdr record "
                                "with owned interactive routing"
                            )
                        return self._retire_managed_dead(
                            record, expected_token=expected_token,
                            expected_token_explicit=expected_token_explicit,
                        )
            tab_closed: bool | None = False
            owned = [pane for pane in panes if pane.tab_id == record.tab_id]
            if record.pane_id is None and record.lifecycle == "launch_failed" and len(owned) == 1:
                # Recover an older partial allocation only when its unique root
                # still identifies an unclaimed shell in the recorded directory.
                info = self.client.pane_info(owned[0].pane_id)
                if (info.agent is None and info.workspace_id == record.workspace_id
                        and os.path.realpath(info.cwd) == os.path.realpath(record.launch.cwd)):
                    record.pane_id = owned[0].pane_id
                    self._save(record)
            if any(pane.pane_id == record.pane_id and pane.tab_id != record.tab_id for pane in panes):
                raise AgentDeliveryError("refusing to archive an agent whose pane moved to another tab")
            if owned:
                if len(owned) != 1 or owned[0].pane_id != record.pane_id or owned[0].workspace_id != record.workspace_id:
                    raise AgentDeliveryError("refusing to close a tab whose pane ownership changed")
            _archive, destination = self._archive_destination(record)
            if owned:
                if (record.launch.adapter == "herdr-pane" or record.lifecycle == "running"
                        or self.client.pane_info(owned[0].pane_id).agent is not None):
                    self._checked_or_failed_pane_report(record, owned[0].pane_id)
                try:
                    output = self.client.read(owned[0].pane_id, source="recent-unwrapped", lines=5000)
                    if not output:
                        output = self.client.read(owned[0].pane_id, source="recent", lines=5000)
                    agent._atomic_json(str(self._directory(name) / "output.json"),
                                       {"text": output, "captured_at": time.time(), "pane_id": record.pane_id})
                except HerdrRunError as exc:
                    raise AgentDeliveryError(f"cannot preserve terminal output before stop: {exc}") from exc
                # Output capture can involve another control round trip. Recheck
                # the owned native identity before acting on that pane again.
                if (record.launch.adapter == "herdr-pane" or record.lifecycle == "running"
                        or self.client.pane_info(owned[0].pane_id).agent is not None):
                    self._checked_or_failed_pane_report(record, owned[0].pane_id)
                record.lifecycle = "stopping"
                self._save(record)
                assert record.pane_id is not None
                self.client.close_pane(record.pane_id)
                try:
                    tab_closed = not any(pane.tab_id == record.tab_id for pane in self.client.panes())
                except HerdrRunError:
                    tab_closed = None
            outcome = "owned-pane-closed" if owned else "owned-runtime-absent"
            evidence = {"tab_closed": tab_closed} if owned else {}
            record.lifecycle = "stopped"
            record.terminal = TerminalState.create(outcome, evidence)
            record._legacy_terminal_authority = False
            self._save(record)
            with self._pinned_agent_directory(name) as pinned:
                snapshot = self._managed_record_snapshot(
                    pinned, expected_token=record.token,
                )
                receipt = self._terminal_retirement_document(snapshot.content)
                self._atomic_snapshot_bytes(
                    pinned, _snapshot_json_bytes(receipt),
                    name=_TERMINAL_RETIREMENT_FILE,
                )
                self._publish_pinned_directory(
                    pinned, destination, expected_record=snapshot.content,
                )
            return self._terminal_retirement_result(
                snapshot.record, destination,
                cast(TerminalState, snapshot.record.terminal),
            )

    def _read_relocation_journal(
        self, record: AgentRecord,
    ) -> _RelocationJournal | None:
        path = self._directory(record.name) / "relocation.json"
        try:
            path.lstat()
        except FileNotFoundError:
            return None
        except OSError as exc:
            raise AgentDeliveryError(f"cannot inspect relocation journal: {exc}") from exc
        raw = agent._read_queue_json(
            str(path), "relocation journal", require_private=True,
            max_artifact_bytes=_MAX_AGENT_RECORD_BYTES,
        )
        journal = _RelocationJournal.from_document(raw)
        if journal.token != record.token:
            raise AgentDeliveryError("relocation journal belongs to another agent generation")
        return journal

    def _finish_relocation(
        self, record: AgentRecord, journal: _RelocationJournal, pane: Pane,
    ) -> dict[str, object]:
        if pane.terminal_id != journal.terminal_id:
            raise AgentDeliveryError("relocated pane terminal identity changed")
        if pane.workspace_id != journal.target_workspace_id:
            raise AgentDeliveryError("relocated pane is not in the intended workspace")
        panes = self.client.panes()
        if sum(item.tab_id == pane.tab_id for item in panes) != 1:
            raise AgentDeliveryError("relocated destination tab is not a one-pane tab")
        candidate = replace(
            record, workspace_id=pane.workspace_id,
            tab_id=pane.tab_id, pane_id=pane.pane_id,
        )
        self._checked(candidate)
        agent._relocate_existing_binding(
            self._queue(record.name), record.target(), candidate.target(),
        )
        self._save(candidate)
        persisted = self._load_expected(record.name, record.token)
        if (persisted.workspace_id, persisted.tab_id, persisted.pane_id) != (
            pane.workspace_id, pane.tab_id, pane.pane_id,
        ):
            raise AgentDeliveryError("relocation routing commit did not persist")
        self._checked(persisted)
        agent._validate_existing_binding(self._queue(record.name), persisted.target())
        current = [
            item for item in self.client.panes()
            if item.terminal_id == journal.terminal_id
        ]
        if len(current) != 1 or current[0] != pane:
            raise AgentDeliveryError("relocated terminal changed after routing commit")
        with self._pinned_agent_directory(record.name) as pinned:
            metadata = os.stat(
                "relocation.json", dir_fd=pinned.descriptor, follow_symlinks=False,
            )
            if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid()
                    or stat.S_IMODE(metadata.st_mode) & 0o077 or metadata.st_nlink != 1):
                raise AgentDeliveryError("unsafe relocation journal during completion")
            os.unlink("relocation.json", dir_fd=pinned.descriptor)
            _fsync_pinned_directory(pinned.descriptor, "agent directory")
        return {
            "name": persisted.name,
            "token": persisted.token,
            "workspace_id": pane.workspace_id,
            "tab_id": pane.tab_id,
            "pane_id": pane.pane_id,
            "terminal_id": pane.terminal_id,
            "relocated": True,
        }

    def relocate(
        self, name: str, *, workspace_id: str | None = None,
        workspace_label: str | None = None, new_tab: bool = False,
    ) -> dict[str, object]:
        """Move one live interactive pane through a recoverable routing transaction."""
        if not new_tab:
            raise AgentDeliveryError("relocate currently requires --new-tab")
        if (workspace_id is None) == (workspace_label is None):
            raise AgentDeliveryError(
                "relocate requires exactly one of workspace id or workspace label"
            )
        selector = workspace_id if workspace_id is not None else workspace_label
        if selector is None or not selector or "\0" in selector:
            raise AgentDeliveryError("workspace selector must be nonempty and contain no NUL")
        with self._lock(name):
            record = self._load(name)
            self._reconcile_goal_transaction(record)
            if (record.lifecycle != "running" or record.launch.mode != "interactive"
                    or record.launch.backend != "herdr" or record.pane_id is None
                    or record.tab_id is None or record.workspace_id is None):
                raise AgentDeliveryError(
                    "relocate requires a running interactive Herdr session with complete routing"
                )
            target_workspace = workspace_id
            if target_workspace is not None:
                self.client.workspace_label(target_workspace)
            else:
                assert workspace_label is not None
                target_workspace = self.client.workspace_id_for_label(workspace_label)
                if target_workspace is None:
                    raise AgentDeliveryError(
                        f"workspace label {workspace_label!r} does not exist"
                    )
            assert target_workspace is not None
            journal = self._read_relocation_journal(record)
            if journal is not None and journal.target_workspace_id != target_workspace:
                raise AgentDeliveryError(
                    "unfinished relocation targets another workspace; retry that exact target"
                )
            if journal is None:
                if target_workspace == record.workspace_id:
                    raise AgentDeliveryError("agent is already in the requested workspace")
                self._checked(record)
                panes = self.client.panes()
                old = [item for item in panes if item.pane_id == record.pane_id]
                if (len(old) != 1 or old[0].tab_id != record.tab_id
                        or old[0].workspace_id != record.workspace_id
                        or old[0].terminal_id is None):
                    raise AgentDeliveryError(
                        "cannot prove the current pane/tab/terminal routing for relocation"
                    )
                if sum(item.tab_id == record.tab_id for item in panes) != 1:
                    raise AgentDeliveryError("relocate refuses a multi-pane source tab")
                journal = _RelocationJournal(
                    record.token, old[0].terminal_id,
                    _PaneRoute(record.workspace_id, record.tab_id, record.pane_id),
                    target_workspace,
                )
                agent._atomic_json(
                    str(self._directory(name) / "relocation.json"),
                    journal.to_document(),
                )

            panes = self.client.panes()
            current = [
                item for item in panes if item.terminal_id == journal.terminal_id
            ]
            if len(current) != 1:
                raise AgentDeliveryError(
                    "cannot uniquely locate the relocation terminal generation"
                )
            pane = current[0]
            old_route = _PaneRoute(
                pane.workspace_id, pane.tab_id, pane.pane_id,
            )
            if old_route == journal.old:
                self._checked(record)
                fresh_panes = self.client.panes()
                fresh = [
                    item for item in fresh_panes
                    if item.terminal_id == journal.terminal_id
                ]
                if (len(fresh) != 1
                        or _PaneRoute(
                            fresh[0].workspace_id, fresh[0].tab_id, fresh[0].pane_id,
                        ) != journal.old
                        or sum(item.tab_id == journal.old.tab_id
                               for item in fresh_panes) != 1):
                    raise AgentDeliveryError(
                        "source pane routing changed before relocation"
                    )
                pane = fresh[0]
                moved = self.client.move_pane_to_new_tab(
                    pane.pane_id, expected_terminal_id=journal.terminal_id,
                    workspace_id=target_workspace, label=record.name,
                )
                if (moved.previous_pane_id, moved.previous_tab_id,
                        moved.previous_workspace_id) != (
                    journal.old.pane_id, journal.old.tab_id,
                    journal.old.workspace_id,
                ):
                    raise AgentDeliveryError(
                        "pane move returned a different source routing identity"
                    )
                pane = moved.pane
            elif pane.workspace_id != target_workspace:
                raise AgentDeliveryError(
                    "relocation terminal is neither at its old route nor intended destination"
                )
            return self._finish_relocation(record, journal, pane)

    @staticmethod
    def _require_automation(record: AgentRecord) -> None:
        if record.paused:
            raise AgentDeliveryError(f"agent {record.name!r} is paused for human input; resume it before automated submission")

    def pause(self, name: str, *, paused: bool = True) -> dict[str, object]:
        """Pause automated input without interrupting an active harness turn."""
        with self._lock(name):
            record = self._load(name)
            self._reconcile_goal_transaction(record)
            self._checked(record)
            record.paused = paused
            self._save(record)
            return {"name": name, "token": record.token, "paused": paused}

    def attach(self, name: str, *, expected_token: str | None = None) -> dict[str, object]:
        """Focus the verified native terminal; input ownership changes separately."""
        with self._lock(name):
            record = self._load_expected(name, expected_token)
            info = self._checked(record)
            self.client.focus_pane(info.pane_id)
            return {"name": name, "pane_id": info.pane_id, "paused": record.paused}
