---
title: 'message-rerender-loses-formatting: a block post re-read from history loses its lines and links'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-15T17:06:00.000000000+00:00
updated_at: 2026-11-15T17:06:00.000000000+00:00
---

# Description

Owner UI bug 099 (fast-track, owner rule 286): a coordinator reply in a
Slack thread "WAS formatted and then I blinked and it was mushed together
and busted". Single newlines collapsed into one run-on paragraph, and
Slack mrkdwn links `<https://…|GH #329>` showed as literal
`<https://… GH #329>`.

Evidence: the live timeline payload for that message holds one flattened
line whose links read `<[https://…/329 GH](https://…/329%7CGH) #329>`;
the app log shows the thread served `cache-first` on every poll. The
message first arrived on the live event stream with its line breaks and
links intact, but the next cache-first timeline read re-fetched it
through the history reader, which returns block posts flattened onto
one line with each link double-converted, and that mangled text is what
re-rendered on screen.

# Design

- `slack/mrkdwn.rs`: `repair_fallback_links` undoes the exact
  double-converted shape (`<url|label>` as `[url word](url%7Cword) rest`)
  only when the decoded address provably equals the link text with its
  first space for the pipe; `split_glued_bullets` puts ` • `-flattened
  bullets back on their own lines when a line holds at least two and a
  following piece reads as an item. Both run in `to_markdown_with`, so
  new cache writes store the live shape.
- `render.rs`: `Markup::prepare` (Slack) applies the same two repairs,
  so a row already cached in the flattened shape draws as its first
  render did; every serving path (live event, cache-first read,
  refresh, edit update, history page) ends in this one renderer.
  `SearchText::VERSION` render-7 -> render-8 (rebased over #1607) (digest unchanged, pinned).
- Untouched on purpose: a genuine Markdown link, a lone in-sentence
  bullet, and a plain enumeration pass through unchanged (negative
  controls in the unit tests).

# Acceptance Criteria

- Unit: the history shape of the exact message normalizes to the live
  shape; both draw identical HTML with bullets as separate list items
  and `GH #329` as a link label, with no `%7C` or literal brackets.
- Browser (`message_rerender_browser.py`, registered in
  `tests.dag.yaml`): at 390x844 and 1280x800 the two bullets are on
  separate lines, `GH #329` is a link label with the right address, and
  no raw link markup is on screen; screenshots inspected.
- Full vibe-talk library suite green; capped `make validate` selection
  and gate green on the landed head.

# Notes

[watermelon-full-preview] Owner UI request 099; prior work reused:
#463 fidelity/--slack-mrkdwn, the newline rebuild and
`markdown-tables-render` (glued-table repair precedent), #1584
short_refs overlay, the serve=true message cache, #1227. Deployed by
the coordinator on landing (owner 096: test instances only for live
experiments; the live instance was only read, never restarted).
