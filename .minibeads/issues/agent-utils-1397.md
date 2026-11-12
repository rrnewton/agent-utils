---
title: 'discuss-dock-close: closing a docked Discuss panel frees its column'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-12T20:10:00+00:00
updated_at: 2026-11-12T20:10:00+00:00
---

# Description

Docking the Discuss panel and then closing it still reserved the right column: the main interface stayed squeezed to the left beside a blank column. Switching to overlay and then closing restored the normal view. The screen mark that drives the docked grid followed the remembered layout mode alone, not whether the panel was open, and closing never cleared it.

# Acceptance Criteria

- Dock, then close by any exit (the panel close act, the X icon, Escape, the bar toggle): `#screen-main` carries no `data-discuss` attribute and the layout returns to one column.
- Reopening restores the remembered mode: a panel left docked docks again, and maximized likewise.
- Overlay is unaffected: it never marks the screen, open or closed.
- A page unit test covers dock/close/reopen and the other exits; a browser walk at 1280x800 covers the freed column if the walk makes that quick.

# Design

`data-discuss` on `#screen-main` reflects mode AND open. One helper syncs the screen mark from the remembered mode plus current openness: it is re-applied from `applyMode()` and from `setOpen()`, the single place every exit lands (close, X, Escape, and the bar toggle all hide through it). The remembered mode choice itself is untouched, so reopening re-applies it via the stored-layout path. No stylesheet change: the docked two-column grid already keys only off the attribute's presence.

# Notes

Owner report via the coordinator, fast-track for the Discuss batch. No Mod+8 Discuss chord exists in the page today, so there is no such exit to cover. View change does not close the panel through a separate path; it goes through the same close act.

## Resolution

Landed on main as e5fc567c in the 4:30 PM ET Discuss batch and deployed (live about 5:53 PM ET, as reported by the coordinator). Closing a docked or maximized Discuss panel now clears the screen mark so no column stays reserved, while the remembered mode is kept and re-applied on reopening; overlay never marks the screen. Covered by the page unit test for dock/close/reopen across every exit; the browser-walk step was added for the gate walk.
