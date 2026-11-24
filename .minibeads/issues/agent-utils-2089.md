---
title: 'agentctl-restore-multi-project-images: restore must remount the slot images of every configured wrkslots project'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- wrkslots
created_at: 2026-11-22T05:40:00-05:00
updated_at: 2026-11-22T05:40:00-05:00
---

# Description

`agentctl restore` runs `wrkslots image status` (and remounts) for a single wrkslots project's image. A second wrkslots project with its own image may be recorded by lanes in the same registry; after a reboot, a lane revived into that project would start in an empty mount point, because its project's image was never remounted and its bind was waved through as not image-backed.

# Design

Compute the desired slotted agents (`slot_agents`, read-only) before the remount step and derive the project set from them: the restore project (the registry's parent) always first, then the distinct canonicalised recorded slot projects (self-referential records healed exactly as `slot_agents` already heals them), sorted for determinism. Run `remount_slot_images` once per project with the same resolved wrkslots binary and that project as the working directory; one project's failure (nonzero status, warnings, spawn error) never blocks the others and never fails restore.

The report keeps `slot_images` as exactly today's restore-project row (same fields, same content) so existing readers do not break, and adds one additive key, `slot_images_by_project`: an array with one object per project in the order above, each object holding that project's remount row fields plus its `project`. A dry run keeps `slot_images` as `{action: skipped, reason: dry run}` and lists every discovered project in `slot_images_by_project` as `{action: skipped, reason: dry run, project}`. The new key round-trips through the existing progress-report writer, which clones the whole report, so no allowlist change is needed.

`verify_slot_bind` receives the agent's own project row. It refuses the lane when that row is missing, when the row's action is not `ran` (the status call failed or never ran), when a not-mounted warning names the slot, or when the slot's phase is not ready/mounted. It keeps the existing verified outcome for a slot genuinely absent from its own project's successful status (a plain worktree, not image-backed), and the `findmnt` and recorded-directory checks are unchanged.

The Eden daemon step covers the same per-project set in a separate change: one `wrkslots eden restart-slot-daemons` invocation per project with that project as its root and working directory, naming only that project's Eden slots, only when that project's image row ran. Slots recorded under no covered project keep refused rows and are never passed to the verb, a project with only plain worktrees produces no invocation, and every existing refusal and degrade semantic (never fails restore, refusals counted across projects, notice de-duplication) is kept.

# Acceptance Criteria

- After a simulated reboot, agents in two different wrkslots projects revive into mounted slots, with one image-status invocation per project in that project's working directory; `slot_images` is unchanged and `slot_images_by_project` carries one row per project.
- A project whose image is reported not mounted holds only its own agents with a bind reason; the other project's agents revive.
- A project whose image-status command fails (nonzero) holds only its own agents; the other project's agents revive and restore does not fail.
- An agent recorded under a project with no image row is held, never waved through; a plain-worktree project (status succeeds, slot not listed as image-backed) revives exactly as before.
- Dry-run reports list every discovered project as skipped/dry-run under `slot_images_by_project`.
- A single-project registry produces the same report (apart from the additive key) and the same command sequence as before.
- An Eden-backed slot in a second project restarts with that project as the verb's root; a second project whose image failed refuses its Eden slots without an invocation while the first project's restart normally; a plain-worktree second project produces no Eden invocation.
- Deterministic fake-host tests cover all of the above.

# Notes

Filed by the coordinator; the owning lane refined the design and acceptance criteria as it worked, including the per-project Eden step and the precise hold rule (hold only when the agent's own project is image-backed and not ready, or when its status call failed or produced no row).
