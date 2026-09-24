# Shared host admission: Phase 1 shadow core and migration design

Status: implementation design accompanying a shadow-only library slice. No
existing admission authority is cut over by this change.

Consumer names are intentionally omitted under this repository's reusable-tool
policy. The neutral labels below mean:

- **DAG runner**: the existing build/test runner in this repository.
- **validation consumer**: a downstream validator with repository and evidence
  policy that cannot belong in a generic library.
- **agent supervisor**: a downstream process-lifecycle manager.
- **private cleanup plugin**: a downstream extension for deployment-specific
  resources.

## Problem and current ownership

The DAG runner already owns an opt-in, host-wide memory-admission ledger. It
serializes requests with flock, accounts for live process holders, and returns
grant, queue, or refuse. Its deliberate compatibility behavior treats missing
host memory measurements as no gate. Existing command output, exit codes, and
the `memory-admissions.json` format are public behavior and remain unchanged.

The validation consumer has a separate, richer authority. It combines a
priority/FIFO queue, memory reservations, a finite validation-slot pool,
exclusive benchmark admission, exact source eligibility, owner-watch policy,
run records, deadlines, quarantine, and evidence publication. Only its memory
and named-concurrency resources are candidates for eventual sharing. Source
eligibility, evidence, receipts, cleanup, and quarantine stay with that
consumer.

The agent supervisor needs the same generic host measurements and resource
lease, but its launch specification, desired state, restart policy, process
identity, cgroups, and terminal receipts are lifecycle policy, not admission
policy.

Running these systems as independent authoritative resource ledgers can
double-book one host. Replacing either one abruptly can also strand live owners
or turn a transient queue into an unsafe grant. Phase 1 therefore adds one
generic protocol and comparison adapter without changing any current authority.

## Generic mechanism boundary

The generic library owns exactly these concerns:

1. One versioned, private, bounded JSON ledger protected by one stable flock.
   Its parent must already exist, be owned by the caller, and deny group/world
   writes. Every path component is opened without following symlinks; the lock,
   ledger, temporary file, rename, and directory fsync are relative to one held
   parent descriptor. The ledger is capped at 16,384 combined leases/tickets
   before a write can make a valid file unreadable.
2. A transaction that observes owners, removes only positively absent owners,
   orders tickets, decides, and commits under that lock.
3. Four outcomes: `GRANT`, `QUEUE`, `REFUSE`, and `UNKNOWN`.
4. FIFO within numeric priority, with a stable sequence retained across polls.
5. A process owner bound to host id, boot id, PID, and process start ticks.
6. An atomic request containing memory and typed named-token counts.
7. A fresh snapshot containing MemTotal, MemAvailable, SwapTotal, SwapFree,
   memory PSI some/full, capture time, host id, boot id, and bounded error codes.
8. Caller-supplied resource policy: memory budget/reserve, token capacities,
   explicit swap branch, optional PSI thresholds, and freshness limit.
9. Sanitized structured diagnostics containing only safe identifiers, counts,
   budgets, reason codes, and queue/lease ids.

The generic library does not parse repository revisions, determine validation
eligibility, mint test receipts, inspect agent goals, decide restart policy, or
interpret deployment-specific containers. A generic grant proves only that the
resource transaction succeeded.

## Decision and ownership semantics

`GRANT` records memory and every named token together. A caller never holds one
dimension while waiting for another. `QUEUE` means the request could be valid
after earlier tickets, current leases, live memory, swap, named tokens, or an
enabled PSI predicate change. `REFUSE` means waiting cannot satisfy the current
policy, such as a request larger than the total budget or token capacity.
`UNKNOWN` means a required observation, owner proof, snapshot field, or state
record cannot be trusted.

Owner cleanup is conservative:

- same host and boot plus matching PID start ticks is alive;
- same host with a different boot is positively absent;
- a reused PID with different start ticks is positively absent;
- an unreadable process record or different host is unknown;
- unknown owners retain their rows and block affected decisions;
- queued tickets are owner-checked exactly like leases, so a dead waiter cannot
  block FIFO forever;
- reusing a request id with different request bytes is unknown/conflict, never a
  new request.

The initial swap-floor policy is `max(configured fraction of SwapTotal,
largest measured leaf swap use)`. Equality passes. A host with zero swap must
select the explicit no-swap branch and its extra memory reserve; it never
silently satisfies the floor branch. PSI fields are always sampled, but PSI has
no authority unless the caller supplies thresholds. If a threshold is enabled,
its missing/malformed field is unknown.

## One canonical protocol in two implementations

The Python distribution and Rust crate use the same `v1` schema identifiers,
field meanings, integer units, ordering rules, and fixture corpus. The shared
corpus covers exact equality, large integer arithmetic, malformed/missing
measurements, stale snapshots, PID reuse, reboot, dead and unknown queue owners,
priority/FIFO order, named-token atomicity, no-swap policy, swap-floor formula,
and request-id conflicts. The differential compares the raw concatenated
canonical decision bytes, including framing, not just verdicts or normalized
lines. It also proves Python-written/Rust-read and Rust-written/Python-read
ledger interoperability and exact writer bytes.

Ledger writers manually construct sorted object keys and stable collections so
their bytes do not depend on Rust dependency-feature unification and either edition can
read the same state. Strict readers reject unknown structural fields, unsafe
files, oversized state, duplicate ids, invalid owner identities, and unsupported
schema versions. Writes use descriptor-relative private same-directory temporary
files, file fsync, atomic replacement, and directory fsync.

## DAG runner compatibility adapter

The compatibility adapter is intentionally read-only. Its existing budget and
headroom inputs are derived limits, not raw `MemTotal`/`MemAvailable`; therefore
the adapter does not fabricate a host snapshot from them. Until a future adapter
supplies an actual captured snapshot, the generic side conservatively reports
unknown while returning both results. The existing verdict remains the
authoritative result. The adapter does not open the generic ledger, reserve a
token, alter command output, or change exit status.

This deliberately limits Phase 1 to wiring and authority-preservation checks.
Shadow telemetry must report the unknown result; it must not silently change the
live result. A later adapter can accept a real timestamped host snapshot, and a
later authority decision can choose fail-closed behavior only as a separately
reviewed compatibility change.

## Validation-consumer boundary

The validation consumer retains all domain decisions. Before requesting generic
resources it establishes source eligibility and owner-watch policy. Once
eligible, it eventually submits one generic transaction for memory and all
required validation tokens. It may not hold an old slot or old memory
reservation while queued for the new transaction. A generic grant is handed to
the exact validation process or unit identity; it does not set an evidence flag
or create a receipt.

The current slot counts, benchmark-all-slots rule, source checks, run records,
deadlines, cleanup, quarantine, and evidence labels remain unchanged during
shadow comparison. Their existing tests remain the compatibility contract.

## Shadow rollout and observability

Phase 1 rollout has no actuation:

1. Run the generic pure decision beside each current authority.
2. Record canonical sanitized inputs and both outcomes without acquiring a new
   lease.
3. Classify every mismatch as policy, measurement, ownership, arithmetic, or
   ordering rather than forcing them to agree.
4. Measure decision latency, flock wait, queue age, holder count, reserved
   memory, named-token use, snapshot age, swap floor/headroom, PSI telemetry,
   and unknown reason counts.
5. Reject any instrumentation that serializes command lines, environment,
   repository metadata, goals, evidence contents, or private plugin state.

The shadow ledger is not opened by compatibility adapters. There is therefore
no second set of live reservations and no accidental partial cutover.

## Single-authority cutover protocol

Authority migration is a later phase and must be explicit:

1. Teach old and new binaries the versioned protocol and authority marker while
   leaving the old path authoritative.
2. Prove shadow parity or document every approved policy difference.
3. Stop new old-authority admissions.
4. Wait for all old owners to drain, or import every exact owner under the one
   generic lock with host, boot, PID, start ticks, memory, tokens, and request
   identity intact.
5. Read back one ledger containing every live owner and one authority marker.
6. Switch memory and named-token authority atomically. Do not move memory first
   and tokens later.
7. Enable new requests only after the readback.
8. Keep domain eligibility/evidence logic in the validation consumer.

Mixed binaries are allowed only when they share the same lock and compatible
protocol. Two independent authoritative files are forbidden. A polling caller
keeps one ticket; retrying must not mint a new FIFO position.

## Rollback

Before cutover, rollback means discarding shadow results and leaving the current
authority untouched. After cutover:

1. stop new generic grants;
2. retain or import exact live generic owners until they drain;
3. verify the generic ledger has no live lease or ticket that the old authority
   cannot represent;
4. switch the authority marker back before old grants resume.

Never enable the old authority while generic leases remain live. Never delete
unknown owners to make a rollback finish. A failed or incomplete migration is
unknown and paused, not permission to run both gates.

## Why cleanup stays private

Deployment-specific cleanup knows container labels, private process substrates,
retention classes, ownership-transfer rules, and exact target selection. Those
concepts are not portable resource admission and must not enter the OSS ledger.
A private plugin may consume an immutable generation-terminal event after the
supervisor has proved its owner terminal. It is report-only until it has an
exact-generation, exact-target API and independent mutation tests. The plugin
cannot assert process death, alter an admission verdict, release an unrelated
lease, or trigger restart.

## Current limits and next adapters

This slice does not cut over the DAG runner, validation consumer, or agent
supervisor. It does not implement systemd-unit owners, lease handoff from a
launcher to a unit invocation, parent/child incremental reservations, queue
cancellation, PSI hysteresis, or a daemon. Those are later protocol additions
and must preserve one authority.

The next safe adapters are:

1. read-only shadow sampling in the DAG runner using the compatibility result;
2. an agent-supervisor shadow request with restart still disabled;
3. a validation-consumer shadow translation of memory plus named tokens, with
   all repository/evidence checks retained outside the core;
4. exact process-to-systemd-owner handoff and cancellation fixtures before any
   live authority migration.
