---
title: 'docked-cancel-putdown: after a docked reply is cancelled, a later tap-outside put-down leaves its inline box below the visible area'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-11T17:43:25+00:00
updated_at: 2026-11-11T17:43:25+00:00
---

# Description

[opus 5.5] Seen while testing #1315 floating-reply-dock, in its quick desktop walk (1280x800): a floating reply was docked into the inline reply editor, then discarded with Cancel. A later reply put down by tapping outside the box left its inline box below the visible area of the list. When the docked reply was sent instead of cancelled, the later put-down showed its box as expected.

The put-down and discard paths existed before #1315 and were not changed by it, so this is probably older state (an anchor, a held row or a pending scroll target) left behind by the Cancel path, which the next put-down then reuses.

# Design

- Reproduce it in a browser walk: dock a floating reply, Cancel it, open another reply, tap outside to put it down, then check that its inline box is in view.
- Find what Cancel leaves behind (reply session state, held rows, the scroll owner's pending target) and clear it on Cancel. Any scroll goes through the single scroll owner (#1251); no direct scrollTop writes.

# Acceptance Criteria

- After dock then Cancel, a later tap-outside put-down shows its inline box in view, at desktop 1280x800 and on a phone.
- The walk is added to an existing classified browser test file, with no wall-clock waits (owner rule 346).

# Notes

Related: #1315 floating-reply-dock, #1251 scroll-single-owner. Numbered from devbig030's interim block (1400-1469).
