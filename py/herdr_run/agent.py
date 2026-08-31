"""Durable, serialized messaging for interactive agents hosted by Herdr.

Target identity is supplied by adapters.  This module owns every transport
property: durable FIFO files, idle/done readiness, atomic multiline submission,
working-state confirmation, at-most-once ambiguity quarantine, status, and reading.
"""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
import re
import stat
import tempfile
import time
from collections.abc import Callable
from dataclasses import dataclass

from herdr_run.client import AgentPaneInfo, HerdrClient
from herdr_run.errors import (
    AgentDeliveryError,
    AgentPending,
    AgentPossiblySubmitted,
    HerdrUnavailable,
)

__all__ = ["Target", "QueueResult", "resolve_target", "enqueue", "drain", "send", "status", "read"]


@dataclass(frozen=True)
class Target:
    """Identity assertions for one already-running interactive Herdr agent."""

    pane_id: str | None = None
    session_agent: str | None = None
    session_value: str | None = None
    expected_agent: str | None = None
    expected_workspace: str | None = None
    expected_cwd: str | None = None


@dataclass(frozen=True)
class QueueResult:
    """Structured outcome of one durable queue send or drain operation."""

    message_id: str
    delivered: tuple[str, ...]
    quarantined: tuple[str, ...]
    pending: tuple[str, ...]
    blocked: str | None = None
    outcome: str = "delivered"


class _PossiblySubmitted(AgentDeliveryError):
    """The atomic pane injection happened, but its working transition was not observed."""


_MESSAGE_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,254}\Z")
_U64_MAX = (1 << 64) - 1


def _reject_json_constant(value: str) -> None:
    raise ValueError(f"non-standard JSON constant {value!r}")


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
    if not (target.pane_id or target.session_value):
        raise AgentDeliveryError("target needs --pane or a stable session value")
    asserted_pane_id = target.pane_id
    pane_id = asserted_pane_id
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
        if asserted_pane_id is not None and asserted_pane_id != pane_id:
            raise AgentDeliveryError(
                f"refusing session target pane {pane_id!r}: expected exact pane {asserted_pane_id!r}"
            )
    if pane_id is None:
        raise AgentDeliveryError("target needs --pane or a stable session value")
    info = client.pane_info(pane_id)
    _validate(client, info, target)
    return info


def _fsync_dir(path: str) -> None:
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _dirs(root: str) -> tuple[str, str, str, str]:
    return tuple(os.path.join(root, name) for name in ("inbox", "inflight", "processed", "failed"))  # type: ignore[return-value]


def _validate_private_directory(path: str, purpose: str, *, tighten: bool = False) -> None:
    """Require queue-owned directories to be real, private, and owned by this account."""

    try:
        metadata = os.stat(path, follow_symlinks=False)
    except OSError as exc:
        raise AgentDeliveryError(f"cannot inspect {purpose} {path}: {exc}") from exc
    if not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != os.getuid():
        raise AgentDeliveryError(f"unsafe {purpose}: {path}")
    if stat.S_IMODE(metadata.st_mode) & 0o077:
        if not tighten:
            raise AgentDeliveryError(f"{purpose} is not private: {path}")
        try:
            os.chmod(path, 0o700, follow_symlinks=False)
        except OSError as exc:
            raise AgentDeliveryError(f"cannot make {purpose} private {path}: {exc}") from exc


def _open_private_lock(path: str, purpose: str) -> int:
    """Open a same-user private lock without following a planted symlink."""

    flags = os.O_CREAT | os.O_RDWR | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = -1
    keep_open = False
    try:
        descriptor = os.open(path, flags, 0o600)
        metadata = os.fstat(descriptor)
        if (
            not stat.S_ISREG(metadata.st_mode)
            or metadata.st_uid != os.getuid()
            or stat.S_IMODE(metadata.st_mode) & 0o077
        ):
            raise AgentDeliveryError(f"unsafe {purpose}: {path}")
        keep_open = True
        return descriptor
    except OSError as exc:
        raise AgentDeliveryError(f"cannot open {purpose} {path}: {exc}") from exc
    finally:
        if descriptor >= 0 and not keep_open:
            os.close(descriptor)


def _read_queue_json(path: str, purpose: str, *, require_private: bool) -> object:
    """Read one owned regular queue artifact without following a symlink."""

    flags = (
        os.O_RDONLY
        | getattr(os, "O_CLOEXEC", 0)
        | getattr(os, "O_NOFOLLOW", 0)
        | getattr(os, "O_NONBLOCK", 0)
    )
    descriptor = -1
    try:
        descriptor = os.open(path, flags)
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid():
            raise AgentDeliveryError(f"unsafe {purpose}: {path}")
        if require_private and stat.S_IMODE(metadata.st_mode) & 0o077:
            raise AgentDeliveryError(f"{purpose} is not private: {path}")
        with os.fdopen(descriptor, encoding="utf-8") as handle:
            descriptor = -1
            return json.load(handle, parse_constant=_reject_json_constant)
    except (OSError, UnicodeError, json.JSONDecodeError, ValueError) as exc:
        raise AgentDeliveryError(f"cannot read {purpose} {path}: {exc}") from exc
    finally:
        if descriptor >= 0:
            os.close(descriptor)


def _prepare(root: str) -> tuple[str, str, str, str]:
    root_existed = os.path.lexists(root)
    os.makedirs(root, mode=0o700, exist_ok=True)
    _validate_private_directory(root, "queue directory", tighten=True)
    if not root_existed:
        _fsync_dir(os.path.dirname(os.path.abspath(root)))
    paths = _dirs(root)
    for path in paths:
        os.makedirs(path, mode=0o700, exist_ok=True)
        _validate_private_directory(path, "queue state directory", tighten=True)
    _fsync_dir(root)
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
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
        _fsync_dir(parent)
    finally:
        if temporary:
            try:
                os.unlink(temporary)
            except FileNotFoundError:
                pass


def _atomic_json_create(path: str, document: dict[str, object]) -> None:
    """Durably create one JSON artifact without replacing an existing name."""

    parent = os.path.dirname(path)
    temporary = ""
    try:
        with tempfile.NamedTemporaryFile(
            "w", encoding="utf-8", dir=parent, prefix=".message.", delete=False
        ) as handle:
            temporary = handle.name
            os.fchmod(handle.fileno(), 0o600)
            json.dump(document, handle, indent=2, sort_keys=True, allow_nan=False)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.link(temporary, path, follow_symlinks=False)
        os.unlink(temporary)
        temporary = ""
        _fsync_dir(parent)
    finally:
        if temporary:
            try:
                os.unlink(temporary)
            except FileNotFoundError:
                pass


def enqueue(root: str, text: str, *, message_id: str | None = None) -> str:
    """Persist a prompt before any readiness or transport operation."""
    return _enqueue(root, text, message_id=message_id, serialize=True)


def _enqueue(
    root: str,
    text: str,
    *,
    message_id: str | None,
    serialize: bool,
) -> str:
    if not text:
        raise AgentDeliveryError("message must not be empty")
    inbox, inflight, processed, failed = _prepare(root)
    identifier = f"{time.time_ns():020d}-{os.getpid()}" if message_id is None else message_id
    if _MESSAGE_ID.fullmatch(identifier) is None:
        raise AgentDeliveryError(
            "message id must be 1-255 ASCII letters, digits, dots, underscores, or hyphens "
            "and must start with a letter or digit"
        )
    filename = f"{identifier}.json"
    path = os.path.join(inbox, filename)
    descriptor = -1
    try:
        if serialize:
            lock_path = os.path.join(root, ".delivery.lock")
            descriptor = _open_private_lock(lock_path, "queue delivery lock")
            fcntl.flock(descriptor, fcntl.LOCK_EX)
        if any(os.path.lexists(os.path.join(directory, filename)) for directory in (inbox, inflight, processed, failed)):
            raise AgentDeliveryError(f"message id already exists: {identifier}")
        try:
            _atomic_json_create(
                path,
                {
                    "id": identifier,
                    "text": text,
                    "queued_at": time.time(),
                    "delivery_attempts": 0,
                },
            )
        except FileExistsError as exc:
            raise AgentDeliveryError(f"message id already exists: {identifier}") from exc
    finally:
        if descriptor >= 0:
            os.close(descriptor)
    return identifier


def _binding(target: Target) -> dict[str, object]:
    """Identity authority for a queue: stable session when present, otherwise exact pane."""
    identity: dict[str, object]
    if target.session_value is not None:
        identity = {"kind": "session", "agent": target.session_agent, "value": target.session_value}
    else:
        identity = {"kind": "pane", "pane_id": target.pane_id}
    identity.update(
        {
            "expected_agent": target.expected_agent,
            "expected_workspace": target.expected_workspace,
            "expected_cwd": None if target.expected_cwd is None else _real(target.expected_cwd),
        }
    )
    return identity


def _target_lock_path(pane_id: str) -> str:
    """Return the fixed host-wide lock path for one resolved live pane."""
    identity: dict[str, object] = {"kind": "pane", "pane_id": pane_id}
    # ``ensure_ascii=False`` pins the command's UTF-8 lock encoding for every resolved pane id.
    encoded = json.dumps(identity, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    digest = hashlib.sha256(encoded).hexdigest()
    lock_root = os.path.join("/tmp", f"herdr-agent-target-locks-{os.getuid()}")
    os.makedirs(lock_root, mode=0o700, exist_ok=True)
    _validate_private_directory(lock_root, "host-wide target lock directory")
    return os.path.join(lock_root, f"{digest}.lock")


def _lock_resolved_target(
    client: HerdrClient, target: Target
) -> tuple[int, str, AgentPaneInfo]:
    """Lock the initially resolved pane and prove the target did not move while waiting."""

    initial = resolve_target(client, target)
    lock_path = _target_lock_path(initial.pane_id)
    descriptor = _open_private_lock(lock_path, "host-wide target lock")
    keep_open = False
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        confirmed = resolve_target(client, target)
        if confirmed.pane_id != initial.pane_id:
            raise AgentDeliveryError(
                f"target moved from pane {initial.pane_id!r} to {confirmed.pane_id!r} "
                "while waiting for its host-wide lock"
            )
        keep_open = True
        return descriptor, initial.pane_id, confirmed
    finally:
        if not keep_open:
            os.close(descriptor)


def _bind_queue(root: str, target: Target) -> None:
    """Create or verify the durable queue-to-target binding under its own lock."""
    if not (target.pane_id or target.session_value):
        raise AgentDeliveryError("target needs --pane or a stable session value")
    _prepare(root)
    lock_path = os.path.join(root, ".binding.lock")
    binding_path = os.path.join(root, "target.json")
    descriptor = _open_private_lock(lock_path, "queue binding lock")
    expected = _binding(target)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        if os.path.lexists(binding_path):
            actual = _read_queue_json(
                binding_path, "queue target binding", require_private=True
            )
            if actual != expected:
                raise AgentDeliveryError(
                    f"queue {root} is bound to {actual!r}, refusing different target {expected!r}"
                )
        else:
            _atomic_json(binding_path, expected)
    finally:
        os.close(descriptor)


def _validate_existing_binding(root: str, target: Target) -> None:
    """Read-only binding validation for observational commands such as status."""

    if not os.path.lexists(root):
        return
    _validate_private_directory(root, "queue directory")
    binding_path = os.path.join(root, "target.json")
    if not os.path.lexists(binding_path):
        return
    expected = _binding(target)
    actual = _read_queue_json(binding_path, "queue target binding", require_private=True)
    if actual != expected:
        raise AgentDeliveryError(
            f"queue {root} is bound to {actual!r}, refusing different target {expected!r}"
        )


def _validate_existing_queue(root: str) -> None:
    """Validate every existing queue directory without creating or tightening it."""

    if not os.path.lexists(root):
        return
    _validate_private_directory(root, "queue directory")
    for path in _dirs(root):
        if os.path.lexists(path):
            _validate_private_directory(path, "queue state directory")


def _transition(source: str, destination: str) -> None:
    """Durably rename an artifact and sync both directory entries."""
    source_parent = os.path.dirname(source)
    destination_parent = os.path.dirname(destination)
    if os.path.lexists(destination):
        raise AgentDeliveryError(f"refusing to replace durable queue artifact {destination}")
    os.replace(source, destination)
    _fsync_dir(destination_parent)
    if source_parent != destination_parent:
        _fsync_dir(source_parent)


def _failed_metadata(failed_path: str, *, outcome: str, error: str) -> None:
    _atomic_json(
        failed_path + ".error",
        {"artifact": os.path.basename(failed_path), "outcome": outcome, "error": error, "failed_at": time.time()},
    )


def _quarantine_raw(path: str, failed: str, *, outcome: str, error: str) -> str:
    """Preserve malformed bytes exactly, add separate durable metadata, and advance FIFO."""
    basename = os.path.basename(path)
    destination = os.path.join(failed, basename)
    _transition(path, destination)
    _failed_metadata(destination, outcome=outcome, error=error)
    return basename[:-5] if basename.endswith(".json") else basename


def _recover_inflight(inflight: str, failed: str) -> list[str]:
    """Never resubmit a prompt whose process died after durable injection intent."""
    recovered: list[str] = []
    for name in sorted(entry for entry in os.listdir(inflight) if entry.endswith(".json")):
        source = os.path.join(inflight, name)
        destination = os.path.join(failed, name)
        _transition(source, destination)
        _failed_metadata(
            destination,
            outcome="possibly_submitted",
            error="recovered an inflight prompt after process restart; refusing automatic resubmission",
        )
        recovered.append(name[:-5])
    return recovered


def _load(path: str) -> dict[str, object]:
    raw = _read_queue_json(path, "queued message", require_private=False)
    if (
        not isinstance(raw, dict)
        or not isinstance(raw.get("text"), str)
        or not raw["text"]
    ):
        raise AgentDeliveryError(
            f"queued message {path} must have a nonempty string text field"
        )
    if "id" in raw and not isinstance(raw["id"], str):
        raise AgentDeliveryError(f"queued message {path} has a non-string id field")
    return {str(key): value for key, value in raw.items()}


def _delivery_attempts(document: dict[str, object], path: str) -> int:
    """Read the current or legacy attempt count as one strict unsigned 64-bit integer."""

    key = "delivery_attempts" if "delivery_attempts" in document else "tui_delivery_attempts"
    if key not in document:
        return 0
    value = document[key]
    if type(value) is not int or not 0 <= value <= _U64_MAX:
        raise AgentDeliveryError(
            f"queued message {path} has an invalid nonnegative integer {key} field"
        )
    return value


def _wait_ready(
    client: HerdrClient,
    target: Target,
    timeout: float,
    *,
    sleep: Callable[[float], None],
    monotonic: Callable[[], float],
    locked_pane_id: str,
    initial_info: AgentPaneInfo | None = None,
) -> AgentPaneInfo:
    deadline = monotonic() + timeout
    while True:
        info = initial_info if initial_info is not None else resolve_target(client, target)
        initial_info = None
        if info.pane_id != locked_pane_id:
            raise AgentDeliveryError(
                f"target moved from locked pane {locked_pane_id!r} to {info.pane_id!r}"
            )
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
    info: AgentPaneInfo,
    text: str,
    *,
    working_timeout: float,
) -> None:
    try:
        client.run(info.pane_id, text)
    except Exception as exc:
        # The terminal server may have accepted the atomic text+Enter before the client lost its
        # response. Once pane.run is entered, failure is ambiguous and must never be retried.
        raise _PossiblySubmitted(
            f"pane {info.pane_id} pane-run outcome is unknown; prompt may have been submitted: {exc}"
        ) from exc
    try:
        client.wait_agent_status(info.pane_id, "working", max(1, int(working_timeout * 1000)))
    except HerdrUnavailable as exc:
        raise _PossiblySubmitted(
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
    _bind_queue(root, target)
    inbox, inflight, processed, failed = _prepare(root)
    lock_path = os.path.join(root, ".delivery.lock")
    delivered: list[str] = []
    quarantined: list[str] = []
    blocked: str | None = None
    descriptor = _open_private_lock(lock_path, "queue delivery lock")
    target_descriptor = -1
    locked_pane_id = ""
    initial_info: AgentPaneInfo | None = None
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        quarantined.extend(_recover_inflight(inflight, failed))
        for path in sorted(os.path.join(inbox, name) for name in os.listdir(inbox) if name.endswith(".json")):
            try:
                document = _load(path)
                attempts = _delivery_attempts(document, path)
            except AgentDeliveryError as exc:
                quarantined.append(
                    _quarantine_raw(path, failed, outcome="invalid_message", error=str(exc))
                )
                continue
            identifier = str(document.get("id", os.path.basename(path)[:-5]))
            if attempts >= max_attempts:
                blocked = (
                    f"message {identifier} reached the maximum delivery-attempt count "
                    f"({attempts} >= {max_attempts}); retained pending"
                )
                document["delivery_state"] = "pending"
                document["delivery_error"] = blocked
                document["delivery_blocked_at"] = time.time()
                _atomic_json(path, document)
                break
            while attempts < max_attempts:
                # Readiness is entirely pre-injection. Keep the artifact in inbox while the pane
                # is busy so a process death during an ordinary wait remains safely retryable.
                try:
                    if target_descriptor < 0:
                        # Resolve before choosing the lock so exact-pane and stable-session
                        # callers serialize on the same live pane. Re-resolve after acquisition
                        # and on every readiness poll so a moving session cannot escape the lock.
                        target_descriptor, locked_pane_id, initial_info = _lock_resolved_target(
                            client, target
                        )
                    info = _wait_ready(
                        client,
                        target,
                        ready_timeout,
                        sleep=sleep,
                        monotonic=monotonic,
                        locked_pane_id=locked_pane_id,
                        initial_info=initial_info,
                    )
                    initial_info = None
                except (AgentDeliveryError, HerdrUnavailable) as exc:
                    document["delivery_state"] = "pending"
                    document["delivery_error"] = str(exc)
                    document["delivery_blocked_at"] = time.time()
                    _atomic_json(path, document)
                    blocked = str(exc)
                    break
                # ``inflight`` is a durable at-most-once barrier. Once this rename commits, a
                # crash is treated as possibly submitted. Readiness was already proven above;
                # this transition occurs immediately before pane.run.
                inflight_path = os.path.join(inflight, os.path.basename(path))
                # Rename first: a crash at every later instruction leaves the artifact in the
                # restart-quarantined directory. Updating the inbox file before this rename
                # would leave a small but real restart-resubmission window.
                _transition(path, inflight_path)
                document["possibly_submitted"] = True
                document["delivery_state"] = "inflight"
                document["inflight_at"] = time.time()
                _atomic_json(inflight_path, document)
                try:
                    _deliver_one(
                        client, info, str(document["text"]), working_timeout=working_timeout,
                    )
                except _PossiblySubmitted as exc:
                    attempts += 1
                    document["delivery_attempts"] = attempts
                    document["tui_delivery_attempts"] = attempts
                    document["delivery_error"] = str(exc)
                    document["possibly_submitted"] = True
                    document["delivery_failed_at"] = time.time()
                    _atomic_json(inflight_path, document)
                    failed_path = os.path.join(failed, os.path.basename(path))
                    _transition(inflight_path, failed_path)
                    _failed_metadata(failed_path, outcome="possibly_submitted", error=str(exc))
                    quarantined.append(identifier)
                    break
                else:
                    document["delivery_state"] = "processed"
                    document["confirmed_at"] = time.time()
                    _atomic_json(inflight_path, document)
                    _transition(inflight_path, os.path.join(processed, os.path.basename(path)))
                    delivered.append(identifier)
                    break
            if blocked is not None:
                break
    finally:
        if target_descriptor >= 0:
            os.close(target_descriptor)
        os.close(descriptor)
    pending = tuple(name[:-5] for name in sorted(os.listdir(inbox)) if name.endswith(".json"))
    outcome = "pending" if blocked is not None else ("possibly_submitted" if quarantined else "delivered")
    return QueueResult("", tuple(delivered), tuple(quarantined), pending, blocked, outcome)


def send(client: HerdrClient, target: Target, root: str, text: str, **kwargs: object) -> QueueResult:
    """Durably enqueue one prompt, drain its bound FIFO, and return confirmed delivery."""

    _bind_queue(root, target)
    # Generated identifiers are collision-resistant and the no-replace inbox create is atomic.
    # Do not wait behind a long-running drain merely to persist a new prompt; the subsequent drain
    # and terminal-artifact inspection resolve any cross-sender consumption safely.
    identifier = _enqueue(root, text, message_id=None, serialize=False)
    result = drain(client, target, root, **kwargs)  # type: ignore[arg-type]
    filename = f"{identifier}.json"
    failed_path = os.path.join(root, "failed", filename)
    if identifier in result.quarantined or os.path.lexists(failed_path):
        detail = "unknown delivery failure"
        try:
            failed_document = _load(failed_path)
            recorded = failed_document.get("delivery_error")
            if isinstance(recorded, str):
                detail = recorded
        except AgentDeliveryError:
            pass
        raise AgentPossiblySubmitted(
            f"message {identifier} has an ambiguous outcome after one injection: {detail}; "
            f"it is retained under {root}/failed",
            message_id=identifier,
            artifact=failed_path,
        )
    inflight_path = os.path.join(root, "inflight", filename)
    if os.path.lexists(inflight_path):
        raise AgentPossiblySubmitted(
            f"message {identifier} remains behind the durable inflight barrier; "
            "automatic resubmission is unsafe",
            message_id=identifier,
            artifact=inflight_path,
        )
    inbox_path = os.path.join(root, "inbox", filename)
    if identifier in result.pending or os.path.lexists(inbox_path):
        raise AgentPending(
            f"message {identifier} remains pending without consuming a retry attempt: {result.blocked}",
            message_id=identifier,
            artifact=inbox_path,
        )
    processed_path = os.path.join(root, "processed", filename)
    if identifier in result.delivered or os.path.lexists(processed_path):
        delivered = result.delivered
        if identifier not in delivered:
            delivered = (*delivered, identifier)
        return QueueResult(
            identifier,
            delivered,
            result.quarantined,
            result.pending,
            result.blocked,
            "delivered",
        )
    raise AgentDeliveryError(
        f"message {identifier} disappeared without a durable terminal artifact"
    )


def status(client: HerdrClient, target: Target, root: str) -> dict[str, object]:
    """Read validated live-agent and queue state without creating or changing queue files."""

    _validate_existing_queue(root)
    _validate_existing_binding(root, target)
    info = resolve_target(client, target)
    inbox, inflight, _processed, failed = _dirs(root)

    def identifiers(path: str) -> list[str]:
        if not os.path.lexists(path):
            return []
        _validate_private_directory(path, "queue state directory")
        return sorted(name[:-5] for name in os.listdir(path) if name.endswith(".json"))

    return {
        "pane_id": info.pane_id, "agent": info.agent, "agent_status": info.status,
        "session_agent": info.session_agent, "session_value": info.session_value,
        "workspace_id": info.workspace_id, "cwd": info.cwd,
        "pending": identifiers(inbox),
        "inflight": identifiers(inflight),
        "failed": identifiers(failed),
    }


def read(client: HerdrClient, target: Target, *, lines: int = 500) -> str:
    """Read recent terminal output from a validated interactive-agent target."""

    info = resolve_target(client, target)
    text = client.read(info.pane_id, source="recent-unwrapped", lines=lines)
    return text if text else client.read(info.pane_id, source="recent", lines=lines)
