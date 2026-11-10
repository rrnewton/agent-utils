---
title: 'frozen-validate-gitlink-authority: frozen-validate cleanup refuses every checkout when a submodule is ahead of the pinned gitlink'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
- frozen-validate
created_at: 2026-11-10T09:13:30.090805207+00:00
updated_at: 2026-11-10T09:13:30.090805207+00:00
---

# Description

Frozen-validate cleanup refuses EVERY frozen checkout (enclosing checkout HEAD differs from the outer authority pinned Gitlink) whenever the shared checkouts submodule is ahead of the outer repository HEAD gitlink for it. That is the normal state between submodule snapshots, since any landing in the submodule repository moves it before the next snapshot. Observed 2026-11-10 3:40-4:04 AM ET: the reclaim sweep refused all three abandoned probe checkouts (about 140 GB) with this message, in py/wrkslots/cli.py _frozen_validation_authority_commit (lines 49803-49838 at cc487cc3). Source task: https://github.com/rrnewton/dev-hermit-beads/blob/main/issues/dh-x9lrah.md

# Design

Compare against the gitlink recorded for the commit the frozen checkout was created FROM, not the outer authority current HEAD. The genuine chain-mismatch refusal must still fire when the loaded chain differs from the chain recorded for the checkout.

# Acceptance Criteria

A frozen checkout created at the old gitlink stays recoverable when the submodule is one landing ahead; a genuine chain mismatch is still refused. Tests cover both.
