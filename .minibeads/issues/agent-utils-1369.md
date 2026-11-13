---
title: 'wrkslots-finish-provisioned-files: finish tolerates unchanged provisioning output'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-12T13:00:00.000000000+00:00
updated_at: 2026-11-12T13:00:00.000000000+00:00
---

# Description

[watermelon-full-preview] A downstream team's provisioning creates an ignored symlink (and an
ignored directory tree inside a submodule checkout) at create time, through its own
post-provision hooks. `wrkslots finish` then refuses every fresh slot with "checkout is dirty
or has untracked/ignored files", naming that provisioned file. A lane that did everything
right cannot finish, so the mandatory wind-down never ends in a removed slot.

# Design

- At create time, after the post-provision hooks run, record what provisioning made in the
  slot record: every untracked/ignored path in each checkout and each submodule checkout,
  with kind (file, symlink, directory) and the symlink target or file hash. Small trees are
  recorded as an exact recursive manifest; a directory over the size/file-count threshold
  is fingerprinted by (path, kind, file count, total bytes, max mtime) instead of per-file
  hashes, because an exact manifest of a several-hundred-megabyte vendor tree at create
  and again at finish is too costly. Trade-off: a content change that preserves count,
  bytes and max mtime passes for such a tree; recorded provisioning output is build output,
  so that risk is accepted and documented here, not hidden.
- Provisioning outputs (recorded at create, or a declared list in config) count as removable
  build output at finish, not dirty work. Finish ignores recorded paths while they are
  unchanged (same kind, target, hash, manifest, or large-tree fingerprint; a missing
  recorded path counts as changed) and still refuses when one changed or anything else
  is dirty.
- Escape hatch and fallback for slots created before this record existed: a configured
  allowlist of checkout-relative paths/globs that finish tolerates. Slots with no record
  rely on the allowlist alone; that weaker rule is documented where the key is defined.
- Both editions (Python and the Rust port), keeping the finish differential exact.

# Acceptance Criteria

- A hook-created ignored symlink lets finish pass; the same file modified refuses.
- An unrelated ignored file refuses.
- An existing slot without a provisioning record behaves per the documented fallback.
- A hook-created ignored directory inside a submodule checkout is covered by the snapshot.

# Notes

[watermelon-full-preview] Follow-on to #1364 wrkslots-finish-commit-accounting, same finish
surface. Number reserved by coordb (fetch-then-land numbering).

## Resolution

[watermelon-full-preview] Landed as fe29e325 on github/main.
