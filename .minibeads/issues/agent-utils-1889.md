---
title: 'vibe-talk-thread-ref-opens-thread-view: a :thread: reference opens the thread view, root highlighted'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-18T21:00:00-05:00
updated_at: 2026-11-18T21:00:00-05:00
---

# Description

[watermelon-full-preview] Owner bug (thread 194, 2026-11-18 5:35 PM PT), verbatim: "By the way I just clicked a link to thread showing as a short (:thread:180). It linked here: <https://internalmeta.slack.com/archives/C0CC755QYSK/p1795014797167219> But clicking it jumped me to that MESSAGE in the All view. It did NOT jump me to the thread view. As i said before that is the difference between linking a MESSAGE and linking a thread. If i click the thread I want to jump TO the thread -- thread view, seeing all messages in the thread. But yes highlighting the first on."

Context: #1565 vibe-talk-short-ref-links (closed) specified that a :thread: ref drills into the thread view (or the right column in two-column view); #657 thread-reference-links (in-app link jumps) is still open. Root cause found in this lane: a raw `:thread:NNNN` in message text renders with `data-shortref="thread"` and a thread target, but a bridge-expanded `:thread:` reference (the shape the owner clicked: a Slack permalink whose label is `:thread:NNNN`, drawn 🧵NNN) renders as a plain message permalink with only `data-msg` and no thread intent, so the click handler treats it as a message jump to All. The one-column thread branch of the click handler also never highlights the root message.

# Design

Render (`vibe-talk/src/render.rs`, `mark_message_links`): when a link's address is a recognised Slack message permalink and its whole label is a thread short reference (🧵NNN, optionally with its filed slug) that resolves through the installed short-refs to that same channel and root, mark it `data-shortref="thread"` with a thread target (`data-msg` naming the thread, no message), exactly as a raw `:thread:` reference is marked. A label that does not resolve to that root keeps today's plain message marking, so no link ever gains thread intent it did not earn.

Page (`vibe-talk/web/src/view/shell/navigation.ts`, `jumpToShortRef`): a thread tap opens the thread view from All, Main and two-column views — the right column in two columns, the thread view otherwise, across channels as a message jump crosses them — with the existing Back level, and highlights the root message (the first message) once the thread is shown. Plain message links, including a pasted permalink to a root that has replies, keep today's message jump; whether such a permalink should instead open the thread view is the owner's open choice, deliberately unchanged here.

# Acceptance Criteria

- Clicking a `:thread:` reference (raw or bridge-expanded) from All, Main and two-column views opens the thread view (right column in two columns) with the root message highlighted.
- A plain message link, and a pasted permalink to a thread root, still jump to the message as today.
- A thread label that resolves to a different root than its permalink keeps the plain message marking.
- Cheap committed tests: render keeps the thread intent on a bridge-expanded reference; existing render and navigation suites pass.

# Notes

Related: #1565 vibe-talk-short-ref-links (closed, specified this behaviour), #657 thread-reference-links (open, in-app thread jumps), #1713 vibe-talk-short-ref-link-double-render (bridge-expanded shape), #543 in-app-message-links. Owner clarification (thread 193): dynamic testing is manual exercise on a real test server with evidence, not a new slow browser DAG step; screenshots follow the standing matrix on the #1855 harness.

## Resolution

Landed 64ab9596 on github/main (gate pass on 65f60bfd, owner priority; renumbered from #1877 after a number collision on main). Bridge-expanded `:thread:` references now render with `data-shortref="thread"` and a thread target; the click opens the thread view (right column in two columns) from All, Main and two-column views with the root message highlighted. Plain message links and pasted permalinks keep the message jump, as designed. Evidence: cargo render tests (2 new), short-refs browser walk at 390px and 1280px asserting the root highlight, screenshot matrix in /tmp/mu-threadref-shots (BUILD.txt head 9ba33d19, pre-renumber tree; code-identical except the renumber) and /tmp/mu-threadref-browsertest. Not verified live: a production bridge file end to end; coordb deploys.
