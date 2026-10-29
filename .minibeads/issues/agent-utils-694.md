---
title: 'muse-bridge-watch-queue: turn-queued misreads Muse''s input queue inside a goal'
status: open
priority: 2
issue_type: bug
created_at: 2026-10-29T15:54:23.508747723+00:00
updated_at: 2026-10-29T15:54:23.508747723+00:00
---

# Description

[opus 5.5] From #679 muse-bridge-study. watch-agents reported a Muse goal subject as "inside one
turn for 2 h 38 m; 1 input queued in Muse's queued input, submitted when the turn ends"
(turn-queued). For Muse that is wrong twice: Muse takes a `◆ Queued input` item at its next
step (13-21 s later in the study), not when the turn ends, and inside a `/goal` the "turn" the
watcher times is the whole goal. With the owner's auto-interrupt policy on (60 min, 2 checks),
the rule could interrupt a Muse goal to deliver input Muse was about to take anyway.

Wanted: for Muse, time how long a queued item has stayed queued (it is visible on screen) and
report turn-queued only when that passes the threshold, not the turn's age.
