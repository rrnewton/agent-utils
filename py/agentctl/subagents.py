"""Named, long-lived interactive agents sharing a Herdr workspace.

Herdr owns the terminal and harness process; this module owns durable names,
launch intent, queue routing, output snapshots, and conservative tab teardown.
It never closes a workspace, silently restarts a conversation, or changes the
harness's permission settings. Coordinators and humans see the same terminal.
"""
from __future__ import annotations

import copy
import fcntl
import hashlib
import ctypes
import errno
import json
import math
import os
import re
import stat
import sys
import time
import uuid
from collections.abc import Callable, Iterator, Sequence
from contextlib import contextmanager
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import cast

from agentctl import agent
from agentctl.client import (
    AgentPaneInfo,
    CustomProcessIdentity,
    HerdrClient,
    Pane,
    PaneShellProof,
    muse_auto_review_idle_composer,
    muse_idle_composer,
    muse_prompt_in_composer,
    muse_prompt_is_exact_composer,
    muse_prompt_transcript_count,
    muse_startup_metadata,
    muse_trust_prompt,
    muse_verified_process_composer,
    muse_verified_process_goal_paused,
    muse_verified_process_idle_composer,
)
from agentctl.errors import AgentDeliveryError, HerdrRunError, HerdrUnavailable
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
_MAX_AGENT_RECORD_BYTES = 1 << 20
_MAX_SNAPSHOT_BYTES = 16 << 20
_SHA256 = re.compile(r"[0-9a-f]{64}\Z")
_NESTED_SESSION_SCHEMAS = frozenset({
    "agentctl-session/v2", "agentctl-session/v3",
})
_NESTED_LAUNCH_SCHEMAS = frozenset({
    "agentctl-launch/v1", "agentctl-launch/v2",
})


def _rename_directory_noreplace_at(
    source_parent: int,
    source_name: str,
    destination_parent: int,
    destination_name: str,
) -> None:
    """Atomically move one entry between pinned directories without replacement."""
    try:
        renameat2 = ctypes.CDLL(None, use_errno=True).renameat2
    except AttributeError as exc:
        raise AgentDeliveryError(
            "cannot archive agent: atomic no-replace rename is unavailable"
        ) from exc
    renameat2.argtypes = [
        ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint,
    ]
    renameat2.restype = ctypes.c_int
    ctypes.set_errno(0)
    if renameat2(
        source_parent,
        os.fsencode(source_name),
        destination_parent,
        os.fsencode(destination_name),
        1,
    ) == 0:
        return
    number = ctypes.get_errno()
    if number == errno.EEXIST:
        raise AgentDeliveryError(
            f"refusing to replace existing agent archive {destination_name}"
        )
    if number in (errno.ENOSYS, errno.EINVAL, errno.EOPNOTSUPP):
        raise AgentDeliveryError(
            "cannot archive agent: filesystem lacks atomic no-replace rename support"
        )
    raise AgentDeliveryError(
        f"cannot archive agent {source_name} as {destination_name}: {os.strerror(number)}"
    )


def _fsync_pinned_directory(descriptor: int, label: str) -> None:
    """Durably order one already-pinned directory; ``label`` is diagnostic only."""
    del label
    os.fsync(descriptor)


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


@dataclass
class AgentRecord:
    """Versioned launch identity for managed interactive agents."""
    name: str
    token: str
    harness: str
    cwd: str
    created_at: float
    schema: int = 1
    lifecycle: str = "starting"
    workspace_id: str | None = None
    tab_id: str | None = None
    pane_id: str | None = None
    session_agent: str | None = None
    session_value: str | None = None
    model: str | None = None
    resume: str | None = None
    arguments: list[str] = field(default_factory=list)
    startup_warning: str | None = None
    effective_reasoning_effort: str | None = None
    error: str | None = None
    goal: str | None = None
    goal_delivery: str | None = None
    goal_session_id: str | None = None
    goal_command: list[str] | None = None
    goal_messages: dict[str, str] = field(default_factory=dict)
    goal_message_id: str | None = None
    adapter: str = "herdr"
    mode: str = "interactive"
    backend: str = "herdr"
    paused: bool = False
    runtime_home: str | None = None
    pane_reported_by_agentctl: bool = False
    custom_process_identity: CustomProcessIdentity | None = None
    foreign_shell_identity: CustomProcessIdentity | None = None
    _unknown: dict[str, object] = field(default_factory=dict, init=False, repr=False)
    _nested_storage: dict[str, object] | None = field(
        default=None, init=False, repr=False,
    )
    _session_source: str | None = field(default=None, init=False, repr=False)

    def to_document(self) -> dict[str, object]:
        """Serialize in the decoded row's schema without duplicating launch intent."""
        if self._nested_storage is not None:
            return self._nested_document()
        document = asdict(self)
        document.pop("_unknown")
        document.pop("_nested_storage")
        document.pop("_session_source")
        if self._unknown.keys() & document.keys():
            raise AgentDeliveryError("unknown agent metadata conflicts with a known schema field")
        document.update(self._unknown)
        return document

    def _nested_document(self) -> dict[str, object]:
        """Update mutable fields while retaining one canonical nested LaunchSpec."""
        assert self._nested_storage is not None
        document = copy.deepcopy(self._nested_storage)
        launch_value = document.get("launch")
        if not isinstance(launch_value, dict):  # pragma: no cover - decode invariant
            raise AgentDeliveryError("nested agent record lost its launch specification")
        launch = cast(dict[str, object], launch_value)
        argv = launch.get("argv")
        projection = {
            "harness": self.harness,
            "cwd": self.cwd,
            "adapter": self.adapter,
            "mode": self.mode,
            "backend": self.backend,
            "model": self.model,
            "resume": self.resume,
            "runtime_home": self.runtime_home,
        }
        if any(launch.get(key) != value for key, value in projection.items()) or (
            not isinstance(argv, list) or list(argv[1:]) != self.arguments
        ):
            raise AgentDeliveryError(
                "decoded nested launch intent changed; migrate through a canonical writer"
            )
        document.update({
            "name": self.name,
            "token": self.token,
            "created_at": self.created_at,
            "lifecycle": self.lifecycle,
            "workspace_id": self.workspace_id,
            "tab_id": self.tab_id,
            "pane_id": self.pane_id,
            "startup_warning": self.startup_warning,
            "effective_reasoning_effort": self.effective_reasoning_effort,
            "error": self.error,
            "paused": self.paused,
            "pane_reported_by_agentctl": self.pane_reported_by_agentctl,
            "foreign_shell_identity": (
                None if self.foreign_shell_identity is None
                else asdict(self.foreign_shell_identity)
            ),
        })
        custom_identity = (
            None if self.custom_process_identity is None
            else asdict(self.custom_process_identity)
        )
        if document["schema"] == "agentctl-session/v2":
            document.update({
                "session_agent": self.session_agent,
                "session_value": self.session_value,
                "goal": self.goal,
                "goal_delivery": self.goal_delivery,
                "goal_session_id": self.goal_session_id,
                "goal_command": self.goal_command,
                "goal_messages": dict(self.goal_messages),
                "goal_message_id": self.goal_message_id,
                "custom_process_identity": custom_identity,
            })
        else:
            document["native_session"] = (
                None if self.session_value is None else {
                    "schema": "agentctl-native-session/v1",
                    "agent": self.session_agent or self.harness,
                    "value": self.session_value,
                    "source": self._session_source or "observed",
                }
            )
            document["goal"] = {
                "schema": "agentctl-goal/v1",
                "objective": self.goal,
                "message_id": self.goal_message_id,
                "native_command": self.goal_command,
            }
            document["custom_process_identity"] = custom_identity
        return document

    @classmethod
    def load(cls, path: Path, name: str) -> AgentRecord:
        """Reject malformed or non-private state before using any recorded identity."""
        value = agent._read_queue_json(str(path), "agent record", require_private=True)
        return cls._from_value(value, path, name)

    @classmethod
    def _from_value(cls, value: object, path: Path, name: str) -> AgentRecord:
        """Validate an already identity-bound record value."""
        if not isinstance(value, dict):
            raise AgentDeliveryError(f"invalid agent record: {path}")
        source = cast(dict[str, object], value)
        nested_storage: dict[str, object] | None = None
        nested_session_source: str | None = None
        if source.get("schema") in _NESTED_SESSION_SCHEMAS:
            nested_storage = copy.deepcopy(source)
            if source.get("schema") == "agentctl-session/v3":
                native = source.get("native_session")
                if isinstance(native, dict):
                    candidate = native.get("source")
                    if isinstance(candidate, str):
                        nested_session_source = candidate
            else:
                nested_session_source = (
                    "observed" if source.get("session_value") is not None
                    else "asserted" if source.get("goal_session_id") is not None
                    else None
                )
            document = cls._normalize_nested_storage(source, path)
        else:
            document = source
        for key in ("name", "token", "harness", "cwd", "lifecycle"):
            if not isinstance(document.get(key), str) or not document[key]:
                raise AgentDeliveryError(f"invalid agent record field {key}: {path}")
        for key in ("workspace_id", "tab_id", "pane_id", "session_agent", "session_value", "model", "resume", "startup_warning", "effective_reasoning_effort", "error", "goal", "goal_delivery", "goal_session_id", "goal_message_id"):
            if document.get(key) is not None and not isinstance(document[key], str):
                raise AgentDeliveryError(f"invalid agent record field {key}: {path}")
        created = document.get("created_at")
        args = document.get("arguments")
        goal_command = document.get("goal_command")
        if (document.get("schema") != 1 or isinstance(document.get("schema"), bool)
            or document["name"] != name or not isinstance(created, (int, float))
            or isinstance(created, bool) or not math.isfinite(created)
            or re.fullmatch(r"[a-z0-9-]{1,80}", str(document["token"])) is None
            or not isinstance(args, list) or any(not isinstance(item, str) for item in args)):
            raise AgentDeliveryError(f"invalid agent record: {path}")
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
        known = {key for key, field_info in cls.__dataclass_fields__.items() if field_info.init}
        fields = {key: document[key] for key in known if key in document}
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
        if custom_identity is not None:
            fields["custom_process_identity"] = custom_identity
        if foreign_shell_identity is not None:
            fields["foreign_shell_identity"] = foreign_shell_identity
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
        record = cls(**fields)  # type: ignore[arg-type]
        record._unknown = {key: value for key, value in document.items() if key not in known}
        record._nested_storage = nested_storage
        record._session_source = nested_session_source
        return record

    @classmethod
    def _normalize_nested_storage(
        cls, document: dict[str, object], path: Path,
    ) -> dict[str, object]:
        """Decode v2/v3 rows at the boundary while leaving launch data canonical."""
        schema = document.get("schema")
        common = {
            "schema", "name", "token", "created_at", "lifecycle", "launch",
            "workspace_id", "tab_id", "pane_id", "startup_warning",
            "effective_reasoning_effort", "error", "paused",
            "pane_reported_by_agentctl", "foreign_shell_identity",
            "runner_identity", "extensions",
        }
        expected = (
            common | {
                "session_agent", "session_value", "goal", "goal_delivery",
                "goal_session_id", "goal_command", "goal_messages",
                "goal_message_id", "custom_process_identity",
            }
            if schema == "agentctl-session/v2"
            else common | {"native_session", "goal", "custom_process_identity"}
        )
        if set(document) != expected:
            raise AgentDeliveryError(f"invalid agent record {schema} fields: {path}")
        launch_value = document.get("launch")
        if not isinstance(launch_value, dict):
            raise AgentDeliveryError(f"invalid agent record launch specification: {path}")
        launch = cast(dict[str, object], launch_value)
        launch_fields = {
            "schema", "harness", "cwd", "adapter", "mode", "backend",
            "model", "resume", "profile", "argv", "environment_names",
            "runtime_home", "runtime_ownership", "executable",
        }
        if launch.get("schema") == "agentctl-launch/v2":
            launch_fields.add("permission_mode")
        elif launch.get("schema") not in _NESTED_LAUNCH_SCHEMAS:
            raise AgentDeliveryError(f"invalid agent record launch schema: {path}")
        if set(launch) != launch_fields:
            raise AgentDeliveryError(f"invalid agent record launch fields: {path}")
        lifecycle = document.get("lifecycle")
        if lifecycle not in {
            "starting", "running", "stopping", "stopped",
            "launch_failed", "adopt_failed",
        }:
            raise AgentDeliveryError(f"invalid agent record lifecycle: {path}")
        harness = launch.get("harness")
        cwd = launch.get("cwd")
        adapter = launch.get("adapter")
        mode = launch.get("mode")
        backend = launch.get("backend")
        ownership = launch.get("runtime_ownership")
        profile = launch.get("profile")
        permission_mode = launch.get("permission_mode")
        if (not isinstance(harness, str) or _KIND.fullmatch(harness) is None
                or not isinstance(cwd, str) or not Path(cwd).is_absolute()
                or adapter not in ("herdr", "herdr-pane", "herdr-foreign")
                or mode != "interactive" or backend != "herdr"
                or profile is not None
                    and (not isinstance(profile, str) or not profile or "\0" in profile)
                or permission_mode is not None
                or launch.get("runtime_home") is not None):
            raise AgentDeliveryError(
                f"invalid nested interactive launch contract: {path}"
            )
        if ((adapter == "herdr-foreign" and ownership != "foreign")
                or (adapter != "herdr-foreign" and ownership != "owned")
                or (adapter == "herdr-pane" and harness != "muse")):
            raise AgentDeliveryError(
                f"inconsistent launch ownership or adapter: {path}"
            )
        for field_name in ("model", "resume"):
            field_value = launch.get(field_name)
            if (field_value is not None
                    and (not isinstance(field_value, str)
                         or not field_value or "\0" in field_value)):
                raise AgentDeliveryError(
                    f"invalid agent record launch {field_name}: {path}"
                )
        argv = launch.get("argv")
        environment_names = launch.get("environment_names")
        if (not isinstance(argv, list)
                or any(not isinstance(item, str) or not item or "\0" in item for item in argv)
                or not isinstance(environment_names, list)
                or any(not isinstance(item, str)
                       or _ENVIRONMENT_NAME.fullmatch(item) is None
                       for item in environment_names)):
            raise AgentDeliveryError(f"invalid agent record launch vector: {path}")
        executable = launch.get("executable")
        if executable is not None and (
            not isinstance(executable, dict)
            or set(executable) != {"path", "device", "inode"}
            or not isinstance(executable.get("path"), str)
            or not os.path.isabs(cast(str, executable["path"]))
            or any(not isinstance(executable.get(key), int)
                   or isinstance(executable.get(key), bool)
                   or cast(int, executable[key]) <= 0
                   for key in ("device", "inode"))
        ):
            raise AgentDeliveryError(
                f"invalid agent record launch executable identity: {path}"
            )
        expected_program = (
            cast(dict[str, object], executable).get("path")
            if isinstance(executable, dict) else harness
        )
        if ((adapter == "herdr-foreign" and argv)
                or (adapter != "herdr-foreign"
                    and (not argv or argv[0] != expected_program))):
            raise AgentDeliveryError(f"invalid agent record launch argv: {path}")
        if adapter in ("herdr", "herdr-pane"):
            try:
                structured = harness_arguments(
                    harness,
                    model=cast(str | None, launch.get("model")),
                    resume=cast(str | None, launch.get("resume")),
                )
            except AgentDeliveryError as exc:
                raise AgentDeliveryError(
                    f"invalid nested launch presets in {path}: {exc}"
                ) from exc
            if tuple(argv[1:1 + len(structured)]) != structured:
                raise AgentDeliveryError(
                    f"nested launch argv contradicts model or resume: {path}"
                )
        if document.get("runner_identity") is not None:
            raise AgentDeliveryError(
                f"interactive nested record cannot contain runner identity: {path}"
            )
        if (adapter == "herdr-pane"
                and lifecycle in {"running", "stopping", "stopped"}
                and document.get("custom_process_identity") is None):
            raise AgentDeliveryError(
                f"active nested Muse record has no runtime process identity: {path}"
            )
        extensions = document.get("extensions")
        if not isinstance(extensions, dict) or any(
            not isinstance(key, str) or key in expected or key in launch_fields
            for key in extensions
        ):
            raise AgentDeliveryError(f"invalid agent record extensions: {path}")
        normalized = {
            key: value for key, value in document.items()
            if key not in {"schema", "launch", "extensions", "native_session"}
        }
        normalized.update({
            "schema": 1,
            "harness": launch.get("harness"),
            "cwd": launch.get("cwd"),
            "adapter": launch.get("adapter"),
            "mode": launch.get("mode"),
            "backend": launch.get("backend"),
            "model": launch.get("model"),
            "resume": launch.get("resume"),
            "arguments": list(argv[1:]),
            "runtime_home": launch.get("runtime_home"),
        })
        if schema == "agentctl-session/v3":
            native = document.get("native_session")
            if native is None:
                normalized.update({
                    "session_agent": None,
                    "session_value": None,
                    "goal_session_id": None,
                })
            elif (isinstance(native, dict)
                    and set(native) == {"schema", "agent", "value", "source"}
                    and native.get("schema") == "agentctl-native-session/v1"
                    and native.get("source") in ("observed", "asserted")
                    and native.get("agent") == harness
                    and isinstance(native.get("value"), str)
                    and bool(native.get("value"))):
                normalized.update({
                    "session_agent": native.get("agent"),
                    "session_value": native.get("value"),
                    "goal_session_id": native.get("value"),
                })
            else:
                raise AgentDeliveryError(f"invalid agent record native session: {path}")
            goal = document.get("goal")
            if (not isinstance(goal, dict)
                    or set(goal) != {
                        "schema", "objective", "message_id", "native_command",
                    }
                    or goal.get("schema") != "agentctl-goal/v1"):
                raise AgentDeliveryError(f"invalid agent record goal state: {path}")
            normalized.update({
                "goal": goal.get("objective"),
                "goal_delivery": None,
                "goal_command": goal.get("native_command"),
                "goal_messages": {},
                "goal_message_id": goal.get("message_id"),
            })
        else:
            observed = normalized.get("session_value")
            asserted = normalized.get("goal_session_id")
            session_agent = normalized.get("session_agent")
            if (observed is not None and asserted is not None
                    and observed != asserted):
                raise AgentDeliveryError(
                    f"contradictory native session identities in {path}"
                )
            if (observed is not None
                    and (session_agent != harness
                         or not isinstance(observed, str) or not observed)):
                raise AgentDeliveryError(
                    f"invalid native session identity in {path}"
                )
        return normalized

    def target(self) -> agent.Target:
        """Pin the exact pane and, when available at launch, its durable session."""
        if not self.pane_id:
            raise AgentDeliveryError(f"agent {self.name!r} has no confirmed pane; inspect its launch error")
        return agent.Target(
            pane_id=self.pane_id,
            session_agent=(
                self.session_agent if self._session_source != "asserted" else None
            ),
            session_value=(
                self.session_value if self._session_source != "asserted" else None
            ),
            expected_agent=self.harness,
            expected_cwd=self.cwd,
        )


@dataclass(frozen=True)
class _DeadPaneProof:
    """One exact absent-agent, one-pane-tab, idle-shell observation."""

    info: AgentPaneInfo
    presentation: Pane
    shell: PaneShellProof


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


class _WorkspaceClient:
    """Add exact workspace checks to every queue readiness probe, after enqueue."""

    def __init__(self, client: HerdrClient, record: AgentRecord, *, queue: str | None = None, check_prompt: bool = True) -> None:
        self.client, self.record = client, record
        self.goal_objective: str | None = None
        self.queue = queue
        self.check_prompt = check_prompt
        self.custom_submission: tuple[str, str, int] | None = None

    def pane_info(self, pane_id: str) -> AgentPaneInfo:
        # Sessions started by this manager have a second lifecycle identity in
        # Herdr's named-agent registry.  Adopted sessions deliberately do not:
        # assigning or replacing a native name would mutate a runtime this
        # registry does not own.  Their exact pane/session/workspace/cwd/harness
        # assertions remain the authority instead.
        if (self.record.adapter == "herdr"
                and self.client.agent_pane(self.record.name) != self.record.pane_id):
            raise HerdrUnavailable(f"agent {self.record.name!r} no longer owns its recorded pane")
        info = self.client.pane_info(pane_id)
        if info.workspace_id != self.record.workspace_id:
            raise HerdrUnavailable(f"agent {self.record.name!r} workspace identity changed")
        if pane_id == self.record.pane_id:
            if self.record.goal_session_id is not None and info.session_value is not None and info.session_value != self.record.goal_session_id:
                raise HerdrUnavailable(f"agent {self.record.name!r} native session identity changed")
            if self.check_prompt and info.status in ("idle", "done") and info.agent == "claude":
                screen = self.client.read(pane_id, source="visible", lines=200)
                if ("Quick safety check: Is this a project you created or one you trust?" in screen
                    and "No, exit" in screen and "Yes, I trust this folder" in screen):
                    raise HerdrUnavailable("Claude workspace trust prompt requires human attention; no input was submitted")
            if self.record.adapter == "herdr-pane":
                self.client.verify_custom_harness(
                    pane_id, self.record.harness, self.record.custom_process_identity
                )
                reported_status = info.status
                screen = self.client.read(pane_id, source="visible", lines=200)
                if muse_trust_prompt(screen):
                    raise HerdrUnavailable(
                        "Muse workspace trust prompt requires human attention; no input was submitted"
                    )
                info = AgentPaneInfo(
                    pane_id=info.pane_id,
                    workspace_id=info.workspace_id,
                    cwd=info.cwd,
                    agent=info.agent,
                    status=(
                        "paused"
                        if (reported_status in ("idle", "done")
                            and muse_verified_process_goal_paused(screen))
                        else "idle"
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
        return self.client.panes(self.record.workspace_id)

    def workspace_label(self, workspace_id: str) -> str:
        return self.client.workspace_label(workspace_id)

    def prompt_agent(self, pane_id: str, command: str) -> None:
        self.goal_objective = None
        if self.queue is not None:
            for identifier, objective in self.record.goal_messages.items():
                path = Path(self.queue) / "inflight" / f"{identifier}.json"
                if command == f"/goal {objective}" and os.path.lexists(path):
                    document = agent._read_queue_json(str(path), "goal message", require_private=True)
                    if isinstance(document, dict) and document.get("text") == command:
                        self.goal_objective = objective
                        break
        if self.record.adapter != "herdr-pane":
            self.client.prompt_agent(pane_id, command)
            return
        if "\0" in command or "\x1b" in command:
            raise HerdrUnavailable(
                "Muse pane prompts cannot contain NUL or terminal escape characters"
            )
        info = self.pane_info(pane_id)
        if info.status != "idle":
            raise HerdrUnavailable(
                f"custom pane {pane_id} is not at a verified idle Muse composer"
            )
        before = self.client.read(pane_id, source="visible", lines=200)
        if not muse_auto_review_idle_composer(before):
            raise HerdrUnavailable(
                "Muse input requires the legacy reviewed Auto-review composer; "
                "current YOLO input remains pending until Herdr provides an "
                "effect-coupled process-generation guard"
            )
        self.client.send_text(
            pane_id, f"{_BRACKETED_PASTE_START}{command}{_BRACKETED_PASTE_END}"
        )
        deadline = time.monotonic() + 2.0
        staged = ""
        while time.monotonic() < deadline:
            self.client.verify_custom_harness(
                pane_id, self.record.harness, self.record.custom_process_identity
            )
            staged = self.client.read(pane_id, source="visible", lines=200)
            if staged != before and muse_prompt_is_exact_composer(staged, command):
                break
            time.sleep(min(0.05, max(0.0, deadline - time.monotonic())))
        else:
            raise HerdrUnavailable(
                "literal text insertion did not produce exact visible Muse editor evidence; Enter was not sent"
            )
        self.client.verify_custom_harness(
            pane_id, self.record.harness, self.record.custom_process_identity
        )
        self.client.send_keys(pane_id, "Enter")
        self.custom_submission = (
            staged, command, muse_prompt_transcript_count(staged, command),
        )

    def wait_agent_status(self, pane_id: str, status: str, timeout_ms: int) -> None:
        if self.record.adapter == "herdr-pane" and status == "working":
            if self.custom_submission is None:
                raise HerdrUnavailable(
                    "custom pane harness has no pending submission receipt"
                )
            staged, command, prior_count = self.custom_submission
            deadline = time.monotonic() + timeout_ms / 1000
            while time.monotonic() < deadline:
                self.client.verify_custom_harness(
                    pane_id, self.record.harness, self.record.custom_process_identity
                )
                screen = self.client.read(pane_id, source="visible", lines=200)
                if (screen != staged
                        and muse_prompt_transcript_count(screen, command) > prior_count
                        and not muse_prompt_in_composer(screen, command)):
                    self.custom_submission = None
                    return
                time.sleep(min(0.05, max(0.0, deadline - time.monotonic())))
            raise HerdrUnavailable(
                "Muse did not show a verified post-Enter screen transition"
            )
        if self.goal_objective is None or status != "working":
            self.client.wait_agent_status(pane_id, status, timeout_ms)
            return
        started = time.monotonic()
        try:
            self.client.wait_agent_status(pane_id, status, min(1000, timeout_ms))
            return
        except HerdrUnavailable:
            self.pane_info(pane_id)
            screen = self.client.read(pane_id, source="visible", lines=200)
            if _goal_replacement_selected(screen, self.goal_objective):
                self.client.send_keys(pane_id, "Enter")
            remaining = max(1, timeout_ms - int((time.monotonic() - started) * 1000))
            self.client.wait_agent_status(pane_id, status, remaining)

    def read(self, pane_id: str, *, source: str, lines: int) -> str:
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
    def _pinned_agent_directory(self, name: str) -> Iterator[_PinnedAgentDirectory]:
        """Hold one private agent-directory inode across proof and publication."""
        path = self._directory(name)
        agent._validate_private_directory(str(path), "agent directory")
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

    def _record_bytes(
        self, pinned: _PinnedAgentDirectory, *, require_active_name: bool = True,
    ) -> bytes:
        """Read one bounded record relative to the held directory generation."""
        path = pinned.path / "agent.json"
        if require_active_name:
            self._verify_pinned_agent_directory(pinned)
        flags = (
            os.O_RDONLY
            | getattr(os, "O_CLOEXEC", 0)
            | getattr(os, "O_NOFOLLOW", 0)
            | getattr(os, "O_NONBLOCK", 0)
        )
        descriptor = -1
        try:
            descriptor = os.open("agent.json", flags, dir_fd=pinned.descriptor)
            before = os.fstat(descriptor)
            if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid()
                    or stat.S_IMODE(before.st_mode) & 0o077 or before.st_nlink != 1
                    or before.st_size > _MAX_AGENT_RECORD_BYTES):
                raise AgentDeliveryError(f"unsafe agent record for recovery: {path}")
            content = bytearray()
            while True:
                remaining = _MAX_AGENT_RECORD_BYTES + 1 - len(content)
                if remaining <= 0:
                    raise AgentDeliveryError(
                        f"agent record exceeds {_MAX_AGENT_RECORD_BYTES} bytes: {path}"
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
                raise AgentDeliveryError(f"agent record changed while hashing: {path}")
            if require_active_name:
                self._verify_pinned_agent_directory(pinned)
            return bytes(content)
        except AgentDeliveryError:
            raise
        except OSError as exc:
            raise AgentDeliveryError(f"cannot hash agent record {path}: {exc}") from exc
        finally:
            if descriptor >= 0:
                _close_descriptor(
                    descriptor, "agent record", primary=sys.exc_info()[1],
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
        try:
            document = json.loads(content)
        except (UnicodeError, json.JSONDecodeError) as exc:
            raise AgentDeliveryError(
                f"cannot inspect legacy agent record {path}: {exc}"
            ) from exc
        if not isinstance(document, dict):
            raise AgentDeliveryError(f"invalid legacy agent record: {path}")
        if "foreign_shell_identity" in document:
            raise AgentDeliveryError(
                "--recover-legacy-adoption requires foreign_shell_identity to be absent, "
                "not null or populated"
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
        try:
            document = json.loads(content)
        except (UnicodeError, json.JSONDecodeError) as exc:
            raise AgentDeliveryError(f"cannot inspect agent record {path}: {exc}") from exc
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
        agent._atomic_json(str(self._directory(record.name) / "agent.json"), record.to_document())

    def _queue(self, name: str) -> str:
        return str(self._directory(name) / "queue")

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
            if ((other.session_agent or other.harness) == session_agent
                    and session_value in (other.session_value, other.goal_session_id)):
                return other
        return None

    def start(
        self, name: str, *, cwd: str, workspace_id: str | None = None,
        harness: str = "codex", model: str | None = None, resume: str | None = None,
        reasoning_effort: str | None = None,
        harness_args: Sequence[str] = (), environment: Sequence[str] = (),
        brief: str | None = None,
        startup_timeout: float = 30.0, ready_timeout: float = 900.0,
        working_timeout: float = 30.0, max_attempts: int = 3,
    ) -> dict[str, object]:
        """Create one new tab and start its interactive harness without stealing focus.

        Failed launches retain their record and terminal for diagnosis. Stop the
        named agent after inspecting it to archive its artifacts and release its name.
        """
        _name(name)
        root = str(Path(cwd).expanduser().resolve())
        if not Path(root).is_dir():
            raise AgentDeliveryError(f"cwd is not a directory: {root}")
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
                record = AgentRecord(name, uuid.uuid4().hex, harness, root, time.time(),
                                     model=model, resume=resume, arguments=list(arguments),
                                     adapter="herdr-pane" if harness == "muse" else "herdr")
                self._save(record)
                try:
                    self._create_presentation(record, workspace_id, environment)
                    assert record.pane_id is not None
                    if record.adapter == "herdr-pane":
                        def persist_identity(identity: CustomProcessIdentity) -> None:
                            record.custom_process_identity = identity
                            self._save(record)

                        self.client.start_pane_agent(
                            name, harness, record.pane_id, arguments,
                            timeout=startup_timeout, on_observed=persist_identity,
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
                    if info.session_agent is not None and info.session_value is not None:
                        owner = self._identity_owner(
                            info.session_agent, info.session_value, exclude=name
                        )
                        if owner is not None:
                            try:
                                self.client.close_pane(record.pane_id)
                            except HerdrRunError as close_error:
                                record.session_agent = record.session_value = None
                                raise AgentDeliveryError(
                                    f"native session is already registered as {owner.name!r}; "
                                    f"could not close the conflicting new pane: {close_error}"
                                ) from close_error
                            record.session_agent = record.session_value = None
                            raise AgentDeliveryError(
                                f"native session is already registered as {owner.name!r}; "
                                "closed the conflicting new pane"
                            )
                        try:
                            agent.resolve_target(self.client, record.target())
                        except HerdrRunError as identity_error:
                            record.session_agent = record.session_value = None
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
                        raise
                    if (final_info.session_agent != record.session_agent
                            or final_info.session_value != record.session_value):
                        record.session_agent = record.session_value = None
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
            return self.status(name)

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
                                and (other.session_agent or other.harness)
                                    == info.session_agent
                                and info.session_value in (
                                    other.session_value, other.goal_session_id,
                                ))
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
            name, uuid.uuid4().hex, harness, root, time.time(),
            lifecycle="running", workspace_id=info.workspace_id,
            tab_id=presentation.tab_id, pane_id=info.pane_id,
            session_agent=info.session_agent,
            session_value=info.session_value,
            adapter="herdr-foreign", mode="interactive", backend="herdr",
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
        agent._atomic_json(str(destination / "agent.json"), record.to_document())
        return destination

    def _create_presentation(
        self, record: AgentRecord, workspace_id: str | None,
        environment: Sequence[str],
    ) -> None:
        # Independent registries can share the default workspace. Serialize label
        # resolution and creation host-wide, releasing before any harness startup.
        lock = agent._open_private_lock(agent._target_lock_path("managed-workspace:subagents"), "workspace allocation lock")
        try:
            fcntl.flock(lock, fcntl.LOCK_EX)
            selected = workspace_id or os.environ.get("HERDR_WORKSPACE_ID")
            if selected:
                self.client.workspace_label(selected)
            else:
                selected = self.client.workspace_id_for_label("subagents")
            if selected is None:
                selected, tab, pane = self.client.create_workspace(
                    label="subagents", cwd=record.cwd, environment=environment
                )
                record.workspace_id, record.tab_id, record.pane_id = selected, tab, pane
                self._save(record)
                self.client.rename_tab(tab, record.name)
            else:
                record.workspace_id = selected
                record.tab_id, record.pane_id = self.client.create_tab_with_pane(
                    workspace_id=selected, label=record.name, cwd=record.cwd,
                    environment=environment,
                )
                self._save(record)
        finally:
            os.close(lock)

    def _checked(self, record: AgentRecord, *, ready: bool = False) -> AgentPaneInfo:
        if record.adapter not in ("herdr", "herdr-pane", "herdr-foreign"):
            raise AgentDeliveryError("this operation requires the interactive Herdr adapter")
        client = cast(HerdrClient, _WorkspaceClient(self.client, record, check_prompt=ready))
        info = agent.resolve_target(client, record.target())
        if info.workspace_id != record.workspace_id:
            raise AgentDeliveryError(f"agent {record.name!r} workspace identity changed")
        return info

    def _checked_or_failed_pane_report(self, record: AgentRecord, pane_id: str) -> None:
        """Prove a failed custom launch is still ours or has returned to its shell."""
        if (record.lifecycle in ("starting", "launch_failed")
                and record.adapter == "herdr-pane"
                and record.custom_process_identity is not None):
            info = self.client.pane_info(pane_id)
            if (info.pane_id == pane_id and info.workspace_id == record.workspace_id
                    and os.path.realpath(info.cwd) == os.path.realpath(record.cwd)):
                try:
                    self.client.verify_custom_harness(
                        pane_id, record.harness, record.custom_process_identity
                    )
                    return
                except HerdrUnavailable:
                    pass
        # Preserve the original failed-launch fallback exactly. A stale report plus
        # return to the original shell is sufficient only after agentctl itself
        # reported that pane; an unreported pre-readiness pane remains unowned.
        if (record.lifecycle == "launch_failed" and record.adapter == "herdr-pane"
                and record.pane_reported_by_agentctl):
            info = self.client.pane_info(pane_id)
            if (info.pane_id == pane_id and info.workspace_id == record.workspace_id
                    and os.path.realpath(info.cwd) == os.path.realpath(record.cwd)
                    and info.agent == record.harness
                    and self.client.pane_is_idle_shell(pane_id)):
                return
        if (record.lifecycle == "starting" and record.adapter == "herdr-pane"
                and record.custom_process_identity is None):
            raise HerdrUnavailable(
                f"cannot prove starting custom harness ownership in pane {pane_id}"
            )
        self._checked(record)

    def status(self, name: str) -> dict[str, object]:
        """Report live state or a visible probe error, preserving every durable record."""
        return self._status_record(self._load(name))

    def _status_record(self, record: AgentRecord) -> dict[str, object]:
        """Probe one pinned record without resolving its name a second time."""
        name = record.name
        result: dict[str, object] = record.to_document()
        result["queue"] = self._queue(name)
        result["output"] = str(self._directory(name) / "output.json")
        result["goal_source"] = "requested" if record.goal is not None else None
        result["goal_delivery"] = self._goal_delivery(record)
        try:
            client = cast(
                HerdrClient,
                _WorkspaceClient(self.client, record, check_prompt=False),
            )
            agent.resolve_target(client, record.target())
            result.update(agent.status(client, record.target(), self._queue(name)))
            result["probe_error"] = None
        except HerdrRunError as exc:
            result["agent_status"], result["probe_error"] = "unknown", str(exc)
        return result

    def list(self) -> list[dict[str, object]]:
        """List each row independently; one malformed row cannot hide the fleet."""
        if not self.registry.exists():
            return []
        agent._validate_private_directory(str(self.registry), "agent registry")
        rows: list[dict[str, object]] = []
        for path in sorted(self.registry.iterdir()):
            if not _NAME.fullmatch(path.name) or path.name == "archive":
                continue
            try:
                rows.append(self.status(path.name))
            except HerdrRunError as exc:
                rows.append({
                    "name": path.name,
                    "agent_status": "unknown",
                    "probe_error": str(exc),
                    "record_error": True,
                })
        return rows

    def send(self, name: str, text: str, *, message_id: str | None = None, expected_token: str | None = None, **options: object) -> agent.QueueResult:
        """Serialize against stop, then use the existing durable submission transport."""
        with self._lock(name):
            record = self._load_expected(name, expected_token)
            self._require_automation(record)
            client = cast(HerdrClient, _WorkspaceClient(self.client, record, queue=self._queue(name)))
            return agent.send(client, record.target(), self._queue(name), text, message_id=message_id, **options)

    def drain(self, name: str, **options: object) -> agent.QueueResult:
        """Retry only messages that the shared queue knows were never submitted."""
        with self._lock(name):
            record = self._load(name)
            self._require_automation(record)
            client = cast(HerdrClient, _WorkspaceClient(self.client, record, queue=self._queue(name)))
            return agent.drain(client, record.target(), self._queue(name), **options)  # type: ignore[arg-type]

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
                if info.status in ("idle", "done"):
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
                info = self._checked(record)
                for existing in (record.goal_session_id, record.session_value, info.session_value):
                    if existing is not None and existing != session_id:
                        raise AgentDeliveryError("refusing to replace an already bound native session")
                owner = self._identity_owner(record.harness, session_id, exclude=name)
                if owner is not None:
                    raise AgentDeliveryError(
                        f"native session is already registered as {owner.name!r}"
                    )
                reported: list[str] = []
                for pane in self.client.panes():
                    live = self.client.pane_info(pane.pane_id)
                    if (live.session_agent == record.harness
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
                if (record._nested_storage is not None
                        and record._nested_storage.get("schema")
                        == "agentctl-session/v3"):
                    record.session_agent = record.harness
                    record.session_value = session_id
                    record.goal_session_id = session_id
                    record._session_source = "asserted"
                else:
                    record.goal_session_id = session_id
                if goal_command is not None:
                    record.goal_command = list(goal_command)
                self._save(record)
                return {"name": name, "session_id": session_id, "source": "explicit"}

    def _goal_delivery(self, record: AgentRecord) -> str | None:
        if record.goal_message_id is not None:
            queue = Path(self._queue(record.name))
            filename = f"{record.goal_message_id}.json"
            for folder, outcome in (("processed", "delivered"), ("failed", "possibly_submitted"), ("inflight", "possibly_submitted"), ("inbox", "pending")):
                if os.path.lexists(queue / folder / filename):
                    return outcome
        return record.goal_delivery

    def _goal_result(self, record: AgentRecord, command: Sequence[str] | None) -> dict[str, object]:
        result: dict[str, object] = {"name": record.name, "goal": record.goal,
            "delivery": self._goal_delivery(record), "source": "requested", "native_status": "unverified"}
        if record.harness != "codex":
            return result
        session = record.goal_session_id or record.session_value
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
                    or os.path.realpath(info.cwd) != os.path.realpath(record.cwd)):
                raise AgentDeliveryError(
                    f"refusing to {operation} {record.name!r}: recorded pane, workspace, or cwd changed"
                )
            if info.agent is not None:
                raise AgentDeliveryError(
                    f"refusing to {operation} {record.name!r}: pane still reports agent {info.agent!r}"
                )
            if info.session_agent is not None or info.session_value is not None:
                raise AgentDeliveryError(
                    f"refusing to {operation} {record.name!r}: absent agent has native session identity"
                )
            return presentation, info

        presentation, info = snapshot()
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
        if name not in {"agent.json", "output.json"}:
            raise AgentDeliveryError("unsupported pinned registry artifact name")
        limit = _MAX_AGENT_RECORD_BYTES if name == "agent.json" else _MAX_SNAPSHOT_BYTES
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
        if (record.adapter != "herdr-foreign"
                or record.lifecycle != "running"
                or record.mode != "interactive"
                or record.backend != "herdr"
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
        if (record.adapter != "herdr" or record.lifecycle != "running"
                or record.mode != "interactive" or record.backend != "herdr"):
            raise AgentDeliveryError(
                "managed-dead retirement requires a running herdr record in "
                "interactive/herdr mode"
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
        stopped_bytes = agent._json_text(final.to_document()).encode("utf-8")
        if len(stopped_bytes) > _MAX_AGENT_RECORD_BYTES:
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
        self.client.close_pane(final.pane_id)
        self._atomic_snapshot_bytes(pinned, stopped_bytes, name="agent.json")
        self._publish_pinned_directory(
            pinned, destination, expected_record=stopped_bytes,
        )
        try:
            tab_closed: bool | None = not any(
                pane.tab_id == final.tab_id for pane in self.client.panes()
            )
        except HerdrRunError:
            tab_closed = None
        return {
            "name": record.name,
            "archive": str(destination),
            "pane_closed": True,
            "tab_closed": tab_closed,
            "managed_dead": True,
        }

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
        with self._lock(name):
            record = self._load(name)
            self._require_automation(record)
            record.goal, record.goal_delivery = text, "pending"
            identifier = f"{time.time_ns():020d}-{os.getpid()}"
            record.goal_message_id = identifier
            if record.harness == "codex":
                record.goal_messages[identifier] = text
            self._save(record)
            prompt = f"/goal {text}" if record.harness == "codex" else f"Your ongoing goal: {text}\nWork toward this goal and report completion or blockers."
            try:
                client = cast(HerdrClient, _WorkspaceClient(self.client, record, queue=self._queue(name)))
                result = agent.send(client, record.target(), self._queue(name), prompt, message_id=identifier, **options)
            except HerdrRunError as exc:
                record.goal_delivery = str(getattr(exc, "outcome", "failed"))
                self._save(record)
                raise
            record.goal_delivery = result.outcome
            self._save(record)
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
            record = self._load_expected(name, expected_token)
            confirmed_record = self._load_expected(name, record.token)
            if confirmed_record.to_document() != record.to_document():
                raise AgentDeliveryError(f"agent {name!r} record changed before stop")
            record = confirmed_record
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
            if record.adapter == "herdr-foreign":
                # This registry owns only delivery state.  Revalidate and retain
                # one final snapshot, but never close, rename, signal, or
                # otherwise mutate the adopted runtime.
                def inspect_foreign() -> tuple[str, AgentPaneInfo, Pane]:
                    presentations = [
                        pane for pane in self.client.panes()
                        if pane.pane_id == record.pane_id
                    ]
                    if len(presentations) != 1:
                        raise AgentDeliveryError(
                            f"refusing to unregister adopted agent {name!r}: "
                            f"expected one recorded pane, found {len(presentations)}"
                        )
                    presentation = presentations[0]
                    if presentation.workspace_id != record.workspace_id:
                        raise AgentDeliveryError(
                            f"refusing to unregister adopted agent {name!r}: "
                            "recorded presentation workspace changed"
                        )
                    info = self.client.pane_info(presentation.pane_id)
                    if (info.pane_id != record.pane_id
                            or info.workspace_id != record.workspace_id
                            or os.path.realpath(info.cwd) != os.path.realpath(record.cwd)):
                        raise AgentDeliveryError(
                            f"refusing to unregister adopted agent {name!r}: "
                            "recorded pane, workspace, or cwd changed"
                        )
                    shell_identity = record.foreign_shell_identity
                    if shell_identity is None:
                        raise AgentDeliveryError(
                            f"refusing to unregister adopted agent {name!r}: "
                            "legacy record has no identity-bound pane shell"
                        )
                    if self.client.pane_shell_identity(info.pane_id) != shell_identity:
                        raise AgentDeliveryError(
                            f"refusing to unregister adopted agent {name!r}: "
                            "recorded pane shell generation changed"
                        )
                    if info.agent is None:
                        # A live agent is pinned by its harness and, when one was
                        # reported, native session.  A returned shell has no such
                        # process identity, so its recorded tab remains part of
                        # the fallback proof.  This still lets an operator
                        # unregister a fully revalidated live agent after moving
                        # its pane between tabs in the same workspace.
                        if presentation.tab_id != record.tab_id:
                            raise AgentDeliveryError(
                                f"refusing to unregister adopted agent {name!r}: "
                                "recorded tab changed while the agent was absent"
                            )
                        if info.session_agent is not None or info.session_value is not None:
                            raise AgentDeliveryError(
                                f"refusing pane {info.pane_id}: absent agent has "
                                "native session identity"
                            )
                        if not self.client.pane_is_same_idle_shell(
                            info.pane_id, shell_identity
                        ):
                            raise AgentDeliveryError(
                                f"refusing pane {info.pane_id}: absent agent is not "
                                "at the recorded identity-bound idle shell process group"
                            )
                        return "absent", info, presentation
                    return "live", self._checked(record), presentation

                state, info, presentation = inspect_foreign()
                output = self._bounded_terminal_text(
                    info.pane_id, operation="unregistering"
                )
                try:
                    final_state, final_info, final_presentation = inspect_foreign()
                except (AgentDeliveryError, HerdrRunError) as exc:
                    raise AgentDeliveryError(
                        f"refusing to unregister adopted agent {name!r}: "
                        "runtime identity could not be reverified after output capture"
                    ) from exc
                if (
                    final_state,
                    final_info.pane_id,
                    final_info.workspace_id,
                    os.path.realpath(final_info.cwd),
                    final_info.agent,
                    final_info.session_agent,
                    final_info.session_value,
                    final_presentation.tab_id,
                    final_presentation.workspace_id,
                ) != (
                    state,
                    info.pane_id,
                    info.workspace_id,
                    os.path.realpath(info.cwd),
                    info.agent,
                    info.session_agent,
                    info.session_value,
                    presentation.tab_id,
                    presentation.workspace_id,
                ):
                    raise AgentDeliveryError(
                        f"refusing to unregister adopted agent {name!r}: "
                        "runtime identity changed during output capture"
                    )
                archive, destination = self._archive_destination(record)
                agent._atomic_json(str(self._directory(name) / "output.json"),
                                   {"text": output, "captured_at": time.time(),
                                    "pane_id": record.pane_id})
                try:
                    persisted_state, persisted_info, persisted_presentation = inspect_foreign()
                except (AgentDeliveryError, HerdrRunError) as exc:
                    raise AgentDeliveryError(
                        f"refusing to unregister adopted agent {name!r}: "
                        "runtime identity could not be reverified before archival"
                    ) from exc
                if (
                    persisted_state,
                    persisted_info.pane_id,
                    persisted_info.workspace_id,
                    os.path.realpath(persisted_info.cwd),
                    persisted_info.agent,
                    persisted_info.session_agent,
                    persisted_info.session_value,
                    persisted_presentation.tab_id,
                    persisted_presentation.workspace_id,
                ) != (
                    state,
                    info.pane_id,
                    info.workspace_id,
                    os.path.realpath(info.cwd),
                    info.agent,
                    info.session_agent,
                    info.session_value,
                    presentation.tab_id,
                    presentation.workspace_id,
                ):
                    raise AgentDeliveryError(
                        f"refusing to unregister adopted agent {name!r}: "
                        "runtime identity changed before archival"
                    )
                record.lifecycle = "stopped"
                self._save(record)
                os.rename(self._directory(name), destination)
                agent._fsync_dir(str(archive))
                agent._fsync_dir(str(self.registry))
                return {"name": name, "archive": str(destination),
                        "pane_closed": False, "tab_closed": False,
                        "runtime_preserved": True}
            panes = self.client.panes()
            if (record.adapter == "herdr" and record.lifecycle in ("running", "stopping")
                    and record.pane_id is not None):
                recorded = [pane for pane in panes if pane.pane_id == record.pane_id]
                if len(recorded) == 1:
                    observed = self.client.pane_info(record.pane_id)
                    if observed.agent is None:
                        if record.lifecycle != "running":
                            raise AgentDeliveryError(
                                "managed-dead retirement requires a running herdr record"
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
                        and os.path.realpath(info.cwd) == os.path.realpath(record.cwd)):
                    record.pane_id = owned[0].pane_id
                    self._save(record)
            if any(pane.pane_id == record.pane_id and pane.tab_id != record.tab_id for pane in panes):
                raise AgentDeliveryError("refusing to archive an agent whose pane moved to another tab")
            if owned:
                if len(owned) != 1 or owned[0].pane_id != record.pane_id or owned[0].workspace_id != record.workspace_id:
                    raise AgentDeliveryError("refusing to close a tab whose pane ownership changed")
            archive, destination = self._archive_destination(record)
            if owned:
                if (record.adapter == "herdr-pane" or record.lifecycle == "running"
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
                if (record.adapter == "herdr-pane" or record.lifecycle == "running"
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
            record.lifecycle = "stopped"
            self._save(record)
            os.rename(self._directory(name), destination)
            agent._fsync_dir(str(archive))
            agent._fsync_dir(str(self.registry))
            return {"name": name, "archive": str(destination), "pane_closed": bool(owned), "tab_closed": tab_closed}

    @staticmethod
    def _require_automation(record: AgentRecord) -> None:
        if record.paused:
            raise AgentDeliveryError(f"agent {record.name!r} is paused for human input; resume it before automated submission")

    def pause(self, name: str, *, paused: bool = True) -> dict[str, object]:
        """Pause automated input without interrupting an active harness turn."""
        with self._lock(name):
            record = self._load(name)
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
