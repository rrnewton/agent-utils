---
title: 'discuss-collapsible-sections: fold the summary by workstream, all collapsed'
status: open
priority: 1
issue_type: feature
assignee: mu-discussfoot
labels:
- vibe-talk
created_at: 2026-11-12T19:00:00+00:00
updated_at: 2026-11-12T19:00:00+00:00
---

# Description

[watermelon-full-preview] Owner reply 007 (via coordb): "I'd also like collapsable sections inside the summary document that comes out and for them to all start collapsed. There can be an expand/collapse all button."

# Design

- ONE shared helper, `vibe-talk/web/src/view/collapse.ts` (`foldSectionsByHeading` + `setAllSections`): fold by heading, all collapsed by default, designed to fit the document reader (`#1237 documents-collapsible-sections`, still open) as well.
- The Discuss answer uses it: each heading section folds, all collapsed, with an Expand all / Collapse all bar at the top of the answer body (placement agreed with mu-discussui).
- Wiring the reader (`#1237`) is a follow-up; the helper is named there.

# Acceptance Criteria

- A rendered Discuss answer with headings opens with every section collapsed; Expand all opens them all and Collapse all closes them all.
- Page tests pin the helper; walks pass at phone and desktop; gated at owner tier in the evening batch.

# Notes

Owner reply 007. Depends on #1349 discuss-markdown-render (landed). Helper shared with #1237.
