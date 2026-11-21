---
title: 'vibe-talk-read-pause-reply-hold-clock: the held-read snapshot compares a free-running audio clock exactly'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- voice
- browser
created_at: 2026-11-20T18:00:00-05:00
updated_at: 2026-11-20T22:00:00-05:00
---

# Description

[watermelon-full-preview] `vibe-talk/tests/read_pause_reply_browser.py`'s held snapshot for the agent voice was `{count, clock}` with `clock = readAloudContext.currentTime`, and `check_held_still` demanded exact equality after a settle. Gate sample: count stayed "2 of 3" (the read did not move) while the clock advanced 3.262 -> 3.712 (+0.45s) and the walk failed with "the read moved while its composer held it". `AudioContext.currentTime` is a free-running wall clock: it advances whenever the context runs, held read or not, and context suspension lands async, so exact equality across a settle window whose wall duration grows under load is a wall-clock assertion. Verdict reported before fixing: a test problem (owner rule 346), not a page bug.

# Design

- Judge the hold by the read's own events: strip count unchanged, lit sentence unchanged, and steer-step count unchanged (a chunk started during the hold moves one). The device-voice branch already judges by started-utterance count alone and is unchanged.
- Drop the exact `AudioContext.currentTime` equality everywhere it judged a hold (the composer-pause snapshot and the earlier pause walk's held check, which keeps its count and steer-step checks). The resume paths still wait for the clock to move, by condition, as before.

# Acceptance Criteria

- `read-pause-reply-browser` passes repeatedly locally and in gate on a loaded host, with no exact audio-clock equality judging a hold.
- A read that genuinely advances during a hold still fails (count, lit sentence, or steer steps move).

# Notes

[watermelon-full-preview] mu-discussfix. Number 2020 pre-assigned by coordb. Sibling: #2021 vibe-talk-scroll-stability-arrival-offer.

## Resolution

[watermelon-full-preview] Landed on github/main as 5d37a560: clean full gate pass on 0f5363dd (stack replayed to 5d37a560).
