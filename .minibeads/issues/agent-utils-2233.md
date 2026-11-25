---
title: 'vibe-talk-glossary-popup: a Glossary button in the WTF and defined-term popups opens a full-screen searchable alphabetical glossary'
status: in_progress
priority: 1
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-25T08:50:00-05:00
updated_at: 2026-11-25T08:50:00-05:00
---

# Description

Owner request (2026-11-25, part B of a three-part ask): "when I pop open the wtf window I want a button to jump to a separate glossary page or full screen popup like discuss. Yeah the latter for now. When I click an existing defined term I want the pop up definition to share the same link to go to glossary. Searchable alphabetical by default."

Today the WTF? press (#2156 vibe-talk-wtf-glossary-lookup) and a tap on an already-linked glossary term both open the same small glossary popover with one entry. There is no way to see the whole glossary.

# Design

- One "Glossary" button in the shared glossary popover, so the WTF answer and a defined-term popup carry the same button and link. It is a real button, reachable by keyboard, shown in every popover state.
- The button opens a full-screen glossary popup that uses the Discuss panel's full-screen geometry and head (the full sheet on a phone, the maximized panel on a desk) and stands above an open Discuss without disturbing it.
- Data: the existing read-only `GET /api/v1/glossary/terms` (reviewed warm-up entries plus learned entries; no model call, no upstream call). No server change.
- Each entry shows the term, a Reviewed or Learned badge from the entry's origin, the definition, and its source link when there is one.
- Alphabetical by default with letter headers; one search field filters term and definition as the reader types (term matches first). Opened from a known term, the popup scrolls to that entry and highlights it.
- Escape, the X and the phone Back button close it; focus moves to the search field on open and returns to what opened the popover on close.
- Empty state, no-match state and a load failure with Retry.
- Designed so #2234 vibe-talk-glossary-term-index can add label rows and a per-term mentions action on the same rows later; this issue does not build that.

# Acceptance Criteria

- Page tests at phone and desktop sizes: the popup opens from the WTF popover and from a defined-term popup; entries are alphabetical; search filters by term and definition; empty and no-match states; Escape and the X close it and focus returns.
- A real-server test serves the page's exact terms request through the router.
- Screenshot matrix (light and dark, keyboard open and closed, phone and desktop, Discuss open on phone) plus the glossary popup open with a search typed, and the WTF popover and a defined-term popup showing the button. Fixture glossary content is synthetic.
- Exercised dynamically on a local test server before landing.

# Notes

Owner request part A (logging WTF calls to iterate on the engine) and part C (#2234) are separate.
