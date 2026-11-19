---
title: 'wrkslots-archive-ignored-long-symlink: --archive-ignored dies on a long symlink target'
status: open
priority: 1
issue_type: bug
assignee: mu-archsymlink
labels:
- wrkslots
created_at: 2026-11-18T18:00:00.000000000+00:00
updated_at: 2026-11-18T18:00:00.000000000+00:00
---

# Description

`wrkslots remove --archive-ignored` dies in `_archive_ignored_files` with tarfile "ValueError: linkname is too long" on a slot whose gitignored files include a symlink with a target over 100 bytes (a uv cache link at 135 bytes was reported). `py/wrkslots/cli.py` writes that tar with `format=tarfile.USTAR_FORMAT` (from 612273f9, 2026-11-11); USTAR caps symlink targets at 100 bytes and member names at 255. The crash happens in the lock-free salvage before any write, so there is no state damage, but no slot holding such a link can be removed with --archive-ignored. The hand-rolled Rust twin in `rs/wrkslotsd/src/remove_write.rs` (`ustar_header`) refuses the same archives with "ignored-files archive link target is too long".

# Design

Write the ignored-files archive in PAX format in both editions, byte-identically (a per-member `x` extended header carrying `path`/`linkpath` records only when they overflow USTAR, the member header truncated as Python's tarfile writes it), so existing cross-edition byte comparisons and receipt sha256 values for in-limit archives are unchanged. Every reader of these archives must handle PAX: the Python verify (`tarfile.open(..., "r:")`, which is format-transparent) and the Rust verifier (parse `x` headers and apply their path/linkpath overrides to the next member). The archive is a file, not a record field: receipts name only its path, size and sha256, so no persisted record changes shape.

# Acceptance Criteria

A slot with an ignored symlink whose target is over 100 bytes and an ignored file whose member name is over 255 bytes removes with `remove --archive-ignored` successfully in both editions, and the archive verifies and round-trips (member names, symlink target and file bytes intact). In-limit archives keep the exact bytes they had before (USTAR member layout unchanged).

# Notes
