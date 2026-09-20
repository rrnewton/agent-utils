"""Named, long-lived interactive agents sharing a Herdr workspace.

Herdr owns the terminal and harness process; this module owns durable names,
launch intent, queue routing, output snapshots, and conservative tab teardown.
It never closes a workspace, silently restarts a conversation, or changes the
harness's permission settings. Coordinators and humans see the same terminal.
"""
from __future__ import annotations

import fcntl
import math
import os
import re
import time
import uuid
from collections.abc import Callable, Iterator, Sequence
from contextlib import contextmanager
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import cast

from agentctl import agent
from agentctl.client import AgentPaneInfo, HerdrClient, Pane
from agentctl.errors import AgentDeliveryError, HerdrRunError, HerdrUnavailable

_NAME = re.compile(r"[a-z][a-z0-9-]{0,31}\Z")
_KIND = re.compile(r"[a-z][a-z0-9-]{0,63}\Z")
_ENVIRONMENT_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z")


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
    elif model is not None or resume is not None:
        raise AgentDeliveryError("model and resume presets support codex/claude; use harness arguments for other kinds")
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
    _unknown: dict[str, object] = field(default_factory=dict, init=False, repr=False)

    def to_document(self) -> dict[str, object]:
        """Preserve unrecognized fields alongside the known schema, never in a wrapper."""
        document = asdict(self)
        document.pop("_unknown")
        if self._unknown.keys() & document.keys():
            raise AgentDeliveryError("unknown agent metadata conflicts with a known schema field")
        document.update(self._unknown)
        return document

    @classmethod
    def load(cls, path: Path, name: str) -> AgentRecord:
        """Reject malformed or non-private state before using any recorded identity."""
        value = agent._read_queue_json(str(path), "agent record", require_private=True)
        if not isinstance(value, dict):
            raise AgentDeliveryError(f"invalid agent record: {path}")
        document = cast(dict[str, object], value)
        for key in ("name", "token", "harness", "cwd", "lifecycle"):
            if not isinstance(document.get(key), str) or not document[key]:
                raise AgentDeliveryError(f"invalid agent record field {key}: {path}")
        for key in ("workspace_id", "tab_id", "pane_id", "session_agent", "session_value", "model", "resume", "error", "goal", "goal_delivery", "goal_session_id", "goal_message_id"):
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
        known = {key for key, field_info in cls.__dataclass_fields__.items() if field_info.init}
        fields = {key: document[key] for key in known if key in document}
        if document.get("adapter", "herdr") not in ("herdr", "herdr-foreign", "turn-runner"):
            raise AgentDeliveryError(f"unsupported runtime adapter in {path}")
        if document.get("mode", "interactive") not in ("interactive", "headless"):
            raise AgentDeliveryError(f"invalid execution mode in {path}")
        if document.get("backend", "herdr") not in ("herdr", "tmux"):
            raise AgentDeliveryError(f"invalid terminal backend in {path}")
        if not isinstance(document.get("paused", False), bool):
            raise AgentDeliveryError(f"invalid pause state in {path}")
        home = document.get("runtime_home")
        if home is not None and (not isinstance(home, str) or not Path(home).is_absolute()):
            raise AgentDeliveryError(f"invalid runtime directory in {path}")
        record = cls(**fields)  # type: ignore[arg-type]
        record._unknown = {key: value for key, value in document.items() if key not in known}
        return record

    def target(self) -> agent.Target:
        """Pin the exact pane and, when available at launch, its durable session."""
        if not self.pane_id:
            raise AgentDeliveryError(f"agent {self.name!r} has no confirmed pane; inspect its launch error")
        return agent.Target(
            pane_id=self.pane_id, session_agent=self.session_agent,
            session_value=self.session_value, expected_agent=self.harness,
            expected_cwd=self.cwd,
        )


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
        self.client.prompt_agent(pane_id, command)

    def wait_agent_status(self, pane_id: str, status: str, timeout_ms: int) -> None:
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
        arguments = harness_arguments(harness, model=model, resume=resume, extra=harness_args)
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
                                     model=model, resume=resume, arguments=list(arguments))
                self._save(record)
                try:
                    self._create_presentation(record, workspace_id, environment)
                    assert record.pane_id is not None
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
        directory.mkdir(mode=0o700)
        agent._fsync_dir(str(self.registry))
        record = AgentRecord(
            name, uuid.uuid4().hex, harness, root, time.time(),
            lifecycle="running", workspace_id=info.workspace_id,
            tab_id=presentation.tab_id, pane_id=info.pane_id,
            session_agent=info.session_agent,
            session_value=info.session_value,
            adapter="herdr-foreign", mode="interactive", backend="herdr",
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
        if record.adapter not in ("herdr", "herdr-foreign"):
            raise AgentDeliveryError("this operation requires the interactive Herdr adapter")
        client = cast(HerdrClient, _WorkspaceClient(self.client, record, check_prompt=ready))
        info = agent.resolve_target(client, record.target())
        if info.workspace_id != record.workspace_id:
            raise AgentDeliveryError(f"agent {record.name!r} workspace identity changed")
        return info

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
            self._checked(record)
            result.update(agent.status(self.client, record.target(), self._queue(name)))
            result["probe_error"] = None
        except HerdrRunError as exc:
            result["agent_status"], result["probe_error"] = "unknown", str(exc)
        return result

    def list(self) -> list[dict[str, object]]:
        """List every registered agent; unavailable Herdr is not evidence of death."""
        if not self.registry.exists():
            return []
        agent._validate_private_directory(str(self.registry), "agent registry")
        return [self.status(path.name) for path in sorted(self.registry.iterdir())
                if _NAME.fullmatch(path.name) and path.name != "archive"]

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

    def stop(self, name: str, *, expected_token: str | None = None) -> dict[str, object]:
        """Close only the owned single-pane tab and archive the complete record/queue.

        A confirmed missing pane permits archival. A probe failure, changed agent
        identity, or extra human-created panes refuses teardown and keeps state.
        """
        with self._lock(name):
            record = self._load_expected(name, expected_token)
            if record.adapter == "herdr-foreign":
                # This registry owns only delivery state.  Revalidate and retain
                # one final snapshot, but never close, rename, signal, or
                # otherwise mutate the adopted runtime.
                info = self._checked(record)
                try:
                    output = self.client.read(info.pane_id, source="recent-unwrapped", lines=5000)
                    if not output:
                        output = self.client.read(info.pane_id, source="recent", lines=5000)
                    agent._atomic_json(str(self._directory(name) / "output.json"),
                                       {"text": output, "captured_at": time.time(),
                                        "pane_id": record.pane_id})
                except HerdrRunError as exc:
                    raise AgentDeliveryError(
                        f"cannot preserve terminal output before unregistering: {exc}"
                    ) from exc
                self._checked(record)
                record.lifecycle = "stopped"
                self._save(record)
                archive = self.registry / "archive"
                archive.mkdir(mode=0o700, exist_ok=True)
                agent._validate_private_directory(str(archive), "agent archive")
                destination = archive / f"{name}-{record.token}"
                os.rename(self._directory(name), destination)
                agent._fsync_dir(str(archive))
                agent._fsync_dir(str(self.registry))
                return {"name": name, "archive": str(destination),
                        "pane_closed": False, "tab_closed": False,
                        "runtime_preserved": True}
            panes = self.client.panes()
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
                if record.lifecycle == "running" or self.client.pane_info(owned[0].pane_id).agent is not None:
                    self._checked(record)
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
                if record.lifecycle == "running" or self.client.pane_info(owned[0].pane_id).agent is not None:
                    self._checked(record)
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
            archive = self.registry / "archive"
            archive.mkdir(mode=0o700, exist_ok=True)
            agent._validate_private_directory(str(archive), "agent archive")
            destination = archive / f"{name}-{record.token}"
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
