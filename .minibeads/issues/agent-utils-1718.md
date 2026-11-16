---
title: 'vibe-talk-fold-run-buttons-margins: First 5 at the left margin, Last 5 at the right margin of a folded-run line'
status: open
priority: 0
issue_type: task
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-16T14:57:54+00:00
updated_at: 2026-11-16T14:57:54+00:00
---

# Description

[watermelon-full-preview] Owner, :thread:118 at 9:50 AM ET: "Ugh the "First 5 / Last 5" unfolding buttons are next to each other on the RIGHT end of the line. The first 5 was supposed to be on the left margin, last 5 on the right margin."

Owner addendum, 9:51 AM ET: "But do capitilize in "First 5"... and "Last 5"". The labels become "First 5" and "Last 5", capitalized, without the parentheses.

Context: fe9a8aa3 (Nov 13, owner 040 fast-track) put both "(first 5)" and "(last 5)" at the far end of the folded-run line (`readrunlines.ts`, `.read-run-peels` pinned `right: 0.25rem` in `05-inbox-view.css`). Each peel reveals five messages from its edge while the line stays with its count reduced.

# Design

On a folded-run line, "First 5" sits at the LEFT margin and "Last 5" at the RIGHT margin, with the run summary between them, everywhere the line is drawn (channel lists, threads unpacked under cards, Thread-list card runs where the code is shared). At phone widths neither button overlaps the summary text; the summary truncates or wraps gracefully; each button keeps a usable touch target. No other changes.

# Acceptance Criteria

- A page test at phone (about 412x915, scale 2.6) and desktop (1280x800): the First 5 button's left edge sits at the line's left content edge and the Last 5 button's right edge at its right content edge (within a few px), with no overlap between them and the summary; each button still unfolds what it says. Condition-based waits only.
- Acceptance screenshots at phone and desktop on a local test instance (fixture data or a fake API; never the live instance or its database), showing a folded run with both buttons.
