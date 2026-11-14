---
title: 'vibe-talk-recap-drops-thread-replies: recap silently drops in-window thread replies of pre-window threads'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-14T00:00:00.000000000+00:00
updated_at: 2026-11-14T00:00:00.000000000+00:00
---

# Description

`vibe-talk recap --hours 36` silently drops thread replies that are inside the window. A thread whose root is older than the window but has replies inside it shows 1 message at --hours 36 and 4 at --hours 60; another shows 1 vs 15; 20 owner messages at 36 h vs 45 at 60 h. The drop is in the upstream read path, not the cache read: for Slack, the cache holds no Flat scope, so recap's flat walk reads upstream, and a flat page only surfaces the replies of roots on that page (with a per-page cap on threads read), so replies of a pre-window root never arrive. The thread-opening walk also fetches a thread's messages but keeps only its root and opening, discarding any in-window replies it read.

# Design

In `ops::recap`, after the flat walk, sweep the Threads view for threads whose latest activity is inside the window and read each such thread's Thread timeline, merging every message whose own time is in the window (dedup by id afterwards). A backend that refuses thread reads keeps the flat answer alone. The window stays half-open (`since_ms <= created < until_ms`) and every bound still reports `truncated`.

# Acceptance Criteria

- Every message whose own time is in the window is returned, whatever its root's age.
- Regression test: a fixture backend whose Flat view carries only main-timeline messages (roots and standalones, as Slack's does) with an old root and in-window replies, plus Threads and Thread views, returns the in-window replies. Fixed instants only, no wall clock.
- Existing recap behaviour is unchanged: per-message Done tags, noise counting, context openings, the page-bound truncation notice, and the no-thread-timeline fallback.

# Notes

Owner tooling request 058, first of two. No consumer names, host names or local paths are recorded here.

## Resolution

[watermelon-full-preview] Fixed in `vibe-talk/src/ops.rs` (`recap`): a Threads-view sweep collects threads active in the window and merges their in-window Thread-timeline messages into the window before grouping. Regression test `recap_returns_in_window_replies_of_a_pre_window_thread` in `vibe-talk/tests/recap.rs` uses a main-only Flat fixture backend; all 8 recap integration tests pass.
