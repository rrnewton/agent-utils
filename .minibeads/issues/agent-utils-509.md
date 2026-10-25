---
title: 'agentctl-child-done-notice: tell the coordinator the moment a child finishes, and advance the landing queue'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
created_at: 2026-10-25T15:01:25.042769164+00:00
updated_at: 2026-10-25T15:01:25.042769164+00:00
---

# Description

[opus 5.5] A finished child waits for its parent. #508 parent-child-responsiveness measured it: a `DONE:` on a child's screen is not a notification, and `agentctl watch-agents` deliberately ignores an idle agent that shows a verdict, so DONE episodes waited 11.3 min p50 (finished children sat 12.1 min p50, max 28.6, until stopped) while the parent waited for its own periodic sweep. When the child sent a message instead, the parent acted in about 1 min. The watcher's only verdict-related notices so far were false "idle without a verdict" reports on long Muse replies: the verdict line sits at the start of the reply, above the 30-line marker window, and behind Muse's `◆ ` reply bullet.

Wanted, in the Rust agentctl (the Python edition already refuses `watch-agents` as Rust-only):

1. A `child-done` episode: idle or done, work delivered and drained, with a verdict in the last reply. Sent at once (`--child-done-seconds`, default 0), one message per child naming the verdict line, deduplicated per turn (a new verdict line or a new turn is a new episode), with deliver-while-working, routed through one notice-target function so a manager tree can redirect it later.
2. Verdict detection reads the last reply: the region after the last user-turn row (`❯ text`, `› text`, `> text`) above the composer, with a leading reply bullet (`●`, `⏺`, `◆`, `•`) stripped, instead of only the bottom 30 lines. An older turn's verdict above a newer user prompt no longer counts.
3. `--idle-minutes` default 15 -> 5. Batching per kind: `child-done` is never held by `--min-notice-interval` and does not hold other kinds back.

# Design

Landing queue (study recommendation 7), smallest design. The coordinator serializes landings by telling a ready child to hold; the go comes only when the coordinator next notices that the landing child finished (13 min p50 waits, tail 113 min). Proposal: `agentctl hold NAME --after OTHER [--note TEXT]` records "NAME lands after OTHER" in `REGISTRY/landing-queue.json` (one entry per holder; `agentctl hold --list` prints the queue, `agentctl hold NAME --release` drops an entry). Nothing else reads or writes that file except the watcher.

The trigger is the `child-done` episode above: when `watch-agents` reports OTHER finished with a `DONE:`-style verdict (any marker except `BLOCKED:`), the same notice to the coordinator gains a line "landing queue: NAME is next (NOTE)" for every holder waiting on OTHER, and those entries are marked released by that episode (re-announced only if the notice is rebuilt for the same episode after a crash). A chain (B after A, C after B) advances one step per landing. The watcher still types only into the coordinator: it never sends NAME the go itself, because the coordinator should check that OTHER really landed before releasing the next push.

Not covered: a LANDED that arrives only as a message to the coordinator (`agentctl send coordinator 'OTHER: LANDED ...'`) rather than as OTHER's closing verdict; reading the coordinator's queue would couple the watcher to the queue format that is moving into its own crate. Briefs already end with a `DONE:` after landing, which is the signal used.

# Acceptance Criteria

watch-agents queues a child-done notice in the pass that first sees a child idle with a verdict; long Muse replies whose reply starts with the verdict are not reported idle-no-verdict; a child-done notice is not delayed by the rate limit; the landing-queue line appears in the finishing child's notice; unit tests cover each; docs and help describe the new flags and defaults.
