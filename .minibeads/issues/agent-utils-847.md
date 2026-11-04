---
title: 'read-aloud-table-rows: read tables row by row with header labels, highlight in step'
status: open
priority: 1
issue_type: bug
assignee: cl-refactor
created_at: 2026-11-04T19:00:42.856066401+00:00
updated_at: 2026-11-04T19:00:42.856066401+00:00
---

# Description

The owner, on read-aloud of a table: "after reading the upper left cell, it jumped down to the cell directly below it ... skipped ... highlights proceeded faster than they should. I expected it to read a row at a time from left to right".

# Design

- Read-aloud walks a table row by row, left to right. Each data cell is read with its column header as the label (for example "Status: open"); the header row itself is not read as data.
- The highlight moves cell by cell in step with the speech: never ahead of it, and no skipped cells.
- This shares table parsing with #663 vibe-talk-pipe-tables, so a Markdown pipe table rendered in a message reads the same way.

# Acceptance Criteria

- Page tests with a 3x3 table (header + 2 rows): the spoken order and labels are as above, and the highlight sequence matches the utterance boundaries.
- Gate passes.

# Notes

Owner ask lost while the coordinator was down (2026-11-03/04), relayed by the lead 2026-11-04. Related: #663 vibe-talk-pipe-tables. Before more #624 batches.
