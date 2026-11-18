---
title: 'vibe-talk-discuss-uncovered-messages-line: show and clean up messages no summary in this scope covered'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-18T10:29:40-05:00
updated_at: 2026-11-18T10:29:40-05:00
---

# Description

[watermelon-full-preview] Owner: "If we've done prior summaries on a thread but they are NOT contiguous ... I think there should be a little line at the bottom: [Summarize] N messages not included since MM/DD. Where MM/DD is the time of the FIRST summary in the timeline. I.e. once we start summarizing this would tell us if we have missed anything. Pressing summarize here would do a targeted cleanup of everything else that was missed. So the default is to get contiguous chunks of time. But if you mess it up and have a gap, this helps you clean up."

Builds on #1842's per-scope covered ranges, and (per coordb, owner thread 185 dynamic-testing rule) on a real-route browser walk: a test-only HTTP inject endpoint on the fake upstream seeds non-owner messages, and the walk drives real Summarize presses through the real router with only the model stubbed.

# Design

- Per scope, once at least one summary exists, the foot of the Discuss pane shows a line with a Summarize button: "N messages not included since MM/DD", where N is the scope's unread messages at or after the first covered range's start that no covered range includes, and MM/DD is that first summary's date. Hidden when N is 0 or no summary exists in the scope.
- Pressing it summarizes exactly those uncovered messages: the catch-up request gains an explicit message-id set (the uncovered ids), recorded as covered on success, so the line then reads none and hides.
- A test-only fake-seed endpoint (write scope, only where the fake chat backend is in use) posts a non-owner message with caller-chosen author, text and optional timestamp, so browser walks can seed unread messages on a test instance. The committed real-route walk uses it: seed, Summarize, seed more, Summarize (second bound from the first end), make a gap, see the line, press it, N = 0.

# Acceptance Criteria

- A gap shows the line with the right N and the first summary's MM/DD; pressing it summarizes exactly the missed messages and the line disappears (N = 0).
- Thread and main scopes each keep their own line; a scope with no summary, or no gap, shows none.
- The fake-seed endpoint refuses where the fake backend is not in use.
- Committed real-route browser walk and page tests at phone and desktop sizes; screenshot matrix (gap -> line -> press -> gone; thread and main; phone and desk; light and dark) inspected.

# Notes

Lands after #1842 (per-scope covered ranges). The model stub for the real-route walk is a loopback Messages-API stub named in the test's own config; no live instance or database is touched.

## Resolution

[watermelon-full-preview] Fixed on this branch: the Discuss pane's foot now shows, per scope, a Summarize line "N messages not included since MM/DD" (the first covered range's date) counting the scope's unread messages at or after the first summary that no covered range includes, hidden when there is no summary or nothing missed. Pressing it posts the exact uncovered message-id set (new `message_ids` on the catch-up request, filtered exactly server-side) and records them covered, so the line goes. A test-only `POST /channels/{id}/fake-seed` (write scope, 404 unless the fake backend is served) seeds non-owner messages with chosen author, text and timestamp, including thread messages; the fake serves its timelines. The committed real-route walk (`discuss_uncovered_browser`, one gate step) boots the real server with only the model stubbed and drives seed -> Summarize -> seed -> Summarize (second bound from the first end) -> gap -> line -> cleanup on main and in a thread. Tests: new panel unit test (gap shows the line with the right N and date, press posts exactly the missed ids, line hides; fails on the unpatched base), fake timeline unit tests, contract samples; catchup/settings/contract suites, all typechecks and the real-route walk pass. Screenshot matrix on the real-route test instance: /tmp/mu-discussscope2-shots-1843/ (52 images: phone 412x915 and desktop 1280x800, light and dark, keyboard open and closed, the gap -> line -> press -> gone sequence on main and in a thread), all inspected.
