---
title: 'wrkslots-relocate-unbind-under-records-lock: relocate and unbind still mount and unmount outside the bind-records lock'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-24T13:10:00-05:00
updated_at: 2026-11-24T13:10:00-05:00
---

# Description

#2178 put bind establishment (ensure_binds, bind_slot, ensure_image_mounted) under the bind-records flock, but relocate_bind still calls _bind_private outside the lock (including its rename-failure rebind at the source), and unbind_slot/unmount_bind unmount outside the lock. An unmount outside the lock can race a locked bind or collapse of the same target: a collapse that has just re-read the mount table can unmount a layer another actor is removing, and a relocate can re-stack a bind at its source while a collapse is walking the stack down.

# Design

Take the bind-records lock around relocate_bind's unmount/rename/bind sequence and around unbind_slot's unmount, with the same bounded wait as bind establishment, keeping each slot's work under the lock constant-time. Preserve the existing crash orders (unmount, rename, mount, record for relocate; unmount, then record for unbind) and their refusal behaviour.

# Acceptance Criteria

- relocate_bind and unbind_slot perform their mounts and unmounts under the bind-records lock.
- The documented crash orders and refusal messages are unchanged.
- Deterministic tests (fakes only) prove a concurrent bind or collapse of the same target serialises with a relocate/unbind instead of interleaving.

# Notes

Follow-up split out of #2178's review, where the remaining unlocked mounters were named. Operators are told in the user guide to collapse a doubled slot before removing or relocating it; this issue closes the in-tree race behind that advice.
