---
title: 'wrkslots-absent-cargo-home-journal: an interrupted cargo-home removal whose paths are gone must not block every validation'
status: open
priority: 0
issue_type: bug
assignee: mu-validate
created_at: 2026-11-05T07:10:44.521788465+00:00
updated_at: 2026-11-05T07:10:44.521788465+00:00
---

# Description

P0. An interrupted `ownerless-validate-remove` mutation of a validate cargo home has blocked every validation on a consumer host since about 01:55 ET on 2026-11-05, merge queue included.

**State**
- The host's `ACTIVE.<host>.journal` records:
  - kind `ownerless-validate-remove`, phase `prepared`, target_kind `cargo-home`;
  - path `ignored/validate/cargo-homes/validate-cargo-<id>`, and a fenced sibling `.validate-cargo-<id>.ownerless-validate.<hex>`;
  - authorization evidence: terminal-record, `no_authored_work=true`, `no_live_use=true`.
- Both the target and the fenced path are now absent; something outside wrkslots deleted them, and that is being investigated separately.
- A dangling `cargo-homes/slot-N -> validate-cargo-<id>` symlink remains.

**Symptoms**
- Every validate admission fails in classify-create-journals with "REFUSED: interrupted 'ownerless-validate-remove' mutation ACTIVE.<host>.journal cannot coexist".
- `wrkslots recover --coordinator-pid ...` exits 3 with "validation path disappeared during prepared cleanup; removal outcome is UNKNOWN, so preserve the journal and inspect the authorized identity".
- recover offers no way forward, and the operators will not hand-edit state.

# Design

(a) classify-create-journals: an interrupted `ownerless-validate-remove` whose target_kind is `cargo-home` does not block creating a new, different validate slot, because it touches a different path. Keep blocking when the new slot would reuse the journal's slot or paths, and for worktree kinds.

(b) recover: for `ownerless-validate-remove` with target_kind `cargo-home`, when the target AND the fenced path are both absent and the recorded authorization has `no_authored_work=true` and `no_live_use=true`:
- finalize the journal as outcome `absent`;
- append an event-log record naming the journal, the slot and the evidence;
- remove a dangling `slot-N` symlink that points at the absent cargo home.

Keep refusing for worktree target kinds, when either path exists, or when the authorization lacks either flag.

# Acceptance Criteria

- A test with a fixture journal in exactly this state shows that admission of a new validate slot succeeds (a), and that recover finalizes it as absent, writes the event, and removes the dangling symlink (b).
- Negative tests:
  - a worktree target kind is still refused;
  - a present target or fenced path is still refused;
  - missing authorization flags are still refused.
- (b) lands FIRST and alone; (a) follows. The consumer's validations run the agent-utils wrkslots pinned in its own submodule (f79789533dc0), and a newer pin can only land through the merge queue this journal blocks. The unblock therefore runs through a user-site wrkslots carrying (b).
- Everything (b) writes (the event record, the journal removal, any state change) is readable by the pinned f79789533dc0 wrkslots, and that version's classify-create-journals passes on the post-recover state. A test runs f79789533dc0's `py/wrkslots` against the post-recover fixture.
- The SHA is sent to the lead at once for the consumer host's deploy.

# Notes

Reported by the lead on 2026-11-05 at 02:10 AM ET as P0. Assigned to mu-validate, which pauses #864 for it.
