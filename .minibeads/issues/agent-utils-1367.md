---
title: 'wrkslots-salvage-thin-bundle-fast: remove-agent-batch salvage writes 15 GB self-contained bundles and holds the registry lock for minutes per clean slot'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- salvage
- lock-scope
created_at: 2026-11-12T13:30:00.000000000+00:00
updated_at: 2026-11-12T13:30:00.000000000+00:00
---

# Description

[watermelon-full-preview] `wrkslots remove-agent-batch --archive-private-history` spends 120-171 s per CLEAN slot (over 300 s on some), all inside the registry lock hold, blocking every other wrkslots mutation (including create). Measured 2026-11-12: 5 slots in 39 min; 93 more would take 5-12 h. Almost all of it is salvage of the nested consumer checkout (a submodule): a SELF-CONTAINED bundle of ~15 GB of history verified with a fresh `git clone --bare`, plus 4 lsof scans of ~150 s.

Related live-journal defect (split to its own lane): while a removal is IN FLIGHT, other mutations refuse with 'interrupted mutation recorded in ACTIVE.<host>.journal; run wrkslots recover first (... the journal records no actor process generation)' even though the journal belongs to a LIVE removal; it should wait (honouring --wait-lock) or say 'busy: live <op> by pid N'. That fix lands separately; removal journal changes here rebase on it.

# Design

- (1) Thin bundle (#1153 option c): use origin/main (or the upstream tip) as the bundle prerequisite so only private commits go in. Prerequisites must be proven reachable from a ref on the remote (or published salvage) at salvage time (ls-remote), else fall back to a self-contained bundle. `git bundle verify` against a store that has the prerequisites must pass before anything is deleted. Keep #1278's promisor-gap exclusion and missing-object refusals; never delete an unaccounted commit. #1278 chose complete-history self-contained bundles deliberately (bundle verifies in an empty repository); thin bundles trade that for speed only when the remote proof holds.
- (2) Do salvage and verification OUTSIDE the registry lock; release the lock between slots in remove-agent-batch. Folds #1213 wrkslots-state-lock-deletion for the remove-agent-batch path (say so in both issues). Pregather salvage facts + census lock-free (pattern of #1194/#1287/#1276), take the lock only for journal/fence/record commits, revalidate fingerprints under it.
- (3) One lsof scan per slot, not 4: one process-table/open-file census reused by every check (shared census of #1175 extended to all use checks of the removal).
- Rust parity: if the Rust edition has the same path (salvage_gate etc. in rs/wrkslotsd/src/remove_write.rs), keep the Rust/Python differential exact.

Landing order (coordinator ruling 2026-11-12): (3) single census, then (2) lock narrowing, then (1) thin bundle, each on its own gate.

# Acceptance Criteria

- (3) One lsof//proc census per slot per removal (counted, not timed); every use check reuses it; a process entering between gather and fence still refuses.
- (2) No bundle create/verify, census, or storage deletion runs while the ACTIVE registry locks are held during remove-agent-batch (counted lock-held operations); the lock is released between batch slots; a slot changed between lock-free gather and locked re-check refuses by name and the batch continues.
- (1) A clean slot whose private history is small bundles only private commits when the upstream tip is proven on a live remote ref; bundle verifies against a store holding the prerequisites before deletion; without that proof the self-contained bundle is written as today. Promisor-gap kept commits are still excluded and recorded; other missing objects still refuse naming commit, object, and stores searched.
- Python and Rust produce identical journals and receipts on the fixtures; existing lifecycle, lock-scope, and differential suites pass.

# Notes

Reported 2026-11-12 by a consumer's lead (blocking slot teardown). That consumer pins agent-utils 14bdac8225 (2026-11-11): it already has #1278 and #1280 but not #1287 retire-lock-narrow, so its remove-agent-batch still holds the lock across salvage. #1153 deferred (c) after (b) deduped bundles per commit; the 15 GB nested-checkout history makes the full bundle itself the cost now. See #1153, #1213, #1278, #1280, #1287.

## Update

[watermelon-full-preview] Piece (2) folds in #1213 wrkslots-state-lock-deletion for the remove-agent-batch path; #1213 stays open for its other removal shapes. Piece (3) landed as 191aa5cc.
[watermelon-full-preview] Satellite landed: the live-journal busy fix (removal journals record their actor; concurrent mutations report busy/live instead of interrupted) landed separately as d980adb453a5ee6d9185423dd68a620a50de5586 on github/main (mu-wsjournal lane). Removal changes here rebase on it.
[watermelon-full-preview] Piece (2b): ordinary `wrkslots remove` gets the prepared treatment too, and the configuration flock is released after the config check in prepared commit/completion rounds. This supersedes #1213's "keep #1194 configuration-lock behavior unchanged" for remove (recorded in both issues).
