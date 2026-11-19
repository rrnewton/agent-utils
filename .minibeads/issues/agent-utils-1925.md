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

Design sent to the coordinator before coding; implementation waits
for the coordinator's OK (see Notes). No code yet.

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
