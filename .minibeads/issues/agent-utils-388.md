---
title: 'thread-header-collapse: a tap folds the card at the top of a thread to a thin coloured bar, and a tap opens it again'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-20T04:42:09.361209688+00:00
updated_at: 2026-10-20T05:02:00.380608086+00:00
closed_at: 2026-10-20T05:02:00.380607976+00:00
---

# Description

[opus 5.5] The owner, 2026-10-19, of a phone in the thread view: "On a small mobile screen, I don't like wasting vertical space on the thread header at the top. I like the consistent visual language for the colored thread cards. But I don't always want to use the space. The same way tapping a message expands and contracts it I want tapping that header to collapse it to basically a thin colored bar. Just enough vertical height that you can see the inside color as well as the border color and so it's wide enough to tap on." The card heading a thread (#298 thread-card-header) holds the title, its age, replies and unread and the X, and takes about 220 px of a phone's height.

# Acceptance Criteria

A tap or click on the card at the top of a thread, anywhere but its X (or a link in it), folds it to a thin bar in the card's own fill and border colours: 12-16 px of visible colour, with a press target at least 24 px tall. A tap on the bar opens the card again. The choice is one per device, for every thread, kept across a reload like the neighbouring settings. The card is a button to the keyboard and assistive technology, saying aria-expanded. The thread's reader stays where they were as the card folds or opens: on the newest line if they were on it, otherwise with the same message at the same height. The same at a desk in the right column, where the line from the thread's root still meets the card or the bar at its left edge, in its gutter, never crossing a message box (#298, #385 thread-line-overlap). Folding the card replaces #298's press on clamped text opening the card over the list. Page tests with negative controls (a press on the X or a link does not fold it; the folded state survives a reload) and Chromium screenshots at 393x852 and 1280x800, folded and open, dark and light.
