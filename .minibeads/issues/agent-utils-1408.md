---
title: 'eden-image-refuse: create --eden silently ignores --representation image and its ceilings'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-12T11:21:14+00:00
updated_at: 2026-11-12T11:21:14+00:00
---

# Description

[watermelon-full-preview] `wrkslots create --eden --representation image` (and `--eden` with `--image-ceiling-gib` / `--image-state-ceiling-gib` when the resolved representation is image) silently makes a plain directory slot. In `py/wrkslots/cli.py`, `_cmd_create` returns from the Eden branch (`_provision_eden_slot`, which only `mkdir`s the slot or creates a project-wide worktrees-image subvolume) before the per-slot image branch (`_provision_slot_image`) is reached, and nothing refuses the combination. The caller asked for image containment and gets none, with no signal.

The host Eden checkout cap is a separate, earlier refusal and is not this issue.

# Design

- Refuse before anything is created when `--eden` is combined with a resolved image representation or with an image ceiling flag, naming the offending flags and the supported alternative: `create --eden` without `--representation image` and without the ceiling flags (a plain directory, or a subvolume of the project-wide worktrees image when one is configured), or `create --representation image` without `--eden`.
- The refusal must happen before any slot directory, image, journal, or registry write, and before any Eden clone.
- Keep Rust/Python parity only where both editions parse the flags: the Rust `create`/`create-text` parsers in `rs/wrkslotsd` have no `--eden` or ceiling flags today, so there is no Rust combination to refuse; note that in the delivering commit.
- The unprivileged per-slot FUSE image path for Eden is #1409 eden-fuse-image, not this issue.

# Acceptance Criteria

- A fixture test (no real Eden, no sudo) shows `create --eden --representation image` and `--eden` with `--image-ceiling-gib` refuse with a message naming the flags and the supported alternative, and that nothing was created.
- Existing Eden and image create tests still pass.

# Notes

Numbered from devbig030's interim block (1400-1469). Found by a read-only probe of image-backed Eden slots on 2026-11-11.
