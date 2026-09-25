"""One registry and lifecycle authority across native terminals and turn runners."""
from __future__ import annotations

import hashlib
import json
import math
import errno
import os
import subprocess
import sys
import time
import uuid
from collections.abc import Sequence
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import cast

from agentctl import agent
from agentctl.client import HerdrClient, _bounded_control_command
from agentctl.client import CustomProcessIdentity
from agentctl.errors import AgentDeliveryError, HerdrRunError
from agentctl.launch_contract import (
    OUTER_SESSION_CONTROL,
    RuntimeControl,
    RuntimeLaunchContract,
)
from agentctl.jsonx import as_mapping
from agentctl.profiles import (
    reasoning_arguments,
    validate_muse_headless_arguments,
    validate_structured_harness_argument_conflicts,
)
from agentctl.subagents import (
    AgentRecord,
    LaunchSpec,
    ManagedAgents,
    TerminalState,
    _MAX_TERMINAL_RETIREMENT_BYTES,
    _PinnedAgentDirectory,
    _TERMINAL_RETIREMENT_FILE,
    _name,
)

_WORKER_RPC_SCHEMA = "agentctl-worker-rpc/v2"
_RUNNER_LIVENESS_SCHEMA = "agentctl-runner-liveness/v1"
# Decode-only compatibility for terminal receipts written before the shared
# retirement protocol became the sole current writer.
_STOP_RESULT_SCHEMA = "agentctl-session-stop/v1"
_STOP_RESULT_FILE = "stop-result.json"
_MAX_STOP_RESULT_BYTES = 1 << 20
_MAX_WORKER_REQUEST_BYTES = 1 << 20
_MAX_WORKER_RESPONSE_BYTES = 8 << 20
_MAX_WORKER_STDERR_BYTES = 64 << 10


def _headless_permission_mode(harness: str) -> str:
    """Resolve the one explicit permission mode stored with a new headless launch."""
    if harness != "codex":
        return "native"
    raw = os.environ.get("SUBAGENTS_CODEX_BYPASS_PERMISSIONS", "0")
    if raw not in ("0", "1"):
        raise AgentDeliveryError(
            "SUBAGENTS_CODEX_BYPASS_PERMISSIONS must be exactly 0 "
            "(native permissions) or 1 (bypass approvals and sandbox)"
        )
    return "bypass" if raw == "1" else "native"


def _runtime_launch_contract(record: AgentRecord) -> RuntimeLaunchContract:
    """Project the canonical outer LaunchSpec into the worker boundary schema."""
    return RuntimeLaunchContract.create(
        cwd=record.launch.cwd,
        harness=record.launch.harness,
        model=record.launch.model,
        backend=record.launch.backend,
        mode=record.launch.mode,
        harness_args=record.arguments,
        permission_mode=record.launch.permission_mode or "native",
        runtime_home=record.launch.runtime_home or "",
    )


def _runner_identity(value: object) -> CustomProcessIdentity | None:
    """Parse one complete worker process identity; absence proves no prior generation."""
    if not isinstance(value, dict) or set(value) != {
        "version", "boot_id", "pid", "starttime_ticks",
        "executable_device", "executable_inode",
    }:
        return None
    try:
        identity = CustomProcessIdentity(**value)
    except (TypeError, ValueError):
        return None
    if (identity.version != 1 or identity.pid <= 0
            or identity.starttime_ticks <= 0
            or identity.executable_device <= 0 or identity.executable_inode <= 0):
        return None
    return identity


class WorkerRpcError(AgentDeliveryError):
    """Typed failure at the private worker process boundary."""

    def __init__(self, kind: str, message: str, *, remote_code: str | None = None) -> None:
        super().__init__(message)
        self.kind = kind
        self.remote_code = remote_code


@dataclass(frozen=True)
class WorkerRuntimeEvidence:
    """One token-bound observation from the canonical inner runtime registry."""

    backend: str
    mode: str
    target: str
    session_id: str | None
    pane_id: str | None
    runner_identity: CustomProcessIdentity | None

    @classmethod
    def parse(
        cls, record: AgentRecord, response: dict[str, object],
    ) -> WorkerRuntimeEvidence:
        runtime = as_mapping(response.get("record"), "runtime record")
        try:
            control = RuntimeControl.from_document(runtime.get("control"))
        except ValueError as exc:
            raise AgentDeliveryError(
                f"worker runtime returned invalid control authority: {exc}"
            ) from exc
        if control.kind != OUTER_SESSION_CONTROL or control.generation != record.token:
            raise AgentDeliveryError(
                "worker runtime belongs to another session generation"
            )
        expected_launch = _runtime_launch_contract(record)
        try:
            observed_launch = RuntimeLaunchContract.from_document(
                runtime.get("launch")
            )
        except ValueError as exc:
            raise AgentDeliveryError(
                f"worker runtime returned an invalid owner launch: {exc}"
            ) from exc
        if runtime.get("name") != record.name or observed_launch != expected_launch:
            raise AgentDeliveryError(
                "worker runtime launch receipt disagrees with canonical launch intent"
            )
        backend = runtime.get("backend")
        mode = runtime.get("mode")
        target = runtime.get("tmux_target")
        session = runtime.get("session_id")
        pane = runtime.get("presentation_pane")
        if backend not in ("herdr", "tmux") or mode not in ("headless", "tui"):
            raise AgentDeliveryError("worker returned an invalid runtime route")
        if not isinstance(target, str) or not target:
            raise AgentDeliveryError("worker returned an invalid presentation target")
        if session is not None and (not isinstance(session, str) or not session):
            raise AgentDeliveryError("worker returned an invalid native session identity")
        if pane is not None and (not isinstance(pane, str) or not pane):
            raise AgentDeliveryError("worker returned an invalid presentation pane")
        runner_pid = runtime.get("runner_pid")
        runner_started_at = runtime.get("runner_started_at")
        identity = _runner_identity(runtime.get("runner_identity"))
        if identity is not None:
            if (runner_pid not in (None, identity.pid)
                    or runner_started_at not in (
                        None, str(identity.starttime_ticks),
                    )):
                raise AgentDeliveryError(
                    "worker returned contradictory runner identity fields"
                )
        elif runner_pid is not None or runner_started_at is not None:
            raise AgentDeliveryError("worker returned an invalid runner identity")
        if mode == "tui":
            if (backend != "herdr" or record.launch.harness != "codex"
                    or pane is None or identity is not None):
                raise AgentDeliveryError("worker returned an inconsistent TUI route")
        elif pane is not None:
            raise AgentDeliveryError("headless worker returned a TUI presentation pane")
        return cls(backend, mode, target, session, pane, identity)

    def to_document(self) -> dict[str, object]:
        return {
            "schema": "agentctl-worker-runtime-evidence/v1",
            "backend": self.backend,
            "mode": self.mode,
            "target": self.target,
            "session_id": self.session_id,
            "pane_id": self.pane_id,
            "runner_identity": (
                asdict(self.runner_identity)
                if self.runner_identity is not None else None
            ),
        }


class Sessions(ManagedAgents):
    """Route all public operations through one generation-locked session record."""

    def __init__(self, client: HerdrClient | None = None, registry: str | Path = ".agentctl") -> None:
        super().__init__(client or HerdrClient(), registry)

    def _worker(
        self, record: AgentRecord, action: str, **options: object,
    ) -> dict[str, object]:
        if record.launch.runtime_home is None:
            raise AgentDeliveryError("turn-runner session has no runtime directory")
        environment = dict(os.environ)
        environment["HERDR_SUBAGENTS_HOME"] = record.launch.runtime_home
        owner_launch = _runtime_launch_contract(record)
        command = [sys.executable, str(Path(__file__).with_name("worker_rpc.py"))]
        # The adapter process may be interrupted after committing a request. Keep
        # its canonical record and report uncertainty rather than retrying it.
        raw_deadline = options.pop("_deadline", None)
        if raw_deadline is not None and not isinstance(raw_deadline, float):
            raise WorkerRpcError("deadline", "runtime probe deadline is invalid")
        deadline = raw_deadline
        if action == "start":
            brief = options.pop("brief", None)
            if options:
                raise AgentDeliveryError(
                    "turn-runner start options must derive from the canonical launch specification"
                )
            options = {"brief": brief}
        timeout = 900.0 if action == "migrate" else 90.0
        if deadline is not None:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise WorkerRpcError("deadline", f"runtime {action} probe deadline expired")
            timeout = min(timeout, remaining)
        request = json.dumps({
            "schema": _WORKER_RPC_SCHEMA,
            "action": action,
            "name": record.name,
            "owner_token": record.token,
            "desired_paused": record.paused,
            "owner_launch": owner_launch.to_document(),
            **options,
        }, separators=(",", ":"))
        if len(request.encode("utf-8")) > _MAX_WORKER_REQUEST_BYTES:
            raise WorkerRpcError("request-too-large", "runtime request exceeds its byte limit")
        try:
            completed = _bounded_control_command(
                command,
                input_text=request,
                environ=environment,
                timeout=timeout,
                stdout_limit=_MAX_WORKER_RESPONSE_BYTES,
                stderr_limit=_MAX_WORKER_STDERR_BYTES,
                strict_utf8=True,
            )
        except subprocess.TimeoutExpired as exc:
            raise WorkerRpcError(
                "timeout", f"runtime {action} timed out; inspect session state before retrying",
            ) from exc
        except UnicodeError as exc:
            raise WorkerRpcError(
                "invalid-receipt", f"runtime {action} returned non-UTF-8 diagnostics",
            ) from exc
        except OSError as exc:
            if exc.errno == errno.EFBIG:
                raise WorkerRpcError(
                    "invalid-receipt",
                    f"runtime {action} exceeded its bounded diagnostic channel",
                ) from exc
            raise WorkerRpcError("transport", f"runtime {action} transport failed: {exc}") from exc
        try:
            envelope = as_mapping(
                agent._decode_json_bytes(
                    completed.stdout.encode("utf-8"), "runtime response", "<worker stdout>",
                ),
                "runtime response",
            )
        except (AgentDeliveryError, ValueError, TypeError) as exc:
            raise WorkerRpcError(
                "invalid-receipt",
                f"runtime {action} returned no valid receipt: {completed.stderr.strip()}",
            ) from exc
        if (envelope.get("schema") != _WORKER_RPC_SCHEMA
                or envelope.get("action") != action
                or envelope.get("owner_token") != record.token
                or not isinstance(envelope.get("ok"), bool)):
            raise WorkerRpcError(
                "invalid-receipt", f"runtime {action} returned an invalid typed envelope",
            )
        if completed.returncode:
            try:
                if set(envelope) != {
                    "schema", "action", "owner_token", "ok", "error",
                }:
                    raise TypeError("invalid typed runtime error fields")
                error = as_mapping(envelope.get("error"), "runtime error")
                if set(error) != {"code", "message"}:
                    raise TypeError("invalid typed runtime error payload")
                code = error.get("code")
                message = error.get("message")
                if (envelope.get("ok") is not False or not isinstance(code, str) or not code
                        or not isinstance(message, str) or not message):
                    raise TypeError("invalid typed runtime error")
            except TypeError as exc:
                raise WorkerRpcError(
                    "invalid-receipt", f"runtime {action} returned an invalid error receipt",
                ) from exc
            raise WorkerRpcError("runtime-error", message, remote_code=code)
        if envelope.get("ok") is not True:
            raise WorkerRpcError(
                "invalid-receipt", f"runtime {action} returned failure with exit status zero",
            )
        if set(envelope) != {
            "schema", "action", "owner_token", "ok", "payload",
        }:
            raise WorkerRpcError(
                "invalid-receipt", f"runtime {action} returned invalid success fields",
            )
        try:
            return as_mapping(envelope.get("payload"), "runtime payload")
        except TypeError as exc:
            raise WorkerRpcError(
                "invalid-receipt", f"runtime {action} returned no typed payload",
            ) from exc

    @staticmethod
    def _capabilities(
        record: AgentRecord, runtime: WorkerRuntimeEvidence | None = None,
    ) -> list[str]:
        result = ["send", "status", "read", "wait", "stop", "attach", "pause", "resume"]
        mode = runtime.mode if runtime is not None else record.launch.mode
        if mode == "headless":
            result.extend(("final-answer", "reset", "migrate", "repair"))
        else:
            result.append("terminal-snapshot")
        if record.launch.adapter in ("herdr", "herdr-pane", "herdr-foreign"):
            result.extend(("drain", "goal", "bind-session", "relocate"))
        if record.launch.adapter == "herdr-pane" and record.launch.harness == "muse":
            result.append("reconcile-delivery")
        return result

    def start_session(self, name: str, *, cwd: str, mode: str = "interactive",
                      backend: str = "herdr", harness: str = "codex", model: str | None = None,
                      reasoning_effort: str | None = None,
                      launch_profile: str | None = None,
                      resume: str | None = None, harness_args: Sequence[str] = (),
                      environment: Sequence[str] = (), brief: str | None = None,
                      workspace_id: str | None = None,
                      workspace_label: str | None = None,
                      startup_timeout: float = 30.0,
                      ready_timeout: float = 900.0, working_timeout: float = 30.0,
                      max_attempts: int = 3) -> dict[str, object]:
        """Create a native interactive terminal or a persistent headless runner."""
        _name(name)
        if mode == "interactive":
            if backend != "herdr":
                raise AgentDeliveryError("interactive sessions require Herdr; tmux supports headless runners")
            return super().start(
                name, cwd=cwd, harness=harness, model=model,
                reasoning_effort=reasoning_effort, resume=resume,
                launch_profile=launch_profile,
                harness_args=harness_args, environment=environment, brief=brief,
                workspace_id=workspace_id, workspace_label=workspace_label,
                startup_timeout=startup_timeout,
                ready_timeout=ready_timeout, working_timeout=working_timeout,
                max_attempts=max_attempts,
            )
        if mode != "headless" or backend not in ("herdr", "tmux") or harness not in ("codex", "agy", "muse"):
            raise AgentDeliveryError("headless sessions support codex/agy/muse with herdr/tmux")
        if (resume is not None or workspace_id is not None
                or workspace_label is not None or environment):
            raise AgentDeliveryError(
                "headless start does not accept resume, workspace selection, or environment"
            )
        if isinstance(harness_args, (str, bytes)) or any(
            not isinstance(item, str) or not item or "\0" in item
            for item in harness_args
        ):
            raise AgentDeliveryError("headless harness arguments must be nonempty and contain no NUL")
        if harness != "muse" and harness_args:
            raise AgentDeliveryError("headless harness arguments currently apply only to Muse")
        if harness != "muse" and reasoning_effort is not None:
            raise AgentDeliveryError(
                f"headless {harness} does not support reasoning_effort"
            )
        if harness == "muse":
            validate_structured_harness_argument_conflicts(
                harness, list(harness_args), label="Muse headless launch",
                structured_model=model is not None,
                structured_effort=reasoning_effort is not None,
            )
        arguments = (*reasoning_arguments(harness, reasoning_effort), *harness_args)
        if harness == "muse":
            validate_muse_headless_arguments(list(arguments))
        root = str(Path(cwd).expanduser().resolve())
        if not Path(root).is_dir():
            raise AgentDeliveryError(f"cwd is not a directory: {root}")
        with self._lock(name):
            with self._identity_transaction():
                directory = self._directory(name)
                if os.path.lexists(directory):
                    raise AgentDeliveryError(f"agent {name!r} already registered; stop it before reusing the name")
                directory.mkdir(mode=0o700)
                record = AgentRecord(
                    name, uuid.uuid4().hex,
                    LaunchSpec(
                        harness=harness, cwd=root, adapter="turn-runner",
                        mode=mode, backend=backend, model=model, resume=None,
                        profile=launch_profile, argv=(harness, *arguments),
                        environment_names=(), runtime_home=str(directory / "runtime"),
                        runtime_ownership="owned", executable=None,
                        permission_mode=_headless_permission_mode(harness),
                    ),
                    time.time(),
                )
                self._save(record)
                try:
                    response = self._worker(record, "start", brief=brief)
                    self._sync_worker_record(record, response, save=False)
                    if record.session_value is not None:
                        owner = self._identity_owner(
                            record.session_agent or record.launch.harness,
                            record.session_value, exclude=name,
                        )
                        if owner is not None:
                            try:
                                self._worker(record, "stop")
                            except HerdrRunError as stop_error:
                                detail = f"; could not stop the conflicting new runtime: {stop_error}"
                            else:
                                detail = "; stopped the conflicting new runtime"
                            record.session_agent = record.session_value = None
                            record.session_source = None
                            raise AgentDeliveryError(
                                f"native session is already registered as {owner.name!r}{detail}"
                            )
                    record.lifecycle = "running"
                    self._save(record)
                except WorkerRpcError as exc:
                    # A transport timeout or malformed/lost receipt says
                    # nothing about whether the token-bound runtime committed
                    # startup. Keep the durable intent reconcilable.
                    record.lifecycle = (
                        "launch_failed" if exc.kind == "runtime-error" else "starting"
                    )
                    record.error = str(exc)
                    self._save(record)
                    raise
                except (HerdrRunError, OSError, ValueError) as exc:
                    record.lifecycle, record.error = "launch_failed", str(exc)
                    self._save(record)
                    raise
            return self._status_record(record)

    def _sync_worker_record(
        self, record: AgentRecord, response: dict[str, object], *, save: bool = True,
        promote_running: bool = False,
    ) -> WorkerRuntimeEvidence:
        evidence = WorkerRuntimeEvidence.parse(record, response)

        def apply(target: AgentRecord) -> None:
            # Publish only compatibility caches derived from the typed inner
            # authority. LaunchSpec remains immutable startup intent.
            target.session_value = evidence.session_id
            target.session_agent = (
                target.launch.harness if evidence.session_id is not None else None
            )
            target.session_source = (
                "observed" if evidence.session_id is not None else None
            )
            target.pane_id = evidence.pane_id
            if evidence.mode == "tui":
                target.set_runner_identity(None)
            elif evidence.runner_identity is not None:
                target.set_runner_identity(evidence.runner_identity)
            if promote_running:
                target.lifecycle = "running"
                target.error = None

        if not save:
            apply(record)
            return evidence
        with self._identity_transaction():
            current = self._load_expected(record.name, record.token)
            if current.to_storage_document() != record.to_storage_document():
                raise AgentDeliveryError(
                    "session record changed before worker evidence commit"
                )
            if evidence.session_id is not None:
                owner = self._identity_owner(
                    record.launch.harness, evidence.session_id,
                    exclude=record.name,
                )
                if owner is not None:
                    raise AgentDeliveryError(
                        f"native session is already registered as {owner.name!r}"
                    )
            apply(current)
            self._save(current)
            apply(record)
        return evidence

    @staticmethod
    def _runner_observation(
        record: AgentRecord, response: dict[str, object],
        evidence: WorkerRuntimeEvidence | None = None,
    ) -> tuple[tuple[CustomProcessIdentity, bool | None] | None, str | None]:
        """Parse one internally consistent worker identity and liveness observation."""
        try:
            evidence = evidence or WorkerRuntimeEvidence.parse(record, response)
            if evidence.mode != "headless" or evidence.runner_identity is None:
                raise TypeError("worker runtime has no headless runner identity")
            result = as_mapping(response.get("result"), "worker status result")
            agents = result.get("agents")
            if (not isinstance(agents, list) or len(agents) != 1
                    or not isinstance(agents[0], dict)):
                raise TypeError("worker status result must contain exactly one agent")
            row = as_mapping(agents[0], "worker status row")
            if row.get("name") != record.name:
                raise TypeError("worker status identity has a different name")
            row_identity = _runner_identity(row.get("runner_identity"))
            if row_identity is None or evidence.runner_identity != row_identity:
                raise TypeError("worker status identities disagree")
            runner_alive = row.get("runner_alive")
            if runner_alive is not None and not isinstance(runner_alive, bool):
                raise TypeError("worker status row has invalid runner_alive evidence")
        except TypeError as exc:
            return None, str(exc)
        return (evidence.runner_identity, runner_alive), None

    @classmethod
    def _runner_liveness(
        cls, record: AgentRecord, response: dict[str, object],
    ) -> tuple[dict[str, object] | None, str | None]:
        """Bind one typed liveness observation to the saved runner generation."""
        if record.runner_pid is None or record.runner_started_at is None:
            return None, "saved runner identity is unavailable"
        if record.runner_identity is None:
            return None, "saved runner identity is not boot/image bound"
        observation, error = cls._runner_observation(record, response)
        if observation is None:
            return None, error
        runner_identity, runner_alive = observation
        if runner_identity != record.runner_identity:
            return None, "worker status identity does not match the saved runner"
        if runner_alive is None:
            return None, "worker status could not determine exact runner liveness"
        return {
            "schema": _RUNNER_LIVENESS_SCHEMA,
            "name": record.name,
            "runner_pid": runner_identity.pid,
            "runner_started_at": str(runner_identity.starttime_ticks),
            "runner_identity": asdict(runner_identity),
            "runner_alive": runner_alive,
        }, None

    def status(self, name: str) -> dict[str, object]:
        """Distinguish saved lifecycle, runtime liveness, and supported operations."""
        with self._lock(name):
            return self._status_record(self._load(name))

    def _status_record(
        self, record: AgentRecord, *, deadline: float | None = None,
    ) -> dict[str, object]:
        if record.launch.adapter in ("herdr", "herdr-pane", "herdr-foreign"):
            result = super()._status_record(record, deadline=deadline)
        else:
            result = record.to_document()
            evidence: WorkerRuntimeEvidence | None = None
            try:
                response = self._worker(record, "status", _deadline=deadline)
                # Parse first, but do not let an untrusted/dead replacement
                # receipt overwrite the saved generation before liveness is
                # compared with that generation.
                evidence = WorkerRuntimeEvidence.parse(record, response)
                observation, observation_error = self._runner_observation(
                    record, response, evidence,
                )
                liveness, liveness_error = self._runner_liveness(record, response)
                expected_runtime_home = str(self._directory(record.name) / "runtime")
                first_runner_observation = (
                    record.lifecycle in ("starting", "running")
                    and record.runner_identity is None
                    and observation is not None
                )
                live_runtime_observation = (
                    record.lifecycle == "running"
                    and observation is not None
                    and observation[1] is True
                )
                current_tui = (
                    record.lifecycle == "running"
                    and evidence.mode == "tui"
                )
                if ((first_runner_observation
                        or live_runtime_observation or current_tui)
                        and record.launch.runtime_ownership == "owned"
                        and record.launch.runtime_home == expected_runtime_home):
                    self._sync_worker_record(
                        record, response,
                        promote_running=(
                            record.lifecycle == "starting"
                            and observation is not None
                            and observation[1] is True
                        ),
                    )
                    if evidence.mode == "headless":
                        liveness, liveness_error = self._runner_liveness(
                            record, response,
                        )
                result = record.to_document()
                result["runtime"] = response.get("result")
                result["runtime_evidence"] = evidence.to_document()
                if liveness_error is None and observation_error is not None:
                    liveness_error = observation_error
                result["runtime_liveness"] = liveness
                result["runtime_liveness_error"] = liveness_error
                result["probe_error"] = None
                result["probe_error_kind"] = None
                result["probe_error_code"] = None
            except WorkerRpcError as exc:
                result.update(
                    probe_error=str(exc), agent_status="unknown",
                    probe_error_kind=exc.kind, probe_error_code=exc.remote_code,
                    runtime_liveness=None, runtime_liveness_error=None,
                )
            except (HerdrRunError, OSError, ValueError) as exc:
                result.update(
                    probe_error=str(exc), agent_status="unknown",
                    probe_error_kind="untyped", probe_error_code=None,
                    runtime_liveness=None, runtime_liveness_error=None,
                )
        result["capabilities"] = self._capabilities(
            record, evidence if record.launch.adapter == "turn-runner" else None,
        )
        return result

    def _classify_health(
        self, record: AgentRecord, status: dict[str, object], *,
        deadline: float | None = None,
    ) -> tuple[str, str, str]:
        if record.launch.adapter != "turn-runner":
            return super()._classify_health(record, status, deadline=deadline)
        if record.lifecycle != "running":
            return (
                "unhealthy",
                "lifecycle-not-running",
                f"saved lifecycle is {record.lifecycle!r}, expected 'running'",
            )
        probe_error = status.get("probe_error")
        if probe_error is not None:
            kind = status.get("probe_error_kind")
            return "unknown", "runtime-probe-failed", f"{kind or 'untyped'}: {probe_error}"
        route = status.get("runtime_evidence")
        if isinstance(route, dict) and route.get("mode") == "tui":
            runtime = status.get("runtime")
            agents = runtime.get("agents") if isinstance(runtime, dict) else None
            row = agents[0] if isinstance(agents, list) and len(agents) == 1 else None
            if (not isinstance(row, dict) or row.get("name") != record.name
                    or row.get("backend") != route.get("backend")
                    or row.get("mode") != "tui"
                    or row.get("presentation_pane") != route.get("pane_id")):
                return "unknown", "runtime-evidence-inconsistent", (
                    "worker TUI status disagrees with its token-bound route"
                )
            if row.get("runner_alive") is False or row.get("window_alive") is False:
                return "unhealthy", "runtime-not-live", "worker TUI is not live"
            if row.get("runner_alive") is True and row.get("window_alive") is True:
                return "healthy", "ok", "worker TUI presentation is live"
            return "unknown", "runtime-liveness-unconfirmed", (
                "worker TUI status did not confirm process and pane liveness"
            )
        evidence = status.get("runtime_liveness")
        if (isinstance(evidence, dict)
                and evidence.get("schema") == _RUNNER_LIVENESS_SCHEMA
                and evidence.get("name") == record.name
                and evidence.get("runner_pid") == record.runner_pid
                and evidence.get("runner_started_at") == record.runner_started_at):
            if evidence.get("runner_alive") is False:
                return (
                    "unhealthy",
                    "runner-not-live",
                    f"saved runner pid {record.runner_pid} start {record.runner_started_at} "
                    "is explicitly not alive",
                )
            if evidence.get("runner_alive") is True:
                return "healthy", "ok", "saved worker runner identity is live"
        reason = status.get("runtime_liveness_error")
        return (
            "unknown", "runtime-liveness-unconfirmed",
            str(reason or "worker status did not confirm runner liveness"),
        )

    def send_session(self, name: str, text: str, *, message_id: str | None = None,
                     model: str | None = None, **options: object) -> dict[str, object]:
        """Submit to the selected adapter without reusing another generation's name."""
        record = self._load(name)
        if record.launch.adapter in ("herdr", "herdr-pane", "herdr-foreign"):
            if model is not None:
                raise AgentDeliveryError("per-turn model overrides require a headless session")
            return asdict(super().send(name, text, message_id=message_id, expected_token=record.token, **options))
        if message_id is not None:
            raise AgentDeliveryError("headless requests use monotonically numbered turns; message-id is interactive-only")
        with self._lock(name):
            current = self._load(name)
            if current.token != record.token:
                raise AgentDeliveryError("session was replaced before submission")
            self._require_automation(current)
            return self._worker(current, "send", text=text, model=model)

    def read_session(self, name: str, *, lines: int = 500, output: str = "tail",
                     since_turn: int | None = None) -> str:
        """Read a terminal snapshot or an explicitly selected transcript boundary."""
        if since_turn is not None and output != "since_turn":
            raise AgentDeliveryError("since-turn requires --output since_turn")
        if output == "since_turn" and since_turn is None:
            raise AgentDeliveryError("--output since_turn requires since-turn")
        with self._lock(name):
            record = self._load(name)
            if record.launch.adapter in ("herdr", "herdr-pane", "herdr-foreign"):
                if output not in ("tail", "all") or since_turn is not None:
                    raise AgentDeliveryError("interactive terminals expose snapshots, not final-answer boundaries")
                self._checked(record)
                text = agent.read(self.client, record.target(), lines=lines)
                agent._atomic_json(str(self._directory(name) / "output.json"),
                    {"text": text, "captured_at": time.time(), "pane_id": record.pane_id})
                return text
            response = self._worker(record, "read", mode=output,
                since_turn=since_turn, tail=lines if output == "tail" else None)
            result = as_mapping(response["result"], "read result")
            return str(result.get("text", result.get("output", "")))

    def stop(
        self, name: str, *, expected_token: str | None = None,
        recover_legacy_adoption: bool = False,
        expected_record_sha256: str | None = None,
    ) -> dict[str, object]:
        """Retire a runtime, then archive its canonical identity and artifacts."""
        try:
            record = self._load_expected(name, expected_token)
        except AgentDeliveryError:
            # A caller that retained the exact generation token can reconcile a
            # response lost after the outer archive publication. Never infer a
            # generation merely from an old archive with the same agent name.
            if expected_token is None or os.path.lexists(self._directory(name)):
                raise
            with self._lock(name):
                if os.path.lexists(self._directory(name)):
                    record = self._load_expected(name, expected_token)
                else:
                    return self._completed_stop(name, expected_token)
        if record.launch.adapter in ("herdr", "herdr-pane", "herdr-foreign"):
            return self._stop(
                name, expected_token=record.token,
                recover_legacy_adoption=recover_legacy_adoption,
                expected_record_sha256=expected_record_sha256,
                expected_token_explicit=expected_token is not None,
            )
        if recover_legacy_adoption or expected_record_sha256 is not None:
            raise AgentDeliveryError(
                "legacy adoption recovery is supported only for interactive herdr-foreign records"
            )
        with self._lock(name):
            if not os.path.lexists(self._directory(name)):
                if expected_token is None:
                    raise AgentDeliveryError(
                        f"agent {name!r} disappeared before stop"
                    )
                return self._completed_stop(name, expected_token)
            current = self._load(name)
            if current.token != record.token:
                raise AgentDeliveryError("session was replaced before stop")
            archive, destination = self._archive_destination(current)
            with self._pinned_agent_directory(name) as pinned:
                current_snapshot = self._managed_record_snapshot(
                    pinned, expected_token=current.token,
                )
                if current_snapshot.record.launch.adapter != "turn-runner":
                    raise AgentDeliveryError(
                        "session adapter changed before token-bound stop"
                    )
                current = current_snapshot.record
                if current.lifecycle == "stopped":
                    receipt = self._read_terminal_retirement(
                        pinned, current, current_snapshot.content,
                    )
                    if receipt is None:
                        receipt = self._read_legacy_terminal_receipt(
                            pinned, current, current_snapshot.content, destination,
                        )
                        if receipt is not None:
                            current.terminal = receipt
                            current._legacy_terminal_authority = False
                            migrated_bytes = agent._json_text(
                                current.to_storage_document()
                            ).encode("utf-8")
                            self._atomic_snapshot_bytes(
                                pinned, migrated_bytes, name="agent.json",
                            )
                            self._atomic_snapshot_bytes(
                                pinned,
                                agent._json_text(
                                    self._terminal_retirement_document(migrated_bytes)
                                ).encode("utf-8"),
                                name=_TERMINAL_RETIREMENT_FILE,
                            )
                    if receipt is not None:
                        return self._complete_terminal_publication(
                            current, pinned=pinned, expected_token=current.token,
                        )
                current.lifecycle = "stopping"
                current.error = None
                self._atomic_snapshot_bytes(
                    pinned,
                    agent._json_text(current.to_storage_document()).encode("utf-8"),
                    name="agent.json",
                )
                try:
                    result = self._worker(current, "stop")
                except WorkerRpcError as exc:
                    # The inner process may have committed the token-bound stop
                    # before its receipt was lost. Keep the durable intent so the
                    # next exact-generation stop can reconcile its terminal archive.
                    current.error = str(exc)
                    self._atomic_snapshot_bytes(
                        pinned,
                        agent._json_text(current.to_storage_document()).encode("utf-8"),
                        name="agent.json",
                    )
                    raise
                runtime = _relocate_receipt_paths(
                    result, self._directory(name), destination,
                )
                evidence = self._turn_runner_retirement_evidence(
                    current, destination, runtime,
                )
                current.lifecycle = "stopped"
                current.error = None
                current.terminal = TerminalState.create(
                    "turn-runner-stopped", evidence,
                )
                current._legacy_terminal_authority = False
                stopped_bytes = agent._json_text(
                    current.to_storage_document()
                ).encode("utf-8")
                self._atomic_snapshot_bytes(
                    pinned, stopped_bytes, name="agent.json",
                )
                receipt = self._terminal_retirement_document(stopped_bytes)
                receipt_bytes = agent._json_text(receipt).encode("utf-8")
                if len(receipt_bytes) > _MAX_TERMINAL_RETIREMENT_BYTES:
                    raise AgentDeliveryError(
                        "terminal retirement receipt exceeds "
                        f"{_MAX_TERMINAL_RETIREMENT_BYTES} bytes"
                    )
                self._atomic_snapshot_bytes(
                    pinned, receipt_bytes, name=_TERMINAL_RETIREMENT_FILE,
                )
                self._publish_pinned_directory(
                    pinned, destination, expected_record=stopped_bytes,
                )
                return self._terminal_retirement_result(
                    current, destination,
                    cast(TerminalState, current.terminal),
                )

    def _completed_stop(self, name: str, expected_token: str) -> dict[str, object]:
        """Read a current terminal receipt, or a decode-only legacy receipt."""
        current = self._terminal_archive_receipt(name, expected_token)
        if current is not None:
            return current
        return self._legacy_completed_stop(name, expected_token)

    def _legacy_completed_stop(
        self, name: str, expected_token: str,
    ) -> dict[str, object]:
        """Decode one pre-shared-protocol turn-runner stop receipt."""
        _name(name)
        if not expected_token or any(
            character not in "0123456789abcdefghijklmnopqrstuvwxyz-"
            for character in expected_token
        ) or len(expected_token) > 80:
            raise AgentDeliveryError("expected token has an invalid shape")
        archive = self.registry / "archive"
        destination = archive / f"{name}-{expected_token}"
        agent._validate_private_directory(str(archive), "agent archive")
        with self._pinned_agent_directory(
            name, path=destination, label="archived agent generation",
        ) as pinned:
            snapshot = self._managed_record_snapshot(
                pinned, expected_token=expected_token,
            )
            record = snapshot.record
            if record.lifecycle != "stopped" or record.launch.adapter != "turn-runner":
                raise AgentDeliveryError(
                    "archived stop receipt does not match the requested runtime generation"
                )
            receipt = self._read_legacy_terminal_receipt(
                pinned, record, snapshot.content, destination,
            )
            if receipt is None:
                raise AgentDeliveryError(
                    "archived stop receipt does not match the requested runtime generation"
                )
            self._verify_pinned_agent_directory(pinned)
        return self._terminal_retirement_result(record, destination, receipt)

    def _read_legacy_terminal_receipt(
        self, pinned: _PinnedAgentDirectory, record: AgentRecord, record_bytes: bytes,
        destination: Path,
    ) -> TerminalState | None:
        """Decode a pre-shared-protocol stop result into the canonical outcome."""
        if not self._retirement_artifact_exists(
            pinned, _STOP_RESULT_FILE, purpose="legacy session stop result",
        ):
            return None
        receipt_bytes = self._pinned_artifact_bytes(
            pinned, name=_STOP_RESULT_FILE, limit=_MAX_STOP_RESULT_BYTES,
            purpose="legacy session stop result",
        )
        try:
            value = json.loads(
                receipt_bytes.decode("utf-8"),
                object_pairs_hook=agent._reject_duplicate_json_keys,
                parse_constant=agent._reject_json_constant,
            )
            agent._validate_json_depth(value)
        except (UnicodeError, json.JSONDecodeError, ValueError, RecursionError) as exc:
            raise AgentDeliveryError(
                f"session stop result is invalid: {exc}"
            ) from exc
        if not isinstance(value, dict) or set(value) != {
            "schema", "name", "token", "record_sha256", "result",
        }:
            raise AgentDeliveryError("session stop result has an invalid shape")
        if (value.get("schema") != _STOP_RESULT_SCHEMA
                or value.get("name") != record.name
                or value.get("token") != record.token
                or value.get("record_sha256") != hashlib.sha256(record_bytes).hexdigest()):
            raise AgentDeliveryError(
                "session stop result does not match the archived generation"
            )
        result = as_mapping(value.get("result"), "session stop result payload")
        if result.get("name") != record.name or result.get("archive") != str(destination):
            raise AgentDeliveryError("session stop result payload is inconsistent")
        runtime = result.get("runtime")
        evidence = self._turn_runner_retirement_evidence(
            record, destination, runtime,
        )
        terminal = TerminalState.create(
            "turn-runner-stopped", evidence,
        )
        # Reuse the current strict result validator before migration/publication.
        self._terminal_retirement_result(record, destination, terminal)
        return terminal

    def pause(self, name: str, *, paused: bool = True) -> dict[str, object]:
        """Hand off input after in-flight work; preserve the running conversation."""
        with self._lock(name):
            record = self._load(name)
            if record.launch.adapter == "turn-runner":
                # The outer generation owns desired state. Persist intent first;
                # a lost RPC response is reconciled by the next worker call.
                record.paused = paused
                self._save(record)
                self._worker(record, "pause" if paused else "resume")
            else:
                self._checked(record)
                record.paused = paused
                self._save(record)
            return {"name": name, "token": record.token, "paused": paused}

    def runtime_operation(self, name: str, action: str, **options: object) -> dict[str, object]:
        """Run a headless capability under the same canonical lifecycle lock."""
        with self._lock(name):
            record = self._load(name)
            if record.launch.adapter != "turn-runner":
                raise AgentDeliveryError(f"{action} requires a persistent turn-runner session")
            self._require_automation(record)
            result = self._worker(record, action, **options)
            self._sync_worker_record(record, result)
            return result

    def wait(self, name: str, *, timeout: float = 900.0, **options: object) -> dict[str, object]:
        """Wait for readiness of one pinned generation, without claiming goal completion."""
        if not math.isfinite(timeout) or not 0 <= timeout <= 31_536_000:
            raise AgentDeliveryError("wait timeout must be finite and between 0 and 31536000 seconds")
        initial = self._load(name)
        if initial.launch.adapter in ("herdr", "herdr-pane", "herdr-foreign"):
            return super().wait(name, timeout=timeout, expected_token=initial.token, **options)  # type: ignore[arg-type]
        deadline = time.monotonic() + timeout
        while True:
            with self._lock(name):
                record = self._load_expected(name, initial.token)
                result = self._status_record(record)
                runtime = result.get("runtime")
                if isinstance(runtime, dict):
                    values = runtime.get("agents")
                    if isinstance(values, list) and len(values) == 1 and isinstance(values[0], dict):
                        status = values[0].get("status")
                        runner_alive = values[0].get("runner_alive")
                        if runner_alive is False:
                            raise AgentDeliveryError("worker runner is not alive; inspect status before restarting")
                        if runner_alive is not True:
                            raise AgentDeliveryError("cannot confirm worker runner liveness; inspect status")
                        if status == "idle" and values[0].get("pending", 0) == 0:
                            return result
                        if status in ("dead", "stopped", "blocked", "error"):
                            raise AgentDeliveryError(f"worker requires attention: {status}")
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise AgentDeliveryError("timed out waiting for worker readiness")
            time.sleep(min(0.25, remaining))

    def attach(self, name: str, *, expected_token: str | None = None) -> dict[str, object]:
        """Focus a verified Herdr tab or attach the current terminal to an exact tmux window."""
        initial = self._load_expected(name, expected_token)
        if initial.launch.adapter in ("herdr", "herdr-pane", "herdr-foreign"):
            return super().attach(name, expected_token=initial.token)
        attach_command: list[str] | None = None
        with self._lock(name):
            record = self._load_expected(name, initial.token)
            response = self._worker(record, "status")
            runtime = self._sync_worker_record(record, response)
            target = runtime.target
            if runtime.backend == "herdr":
                if runtime.mode == "tui":
                    pane = runtime.pane_id
                    if pane is None:
                        raise AgentDeliveryError(
                            "worker has no confirmed Herdr pane; use repair"
                        )
                    panes = [
                        entry for entry in self.client.panes()
                        if entry.pane_id == pane
                    ]
                    if len(panes) != 1 or panes[0].tab_id != target:
                        raise AgentDeliveryError(
                            "worker presentation pane no longer belongs to its "
                            "recorded tab; use repair"
                        )
                self.client.focus_tab(target)
                focused = "tab"
            else:
                if not target or ":" not in target:
                    raise AgentDeliveryError("worker has no confirmed tmux target")
                session, window = target.split(":", 1)
                exact = "=" + session + ":=" + window
                completed = _bounded_control_command(["tmux", "display-message", "-p", "-t", exact, "#{window_id}"])
                window_id = completed.stdout.strip()
                if completed.returncode or not window_id.startswith("@") or not window_id[1:].isdigit():
                    raise AgentDeliveryError("worker tmux window is missing or ambiguous; use repair")
                # Resolve to a server-assigned window ID before focusing or attaching.
                # IDs are not reused while the server lives, unlike user-supplied names.
                if os.environ.get("TMUX"):
                    completed = _bounded_control_command(["tmux", "select-window", "-t", window_id])
                    if completed.returncode:
                        raise AgentDeliveryError(completed.stderr.strip())
                else:
                    if not sys.stdin.isatty() or not sys.stdout.isatty():
                        raise AgentDeliveryError("tmux attach requires an interactive terminal; inspect with agentctl read")
                    attach_command = ["tmux", "attach-session", "-t", window_id]
                focused = "window"
            result: dict[str, object] = {"name": name, "backend": runtime.backend,
                "target": target, "focused": focused, "paused": record.paused}
        # User attachment can last indefinitely. It must not monopolize lifecycle
        # ownership and prevent another controller from stopping or messaging it.
        if attach_command is not None:
            completed_attach = subprocess.run(attach_command, check=False)
            if completed_attach.returncode:
                raise AgentDeliveryError(f"tmux attachment failed with exit {completed_attach.returncode}")
        return result


def _relocate_receipt_paths(value: object, source: Path, destination: Path) -> object:
    """Keep path receipts truthful when the containing canonical directory moves."""
    if isinstance(value, dict):
        result: dict[str, object] = {}
        for key, item in value.items():
            if not isinstance(key, str):
                continue
            if key in ("archived_to", "state_path", "runtime_home") and isinstance(item, str):
                try:
                    relative = Path(item).relative_to(source)
                except ValueError:
                    result[key] = item
                else:
                    result[key] = str(destination / relative)
            else:
                result[key] = _relocate_receipt_paths(item, source, destination)
        return result
    if isinstance(value, list):
        return [_relocate_receipt_paths(item, source, destination) for item in value]
    return value
