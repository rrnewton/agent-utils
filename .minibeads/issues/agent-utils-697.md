---
title: 'send-muse-while-working: deliver plain sends and queued backlog into a busy Muse by default'
status: open
priority: 1
issue_type: bug
created_at: 2026-10-29T18:19:44.113033343+00:00
updated_at: 2026-10-29T18:19:44.113033343+00:00
---

# Description

[watermelon-full-preview] Owner rule (2026-10-29): "A goal should never block idle detection. All harnesses continue receiving messages even when they have a longer-term goal set." The chat bridge now delivers into a busy Muse fine (#678 landed, main 92f81d81). But plain `agentctl send` and the agent queue do not:

- Observed on a Muse coordinator registered by `agentctl adopt` (adapter herdr-foreign, harness muse, xhigh YOLO, inside a 14 h /goal turn): `agentctl send NAME --file F` with build 6ef86d9d returned after 900 s with outcome pending: "pane did not become idle/done within 900s; last status=working", although `send --help` says "Codex and Muse ordinary follow-ups are delivered while working by default". Its queue inbox grows all day (watcher notices, sub-agents' done notes, timer reminders: 17 items), and nothing delivers them until the turn ends, which can be hours.
- `send --urgent` did deliver (Muse took the steer in 20 s, no interrupt), BUT it first flushed the entire backlog ahead of its own message in one burst (13 items), which the owner found alarming.

Fix, with tests (unit plus a live check on throwaway Muse agents named ms-*, profile muse-watermelon):

1. Plain send and queue drains deliver into a busy Muse (and Codex) by default as the help promises, including for adopted/foreign panes. Find why the 900 s idle wait happened (adapter herdr-foreign? FIFO behind older items? the drain path vs the send path?).
2. Backlog delivery into a working agent should be paced and readable: e.g. batch queued items into one combined prompt ("N queued messages:" with sender headers), or deliver at most one per tool boundary, instead of a burst of separate submissions.
3. `--urgent` delivers its own message first (jump the queue) and leaves the backlog to the normal paced path.
4. Make sure the watcher still reports a queue that is not draining (age of the oldest item).
