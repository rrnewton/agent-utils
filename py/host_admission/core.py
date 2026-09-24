"""Versioned, fail-closed shared-host admission primitives."""

from __future__ import annotations

import fcntl
import json
import os
import re
import stat
import tempfile
import time
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
from types import TracebackType
from typing import IO, Callable

LEDGER_SCHEMA = "host-admission-ledger/v1"
SNAPSHOT_SCHEMA = "host-admission-snapshot/v1"
DECISION_SCHEMA = "host-admission-decision/v1"
MAX_LEDGER_BYTES = 4 * 1024 * 1024
MAX_RECORDS = 16_384
PPM = 1_000_000
_SAFE_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$")
_MAX_BYTES = 1 << 62
_MAX_U32 = (1 << 32) - 1
_MAX_U64 = (1 << 64) - 1


class AdmissionError(RuntimeError):
    """The shared ledger is unsafe, unreadable, or malformed."""


class Verdict(Enum):
    """The four outcomes of one admission decision."""

    GRANT = "grant"
    QUEUE = "queue"
    REFUSE = "refuse"
    UNKNOWN = "unknown"


class OwnerState(Enum):
    """Observation of one exact process owner."""

    ALIVE = "alive"
    ABSENT = "absent"
    UNKNOWN = "unknown"


class SwapMode(Enum):
    """Explicit swap policy; no implicit zero-swap exception exists."""

    FLOOR = "floor"
    NO_SWAP = "no_swap"
    DISABLED = "disabled"


@dataclass(frozen=True)
class ProcessOwner:
    """A process identity bound to a host and boot."""

    host_id: str
    boot_id: str
    pid: int
    start_ticks: int

    def __post_init__(self) -> None:
        _require_id("host_id", self.host_id)
        _require_id("boot_id", self.boot_id)
        if self.pid < 1 or self.start_ticks < 1:
            raise ValueError("pid and start_ticks must be positive")

    def to_json(self) -> dict[str, object]:
        """Return the strict ledger representation."""
        return {
            "boot_id": self.boot_id,
            "host_id": self.host_id,
            "pid": self.pid,
            "start_ticks": self.start_ticks,
        }

    @classmethod
    def from_json(cls, raw: object) -> "ProcessOwner":
        """Strictly decode one process owner."""
        return _owner_from_json(raw)


@dataclass(frozen=True)
class HostSnapshot:
    """One explicitly timestamped host measurement."""

    captured_at_unix_ms: int
    host_id: str
    boot_id: str
    mem_total_bytes: int | None
    mem_available_bytes: int | None
    swap_total_bytes: int | None
    swap_free_bytes: int | None
    memory_psi_some_avg10_micros: int | None
    memory_psi_full_avg10_micros: int | None
    errors: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        if self.captured_at_unix_ms < 0:
            raise ValueError("captured_at_unix_ms must be non-negative")
        _require_id("host_id", self.host_id)
        _require_id("boot_id", self.boot_id)
        for name in (
            "mem_total_bytes",
            "mem_available_bytes",
            "swap_total_bytes",
            "swap_free_bytes",
            "memory_psi_some_avg10_micros",
            "memory_psi_full_avg10_micros",
        ):
            value = getattr(self, name)
            if value is not None and (not isinstance(value, int) or value < 0 or value > _MAX_BYTES):
                raise ValueError(f"{name} must be a non-negative bounded integer or null")
        for error in self.errors:
            _require_id("snapshot error", error)

    @classmethod
    def from_json(cls, raw: object) -> "HostSnapshot":
        """Strictly decode one host snapshot."""
        expected = {
            "boot_id", "captured_at_unix_ms", "errors", "host_id",
            "mem_available_bytes", "mem_total_bytes",
            "memory_psi_full_avg10_micros", "memory_psi_some_avg10_micros",
            "swap_free_bytes", "swap_total_bytes",
        }
        if not isinstance(raw, dict) or set(raw) != expected:
            raise ValueError("invalid host snapshot")
        host_id, boot_id, errors = raw["host_id"], raw["boot_id"], raw["errors"]
        if not isinstance(host_id, str) or not isinstance(boot_id, str) or not isinstance(errors, list) or any(not isinstance(error, str) for error in errors):
            raise ValueError("invalid host snapshot strings")
        number_names = expected - {"host_id", "boot_id", "errors"}
        values: dict[str, int | None] = {}
        for name in number_names:
            value = raw[name]
            if value is not None and (isinstance(value, bool) or not isinstance(value, int)):
                raise ValueError("invalid host snapshot number")
            values[name] = value
        captured = values["captured_at_unix_ms"]
        if captured is None:
            raise ValueError("captured_at_unix_ms cannot be null")
        return cls(
            captured_at_unix_ms=captured, host_id=host_id, boot_id=boot_id,
            mem_total_bytes=values["mem_total_bytes"],
            mem_available_bytes=values["mem_available_bytes"],
            swap_total_bytes=values["swap_total_bytes"],
            swap_free_bytes=values["swap_free_bytes"],
            memory_psi_some_avg10_micros=values["memory_psi_some_avg10_micros"],
            memory_psi_full_avg10_micros=values["memory_psi_full_avg10_micros"],
            errors=tuple(errors),
        )


@dataclass(frozen=True)
class ResourceRequest:
    """Memory and named tokens acquired as one indivisible request."""

    request_id: str
    caller: str
    owner: ProcessOwner
    memory_bytes: int
    named_tokens: tuple[tuple[str, int], ...] = ()
    priority: int = 0
    metadata_digest: str = "none"

    def __post_init__(self) -> None:
        _require_id("request_id", self.request_id)
        _require_id("caller", self.caller)
        _require_id("metadata_digest", self.metadata_digest)
        if self.memory_bytes < 0 or self.memory_bytes > _MAX_BYTES:
            raise ValueError("memory_bytes is outside the supported range")
        if not -(1 << 31) <= self.priority < (1 << 31):
            raise ValueError("priority is outside the signed 32-bit range")
        previous = ""
        for name, count in self.named_tokens:
            _require_id("token name", name)
            if name <= previous:
                raise ValueError("named_tokens must be strictly sorted and unique")
            if count < 1 or count > _MAX_U32:
                raise ValueError("token count must be positive and bounded")
            previous = name

    def to_json(self) -> dict[str, object]:
        """Return the strict ledger representation."""
        return {
            "caller": self.caller,
            "memory_bytes": self.memory_bytes,
            "metadata_digest": self.metadata_digest,
            "named_tokens": {name: count for name, count in self.named_tokens},
            "owner": self.owner.to_json(),
            "priority": self.priority,
            "request_id": self.request_id,
        }

    @classmethod
    def from_json(cls, raw: object) -> "ResourceRequest":
        """Strictly decode one resource request."""
        return _request_from_json(raw)


@dataclass(frozen=True)
class AdmissionPolicy:
    """Caller-supplied resource policy, separate from admission mechanism."""

    memory_budget_bytes: int | None
    memory_reserve_bytes: int
    token_capacities: tuple[tuple[str, int], ...] = ()
    swap_mode: SwapMode = SwapMode.DISABLED
    swap_floor_fraction_ppm: int = 150_000
    largest_leaf_swap_bytes: int = 0
    no_swap_extra_memory_reserve_bytes: int = 0
    psi_some_max_micros: int | None = None
    psi_full_max_micros: int | None = None
    max_snapshot_age_ms: int = 30_000

    def __post_init__(self) -> None:
        for name in (
            "memory_budget_bytes",
            "memory_reserve_bytes",
            "largest_leaf_swap_bytes",
            "no_swap_extra_memory_reserve_bytes",
        ):
            value = getattr(self, name)
            if value is not None and (value < 0 or value > _MAX_BYTES):
                raise ValueError(f"{name} is outside the supported range")
        if not 0 <= self.swap_floor_fraction_ppm <= PPM:
            raise ValueError("swap_floor_fraction_ppm must be in [0, 1000000]")
        if self.max_snapshot_age_ms < 0:
            raise ValueError("max_snapshot_age_ms must be non-negative")
        for value in (self.psi_some_max_micros, self.psi_full_max_micros):
            if value is not None and not 0 <= value <= 100 * PPM:
                raise ValueError("PSI thresholds must be in millionths of one percent")
        previous = ""
        for name, count in self.token_capacities:
            _require_id("token capacity name", name)
            if name <= previous or count < 0 or count > _MAX_U32:
                raise ValueError("token capacities must be sorted, unique, and bounded")
            previous = name

    @classmethod
    def from_json(cls, raw: object) -> "AdmissionPolicy":
        """Strictly decode one admission policy."""
        expected = {
            "largest_leaf_swap_bytes", "max_snapshot_age_ms", "memory_budget_bytes",
            "memory_reserve_bytes", "no_swap_extra_memory_reserve_bytes",
            "psi_full_max_micros", "psi_some_max_micros", "swap_floor_fraction_ppm",
            "swap_mode", "token_capacities",
        }
        if not isinstance(raw, dict) or set(raw) != expected:
            raise ValueError("invalid admission policy")
        tokens = raw["token_capacities"]
        if not isinstance(tokens, dict):
            raise ValueError("invalid token capacities")
        token_items: list[tuple[str, int]] = []
        for name, count in sorted(tokens.items()):
            if not isinstance(name, str) or isinstance(count, bool) or not isinstance(count, int):
                raise ValueError("invalid token capacity")
            token_items.append((name, count))
        mode = raw["swap_mode"]
        if not isinstance(mode, str):
            raise ValueError("invalid swap mode")
        numeric_names = expected - {"swap_mode", "token_capacities"}
        numbers: dict[str, int | None] = {}
        for name in numeric_names:
            value = raw[name]
            if value is not None and (isinstance(value, bool) or not isinstance(value, int)):
                raise ValueError("invalid admission policy number")
            numbers[name] = value
        required = (
            "memory_reserve_bytes", "swap_floor_fraction_ppm", "largest_leaf_swap_bytes",
            "no_swap_extra_memory_reserve_bytes", "max_snapshot_age_ms",
        )
        if any(numbers[name] is None for name in required):
            raise ValueError("required admission policy number is null")
        return cls(
            memory_budget_bytes=numbers["memory_budget_bytes"],
            memory_reserve_bytes=_required_number(numbers, "memory_reserve_bytes"),
            token_capacities=tuple(token_items), swap_mode=SwapMode(mode),
            swap_floor_fraction_ppm=_required_number(numbers, "swap_floor_fraction_ppm"),
            largest_leaf_swap_bytes=_required_number(numbers, "largest_leaf_swap_bytes"),
            no_swap_extra_memory_reserve_bytes=_required_number(numbers, "no_swap_extra_memory_reserve_bytes"),
            psi_some_max_micros=numbers["psi_some_max_micros"],
            psi_full_max_micros=numbers["psi_full_max_micros"],
            max_snapshot_age_ms=_required_number(numbers, "max_snapshot_age_ms"),
        )


@dataclass(frozen=True)
class LeaseView:
    """Read-only view of one committed lease."""

    lease_id: str
    request: ResourceRequest
    granted_at_unix_ms: int

    def to_json(self) -> dict[str, object]:
        """Return the strict ledger representation."""
        return {
            "granted_at_unix_ms": self.granted_at_unix_ms,
            "lease_id": self.lease_id,
            "request": self.request.to_json(),
        }


@dataclass(frozen=True)
class QueueView:
    """Read-only view of one durable queue ticket."""

    request: ResourceRequest
    sequence: int

    def to_json(self) -> dict[str, object]:
        """Return the strict ledger representation."""
        return {"request": self.request.to_json(), "sequence": self.sequence}


@dataclass(frozen=True)
class Decision:
    """A sanitized, structured decision."""

    verdict: Verdict
    code: str
    request_id: str
    reserved_memory_bytes: int
    memory_budget_bytes: int | None
    memory_headroom_bytes: int | None
    swap_floor_bytes: int | None
    tokens_available: tuple[tuple[str, int], ...]
    blockers: tuple[str, ...] = ()
    queue_sequence: int | None = None
    lease_id: str | None = None

    def to_json(self) -> dict[str, object]:
        """Return only bounded non-secret fields."""
        return {
            "blockers": list(self.blockers),
            "code": self.code,
            "lease_id": self.lease_id,
            "memory_budget_bytes": self.memory_budget_bytes,
            "memory_headroom_bytes": self.memory_headroom_bytes,
            "queue_sequence": self.queue_sequence,
            "request_id": self.request_id,
            "reserved_memory_bytes": self.reserved_memory_bytes,
            "schema": DECISION_SCHEMA,
            "swap_floor_bytes": self.swap_floor_bytes,
            "tokens_available": {name: count for name, count in self.tokens_available},
            "verdict": self.verdict.value,
        }


@dataclass
class Lease:
    """A granted lease whose exact identifier can be released once."""

    lease_id: str
    request: ResourceRequest
    ledger: "HostAdmissionLedger"
    _released: bool = field(default=False, repr=False)

    def release(self) -> None:
        """Release this exact lease idempotently."""
        if not self._released:
            self.ledger.release(self.lease_id, self.request.owner)
            self._released = True

    def __enter__(self) -> "Lease":
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        self.release()


def _require_id(label: str, value: str) -> None:
    if not _SAFE_ID.fullmatch(value):
        raise ValueError(f"{label} is not a safe identifier")


def _required_number(values: dict[str, int | None], name: str) -> int:
    value = values[name]
    if value is None:
        raise ValueError(f"{name} cannot be null")
    return value


def canonical_decision_json(decision: Decision) -> str:
    """Return canonical bytes-as-text used by both implementations."""
    return json.dumps(decision.to_json(), sort_keys=True, separators=(",", ":")) + "\n"


def _token_map(items: tuple[tuple[str, int], ...]) -> dict[str, int]:
    return dict(items)


def _unknown(request: ResourceRequest, code: str, blockers: tuple[str, ...]) -> Decision:
    return Decision(
        verdict=Verdict.UNKNOWN,
        code=code,
        request_id=request.request_id,
        reserved_memory_bytes=0,
        memory_budget_bytes=None,
        memory_headroom_bytes=None,
        swap_floor_bytes=None,
        tokens_available=(),
        blockers=blockers,
    )


def decide(
    request: ResourceRequest,
    snapshot: HostSnapshot,
    policy: AdmissionPolicy,
    leases: tuple[LeaseView, ...],
    queue: tuple[QueueView, ...],
    *,
    now_unix_ms: int,
    owner_states: dict[str, OwnerState],
) -> Decision:
    """Purely decide one request from an already observed ledger and host snapshot."""
    missing: list[str] = []
    if now_unix_ms < snapshot.captured_at_unix_ms or (
        now_unix_ms - snapshot.captured_at_unix_ms > policy.max_snapshot_age_ms
    ):
        missing.append("snapshot_stale")
    for code in snapshot.errors:
        if (
            code == "memory_psi_unreadable"
            and policy.psi_some_max_micros is None
            and policy.psi_full_max_micros is None
        ):
            continue
        if code.startswith("memory_psi_some") and policy.psi_some_max_micros is None:
            continue
        if code.startswith("memory_psi_full") and policy.psi_full_max_micros is None:
            continue
        if code.startswith("swap_") and policy.swap_mode is SwapMode.DISABLED:
            continue
        missing.append(f"snapshot_error:{code}")
    if snapshot.mem_total_bytes is None:
        missing.append("mem_total_missing")
    if snapshot.mem_available_bytes is None:
        missing.append("mem_available_missing")
    if policy.swap_mode is not SwapMode.DISABLED:
        if snapshot.swap_total_bytes is None:
            missing.append("swap_total_missing")
        if snapshot.swap_free_bytes is None:
            missing.append("swap_free_missing")
    if policy.psi_some_max_micros is not None and snapshot.memory_psi_some_avg10_micros is None:
        missing.append("psi_some_missing")
    if policy.psi_full_max_micros is not None and snapshot.memory_psi_full_avg10_micros is None:
        missing.append("psi_full_missing")
    if request.owner.host_id != snapshot.host_id or request.owner.boot_id != snapshot.boot_id:
        missing.append("request_owner_host_or_boot_mismatch")
    unknown_owners = sorted(
        lease.lease_id
        for lease in leases
        if owner_states.get(lease.lease_id, OwnerState.UNKNOWN) is OwnerState.UNKNOWN
    )
    unknown_owners.extend(
        entry.request.request_id
        for entry in queue
        if entry.request.request_id != request.request_id
        and owner_states.get(entry.request.request_id, OwnerState.UNKNOWN) is OwnerState.UNKNOWN
    )
    if unknown_owners:
        missing.append("owner_unknown")
    if missing:
        return _unknown(request, "required_observation_unknown", tuple(sorted(set(missing))))

    live = tuple(
        lease
        for lease in leases
        if owner_states.get(lease.lease_id, OwnerState.UNKNOWN) is OwnerState.ALIVE
    )
    mem_total = snapshot.mem_total_bytes
    mem_available = snapshot.mem_available_bytes
    assert mem_total is not None and mem_available is not None
    budget = policy.memory_budget_bytes
    if budget is None:
        margin = min(8 * 1024**3, mem_total // 8)
        budget = max(0, (mem_total * 850_000) // PPM - margin)
    reserve = policy.memory_reserve_bytes
    swap_floor: int | None = None
    if policy.swap_mode is SwapMode.NO_SWAP:
        assert snapshot.swap_total_bytes is not None and snapshot.swap_free_bytes is not None
        if snapshot.swap_total_bytes != 0 or snapshot.swap_free_bytes != 0:
            return Decision(Verdict.REFUSE, "no_swap_policy_host_has_swap", request.request_id, 0, budget, None, None, ())
        reserve += policy.no_swap_extra_memory_reserve_bytes
    elif policy.swap_mode is SwapMode.FLOOR:
        assert snapshot.swap_total_bytes is not None and snapshot.swap_free_bytes is not None
        if snapshot.swap_total_bytes == 0:
            return Decision(Verdict.REFUSE, "swap_required_but_absent", request.request_id, 0, budget, None, 0, ())
        fraction_floor = (snapshot.swap_total_bytes * policy.swap_floor_fraction_ppm + PPM - 1) // PPM
        swap_floor = max(fraction_floor, policy.largest_leaf_swap_bytes)
    headroom = max(0, mem_available - reserve)
    reserved = sum(lease.request.memory_bytes for lease in live)
    if reserved > _MAX_BYTES:
        return _unknown(
            request, "required_observation_unknown", ("ledger_capacity_invalid",)
        )
    capacities = _token_map(policy.token_capacities)
    used: dict[str, int] = {name: 0 for name in capacities}
    for lease in live:
        for name, count in lease.request.named_tokens:
            if name in used:
                used[name] += count
    available = tuple((name, max(0, capacity - used[name])) for name, capacity in sorted(capacities.items()))

    if request.memory_bytes > budget:
        return Decision(Verdict.REFUSE, "request_exceeds_memory_budget", request.request_id, reserved, budget, headroom, swap_floor, available)
    for name, count in request.named_tokens:
        if name not in capacities:
            return Decision(Verdict.REFUSE, "unknown_named_token", request.request_id, reserved, budget, headroom, swap_floor, available, (name,))
        if count > capacities[name]:
            return Decision(Verdict.REFUSE, "request_exceeds_token_capacity", request.request_id, reserved, budget, headroom, swap_floor, available, (name,))

    current = next((entry for entry in queue if entry.request.request_id == request.request_id), None)
    if current is not None and current.request != request:
        return _unknown(request, "request_id_conflict", ("queued_request_differs",))
    sequence = current.sequence if current is not None else None
    ahead = tuple(
        entry
        for entry in queue
        if entry.request.request_id != request.request_id
        and owner_states.get(entry.request.request_id, OwnerState.UNKNOWN) is OwnerState.ALIVE
        and (
            entry.request.priority > request.priority
            or (
                entry.request.priority == request.priority
                and (sequence is None or entry.sequence < sequence)
            )
        )
    )
    if ahead:
        return Decision(Verdict.QUEUE, "queued_behind_prior_request", request.request_id, reserved, budget, headroom, swap_floor, available, tuple(entry.request.request_id for entry in ahead), sequence)
    if reserved + request.memory_bytes > budget:
        return Decision(Verdict.QUEUE, "aggregate_memory_busy", request.request_id, reserved, budget, headroom, swap_floor, available, (), sequence)
    if request.memory_bytes > headroom:
        return Decision(Verdict.QUEUE, "live_memory_busy", request.request_id, reserved, budget, headroom, swap_floor, available, (), sequence)
    if swap_floor is not None and snapshot.swap_free_bytes is not None and snapshot.swap_free_bytes < swap_floor:
        return Decision(Verdict.QUEUE, "swap_floor_not_met", request.request_id, reserved, budget, headroom, swap_floor, available, (), sequence)
    if policy.psi_some_max_micros is not None:
        assert snapshot.memory_psi_some_avg10_micros is not None
        if snapshot.memory_psi_some_avg10_micros > policy.psi_some_max_micros:
            return Decision(Verdict.QUEUE, "psi_some_above_threshold", request.request_id, reserved, budget, headroom, swap_floor, available, (), sequence)
    if policy.psi_full_max_micros is not None:
        assert snapshot.memory_psi_full_avg10_micros is not None
        if snapshot.memory_psi_full_avg10_micros > policy.psi_full_max_micros:
            return Decision(Verdict.QUEUE, "psi_full_above_threshold", request.request_id, reserved, budget, headroom, swap_floor, available, (), sequence)
    blocked_tokens = tuple(name for name, count in request.named_tokens if count > capacities[name] - used[name])
    if blocked_tokens:
        return Decision(Verdict.QUEUE, "named_tokens_busy", request.request_id, reserved, budget, headroom, swap_floor, available, blocked_tokens, sequence)
    return Decision(Verdict.GRANT, "resources_available", request.request_id, reserved, budget, headroom, swap_floor, available, (), sequence, request.request_id)


def _read_identity(path: Path, label: str) -> str:
    try:
        value = path.read_text(encoding="utf-8").strip()
    except OSError as exc:
        raise AdmissionError(f"could not read {label}") from exc
    _require_id(label, value)
    return value


def _read_meminfo() -> tuple[dict[str, int], tuple[str, ...]]:
    values: dict[str, int] = {}
    errors: list[str] = []
    try:
        lines = Path("/proc/meminfo").read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeError):
        return {}, ("meminfo_unreadable",)
    names = {
        "MemTotal": "mem_total",
        "MemAvailable": "mem_available",
        "SwapTotal": "swap_total",
        "SwapFree": "swap_free",
    }
    for wanted, error_name in names.items():
        matches = [line for line in lines if line.partition(":")[0] == wanted]
        if len(matches) != 1:
            errors.append(f"{error_name}_missing_or_duplicate")
            continue
        parts = matches[0].partition(":")[2].split()
        if len(parts) != 2 or parts[1] != "kB":
            errors.append(f"{error_name}_malformed")
            continue
        if not re.fullmatch(r"[+-]?[0-9]+", parts[0]):
            errors.append(f"{error_name}_malformed")
            continue
        raw = int(parts[0])
        if raw < 0 or raw > _MAX_BYTES // 1024:
            errors.append(f"{error_name}_out_of_range")
            continue
        values[wanted] = raw * 1024
    return values, tuple(errors)


def _read_psi() -> tuple[int | None, int | None, tuple[str, ...]]:
    try:
        lines = Path("/proc/pressure/memory").read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeError):
        return None, None, ("memory_psi_unreadable",)
    out: dict[str, int] = {}
    errors: list[str] = []
    for kind in ("some", "full"):
        rows = [line for line in lines if line.startswith(kind + " ")]
        if len(rows) != 1:
            errors.append(f"memory_psi_{kind}_missing_or_duplicate")
            continue
        avg10_fields = [
            field.partition("=")[2]
            for field in rows[0].split()[1:]
            if field.partition("=")[0] == "avg10"
        ]
        raw = avg10_fields[0] if len(avg10_fields) == 1 else None
        value = _parse_percent_micros(raw)
        if value is None or value > 100 * PPM:
            errors.append(f"memory_psi_{kind}_malformed")
            continue
        out[kind] = value
    return out.get("some"), out.get("full"), tuple(errors)


def _parse_percent_micros(raw: str | None) -> int | None:
    """Parse one non-negative decimal percentage without binary-float drift."""
    if raw is None or not re.fullmatch(r"[0-9]+(?:\.[0-9]+)?", raw):
        return None
    whole, separator, fraction = raw.partition(".")
    if len(fraction) > 6:
        discarded = fraction[6:]
        if any(character != "0" for character in discarded):
            return None
        fraction = fraction[:6]
    micros = int(whole) * PPM
    if separator:
        micros += int(fraction.ljust(6, "0"))
    return micros


def sample_host(*, now_unix_ms: int | None = None) -> HostSnapshot:
    """Sample memory, swap, PSI, host identity, and boot identity once."""
    mem, mem_errors = _read_meminfo()
    some, full, psi_errors = _read_psi()
    return HostSnapshot(
        captured_at_unix_ms=now_unix_ms if now_unix_ms is not None else time.time_ns() // 1_000_000,
        host_id=_read_identity(Path("/etc/machine-id"), "host_id"),
        boot_id=_read_identity(Path("/proc/sys/kernel/random/boot_id"), "boot_id"),
        mem_total_bytes=mem.get("MemTotal"),
        mem_available_bytes=mem.get("MemAvailable"),
        swap_total_bytes=mem.get("SwapTotal"),
        swap_free_bytes=mem.get("SwapFree"),
        memory_psi_some_avg10_micros=some,
        memory_psi_full_avg10_micros=full,
        errors=tuple(sorted((*mem_errors, *psi_errors))),
    )


def _proc_start_ticks(pid: int) -> tuple[OwnerState, int | None]:
    try:
        data = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
    except (FileNotFoundError, ProcessLookupError):
        return OwnerState.ABSENT, None
    except (PermissionError, OSError, UnicodeError):
        return OwnerState.UNKNOWN, None
    rparen = data.rfind(")")
    fields = data[rparen + 2 :].split() if rparen >= 0 else []
    if len(fields) <= 19:
        return OwnerState.UNKNOWN, None
    try:
        return OwnerState.ALIVE, int(fields[19])
    except ValueError:
        return OwnerState.UNKNOWN, None


def probe_process_owner(owner: ProcessOwner, snapshot: HostSnapshot) -> OwnerState:
    """Prove one process owner alive, absent, or unobservable."""
    if owner.host_id != snapshot.host_id:
        return OwnerState.UNKNOWN
    if owner.boot_id != snapshot.boot_id:
        return OwnerState.ABSENT
    state, start_ticks = _proc_start_ticks(owner.pid)
    if state is not OwnerState.ALIVE:
        return state
    return OwnerState.ALIVE if start_ticks == owner.start_ticks else OwnerState.ABSENT


class _Lock:
    def __init__(self, path: Path):
        self.path = path
        self.handle: IO[str] | None = None

    def __enter__(self) -> "_Lock":
        self.path.parent.mkdir(parents=True, exist_ok=True)
        try:
            fd = os.open(self.path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
        except OSError as exc:
            raise AdmissionError("could not open admission lock") from exc
        try:
            metadata = os.fstat(fd)
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.geteuid() or metadata.st_nlink != 1:
                raise AdmissionError("admission lock is not an owned single-link regular file")
            if metadata.st_mode & 0o077:
                raise AdmissionError("admission lock permissions are not private")
            self.handle = os.fdopen(fd, "r+")
        except BaseException:
            os.close(fd)
            raise
        fcntl.flock(self.handle.fileno(), fcntl.LOCK_EX)
        return self

    def __exit__(self, exc_type: type[BaseException] | None, exc: BaseException | None, tb: TracebackType | None) -> None:
        if self.handle is not None:
            fcntl.flock(self.handle.fileno(), fcntl.LOCK_UN)
            self.handle.close()


def _strict_json(data: str) -> object:
    def pairs(values: list[tuple[str, object]]) -> dict[str, object]:
        result: dict[str, object] = {}
        for key, value in values:
            if key in result:
                raise ValueError(f"duplicate key: {key}")
            result[key] = value
        return result

    return json.loads(data, object_pairs_hook=pairs)


def _owner_from_json(raw: object) -> ProcessOwner:
    if not isinstance(raw, dict) or set(raw) != {"boot_id", "host_id", "pid", "start_ticks"}:
        raise ValueError("invalid owner")
    host_id, boot_id, pid, start_ticks = raw["host_id"], raw["boot_id"], raw["pid"], raw["start_ticks"]
    if not isinstance(host_id, str) or not isinstance(boot_id, str) or isinstance(pid, bool) or not isinstance(pid, int) or isinstance(start_ticks, bool) or not isinstance(start_ticks, int):
        raise ValueError("invalid owner fields")
    return ProcessOwner(host_id, boot_id, pid, start_ticks)


def _request_from_json(raw: object) -> ResourceRequest:
    expected = {"caller", "memory_bytes", "metadata_digest", "named_tokens", "owner", "priority", "request_id"}
    if not isinstance(raw, dict) or set(raw) != expected:
        raise ValueError("invalid request")
    tokens = raw["named_tokens"]
    if not isinstance(tokens, dict):
        raise ValueError("invalid named_tokens")
    token_items: list[tuple[str, int]] = []
    for key, value in sorted(tokens.items()):
        if not isinstance(key, str) or isinstance(value, bool) or not isinstance(value, int):
            raise ValueError("invalid named token")
        token_items.append((key, value))
    scalar_names = ("caller", "metadata_digest", "request_id")
    if any(not isinstance(raw[name], str) for name in scalar_names):
        raise ValueError("invalid request string")
    if isinstance(raw["memory_bytes"], bool) or not isinstance(raw["memory_bytes"], int) or isinstance(raw["priority"], bool) or not isinstance(raw["priority"], int):
        raise ValueError("invalid request number")
    return ResourceRequest(
        request_id=raw["request_id"],
        caller=raw["caller"],
        owner=_owner_from_json(raw["owner"]),
        memory_bytes=raw["memory_bytes"],
        named_tokens=tuple(token_items),
        priority=raw["priority"],
        metadata_digest=raw["metadata_digest"],
    )


class HostAdmissionLedger:
    """One flock-serialized durable resource ledger."""

    def __init__(self, path: Path):
        self.path = path
        self.lock_path = path.with_suffix(path.suffix + ".lock")

    def _load(self) -> tuple[int, list[LeaseView], list[QueueView]]:
        try:
            fd = os.open(self.path, os.O_RDONLY | os.O_NOFOLLOW)
        except FileNotFoundError:
            return 0, [], []
        except OSError as exc:
            raise AdmissionError("could not open admission ledger") from exc
        try:
            metadata = os.fstat(fd)
            if (
                not stat.S_ISREG(metadata.st_mode)
                or metadata.st_uid != os.geteuid()
                or metadata.st_nlink != 1
                or metadata.st_mode & 0o077
                or metadata.st_size > MAX_LEDGER_BYTES
            ):
                raise AdmissionError("admission ledger is not a safe bounded regular file")
            with os.fdopen(fd, encoding="utf-8") as handle:
                fd = -1
                raw = _strict_json(handle.read(MAX_LEDGER_BYTES + 1))
        except (UnicodeError, json.JSONDecodeError, ValueError) as exc:
            raise AdmissionError("admission ledger is malformed") from exc
        finally:
            if fd >= 0:
                os.close(fd)
        if not isinstance(raw, dict) or set(raw) != {"leases", "next_sequence", "queue", "schema"} or raw.get("schema") != LEDGER_SCHEMA:
            raise AdmissionError("admission ledger has an unsupported schema")
        next_sequence, raw_leases, raw_queue = raw["next_sequence"], raw["leases"], raw["queue"]
        if isinstance(next_sequence, bool) or not isinstance(next_sequence, int) or not 0 <= next_sequence <= _MAX_U64 or not isinstance(raw_leases, list) or not isinstance(raw_queue, list) or len(raw_leases) + len(raw_queue) > MAX_RECORDS:
            raise AdmissionError("admission ledger fields are invalid")
        try:
            leases: list[LeaseView] = []
            for item in raw_leases:
                if not isinstance(item, dict) or set(item) != {"granted_at_unix_ms", "lease_id", "request"} or not isinstance(item["lease_id"], str) or isinstance(item["granted_at_unix_ms"], bool) or not isinstance(item["granted_at_unix_ms"], int) or not 0 <= item["granted_at_unix_ms"] <= _MAX_U64:
                    raise ValueError("invalid lease")
                _require_id("lease_id", item["lease_id"])
                leases.append(LeaseView(item["lease_id"], _request_from_json(item["request"]), item["granted_at_unix_ms"]))
            queue: list[QueueView] = []
            for item in raw_queue:
                if not isinstance(item, dict) or set(item) != {"request", "sequence"} or isinstance(item["sequence"], bool) or not isinstance(item["sequence"], int) or item["sequence"] < 0:
                    raise ValueError("invalid queue entry")
                queue.append(QueueView(_request_from_json(item["request"]), item["sequence"]))
        except ValueError as exc:
            raise AdmissionError("admission ledger record is invalid") from exc
        ids = [entry.request.request_id for entry in leases] + [entry.request.request_id for entry in queue]
        if len(ids) != len(set(ids)):
            raise AdmissionError("admission ledger contains duplicate request ids")
        if sum(entry.request.memory_bytes for entry in leases) > _MAX_BYTES:
            raise AdmissionError("admission ledger exceeds the aggregate memory bound")
        sequences = [entry.sequence for entry in queue]
        if len(sequences) != len(set(sequences)) or any(
            sequence >= next_sequence for sequence in sequences
        ):
            raise AdmissionError("admission ledger contains invalid queue sequences")
        return next_sequence, leases, queue

    def _store(self, next_sequence: int, leases: list[LeaseView], queue: list[QueueView]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        payload = {
            "leases": [lease.to_json() for lease in sorted(leases, key=lambda item: item.lease_id)],
            "next_sequence": next_sequence,
            "queue": [entry.to_json() for entry in sorted(queue, key=lambda item: item.sequence)],
            "schema": LEDGER_SCHEMA,
        }
        encoded = (json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n").encode()
        if len(encoded) > MAX_LEDGER_BYTES:
            raise AdmissionError("admission ledger would exceed its size bound")
        fd, temp = tempfile.mkstemp(prefix=".host-admission-", dir=str(self.path.parent))
        try:
            os.fchmod(fd, 0o600)
            with os.fdopen(fd, "wb") as handle:
                fd = -1
                handle.write(encoded)
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temp, self.path)
            directory = os.open(self.path.parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
        finally:
            if fd >= 0:
                os.close(fd)
            try:
                os.unlink(temp)
            except FileNotFoundError:
                pass

    def request(
        self,
        request: ResourceRequest,
        snapshot: HostSnapshot,
        policy: AdmissionPolicy,
        *,
        now_unix_ms: int | None = None,
        owner_probe: Callable[[ProcessOwner, HostSnapshot], OwnerState] = probe_process_owner,
    ) -> tuple[Decision, Lease | None]:
        """Atomically sweep, queue, decide, and commit one request."""
        now = time.time_ns() // 1_000_000 if now_unix_ms is None else now_unix_ms
        with _Lock(self.lock_path):
            next_sequence, leases, queue = self._load()
            if any(
                lease.lease_id == request.request_id and lease.request != request
                for lease in leases
            ) or any(
                entry.request.request_id == request.request_id and entry.request != request
                for entry in queue
            ):
                return _unknown(
                    request, "request_id_conflict", ("existing_request_differs",)
                ), None
            request_state = owner_probe(request.owner, snapshot)
            if request_state is not OwnerState.ALIVE:
                return _unknown(
                    request, "request_owner_unconfirmed", (request_state.value,)
                ), None
            states = {
                lease.lease_id: owner_probe(lease.request.owner, snapshot) for lease in leases
            }
            states.update(
                {
                    entry.request.request_id: (
                        request_state
                        if entry.request.request_id == request.request_id
                        else owner_probe(entry.request.owner, snapshot)
                    )
                    for entry in queue
                }
            )
            if any(state is OwnerState.UNKNOWN for state in states.values()):
                return decide(request, snapshot, policy, tuple(leases), tuple(queue), now_unix_ms=now, owner_states=states), None
            original_counts = (len(leases), len(queue))
            leases = [lease for lease in leases if states[lease.lease_id] is OwnerState.ALIVE]
            queue = [
                entry
                for entry in queue
                if states[entry.request.request_id] is OwnerState.ALIVE
            ]
            swept = original_counts != (len(leases), len(queue))
            existing_lease = next((lease for lease in leases if lease.request.request_id == request.request_id), None)
            if existing_lease is not None:
                if swept:
                    self._store(next_sequence, leases, queue)
                decision = Decision(Verdict.GRANT, "already_granted", request.request_id, sum(item.request.memory_bytes for item in leases if item.lease_id != existing_lease.lease_id), policy.memory_budget_bytes, None, None, (), (), None, existing_lease.lease_id)
                return decision, Lease(existing_lease.lease_id, request, self)
            existing_queue = next((entry for entry in queue if entry.request.request_id == request.request_id), None)
            if existing_queue is None:
                if next_sequence == _MAX_U64:
                    if swept:
                        self._store(next_sequence, leases, queue)
                    return _unknown(
                        request, "queue_sequence_exhausted", ("ledger_counter_exhausted",)
                    ), None
                existing_queue = QueueView(request, next_sequence)
                next_sequence += 1
                queue.append(existing_queue)
            states = {lease.lease_id: OwnerState.ALIVE for lease in leases}
            states.update(
                {entry.request.request_id: OwnerState.ALIVE for entry in queue}
            )
            decision = decide(request, snapshot, policy, tuple(leases), tuple(queue), now_unix_ms=now, owner_states=states)
            if decision.verdict is Verdict.GRANT:
                queue = [entry for entry in queue if entry.request.request_id != request.request_id]
                leases.append(LeaseView(request.request_id, request, now))
                self._store(next_sequence, leases, queue)
                return decision, Lease(request.request_id, request, self)
            if decision.verdict is Verdict.REFUSE:
                queue = [entry for entry in queue if entry.request.request_id != request.request_id]
            if decision.verdict is not Verdict.UNKNOWN or swept:
                self._store(next_sequence, leases, queue)
            return decision, None

    def release(self, lease_id: str, owner: ProcessOwner) -> None:
        """Release one exact lease without affecting peers."""
        _require_id("lease_id", lease_id)
        with _Lock(self.lock_path):
            next_sequence, leases, queue = self._load()
            kept = [lease for lease in leases if not (lease.lease_id == lease_id and lease.request.owner == owner)]
            if len(kept) != len(leases):
                self._store(next_sequence, kept, queue)
