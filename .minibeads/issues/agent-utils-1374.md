---
title: 'wrkslots-prepared-journal-abandon: namespace-independent ownerless identity, refuse before journaling, sanctioned abandon of a prepared journal'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-12T15:52:00+00:00
updated_at: 2026-11-12T15:52:00+00:00
---

# Description

[opus 5.5] A consumer's `recover-ownerless-agent-worktree --apply` refused during salvage: `git ls-remote` failed because a credential helper was missing. The refusal left `ACTIVE.<machine>.journal` in phase prepared, with salvage `[]` and the fenced move never done, and that journal blocked every mutation. Bare `recover` then refused with "ownerless agent worktree identity changed; preserve it", although st_dev, st_ino, HEAD and branch all matched.

Root cause: the journal's directory identity was (st_dev, st_ino, mnt_id), and mnt_id (from /proc/self/fdinfo) is local to the caller's mount namespace. The consumer's wrapper ran the prepare step in a private mount namespace (mount 5385) and bare `recover` on the host (mount 386), so the same directory looked different. No verb could set the journal aside: `quarantine-journal` handles create journals only, and the only way out was to resume from the original namespace, which ends when that process exits.

The consumer's review of its wrapper found two more ancestry defects:
(1) The `--apply` handlers of `recover-ownerless-agent-{worktree,cache,clone}` captured the coordinator with `_capture_caller_process`, which requires the coordinator to be in the invoking process ancestry. Run through the host path (systemd-run --user), they refused "coordinator PID ... is not in the invoking process ancestry". `remove` and bare `recover` accept the host-context handoff (`_capture_remove_processes` / `_RecoveryProcesses`).
(2) Bare `recover` passed `processes=` only for worktree journals, so it could not resume clone or cache journals on a host path.

# Design

- **Identity.** New ownerless-agent journals record `_fd_private_cleanup_identity`: (st_dev, st_ino, file-handle type, file handle), the namespace-independent key private cleanup already uses. The file handle comes from name_to_handle_at, names the same object from every mount namespace, and changes when the inode is reused. `_ownerless_identity_matches` compares the full key at every destructive boundary: the facts proof and the fence check. Journal validation compares only st_dev and st_ino. Validation runs on every load, in both editions, and never acts. The Rust edition forbids unsafe code and cannot read file handles, so the split also keeps the differential exact. A legacy three-field identity compares only st_dev and st_ino. On resume, `_ownerless_agent_facts` re-proves the worktree (HEAD, branch, remote digest, Git registration, handoff) and the journal is upgraded to the four-field identity before anything destructive relies on it. This mirrors `_upgrade_legacy_private_cleanup_identity`. The parsers in both editions accept three or four fields.
- **Refuse before journaling.** `_assert_ownerless_salvage_reachable` runs before the journal is written. It reads the salvage facts of the worktree and every initialized submodule, plus the same remote listing (ls-remote through the configured network authority) that salvage reads first. A missing credential helper or an unreachable remote now refuses with no journal and no progress event. Salvage repeats every check before it publishes.
- **Sanctioned abandon.** `recover-ownerless-agent-worktree PATH ... --abandon-prepared` prints a dry run with the journal's sha256. `--apply --coordinator-authorized --coordinator-pid P --journal-sha256 S [--reason TEXT]` performs the abandon. The caller passes the same facts as the `--apply` that wrote the journal (repository, head, branch, remote, digests), and any mismatch refuses. Only a journal that never acted qualifies: kind ownerless-agent-remove, phase prepared, no salvage receipt, fenced path absent, and the worktree either unchanged by st_dev and st_ino or absent. Under the mutation locks the command re-checks the sha256, appends an `operation-quarantined` tombstone (operation `ownerless-agent-remove`, the same payload keys as `quarantine-journal`, so both editions' pending-operation folds already close it), and moves the journal into the control directory's `quarantine/` subdirectory, where the quarantined-journal census lists it. The worktree is kept, and a later removal starts from scratch.
- **Host-context coordinators.** The three ownerless `--apply` handlers capture the coordinator with `_RecoveryProcesses.capture`. Without the runner, generation and proof-FD evidence, that is still `_capture_caller_process`, so every non-host refusal stays the same. Each handler re-checks the coordinator under the lock and passes `processes=` to `_recover_ownerless_agent`, `_recover_ownerless_clone` (and its steps) and `_recover_ownerless_agent_cache`. Bare `recover` passes `processes=` for clone and cache journals too.
- **Rust.** `journal_state` parses three- or four-field identities with Python's messages and compares st_dev and st_ino. The resume plan in `recover_ownerless_write` accepts both shapes. The compat shell refuses `recover-ownerless-agent-worktree --abandon-prepared`, `--journal-sha256` and `--reason` by name, since the abandon stays with the Python edition. The Rust apply paths already refuse "stays with the Python edition" and never checked ancestry, so items (1) and (2) need no Rust change.

# Acceptance Criteria

- A journal written in one mount namespace resumes from another: a real `unshare --user --mount` subprocess, plus a test where every mount ID the resume reads is different. A legacy three-field journal with a foreign mount ID resumes, and its identity is upgraded first. A legacy journal whose inode differs still refuses.
- A refused precondition (the remote is unreachable) leaves no journal and no progress event, and the same command succeeds once the remote is back.
- `--abandon-prepared` sets aside a prepared, never-acted journal, keeps the worktree, writes the tombstone, and leaves nothing outstanding; it also accepts a worktree that is gone. It refuses: phase preserved, phase fenced, salvage receipts in phase prepared, a present fenced path, a replaced worktree, different facts, a stale sha256, a missing `--coordinator-authorized`, and a missing `--journal-sha256`.
- The worktree, clone and cache `--apply` commands and bare `recover` for clone and cache journals accept a host-context coordinator outside the ancestry. Without the handoff they still refuse on ancestry.
- The Rust differentials match Python for three- and four-field journals (parse, foreign mount ID, refusals, resume plan), and the compat shell refuses the abandon flags by name.

# Notes

Number reserved in the devbig014 issue ledger. The consumer's wrapper should also send prepare and resume through the same context; that change lives in the consumer.

Ownerless clone and cache journals still record (st_dev, st_ino, mnt_id) and so have the same namespace limit. They were left unchanged here.

## Resolution

Fixed on branch cl-wsrecover/1374. Tests: `py/wrkslots/tests/test_ownerless_journal_ns.py`, plus new cases in `test_journal_state_rust.py`, `test_recover_ownerless_write_rust.py` and `test_cli_compat_rust.py`.
