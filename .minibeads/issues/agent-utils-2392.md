---
title: 'wrkslots-validate-unit-proof: prompt, deterministic teardown of validate slots (kill the slice, unlazy unmount, one namespace pass, O(1) delete) and a bounded sweep'
status: open
priority: 0
issue_type: feature
assignee: cl-census
labels:
- wrkslots
- validate
- reclamation
created_at: 2026-11-27T11:58:00-05:00
updated_at: 2026-11-27T11:58:00-05:00
---

# Description

Owner directive, 2026-11-27 10:20 AM ET: "that is a complete and embarrassing failure of our work slots infrastructure especially for validate slots. Reclamation must be prompt and deterministic. Work with agent utils and make this so ASAP. Overcome any barriers. Don't get stuck on protocol or some dumb checks it was previously doing such as the scans of all of proc. You need to be thoughtful about the checks it's doing and analyze the algorithm from the perspective of deterministic completion."

Today a validate slot's removal is gated by the same-UID process census (#2376, #1883). On a busy host the census does not close, so finished validate checkouts stay on disk. The consumer had 164 `validate-fresh-*` rows, with 7 stale binds and 2 subvolumes holding data that needed a root script on 2026-11-27.

This issue is v1: a teardown mode for validate slots on today's worktrees-image bind placement whose every step either finishes within a fixed bound or returns a named result. It has no census and no loop that waits for the process table to settle.

# Design

Design note, approved to code by the liaison (1048, then 1050 and 1051 for v1 on today's placement): `ai_docs/plans/2026-11-27-wrkslots-validate-deterministic-reclamation.md` in the harness repository.

`wrkslots remove --validate-complete --unit-proof` (and `--validation-proof-manifest` with `--unit-proof`):

1. Kill: `cgroup.kill` on `validate-<slot>.slice` (the derived name; no new record field), then an event-driven wait (`cgroup.events`, re-read every 0.5 s) bounded at 60 s for `populated 0`. Removal of the cgroup counts as empty. Past the bound the result is `cgroup-not-empty`, with the remaining tasks and their states. An absent slice prints `slice-absent: kill skipped`, and the later steps still run: the proof is never skipped.
2. Unmount the host binds once, never lazily. `EBUSY` is `checkout-in-use-through-host-path`. It needs no holder scan and no retry.
3. One pass over the same-UID `/proc/<pid>/ns/mnt` links in ascending PID order, deduplicated by namespace inode. Each namespace's `mountinfo` is read once, vanished PIDs and zombies are ignored, and the pass has a hard 10 s bound. Its results:
   - `namespace-scan-incomplete` when the bound passes;
   - `checkout-mounted-in-another-namespace` for any other mount of the subvolume or of a directory inside it;
   - `namespace-unreadable:<comm>` for a same-UID process whose namespace link the kernel refuses and which is not on the fixed allow-list (matched by `comm` AND `argv[0]`): the user systemd manager, `(sd-pam)`, `sshd`/`sshd-session`, `gnome-keyring-daemon`.

   Every unreadable PID is always listed. `listns(2)` exists on this kernel, but reading a foreign namespace's mounts needs `CAP_SYS_ADMIN` over it, and wrkslots runs unprivileged.
4. Delete with the existing O(1) privileged `sudo -n btrfs subvolume delete`, bounded at 120 s. Its results are `delete-timeout` or `delete-refused`, and the report says so. It never falls back to a file-by-file delete. A slot that cannot take this path (a per-slot disk image, the direct layout, no bind, initialized submodules) is `slot-not-unit-proof-placed`.

Every refusal re-establishes the bind and deletes nothing. Each teardown prints one line, `unit-proof slot=.. result=.. kill=.. kill_wait_s=.. unmount_s=.. scan_s=.. namespaces=.. unreadable=.. delete_s=.. referenced_bytes=.. remove_s=..`.

`wrkslots sweep-validate-teardowns --coordinator-pid P` re-attempts exactly the slots whose last unit-proof result was transient. It reports `teardown-stuck` after 3 consecutive sweep failures and never touches a slot in any other state.

Persisted state, reported to coordb and lead-014 before landing (no objection): no record field and no config key. The sweep's per-slot results live host-locally at `$XDG_STATE_HOME/wrkslots/validate-teardowns/<digest of the control path>/<slot>.json`, outside the control directory, so pinned readers never see them.

Accepted residuals (cooperative-process assumption, stated to the owner):

- A same-UID process outside the unit that reaches into it through `/proc/<pid>/root|cwd|fd` or ptrace.
- A same-UID process holding the subvolume through the root-owned image root's own path, which only wrkslots uses.
- An allow-listed non-dumpable process holding a private mount namespace copy.
- A namespace kept alive with no process in it, or created from a not-yet-visited namespace during the one pass.

v2, a box with cgroup, PID and network namespaces and the checkout mounted only inside it, removes these residuals and is the follow-up design.

# Acceptance Criteria

- Focused hermetic tests, with exact refusal prefixes:
  - each named result;
  - an empty and an absent slice;
  - `EBUSY` from a process in the host bind;
  - another namespace's mount, including a directory inside the subvolume;
  - an incomplete scan;
  - the unreadable allow-list, where an allow-listed process passes and an unknown one refuses;
  - delete timeout and refusal;
  - no fallback for each unplaced kind;
  - the sweep's re-attempt, its `teardown-stuck` and its dropping of gone rows;
  - an AST pin of the call sites of the unit-proof primitives and of `delete_slot_subvolume`.
- Interim user-level acceptance (real cgroups and real namespaces, no root): kill plus scan on 20 slots, with a positive control (the copy found before the kill), absent-slice and outside-copy negative controls. Done 2026-11-27 at load1 146-157: the mechanics passed 20/20, and the allow-list guard decision for `buck2` is pending.
- Root acceptance on the consumer's real validate units, run by the consumer's lead after its pin reaches the landed #2392 commit or later. The runbook is in the design note. The verdict is computed by a script from the printed `unit-proof` lines, never from prose:
  - 20 teardowns at load1 >= 237 for the whole run (recorded per run), each `retired` within its bounds;
  - one deliberately held slot (a process with its cwd in the host bind) returning `checkout-in-use-through-host-path` and deleting nothing;
  - its sweep re-attempt retiring once the holder exits.
- The user guide documents the mode, the result names (a stable interface: add, never rename) and the residuals.

# Notes

Filed by cl-census (Claude lane); the number was assigned by coordb.

Dependency: the consumer's `--slice` reland (outer bounds) must launch validation boxes under `validate-<slot>.slice`. Until it does, step 1 prints `slice-absent: kill skipped`. The unmount and the scan still decide, so a live straggler gets a named refusal instead of being killed, and the sweep re-attempts it. The result lines show how often the kill was skipped.

Related: #2376 (named census causes, parked), #1883, #2379, #2380.
