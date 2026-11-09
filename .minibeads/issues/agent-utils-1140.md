---
title: 'slot-project-recorded-as-slot: a lane started from its slot records the slot as its own project and revive double-nests'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-09T13:53:37.000000000+00:00
updated_at: 2026-11-09T13:53:37.000000000+00:00
---

# Description

[watermelon-full-preview] Reported from a consumer registry's 2026-11-09
reboot recovery: restore and revive refused three boxed lanes because
their recorded working directory resolves double-nested
(<slot>/worktrees/slots/...), wrkslots then ran against the SLOT's own
.wrkslots.yml ("configured worktrees directory is missing or unsafe:
.../<slot>/worktrees/"), and one slot has no .wrkslots.yml at all.

Root cause is in agentctl, both editions. At start, --slot-project
defaults to the unresolved --cwd (rs/agentctl/src/subagents.rs;
py/agentctl/subagents.py): a lane started with --cwd <slotdir> --slot NAME
and no explicit --slot-project records the slot directory as its own
project in the record and launch spec. Start still worked because it only
forces --project-root when one was explicitly given, and the slot
manager's own discovery found the real project root. Revive, however,
always rebuilds the slot launch with the recorded project forced, so the
slot manager reads the config copy inside the slot worktree and computes
the nested path. Candidates copy slot_project verbatim, so the record
never self-heals. Restore's slot hold/transfer calls force the same wrong
root (reported, non-fatal).

Also in this area: the --slot / --project-box help and error text still say
wrkslots must be on PATH, but project_wrkslots() / slot_shell_command()
never consult PATH (AGENTCTL_WRKSLOTS_BIN, then profiles.json "wrkslots",
then the team folder's bin/wrkslots). The wording should match the lookup.

# Design

- Start side (stop minting bad records): when the resolved project equals
  the slot path the shell-command returned, refuse and name --slot-project.
  Where the slot manager can report its resolved project root, record that
  instead of the unresolved default.
- Revive side (heal existing records): when the recorded slot_project
  equals the recorded cwd, or the forced call fails or mismatches, retry
  the slot shell-command without an explicit project root and accept only
  an exact cwd and isolation match; write the corrected root into the
  candidate and its new launch spec. Restore heals through revive.
- Fix the PATH wording in the --slot / --project-box help and errors to
  name the real lookup order.
- Mirror in the Python edition where it implements the same paths.

# Acceptance Criteria

- start --cwd <slotdir> --slot NAME without --slot-project refuses before
  recording anything, naming the flag that fixes it.
- A pre-existing record with slot_project == cwd revives in its slot, and
  the new generation records the real project root.
- A forced-root result that does not exactly match the recorded cwd and
  isolation still refuses; nothing is created under <slot>/worktrees/.
- Help and error text for slot launches no longer claim a PATH lookup.

# Notes

Consumer answer (via the lead): until the heal lands, re-register the
affected lanes - retire the dead record and start again with --cwd and
--slot-project at the project root plus --resume - which keeps the slot
and conversation under a new token (old queue archived, not replayed).
Hand-editing slot_project in the record and launch spec would also work
but no agentctl command supports it, so it is not the recommended path.
Never create <slot>/worktrees/ or copy a config into the slot to satisfy
the forced root; that mints the nested second registry the layout exists
to prevent.
