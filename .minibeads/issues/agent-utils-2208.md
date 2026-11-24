---
title: 'wrkslots-seal-recovery-empty-census-data-loss: seal recovery deletes a checkout on an unproven census'
status: open
priority: 0
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-24T07:34:00-05:00
updated_at: 2026-11-24T07:34:00-05:00
---

# Description

HIGH (cold gate audit finding H6). Seal recovery on the paired-finish
path (`_finish_remove_paths` in `py/wrkslots/cli.py`, landed by
919e765f for #2128 wrkslots-seal-recover-scoped-census) substitutes an
empty census when the holder census cannot close on a busy host, then
deletes the checkout; the code calls this an "Accepted residual". When
the recorded owner is proven dead and the recorded owner cgroup reads
empty (or is absent), a same-UID census that raises
`_CensusDidNotClose` with no matches yet is replaced by
`_ProcessPathCensus((), (), owner_cgroup_complete=False)`, the
unused check passes vacuously, and the destructive finish runs. The
owner-cgroup decision cannot see a holder outside the owner cgroup (a
step in a sibling scope, a shell in the checkout), and the unclosed
census is exactly the case where such a holder may never have been
scanned: on a hot host this is a data-loss path that deletes a
checkout under a live process.

# Design

Fail closed and by name. Never substitute a census that did not
close. When the census cannot close, seal recovery refuses by name:
the refusal names the slot, states that the census did not close so
the checkout cannot be proven unused, states that the checkout, the
finish journal and the seal are retained (the existing fence rollback
already leaves them in place), and says to retry `wrkslots recover`
when fewer processes are starting. The recorded-owner-cgroup decision
keeps its current roles only: a populated cgroup whose member uses
the slot refuses, and a present but unreadable cgroup refuses
fail-closed; an empty cgroup alone no longer clears the checkout.
No new persisted field, no new event kind or key: the change is
refusal behaviour and text only, so pinned old readers are
unaffected. The Rust mirror is checked too: its census capture
already refuses on non-closure and its destructive batch recovery
defers to the Python edition, so the fix is pinned there by test.

# Acceptance Criteria

- A planted deterministic case (recorded owner proven dead, owner
  cgroup empty, holder census that deterministically does not close
  and found nothing) refuses by name and leaves the checkout, the
  finish journal and the seal in place; the same test deletes the
  checkout on the unfixed code.
- A deliberately broken copy of the fix (the empty-census
  substitution restored) fails that test.
- A census that closes, a census refusal carrying a match, a
  populated owner cgroup with a member using the slot, and an
  unreadable owner cgroup each keep their current verdicts.
- Rust census capture that cannot close returns an error, never an
  empty success.
- No persisted-format change.

# Notes

Pre-assigned #2208 wrkslots-seal-recovery-empty-census-data-loss.
Follows #2128 wrkslots-seal-recover-scoped-census (919e765f), whose
substitution this issue removes; the scoped cgroup decision itself
stays for its refusal cases.
