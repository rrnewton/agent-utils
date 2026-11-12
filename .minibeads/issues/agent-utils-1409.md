---
title: 'eden-fuse-image: design an unprivileged per-slot FUSE image path for create --eden'
status: closed
priority: 2
issue_type: task
labels:
- wrkslots
created_at: 2026-11-12T11:21:14+00:00
updated_at: 2026-11-12T15:21:52+00:00
---

# Description

[watermelon-full-preview] The only image model `create --eden` honours today is the project-wide worktrees image: one loop-mounted btrfs image whose slots are quota'd subvolumes, provisioned with `sudo -n` mount / subvolume create / chown. A per-slot image needs no sudo only via the FUSE backend (`WRKSLOTS_IMAGE_BACKEND=fuse`, fuse2fs), but the Eden branch never reaches it (#1408 eden-image-refuse now refuses that combination), and fuse2fs is started without `-o allow_other`, so Eden's setuid privhelper would probably be refused when mounting inside it. `/etc/fuse.conf` on this host has `user_allow_other`, so an unprivileged path may exist. The owner will choose between allowing sudo for a project-wide worktrees image on this host and this unprivileged path; this issue produces the facts for that choice, not a behaviour change.

# Design

- Write a design note at `ai_docs/reports/research/2026-11-12-eden-fuse-image.md`: how create/remove/doctor would work with a per-slot fuse2fs image mounted with `-o allow_other`, what `allow_other` exposes (any local user can read the mount; state the security trade-off plainly), cleanup when the image holds an Eden checkout (image removal refuses a non-empty image today, so the checkout must be removed first), and the failure modes.
- Run an experiment on this host WITHOUT sudo (never `sudo`, not even `sudo -n true`; `WRKSLOTS_IMAGE_BACKEND=fuse` so the backend never probes it): show fuse2fs with `allow_other` mounts, and that a second FUSE filesystem can mount inside it as this user. Keep images small (a few GiB sparse), inside the slot or /tmp, and unmount and delete them when done.
- Do NOT clone a new Eden checkout: the host Eden checkout cap (3) is full. State precisely what an Eden clone inside such an image would still need to prove, for the owner to decide.

# Acceptance Criteria

- The design note exists with the create/remove/doctor, security, cleanup, and failure-mode sections above.
- The experiment's commands and observed output are recorded in the note, with all experiment images unmounted and deleted afterwards.
- No wrkslots behaviour changes under this issue; any implementation follows as a separate change after the owner's choice.

# Notes

Numbered from devbig030's interim block (1400-1469). Depends on #1408 eden-image-refuse for the interim refusal.

Design and experiment delivered 2026-11-12 in `ai_docs/reports/research/2026-11-12-eden-fuse-image.md`: fuse2fs with `allow_other` mounts with no sudo, a second FUSE filesystem mounts inside it, and a real Eden clone inside it succeeded (privhelper mount proven in the mount table; `sl status` clean; file read/write round-tripped; `eden remove` and unmount clean). Two implementation facts fall out: fresh images need mkfs `root_owner` (wrkslots already does this), and default Eden redirections escape the image to host scratch, which the existing in-slot guard would refuse. Left open deliberately: the owner's choice between this path and the sudo worktrees-image path, and the five still-unproven items listed in the note (full wrkslots remove sequence, confined redirections, lifecycle survival, performance, multi-slot behaviour).

## Resolution

[watermelon-full-preview] Owner decision 2026-11-12: Option S, the sudo worktrees-image path on this host (fuse2fs only as fallback), with sudo approved only for worktrees_image create, mount, unmount and teardown through wrkslots' own `sudo -n` helpers. Proven the same day in a scratch project on top of #1204 wrkslots-eden-checkouts (including the removal sequencing): sparse 10 GiB btrfs image created and loop-mounted by wrkslots, an `--eden` slot created as a quota'd subvolume bind-mounted from the image, a real Eden clone inside it (`sl status` clean, file read), buck-out proven physically inside the image (same device as the slot; the daemon-derived host path is a symlink into the slot), then full teardown (eden remove, daemon-gone verified, unmount, image deleted; findmnt/losetup clean). Peak real allocation 269 MiB on the image file. Also recorded: an Eden daemon escapes its creator's systemd scope (it runs as its own `edenfs@<state dir>.service`, unbounded), so containment for Eden slots needs a drop-in on that unit or Eden's own config; that follow-up is tracked separately as #1423 eden-daemon-containment and is not part of this issue's landing. One gap fixed: the registry storage check no longer judges an Eden checkout's external backing repo by the Git sibling rule, which refused Eden slot removal; regression test included with no real sudo. Adoption recipe for other teams (their own config: image size and quota, `eden_host_cap`, exact commands) is in `ai_docs/reports/research/2026-11-12-eden-worktrees-image-sudo.md`. Remaining lifecycle items from the fuse note (fuse2fs survival, performance) belong to the fallback path and stay open as future work, not under this issue.

Combined-tree check for landing (gate passed on the pre-rebase tree; main then changed `py/wrkslots/cli.py` in the provisioning-output code, away from this change's storage-check hunk): rebased tree `264e6fc3c91ebd5901ee13479a1fdbe9677f29c2`; contained run (`systemd-run --user --scope -p CPUQuota=800% -p MemoryMax=16G -- nice -n 19 ionice -c 3`) of `test_eden.py`, `test_worktrees_image.py` and `test_finish_provisioned.py` (the last exercises the code main touched): 96 passed; strict mypy clean on `cli.py` and `test_eden.py`.
