---
title: 'vibe-talk-discuss-truncated-window-wording: say what a cut-short summary covers and what to do'
status: closed
priority: 2
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-18T18:56:49-05:00
updated_at: 2026-11-18T18:56:49-05:00
---

# Description

[watermelon-full-preview] From the liaison's review of the #1843 shots: after a Summarize whose window was cut short, the status reads "Read N unread of M. Older messages are not covered: fetching them did not finish." It shows whenever the server's answer window is truncated — a long thread beyond the fill walk's bound, a fetch that did not finish, or a capped channel window — so the owner sees it on the live page, and it says what happened but not what the summary covers or what to do next.

# Design

- The cut-short status says it covers the newest part only and that summarizing again later includes more: "Read N unread of M. Older messages weren't fetched, so this covers the newest part only — summarize again later to include more of them." (For a thread each Summarize fetches another page toward the start; for a channel the cache fills over time.)
- Wording only: the truncated condition, the counts and every other status stay as they are.

# Acceptance Criteria

- A truncated answer's status reads as above; an untruncated one still reads "Read N unread of M".
- A page test covers the truncated wording.

# Notes

Tiny follow-up to #1843, from the liaison's screenshot review.

## Resolution

[watermelon-full-preview] The cut-short status now reads "Read N unread of M. Older messages weren't fetched, so this covers the newest part only — summarize again later to include more of them." A page test pins the wording; the real-route walk's shots show it live (desktop-dark-main-02).
