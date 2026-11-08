---
title: 'clean-caches-nested-targets: clean-caches misses cargo target dirs below the slot root'
status: open
priority: 1
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-08T02:40:00.000000000+00:00
updated_at: 2026-11-08T09:30:00.000000000+00:00
---

# Description

`wrkslots clean-caches --dry-run` reported 0 bytes for a consumer's slot holding 76.4 GiB of cargo targets, and missed most of another slot's 152 GiB. The configured cache globs match only targets at the slot ROOT (and the fixed checkout paths). They miss nested targets such as <slot>/<repo>/target, <slot>/temp/<x>/target and <slot>/<repo>/temp/cargo-target, so the regenerable bytes the owner says may always be reclaimed stay behind.

# Design

- Find cargo target dirs slot-wide: a directory holding CACHEDIR.TAG (the cargo signature) together with a .fingerprint subdirectory, or a profile dir with one. Depth-bounded (configurable, default ~6), never following symlinks, staying on the slot's filesystem.
- Report each target per path with its size in --dry-run and the human/json reports. Delete per path, under cargo's own lock where present.
- Keep the configured globs for non-cargo caches.
- Honour #1029 clean-caches-skip-running-gate: skip any slot with a running or queued gate.
- Both editions (py/wrkslots and rs/wrkslotsd), with a parity test.
- Order of work (lead, 2026-11-08): first make the report honest (any cargo target dir found outside the configured globs is printed as a NAMED residual with its byte size, never folded into `removed=0.0 GiB`), then clean it: every registered checkout's `<checkout>/target` is covered by default, plus the slot-wide discovery above.

# Acceptance Criteria

- A fixture slot with nested targets at the three shapes above is reported per path with correct sizes, and cleaned.
- A symlink to a target outside the slot is not followed, and depth beyond the bound is not searched.
- A slot with a running gate is skipped (shared with #1029).
- Both editions agree.
- `clean-caches` never prints `REMOVED: <slot> cache=0.0 GiB` with exit 0 while a target dir with bytes remains in that slot: a residual it did not delete is named with its size, and the totals count only bytes actually freed (measured before and after).
- A test: a slot with a nested checkout target (`<slot>/<checkout>/target`) is cleaned, and the slot's measured bytes go down by that target's size.

# Notes

Reported by a consumer team on 2026-11-07. Bundled with #1029 for the same lane.

## Second report (consumer-b, 2026-11-08), raised to P1

The names below are redactions. `clean-caches` computes a slot's cache from its configured globs only, so a cargo target inside a named checkout (`<slot>/<checkout>/target`) is outside them, while a slot-root `target/` is covered. Repro: a slot whose `<checkout>/target` held 21,352,586,324 bytes; `wrkslots --wait-lock 120 clean-caches --only <slot>` printed `cache_mode=delete slots=1 removed=0.0 GiB` and `REMOVED: <slot> cache=0.0 GiB`, exited 0, and `du` was unchanged afterwards. Two more slots behaved the same (5.6 GB and 6.6 GB). Manual recovery was `cargo clean --manifest-path <slot>/<checkout>/Cargo.toml` (22.7 GiB). Raised to P1 because the report tells operators bytes were freed when none were.
