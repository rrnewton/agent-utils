---
title: 'voice-page-modularity: split voice.js into TypeScript modules per the final plan'
status: closed
priority: 3
issue_type: epic
labels:
- vibe-talk
depends_on:
  agent-utils-619: blocks
  agent-utils-620: blocks
  agent-utils-621: blocks
  agent-utils-622: blocks
  agent-utils-623: blocks
  agent-utils-625: blocks
  agent-utils-627: blocks
  agent-utils-650: blocks
  agent-utils-892: blocks
created_at: 2026-10-28T12:47:41.393557128+00:00
updated_at: 2026-11-15T23:36:21.000000000+00:00
closed_at: 2026-11-15T23:36:21.000000000+00:00
---

# Description

[opus 5.5] Implementation of #623 voice-page-modularity-plan's final plan: mechanical, behaviour-neutral steps, each landed green. Owner 2026-10-28: no hurry; start when other voice-page work settles. This issue is blocked by #623 and by every in-flight lane that edits vibe-talk/web/voice.js (each lane adds its own issue as a blocker); 'mb ready' lists it once all of them are closed.

# Notes

## Resolution

[muse watermelon] Closed 2026-11-15. The final entry (step 3) landed on
github/main in three commits and is deployed:

- 8c2669f663 "vibe-talk: the page's entry is a module (final entry)":
  web/voice.js moved verbatim to web/src/app/main.js and deleted, the legacy
  prelude files (web/src/legacy/imports.ts, web/legacy-imports.d.ts) deleted,
  `takePairingCode()` again the entry's first statement.
- 84e7508fc3 "vibe-talk: emit the entry as a classic script after a generated
  prelude": the page build bundles the modules the entry imports in the old
  strict-IIFE prelude, generated from the entry's own import declarations
  (web/src/app/entry-imports.ts), and emits the entry body as a classic
  strict script with its imports blanked, so top-level const/let keep their
  temporal dead zone and no module-internal name reaches the page.
- 870f244a36 "vibe-talk: refuse a stale entry-imports module at build time":
  the build refuses a generated imports list that no longer matches the
  entry; carries the rustfmt wrap at tests/page_prebuilt.rs.

Evidence chain: rev624 pre-land review READY after the B1 build correction
(plus a clean delta check); contained --all gate on f1c5c569 271/272 with the
only red vibe-talk.check.fmt at tests/page_prebuilt.rs:78 (fixed by the
cargo fmt wrap in 870f244a36); path-selected re-gate on 385cf4da 174/174
"validate: OK"; paired burner load checks tip vs parent 3x 9/9 and, on the
landed tree, 1x 3/3 (fold-steps, reply-continuity, typing-scroll); 214
before/after screenshot pairs (phone, desktop, wide) identical apart from
clock and build-hash text; deploy e4f50735 carrying step 3 live since
5:59 PM ET with its 15-minute health watch clean (healthz ok, 0 errors,
panics or restarts). The emitted asset differs from its parent's only in
shape (+79 bytes; top-level const/let preserved; name delta exactly the
entry's imports).

Deliberate remainder: the entry lands as non-strict JavaScript (checked as
voice.js was); its strict TypeScript conversion is #1523
voice-main-strict-ts, whose first slice also makes a deleted
entry-imports.ts refuse the build instead of building without a prelude.
