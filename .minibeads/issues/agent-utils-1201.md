---
title: "batch-grouped-fencing: remove-agent-batch fences items in groups with one fenced /proc scan per group"
status: closed
priority: 3
issue_type: feature
created_at: 2026-11-10T10:30:00.000000000+00:00
updated_at: 2026-11-10T23:00:00.000000000+00:00
---

# Description

After #1175 census-single-pass, Python `remove-agent-batch` makes one multi-target /proc walk per capture before
the fences, but still one fenced /proc scan per item after that item's fence (by path and by dev/ino). So a batch of
N items costs about N+1 walks plus lsof. On a large host (thousands of processes, 100+ slots) the per-item fenced
scans dominate.

# Design

Approved by the lead (2026-11-10, tm f28ace29), with the addendum's seal and per-slot recovery commands.

**Order.** Today each item runs alone: an unlocked lookup in the shared walk (#1175), then under the locks its
checks, its journal and the rename fence, then unlocked (#894) its salvage re-checks, its own fenced /proc scan
(lsof only if undecided), nested judgment, deletion, then completion under the locks. Grouped: consecutive
eligible items, up to `_AGENT_REMOVE_BATCH_GROUP_MAX` = 8, are each fenced exactly as before (phase F). Then one
scan, `_agent_batch_group_fenced_uses`, covers every member's fenced tree (phase S, with each image member's
sole-file-system proof first). Then each fence is sealed, and each member finishes in turn (phase D), its first
check after the fence served from the group verdict. A group of one, and ordinary remove, run exactly as before.

**Why grouped is still safe.** This is #1108's argument. After the rename nothing names the slot but the fenced
path, so a process can reach the tree only through a reference it already holds (cwd, root, exe, fd, mapping, a
mount in any namespace, a socket bound below the slot) or one passed from a holder. The scan sees every holder.
The one precondition, every member's fence before the scan starts, is asserted in `scan_group`. What grouping
lengthens is the wait between the scan and a member's delete, and two things bound it:
- the seal below;
- `_AGENT_REMOVE_BATCH_GROUP_FRESH_SECONDS` = 60: a member that reaches its check later than that after its group
  scan scans alone.

**(c) The fence parent and same-user walkers.** The fenced directory is a sibling of the slot in the slots
directory, `.<slot>.fenced.<generation>.<32 hex>`, and keeps the slot's mode (only private cleanup seals it
today). The scan does not cover a same-user walker (du, rg, find) that lists the parent and opens its way into
the tree after the scan. That is already true of today's per-item window, and grouping would lengthen it.
- So right after the group scan, each member's fenced directory is chmodded to 0. The scan walks the tree, so it
  cannot run on a sealed one, and a reference taken before the scan is caught by it.
- The mode is restored to `original_mode` at the start of that member's phase D, before its salvage Git checks.
  The window left (the start of D to the delete) is today's per-item window.
- The journal says `x.sealed: true` from phase F, so it is written before any chmod.
- Recovery (`_recover_finish`), resume (`_begin_or_resume_path_fence`) and rollback restore the mode first, and
  each restore can be repeated safely.
- chmod on an image or bind mount point changes the mounted root: a user-namespace bind-mount test shows the
  bound directory itself at mode 0.
- Owned-mount scanning only lists the parent and reads mountinfo, and `doctor` runs on sealed fences in the
  crash tests.
- Root and a deliberate chmod are out of scope.
- Sealing ordinary remove, in both editions, is #1222 remove-fence-seal.

**Budgets.** Each member's item budget is unchanged in amount and meaning. Its paused deadline is shifted at the
start of its phase D by the time it waited, so the group scan, the other members' fences and the other members'
deletes are not charged to it. The group scan has the ordinary per-scan bound
(`_AGENT_REMOVE_BATCH_SCAN_SECONDS`, CPU time) and is charged to no member. If it runs out, no member has a
verdict and each scans alone, so no member is refused because the group ran out of time.

**Journals, interruption, recovery.**
- Each member journals in its own scoped `FINISH.<m>.<slot>.journal` carrying
  `x: {min_wrkslots_version: "0.10.0", group: {id, size}, sealed: true}`. `size` is the planned member count.
- The batch allows only the journals it wrote itself while its group is open.
- A kill in F, S or D leaves one self-sufficient journal per fenced member. Plain `recover` asks for one slot, so
  `wrkslots recover --slot S` decides each on evidence: it restores the mode, takes a fresh census, then resumes,
  or rolls back if the slot is in use.
- Finish recovery excludes its group's sibling journals from create-journal classification: they are fenced
  removals of other slots.
- The report lists `recovery_commands` (human output: `RECOVER:` lines). The journal naming the fence is always
  written before the rename and cleared only after the delete or rollback, so no fenced tree is left without a
  journal.

**Refusal of one member.** Verdicts are per member:
- A use, an owner-cgroup match, a malformed mount table or a cgroup read error refuses that member with its own
  scan's message.
- An undecided tree, or an unreadable process evidence read, makes undecided exactly the members still waiting
  on it, and lsof decides for each of them.
- A member refused before its fence leaves the group, and the rest go on.
- A member refused just after its fence in F (a Git registration repair or journal write failure) leaves its
  journal for `recover --slot`, as one item does today: the batch fences nothing more, and the members already
  fenced still finish.
- A member refused in D is rolled back as today; the later members go on.

**Mixed versions (a hard requirement).** Grouped journals require 0.10.0 (both editions bumped from 0.9.0):
- a 0.9.0 reader refuses by version;
- older readers refuse the unknown `x`;
- the Rust edition refuses by name, and it never resumes finish journals anyway.

Proven against fixtures with four old commits: cc487cc3 (0.7.0, a consumer's pinned version), d4eb6ae5^ = dab2a70c (the
last 0.6.0), 85913e96 (the first 0.6.0) and d0067a4a (devbig014's hand-copied 0.6.0). Tag v0.6.0 holds no
wrkslots. Results are in Notes.

**Tests.**
- A differential: the group verdict equals `_agent_batch_fenced_use` per member, for cwd, fd, exe, mapping,
  socket, hard link, a process holding two trees, an undecided tree, a whole-device member, an owner-cgroup
  refusal and an exited pid.
- Walk count: groups of 2 over 5 items gives 2 group scans and 1 ordinary scan; also the constants.
- Journal shape and seal while fenced.
- T_fresh fallback: the third member scans alone and finds a holder that appeared after the group scan.
- Refused in F, before the fence and after it.
- Crash recovery at the second fence, after the scan, after the seal, and mid-delete.
- Rust refuses a grouped journal, and Python refuses each malformed variant.
- A seal through a bind mount point.

# Acceptance Criteria

- A written design, approved by the lead, in Notes.
- Then: a differential showing identical per-item verdicts to the per-item fenced scan, crash-recovery tests with
  several items fenced, and a walk count of about 1 + groups.

# Notes

Split out of #1175 census-single-pass (option B) by the lead coordinator (2026-11-10).

**Mixed-version proof (2026-11-10, cl-fence).**
- *Method.* Fixture projects only, with the worktree representation forced and sudo stubbed on PATH (every call
  logged and refused; none was made). The NEW tree built three dead agent slots and crashed a 3-member
  remove-agent-batch at two points:
  - after the group seal: three grouped FINISH journals, three fences at mode 0;
  - mid-deletion of the first member.
  Each old commit, run from `git archive` with `python3 -I`, then ran status, doctor, audit, recover,
  `recover --slot slot02`, `remove slot02`, `remove-agent-batch`, clean-caches and `create` against the fixture.
- *Result.* Every journal, registry and event file stayed byte-identical, and every fence kept its mode:
  HANDS-OFF.
- *85913e96 (the first 0.6.0).* Refuses every command at config load: it cannot read today's YAML config. It has
  no `recover --slot` and no `remove-agent-batch`.
- *dab2a70c = d4eb6ae5^ (the last 0.6.0), d0067a4a (devbig014's hand-copied 0.6.0) and cc487cc3 (0.7.0,
  a consumer's pinned version)* all behave the same:
  - `audit`, `recover --slot` and `create --machine testhost` refuse with "finish journal has invalid fields:
    unknown owner, x" (d0067a4a: "owner, removing, x"), the exact-key refusal;
  - plain `recover` refuses with "multiple interrupted mutations ... select one exact slot";
  - `remove` refuses with "interrupted mutation recorded ...; run 'wrkslots recover' first", and
    `remove-agent-batch` with "an interrupted removal is recorded ...";
  - `status` and `doctor` exit 0 and report the journal as untrusted or unfinished.
  None deletes or rolls back a fence. No further marker is needed.
- *The Rust edition* refuses a grouped journal by name in its journal shape check, and its `recover` never resumes
  a finish journal.

**Real loop-image evidence (2026-11-10).** By accident (reported and cleaned up), early fixture scripts built
image-backed fixtures on real loop devices (ext4 on /dev/loop51 and others), because they imported the test helpers
without conftest. On those images:
- a grouped batch fenced 3 image members;
- the seal set each fenced mount ROOT to `d---------`;
- `recover --slot` restored each sealed image member's mode and finished its removal;
- afterwards there were 0 mounts and 0 loop devices.
To stop this recurring, the shared test helper `initialize()` now refuses to create an image-backed fixture
project unless `WRKSLOTS_TEST_ALLOW_IMAGE_FIXTURES=1`, whatever `WRKSLOTS_SLOT_REPRESENTATION` says (the lead's
request). The lifecycle tests that use images on purpose set the opt-in.

**Follow-up:** sealing for ordinary `remove`, in both editions, is #1222 remove-fence-seal.

## Resolution

Delivered by cl-fence (opus 5.5) as designed above. Python `remove-agent-batch` fences up to 8 consecutive items,
scans every fence once with `_agent_batch_group_fenced_uses` (a separate implementation, checked by a differential
against `_agent_batch_fenced_use`), seals the fences, and finishes each member. A batch of N items now walks /proc
about 1 + ceil(N/8) times, instead of N + 1. The report gains `group_fenced_scans`,
`group_fenced_scan_seconds`, `group_size_max`, `group_fallbacks` (with reasons), `group_seal_failures` and
`recovery_commands`. Both editions are now wrkslots 0.10.0. Rust has no remove-agent-batch (PY_ONLY): it refuses a
grouped member's journal by name, and its recover never resumes a finish journal. Ordinary remove and a batch of
one are unchanged on disk. Not covered here: sealing for ordinary remove (#1222).
