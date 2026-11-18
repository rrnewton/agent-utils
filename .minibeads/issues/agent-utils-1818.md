---
title: 'vibe-talk-thread-drill-in-keeps-anchor: thread drill-in keeps the clicked message at the same viewport offset'
status: open
priority: 0
issue_type: feature
assignee: mu-drillanchor
labels:
- vibe-talk
- ui
created_at: 2026-11-17T20:01:38-05:00
updated_at: 2026-11-17T20:01:38-05:00
---

# Description

[watermelon-full-preview] Owner request, fast-tracked (owner thread 158, part, 2026-11-17), verbatim: "when I have the message X on screen and I click to the thread containing it, I want X to still be on screen. IDEALLY we would be able to place X at the SAME place on the screen so that what is above/below changes but not it."

Today drilling into a thread from its Thread(n) button lands on the thread's first unread message or its newest line (`payThreadEntry`, `#221 read-modes`), so the message the reader clicked moves or vanishes. The reader loses their place in the channel exactly when they follow it into its thread.

Scope is anchoring only. The Thread(n) button label is #1819 vibe-talk-thread-button-label (not this issue).

# Design

When the reader drills into a thread from a message X in the channel (the Thread(n) button on X's row), the page measures X's top relative to the scroll container before navigating, and restores it after the thread renders: the thread view opens with X at the same viewport offset it had before the click, so what is above and below X changes but X does not move. If X is the root, the root is anchored. The anchor is kept through late layout such as images and summaries with a ResizeObserver (never a timer), until the reader scrolls or leaves the thread. Back navigation keeps doing what it does today.

Entry points that carry no channel message (the thread picker, thread-list cards, the unread-replies chip that promises the first unread reply) keep their current landings; only a drill-in from a message row carries the anchor.

# Acceptance Criteria

- Drilling into a thread from message X's Thread(n) button opens the thread with X's top within a few px of its offset before the click, at phone (412x915) and desktop (1280x800) sizes.
- The anchor holds through late layout (a late-loading image above X, a summary settling above X): X stays within a few px, corrected by a ResizeObserver or a condition wait, with no timer.
- If X is the root, the root is anchored the same way.
- Back from the thread behaves exactly as before this change.
- Page tests at phone and desktop assert the offsets; the screenshot matrix (light and dark, keyboard open and closed, phone and desktop, Discuss half-pane open in the phone shots, plus before/after of the drill-in) is captured on a test instance and reviewed before announcement.

# Notes

Owner-priority gate: deployed on landing. Lane mu-drillanchor on branch mu-drillanchor/1818.
