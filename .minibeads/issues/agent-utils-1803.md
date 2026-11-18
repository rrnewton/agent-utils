---
title: 'wrkslots-eden-cap-counts-in-image-by-state-dir: count an Eden checkout in-image by its daemon state dir'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-17T15:30:00-05:00
updated_at: 2026-11-17T15:30:00-05:00
---

# Description

[watermelon-full-preview] Regression from #1730 (28bbac8d): `create --eden` mounts at `eden_mount_root` outside the project, but `_eden_cap_report` -> `partition_checkouts` still classifies by mount PATH, so every new in-image slot counts OUTER against `eden_host_cap`. Measured: 7 fresh slots read outer.

# Design

- New `eden.checkouts_by_state_dir`; `count_checkouts` delegates to it.
- A mount counts in-image when its path OR its daemon's `--edenDir` resolves inside an image root; with no known state dir the path decides; the default daemon stays outer.

# Acceptance Criteria

- An outside mount with slot state in-image counts in-image; the 3 new tests fail unpatched and pass patched; `test_eden.py` green.

# Notes

## Resolution

Landed 7ae4408f2735f5a432ddeab9fff924a60c11eb13 on github/main (mu-edenfix). Consumer patch reviewed line by line and applied with attribution; `test_eden.py` 56 passed, the 3 new tests fail unpatched. Gate flakes (D-Bus disconnect; #1778 pid liveness) were 5/5 clean solo with a passed exclusive rerun. No new persisted field.
