---
title: 'wrkslots-local-shard-default: audit, classify-create-journals and clean-caches read other machines'' shards only under --all-machines'
status: closed
priority: 2
issue_type: bug
assignee: mu-validate
depends_on:
  agent-utils-876: blocks
created_at: 2026-11-05T08:36:45.091501022+00:00
updated_at: 2026-11-05T11:38:44.890559824+00:00
closed_at: 2026-11-05T11:38:44.890559714+00:00
---

# Description

`wrkslots audit`, `classify-create-journals` and `clean-caches` read every machine's state shard through an ungated `ACTIVE.*` glob. `status` and `doctor` read foreign shards only under `--all-machines`, by design, and the retire-pending provider is local-only.

So a stale foreign shard skews a host's audit and classify verdicts. That means a copied `ACTIVE.<other-host>.json`, or an `ACTIVE.<other-host>.journal`, left in a project's control directory.

This happened on 2026-11-05. A two-week-old copy of another machine's table sat in a consumer project on the second host. Its cleanup check reported NO RESULT because of a row that belonged to the other machine, and that row could only be recovered there. The consumer has since archived the stale copy and made its own reaper local-only. wrkslots itself still reads foreign shards.

Example sites, from a read of `py/wrkslots/cli.py`:
- `config.control.glob("ACTIVE.*.json")` (around line 8474);
- `glob("ACTIVE.*.journal")` (around lines 25291, 25587 and 31536);
- the tmp-leftover globs (around lines 25257-25259).

Check each site for which command reaches it; the Rust edition (wrkslotsd) may have the same reads.

# Design

- One helper decides which shards a command reads: the local machine's shard by default, and every shard only under `--all-machines`. Use it in `audit`, `classify-create-journals` and `clean-caches`, matching `status` and `doctor`.
- Under the default, a foreign shard is not silently ignored when it is present. Print one NOTE naming it, its machine and its mtime, so a stale copy is visible without skewing the verdict.
- Keep any read that is genuinely global, for example a cross-machine path-collision check, and say why in a comment. Do not let it change a local verdict's status.
- Give both editions the same behaviour, and update the CLI help and userguide for `--all-machines` on these commands.

# Acceptance Criteria

- A test plants a stale foreign `ACTIVE.<other>.json` (with an absent-storage active row) and an `ACTIVE.<other>.journal`.
  - By default, `audit`, `classify-create-journals` and `clean-caches` report the same verdict as without them, plus the NOTE.
  - Under `--all-machines`, the foreign rows are reported as today.
- Existing `status` / `doctor --all-machines` behaviour is unchanged.

# Notes

Reported by the lead on 2026-11-05 at 03:36 AM ET, from a consumer's reading of ec150b5e. P2; it is in the same family as #864 and #871. Assigned to mu-validate after #872 (a); order it among #864, #871 and #868 as convenient.
