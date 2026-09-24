"""Non-authoritative bridge from the current Dagrun memory gate to host-admission.

The existing ``memory-admissions.json`` ledger and :mod:`dagrun.admission`
remain authoritative. This module exists so callers can compare a generic
decision without creating, acquiring, or releasing a second lease.
"""

from __future__ import annotations

from dataclasses import dataclass

from dagrun.admission import Verdict as LegacyVerdict
from host_admission import (
    AdmissionPolicy,
    HostSnapshot,
    LeaseView,
    OwnerState,
    ProcessOwner,
    ResourceRequest,
    SwapMode,
    Verdict,
    decide,
)


@dataclass(frozen=True)
class ShadowComparison:
    """Current authority beside a non-mutating generic shadow verdict."""

    authoritative_verdict: LegacyVerdict
    shadow_verdict: Verdict
    shadow_code: str


def compare_legacy_memory_decision(
    requested_bytes: int,
    *,
    budget_bytes: int | None,
    headroom_bytes: int | None,
    reserved_bytes: int,
) -> ShadowComparison:
    """Compare one current decision without touching either ledger.

    Missing current measurements intentionally grant, while the generic shadow
    returns ``UNKNOWN``. Reporting that disagreement is the purpose of shadow
    mode; the adapter still returns the exact existing verdict as authoritative.
    """
    if budget_bytes is not None and requested_bytes > budget_bytes:
        legacy = LegacyVerdict.REFUSE
    elif budget_bytes is not None and reserved_bytes + requested_bytes > budget_bytes:
        legacy = LegacyVerdict.QUEUE
    elif headroom_bytes is not None and requested_bytes > headroom_bytes:
        legacy = LegacyVerdict.QUEUE
    else:
        legacy = LegacyVerdict.GRANT
    owner = ProcessOwner("shadow-host", "shadow-boot", 1, 1)
    snapshot = HostSnapshot(
        captured_at_unix_ms=1,
        host_id=owner.host_id,
        boot_id=owner.boot_id,
        mem_total_bytes=budget_bytes,
        mem_available_bytes=headroom_bytes,
        swap_total_bytes=None,
        swap_free_bytes=None,
        memory_psi_some_avg10_micros=None,
        memory_psi_full_avg10_micros=None,
    )
    policy = AdmissionPolicy(
        memory_budget_bytes=budget_bytes,
        memory_reserve_bytes=0,
        swap_mode=SwapMode.DISABLED,
    )
    leases: tuple[LeaseView, ...] = ()
    if reserved_bytes:
        peer = ResourceRequest("shadow-peer", "dagrun-shadow", owner, reserved_bytes)
        leases = (LeaseView("shadow-peer", peer, 1),)
    request = ResourceRequest("shadow-request", "dagrun-shadow", owner, requested_bytes)
    shadow = decide(
        request,
        snapshot,
        policy,
        leases,
        (),
        now_unix_ms=1,
        owner_states={"shadow-peer": OwnerState.ALIVE} if reserved_bytes else {},
    )
    return ShadowComparison(legacy, shadow.verdict, shadow.code)
