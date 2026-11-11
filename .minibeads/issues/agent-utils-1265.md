---
title: 'documents-star-hide: star and hide documents, synced per document'
status: open
priority: 1
issue_type: task
assignee: mu-docstate
created_at: 2026-11-11T04:15:00+00:00
updated_at: 2026-11-11T04:15:00+00:00
---

# Description

[watermelon-full-preview] Owner replies 354 and 358 (2026-11-10 ET), two
asks that share per-document state in the vibe-talk Documents view:

(A) Reply 354: "Also I want a little star icon to click to star
documents." A small star toggle on each document card and in the reader
header. The state is per document and synced across devices, so it is
the same on phone and desktop. A way to see starred ones: reuse the
existing filter chips (a Starred chip) or sort starred first; pick the
one that reuses existing UI.

(B) Reply 358: "Let's also swipe on documents to hide them from the
list. Hamburger icon can have a show hidden option." Swipe a document
card (reuse the message rows' existing swipe/Done gesture and its Undo
chip) to hide it. Hidden is per document and synced like the star. A
"Show hidden" toggle in the hamburger menu while in the Documents view
shows hidden cards dimmed, with an unhide action. Desktop equivalent:
a hide button on hover or in a card menu.

Prior work: #629 documents-view-client, #1022, #1047, #1079
documents-chronological, #1081 documents-relative-links, #628
documents-index-server (documents index server), #1176 doc-kind-buckets
(filter chips), #1263 documents-list-cards (parallel lane: one-line
card meta, mentions as link, chips at the bottom, duplicate doc).

# Design

Per-item state already has a home: the synced settings mechanism
(#457 settings-server-side). A thread's card choice is kept per item
as JSON under one synced key (`vibe-talk.voice.thread-card-states`,
`present/rows/packing.ts`), read and written through device storage
and mirrored to the server by `core/settings/sync.ts`, per-key
last-writer-wins on the server clock. Star and hidden follow that
exact shape, so phone and desktop converge through the server state
with no new server API:

- One new synced key on both lists that must agree
  (`SYNCED_KEYS` in `src/client_settings.rs` and
  `SYNCED_SETTING_KEYS` in `web/src/core/settings/keys.ts`, compared
  by an existing server test): `vibe-talk.voice.doc-states`, a JSON
  object mapping canonical document key to its flags
  (`{"s":1}` starred, `{"h":1}` hidden, both allowed). A pure module
  under `view/documents/` (docstate.ts) owns parsing, toggling and
  pruning: malformed values read as no state, entries with neither
  flag are dropped, and the map is capped at the most recently
  changed 500 documents so it stays well under the server's
  64 KiB synced-value limit. A remote value arriving through the
  sync's `apply` redraws the Documents view.

- Star (A): a small star button on each card (filled when starred,
  `aria-pressed`, stops the card's open tap) and the same toggle in
  the reader header beside the title, for the open document. Seeing
  starred reuses the existing filter chips (owner instruction: pick
  the one that reuses existing UI): a Starred chip beside the kind
  buckets in the list's chip bar, composing with the bucket filter
  (a row must pass both). No starred-first sort.

- Hide (B): a card swipe reuses the message rows' gesture shape
  (`view/channel/gestures.ts`: pointer stream, 12px axis decision,
  90px commit, transform follows the finger, the click behind a
  committed swipe is claimed) implemented for document cards in the
  documents list module, since the channel owner is wired to channel
  messages. Either direction past the commit hides. The card leaves
  the list at once and the page's existing Undo chip
  (`#undo-dismiss`, the Done Undo) offers Undo for the usual window;
  Undo unhides. On desktop, where there is no swipe, a Hide button
  on the card does the same. Hidden rows are excluded from the
  normal list (and from the Starred/kind filtering counts' rows).
  The hamburger (#action-menu, #820 voice-page-action-menu) gains a
  "Show hidden" toggle item, offered only while the Documents view
  is up (`view/shell/actionmenu.ts` item list and
  `view/shell/controlbar.ts` per-view visibility, wired in
  `view/channel/listcontrols.ts` like its sibling items): while on,
  hidden cards are shown dimmed (`doc-hidden` class) with an Unhide
  button; starred state stays visible and toggleable there. The
  toggle itself is view state on this device only, not synced.

Frozen by #624: scrollkeep.ts, folding.ts, foldkeys.ts,
keyboard.ts and the frame/scroller CSS are not edited.

Parallel lanes on the same list (coordinate through the
coordinator if lines collide): #1263 documents-list-cards (card
meta line, chips at the bottom, dedupe), open-at-top/back-to-bottom,
and documents search. This lane keeps its list changes to the star
and hide controls, the Starred chip in the same chip bar, the
swipe, and the hidden filtering, and takes the chip bar as it finds
it.

# Acceptance Criteria

- Browser tests at phone (393x852, real swipe) and desktop
  (1280x800): star toggles on a card and in the reader header and
  persists across reload; the Starred chip shows only starred
  documents; a swipe hides a card and Undo brings it back; "Show
  hidden" in the hamburger shows hidden cards dimmed and Unhide
  restores one; desktop hide button hides.
- Sync verified through the server state: after a star and a hide,
  `GET /api/v1/client-settings` on the server holds the document
  keys under the doc-states key, and a fresh page (cleared device
  storage) converges to the same starred/hidden state.
- Focused unit tests cover the doc-states parse/toggle/prune, the
  Starred-plus-bucket filtering, hidden exclusion and dimmed
  show-hidden rendering, and the swipe commit threshold; the server
  synced-keys agreement test covers the new key.
- Selected validation passes locally before the owner-tier gate;
  the gate passes or pre-existing reds are proven on a clean
  github/main checkout.

# Notes

[watermelon-full-preview] mu-docstate, owner replies 354/358,
2026-11-10 11:03/11:08 PM ET. The synced-settings choice, over a
new per-document server API: per-item state (thread cards) already
lives there, the documents list needs no server round-trip to
filter, and the reader header shares the same storage the list
reads. Starred chip chosen over starred-first sort because the
chips already exist (#1176, moved to the bottom by #1263).
