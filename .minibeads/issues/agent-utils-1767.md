---
title: 'vibe-talk-documents-list-filename-search: limited filename and title search in the Documents list'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
- documents
created_at: 2026-11-17T03:59:00-05:00
updated_at: 2026-11-17T03:59:00-05:00
---

# Description

[watermelon-full-preview] Owner, verbatim (2026-11-17 3:59 AM ET): "I want
limited search in documents list. Just for filename or whatever time we
have downloaded. Full text search for downloaded markdown can just be a
server side rg invocation. It can be an optional check box with default
filename and title."

Read as: the Documents list gets a fast, LIMITED search that by default
matches only each document's filename and title (and whatever other list
metadata the page already has downloaded, e.g. path/repo, if cheap),
filtering the list in place as you type, client side, no server round
trip. An optional checkbox (unchecked by default), e.g. "Search text",
switches to full-text search of the downloaded markdown on the server.
"whatever time" is probably voice dictation of "whatever title"; if the
list carries a downloaded/fetched time, showing it is fine but the match
is on filename + title.

Prior work: #1266 documents-search (closed) already searches fetched
documents on the server over title+body through the shared search
language. #1022/#1047/#1079 set the Documents list order (forward
chronological, newest at the bottom) and its live updates; the filter
keeps that order and survives refreshes.

# Design

Keep #1266's server path as the checkbox's full-text mode: it already
covers downloaded markdown (the bounded cache of fetched copies, title
plus body, with the searched count stated), so no new server endpoint
and no `rg` shell-out is needed. The downloaded markdown lives in that
bounded in-memory cache, not as files the server can `rg` without first
materialising them; the existing in-process match is simpler, bounded,
and states its reach. The checkbox therefore reuses
`GET /api/v1/documents/search` exactly as #1266 built it.

Default (unchecked) mode is instant and purely local: typing in the
Documents search field filters the already-downloaded list rows in
place, case-insensitively, over file name, title, and the cheap metadata
already on each row (its key and address, which carry owner/repo and
path). No server call is made. The filter composes with the existing
type, Starred, and hidden filters, never reorders the list, and is
re-applied when a live refresh lands new rows. An empty result says so
("No documents match ...") instead of showing a bare list. Ticking
"Search text" clears the local filter and runs the #1266 server search
(Enter or Search) over fetched documents' text, showing its hits and
the "N fetched documents searched" reach line as today; unticking
returns to the local filter. The reader still opens at the highlighted
match from a full-text hit.

# Acceptance Criteria

- Typing a filename or title fragment (any casing) narrows the Documents
  list in place with no server request while "Search text" is unchecked.
- Ticking "Search text" and searching runs the server full-text search
  over fetched documents and opens a hit in the reader at its match.
- A query matching nothing shows a no-match state in both modes.
- The list's chronological order and live refresh are preserved under
  an active filter.
- Page tests at phone and desktop sizes cover filename, title,
  case-insensitive matching, the no-server-call default, and the
  checkbox path; the existing server tests cover the full-text path.
- Screenshot matrix on a TEST instance (light/dark, keyboard open/closed,
  phone/desktop, Discuss half-pane on phone, filtered, full-text hit,
  and no-match states) is captured and inspected.

# Notes

[watermelon-full-preview] Filed by mu-docfilter from the owner's
2026-11-17 request (FAST-TRACK, owner rule 286). The full-text decision
(keep #1266's server search rather than a server-side `rg`) is recorded
in the Design section above and in the landing report.

## Resolution

[watermelon-full-preview] Delivered on main by
ebfd627c48e85cb73a215ef6402a52ced1dbf935: the Documents search field
filters the list locally by file name and title as the owner types (no
server call), and the "Search text" checkbox switches to #1266's server
full-text search over fetched documents. Page tests in
vibe-talk/tests/js/view/documents.test.mjs cover filename, title,
case-insensitive matching, the no-server-call default, and the checkbox
path; vibe-talk/tests/documents_browser.py covers both modes at phone
and desktop sizes against the real server; the existing server tests
cover the full-text path. Screenshot matrix (16 images, TEST instance)
in /tmp/docfilter-shots, inspected by mu-docfilter; liaison review
pending before announcement. Local validate and owner-priority gate
both passed.
