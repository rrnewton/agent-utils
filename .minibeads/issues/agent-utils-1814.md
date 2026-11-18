---
title: 'vibe-talk-thread-card-title-and-summary-due: a quoted root titles its card from the quote and a 3-message thread goes unsummarised'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-17T20:01:53-05:00
updated_at: 2026-11-17T23:53:32-05:00
closed_at: 2026-11-17T23:53:32-05:00
---

# Description

[watermelon-full-preview] The owner (2026-11-17): a thread whose root opens with a quote and then one line meant as its title ("PROVENANCE THREAD 2026-11-17 TRY2") shows a thread card titled from the first part of the quote ("Not Started: ..."). After a third message was added to that thread it still had no summary. Questions: is the minimum for a summary based on words or message count, why did the third message not trigger one, and should a card that falls back to the root's first message be allowed to show more of it?

# Design

- Reproduce with a fixture of the same shape (a root that opens with a quote block, then a title line, then two replies) and fix the real cause the reproduction exposes, not a guessed one.
- Card title from the root: skip a leading quote block (lines starting with ">") when choosing the card's title text, in the shared server title helper and every caller that duplicates the first-line choice.
- Report, do not build, options for a larger card maximum or a whole-message card option for the owner to choose from.

# Acceptance Criteria

- A fixture thread with a quoted root plus title line and two replies is summarised at three messages, with a test that fails before the fix and passes after.
- A root of a quote block plus "PROVENANCE THREAD 2026-11-17 TRY2" titles its card with that line, not the quote; a root that is only a quote still titles from its text.
- The threshold rule is reported in plain words with file:line for the owner.
- Screenshot matrix on a test instance: light and dark, keyboard open and closed, phone 412x915 and desktop 1280x800, Discuss half-pane open in at least the phone shots, plus before/after of the fixed state; every image looked at.

# Notes

Owner-priority, deployed on landing. Threshold in code today (to verify and quote): message count, not words; see the report on this issue for the confirmed file:line.

## Resolution

Landed in bc73237a. The cache now counts held replies past an outgrown exact root count (root and thread table, marked not exact), so a three-message thread is queued and summarised; card titles skip a leading quote block in the server helper, the Slack title, and the page fallback. The larger-card / whole-message options were reported to the owner, not built, as this issue's Design required.
