---
title: 'rust-primary-wrkslots: port wrkslots to Rust behind the same CLI'
status: open
priority: 2
issue_type: task
created_at: 2026-10-27T00:49:51.191658938+00:00
updated_at: 2026-10-27T00:49:51.191658938+00:00
---

# Description

[muse watermelon] rust-primary campaign milestone 4: port wrkslots to Rust
incrementally behind the same CLI, growing rs/wrkslotsd subcommand by
subcommand, with the existing Python tests driving a differential harness.
Switch the primary edition in validation/editions.json only at parity; the
Python edition then becomes a paused mirror unless parity is cheap.

First slices should be pure readers with exact output parity (status/doctor
over a fixture checkout) before any mutating command moves.

[muse watermelon] Groundwork notes: Python wrkslots is the slot-manager CLI
(~62k lines of cli.py alone, ~40 subcommands); rs/wrkslotsd today is a
read-only observer over EVENTS logs (rebuild/status/explain/plan), a
different surface. First slice plan: build a fixture corpus from the Python
tests (init/create/status --format json over a temp project) and land a
wrkslotsd reader that reproduces `status --format json` for that corpus
byte-for-byte before any mutating command moves. `chat launch` stays held
until the delivery-latency lane lands; migrate/repair fold in here via the
herdr-subagents runtime when this port reaches turn-runner workers.

[muse watermelon] Slice 1 landed: Rust document loader (wrkslotsd parse-config) at differential parity with the Python loader (17-case corpus). Next slice: config validation (the load_config semantics: schema, inheritance, normalization) in wrkslotsd behind a read-only command, same differential pattern.

[muse watermelon] Slice 2 scoping: `_load_config` (py/wrkslots/cli.py)
discovers the root, exact-keys the document, then validates schema,
machine, worktrees/control directories, remote, landed ref, liveness
commands, caps, globs, hooks, disk thresholds, and representation/image/
sandbox settings. Rust slice 2 = `wrkslotsd validate-config --root DIR`
reproducing the resulting Config as canonical JSON for fixture roots built
by the Python tests, error-for-error on the refusal cases. No mutation.

[muse watermelon] Slice 2 landed (358be730): `wrkslotsd validate-config`
reproduces `_load_config` validation as canonical JSON, error-for-error on
refusals, over a Python-built fixture corpus (test_config_validate_rust.py).
Next slice: a pure state reader (status/list JSON over fixture slot state).

[muse watermelon] Slice 3 landed (3ec1769f): `wrkslotsd parse-state` reproduces
the ACTIVE.<machine>.json shard loader in reference mode
(`_load_active_snapshot(..., require_repository=False)` + `_active_to_obj`):
record identity/handoff/checkout/task-scope/import sub-formats, duplicate
and machine-shard checks, and stored-path authority against the validated
configuration. No live-process, Git, or checkout-existence inspection.

[muse watermelon] Slice 4 landed (8d6d86c1): `wrkslotsd parse-archive` reproduces
`_archive_from_obj` / `_load_archive_snapshot`: retirement evidence,
archive-id derivation, historical-import consistency, and archived
checkout path/authority, emitting records as stored.

[muse watermelon] Slice 5 landed (109abe9a): `wrkslotsd parse-handoff` reproduces
`_load_handoff_sidecar` bound to its active record (bounded identity-bound
file read, strict duplicate-refusing JSON, schema 1/2 provenance rules,
content digest binding) and `_handoff_artifact_to_obj` output.

[muse watermelon] Slice 6 landed (9d3666e1): `wrkslotsd parse-hold` reproduces
`_load_hold_snapshot` for the HOLD.<machine>.<slot>.json record
(absent = null; exact keys, schema, filename identity, timestamp, reason).

[muse watermelon] Slice 7 landed (cdbcb513): `wrkslotsd parse-journal` reproduces
`_validate_journal_shape` for create, import-existing, and finish journals
(filename agreement, planned/created shapes, embedded records, finish
phase/archive-id and optional evidence bundles). Other kinds still refuse.

[muse watermelon] Slice 8 landed (e47d3cfd): `parse-journal` also covers the
legacy-validate-remove journal kind (exact fields, actor identity,
coordinator authorization flag).

[muse watermelon] Slice 9 landed (648ee8a6): validation/editions.json now records the
slice 1-8 format readers (the wrkslotsd tool note and the wrkslots format
inventory entry name the Rust modules, the differential corpus, and the
remaining ownerless/absent journal kinds).

[muse watermelon] Slice 10 landed (df82d2a0): `parse-journal` also covers
ownerless-validate-remove, including the validation-recovery authorization
and, when present, the exclusion path validation (recovery target, fenced
path, guard identity). The ownerless-agent/clone/cache and absent-row
journal kinds remain unsupported.

[muse watermelon] Slice 11 landed (0ae36782): `parse-journal` also covers
recover-absent-validate-rows (journal identity, per-row record digest,
and the exact derived ARCHIVED entry per row). The ownerless-agent,
ownerless-clone, cache-relocate, and absent-agent-row kinds remain.

[muse watermelon] Slice 12 landed (aa9cd253): `parse-journal` also covers
ownerless-agent-remove, including its directory-identity check (device,
inode, mount id) on a present target and repository resolution. The
ownerless-clone, cache-relocate, and absent-agent-row kinds remain.

[muse watermelon] Slice 13 landed (a9ac301c): `parse-journal` also covers
ownerless-clone-remove (clone authorization, target/fenced agreement,
directory identity). Remaining: ownerless-agent-cache-relocate and
recover-absent-agent-row.

[muse watermelon] Slice 14 landed (2dc67466): `parse-journal` also covers
ownerless-agent-cache-relocate (host identity, canonical cache paths,
fence, directory identity). Remaining: recover-absent-agent-row.

[muse watermelon] Slice 15 landed (0eaa9f58): `parse-journal` also covers
recover-absent-agent-row (row identity and record digest, branch
witnesses, remote containment, submodule salvage receipts, planned
rescue receipts, and the exact derived ARCHIVED entry under both rescue
roots). That completes every recovery-journal kind.

[muse watermelon] Slice 16 landed (b2ab2951): validation/editions.json updated for
complete journal coverage. After this, the remaining work toward a Rust
wrkslots is composed read commands (list/status-style summaries over the
validated formats), then writers.

[muse watermelon] Slice 17 landed (1a1e6271): `wrkslotsd parse-states`
composes the ACTIVE/ARCHIVED readers with `_ensure_state_shard`'s
partial-shard pairing rule (snapshot mode; EVENTS derivation stays with
`rebuild`). Gate 28 failures were load flakes only: cross-skip-accounting
(71 passed alone), vibe-talk page-1 shift-o-latency (passes alone), and
wrkslots shard 1 (468 passed alone) each passed in a solo rerun, so every
selected step passed in some run. Next: further composed readers or the
first writer slice.

[muse watermelon] Slice 18 landed (7fea9b49): `wrkslotsd parse-global`
composes the per-machine shard readers as one registry (machine-shard
and event-directory discovery, shard-set disagreement without history,
cross-machine slot/agent uniqueness, active-versus-archived overlap) in
snapshot/reference mode; EVENTS-derived machines refuse toward `rebuild`
and the Python lock-based overlap re-read is not reproduced. Adversarial
review found the event-log entry classification, error precedence, and
first-conflict ordering divergences; all fixed with differential cases
(14 passed). Gate 29 failures were load flakes only (cross-skip 71
passed alone; wrkslots shard 0 476 passed alone). Note: the landed
`parse-states` (slice 17) keeps the simpler any-entry event check and
should adopt the same classification. Next: slice 19 parse-holds.

[muse watermelon] Slice 19 landed (44764349): `wrkslotsd parse-holds`
composes the active-shard and hold readers per `_load_record_holds`
(one entry per active record across all machines, per-slot errors
captured) in snapshot mode; EVENTS-derived machines refuse toward
`rebuild`, an invalid event log becomes each record's captured error.
Adversarial review found the corrupt-chain classification, error
precedence, and partial-name edges; all fixed with differential cases
(11 passed in the new suite; library-generated error suffixes remain a
known low divergence). Gate 30 failures were load flakes only
(cross-skip 71 passed alone; wrkslots shard 0 481 passed alone).
Next: slice 20 gives `parse-states` the same event-log classification.

[muse watermelon] Slice 20 landed (09958917): `wrkslotsd parse-states`
adopts the shared event-log classification (complete event file
derives toward `rebuild` after chain validation, unexpected entry
refuses with the event loader's message, an in-progress write of the
first event file refuses with the bootstrap's partial-update message
after shard validation). Adversarial review found the precedence,
symlinked-log, and corrupt-chain divergences; all fixed with
differential cases. Gate 31 failed only on validation-graph (timeout
under load), passed alone (42 passed).

[muse watermelon] Slice 21 landed (2a033777): `wrkslotsd parse-journals`
reproduces `_outstanding_journals` (control-dir journal files plus
operations still pending in each machine's validated event history).
Adversarial review found unvalidated EVENTS machine names and the
bare `ACTIVE.journal` glob edge; both fixed with differential cases
(8 passed). Gate 32 failures were load flakes only (cross-skip and
validation-graph 113 passed together alone; wrkslots shards 488 + 488
passed alone). Upstream vibe-talk commits shared no files; rebased and
pushed without a second gate.

[muse watermelon] Slice 22 landed (5a4fc874): `wrkslotsd parse-status`
reproduces the per-record status object over snapshot states (owner
process diagnosis from this host's /proc, boot and machine identity,
heartbeat age/expiry, hold outcome). Registry storage inconsistencies
and event-derived owner releases stay with the Python `status`.
Adversarial review found the fractional-second heartbeat expiry and
the hold-error refusal; both fixed with differential cases (status
suite 3 passed, 13 with holds). Gate 33 failures were load flakes only
(cross-skip 71 passed alone; wrkslots shard 0 490 passed alone).
Upstream vibe-talk commits shared no files; rebased and pushed
without a second gate.

[muse watermelon] Slice 23 landed (c91fc914): `wrkslotsd parse-doctor`
composes tolerant status records with the snapshot-scope doctor
findings (invalid hold, dead owner, unfinished journal, partial
atomic update, cap breach). Registry storage, slot-image, and
owner-release findings stay with the Python `doctor`. Adversarial
review found the machine-scoping divergence (default local machine,
`--all-machines` for every machine); fixed with differential cases
(3 passed). Gate 34 failures were load flakes only (cross-skip and
validation-graph 113 passed together alone; wrkslots shards 491 + 491
passed alone). Upstream commits shared no files; rebased and pushed
without a second gate.

[muse watermelon] Slice 24 landed (19d6d989): `wrkslotsd parse-storage`
reproduces the filesystem half of `_registry_storage_inconsistencies`
(slot-directory presence/safety, nested checkout entries, managed
directory enumeration). Git registration/identity checks and slot
image mount findings stay with the Python `status`. Adversarial
review found the unguarded slot-images refusal, Python repr details,
and discarded directory-entry errors; all fixed with differential
cases (3 passed). Gate 35 failed under severe host overload (OOM and
CPU timeouts across 13 steps); every failure passed alone
(repository-infrastructure 149, cross-skip+graph 113, dagrun 494+493,
lifecycle host shard 121, wrkslots 493+492, and all four vibe-talk
browser checks). Upstream commits shared no files; rebased and
pushed without a second gate.

[muse watermelon] Slice 25 landed (ed6e32d3): `wrkslotsd parse-storage-git`
reproduces the Git half of `_registry_storage_inconsistencies`
(repository evidence, worktree registration, checkout identity) with
the same controlled Git invocation. Filesystem findings remain in
`parse-storage` (slice 24). Adversarial review found the loose
repository-path copy and error spelling, plus unreadable-slot
inspectability; all fixed reusing the strict loader definitions
(2 passed). Gate 36 failures were load flakes only (cross-skip+graph
113 passed alone, scroll-stability browser passed alone, wrkslots
shards 494 + 493 passed alone). Upstream agentctl commits shared no
files; rebased and pushed without a second gate.

[muse watermelon] Slice 26 landed (5b22315e): `wrkslotsd ensure-states`,
the first writer slice, reproduces `_ensure_state_shard` (shard-pair
bootstrap plus `state-imported` event initialization) behind the
registry locks with atomic writes. The writer refuses unless
WRKSLOTSD_FIXTURE_WRITER=1 attests a temporary registry; the live
registry stays on the Python edition until an approved cutover.
Adversarial review found the missing fixture guard, multi-import
derived logs, Invalid event-log fall-through, 5s lock waits (Python
fails immediately), non-ASCII JSON bytes, and event-dir mode churn;
all fixed with new differential cases (11 passed). Gate 37's ten
failures were one rustfmt fix plus load flakes (validation-graph 149,
wrkslots shards 499 + 499, cross agentctl 777, five vibe-talk browser
checks all passed alone). Upstream agentctl commit shared no files;
rebased and pushed without a second gate.

[muse watermelon] Slice 27 landed (aab4d857): `wrkslotsd hold`, the
second writer slice, reproduces the slot-hold mutation behind the
registry locks (config-directory lock with a configuration-changed
recheck, partial/seal/journal refusals, global active/archived
overlap validation, filesystem+Git storage findings for the target
slot, the `slot-held` event, and the atomic HOLD record). The
already-held decision replays the event log when it derives the
machine, matching the Python loader. Adversarial review found the
snapshot-only hold decision, the dropped overlap check, the missing
seal-journal refusal, the narrower lock set, and the silent
slot-image storage skip; all fixed with new differential cases
(10 passed). Known divergences: storage refusal names the first
finding instead of Python's aggregated text, non-target storage
findings do not print warnings, and `--wait-lock` is not offered
(Python's default is also fail-immediately). Gate 38 failures were
load flakes only (validation-graph 149, agentctl revive test,
wrkslots shards 504 + 504 all passed alone). Upstream commits shared
no files; rebased and pushed without a second gate.

[muse watermelon] Slice 28 landed (e7727a4d): `wrkslotsd unhold`,
the third writer slice, reproduces the hold-release mutation
(locks, partial/seal/journal refusals, global cross-machine
validation, the storage-absent release path with its NOTE, the
`slot-hold-released` event, and HOLD record removal) behind the
fixture-only writer guard. Adversarial review found the
order-dependent overlap check, imported-hold invisibility, the
over-broad storage-absent test, and unhold's missing
configuration-changed recheck; all fixed with new differential
cases (8 passed in this slice's file, 10 in slice 27's). Known
divergences: derived-state reads still prefer the materialized
snapshot over a full event fold (crash windows only; writers
materialize both), storage refusal text names the first finding,
and `--wait-lock` is not offered. Gate 39 failures were load
timeouts only (repository-infrastructure 149, wrkslots shards
508 + 508 passed alone). Upstream commits shared no files;
rebased and pushed without a second gate.

[muse watermelon] Slice 29 landed (c821e7bd): `wrkslotsd heartbeat`,
the fourth writer slice, reproduces the owner heartbeat mutation
(caller-process capture and ancestry proof, owner authorization,
seal/journal disjointness, owner-release refusal with the
publication and sidecar void rules, the `active-state-recorded`
event, and the ACTIVE shard rewrite at the next revision) behind
the fixture-only writer guard. The shared `active_document` now
folds derived event logs (import plus `active-state-recorded`
replacements) the way the Python loader does. Adversarial review
found the incomplete release void rules, the seal-collapse of the
journal guard, name-only seal overlap, and the stale-snapshot
revision regression; all fixed. Known divergences: cleanup-create
journal classification beside a seal is approximated by journal
names, storage refusal text names the first finding, and
`--wait-lock` is not offered. Differential: 6 passed in this
slice's file (35 across the writer files). Gate 40 failures were
load flakes only (repository-infrastructure 149, herdr-agent
differential 103, wrkslots shards 511 + 511 passed alone).
Upstream commits shared no files; rebased and pushed without a
second gate.

[muse watermelon] Slice 30 landed (41fbb81f): `wrkslotsd
write-handoff`, the fifth writer slice, reproduces the handoff
publication mutation (identity-bound source capture, canonical and
outside-Git source checks, owner authorization, intent/completion
publication events with writer and digest validation, the
renameat2-no-replace sidecar write with post-write verification,
and the `handoff-written` event) behind the fixture-only writer
guard. Adversarial review found trusted forged intents, the
unverified completion, the environment-sensitive Git probe, the
missing committed-project HANDOFF.md case, and the barely-validated
existing sidecar; all fixed with new differential cases (9 passed).
Known divergences: duplicate-key JSON in an existing sidecar is
not detected, the cleanup-create journal classification beside a
seal stays approximated, and storage refusal text names the first
finding. Gate 41 failures were load flakes only
(repository-infrastructure 149, thread-card-header browser,
wrkslots shards 516 + 515 passed alone). Upstream commits shared
no files; rebased and pushed without a second gate.

[muse watermelon] Slice 31 landed (b5239e0e): `wrkslotsd release`,
the sixth writer slice, reproduces the owner release mutation
(owner authorization, agent-slot and hold guards, validated
handoff publication, the `owner-released` event, and the
already-released outcome) behind the fixture-only writer guard.
Adversarial review found dropped publication authentication,
the hand-parsed sidecar, missing legacy HANDOFF.md branches,
wrong unpublished-sidecar refusal branches, and weak
already-released detection; all fixed (the strict sidecar loader
and publication validator are now shared). Differential: 6 passed
in this slice's file (50 across the writer files). Gate 42
failures were load flakes only (repository-infrastructure 149,
scroll-stability browser, wrkslots shards 519 + 518 passed
alone). Upstream commits shared no files; rebased and pushed
without a second gate.

[muse watermelon] Slice 32 landed (5fe6a7dc): `wrkslotsd
read-handoff`, the seventh writer slice, reproduces the
coordinator handoff-read mutation (coordinator capture,
publication-bound artifact resolution with legacy projection,
the `handoff-read` retirement-queue event, and the
already-queued outcome) in one lock phase behind the
fixture-only writer guard. Adversarial review found the
unbound sidecar, weak absent-storage and legacy
classification, malformed-history tolerance, and unresolved
legacy paths; all fixed. Known divergences: the two-phase
display race rechecks cannot fire in one lock phase, the
intent writer binds to the current record owner, and the
cleanup-journal classification beside a seal stays
approximated. Differential: 6 passed in this slice's file
(56 across the writer files). Gate 43 failures were load
flakes only (repository-infrastructure 149, cross agentctl
777, wrkslots shards 522 + 521 passed alone). Upstream
commits shared no files; rebased and pushed without a
second gate.

[muse watermelon] Slice 33 landed (3cd02ec8): `wrkslotsd adopt`,
the eighth writer slice, reproduces the owner-binding mutation
(caller capture, generation/agent checks, already-adopted and
historical-owner outcomes, the systemd task-scope verification
with pinned invocation symlink and in-lock re-verification, and
the `owner-bound` event) behind the fixture-only writer guard.
Adversarial review found the same-owner/different-scope refusal,
the single scope verification, and the unpinned symlink read;
all fixed. Known divergences: derived-log records are folded
without per-record strict parsing, global validation reads
snapshots while the target read folds events, and `--wait-lock`
is not offered. Differential: 6 passed in this slice's file
(62 across the writer files). Gate 44 failures were load
timeouts only (repository-infrastructure 149, wrkslots shards
527 + 524 passed alone; one intermittent single-test failure
under load did not reproduce across two logged reruns). Upstream
commits shared no files; rebased and pushed without a second
gate.

[muse watermelon] Slice 34 landed (101ebece): `wrkslotsd create`,
the ninth writer slice, reproduces worktree slot creation
(coordinator/owner authorization, cross-machine and archive
free checks, remote/start-ref validation against the configured
default remote, representation resolution from config with the
image representation refused, CREATE journal lifecycle with
hook running/failed records, post-provision hooks, generation
and checkout identity re-verification before publication, and
the active-state-recorded publication) behind the fixture-only
writer guard. Task scope flags embed the verified scope in the
journal and record. Disk floors apply only when all three
thresholds are configured, measured on the managed root, and
fail closed. Adversarial review findings (destination
validation, stale fetch refs, single operation id, target-clear
overlap, hook failure journaling, post-hooks re-verification)
were all fixed with differential cases. Known divergences:
fetch is an in-repo fetch with stale tracking-ref deletion
rather than Python's isolated authority-checked fetch;
setup-hook installers do not run; image-representation and
validate-batch-sealed creates stay Python. Differential:
6 passed in this slice's file (68 across the writer files).
Gates 45-46 failures were load timeouts passed alone (wrkslots
shards 530 + 529, repository-infrastructure 113) plus three
host-environment failures proven identical on an untouched
origin/main export (two lifecycle process-scan tests and the
standalone wrkslots lifecycle, all refused by unrelated host
process state). Upstream commits shared no files.

[muse watermelon] Slice 35 landed (521244af): `wrkslotsd finish`,
the tenth writer slice, reproduces the finish mutation (owner
authorization, seal and cleanup-journal guards, handoff
preconditions over record paths and slot contents, per-checkout
Git safety: worktree identity, branch, unfinished operations,
ordinary history/index, cache policy with a component fnmatch
port, clean status, remote authority, containment and landed
ancestry, and the single `handoff-recorded` event) behind the
fixture-only writer guard. The fetch lands tracking refs in one
verified update-ref transaction after a scratch-namespace fetch,
not in place. Adversarial review found no Critical/High; the
fnmatch `[^...]` inversion, fetch mutation on refusal paths,
registry-global slot-image refusal, diagnostic quoting, and the
missing pre-write revision recheck were all fixed, with new
differential cases for tracked cache paths, detached HEAD, extra
slot entries, and stale generations. Known divergences: the
fetch does not use Python's isolated network repository or its
proxy/credential plumbing; seal-side journal classification is
approximated by journal names (as in release); slot-image
records stay Python; the Rust storage reader names extra slot
entries with its own text. Differential: 10 passed in this
slice's file (74 across the writer files). Gates 47-48: gate 47
caught a missing `py/` prefix in the components manifest
(fixed); gate 48 failures were load timeouts passed alone
(wrkslots shards 535 + 534, repository-infrastructure 113,
herdr-agent differential 104) plus host-environment failures
proven identical on untouched origin/main exports (two lifecycle
process-scan tests, the standalone wrkslots lifecycle, and the
full agentctl Rust suite, which fails on base with varying
counts under load while its subsets pass). Upstream commits
shared no files; rebased and pushed without a second gate.

[muse watermelon] Decision recorded (coordinator, 2026-10-28, M2):
Rust `chat launch` launches the current Rust bridge generation
(chat init/run state; Slack, fake and plugin providers), not the
Python Google Chat polling bridge. The owner retired Google Chat
on 2026-10-26 and every live bridge on both hosts already runs
Rust `chat run`, so porting the Python polling loop has no users.
Rust launch refuses a Python-format chat.json or polling state
with a clear `chat init` pointer: no silent conversion, no reading
of the old state. Landed as 9086ca74 under #545; recorded here as
instructed.

### Slice 36 landed: `remove` writer (fixture-guarded)

- Commits: `37078493` (writer + differential) and `f6922998` (review fixes) on top of `5effb872`.
- Rust `wrkslotsd remove` covers the ordinary coordinator-authorized agent-slot removal: dead-or-absent owner with the bound registered-liveness protocol (exact one-subject `WRKSLOTS_*` environment; exit 1/2 accepted only for their bound single-line verdicts), expired heartbeat, already-published clean salvage (remote containment of HEAD after a scratch-namespace fetch, ignored files tolerated, no landed-ancestry requirement), handoff gates with validated recorded reads, the `/proc` cwd/exe/fd slot-use scan, evidence + checkout-evidence-refreshed events with replay-exact payloads, the FINISH journal and path fence, atomic no-follow worktree registration repair, archive publication, and the global-validation epilogue.
- Explicit Python-owned refusals: host-context removal, validate slots/batches, live-owner consent, coordinator attestation (including the unregistered-agent attested stop), push/bundle/drift salvage, configured cache globs, initialized submodules, legacy HANDOFF.md quarantine, handoff sidecar retirement, and slot images.
- Accepted divergences: the Rust `/proc` use scan is weaker evidence than Python's lsof+mounts model; refreshed records keep recorded containing refs and move only HEAD (matching Python `_reclaim_preconditions`); journal deletion mirrors Python `_clear_journal`.
- Differential: `py/wrkslots/tests/test_remove_write_rust.py`, 11 tests, including bound/unbound liveness, sidecar presence, ignored files, genuinely unpublished commits via a separate bare remote, and replay of the produced event log through the Rust `rebuild` reader. 27 writer differential tests pass together (remove + finish + create); `wrkslotsd` package tests 88 passed; clippy `-D warnings` and strict mypy clean.
- Gate 52 (`/tmp/rust-campaign-gate52.log`) selected steps: every failure was rerun alone. Infra (113 passed), vibe-talk keyboard and read-aloud, and the wrkslots writer differentials pass. Remaining host-environment failures, proven unrelated to this diff (it touches no Python lifecycle code): the unrelated host process at `/proc/1186775/fd/26` raises ENAMETOOLONG in Python's process-use census, failing `packages.standalone.wrkslots`, the Python side of `test_remove_matches_python` on the host (it passes under a PID namespace, 11/11), and two lifecycle host tests whose censuses observe unrelated host processes. Those steps cannot pass on this host until that unrelated process exits.

### Slice 37 landed: `transfer-owner` writer (fixture-guarded)

- Commits: `821019a8` (writer + differential + #618 filed), `3a849d4f`, `24074e82`, `7ec2657c` (review-driven test strengthening and payload scrubbing), `b6520e48` (#618 client-name scrub).
- Rust `wrkslotsd transfer-owner` covers the ordinary handoff: recorded owner provably dead or unrecorded, released generations refused, successor authorized by caller ancestry or by residence in the slot's own cgroup box (systemd-escaped label, unit-tested against the Python `_cgroup_in_slot_box` cases), successor generation re-read before the `owner-transferred` event and ACTIVE rewrite.
- Differential: `py/wrkslots/tests/test_transfer_owner_write_rust.py`, 9 tests comparing full scrubbed event payloads and ACTIVE documents for dead and unrecorded owners, the already-transferred outcome, live/indeterminate/wrong-agent/stale-generation/outsider refusals, and the released-generation refusal (set up by a helper process that publishes, releases, and exits).
- Validation: gate 54 on the final commit; every failure rerun alone. The transfer differential and wrkslots shard 1 pass; `wrkslotsd` package tests 89 passed; clippy/mypy clean. Remaining host-only failures are the documented foreign `/proc` census blocker (#618: unrelated stuck processes 1186775 and 2991802) affecting `packages.standalone.wrkslots`, the Python side of `test_remove_matches_python`, and two Python lifecycle host tests, plus load-flaked `rust.agentctl.test` timing cases in an untouched crate whose subsets pass (host load average ~29 during the gate).
- Next in #562: the recover family, then the primary-edition switch in `validation/editions.json`. The #618 census fix follows transfer-owner in the coordinator-directed order.
