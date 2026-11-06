---
title: 'wrkslots-fenced-residue-recovery: recover cannot finish a journaled-removed checkout with residue'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-06T18:29:46.465245008+00:00
updated_at: 2026-11-06T20:13:15.241961866+00:00
---

# Description

[watermelon-full-preview] After git worktree remove deletes a checkout registration but leaves ignored residue (.dagrun, caches, ignored trees) in the fenced slot, the finish journal says removed=[checkout] yet recover refuses with 'finished fenced slot contains unexpected entry' on the first leftover, wedging every later removal behind the journal. Fix: when every checkout is journaled removed, no Git registration remains, and the slot is proven unused, recover moves the leftover entries under the local salvage archive with a receipt and finishes the removal. Any other unexpected entry still refuses.

# Acceptance Criteria

The interrupted-removal scenario with residue in the fenced slot recovers: active row archived, fenced dir gone, residue preserved under wrkslots-salvage with a receipt naming its entries; an unexpected entry for a checkout not journaled removed still refuses.
