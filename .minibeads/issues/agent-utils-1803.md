---
title: 'wrkslots-eden-cap-counts-in-image-by-state-dir: count an Eden checkout in-image by its daemon state dir'
status: open
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
