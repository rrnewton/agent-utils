---
title: 'vibe-talk-summary-bullet-restyle-everywhere: Done or archive on a cited message restyles every bullet that cites it, Earlier summaries included'
status: closed
priority: 0
issue_type: bug
assignee: cl-bulletstrike
labels:
- vibe-talk
created_at: 2026-11-25T22:40:00.000000000+00:00
updated_at: 2026-11-26T01:30:00.000000000+00:00
closed_at: 2026-11-26T01:30:00.000000000+00:00
---

# Description

Owner request (2026-11-25, part C of a three-part Discuss request; the pane split and the All view's older-message loading are separate issues), verbatim: "when I clicked the following message after reload it DID go back far enough for that message: [a citation link to an older message in the All view]. But when I archived it I didn't get strikethrough on the bullet on the OLD 'Earlier Summary'. If it's not too much overhead I would like it to apply there. I.e. maybe any message linked to a summary bullet pushes an event which makes a style change to that bullet even if its in earlier summary."

Today the live strike (#1388 discuss-summary-staleness-update, #1820 vibe-talk-summary-strikethrough-live) restates only the current summary: the panel's `restrike` walks every bullet and section under the drawn answer on each mark. The Earlier-summary bodies the Discuss timeline draws are never struck, so archiving or marking Done a message one of them cites leaves that bullet plain, even while the same message's bullet in the current summary strikes through. Each restate also rescans every bullet of the current answer, whatever message changed.

# Design

Event-driven and keyed by message id. A per-panel strike index maps each cited message id to the bullet and section elements that cite it (each entry keeps its own cited ids and its group). Drawing the current answer and drawing the timeline each replace their group's entries from the freshly drawn tree, so a redraw, a restored record or an Update never leaves stale or detached elements indexed, and strike just that group.

A state change that knows its ids (Done, archive, put back, a refused write's take-back, the Discuss panel's own mark buttons) passes them through `onReadStateChanged` and the page's strike refresh; the panel does one index lookup per id and recomputes only the entries found. Changes without an id list (server read state landing, view sync, the settle wrappers) diff the index's own id keys against a cached read bit and touch only entries whose cited id flipped; no DOM walk per event. The current summary's fresh (green) marking keeps its present behaviour.

Deleted upstream counts as done (rule set on the owner side on 2026-11-25; the owner may still object): a cited message the page learns was deleted upstream (a live delete or a refresh's deletions, the page's existing removal record) counts as handled for the strike, so a bullet whose other cited messages are read stays struck, and every citation link naming it is marked deleted with a muted "(deleted)" after the link (a class word and a stylesheet suffix, so the bullet text, search and read-aloud are unchanged). A later read that carries the message after all clears the mark. The removal record notifies the strike refresh with the removed ids, so a delete is one lookup per id like any other change. The page knows only the deletions it was told of; a deletion it never heard of (for example one that happened before a reload and is no longer in its saved removals) is not marked.

# Acceptance Criteria

- Archiving or marking Done a message cited by a bullet in the current summary AND by a bullet in an Earlier summary strikes both bullets at once, with no refresh; putting it back clears both.
- A bullet is struck only while all its cited messages are read; an uncited bullet is never struck.
- Redrawing the current summary or the timeline leaves no stale index entries; a strike update after a redraw reaches the new elements.
- A state change looks up only the bullets citing the changed ids; no scan of all bullets per event.
- A cited message deleted upstream counts as done: its bullet stays (or becomes) struck and its citation link shows the deleted mark, in both the current and an Earlier summary; a page test proves it.
- A page test that fails on main proves the Earlier-summary strike and its clear, at phone and desktop sizes, through the real server router.
- Screenshot matrix on a test instance (light and dark, keyboard open and closed, phone 412x915 and desktop 1280x800, Discuss open) plus the struck Earlier-summary bullet before and after.

# Notes

Owner fast-track (owner-priority gate, deployed on landing). Siblings from the same owner request: the Discuss pane split with a collapsible timeline, and the All view's stale timeline and failing Loading older.

## Resolution

[opus 5.5] Landed on github/main as `6e143fa46175863fb8f7a0d106898a29aef49b1a` (gated head `1abe46ab`, owner-priority gate pass, full selection; same patch as the reviewed `4d7d1c75`, rebased only). A per-panel strike index (`view/catchup/strikeindex.ts`) maps each cited message id to the bullets, sections and citation links naming it, in the current summary and every Earlier summary (never a follow-up turn); each draw replaces its own group, and an element found detached is dropped when an event reaches it. Done, put back and refused take-backs pass their ids through `onReadStateChanged`, so a change restyles with one lookup per id; changes without ids diff a cached per-id state. A message deleted upstream counts as done: the removal record tells a watcher the ids it removes or takes back, and the citing links wear `discuss-cite-deleted`, which the stylesheet suffixes with a muted "(deleted)". Tests: unit tests for the index, the panel (current and Earlier bullet struck on a Done, cleared on put back, struck and marked when deleted; fails on the old page with "the earlier summary's bullet did not strike") and the removal watcher; screenshot scenes `130-discuss-strike-earlier` (real catchup, record and dismiss routes; the model stub cites window lines only while a scene turns citing on) and `131-discuss-strike-deleted` (threaded client configuration, real timeline reads with the deleted message taken out, a live `message_delete`), reviewed with the standard Discuss matrix in light and dark at 412x915 and 1280x800. Known limit: the page marks only deletions it was told of in this session.
