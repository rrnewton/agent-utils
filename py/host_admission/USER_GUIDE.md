# Host admission library

Construct a HostSnapshot, AdmissionPolicy, and ResourceRequest, then call
HostAdmissionLedger.request. A grant returns one lease containing every
requested resource. Queue means the request can fit after pressure or an
earlier ticket clears. Refuse means it cannot fit under the policy. Unknown
means a required measurement or owner proof is unavailable.

Retain the returned lease for the full lifetime of the admitted work and
release it explicitly. Dropping an in-process lease object is not an ownership
transition: the durable row remains until explicit release or a later census
positively proves that its exact host, boot, PID, and start ticks are absent.

Create the ledger parent before use with caller ownership and mode `0700` (or
another mode with no group/world write bits). Parent components and final files
must not be symlinks. A malformed/unsafe/full ledger fails closed and remains
unchanged.

The default deployment mode for a new caller is shadow-only: call the pure
decide function, retain the current admission authority, and compare the
structured result. Never run two authoritative ledgers for the same resource
pool.

PSI is always represented in a snapshot but is telemetry-only unless a caller
sets an explicit threshold. Swap handling is also explicit: floor, no-swap, or
disabled. A no-swap host is never silently treated as satisfying a swap-floor
policy.
