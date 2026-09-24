"""Generic shared-host resource admission.

The public API is deliberately mechanism-only: fresh host measurements, exact
process ownership, memory and named-token requests, one durable ledger, and a
four-way decision. Repository, validation, agent, and evidence policy belongs
to callers.
"""

from .core import (
    AdmissionError,
    AdmissionPolicy,
    Decision,
    HostAdmissionLedger,
    HostSnapshot,
    Lease,
    LeaseView,
    OwnerState,
    ProcessOwner,
    QueueView,
    ResourceRequest,
    SwapMode,
    Verdict,
    canonical_decision_json,
    decide,
    probe_process_owner,
    sample_host,
)

__version__ = "0.1.0"

__all__ = [
    "AdmissionError",
    "AdmissionPolicy",
    "Decision",
    "HostAdmissionLedger",
    "HostSnapshot",
    "Lease",
    "LeaseView",
    "OwnerState",
    "ProcessOwner",
    "QueueView",
    "ResourceRequest",
    "SwapMode",
    "Verdict",
    "canonical_decision_json",
    "decide",
    "probe_process_owner",
    "sample_host",
]
