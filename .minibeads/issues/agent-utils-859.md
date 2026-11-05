---
title: 'wrkslots-unlisted-module-salvage: salvage an unlisted submodule repo and retire a dead owner''s slot instead of refusing'
status: open
priority: 1
issue_type: bug
assignee: mu-validate
created_at: 2026-11-05T00:28:47.850557378+00:00
updated_at: 2026-11-05T00:28:47.850557378+00:00
---

# Description

Retiring a slot whose owner is proven dead is refused when the slot carries a submodule repository that is no longer listed at the current source pins (for example, a module the superproject has since dropped). wrkslots cannot judge an unlisted module repository, so it refuses the whole removal. On one host this blocks disk reclaim for 4 old slots.

# Design

- When the slot's owner is proven dead, bundle-salvage any unlisted module repository with the same treatment as listed modules: a verified bundle in the wrkslots salvage area, with unpushed refs preserved. Then proceed with the removal instead of refusing.
- Keep refusing when the owner is alive, or when owner death is not proven.
- The salvage record names the module path, the bundle and the preserved refs, so the repository can be restored.

# Acceptance Criteria

- Tests: a slot with an unlisted module repo and a proven-dead owner is salvaged (the bundle verifies, refs are preserved) and removed; the same slot with a live owner still refuses with today's text.
- Gate passes.

# Notes

P1, a disk-reclaim blocker reported by the other team via the lead (2026-11-04). The exact refusal text is on the lead host (copy requested). Assigned to mu-validate after #857.
