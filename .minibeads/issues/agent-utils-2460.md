---
title: 'wrkslots-proof-mirror-retire-digest-only: proof-mirror retirement considers only mirror-digest entries; doctor flags any other entry under proof-mirrors'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
- reclamation
created_at: 2026-11-28T07:50:00-05:00
updated_at: 2026-11-28T07:50:00-05:00
---

# Description

Follow-up from the #2449 review. Proof-mirror retirement (`_proof_mirror_retirement_candidates` and `_retire_proof_mirrors` in `py/wrkslots/cli.py`, #1692, run by `clean-caches` on an apply) treats every entry of `<control>/proof-mirrors` as a mirror. An entry with no readable `record.json` counts as idle, because `fetched_at` falls back to 0. An entry whose name is not a live remote digest counts as orphaned. Both are passed to `shutil.rmtree`.

So any directory that is not a mirror and sits under `proof-mirrors` is deleted on the next `clean-caches` apply. #2449 closed the known path (a slot named `proof-mirrors` is now refused in the nested layout), but the retirement rule is still wider than the collection it manages.

A non-directory entry under `proof-mirrors` makes the `.lock` open (`os.open(entry / ".lock", ...)`) raise `NotADirectoryError`. Nothing catches it, so one stray file fails the whole apply run.

# Design

- Retirement considers only entries whose name is a 64-hex SHA-256 digest (the shape `_proof_mirror_directory` creates) and that are real directories, not symlinks. Every other entry is left in place.
- Doctor's existing proof-mirror finding names each entry that is not a mirror, with a remedy to inspect it. It removes nothing.
- A non-directory entry is skipped and reported, never opened.

# Acceptance Criteria

- A test plants a non-digest directory, a file and a symlink under `<control>/proof-mirrors` beside an idle digest mirror. After a `clean-caches` apply, only the digest mirror is gone, the apply exits 0, and doctor names the three other entries.
- The test fails on the base commit.

# Notes

Found by the consumer's review seat while reviewing #2449, as F4.
