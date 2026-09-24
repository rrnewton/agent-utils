# host-admission

`host-admission` is a small library for atomically admitting shared-host work
against memory and named-token budgets. It keeps one private, flock-serialized
ledger and reports four distinct outcomes: grant, queue, refuse, and unknown.

The library deliberately does not decide whether a repository revision is
eligible, whether test evidence is valid, or whether an agent should restart.
Callers retain those policies and submit one resource request only after their
own domain checks succeed.

The initial API is intended for shadow evaluation and compatibility adapters.
Callers should not replace an existing authoritative admission controller until
their old owners have drained or been imported under a reviewed migration.

Ledger parents must already exist, be owned by the caller, and deny group and
world writes. Lock, temp, rename, and fsync operations are relative to one
pinned parent descriptor; the library never creates deployment directories.
Returned leases retain that exact parent for cleanup and never release from a
replacement directory with the same pathname.
