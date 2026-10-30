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

### Slice 38 landed: `recover-unbound-owner` writer (fixture-guarded)

- Commits: `3141c422` (writer + differential), `50ca89ae` (clippy allowance), `a7a82835` (review fixes), rebased over unrelated vibe-talk commits with no shared files.
- Rust `wrkslotsd recover-unbound-owner` records coordinator recovery evidence for a historical agent slot with no recorded owner: non-empty note/validation, coordinator generation re-verified under the locks, historical-owner refusal, registered-liveness dead requirement (shared bound protocol from the remove writer), freshly verified checkouts via finish's checkout-safety check, and the `owner-recovery-recorded` event with the recovery handoff. Validate-type slots refuse explicitly (their liveness authority is the validation-run evidence, still Python).
- Differential: `py/wrkslots/tests/test_recover_unbound_write_rust.py`, 4 tests with full scrubbed payload comparison, a stale recorded HEAD genuinely refreshed per-root, and no-event assertions on refusals.
- Gate 56: infra and vibe-talk screenshots pass on solo rerun; wrkslots shards pass except the three remove tests blocked on this host by the same-device foreign descriptor (#618 residual, passing in a PID namespace, 15/15 remove+recover tests); agentctl timing flakes remain load-related in an untouched crate. `wrkslotsd` 90 package tests pass; clippy/mypy clean.
- Next in #562: the remaining recover family (recover-absent rows, ownerless-agent recovery, the main `recover`), then the opt-in edition switch.

### Slice 39 landed: `recover-absent-agent-row(s)` outcomes (fixture-guarded)

- Commits: `046bce36` (commands + differential), `9739ff79` (apply flag id), `8ef408bf` (review fixes), rebased over unrelated commits with no shared files.
- Rust covers the non-destructive outcomes: already-recovered recognition bound to the exact archived record and its recovery event (revision continuity, evidence keys), interrupted-journal would-resume with the journal validated as the exact durable plan, other-journal refusals, generation/record-digest validation, still-has-physical-storage refusal, and the 128-row batch JSON shell including Python's planned/resume row line and batch-level recovery-required state errors. The destructive census-and-salvage proof path (host/process/systemd censuses, submodule salvage, rescue-ref pushes, registration removal) refuses explicitly as Python-owned once storage is proven absent.
- Differential: `py/wrkslots/tests/test_recover_absent_write_rust.py`, 9 tests. Gate 58: wrkslots shards pass on solo rerun (554 + 554), infra passes alone; remaining host-only failures are #651 part 2 (standalone under /tmp) and the stuck pidns-init lifecycle test. `wrkslotsd` 90 package tests pass; clippy/mypy clean.
- Next in #562: `recover-absent-validate-rows`, the ownerless-agent recovery family, and the main `recover`, then the opt-in edition switch.

### Slice 40 landed: `recover-absent-validate-rows` outcomes (fixture-guarded)

- Commits: `152953bc` (command + differential), `5aecfdeb` (review fixes), `fc9bd9b5` (nits), rebased over unrelated commits with no shared files.
- Rust covers the input contract (bounded schema-1 JSON input opened with O_NOFOLLOW|O_NONBLOCK and a hard cap, exact row identities, input digest), the local-machine check, interrupted-batch journal refusals, already-recovered recognition bound to the exact archive record and recovery event with ARCHIVED derived from the event log, row type/generation/digest validation, the still-has-physical-storage refusal, and Python's refusal outcome payload on stdout. The validation census plan/apply path refuses explicitly as Python-owned.
- Differential: `py/wrkslots/tests/test_recover_validate_write_rust.py`, 8 tests including recovery-before-later-archive recognition. Gate 59: wrkslots shards pass on solo rerun (557+1 flake documented in the transfer differential, which passes 9/9 alone; shard 1 558 passed), infra and vibe-talk page slice pass alone; remaining host-only failures are #651 part 2 (standalone under /tmp) and the stuck pidns-init lifecycle test. `wrkslotsd` 90 package tests pass; clippy/mypy clean.
- Next in #562: the ownerless-agent recovery family and the main `recover`, then the opt-in edition switch.

### Slice 41 landed: `recover-ownerless-agent-worktree` plan outcomes (fixture-guarded)

- Commits: `1d8b49ec` (plans + differential), `744d1ba4` (clippy nit), `ff171a23` (adversarial-review fixes), rebased over unrelated vibe-talk/minibeads commits with no shared files.
- Rust `wrkslotsd recover-ownerless-agent-worktree` covers the read-only plan outcomes of the Python command: the local-machine check, interrupted-journal resume planning with the journal validated as the exact durable plan (schema, machine, slot, fenced shape/parent, phase set, authorization keys and identity triple), registry collision refusal, target resolution mirroring Python's relative-inside rule (relative spelling only, no parent components, no symlink components), repository binding with managed repositories allowed, and the worktree facts proof: directory and Git identity, HEAD/branch, HANDOFF digest binding, ordinary history/index (grafts, shallow, replacement refs, assume-unchanged, skip-worktree), the full remote authority (no push URLs, no push divergence, no dash-leading or empty URLs), and detached-HEAD containment against freshly fetched remote heads. The destructive apply path (salvage, fencing, registration removal, process/systemd censuses) refuses explicitly as Python-owned.
- Differential: `py/wrkslots/tests/test_recover_ownerless_write_rust.py`, 7 tests including grafts, push-URL, absolute-path, and minimal-journal refusals. `wrkslotsd` 90 package tests pass; clippy `-D warnings` and strict mypy clean.
- Gate 60 (`/tmp/rust-campaign-gate60.log`): 197 steps passed; every failure rerun alone on the final commit. Infra passes alone (113), wrkslots shard 1 passes alone (561), shard 0 passes except one load-flaked liveness test that passes alone, and the agentctl Rust suite's two timing flakes pass alone in an untouched crate. Two steps fail only on foreign host state: `packages.standalone.wrkslots` on the unrelated `/proc/1186775/fd/26` ENAMETOOLONG descriptor (passes in a PID namespace) and lifecycle host shard 1 on the stuck foreign pidns-init PID 2991802 (#651 part 2 territory).
- Next in #562: the remaining ownerless family (`recover-ownerless-agent-clone`, `recover-ownerless-agent-cache`, `recover-ownerless-validate-batch`), the main `recover`, then the opt-in edition switch.

### Slice 42 landed: `recover-ownerless-agent-clone` plan outcomes (fixture-guarded)

- Commits: `13ca8afe` (plans + differential), `83ad2c5e` (CLI surface test).
- Rust `wrkslotsd recover-ownerless-agent-clone` covers the plan outcomes that do not depend on the full clone inventory: the local-machine check, interrupted-journal resume planning with the journal validated as the exact durable plan (exact keys, authorization identity and digests, fenced name, phase set, rescue/record shapes, directory identity as dev/inode/mount-id), registry and hold collisions, target binding, and the root clone identity facts (standalone Git directory rather than a linked worktree, existing HEAD, clean checkout with the proof's tree configuration pinned, root HANDOFF digest binding). The inventory-bound plan and the destructive apply path (rescue pushes, fencing, removal, censuses) refuse explicitly as Python-owned.
- Differential: `py/wrkslots/tests/test_recover_clone_write_rust.py`, 6 tests. `wrkslotsd` 91 package tests pass; clippy/mypy clean.
- Gate 62 (`/tmp/rust-campaign-gate62.log`): 200 steps passed; the only failures were infra and wrkslots-shard timeouts under load, each passing alone (infra 113; shards 565+1 heartbeat flake and 565+1 liveness flake, both passing alone).
- Also landed since slice 41: #651 part 2 (`1b3e01d2`, closed) — find-census ENAMETOOLONG diagnostics now use the #618 device proof; standalone and lifecycle host steps pass in-gate on the loaded host.
- Next in #562: `recover-ownerless-agent-cache` and `recover-ownerless-validate-batch` plan outcomes, the main `recover`, then the opt-in edition switch.

### Slice 43 landed: `recover-ownerless-agent-cache` and validate-batch plan outcomes (fixture-guarded)

- Commit: `4c72b5ad` (plans + differential + CLI surface).
- Rust `wrkslotsd recover-ownerless-agent-cache` covers the plan outcomes: local-machine check, interrupted-journal resume planning, registry collision, destination-exists refusal, and the exact cache tree shape (validate/cache/rust-script only, no symlinks, special files, or authored-work markers). The relocation and the process/systemd censuses in the tree proof refuse explicitly as Python-owned. For `recover-ownerless-validate-batch` and `classify-ownerless-validate-batch`, Rust reproduces the argument-shape contract (exactly one target kind, aligned counts, proof alignment, batch limit of 8, duplicate checkouts, coordinator authorization shape) and then refuses explicitly; their shared census and recovery stay Python-owned.
- Differential: `py/wrkslots/tests/test_recover_cache_write_rust.py`, 8 tests. `wrkslotsd` 91 package tests pass; clippy/mypy clean.
- Gate 63 (`/tmp/rust-campaign-gate63.log`): 200 steps passed; only infra and wrkslots-shard timeouts under load failed, each passing alone (infra 113; shard 1 570; shard 0 567 plus the known transfer load-flake passing 9/9 alone).
- Next in #562: the main `recover`, then the opt-in edition switch.

### Slice 44 landed: main `recover` dispatch outcomes (fixture-guarded)

- Commit: `7bb11a10` (dispatch outcomes + differential + CLI surface).
- Rust `wrkslotsd recover` covers the dispatcher's read-only outcomes: coordinator process capture, the flag-combination contract (legacy/ownerless/slot/discard-stale/abort-import combinations, exactly-one-target, frozen-checkout requirements), partial-state refusal, the no-journal refusal, the ownerless-cleanup already-recorded refusal, and journal schema/machine binding with kind-specific flag checks. Resuming a journal, recovering partial updates, and starting ownerless validation cleanup refuse explicitly as Python-owned.
- Differential: `py/wrkslots/tests/test_recover_main_write_rust.py`, 6 tests. `wrkslotsd` 91 package tests pass; clippy/mypy clean.
- Gate 64 (`/tmp/rust-campaign-gate64.log`): 199 steps passed; failures were infra and wrkslots-shard timeouts plus one vibe-talk browser flake, each passing alone (infra 113; shards 573 + 573; scroll-stability browser passes solo).
- Next in #562: the opt-in edition switch (`WRKSLOTS_ENGINE=rust`, default still Python) and the switch plan to the coordinator; stop before any default flip.

### Edition switch (opt-in) landed: `WRKSLOTS_ENGINE=rust`, default still Python

- Commits: `0d1f057a` (resolver opt-in + editions model + resolver tests), `99378c75` (manifest sort), `5aa6ebbc` (setup/dispatch-wiring launcher inventory).
- `common/bin/wrkslots` with `WRKSLOTS_ENGINE=rust` (or `rs`) now dispatches to the Rust edition (`wrkslotsd` through the new tracked `rs/bin/wrkslotsd` cargo launcher). Unset/empty/`python` keeps the Python primary; the generic `DAGRUN_ENGINE`/`AGENT_UTILS_ENGINE` overrides keep refusing a Rust request for wrkslots (its mirror is not at parity), and an invalid `WRKSLOTS_ENGINE` is a usage error. The default flips only by changing `validation/editions.json`, never by this variable. Smoke-tested end to end on a fixture registry (Rust `parse-state` runs; the default still runs Python).
- Gate 66 (`/tmp/rust-campaign-gate66.log`): the two real failures (tracked-launcher inventories in `setup` and the dispatch-wiring contract) were fixed and verified directly (`./setup rs` OK; wiring/resolver/editions tests pass); the remaining failures were infra/shard timeouts and one adopt load-flake, each passing alone (infra 113; shards 573 and 572+flake).

#### Switch plan (for coordinator trial; default flip needs separate approval)

- Coverage: every reader is at differential parity (config, ACTIVE/ARCHIVED shards, handoff, holds, journals, status, doctor, storage, event-log replay). Writers at fixture differential parity: ensure-states, hold/unhold, heartbeat, write/read-handoff, release, adopt, create (worktree representation), finish, remove (ordinary path), transfer-owner, recover-unbound-owner, recover-absent agent/validate row outcomes, ownerless worktree/clone/cache/validate-batch plan outcomes, and the main recover dispatch outcomes. Still Python-owned, refusing by name in Rust: destructive recovery paths (censuses, salvage, fencing, removal), the clone inventory and its digest, cache relocation, validate-batch census/recovery, slot-image operations, validate-batch seals, image-backed create, and host-context remove corners.
- What changes for existing slots and registries under the opt-in: nothing on disk. Both editions read and write the same durable formats (one Rust type per format; Python validates against the same files the differentials replay). Invocation changes only: the Rust edition has its own CLI surface (subcommand names shared, flags such as `--root`, JSON output). One trial blocker to decide first: Rust writers still require `WRKSLOTSD_FIXTURE_WRITER=1` and refuse any registry without it, so a harness trial today exercises readers fully and writers not at all; lifting that guard for a named trial registry is a deliberate coordinator/owner decision, not part of this switch.
- Rollback: unset `WRKSLOTS_ENGINE` (or set it to `python`); dispatch returns to the Python primary immediately, with no state migration, because the formats never changed. Any state a Rust writer produced under a lifted guard is ordinary wrkslots state the Python edition reads (proven by the replay differentials).
- Proposed trial: coordinator runs the harness with `WRKSLOTS_ENGINE=rust` for read/status/doctor traffic first; report any refusal-by-name surprises; only then decide on a guarded writer trial, and only after that on flipping `primary` in `validation/editions.json`.

### Trial phase-1 stop fixed: Rust readers handle event-log-derived state; transfer flake made deterministic

- Commits: `f56a087d` (readers), `f313feec` (transfer determinism; final rebased SHAs in git log).
- The coordinator's trial stopped at `parse-status`: machines whose state is derived from the append-only event log refused toward `rebuild`. `parse-status`, `parse-holds`, `parse-states`, and `parse-global` now read the active document, archives, and holds from the chain fold like the Python edition (the hold fold is shared with the writer-side helper); only a corrupt chain is captured or refused. Regression coverage copies the live registry's shape (stale snapshot shard beside a complete EVENTS chain); the pair/global/holds derived cases now assert parity instead of refusal. Verified read-only against the live registry: all phase-1 Rust read commands succeed.
- `test_transfer_owner_write_rust.py` was flaky (~1/3 of runs, alone): `previous_record_sha256` binds a record whose checkout HEAD came from wall-clock-timestamped fixture commits and whose heartbeat is refreshed at transfer time. Fixture commits now use pinned dates and the digest is not compared (the record it binds is, field by field); 12/12 runs pass.
- Gate 68 (`/tmp/rust-campaign-gate68.log`): everything passes alone (infra 113; wrkslots shards 578 + 577) except `repository.check.python-types`, which fails identically on untouched `origin/main` in `py/wrkslots/tests/test_census_long_paths.py` (landed upstream in `f7414a71`); reported to the coordinator as a finding, not fixed here.
- The trial script's rollback is now surgical: it removes only `rust-trial-1` artefacts and never restores the state tar automatically; the tar is a manual last resort (`trial.sh restore-backup`). Ready for the coordinator to rerun the trial.

### Switch-plan additions from the live trial (must close before any default flip)

- **Image-backed registry parity gap.** The live phase-2 trial stopped at Rust `adopt`: writers that run storage diagnosis refuse outright when the registry contains any slot-image records (`slot image records are present; storage diagnosis stays with the Python status`), even when the target slot itself is a worktree slot. Before the default flip, storage diagnosis in the Rust edition must handle image records (diagnose them, or scope the proof to the target record the way Python does), so mixed registries work. The live registry is image-backed by default, so this blocks the flip, not just the trial.
- **Sidecar-retirement gap.** Rust `remove` refuses any slot whose handoff sidecar is still present; sidecar retirement (verify the read provenance, retire the artifact) must be ported so the standard write-handoff → read-handoff → remove lifecycle completes in Rust.
- Trial cleanup note: the trial's rollback deleted the scratch source repository while `rust-trial-1` was still registered, stranding it (`repository-evidence-unavailable`). mu-rust cleaned it up with the Python edition (recreated the exact scratch commit, repaired the worktree, transfer/handoff/release/read/remove on the release basis) and confirmed `status` clean with no other slot touched. The script's rollback now deletes the scratch repository only after removal succeeds.

### Default-flip blockers closed: image-backed storage (slice 45) and sidecar retirement (slice 46)

- Commits: `33378e46` (slice 45), `caa84ee2` (slice 46), landed directly on `main`.
- **Slice 45 — image-backed storage in adopt and related writers.** `parse-storage` and `parse-storage-git` no longer refuse a registry containing slot-image records. Each record is validated like `slotimage.read_record` (schema, field types, canonical location, canonical/fence/staging placement); a corrupt or misplaced record produces the oracle's `cannot read the slot image record` refusal rather than silently counting as not-image-backed. A ready/discarding image at its slot path is judged by a full `mount_state` port (mountinfo, FUSE same-file identity, loop/ext4 identity via superblock UUID + statvfs fsid, sysfs backing and udev evidence): only a mount provably holding that slot's own image counts as mounted; a foreign filesystem or sub-bind still refuses with the oracle's detail, including the discard-phase held-device message. Inside a coordinator box the host mount table is unreadable to this reader, so it fails closed as unverifiable. Unmounted records still join the Git repository census (`slot_inspectable = False` parity). The writers sharing the target-storage guard (`adopt`, `finish`, `remove`) inherit all of this. Differential fixtures are built from registries that include image records (adopt test, both storage diagnoses, shared-repository census, corrupt/non-canonical records).
- **Slice 46 — sidecar retirement in remove.** `remove` no longer refuses a present handoff sidecar. Gates mirror `_assert_handoff_read`/`_resolve_handoff_artifact`: intent-without-sidecar refuses (completed vs outstanding remedies), a `write-handoff` sidecar must byte-match a completed owner-authenticated intent (legacy projections must not supersede an intent and need their own provenance-bearing read), an unread sidecar refuses with the read-handoff remedy, and a disappeared read artifact still refuses. After archiving, the sidecar retires to its identity-bound `HANDOFF-RETIRED.*` name via `renameat2(RENAME_NOREPLACE)` + directory fsync and is re-validated; an existing retired artifact is validated against the recorded read digest (idempotent resume). `handoff_state` gained `load_sidecar_artifact`/`validate_sidecar` extracted from `parse_handoff`. Tests drive the real write/read-handoff lifecycle in an owner child process (owner must be the invoking process), plus unread, intent-mismatch, and image-registry removal cases. Legacy `HANDOFF.md` quarantine remains Python-owned by name.
- Adversarial review before landing found and we fixed: any-mount-counts-as-mounted (now identity-proven mounts only), corrupt IMAGE.json silently ignored (now refuses), remove's missing publication gate (now ported), and the Git census skip. Remaining agreed fail-closed wording divergences: schema-1 sidecars and the committed-HANDOFF.md allowance in Python's disappeared-artifact branch.
- Validation: 45 tests across the six affected differential files pass; clippy and strict mypy clean. Gate 70 (`/tmp/rust-campaign-gate70.log`) on the rebased commit: 38 steps passed, 3 timed out under load (validation-graph 122s/120s, wrkslots shards 601s/600s); each passes alone on the same commit (validation-graph 42 passed in 11s; shard 0: 583 passed; shard 1: 582 passed). The two upstream commits that landed during the gate share no files with these slices, so the push proceeded without a second gate.
- Ready for the trial rerun: dry run on the image-records registry copy first, then live (coordinator runs live). Default flip, publishing, and slot release remain coordinator/owner decisions.

### Slice 47 (remove cache policy) and phase-2 dry run on the image-records copy: PASS

- Commits: `092eb96b` (slice 47), `53487624` (adopt fixture date pinning), landed on `main`.
- The first dry run got through create, adopt, heartbeat, write-handoff, release, and read-handoff under Rust with identical Python read-back, then `remove` refused: any configured `cache_globs` sent removal to the Python edition. The live registry config carries `cache_globs`, so this was a third trial blocker, found by the dry run.
- Slice 47: `remove`'s salvage gate now runs the oracle's cache-policy assertion (extracted from `finish` as `assert_cache_policy_untracked`), expands the globs in the checkout, and fails closed only where full nested-repository judging has no Rust counterpart (symlink/mount-crossing cache paths, a nested `.git` inside a cache). Ordinary caches are excluded from the cleanliness status like the oracle. Also fixed a latent flake in the adopt differential: fixture commit dates are pinned (two fixture histories straddling a second produced different checkout heads).
- Gate 72 (`/tmp/rust-campaign-gate72.log`) on `53487624`: all steps pass in some run — shard-1 timed out in the gate (602s) and passes alone (583 passed, 502s); the adopt flake in gate 71's shard-0 is fixed by the date pinning above.
- **Dry-run outcome (mu-rust, 2026-10-29):** copy state was first reset to an empty registry (the earlier stopped runs' `rust-trial-1` could not be removed by either edition: its liveness authority is unverifiable in the copy; the reset kept the config and the `slot-images/agent/existing-image-slot` record). `trial.sh phase2` with `REGISTRY` at the copy: **PASS**. Every step ran under Rust — create, adopt, heartbeat, write-handoff, release, read-handoff, remove ("removed and archived") — Python read the slot identically after each step, the sidecar was retired to `HANDOFF-RETIRED.*` (canonical sidecar gone), the slot directory is gone, and both editions' final status agree (0 active). Ready for the coordinator's live run.

### Live trial PASSED on this harness (coordinator, 2026-10-29)

- Coordinator's live run on `main` `f559d7af`: phase 1 read-only comparison identical between editions; phase 2 on disposable slot `rust-trial-2` (`rust-trial-1` was archived, and finished slot names are never reused; the trial script's `SLOT` is now overridable): create, adopt, heartbeat, write-handoff, release, read-handoff, and removal all ran under Rust with Python read-back identical after every step, status clean afterwards, other slots untouched.
- The default is **not** flipped: the coordinator is asking the owner about a soak period on this harness first. The opt-in `WRKSLOTS_ENGINE=rust` switch remains the way to exercise the Rust edition.
- Campaign continues with the remaining planned Rust editions for the Python-only read commands `audit` and `unpushed` (the phase-1 known gaps), then M5 (`wrkviz`, `parallel-experiment-runner`) after the flip decision. M3 publishing still waits for the owner's OK.

### Slice 48: unpushed diagnosis reader

- Commits: `b7881d34` (reader), `1f7aaec9` (manifest sort), `adafee73` (CLI surface inventory), landed on `main`.
- New `wrkslotsd unpushed` command reproducing the Python diagnosis: registry-storage gate (target findings refuse; other slots retained with the oracle's warning texts), per-checkout worktree/branch verification, isolated remote fetch with authority re-check, and PUBLISHED / UNPUSHED-OR-STALE rows with containing refs and two-readings evidence (same-named ref, touched files, diagnostic diff command, null exactly when published).
- Differential fixture uses a separate bare remote (a source repository acting as its own remote can never be unpushed, since worktree commits move its branch refs directly). Published, unpublished, slot-filter, and branch-changed cases all match Python.
- Gate 75: the only failures were load timeouts (validation-graph, both wrkslots shards) plus the CLI-surface inventory catching the new subcommand name; each passes alone on the final commit (42 in 22s; 586 and 585 passed solo; full `cargo test -p wrkslotsd` 91 passed).
- Next: `audit` (slice 49+, the last Python-only read command): registered-row verdicts first, then cache census, unregistered/journal/seal rows, disk state, and gate output.

### Slice 49: audit census reader (stage 1, agent rows)

- Commit: `45f9db60`, landed on `main`.
- New `wrkslotsd parse-audit` reproducing the Python `audit` payload for agent rows: per-record verdicts HELD / DELETABLE / RECOVERABLE / BLOCKED with the oracle's reason assembly — hold handling via the fixture-root hold loader, owner/liveness/heartbeat diagnosis, owner-release state and void reasons (full sidecar/publication checks), handoff-read gate, storage/checkout verification (worktree/branch/remote), slot-unused census — plus unregistered directory rows, interrupted-journal rows (and the journal override on registered rows), stray Git registration overrides, cache allocated-byte accounting, and the disk / attention / unknown gate summary.
- Deliberately still Python-owned, refusing by name in Rust (later audit stages): validation-row liveness, validation-batch seal evidence, the bounded/resumable cache-state census machinery, nested-Git judging inside caches, legacy HANDOFF.md identity/projection corners, and validation unregistered rows.
- Differential: `py/wrkslots/tests/test_audit_state_rust.py` (6 cases: deletable, held, absent/recoverable, unregistered directory, interrupted journal, cache bytes). Live-measured fields are scrubbed as in earlier slices (`metrics` phase timings, `heartbeat_age_seconds`, disk `free_bytes`); `cache_bytes` is normalized in generic equality because allocated-block counts are filesystem-timing sensitive (one 4096-byte directory block), with a dedicated test asserting both editions count >0 and agree within one block.
- Focused checks: audit differential 6 passed; strict mypy clean on the new test; clippy clean for `wrkslotsd`.
- Gate 77 (`/tmp/rust-campaign-gate77.log`) on the slice commit: 200 steps passed; the 4 failures were load timeouts (validation-graph, cross-skip-accounting, both wrkslots shards). Each passes alone on the same commit: infra pair 113 passed in 14s; shard 0: 589 passed; shard 1: 588 passed on its third solo run (the first two solo runs each lost one rotating, load-sensitive differential — `recover_ownerless` then `hold` — and each of those tests passes alone in ~25s). The two upstream commits that landed before the push are vibe-talk only and share no files with this slice, so the push proceeded without a second full gate.
- Next: remaining audit stages (validation rows, seal evidence, bounded census), then M5 (`wrkviz`, `parallel-experiment-runner`) subject to the coordinator's default-flip/soak direction. Default not flipped; nothing published; slot kept.

### Slice 50: audit seal evidence (stage 2)

- Commit: `a06f214f`, landed on `main`.
- `wrkslotsd parse-audit` no longer refuses when a `VALIDATE-BATCH-SEAL.<machine>.journal` is present. Each seal journal is validated like the Python oracle (exact keys, schema 1, kind, filename/machine agreement, timestamp, actor identity, cleanup targets with relative-inside paths equal to the validation slot directory, permission bits, cleanup identities, batch limit of 8, no repeated slot). A matching seal blocks the registered row with `interrupted validation-batch seal requires wrkslots recover`; a generation mismatch makes the row UNKNOWN with the sealed/active generations named; an unrepresented target publishes its own BLOCKED row with the recorded path; an unreadable journal publishes an UNKNOWN row naming the journal file, so one bad journal hides nothing. Seal reasons insert after journal reasons and before stray-registration reasons, matching the oracle's order.
- Two stage-1 divergences the new fixtures exposed are fixed: unregistered validation rows now report the oracle's census fields (`cache_bytes` counted as 0, complete) instead of an error, and the on-disk scan no longer counts the `validate` directory itself as an agent slot.
- Differential: `test_audit_state_rust.py` now has 12 cases (6 new: matching seal, generation mismatch, unrepresented target, unreadable seal, sealed unregistered validate directory, plain validate unregistered directory), all passing. Strict mypy clean; clippy clean; `cargo test -p wrkslotsd` passes.
- Gate 79 (`/tmp/rust-campaign-gate79.log`) on the slice commit: 39 selected steps passed; both wrkslots shards timed out under load and pass alone on the same commit (shard 0: 592 passed; shard 1: 591 passed). The upstream commit that landed before the push is vibe-talk only and shares no files with this slice, so the push proceeded without a second full gate.
- Still Python-owned, refusing by name in Rust: validation-row liveness (including its nested-Git cache judging) and the bounded/resumable cache-state census machinery. Remaining after audit: M5 (`wrkviz`, `parallel-experiment-runner`) subject to the coordinator's default-flip/soak direction. Default not flipped; nothing published; slot kept.

### Slice 51: audit validation rows (stage 3a, clean-evidence liveness)

- Commit: `27f1344d`, landed on `main`.
- `wrkslotsd parse-audit` now judges validation rows instead of refusing them. Liveness comes from the run authority: a missing stable host identity, a row registered on another machine, and a row whose owner or coordinator lease names another stable host are unverifiable with the oracle's messages. A local row with no retained run handles is `dead` when no active or queued user-systemd unit names its slot, checkout, or fenced paths, read through the same `busctl` unit listing and Unit/Service/Scope property calls the oracle makes. Absent validation rows name the `recover-absent-validate-rows` plan (with the oracle's sorted-key JSON spellings).
- Deliberately still refusing by name: any retained run handle under `ignored/validate/runs` (`audit validation-run handle evidence stays with the Python edition for now`) and unreadable host evidence (`audit validation-run host evidence stays with the Python edition for now: ...`). Nested-Git judging inside validation caches still surfaces through the existing cache error, and the bounded/resumable cache-state census machinery remains Python-owned.
- Differential: `test_audit_state_rust.py` now has 16 cases (4 new: validation deletable, validation absent, validation foreign-machine, and the run-handle refusal, the last Rust-only by design). One full-file run lost the foreign-machine case to transient host state; it passes alone and the full file passes on rerun (16 passed). Strict mypy clean; clippy clean; `cargo test -p wrkslotsd` passes.
- Gate 81 (`/tmp/rust-campaign-gate81.log`) on the slice commit: 38 selected steps passed; validation-graph and both wrkslots shards timed out under load and pass alone on the same commit (42 in 12s; shard 0: 592 passed, 2 skipped; shard 1: 593 passed). Upstream commits that landed before the push share no files with this slice, so the push proceeded without a second full gate.
- Remaining audit corners: retained run-handle evidence and the bounded census (stage 3b), then M5 (`wrkviz`, `parallel-experiment-runner`) subject to the coordinator's default-flip/soak direction. Default not flipped; nothing published; slot kept.

### Slice 52: audit retained run handles (stage 3b)

- Commit: `967d6364`, landed on `main`.
- `wrkslotsd parse-audit` now judges stable retained validation run handles instead of refusing whenever `ignored/validate/runs` contains a JSON file. The census strictly reads and identifies handle files, applies the file-count, byte, replacement, timestamp-settle, and late-change rules, and binds `checkout` and `source_checkout` paths to validation rows, including sibling-row evidence in the batched census. Recorded process generations, live cgroup processes, retained-unit control groups and member counts, active or queued units, and units naming row paths are judged with the oracle's alive/unverifiable outcomes. The host process view and whole cgroup hierarchy are proved before a dead verdict, and process observations bracket the systemd evidence.
- Differential: `test_audit_state_rust.py` now has 24 cases (9 new for handles: unrelated, related, relative path, nested source, dead process, live process, other boot, duplicate field, and mismatched unit). The full file passes (24 passed); strict mypy and clippy are clean; `cargo test -p wrkslotsd` passes.
- Gate 83 (`/tmp/rust-campaign-gate83.log`) on the slice commit: 38 selected steps passed; validation-graph and both wrkslots shards timed out under load and pass alone on the same commit (42 in 11s; shard 0: 598 passed after one solo run lost a transient heartbeat differential that passes alone in under a second; shard 1: 597 passed). Upstream commits that landed before the push share no files with this slice, so the push proceeded without a second full gate.
- Still Python-owned by name: unreadable host evidence in the no-handle case, nested-Git judging inside validation caches, and the bounded/resumable cache-state census machinery. Remaining after audit: M5 (`wrkviz`, `parallel-experiment-runner`) subject to the coordinator's default-flip/soak direction. Default not flipped; nothing published; slot kept.

### Slice 53: bounded cache census for audit (stage 4)

- Commit: `31656949`, landed on `main`.
- `wrkslotsd parse-audit` now runs the bounded cache census instead of an unbounded allocated-byte walk. Work is debited per entry and per directory from the shared invocation budget; wall expiry is the oracle's traversal-timeout error; entries are accounted by lstat; filesystem and mount crossings refuse; nested-Git metadata errors; overlapping globs are deduplicated; symlinked cache candidates refuse. Each root is measured, re-verified against the recorded identities and bytes, and re-stated at finalize before a total may publish. `parse-audit` gains `--cache-census-state`, `--cache-work-limit`, and `--cache-wall-seconds` with the Python defaults and validation messages.
- The census state store is established like the oracle: canonical explicit paths only, a project-keyed default below the cache home with a symlink-free component chain, and an owner-bound 0600 key published without replacement. A fresh-root state envelope (schema `wrkslots-audit-cache-census/v6`, registry revision, HMAC) is persisted after each census, and a persist failure errors cache subjects. Resuming persisted partial progress stays with the Python edition: the Rust census restarts each invocation, which yields identical measurements for stable caches. Validation-row nested-Git disposability judging also stays Python-owned by name.
- Differential: `test_audit_state_rust.py` now has 29 cases (5 new for the census: partial-then-complete across two invocations, flat-cache small limit, nested Git, symlinked candidate, in-project state refusal), and the full file passes (29 passed). An adversarial review found per-directory-only debiting, double-counted directories, missing verify/finalize, followed symlinks, lexical mount detection, XDG and key-candidate divergences, and timeout-outcome differences; all were fixed before landing. Strict mypy and clippy are clean; `cargo test -p wrkslotsd` passes.
- Gate 85 (`/tmp/rust-campaign-gate85.log`) on the slice commit: 38 selected steps passed; validation-graph and both wrkslots shards timed out under load and pass alone on the same commit (42 in 11s; shards 600 and 600 passed). Upstream commits that landed before the push share no files with this slice, so the push proceeded without a second full gate.
- Remaining audit parity corners are now narrow: resume-state reuse, validation-row nested-Git disposability, planning-budget exhaustion inside checkout enumeration, unregistered-slot cache planning, and generic JSON error spellings. The audit reader otherwise matches the oracle on fixtures. Remaining campaign work: M5 (`wrkviz`, `parallel-experiment-runner`) subject to the coordinator's default-flip/soak direction. Default not flipped; nothing published; slot kept.

### Slice 54: unregistered-slot cache planning for audit

- Commits: `d233d825` (slice), `4e6e597f` (rustfmt follow-up), landed on `main`.
- `wrkslotsd parse-audit` now plans caches for unregistered agent slots instead of reporting every one as containing no Git checkouts. Nested-layout entries are each judged (non-directory or symlink entries refuse; each directory must be a Git checkout root, with the oracle's `Git refused in ...` / `Git reported an unsafe repository root` spellings; an empty slot keeps the no-checkouts error); under flat layout the slot itself is the single candidate. Caches are planned under the global `cache_globs` policy alone, and when only `repo_cache_globs` is configured the row carries the oracle's no-trusted-repository-name-mapping refusal.
- The shared planner now judges every glob match of any entry kind, for registered and unregistered subjects alike: tracked-source/submodule cache policy (reusing the finish slice's `assert_cache_policy_untracked` port, with `unregistered-<n>` labels), symlink components, mount components, and mount points crossed or contained (mount table read once per audit). Symlinked directories are descended only where a non-`**` glob component can reach them, as the oracle's walk does, with an ancestor-identity guard against link cycles.
- Differential: `test_audit_state_rust.py` now has 36 cases (7 new: unregistered cache counting, non-checkout entry, file entry, repo-globs-only refusal, tracked cache, symlinked ancestor, symlinked file candidate) and the full file passes (36 passed). Strict mypy and clippy clean; `cargo test -p wrkslotsd` passes (91 + 1).
- Review: one adversarial reviewer probed two HIGH divergences (tracked-source policy never judged; symlink handling covering only the final-directory case) and one MEDIUM (mount candidates not refused at planning); all fixed before landing. The reviewer's journal-slot and planning-budget findings are the named deferrals below, not silent gaps.
- Gate 88 (`/tmp/rust-campaign-gate88.log`): all selected steps passed except `repository.format.rust` (fixed in `4e6e597f`, gate's exact rustfmt command then clean) and both wrkslots shards, which timed out under host load and pass solo on the final commit (603 and 603; shard 0's one live-process timing failure passes alone in 12s). Upstream commits before the push were vibe-talk only, sharing no files with this slice.
- Still Python-owned by name: resume-state reuse, validation-row nested-Git disposability judging, cache-planning budget exhaustion inside checkout enumeration, journal-slot cache planning (an interrupted journal's recorded checkouts), the `**`-through-symlink reachability corner, first-symlink-error choice among several candidates, and generic JSON/OS error spellings. The audit reader otherwise matches the oracle on fixtures. Remaining campaign work: the corners above as small slices, then M5 (`wrkviz`, `parallel-experiment-runner`) subject to the coordinator's default-flip/soak direction. Default not flipped; nothing published; slot kept.

### Slice 55: oracle glob walk and planning budget for audit

- Commit: `ca39d75a`, landed on `main`.
- `wrkslotsd parse-audit` planning now ports the Python glob walk itself (per-pattern positions with `**` closure, symlink following for non-final components only, matches in the oracle's order) instead of an enumerate-everything approximation, and each subject plans under a fair share of what earlier subjects left of the planning work and wall allowances. Exhaustion publishes the oracle's two spellings, including a `{value:g}` wall rendering pinned by a Rust unit test (`fmt_g`). Census argument validation happens before any state is read, as in the oracle.
- Checkout planning now brackets the oracle's directory-identity check (device, inode, mount id) and verifies each checkout against its stored repository (`verify_existing_worktree`); unregistered roots verify as their own Git roots. A journal slot occupies its share position — so its peers' shares shrink exactly as in the oracle — even though planning from its journal stays Python-owned.
- Differential: `test_audit_state_rust.py` now has 39 cases (3 new: single-target work exhaustion, two-target fair share, wrong-repository checkout) and the full file passes (with two rotating host-load flakes passing on rerun). Strict mypy and clippy clean; `cargo test -p wrkslotsd` passes (92 + 1).
- Review: one adversarial reviewer found repository-identity verification omitted (HIGH), journal slots dropped from share positions (HIGH), `fmt_g` corrupting large and scientific totals (MEDIUM), and mid-listing entry errors swallowed (LOW); all fixed before landing.
- Gate 90 (`/tmp/rust-campaign-gate90.log`): 39 selected steps passed; both wrkslots shards timed out under host load and pass solo on the final commit (604 + 604, each with one known load flake that passes alone: the remove cache-glob census test in 49s, the cache-bytes timing test in 11s). Upstream commits before the push shared no files with this slice.
- Still Python-owned by name: resume-state reuse, validation-row nested-Git disposability judging, journal-slot cache planning (an interrupted journal's recorded checkouts), non-UTF-8 entry-name glob matching, and generic JSON/OS error spellings. The audit reader otherwise matches the oracle on fixtures. Remaining campaign work: the corners above as small slices, then M5 (`wrkviz`, `parallel-experiment-runner`) subject to the coordinator's default-flip/soak direction. Default not flipped; nothing published; slot kept.

### Slice 56: nested-Git judging in validation caches for audit

- Commit: `954fe749`, landed on `main`.
- `wrkslotsd parse-audit` now judges every nested repository in a completed validation cache as completed validation removal judges it: `.git` must be a real directory that Git reports as both its repository and common directory, `config`/`config.worktree` set no `include`/`includeIf` or `core.worktree`, and there are no registered linked worktrees or submodule repositories. Judgment runs with the oracle's controlled Git environment (`--git-dir`, no user/system config, grafts and replace objects off) under a 900-second-per-root, 120-second-per-command budget shared with the census wall deadline; a deadline expiry mid-judgment publishes the root partial, never an error.
- Paths cross the Git boundary as OS bytes (`OsString` arguments, byte-compared reported paths), so caches under non-UTF-8 directory names judge correctly; the environment passes through `vars_os` like `os.environ`.
- Differential: `test_audit_state_rust.py` now has 43 cases (4 new: disposable nested repository, `core.worktree` refusal, `.git` file refusal, registered linked worktrees) and the full file passes (rotating host-load flakes pass on rerun). Strict mypy and clippy clean; `cargo test -p wrkslotsd` passes (92 + 1).
- Review: one adversarial reviewer probed non-UTF-8 path mangling (HIGH), judgment-timeout outcome (`error` where the oracle publishes `partial`), a cache root itself named `.git`, swallowed subdirectory listing errors, and an `env::vars()` panic surface; all fixed before landing. Its extra probes (symlink `.git`, `include.path`, `.git/modules`, empty `.git`, `commondir` elsewhere, first-refusal order) matched byte-for-byte.
- Gate 92 (`/tmp/rust-campaign-gate92.log`): every selected step passed except both wrkslots shards, which timed out under host load and pass solo on the final commit (607, and 605 + 2 load flakes passing on rerun in 25s). Upstream commits before the push shared no files with this slice.
- Still Python-owned by name: resume-state reuse, journal-slot cache planning (an interrupted journal's recorded checkouts), non-UTF-8 entry-name glob matching, and generic JSON/OS error spellings. The audit reader otherwise matches the oracle on fixtures. Remaining campaign work: those two slices, then M5 (`wrkviz`, `parallel-experiment-runner`) subject to the coordinator's default-flip/soak direction. Default not flipped; nothing published; slot kept.

### Slice 57a: validated journal reading for audit

- Commit: `0d2aecdd`, landed on `main`.
- `wrkslotsd parse-audit` now reads journal state through the same validation the oracle applies instead of the old filename scan: journal shape validation is shared with the `parse-journal` reader (`validate_journal_shape` was extracted so both readers use one implementation), pending operations are reconstructed from the append-only event log, scoped create/finish episode provenance is judged, a completed create requires the exact stored ACTIVE record, duplicate journals are detected after all validation, and unsupported journal kinds refuse by name. Journal slots now occupy fair-share positions in cache planning exactly as the oracle orders them; planning from a journal's recorded checkouts remains the next slice (57b), and resume-state reuse follows as slice 58, per the coordinator's funding so the audit has no Python-owned corners left before any default flip.
- Differential: `test_audit_state_rust.py` now has 47 cases (4 new provenance cases) and the full file passes (one rotating host-load flake passes alone). Journal-state differentials still pass (35). Strict mypy and clippy clean; `cargo test -p wrkslotsd` passes (92 + 1).
- Review: one adversarial reviewer found the kind subset missing (unsupported-kind refusal), a 1 MiB vs 16 MiB journal read limit and wrong label, unvalidated machine names, duplicate detection interleaved with validation, episode `opens` read with `as_i64` (accepting `false`/`0.0`), and repr-style quoting in refusal messages; all fixed before landing.
- Gate 94 (`/tmp/rust-campaign-gate94.log`): every selected step passed except both wrkslots shards, which timed out under host load and pass solo on the final commit (shard 0: 608 + 1 load flake that passes alone in 13.78s; shard 1: 608 + 1 load flake, the heartbeat differential, passing alone). The push was preceded by a rebase onto five upstream commits (vibe-talk and agentctl only) sharing no files with this slice, so no second gate was owed.
- Still Python-owned by name: resume-state reuse (slice 58), journal-slot cache planning (slice 57b, in progress), non-UTF-8 entry-name glob matching, and generic JSON/OS error spellings. Default not flipped; nothing published; slot kept.

### Slice 57b: journal-slot cache planning for audit

- Commit: `b32d97a8`, landed on `main`.
- `wrkslotsd parse-audit` now plans caches for slots that exist only as validated recovery journals (an interrupted journal's recorded checkouts), constructed in path order from the validated journal's path. Journaled checkouts occupy their fair-share planning positions and plan from the journal record like registered subjects; a missing journaled checkout is skipped while a still-registered one refuses, with the oracle's spellings. The shared `ensure_no_symlink_components` is now missing-tolerant (`Path.is_symlink` semantics), matching the oracle, and the audit-local duplicate was deleted in favour of `config_validate`'s version.
- Differential: `test_audit_state_rust.py` now has 50 cases (3 new: scoped journal cache, missing checkout, registered-missing refusal) and the full file passes (one rotating seal load flake passes alone in 10s). Finish/remove differentials still pass (27). Strict mypy and clippy clean; `cargo test -p wrkslotsd` passes (92 + 1).
- Review: one adversarial reviewer found the missing-checkout tolerance divergence (HIGH), the missing-repository spelling order, and path-order construction issues; all fixed before landing.
- Gate 96 (`/tmp/rust-campaign-gate96.log`): every selected step passed except both wrkslots shards, which timed out under host load and pass solo on the final commit (611 and 610, both rc 0). The push was preceded by a rebase onto two upstream commits (vibe-talk and agentctl only) sharing no files with this slice, so no second gate was owed.
- Still Python-owned by name: resume-state reuse (slice 58, next), non-UTF-8 entry-name glob matching, and generic JSON/OS error spellings. Default not flipped; nothing published; slot kept.
