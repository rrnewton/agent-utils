---
title: 'vibe-talk-short-ref-links: :thread:/:email:/:issue: short references resolve into links in vibe-talk'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-15T13:00:00.000000000+00:00
updated_at: 2026-11-15T14:30:00.000000000+00:00
closed_at: 2026-11-15T14:30:00.000000000+00:00
---

# Description

Owner UI request 093 (fast-tracked), the vibe-talk half of the design note
`ai_docs/plans/2026-11-15-thread-and-message-short-refs.md`. The bridge half
(`#658 chat-bridge-permalinks`, mu-threadlink) assigns thread and message
numbers and publishes `short-refs.json` (`agentctl-short-refs/v1`).

- vibe-talk reads the bridge's `short-refs.json` locally (path configured, not
  hard-coded), mtime-cached. No network either side.
- `:thread:NNNN` renders as a thread link: drill into the thread, scrolled to
  the top; in two-column view it opens on the right, otherwise the in-thread view.
- `:email:NNNN` renders as a message link: highlight the message in Main if the
  current view shows it (Main, All, Two column, or thread view with main
  messages on). If not, make it visible: turn on main messages in thread view,
  or jump to All when inside a thread. Reuse the existing in-app message-link
  navigation (`#543 in-app-message-links` and its successors).
- `:issue:NNNN` renders from the configured `issue_url_template`.
- An unknown or unresolvable short id stays as plain text (never a wrong
  link), with a subtle marker.
- Q2 fix (vibe-talk half): stop embedding the raw URL inside the document
  link's "Opens here — <url>" tooltip/title, so serialised Markdown no longer
  double-wraps.

# Design

Agreed with mu-threadlink before coding: file `<bridge-state-dir>/short-refs.json`,
schema `agentctl-short-refs/v1` with fields `threads[].{n,channel,root_ts,root_message}`
and `messages[].{n,channel,message,thread_root_ts?}`; `n` numeric, display
zero-padded to three digits; a shared JSON fixture (two threads, three messages)
is used verbatim in the Rust tests.

Server side: a new `short_refs` module reads the configured
`[render] short_refs_files`, re-reading a file when its mtime or size changes;
a missing or stale file resolves nothing. Rendering resolves at display time
in `render`: a resolved thread/message becomes a link marked `data-msg` plus
`data-shortref="thread"|"email"` (the `#543` marking), its `href` the in-app
deep link; an issue becomes a link through `[render] issue_url_template`
(checked at load: https, exactly one `{n}`). An unresolvable reference stays
plain text under a `shortref-unknown` span. The marker chosen: a subtle dotted
underline with reduced opacity, stated here and in the stylesheet. The search
render version bumps to `render-6` and folds a short-refs fingerprint in, so
bodies and indexes drawn under other resolutions are drawn again.

Page side (`web/src/view/shell/navigation.ts` only; `web/voice.js` and
`web/src/app/main.js` untouched): a tap on a `data-shortref` link goes through
the existing message-link jump. A thread link in two columns arrives at a
two-column place with the thread on the right; otherwise the existing jump
opens the in-thread view at its top. A message link whose row is already on
screen highlights it in place; otherwise it lands in All (`view: "flat"`),
which covers both the inside-a-thread case the owner named and a cards view
without main messages shown.

Q2: the "Opens here — <url>" titles in `web/src/view/documents/render.ts` and
`web/src/view/shell/navigation.ts` become plain "Opens here", so a copied link
carries no title to mangle. Two existing JS assertions that pinned the raw
address in the tooltip are updated to the new text.

# Acceptance Criteria

- Unit tests: the agreed fixture resolves threads and messages (a root names
  no thread over itself, a reply names its root); other schemas resolve
  nothing; issue templates are checked. Render tests: resolved references draw
  as marked links with in-app addresses, unknown ones stay plain and marked,
  references in code stay text.
- Browser test at 390 px and desktop (`tests/short_refs_browser.py`, real
  Chromium): the thread link drills into the thread, the message link
  highlights its message in Main, the issue link uses the template, the
  unknown reference is no link; screenshots at both sizes, looked at.
- No touch to `vibe-talk/web/voice.js` or `web/src/app/main.js`; no live
  configuration edited. The report names the exact overlay lines for
  `~/.config/vibe-talk-slack/app-devbig014-overlay.toml`.

# Notes

Sibling lanes: mu-pickername (`#1563`, thread picker and header)
touches `web/src/view/channel/*` and thread tests;
mu-sumfallback (`#1564`) touches `src/config.rs`, `src/contract.rs` and
`src/http/api.rs`. This lane touches `src/config.rs` only in `RenderConfig`
(new `[render]` fields, disjoint from the summariser changes) and no picker or
health-banner files. On the email fallback, the owner's "turn on main messages
in thread view" is realised as the All landing wherever the message is not
already shown: navigation has no dependency that flips that setting, and All
shows and highlights the message in every such case.

## Resolution

Landed cc91f8d1 (gate pass on c2667620, owner priority; mu-shortrefs-vt). vibe-talk renders :thread:/:email:/:issue: from the bridge's short-refs file (thread link drills in at the top; message link highlights or jumps to All; unknown ids stay plain with a dotted underline) and document-link tooltips no longer embed the raw URL. Live on devbig014 2026-11-15 08:38 ET with [render] short_refs_files and issue_url_template set; resolves end to end once #658's writer lands. Not tested: two-column right-pane open in a browser walk; a live bridge file.
