---
title: 'documents-list-cards: one-line card meta, bottom filter chips, no duplicate documents'
status: closed
priority: 1
issue_type: task
assignee: mu-doclist
created_at: 2026-11-11T04:11:00+00:00
updated_at: 2026-11-11T04:11:00+00:00
---

# Description

[watermelon-full-preview] Owner replies 353 and 357 (2026-11-10 ET), from a
phone screenshot of the vibe-talk Documents list. Three asks in one lane
because they share the list's files:

(A) Reply 353: "These blue buttons for "3 mentions" are overkill and wasting
space vertically. Let's make it normal hyperlinked text and the "fetched" and
mentions and GitHub markdown links should all fit on one line." Per card, the
"N mentions" pill becomes plain hyperlinked text with the same action (it
opens the mentioning-messages pop-up; one mention still jumps straight to
that message). The meta line holds everything on ONE line:
"<filename> · mentioned 6h ago · fetched just now · 3 mentions ·
github_markdown" (or the kind label, e.g. paste). When the width is short,
ellipsize the filename first; never wrap onto extra rows. Cards get
correspondingly shorter.

(B) Reply 357: "We left our filter chips at the top of the scrollback in
document list. Thats an error. They should be at the bottom where we start".
The list is newest-at-bottom (#1079 documents-chronological) and opens at
the bottom, so the filter chips (kind/bucket: All / Markdown / Pastes /
Issues / Other files; and a Starred chip later, from a parallel lane) must
sit at the BOTTOM, docked above the bottom bar, visible on open without
scrolling. Keep the chips' API simple: one bar element the star lane can
add a chip to.

(C) Same screenshot: one document (file
2026-11-10-android-quest-research-proposal.md) appears TWICE in the list.
Investigation in this lane: the server index (vibe-talk/src/documents.rs)
is a HashMap keyed by the canonical document key
`github:{owner}/{repo}@{ref}:{path}`, so it cannot return the same key
twice; the duplicate is two distinct keys for the same file differing in
the ref (or host form) segment, i.e. two URLs for the same document. The
list fix here: dedupe in the client list by document key, and collapse
rows that name the same document (same kind, owner/repo and path for
GitHub kinds; the key itself for paste/gist kinds) into one entry —
newest mention wins the place, mentions are unioned, mention_count is
the union's size, fetched if either copy is fetched. The deeper server
canonicalisation (the ref being part of the key) is filed separately if
the owner wants one entry server-side too; say so on landing.

Prior work: #629, #1022, #1047, #1079 (newest at bottom), #1081,
#1235/#1236 (documents reader), #1176 (kind buckets). Frozen by #624:
scrollkeep.ts, folding.ts, foldkeys.ts, keyboard.ts and the frame/scroller
CSS must not be edited.

Parallel lanes on nearby code (star toggle + Starred filter, open-at-top /
back-to-bottom, documents search) share the list's neighbourhood; this
lane keeps its change to list-card layout, chips placement and dedupe.

# Acceptance Criteria

- Browser test at phone (393x852) and desktop (1280x800): one meta line
  per card with no wrap, the mentions text link opens the
  mentioning-messages pop-up, the filter chips are visible at the bottom
  on open without scrolling, and the duplicate case renders one entry.
- Focused unit tests cover the one-line meta composition, the mentions
  link action, the bottom chips placement, and dedupe (exact key and
  same-document-different-URL).
- Selected validation passes locally before the owner-tier gate; the gate
  passes or pre-existing reds are proven on a clean github/main checkout.

## Resolution

Landed 3935b9df on github/main. (A) Card meta is one ellipsising line with mentions as linked text opening the same pop-up. (B) Filter chips are the list's last row, visible on open at the bottom (no sticky: the scroller containment guard forbids positioned escapes). (C) Client dedupe by document identity (owner/repo+path, ref ignored) with unioned mentions; the server key still carries the ref, filed here as the deeper canonicalisation follow-up if wanted server-side. Chips API: one `.doc-buckets` bar from bucketBar() for the star lane to extend.
