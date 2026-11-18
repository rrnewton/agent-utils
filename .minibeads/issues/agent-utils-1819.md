---
title: 'vibe-talk-thread-button-shows-slug: expand the Thread(n) button to show the thread slug when the row has room'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-17T20:02:16-05:00
updated_at: 2026-11-17T20:02:16-05:00
---

# Description

[watermelon-full-preview] Owner UI request, thread 158 (2026-11-17), fast-tracked. The owner asks: "ALSO if we have ROOM on a given display weren't we supposed to expand that "Thread(57)" button to show the name of the thread? It could just be thread emoji followed by the slug ":thread:my-thread-slug(57)" with the count there still. Do we have a grayscale version of that emoji?"

Scope is the button label only. #1818 owns anchoring. When the message row has room, the Thread(n) button shows the thread emoji, then the slug, then the count, e.g. "🧵 my-thread-slug (57)", truncating the slug with an ellipsis to fit. When there is no room it falls back to today's "Thread(57)". The slug comes from the existing slug source, the same one #1796's :thread: refs use.

# Design

- Prior-work finding (tracker, git log and ai_docs searched in this lane): no earlier issue or plan asks for, or specifies, expanding the Thread(N) button to show the slug. Adjacent landed work only: #252 thread-button-state (grey Thread until the first reply), #182 thread-display-names / #1563 thread-picker-names-and-width (slug plus readable title in the picker and header), #1607/#1796 (🧵 plus number, and 🧵NNN/slug, for :thread: refs in message text). This issue is the first ask for the button itself.
- Button only (`vibe-talk/web/src/view/channel/replycounts.ts` `paintThreadBadge`, `vibe-talk/web/src/view/channel/threadlist.ts` `addThreadDecoration`, and the badge stylesheet). The coloured open state with a known slug and replies draws emoji, slug and count as three spans ("🧵 my-thread-slug (57)"); the slug span truncates with an ellipsis. The grey start state and any thread without a slug keep today's exact labels ("Thread", "Thread(N)").
- Slug source: the thread's filed `display_name` the page already holds — the thread summary's `display_name`, else the root's own `thread.display_name` — which is the same filed name #1796 joins onto :thread: refs server-side (`ThreadNames` via `short_refs::thread_slug`, held to `valid_thread_slug`). The badge reads it from the page store at paint time, so no new fetch is made.
- Room: the row's list is already an inline-size container (`rows`); the expanded form is capped and the slug ellipsises, and under the narrow container threshold the slug and emoji are hidden and the compact fallback shows. A `data-thread-slug` marker records the expanded form for tests.
- Grayscale emoji: no grayscale thread-emoji asset exists in the tree (the only thread emoji is U+1F9F5 🧵 from `slack/emoji_names.tsv`, drawn as text). The emoji is drawn in a span with `filter: grayscale(1)` and reduced opacity; colour and grayscale are both captured in the screenshot matrix and the version that reads well in light and dark theme ships.

# Acceptance Criteria

- With a filed slug and room, the button reads "🧵 <slug> (N)" with the count still present; a long slug truncates with an ellipsis and the button never wraps the row's header line.
- Without room, or without a slug, the button reads exactly "Thread(N)" (and "Thread" for the grey start state), as today; existing Thread(N) page tests still pass.
- Page tests at phone (412x915) and desktop (1280x800), light and dark, keyboard open and closed, with the Discuss half-pane open in at least the phone shots, covering the expanded, truncated and fallback forms; condition-based waits only.
- Screenshot matrix on a local test instance (never the live instance or its database): light and dark theme × keyboard open (visualViewport shrink emulated) and closed × phone 412x915 (scale 2.6) and desktop 1280x800, Discuss half-pane open in at least the phone shots, plus before/after of the button and both the colour and grayscale emoji; every image looked at and reported, folder path and one line per image in the handoff.
- `make validate` selection for the change passes; the landed SHA is reported to coordb, who deploys.

# Notes

[watermelon-full-preview] mu-threadbtn, 2026-11-17. Investigation and implementation in this lane; see the handoff for the prior-work report, the grayscale answer, and the screenshot folder.
