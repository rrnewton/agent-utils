---
title: 'stale-root-decisions: ready decisions wait in plan order behind still-running stale roots, and stale roots drop unrelated stacked greens'
status: closed
priority: 1
issue_type: bug
assignee: mu-predict
created_at: 2026-11-08T23:54:22.000000000+00:00
updated_at: 2026-11-09T04:13:48.000000000+00:00
closed_at: 2026-11-09T04:13:48.000000000+00:00
---

# Description

Three linked defects in the landing loop around green disjoint roots whose base moved, all in one live run on 2026-11-08 with `keep_stale_roots` on. Five roots started on one base; another batch then landed, making the remaining roots stale. Two stale roots came back green within two minutes but were not requeued for rebuild until a slow earlier-planned root finished more than seven minutes later, when all three were requeued at once (8.7 and 7.0 minutes of idle, finished validation). Later in the same run, a stacked flight went green already standing on the current base through its stacked parent, so it could have landed at once, but no landing followed for about nine minutes because an earlier-planned, still-running stale root sat ahead of it in plan order. Worst, when that stale root finally returned green, its branch discarded every stacked flight in the window, including the finished green one built on a different, fresh candidate: about 1062 s of validation silently thrown away (no event, no item history line), and the change later rebuilt from scratch and landed about 35 minutes late. The run-loop causes: the landing pass only pops a decided result from the front (with one reorder for decidable roots), and the stale-root-green branch calls the general `discard`, which keeps only flights that are rooted, culprit-search probes, or red batches waiting on probes.

# Design

- A stale green disjoint root is requeued for rebuild as soon as its own verdict arrives, independent of other disjoint roots' plan order (disjoint by construction, their results cannot affect it).
- A green flight whose whole candidate ancestry has landed lands on arrival: the landing pass skips a stale still-running disjoint root, which is decided when its own result arrives. Order is preserved wherever a flight's own ancestry or a landing conflict requires it (stacked chains still land in ancestry order).
- The stale-root-green branch discards only flights whose candidate descends from the stale root's candidate tip (parent-chain ancestry; none for a never-promoted root). Stacked flights built on the current base or on other roots are kept.
- No silent drops: every flight a discard drops gets an event and an item history line naming the dropped flight and its discarded result (for example, dropped with a green result), so thrown-away validation is always visible in the run record.
- No policy change to landing rules themselves: a stale green still never lands without its rebuild (the pending level-0 delta question is untouched).

# Acceptance Criteria

- A stale green root is requeued within the same decision pass in which its verdict arrives, even while an earlier-planned disjoint root is still running (test with a slow earlier root holding back a later stale green).
- A finished green stacked flight whose parent has landed lands immediately even with a stale still-running disjoint root ahead of it in plan order.
- The exact three-flight shape is tested: stale root A returns green while stacked B sits on fresh root C - B survives (and lands when C has landed), while a flight actually stacked on A is discarded, and each discard emits its event and item line.
- Existing disjoint-root, stale-green and kept-roots behaviour at defaults is unchanged apart from the prompt decisions and narrower discard above (full merge-queue step passes).

# Notes

Reported 2026-11-08 from one live run (20:06 landing, then 20:07 and 20:12 green results with no decision until the stale root arrived at 20:14; a discarded green rebuilt at 20:19). Combines three coordinator items from the same run and code path, after the disjoint-root stacking work landed. A simulate case is welcome if cheap, but the gated run tests are the required evidence.

## Resolution

[watermelon-full-preview] Landed on main as 790c0719eba7660b82b6ef57da7cc244cad1e2ce. The decision loop requeues a stale green disjoint root at any index on its own arrival, selects a later plain green standing on the current base behind stale still-running roots only (dependencies and plan conflicts still block), discards only git/parent-chain descendants of a stale candidate, and records every discard-path drop as a `flight-discarded` event plus item history with its captured verdict. The exact six-flight gated test proves prompt requeue, descendant-only discard with the green recorded, and B landing while the slow root is still gated. Gate note, stated plainly: the nested exclusive cross-differential ticket was stopped (verified PID 4192910) while two main-phase failures were still outstanding, which lapses the clean-main-phase waiver condition; no redo was needed for this merge-queue-only diff after both failures classified clean - agentctl sigterm timing and an ETXTBSY fake-executable race in one validate unit (exact test passes locally; whole-crate rerun clean). Exact refreshed-tree evidence: `rust.merge-queue.test` rerun pass on f0fab399 (593 cases, 1082.345 CPU-s), fmt/clippy -D warnings/client-names exits 0.
