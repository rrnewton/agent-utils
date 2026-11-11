---
title: 'wrkslots-migrate-signature-reclaim: migrate compare proofs and original reclaim'
status: open
priority: 2
issue_type: task
created_at: 2026-11-11T10:00:00.000000000+00:00
updated_at: 2026-11-11T10:00:00.000000000+00:00
---

# Description

Python #1149 (6092bff3): the migrate compare now proves content,
not just shape, and binds migrate reclaims the original instead
of leaving it beneath the bind. Rust parity in the image
handlers:

- `listing_signature` gains mode and proof (SHA-256 of file
  contents, link target for symlinks), and the migrate compare
  also uses `git_state_signature` (HEAD plus porcelain status
  per checkout).
- Binds migrate order: census (glue), copy, compare, rename the
  original aside to `.<slot>.premigrate-<rand>`, bind, census
  the displaced copy, then remove it; a failed bind rolls the
  rename back.
- The remove guard's image side: unbind (unmount, then drop the
  record under the records flock), so a remove interrupted there
  retries from the bound state.

# Design

- All in the already-registered `worktrees_image` module; the
  lifecycle census, locks and registry sequencing stay with the
  dispatch/core glue, which calls these helpers.
- Fixture trees and fake runners only; git state is read from
  temp fixture repositories.

# Acceptance Criteria

- Signature tests prove same-size content change, mode change,
  and link retarget are all caught, and git state matches.
- Migrate reclaims the original on success and restores it on a
  failed bind.
- Unbind drops the record only after the unmount step.

# Notes

Part of #562 rust-primary-wrkslots P8. Follows #1279.
