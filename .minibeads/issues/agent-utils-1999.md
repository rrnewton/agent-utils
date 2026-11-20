---
title: 'vibe-talk-documents-download-jump: pressing download on a Documents card jumps the list'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- documents
- ui
created_at: 2026-11-20T16:46:00-05:00
updated_at: 2026-11-20T16:46:00-05:00
---

# Description

[watermelon-full-preview] Owner report (2026-11-20, about 4:40 PM ET, phone, Documents list): "I'm not sure but I think I was just seeing some chaotic scrolling there as well where I would press the download on one document and then my screen jumped". Context from the liaison: the list was on the All filter, with pastes and two starred markdown cards whose fetches had errored. Their fetches then succeeded (two 200s). Suspects, to be tested not assumed: the list re-sorts or re-renders when a card's fetch state changes; the error toast/status line or a card's meta line changes height above the viewport; focus moves.

# Design

- Reproduce on a fixture at phone 412x915 (also 393, and desktop 1280): a list long enough to scroll, tap download on a card mid-list (one that errors first, then one that succeeds, and a retry from error to success), and record each card's rect.top plus scrollTop before and after, every frame for about 1.5 s. Name which writer or reflow moves it.
- Fix by holding the tapped card in place: anchor it across the state change. Do not re-sort the list on a fetch-state change; if order must change, do it without moving the reader.
- A browser test asserting the tapped card moves at most 3px, for error-to-success, success and the retry, at phone and desktop.
- Screenshots per the screenshot matrix (light and dark, phone 412 and desktop 1280) of before and after the tap, with BUILD.txt.

# Acceptance Criteria

- The tapped card moves at most 3px across a download tap for error, success and error-to-success retry, at phone and desktop sizes.
- No re-sort of the list on a fetch-state change moves the reader; any order change lands without moving the reader.
- The moving writer or reflow is named from a per-frame trace of card rect.top and scrollTop.
- Existing documents suites stay green; screenshots with BUILD.txt for review.

# Notes

Owner-priority, fast-track. Browser verification runs on the host with browser libraries; this lane hands the exact commands to the coordinator.
