---
title: 'clean-caches-nested-targets: clean-caches misses cargo target dirs below the slot root'
status: open
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-08T02:40:00.000000000+00:00
updated_at: 2026-11-08T02:40:00.000000000+00:00
---

# Description

`wrkslots clean-caches --dry-run` reported 0 bytes for a consumer's slot holding 76.4 GiB of cargo targets, and missed most of another slot's 152 GiB. The configured cache globs match only targets at the slot ROOT (and the fixed checkout paths). They miss nested targets such as <slot>/<repo>/target, <slot>/temp/<x>/target and <slot>/<repo>/temp/cargo-target, so the regenerable bytes the owner says may always be reclaimed stay behind.

# Design

- Find cargo target dirs slot-wide: a directory holding CACHEDIR.TAG (the cargo signature) together with a .fingerprint subdirectory, or a profile dir with one. Depth-bounded (configurable, default ~6), never following symlinks, staying on the slot's filesystem.
- Report each target per path with its size in --dry-run and the human/json reports. Delete per path, under cargo's own lock where present.
- Keep the configured globs for non-cargo caches.
- Honour #1029 clean-caches-skip-running-gate: skip any slot with a running or queued gate.
- Both editions (py/wrkslots and rs/wrkslotsd), with a parity test.

# Acceptance Criteria

- A fixture slot with nested targets at the three shapes above is reported per path with correct sizes, and cleaned.
- A symlink to a target outside the slot is not followed, and depth beyond the bound is not searched.
- A slot with a running gate is skipped (shared with #1029).
- Both editions agree.

# Notes

Reported by a consumer team on 2026-11-07. Bundled with #1029 for the same lane.
