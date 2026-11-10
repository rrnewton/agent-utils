---
title: 'wrkslots-shared-submodule-objects: share one submodule object store across slots instead of a full clone per slot'
status: open
priority: 1
issue_type: task
assignee: mu-submod
created_at: 2026-11-10T13:12:43.780152515+00:00
updated_at: 2026-11-10T13:12:43.780152515+00:00
---

# Description

A consumer project's `.git/worktrees` is 238G: 106 registrations, 103 of them for
ACTIVE wrkslots rows. Each registration carries its own submodule gitdir
(`.git/worktrees/<slot>/modules/<submodule>`), a FULL object store of 2-28G each,
on /data, outside the worktree image, although the slots themselves live in the
image. That is probably the biggest single disk fix available (owner's 1 TB
headroom goal, reported 2026-11-10 8:05 AM ET via the coordinator).
Never name a consuming project in this tree; the case above is "a consumer's
project".

Survey (mu-submod, 2026-11-10, `py/wrkslots/cli.py` at slot HEAD 0d97b7a6):

- wrkslots itself never clones submodules. `_GitVcs.add_worktree`
  (cli.py:15987) is a plain `git worktree add -b <branch> <dest> <start>`, which
  creates the linked-worktree administration `<common>/worktrees/<slot>/`.
- Submodules are populated by the configured post-provision hooks
  (`_run_post_provision_hooks`, cli.py:30287; e.g. a hook running
  `git submodule update --init --recursive`). Git's stock behaviour for a linked
  worktree then clones every submodule into
  `<admin>/modules/<name>/` (`_worktree_module_directories`, cli.py:13761:
  "Git clones each submodule of a linked worktree into
  <administrative directory>/modules/<submodule name>/"), with its own full
  `objects/` store and no `objects/info/alternates`. That per-slot clone is the
  2-28G. `_apply_slot_git_identity` (cli.py:30436) already states it: "Each
  populated submodule is a clone private to this slot".
- The primary checkout's shared store already exists at
  `.git/modules/<name>/` (`_WorktreeModule.shared`, cli.py:13683).
- The "source submodule is unavailable" refusal is in
  `_submodule_salvage_checkouts_uncached` (cli.py:25763, 25780): when the source
  checkout is not a Git root for that submodule, salvage judges the slot copy by
  its own module repository, or refuses when that repository is outside the
  project root / its remote is a local module mirror with no source authority.
- Retirement today: salvage first (`_submodule_salvage_checkouts`,
  `_submodule_private_tips` cli.py:24879, `_durable_submodule_repositories`
  cli.py:24965), then `_GitVcs.remove_worktree` (cli.py:18634) runs
  `git worktree remove [--force]`, which deletes the administration directory
  including `modules/`. Absent-agent recovery has a parallel path
  (`worktree_modules`, cli.py:16394; `_retain_worktree_modules`,
  cli.py:14222) that salvages unpublished submodule commits before removing a
  registration. So (2) mostly exists for stock clones; it must be extended, not
  assumed, for shared stores (see Design).

# Design

Chosen approach: linked worktree of the primary submodule repository, NOT
alternates. Verified on scratch projects under /tmp (git 2.53.0-Meta,
mu-submod, 2026-11-10):

- Stock slot: `git submodule update --init` in a linked superproject worktree
  created `.git/worktrees/slotA/modules/leaf` at 47,112 bytes for a 60-commit
  leaf whose primary `.git/modules/leaf` is 47,092 bytes: a full duplicate.
- Shared slot: from the primary submodule checkout,
  `git worktree add <slot>/<subpath> <gitlink>` creates
  `.git/modules/leaf/worktrees/leaf` (HEAD, index, logs, config.worktree only);
  the shared `.git/modules/leaf` stayed 47,598 bytes total for primary plus one
  slot. `git submodule status` in the slot is clean, a later
  `git submodule update --init` is a no-op (rc 0), and a private detached commit
  made in the slot's submodule survived `git gc --prune=now` in the primary
  submodule repository, because gc in a single repository reaches every
  worktree HEAD and reflog. Removing the leaf worktree
  (`git worktree remove --force <slot>/<subpath>` from the primary submodule
  checkout) was clean and left the primary checkout intact.

Why not alternates: a per-slot gitdir whose `objects/info/alternates` points at
the primary's `objects/` borrows every object it does not hold itself. `git gc`
/ `git prune` in the primary deletes objects unreachable from the PRIMARY's
refs; a slot that checked out (without fetching) a commit present only in the
primary borrow, then built private commits on it in its own store, loses the
borrowed ancestors when the primary prunes. Protecting that needs keep-refs per
slot plus `gc.pruneExpire=never` / disabling auto-gc on the primary, i.e.
safety by configuration that any later manual `git gc --prune=now` in the
primary silently breaks. The linked-worktree design has one repository, one
object store, one ref store: reachability is computed over all worktrees, so
no slot can be pruned out from under another, and no protective configuration
is load-bearing. Alternates are therefore rejected as the primary mechanism.

Mechanics and hazards the implementation must handle:

1. `core.worktree` corruption (observed): a submodule gitdir carries
   `core.worktree` in its SHARED config (e.g. `../../../leaf`). A bare
   `git -C .git/modules/<name> worktree add ...`, and even
   `git -C <primary>/<subpath> worktree add ...` without preparation, rewrites
   that shared `core.worktree` to the new slot path (relative to the gitdir),
   after which every command in the primary submodule fails with
   "cannot chdir to '../../../../../../<slot>/leaf'". Preparation, verified in
   the scratch trial: set `extensions.worktreeConfig=true` in the submodule
   repository's shared config, move `core.worktree` into the main worktree's
   `config.worktree` (absolute path of the primary submodule checkout), unset
   it from the shared config. After that, `worktree add` creates a per-worktree
   `config.worktree` and does not touch the shared config; primary and slot
   both work. This mirrors the `extensions.worktreeConfig` precedent wrkslots
   already uses for slot identity (#1096, cli.py:30381-30493), including its
   refusal mode: if the shared config cannot be safely converted (e.g. a
   `core.bare` interaction), refuse sharing for that submodule and fall back to
   the stock per-slot clone, reporting it, rather than corrupting the primary.
   The conversion mutates only the primary submodule repository's config,
   never its working tree; it is idempotent and recovery repeats it.
2. Creation point: in `_cmd_create` (cli.py:31093-31149), after
   `add_worktree` and BEFORE `_run_post_provision_hooks`, for each checkout,
   recursively (nested submodules included): for each declared submodule whose
   source repository has an initialized `.git/modules/<name>` repository,
   prepare it as above and add a linked worktree of it at the slot's submodule
   path, detached at the superproject's recorded gitlink. The consumer's hook
   (`git submodule update --init --recursive`) then sees populated submodules
   at the gitlink and does nothing, as verified. When the source has no such
   module repository (primary never initialized that submodule), do not
   populate the primary's working tree to create one: leave the stock
   hook-clone behaviour for that submodule and note it in the create output.
   A submodule already populated by an earlier hook run on recovery
   (`recover` re-runs hooks) must be detected (populated, at gitlink, gitdir
   under the shared `worktrees/`) and left alone; a stock private clone found
   on recovery is left as-is (migration is the separate path below), never
   silently replaced.
3. Shared-repository hazards, accepted and bounded: refs and config of the
   submodule repository are shared, so a branch checked out in the primary
   cannot be checked out in a slot (Git refuses; submodules are normally
   detached at the gitlink, so this is the existing submodule-update shape);
   concurrent `gc`/`fetch` in the shared repository serialize on Git's own
   locks; `git submodule deinit` in any peer affects the common config (the
   existing warning at cli.py:18642 already forbids wrkslots doing this).
   Per-slot submodule identity (#1096) currently writes plain `git config` in
   "a clone private to this slot" (cli.py:30436): with a shared repository
   that write would leak to every peer, so identity for a shared submodule
   must use `git config --worktree` (with the `extensions.worktreeConfig`
   already enabled by the preparation) or be skipped with a report. This is a
   required part of the change, not a follow-up.
4. Retirement (remove/retire, absent-agent recovery): salvage runs first as
   today, but `_submodule_private_tips` / `_durable_submodule_repositories`
   assume the slot's module repository is deleted with the registration and
   the source repository is a different repository that outlives it. With a
   shared repository those are the same repository: a detached private HEAD
   commit IS in the durable store but becomes unreachable (except through the
   removed worktree's reflog, which removal deletes) once the leaf worktree is
   removed. So: before removing a shared leaf worktree, any HEAD/ref/reflog
   tip not reachable from the shared repository's refs or remote-tracking refs
   must be salvaged exactly like a private tip today (bundled / rescue-pushed
   by the existing salvage machinery), and only then
   `git worktree remove --force <slot>/<subpath>` run from the primary
   submodule checkout, before the superproject `git worktree remove`.
   The shared `.git/modules/<name>` itself is never removed by slot
   retirement. The census / retained-modules paths (`worktree_modules`,
   `_retain_worktree_modules`) must treat a shared leaf (gitdir under
   `.git/modules/<name>/worktrees/`) as "no per-slot module repository to
   retain": the administration `modules/` directory should not even exist for
   a fully shared slot.
5. Migration / reclaim for existing slots (`wrkslots` new subcommand, name to
   follow CLI conventions, e.g. `share-submodules`): read-only survey and
   dry-run by default; `--apply` to convert. For one named registered slot
   (own project's slots only; never another project's root): inventory each
   per-slot module repository under `.git/worktrees/<slot>/modules/`
   (existing `_worktree_module_directories` walk), list its HEAD, refs and
   reflog tips, and for each tip verify reachability from the shared
   repository; transfer what is missing into the shared repository first
   (fetch from the slot module repository into a wrkslots keep-ref in the
   shared repository, read back), so no object the slot needs is lost. Only
   when every tip is reachable in the shared store: remove the slot's
   submodule checkout, add the linked worktree of the shared repository at
   the same path and HEAD, verify `git submodule status`, status and log in
   the slot, then delete the private module repository and report bytes
   before/after per submodule. Any tip that cannot be transferred or verified
   refuses that submodule and leaves it untouched. Interrupted conversion must
   be resumable or safely restorable (journal, following the existing
   create/remove journal patterns); a half-converted slot must never be left
   with a submodule pointing at a deleted private store.

Implementation scope: `py/wrkslots/cli.py` (create preparation, identity,
retirement, migration command, help text), `py/wrkslots/USER_GUIDE.md` /
README as the CLI surface requires, and tests in
`py/wrkslots/tests/test_lifecycle.py` (or a focused sibling) covering the
Acceptance Criteria. No consumer is named anywhere; scratch trials stay under
/tmp.

# Acceptance Criteria

- New slot with a real submodule: the slot's submodule gitdir is a linked
  worktree of the primary `.git/modules/<name>` repository; measured
  before/after bytes per new slot on a scratch project are reported (stock
  per-slot clone vs shared), with the shared increment in KB/low MB, not the
  submodule's object-store size.
- A private commit made in a slot's shared submodule survives
  `git gc --prune=now` run in the primary submodule repository, and survives
  slot creation of a second slot sharing the same submodule.
- The primary submodule checkout and a second peer slot are byte- and
  status-identical before and after create, a submodule identity write, and
  retirement of the first slot (extending the existing peer-preservation
  tests, e.g. `submodule_peer_snapshot`).
- Retirement salvages an unpushed commit made in a shared submodule (bundled
  like the top-level checkout) before removing the leaf worktree registration,
  and never deletes the shared `.git/modules/<name>` store.
- Migration dry-run on a stock (private-clone) slot reports per-submodule
  private bytes, tips and reachability without changing anything; `--apply`
  converts it, the slot's submodule HEAD/history are intact afterwards, the
  private store is gone, and the reclaimed bytes are reported. A slot whose
  submodule holds a tip unreachable from, and untransferable to, the shared
  store is refused and left untouched.
- Submodules with no initialized source module repository keep stock
  behaviour and are reported as unshared at create; the existing
  "source submodule is unavailable" refusal behaviour (#910/#871) is
  unchanged for salvage.
- Existing wrkslots lifecycle tests pass; new focused tests are kept with the
  project.

# Notes

Filed by mu-submod (2026-11-10) from the coordinator's brief
(2026-11-10 8:05 AM ET). Design above is awaiting the coordinator's OK before
implementation (brief step c). Scratch evidence: /tmp/scratch3.sh,
/tmp/scratch5.sh (ephemeral, this host).

Design approved by the coordinator 2026-11-10 with four points: (1) primary
preparation explicit/idempotent/logged with a doctor check mode, refusing
non-expected states; (2) migration dry-run by default, live-use census or
per-slot --owner-consent, bytes per slot in the dry run; (3) measure gc/fetch
side effects at about 100 linked worktrees; (4) #1096 identity --worktree
scoped for shared submodules.

Implementation notes (mu-submod, 2026-11-10): create shares before hooks via
_share_checkout_submodules; preparation is _prepare_shared_submodule_repo
(logged, _shared_module_prep_state refuses unexpected states); doctor emits
shared-submodule-unprepared / shared-submodule-private findings; retirement
narrows _submodule_private_tips for shared worktrees to their own
HEAD/per-worktree refs/reflogs and _GitVcs.remove_worktree removes only the
stale shared registration; migration is `share-submodules SLOT [--apply]
[--owner-consent]`. Two consequences found while implementing: a shared
slot's submodule origin is the shared repository's own remote (not a stock
clone's local-mirror URL; per-worktree remote.origin.url overrides were
rejected because they break the exactly-one-fetch-URL authority check), and
scripts/setup-hooks.sh installers still run in shared submodules (they are
the same repository the primary's installer configures).

Scope change after the first full gate (mu-submod, 2026-11-10): the first
implementation shared by default, and the gate failed 161 lifecycle tests,
all in paths that assume the stock layout (recover-absent-agent-row
submodule recovery, abort-create administration pruning, the Rust edition's
create differential). Those paths do not yet handle shared layouts, so a
default-on create would manufacture slots their recovery cannot judge.
Sharing is therefore opt-in as `share_submodules: true` in the project
configuration (absent/false keeps the historical private clone; list the
key in ignorable_keys for older readers). Regular remove, salvage, doctor
and the share-submodules migration handle shared slots now; absent-agent
recovery parity and the Rust edition are follow-ups, and the default stays
off until they land. The consumer project gets the full disk win by setting
the one key.

Measured side effects (scratch, git 2.53.0-Meta, /tmp/submod-measure,
/tmp/measure100.sh): leaf repository of 300 commits; primary
.git/modules/leaf 116,138 bytes. One stock slot's private module clone:
116,154 bytes per slot. 100 shared linked worktrees: 50,692 bytes of
worktree administration in total (about 507 bytes per slot), shared
repository 116,138 -> 168,118 bytes for all 100 slots. Raw creation of 100
(superproject + leaf worktree adds) 11.97 s, removal 8.67 s.
`git gc --prune=now` in the shared repository with 100 worktrees: 0.26 s
(0.29 s with a private slot commit present, which survived).
`git fetch` of one new upstream commit: 0.098 s shared vs 0.047 s in a stock
private clone (tiny repository; no order-of-magnitude cost). Earlier trial
(60-commit leaf): stock per-slot clone 47,112 bytes vs primary-plus-one-shared
47,598 bytes total.
