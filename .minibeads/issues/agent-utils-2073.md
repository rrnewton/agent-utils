---
title: 'vibe-talk-new-summary-button: a New summary button next to Update'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-21T13:55:00-05:00
updated_at: 2026-11-21T13:55:00-05:00
---

# Description

The owner: "I just tried to update button for the first time on an existing summary and I really like it. It works well. I would like a little button next to it for quote new Summary. The idea is that maybe I want to cut a fresh summary with a new messages since the last one instead of updating the current summary with the new messages, both are valid options. If I click new summary, of course the old one goes into the timeline as an earlier summary."

# Findings (where Update lives today)

Only Discuss (catch-up) summaries carry an Update button: the staleness
banner over a saved Discuss record (`#discuss-stale` in the panel, one
button per scope — channel or thread). There is no Update button on any
other summary surface. Update (`POST /catchup/update`, `#1388` landing 2)
spends one cache-only model call over only the unread messages the saved
summary did not cover, with the old summary text as context, and merges:
the merged text replaces `current_summary` in place, covered ids union,
the timeline is untouched. Records are one `DiscussRecord` per
(channel, thread) in the store: `current_summary` on top, `timeline`
(oldest first) holding follow-up turns and superseded summaries; a fresh
Summarize already archives the summary it replaces into that timeline.

# Design

Next to Update in the same banner, a small New summary button with the
same style on phone and desktop. It calls a new route,
`POST /catchup/new-summary` (same request body as Update — the scope
alone — and the usual `CatchupResponse`, so the change is
back-compatible): one cache-only model call over only the messages new
since the previous summary's end (unread now, not in its covered set),
with no old summary text as context. The fresh summary becomes the
record's `current_summary` (covered ids = only the new range, its
`since_ms` = the first new message); the previous summary is appended to
the timeline as an earlier summary, drawn in the existing folded style
and labelled with the range it covers (`covers HH:MM–HH:MM` from its own
covered links, beside its made-time and unread count). Update keeps its
current in-place behaviour. After the call the panel reloads the record,
so a reload (or any reopen) shows the same two summaries — persistence
is the record itself, no new storage.

Edge cases, as chosen:

- No new messages since the last summary: New summary is **disabled
  with a short reason** (`No new messages since the last summary`, as
  its title/`aria-disabled`); the server also refuses without a model
  call. Update stays offered in that state, as today.
- A New summary (or Update) while one is in flight starts nothing: the
  panel's standing `pending` guard covers both banner buttons.
- A model answer of "nothing new" keeps the standing summary on top
  rather than archiving it under an empty one.

# Acceptance Criteria

- New summary creates a second summary over the newer range and keeps the first; browser checks with a model stub; screenshots phone and desktop, light and dark.

# Tests

- Server (`tests/catchup.rs`): a New summary covers only the new ids,
  carries no old text in prompt or answer, archives the old summary with
  its covered range, round-trips through the record route; refusals
  (no summary yet, nothing new) spend no model call. The standing Update
  tests still prove the in-place merge.
- Panel (`tests/js/view/catchup.test.mjs`): New summary makes its one
  fresh call and no full re-summarize, the fresh card stands on top and
  the first is in the timeline labelled with its range; disabled with
  its reason when nothing is new; a second press in flight starts
  nothing (nor does Update).
- Browser (`tests/discuss_new_summary_browser.py`, dag step
  `discuss-new-summary-browser`): real Chromium at phone 412 and desk
  1280 over a network-edge model stub — fresh card over the newer
  range, first kept and range-labelled, reload persistence, Update
  still merging in place with no new timeline entry.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
