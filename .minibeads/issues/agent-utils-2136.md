---
title: 'vibe-talk-discuss-summary-search: search button on the Discuss bar filtering all summary bullets'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-22T17:45:00-05:00
updated_at: 2026-11-22T17:45:00-05:00
---

# Description

Owner request: a Search button on the Discuss bar. The icon row at the top right has room for one more square icon. Typing filters ALL bullets of the current summary and of every earlier summary, giving a quick list of every summary row that matches, with just enough headings to know where each hit is.

# Design

- A square search icon in the Discuss bar icon row, matching the others.
- Live filtering as the owner types; case-insensitive; several words must all match (the same rule as the unified read CLI).
- Results are a flat list of hits; each hit is labelled with its place (current summary, or which earlier summary) and its topic heading, nothing more.
- Each hit keeps its message links, and a click jumps in place exactly like summary links (stay in the All view, no thread drill-in).
- Data: use the earlier summaries already loaded in the page if they are complete there; otherwise add a server route that reads only the local summary store (no upstream calls).

# Acceptance Criteria

- End-to-end test through the real server router with the page exact request.
- Screenshots: phone and desktop, light and dark, keyboard open and closed, Discuss open, with a query showing hits from the current and an earlier summary.
- No upstream (chat service) call is made by a search.

# Notes

Owner UI request (fast-track); filed by the coordinator.
