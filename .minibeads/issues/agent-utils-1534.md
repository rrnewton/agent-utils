---
title: 'agents-md-skills-overhaul-plan: audit AGENTS and skills, state the vibe-talk state-sync model, and guard cache-only reads (plan for owner review)'
status: open
priority: 1
issue_type: task
created_at: 2026-11-14T16:31:05.000000000+00:00
updated_at: 2026-11-14T16:31:05.000000000+00:00
---

# Description

[watermelon-full-preview] Owner commission, 2026-11-14: vibe-talk is a state-sync
system. The client talks only to the server; the server is the only gateway to
the chat provider and minimizes upstream traffic by caching as much as possible
and using push subscriptions. That model must be stated once, linked clearly
from `AGENTS.md`, so no read path starts calling the provider directly out of
sloppiness. Trigger: the `vibe-talk recap` server route failed 502 on a
provider rate limit for thread replies, while Discuss Summarize did not go
upstream. This issue is the audit and the overhaul plan only; the owner
reviews the plan before any large edits.

Audit at `ba329b5a` (no builds or tests run; counts executed, bodies opened):

- Routes (A): 126 main-server bindings audited from `vibe-talk/src/http/mod.rs:169`
  — 65 legitimate-gateway, 51 should-be-cache-only, 10 mixed, 0 unclear. Worst
  offender is recap: handler `vibe-talk/src/http/api.rs:2463` to
  `vibe-talk/src/ops.rs:6951`, walking `ops::timeline` at `:6984` (up to 50 plus
  25x50 foreground reads) instead of the cache-only `catchup_fetch` walk at
  `:6769`; a rate-limit answer becomes 502 at `vibe-talk/src/http/api.rs:116`.
  Root cause is one gate: `may_serve` is Write-only (`vibe-talk/src/ops.rs:530`),
  so Read-token reads bypass the cache. The CLI process constructs no provider
  client (only production construction `vibe-talk/src/providers.rs:179`), but
  six read commands still cause upstream reads server-side; search is the only
  fully cache-only read command. Discuss is cache-clean; per-message Summarize
  is not on a miss (`vibe-talk/src/ops.rs:7976,8023`).
- State model (B): 18 mechanisms reconstructed (serve, cache-first, stand-in,
  cache-older, SSE subscriptions, 45 s page poll, live poller, event tail,
  relist, server-scheduled refresh, backfill, coverage and merge). No single
  current canonical doc states the model: the archived replication model
  predates the cache-first, scheduled-refresh and cache-older work, and
  #630 state-sync-model is open with its promised design doc absent. Defaults
  collapse the model (cache and serve off, live poll 0, relist 0). Server-
  scheduled refresh is #1227 poll-storm-stale-thread (closed), superseding the
  mechanism of #622 cache-first-reads, not its owner expectation.
- Docs and skills: top `AGENTS.md` is 329 lines; only 3 `AGENTS.md` files exist
  and zero are per-product; `vibe-talk/README.md` is 11708 lines with no
  `vibe-talk/AGENTS.md`. 19 skills total 2235 lines and state handover,
  sender-header and vibe-search rules two to three times each. Verification
  today is seeded reference-model tests, one exhaustive checker and
  Python-vs-Rust differentials; no TLA, property-framework or fuzzing
  framework files exist, which must never be broadened to "no model testing"
  (cache, coverage and search seeded tests are exactly that, hand-rolled).

# Design

Target structure (full outlines and tables in the harness audit report for
this issue; this section is the decision record):

- Slim top `AGENTS.md` to 50-80 lines of repo law only (boundary, publication,
  validate, naming and attribution, messaging, close-what-you-finish, docs
  placement, product map). Mechanics move to a new `validation/README.md` and
  a new repo-landing skill; product rules move to per-product files. Nothing
  may exist only in the old 329-line text.
- Per-product `AGENTS.md` (or pointer stub where no local rule exists) for
  vibe-talk first (detailed: state model in five lines plus canonical link,
  cache-only classes, where routes and cache live, verification pointers,
  sibling exclusions), then agentctl, wrkslots, dagrun, herdr-run, tick-hub,
  wrkviz, merge-queue, agent-teams, pr-landing and shared pointer blocks.
  Each states its canonical-doc home, rendered-copy warning and validation
  entry point. Libraries get index lines only; do not invent rules for them.
- Skills, one activity each: merge the relay skill into agent-teams (after
  settling live-vs-shelved), merge the legacy pane skill into agentctl, merge
  the recap skill into a new vibe-talk skill, split agentctl and the setup
  wizard, create vibe-talk and agent-utils-repo-landing skills, do not create
  a validation-authority skill (replace dangling deferrals with fallback
  text), defer new tool skills without usage evidence. Duplicated rules get
  exactly one owner; others keep short pointers.
- One canonical vibe-talk state-model page: topology and authorities,
  default-off collapse, cache paths, subscriptions and live push, who may
  start an upstream read, consistency guarantees and non-guarantees, verified
  versus not verified with links, and an index of module headers. It
  partially discharges the documentation deliverable of #630 state-sync-model;
  the revisioned-sync implementation remains that issue's.
- Mechanical guard, most robust first: (1) type-level split — a cache-reads
  trait beside `vibe-talk/src/held_replies.rs:43` exposing no provider and no
  cache-write escape, following the existing grant precedent in
  `vibe-talk/src/cache/grant.rs:12`; (2) backstop counting-fake test counting
  every provider method. Grep guards, runtime counters and doc-only rules are
  rejected with reasons in the report. Both halves ride the existing
  `vibe-talk/tests.dag.yaml` test node with no new validation wiring.
- Phased lanes after owner approval (disjoint files within a phase):
  Phase 1 top-agents-slim (M), product-agents-core (M), skills-dedup (M),
  skills-create (M), docs-index-repair (S); Phase 2 vibe-agents-state-doc (M);
  Phase 3 cache-only-guard (M) then recap-read-serve (L); Phase 4
  verify-close-1534 (S). Sibling work on recap replies, cache-only prompts
  and cache completeness is consumed, never duplicated; cross-check it
  before Phase 3 asserts any recap fact.

# Acceptance Criteria

- Owner approves this plan (Phase 0) before any overhaul edit starts; this
  audit lane itself changes no docs or code beyond this issue file.
- Top `AGENTS.md` ends at most 80 lines with every moved rule homed and
  pointed to; every product above has its file or explicit stub.
- The canonical state-model page is the link target from vibe-talk guidance
  and module headers, the archived model is marked superseded for read
  paths, and known README cadence drift is fixed.
- Each duplicated skill rule has one owner and the skills index lists every
  skill including the relay disposition.
- Cache-only reads (Discuss and catchup first) cannot reach the provider
  trait by construction, with the counting test covering all methods.
- Every lane validates with a bare, unpiped `make validate` (close lane with
  `make validate-all`) and records the exact tested commit and result.

# Notes

[watermelon-full-preview] Unverified in this audit, do not over-claim: no test
suite was executed (route inventory, recap tests and seeded model tests are
source-inspected only); the second chat provider was not traced
method-by-method; background upstream consumers were located, not
route-audited; large parts of the vibe-talk README and several minor READMEs
were counted, not read; live deployment cadences are operator configuration,
not in the tree. Separate finding to file or fold into Phase 3, not fixed
here: the mark-read CLI posts a different field name than the REST handler
expects (`vibe-talk/src/http/api.rs:6199`), confirmed by shape only. The five
loopback mock-binary routes are explicitly out of scope for the 126-route
census unless the owner asks for a mini-audit. Formal debts stay with their
own issues (#654, #469, #630, #400, #1415); this plan references them and
must not claim TLA, property-framework or fuzzing coverage that does not exist.
