---
title: 'wrkslots-eden-finish-per-checkout-drafts: Eden finish must account only the drafts of its own checkout, not every draft in the shared backing store'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-19T18:26:58-05:00
updated_at: 2026-11-19T18:26:58-05:00
---

# Description

[watermelon-full-preview] Consumer-reported blocker at 74549a37 with
f23d3fd0e6 (#1702 wrkslots-eden-finish-accounting): `eden.py
classify_drafts` runs `sl log -r 'draft()'` on the checkout. Every
in-image Eden checkout on a host shares one backing repo, so
`draft()` returns every draft in that store (879 on the reporting
host) and `wrkslots finish` for any in-image Eden slot refuses with
landed=0 open=27 unaccounted=850, naming other lanes' and projects'
drafts. No in-image Eden slot can finish. `draft() & ::.` alone
goes empty after the required `goto <head>`, `sl journal` is
store-wide on an Eden mount, and `--discard-unaccounted` would
record other lanes' commits as this slot's discards.

# Design

[watermelon-full-preview] Approved by the coordinator with three
notes; implemented as approved. A wrkslots-owned per-checkout draft
ledger: at create, heartbeat, write-handoff and finish, wrkslots
unions the draft ancestors of the mount's working-copy parent
(`draft() & ::.`) into `<slot>/.eden-state/wrkslots-drafts/<checkout>.json`
(schema 1, sorted nodes, capture time, last error). Finish accounts
only ledger nodes closed under `successors()` (rewrites), never
`draft()` alone and never descendants. Explicit first capture for
pre-change slots is the verb `wrkslots capture-drafts <slot>`, run
from the slot's draft tip; finish refuses a ledger-less checkout
naming that command, and no touchpoint seeds one silently.
Residual window, as directed: a wholly new stack built off a
public revision and abandoned between captures, with the working
copy never returning to it, is not in the ledger. Those commits
stay in the shared backing store (they are ordinary Sapling
commits there; slot removal removes the mount, not the store), so
finish missing them loses accounting, not the commits themselves;
`capture-drafts` from that stack's tip still records them. No
bookmark mechanism is added.
Journal question (stage B step 1, settled without a FUSE mount):
an Eden mount's Sapling repo dir is per mount (its `shared` file
names only bookmarks), so Sapling journal storage is structurally
per working copy; no scratch Eden mount was created and the
journal is not used for attribution, because the consumer report
states the Eden-visible journal is store-wide and only a real
mount could settle that conflict.

# Acceptance Criteria

- Coordinator approves the design in Notes before any code.
- A finish for one Eden checkout accounts only that checkout's own
  drafts: another lane's draft in the same backing store never
  blocks it, is never listed as its unaccounted work, and is never
  recorded as its discard.
- The checkout's own unlanded draft blocks finish even after the
  working copy moves to a public revision, including a draft stack
  created and left between wrkslots touchpoints where the chosen
  mechanism proves it, and an amended draft is followed to its
  successor.
- Checkouts created before the change keep failing closed until
  they are explicitly scoped; they are never silently passed and
  the store-wide list is never auto-attributed to them.
- At least one test uses real Sapling with two working copies
  sharing one backing store and fails on the old code; the Python
  edition is covered and the Rust edition is checked for parity.
- No new persisted record field, file or event kind is implemented
  before the coordinator rules on the reader-skew report in Notes.

# Notes

[watermelon-full-preview] Lane mu-edendrafts/1925. Reader skew: a
consumer pins an old wrkslots reader (writer_ceiling 2); any new
persisted field, file or event kind is reported to the coordinator
before implementing. Prior work: #1702 wrkslots-eden-finish-accounting
(f23d3fd0), #1803 wrkslots-eden-cap-counts-in-image-by-state-dir,
#1804 wrkslots-eden-multiple-backing-repos (lane mu-edenfix).

[watermelon-full-preview] Stage B implemented (pending the
coordinator's cold review before landing). No new record field and
no new event kind: the only new persisted thing is the sidecar file
above, which no record reader parses. Old-reader proof against a
scratch Eden slot holding the sidecar, with fake eden/sl: from 74549a37 and from
096a8d74, status, doctor and audit all exit 0 with no sidecar
finding (the one doctor finding and the audit owner-live BLOCKED
line are identical on the new code and are fake-daemon artefacts).
74549a37 finish still refuses store-wide (unaccounted=2, naming
the other lane's draft) — fail-safe. 096a8d74 predates #1702 draft
accounting entirely: its finish passes with unaccounted=0 with
and without the sidecar, so that pass is pre-existing blindness at
that pin, not an effect of the file. New-code finish on the same
scratch refuses unaccounted=1, naming only the own draft.
`capture-drafts s1` on the scratch reports nodes=1,
store_drafts=2, not_attributed=1 and writes only the own node.
Rust: no Sapling draft classification exists in rs/wrkslotsd
(finish accounting for Eden is Python-only there), and the sidecar
needs no Rust record change; test_cli_compat_rust passes.
