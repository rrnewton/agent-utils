---
title: 'wrkslots-ignored-evidence-salvage: removal refuses to delete gitignored files unless they are archived or explicitly discarded'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-11T15:04:35+00:00
updated_at: 2026-11-11T15:04:35+00:00
---

# Description

[opus 5.5] wrkslots salvages committed and private Git history, but never gitignored files. In a consumer's removals of three slots on 2026-11-11, two slots held evidence under `ignored/evidence/` that the consumer's issues cited, and it is now gone. The third slot's ignored tree survived only because it was copied out by hand first. Removal deleted those files without warning.

# Design

Deterministic rather than heuristic:

- By default, `remove` (and every path that deletes a slot's checkouts) REFUSES when any checkout in the slot holds gitignored files outside the known regenerable caches. Those caches are: cargo `target`, `node_modules`, page build trees, and nested agent worktrees once their own checks have passed. The refusal names each checkout and the top-level paths found, with sizes.
- `--archive-ignored` bundles those files into the slot's salvage archive (a tar beside the repository bundle, size-capped with a clear refusal above the cap), lists every archived file with size and sha256 in the receipt, and verifies the archive before deletion.
- `--discard-ignored` deletes them explicitly, and the receipt still lists what was discarded.
- The regenerable-cache list shares one definition with `clean-caches`.
- A nice-to-have warning only, not the safety mechanism: flag ignored paths that are referenced from issues, ai_docs or reviews in the source repository.
- Python and Rust `remove_write.rs` together, keeping the differential exact.

# Acceptance Criteria

- A fixture slot with an ignored evidence file refuses removal by default, naming the file.
- `--archive-ignored` archives the file, lists it in the receipt with its sha256, verifies the archive, then removes.
- `--discard-ignored` removes and lists the discarded file.
- Cache-only ignored content (target, node_modules) never blocks removal.
- Tests use no wall-clock budgets.

# Notes

Lead ruling 2026-11-11: P1. Design, fixtures and tests first; gate only after #1287 lands, rebased on top, because #1287 and the /proc-scan P0 are both in the remove path of py/wrkslots/cli.py. Interim rule for both teams until this lands: before any slot removal, run `git status --ignored --short` in each checkout and hand-archive anything that isn't a regenerable cache.

A transient seen in the same run: one attempt refused with systemd-run "Remote peer disconnected" (the host handoff failed; nothing changed). A clearer message or a bounded retry would help.

## Resolution

Removal now judges gitignored files before salvage in both editions. By default `remove` refuses when any checkout holds gitignored files outside the regenerable caches (the cache globs `clean-caches` uses via `_cache_globs_for`, a checkout's own cargo `target` when no per-repository policy is configured, and marker-proven nested cargo targets), naming each checkout, file and size; a bounded warning-only scan also names ignored paths referenced from `.minibeads/issues`, `ai_docs` or `reviews`. `--archive-ignored` writes a deterministic USTAR tar beside the salvage bundles (512 MiB total cap, refused above it), verifies every member's sha256 on read-back, and lists each file with size, sha256 and disposition in the salvage receipt; `--discard-ignored` deletes explicitly with the same listing. The Python salvage recheck re-verifies the inventory and archive before deletion, and Rust `remove_write.rs` `salvage_gate` produces the identical receipt fields and byte-identical tar, keeping the differential exact. `slotimage._run_on_host` retries a systemd-run "Remote peer disconnected" once and then reports that the handoff did not run and nothing changed. Validation-slot removal and ownerless-worktree recovery keep their own ignored-residue contracts and are unchanged.
