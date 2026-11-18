---
title: 'vibe-talk-discuss-off-in-documents: disable Discuss while the Documents view is active'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-17T19:57:00-05:00
updated_at: 2026-11-18T03:10:47-05:00
---

# Description

[watermelon-full-preview] Discuss is shown in the Documents view, where it cannot work: its scope is the channel or thread on screen, and Documents has neither. Disable Discuss while Documents is active — disable its control with a short tooltip saying why, close any open panel on switching to Documents, and refuse every other entry point (menu, Mod+8, history restore) there; switching back to the channel restores the control and its transcript with no new model call. Scope is the Documents gating only: #1811 owns the Discuss title and scoping, #1821 owns persistence.

# Design

Implemented now (gating only): the Discuss view learns the major view; in Documents `available()` is false, the menu's Discuss stands `aria-disabled` with the tooltip "Discuss is not available in Documents" (the bar keeps it hidden — its strip is priced to the edge), clicks/keys/restores do nothing and say why in place, and `sync()` closes an open panel (its snapshot is kept, so returning and reopening restores it).

Sketch only — Discuss over documents, NOT implemented (about 20 lines):
- A Discuss session needs an explicit set under discussion, not "Documents".
- Entry: from the Documents list, select documents (checkboxes or "Discuss
  these") or, in the reader, "Discuss this document"; the set is named in
  the panel header as its scope, e.g. "3 documents".
- Only fetched documents can be discussed: a document whose body has not
  been fetched is listed in the set as unavailable, never silently dropped.
- The reader offers "Fetch" for an unavailable member; fetching joins it to
  the set, and the panel notes the set changed (no silent re-summarize).
- Summarize sends each fetched document's text with its title as citation
  anchor; citations jump to the document and section, as message citations
  jump to their message today.
- The bound is explicit like the message bound: the selected set, capped,
  with "N of M fetched" shown before Summarize is pressed.
- An empty or all-unfetched set disables Summarize and says why in place.
- Persistence, titles and cross-session scope belong to #1821/#1811.

# Acceptance Criteria

- In Documents: the menu's Discuss is disabled with the tooltip above, an open Discuss panel is closed, and bar/menu/Mod+8/history-restore cannot open it; no catch-up request is made.
- Back on the channel view the control is enabled again and reopening Discuss restores the kept transcript without a model call.
- Focused page tests cover the gating at phone and desktop sizes; the design sketch above stays unimplemented.

# Notes

## Resolution

Landed 190f8e277679be734949b6f2797317e5d2ba9451 on github/main (gated 19f1cad0, overlap replay verified tree adfdfc94). In Documents: sync() closes an open Discuss panel (snapshot kept; reopening on the channel restores it with no model call), the menu's Discuss stands aria-disabled with the tooltip "Discuss is not available in Documents", and bar/menu/Mod+8/history-restore/restore-from-persistence cannot open it there; switching back restores the control. Tests: new catchup unit test (fails unpatched) plus Documents menu assertions in the page shell suite; focused suites, typecheck, discuss_browser and discuss-dock walks green; gate PASS with screenshots-desks flake rerun passed. Screenshot matrix (16, test instance, inspected): /tmp/mu-discussdocs-shots-1812/. The #1821 persistence interaction was verified in a throwaway browser script (persisted-open Discuss closes in Documents, restores on the channel, stays closed across a Documents reload) — recording that as a maintained browser assertion is a follow-up. The Design sketch above remains unimplemented, as asked.
