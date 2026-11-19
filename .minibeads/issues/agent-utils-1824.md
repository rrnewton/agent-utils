---
title: 'wrkslots-submodule-remote-refusal-names-reason: name why a submodule remote refusal fired, with the URLs involved'
status: closed
priority: 0
issue_type: bug
assignee: mu-mirrorreason
labels:
- wrkslots
created_at: 2026-11-17T00:00:00+00:00
updated_at: 2026-11-19T18:52:15-05:00
---

# Description

[watermelon-full-preview] `py/wrkslots/cli.py`, `_submodule_network_authority`, refuses a
slot submodule whose remote cannot be judged with one generic sentence:
"submodule {name} remote {remote} differs from its configured source; preserve
the slot for inspection". The sentence does not say why the judgement failed,
so a consumer lost an hour discovering that their slot submodule's origin was
a local mirror whose own origin named a fork
(`https://github.com/<owner>/reverie.git`) while the source and `.gitmodules`
named the upstream (`https://github.com/<other>/reverie.git`).
`_mirrors_source_remote` correctly refused; only the message was missing.

# Design

- Keep the refusal decision exactly as it is: no acceptance change, no new
  persisted fields, message text only.
- Name the reason and the URLs involved for each failing case: the slot remote
  names a different repository than the source remote (both URLs shown); the
  slot remote is a local mirror whose own origin names a different repository
  ("the mirror <path>'s origin names a different repository: <mirror origin
  url> (source remote: <source url>)"); the mirror has no origin; the mirror
  path is not a repository; the source's own remote is not one that can be
  judged under the existing rule about where it must lead.
- Add one remedy line: "set the slot submodule's origin to the .gitmodules URL
  (git -C <checkout> remote set-url origin <url>), or point the mirror at that
  repository".
- Mirror the message in the Rust edition if `rs/wrkslotsd` has the same check
  (search "differs from its configured source"), keeping both editions' texts
  identical where both exist.

# Acceptance Criteria

- One test per failing case: the refusal text names the reason and the URLs
  involved, and carries the remedy line.
- Existing refusal tests stay green, with expected text updated only where
  they asserted the old wording.
- Python and Rust parity where the Rust edition has the same check.
- Tests use neutral example URLs only; no real owner or repository names.

# Notes

## Resolution

[watermelon-full-preview] Landed on main as `48efd5c7` (tip `393060bc`) on
2026-11-19. `_submodule_network_authority` keeps its exact accept/refuse
decision; the new `_mirror_refusal_reason` names the failing case with the
URLs involved (different repository with both URLs; mirror origin naming a
different repository; mirror with no origin; mirror path not a repository;
source remote that cannot be judged) and every refusal carries the remedy
line (`git -C <checkout> remote set-url origin <url>`, or point the mirror at
that repository). Five new tests, one per case, fail on base and pass after.
The Rust edition has no matching check, so this is Python only.
python.wrkslots.shard-4 and shard-7 were waived by the coordinator on the
gated tree (pure CPU-TIMEOUT, zero failing tests; see #1926
wrkslots-shard-cpu-budgets-hot-host), and the combined landed tree passed
py_compile, strict mypy, the overlap commits' tests (21), the lifecycle
census selection (291) and the client-names check.
