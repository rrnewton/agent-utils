---
title: 'vibe-talk-glossary-popup: a Glossary button in the WTF and defined-term popups opens a full-screen searchable alphabetical glossary'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-25T08:50:00-05:00
updated_at: 2026-11-25T12:48:57-05:00
closed_at: 2026-11-25T17:48:57.000000000+00:00
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

## Resolution

Landed on main as f631fcdd in the stacked range e637f97c (with #2230 and #2242, one owner gate on the stack top, green 2026-11-25 12:22 PM ET), deployed 12:33 PM ET 11-25, and the 15-minute deploy watch came back clean (12:33 to 12:48 PM ET: no errors, no restarts). The glossary popover carries one Glossary button in its top corner beside the X, the same for a WTF? answer and a tapped defined term, and adds no row. It opens a full-screen glossary drawn by Discuss's own full-screen and head rules under classes of its own: alphabetical with letter headings, a search that filters term then definition matches, Reviewed and Learned badges, empty, no-match and failed-with-Retry states, opened at the known term it came from. It reads the existing read-only terms route (no server change), renders text only (message sources link only as checked in-app addresses and jump through the page's jump path; document sources open in Documents), and is modal: everything else inert and dimmed, the key dispatch standing down, Tab trapped, Escape, the X and the phone's Back closing it alone, focus returning, an open Discuss left open. Side fix: the popover's document source now switches to Documents. Evidence: page tests (both popups at phone and desktop, alphabetical order, search, empty and failed states, markup and a javascript: source rendered inert, inert plus keys plus Tab, Back with Discuss beneath, message jump and document open, two negative controls), a real-router test of the terms request, and screenshot scenes 107-117 (light and dark, phone and desktop, keyboard open and closed, a logged end-to-end walk), re-taken from the landed tree: scene 111 pixel-identical to the stack-top shots in all four, scene 115 identical apart from sub-visible border antialiasing and the dimmed bar under the popup. Side findings filed as #2244 vibe-talk-discuss-hours-field-scale; the scene 77 order dependency is the coordinator's #2243. Part C stays open as #2234 vibe-talk-glossary-term-index.
