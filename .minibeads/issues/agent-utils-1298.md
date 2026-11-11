---
title: 'review-seat: a reviewer agent runs inside the implementors slot as a read-only co-tenant'
status: open
priority: 0
issue_type: feature
created_at: 2026-11-11T14:48:05.706949692+00:00
updated_at: 2026-11-11T14:48:14.563623828+00:00
---

# Description

Owner direction (Slack 6225, 2026-11-11): a reviewer should inherit the slot the implementor used and go back and forth like coroutines; today reviewers are given fresh slots. A fresh reviewer slot repeats the full checkout (worktrees, submodule stores, build caches) to read a tree that already exists, reviews a reconstruction rather than the implementor's exact state (uncommitted notes, generated files, local config), and hand-off is by message archaeology instead of by turn. Putting a reviewer in the slot today is possible by starting an agent with its working directory in the slot and no slot of its own, but that seat is fully writable with no enforcement: nothing stops the reviewer editing the implementor's tree, racing the implementor, or being mistaken for the slot owner by lifecycle commands.

# Design

Add a review seat: start --review-seat <slot> (name settled in implementation) starts a second agent whose working directory is the implementor's existing slot. The slot stays owned by the implementor; the slot tool records the reviewer as a co-tenant (reviewer name, harness pid, since when), never as owner. The seat is enforced read-only for the slot tree through the existing box (user-namespace isolation where root isolation is unavailable, root isolation where it is), writable only under its own verdict directory inside the slot (for example <slot>/.review/<reviewer>/ holding its verdict and status files). No new slot, worktree, or submodule clone is created.

Coroutine turn-taking: an explicit handoff verb moves the turn implementor -> reviewer -> implementor; who is on is recorded in the slot/agent state both sides and status can read, and the watcher delivers the turn notice to the side now on. Only the side that is on may be sent work that writes; sending work to the off side queues it behind the turn instead of typing it. The implementor pausing or stopping does not strand the seat: stopping the reviewer never affects slot ownership, and slot removal refuses while a seat is occupied until the seat is stopped or explicitly cleared. Muse seats work at least through user-namespace isolation; other harnesses through root isolation where available, refusing a seat with no isolation rather than seating writable.

# Acceptance Criteria

- Starting a review seat in an occupied slot creates no slot, worktree, or submodule clone; slot status lists the implementor as owner and the reviewer as co-tenant, and the reviewer's record names the seat.
- The reviewer can read the whole slot tree and write only under its verdict directory; an attempted write elsewhere in the slot fails in both isolation modes, and stopping the reviewer leaves the implementor and slot untouched.
- Handoff moves the turn exactly once per invocation, records who is on, and the watcher notifies the side now on; work sent to the off side queues behind the turn and delivers when it comes on.
- Slot removal and finish refuse while a seat is occupied, naming the reviewer and the stop/clear command; after the seat stops they proceed as today.
- A seat requested with no available isolation refuses instead of seating writable; fake-harness tests prove read-only enforcement, turn delivery, and removal refusal in both editions.

# Notes

Owner direction Slack 6225 (2026-11-11); P0 for implementation by a fresh lane. Part B2 of the spawning-in-tools batch: B1 is the spawn-together issue filed alongside this one, B3 is the harness-layout-mode refusal. Today start with a slot working directory and no slot already seats a reviewer writable and unenforced; slot boxes are writable-slot only and root isolation does not cover every harness, which the design answers per harness above.
