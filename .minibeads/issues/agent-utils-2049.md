---
title: 'wrkslots-dir-fsync-failure-after-replace: a failed directory fsync after the rename is reported as not committed'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T09:47:00-05:00
updated_at: 2026-11-21T09:47:00-05:00
---

# Description

[watermelon-full-preview] Found by the #2043 fault harness (mu-slotmodel). An EIO on the directory fsync after `os.replace` in `_atomic_write_bytes` (py/wrkslots/cli.py), swept by offset on an unbound create: offsets 0-2 refuse consistently with nothing durable; offset 3 (the CREATE journal replace) refuses rc 3 with "cannot durably replace state file" although the journal is durable and one `recover` completes the create; offsets 4-5 escape provisioning as an uncaught OSError traceback from `slotimage._write_record`, with the journal likewise committed. The reverse case (success reported, nothing durable) was not seen.

A failed directory fsync after a successful rename means "renamed but durability unknown": the rename has landed, only its crash durability is in doubt.

# Design

- Split the atomic write at the rename in both editions: failures before or at `os.replace` keep the consistent "cannot durably replace" refusal; a directory-fsync failure after the rename raises a classified refusal saying the state may be committed and naming `wrkslots recover`, with no retry: on Linux a failed fsync can clear the error state (fsyncgate), so a second fsync returning 0 would not prove durability.
- `slotimage._write_record` gets the same handling as `ImageError`; the Rust mirrors (`ensure_states::atomic_write_bytes`, `image_record::write_record`) match.
- No persisted-format, journal, or lock change; only error classification and message text.

# Acceptance Criteria

- Regression `test_fault_fsync.py` (recreated from the #2043 harness, pinned contract flipped): offset 0 refuses consistently with nothing durable; the journal-boundary offset reports may-be-committed with the journal durable and one recover completing the create; the provisioning record write classifies the same way; the sweep never reports success without durable state.
- No uncaught traceback on any offset.
