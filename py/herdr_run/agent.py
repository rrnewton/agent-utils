"""Durable, serialized messaging for interactive agents hosted by Herdr.

Target identity is supplied by adapters.  This module owns every transport
property: durable FIFO files, idle/done readiness, atomic multiline submission,
working-state confirmation, bounded retries/quarantine, status, and reading.
"""

from __future__ import annotations

import fcntl
import json
import os
import tempfile
import time
from collections.abc import Callable
from dataclasses import dataclass

from herdr_run.client import AgentPaneInfo, HerdrClient
from herdr_run.errors import AgentDeliveryError, HerdrUnavailable

__all__ = ["Target", "QueueResult", "resolve_target", "enqueue", "drain", "send", "status", "read"]


@dataclass(frozen=True)
class Target:
    pane_id: str | None = None
    session_agent: str | None = None
    session_value: str | None = None
    expected_agent: str | None = None
    expected_workspace: str | None = None
    expected_cwd: str | None = None


@dataclass(frozen=True)
class QueueResult:
    message_id: str
    delivered: tuple[str, ...]
    quarantined: tuple[str, ...]
    pending: tuple[str, ...]


def _real(path: str) -> str:
    return os.path.realpath(os.path.abspath(path))


def _validate(client: HerdrClient, info: AgentPaneInfo, target: Target) -> None:
    failures: list[str] = []
    if target.expected_agent is not None and info.agent != target.expected_agent:
        failures.append(f"agent is {info.agent!r}, expected {target.expected_agent!r}")
    if target.session_agent is not None and info.session_agent != target.session_agent:
        failures.append(f"session agent is {info.session_agent!r}, expected {target.session_agent!r}")
    if target.session_value is not None and info.session_value != target.session_value:
        failures.append(f"session is {info.session_value!r}, expected {target.session_value!r}")
    if target.expected_workspace is not None:
        label = client.workspace_label(info.workspace_id)
        if label != target.expected_workspace:
            failures.append(f"workspace is {label!r}, expected {target.expected_workspace!r}")
    if target.expected_cwd is not None and _real(info.cwd) != _real(target.expected_cwd):
        failures.append(f"cwd is {info.cwd!r}, expected {_real(target.expected_cwd)!r}")
    if failures:
        raise AgentDeliveryError(f"refusing pane {info.pane_id}: " + "; ".join(failures))


def resolve_target(client: HerdrClient, target: Target) -> AgentPaneInfo:
    """Resolve by stable session when supplied, then revalidate every asserted field."""
    pane_id = target.pane_id
    if target.session_value is not None:
        matches: list[str] = []
        for pane in client.panes():
            info = client.pane_info(pane.pane_id)
            if info.session_value == target.session_value and (
                target.session_agent is None or info.session_agent == target.session_agent
            ):
                matches.append(pane.pane_id)
        if len(matches) != 1:
            raise AgentDeliveryError(
                f"expected exactly one live pane for session {target.session_value!r}, found {len(matches)}"
            )
        pane_id = matches[0]
    if pane_id is None:
        raise AgentDeliveryError("target needs --pane or a stable session value")
    info = client.pane_info(pane_id)
    _validate(client, info, target)
    return info


def _dirs(root: str) -> tuple[str, str, str]:
    return tuple(os.path.join(root, name) for name in ("inbox", "processed", "failed"))  # type: ignore[return-value]


def _prepare(root: str) -> tuple[str, str, str]:
    paths = _dirs(root)
    for path in paths:
        os.makedirs(path, mode=0o700, exist_ok=True)
    return paths


def _atomic_json(path: str, document: dict[str, object]) -> None:
    parent = os.path.dirname(path)
    temporary = ""
    try:
        with tempfile.NamedTemporaryFile("w", encoding="utf-8", dir=parent, prefix=".message.", delete=False) as handle:
            temporary = handle.name
            os.fchmod(handle.fileno(), 0o600)
            json.dump(document, handle, indent=2, sort_keys=True)
            handle.write("\n")
        os.replace(temporary, path)
    finally:
        if temporary:
            try:
                os.unlink(temporary)
            except FileNotFoundError:
                pass


def enqueue(root: str, text: str, *, message_id: str | None = None) -> str:
    """Persist a prompt before any readiness or transport operation."""
    if not text:
        raise AgentDeliveryError("message must not be empty")
    inbox, _processed, _failed = _prepare(root)
    identifier = message_id or f"{time.time_ns():020d}-{os.getpid()}"
    path = os.path.join(inbox, f"{identifier}.json")
    if os.path.exists(path):
        raise AgentDeliveryError(f"message id already exists: {identifier}")
    _atomic_json(path, {"id": identifier, "text": text, "queued_at": time.time(), "delivery_attempts": 0})
    return identifier


def _load(path: str) -> dict[str, object]:
    try:
        with open(path, encoding="utf-8") as handle:
            raw: object = json.load(handle)
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise AgentDeliveryError(f"cannot read queued message {path}: {exc}") from exc
    if not isinstance(raw, dict) or not isinstance(raw.get("text"), str):
        raise AgentDeliveryError(f"queued message {path} has no string text field")
    return {str(key): value for key, value in raw.items()}


def _wait_ready(
    client: HerdrClient,
    target: Target,
    timeout: float,
    *,
    sleep: Callable[[float], None],
    monotonic: Callable[[], float],
) -> AgentPaneInfo:
    deadline = monotonic() + timeout
    while True:
        info = resolve_target(client, target)
        if info.status in ("idle", "done"):
            return info
        if info.status == "blocked":
            raise AgentDeliveryError(f"pane {info.pane_id} is blocked; resolve its visible prompt")
        if monotonic() >= deadline:
            raise AgentDeliveryError(
                f"pane {info.pane_id} did not become idle/done within {timeout:g}s; last status={info.status}"
            )
        sleep(min(0.25, max(0.0, deadline - monotonic())))


def _deliver_one(
    client: HerdrClient,
    target: Target,
    text: str,
    *,
    ready_timeout: float,
    working_timeout: float,
    sleep: Callable[[float], None],
    monotonic: Callable[[], float],
) -> None:
    info = _wait_ready(client, target, ready_timeout, sleep=sleep, monotonic=monotonic)
    client.run(info.pane_id, text)
    try:
        client.wait_agent_status(info.pane_id, "working", max(1, int(working_timeout * 1000)))
    except HerdrUnavailable as exc:
        raise AgentDeliveryError(
            f"pane {info.pane_id} did not confirm idle/done -> working submission: {exc}"
        ) from exc


def drain(
    client: HerdrClient,
    target: Target,
    root: str,
    *,
    ready_timeout: float = 900.0,
    working_timeout: float = 30.0,
    max_attempts: int = 3,
    sleep: Callable[[float], None] = time.sleep,
    monotonic: Callable[[], float] = time.monotonic,
) -> QueueResult:
    """Serialize and drain a FIFO; poison prompts are retained in ``failed``."""
    inbox, processed, failed = _prepare(root)
    lock_path = os.path.join(root, ".delivery.lock")
    delivered: list[str] = []
    quarantined: list[str] = []
    descriptor = os.open(lock_path, os.O_CREAT | os.O_RDWR, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        for path in sorted(os.path.join(inbox, name) for name in os.listdir(inbox) if name.endswith(".json")):
            document = _load(path)
            identifier = str(document.get("id", os.path.basename(path)[:-5]))
            attempts_raw = document.get("delivery_attempts", document.get("tui_delivery_attempts", 0))
            attempts = attempts_raw if isinstance(attempts_raw, int) and attempts_raw >= 0 else 0
            while attempts < max_attempts:
                try:
                    _deliver_one(
                        client, target, str(document["text"]), ready_timeout=ready_timeout,
                        working_timeout=working_timeout, sleep=sleep, monotonic=monotonic,
                    )
                except (AgentDeliveryError, HerdrUnavailable) as exc:
                    attempts += 1
                    document["delivery_attempts"] = attempts
                    document["tui_delivery_attempts"] = attempts
                    document["delivery_error"] = str(exc)
                    document["delivery_failed_at"] = time.time()
                    _atomic_json(path, document)
                    if attempts >= max_attempts:
                        os.replace(path, os.path.join(failed, os.path.basename(path)))
                        quarantined.append(identifier)
                        break
                else:
                    os.replace(path, os.path.join(processed, os.path.basename(path)))
                    delivered.append(identifier)
                    break
    finally:
        os.close(descriptor)
    pending = tuple(name[:-5] for name in sorted(os.listdir(inbox)) if name.endswith(".json"))
    return QueueResult("", tuple(delivered), tuple(quarantined), pending)


def send(client: HerdrClient, target: Target, root: str, text: str, **kwargs: object) -> QueueResult:
    identifier = enqueue(root, text)
    result = drain(client, target, root, **kwargs)  # type: ignore[arg-type]
    if identifier in result.quarantined:
        failed_path = os.path.join(root, "failed", f"{identifier}.json")
        detail = "unknown delivery failure"
        try:
            failed_document = _load(failed_path)
            recorded = failed_document.get("delivery_error")
            if isinstance(recorded, str):
                detail = recorded
        except AgentDeliveryError:
            pass
        raise AgentDeliveryError(
            f"message {identifier} failed after bounded retries: {detail}; "
            f"it is retained under {root}/failed"
        )
    return QueueResult(identifier, result.delivered, result.quarantined, result.pending)


def status(client: HerdrClient, target: Target, root: str) -> dict[str, object]:
    info = resolve_target(client, target)
    inbox, _processed, failed = _prepare(root)
    return {
        "pane_id": info.pane_id, "agent": info.agent, "agent_status": info.status,
        "session_agent": info.session_agent, "session_value": info.session_value,
        "workspace_id": info.workspace_id, "cwd": info.cwd,
        "pending": sorted(name[:-5] for name in os.listdir(inbox) if name.endswith(".json")),
        "failed": sorted(name[:-5] for name in os.listdir(failed) if name.endswith(".json")),
    }


def read(client: HerdrClient, target: Target, *, lines: int = 500) -> str:
    info = resolve_target(client, target)
    return client.read(info.pane_id, source="recent-unwrapped", lines=lines)
