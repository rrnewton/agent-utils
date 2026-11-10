---
title: "direct-validate-path: keep validation slots at worktrees/validate in the direct layout and stop history bricking every command"
status: open
priority: 1
issue_type: bug
created_at: 2026-11-10T00:00:00.000000000+00:00
updated_at: 2026-11-10T00:00:00.000000000+00:00
---

# Description

#1162 worktrees-image-at-slots-dir added `worktrees_image.mount_at: worktrees_dir` and `wrkslots worktrees-image migrate-layout --to direct|binds`. In the direct layout a validation slot's path is `worktrees/slots/validate/<name>`; the bind layout uses `worktrees/validate/<name>`.

`_assert_record_paths` checks every record's stored checkout path, including archived and replayed history, against the current layout. After a successful `--to direct`, every command refuses, for example "slot vdry checkout project path does not match its slot: 'worktrees/validate/vdry' != 'worktrees/slots/validate/vdry'" for a slot removed before the switch. That includes `audit`, `create` and `migrate-layout --to binds`, so the supported rollback is bricked too. A project whose history holds thousands of validate rows at `worktrees/validate/...` is stuck.

Tooling in the first adopter hardcodes `worktrees/validate` in about ten files, so moving validation slots in the direct layout is itself the wrong contract.

# Design

Keep validation slots at `worktrees/validate/<name>` in the direct layout too: `<image>/validate` inside the image bind-mounted at `worktrees/validate`, one mount owner, with a subvolume per validation slot under it surfaced at `worktrees/validate/<name>`. Keep or drop the "validate is refused as an agent slot name in direct mode" rule according to where `<image>/validate` sits, and explain the choice.

Defence in depth: `_assert_record_paths` must not judge archived or replayed history against the current layout. Check active rows against the current layout and historical rows against the layout they were created under, or skip the layout-path check for archived rows while keeping the identity checks. No command may become unusable because of history.

`migrate-layout` must refuse before switching if any post-switch command would refuse: run the path assertions against the target layout as part of its preflight.

The Rust settings parser already accepts `mount_at`/`quota_mode`; check that nothing in Rust computes the direct-mode validate path differently and share any path rule with the Rust handlers.

# Acceptance Criteria

- A registry with archived validate rows at `worktrees/validate` (and archived agent rows) migrates with `migrate-layout --to direct`, after which `audit`, `status`, `doctor`, `create` (agent and validate), `remove`, and `migrate-layout --to binds` all succeed.
- A round trip binds -> direct -> binds succeeds on a real loop btrfs image.
- A validate slot created in direct mode lives at `worktrees/validate/<name>`.
- `migrate-layout` refuses before switching when a post-switch path assertion would refuse.

# Notes

Reported by the first adopter's lead after a scratch-project dry run against #1162 (f7477253 + e4f51cb1). Scratch projects and fixtures only; never point wrkslots at another team's project.
