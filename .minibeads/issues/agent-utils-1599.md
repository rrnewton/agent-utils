---
title: 'image-fuse-otrunc: never silently run Git checkouts on a truncate-ignoring fuse image backend'
status: open
priority: 1
issue_type: bug
external_ref: gh-290
created_at: 2026-11-15T17:47:04.000000000+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

fuse2fs (1.47.1 in the report) ignores O_TRUNC: writing 41 bytes then 6 bytes with truncation leaves a 41-byte file. Git rewrites each submodule's .git pointer file with a shorter path, so all of them corrupted and image-slot create failed. Backend selection compounds it: auto falls back to fuse silently when sudo is unavailable (resolve_backend in py/wrkslots/slotimage.py), so users hit the corruption unknowingly. Recovering a failed image create also registered a slot whose submodules were empty, with no warning.

# Design

Probe the mounted backend once per image (write long content, rewrite shorter with O_TRUNC, compare) or refuse/warn loudly when auto resolves to fuse for a slot that will hold Git checkouts, requiring an explicit opt-in until a truncate-honouring fuse2fs is detected. Print the resolved backend and the reason at create/mount. Document the fuse2fs O_TRUNC hazard in the user guide. Make recovery re-initialise or explicitly refuse empty submodule directories instead of leaving them registered silently.

# Acceptance Criteria

- Test: auto without sudo warns naming the fuse fallback (or refuses, per the chosen policy).
- Test: the backend probe detects a truncate-ignoring file system (fake/fixture) and refuses.
- Test: recovery does not leave an empty submodule directory without a named refusal.

# Notes

[watermelon-full-preview] Filed from GitHub #290 image-fuse-otrunc (https://github.com/rrnewton/agent-utils/issues/290) during Request 098 triage. Severity was flagged first in #634 github-triage-pass.
