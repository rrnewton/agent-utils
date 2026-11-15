---
title: "discord-split-merge-regression: a Discord post split at the cap is not combined on the page"
status: open
priority: 1
issue_type: bug
created_at: 2026-11-15T12:11:00.000000000+00:00
updated_at: 2026-11-15T12:11:00.000000000+00:00
---

# Description

The owner, 2026-11-15 about 7:10 AM ET (report 080, via the liaison): "Message merging in
discord seems to have regressed... I'm getting split messages (from being over size) that
are not being merged even though I have the setting on in vibetalk". An agent's over-size
reply, split by the official Discord plugin into several Discord messages at the
2,000-character cap, shows as separate messages with Combine messages on.

The setting is `vibe-talk.voice.combine` ("Combine messages", on unless off, synced
server-side). Merging is page-side only: `glom` groups the listed messages by `joinsGroup`
in `web/src/present/rows/grouping.ts` — combine on, same thread, the joining message not a
thread root, same author, same noise flag, timestamps 0–5,000 ms apart. The server and its
cache keep messages one-to-one.

Cause: on the default Discord path the server names every ordinary channel message its
own potential thread root (`src/discord/threads.rs`: `is_root` with its own id as the
thread id and a reply count of 0, so the reader can start its thread). The plugin's split
chunks are plain channel messages — only the first carries a reply reference — so chunk
two onwards each arrive as a NEW root with a different thread id, and `joinsGroup` vetoes
the join on both counts whatever the time gap. Slack survives because a split Slack post
continues as replies in the first part's thread (one thread id, not roots). Reproduced
with the real built module: Discord-shaped chunks join neither way; the Slack threaded
shape and a threadless shape both join.

Regressing commits: 04e26f6e (2026-10-25, "start native Discord threads from main
messages") gave ordinary native timeline messages potential-root records; before it those
messages carried no thread and the chunks combined under the same rule. dd1beeba
(2026-10-23, `#463 slack-text-fidelity`) is the other half: it shaped the root-aware join
rule around Slack's threaded continuations. The page fixtures could not see the break —
their split-post messages carried no thread field at all.

# Design

Page-side, in `grouping.ts` only (the lane does not touch `web/voice.js` or
`web/src/app/main.js`). A potential root — `is_root` with a reply count of 0, the server's
own "unanswered root" notion (the cache store reads it the same way in
`src/cache/rows.rs`) — counts as unthreaded for the join comparison, and only a root WITH
replies heads a thread for combining: it may head a row, never join one. Every other guard
is today's: combine on, same author, adjacent in the listing, same noise flag, 0–5,000 ms
apart (the plugin posts the chunks back to back, well inside the window). A chunk that
later gains a reply stops being a potential root, so the next render splits it back out as
its own row's head. Actions on a combined row keep addressing its head chunk, as they do
for Slack combined rows.

# Acceptance Criteria

- Three Discord chunks at the cap (first answering the prompt, consecutive ids, a second
  apart) draw as one row standing for all three ids, with Combine messages on.
- A chunk that gains a reply re-splits as its own row on the next read.
- Never combined: a different author, another author's message interleaved, a gap past
  5,000 ms, combining off, or a root with replies joining the row above it.
- Slack's threaded continuation (a root heading the parts in its thread) still combines.
- Fixtures carry the real thread records, at the module, page, and browser level, so this
  class cannot hide behind a threadless fixture again.

# Notes

Owner report 080 via the lead coordinator, 2026-11-15. Finding (cause plus regressing
commit) sent to the coordinator before the fix, as ordered.

## Resolution

`grouping.ts` gains `groupThreadOf`/`headsThread`: a potential root (`is_root`, reply
count 0) counts as unthreaded for combining, and only a root with replies is barred from
joining a row. Tests: the module test in `tests/js/present/rows.test.mjs`; page tests in
`tests/js/page/rows.mjs` over fixtures that now carry the real potential-root records,
plus a three-chunk fixture cut at the 2,000-char cap; and
`tests/discord_split_merge_browser.py` (390x844 and 1280x800) asserting the three chunks
are one row on screen while another author's message and a later post keep their own
rows. The 5-second window is unchanged: the plugin posts the chunks back to back, so
they fall well inside it.
