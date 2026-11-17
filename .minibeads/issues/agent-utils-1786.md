---
title: 'vibe-talk-search-scope-server-hits: in-scope server hits as cards, outside hits behind a chip under the search bar'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
- search
created_at: 2026-11-17T12:36:00.000000000-05:00
updated_at: 2026-11-17T12:36:00.000000000-05:00
---

# Description

[watermelon-full-preview] Owner, 2026-11-17 (owner thread 134, follow-up to #1762 vibe-talk-server-hits-collapsed-line), verbatim: "Ok the search results is better.. it has a separator line at the BOTTOM "On the server, not loaded here" With a button showing the number of messages. But now I'm not sure if I like it. If I am in a THREAD I have a contiguous suffix of the threads messages loaded (right?) and I search it filters to a subset of those... if there are older messages in the SAME THREAD that are hits... well if we DID load back further in the thread, cards would appear. If we want to just show server hits as cards, that would just be a sparse simulation of what loading a longer suffix of the message history right? If there are hits OUTSIDE the thread... that's a different question. Our server side hits are basically currently a disorganized mix of showing hits outside the current scope (thread) and hits in the current thread? I think we should just show the hits inside this thread as cards in the normal way... we just simulate a further-back load. But we could show a big button/chip UNDER the search bar indicating "Show XYZ hits outside this thread." If I click it it can show the extra messages in the current "server side" style -- quote styling, not full boxes. They currently have little headers that say: "> herdr-chat-bridge Nov 2, 11:17 AM in a thread". I think that should say the thread slug .. "in thread x-y-z" if available. And when it is clicked on we should put them inline in the chronological timeline interspersed with the current-thread hits."

The liaison's reading, which the owner saw:

1. In a THREAD view, server hits that belong to THIS thread render as normal message cards in chronological order, as if the thread had been loaded further back. Retire the bottom "On the server, not loaded here" separator for in-scope hits.
2. Hits OUTSIDE the current thread go behind a prominent chip UNDER the search bar: "Show N hits outside this thread". Clicking it shows them in the existing server-hit quoted style (not full cards), INTERLEAVED chronologically with the in-thread hits.
3. Quoted-hit headers say "in thread <slug>" (the thread's short name/slug when known) instead of "in a thread".
4. Channel view: apply the same rule (in-channel hits as cards; "Show N hits in other channels" behind a chip). The owner may object, so keep it a small, separable change and say so in the report.

Today (#1762, landed e27051df) every fresh server hit — in-scope and out-of-scope mixed — waits behind one tappable line at the foot of the list and draws as quoted snippets in `#server-hits`; the partition this issue asks for does not exist.

# Acceptance Criteria

- In a thread view, a server hit in this thread draws as a normal message card in the chronological timeline (a sparse simulation of a further-back load: just those messages, placed by time); no bottom separator presents it.
- A chip under the search bar reads "Show N hits outside this thread" (thread view) or "Show N hits in other channels" (channel view); clicking it interleaves the outside hits, in quoted server-hit style, chronologically in the timeline; clicking again puts them away. The chip is hidden when there are no outside hits.
- A quoted hit in a thread says "in thread <slug>" when the thread's slug (its filed `display_name`) is known, and "in a thread" only when it is not.
- In a channel view, in-channel server hits draw as cards the same way; the channel-view change is small and separable from the thread-view change.
- Typing still filters the loaded cards instantly (#1762): no server request and no server-section re-render while typing; server hits are placed only when the deferred answer lands.
- Page tests at phone (412x915) and desktop (1280x800) sizes cover the exact behaviour, in light and dark theme, with the keyboard open (visualViewport shrink) where layout is involved, and the Discuss half-pane open in at least the phone shots.
- Screenshot matrix per the standing acceptance (light/dark x keyboard open/closed x phone/desktop, Discuss half-pane open in at least the phone shots, before and after the targeted state), reviewed image by image, folder path and one line per image in the report to coordb; the liaison reviews before anything is announced.

# Notes

## Resolution

[watermelon-full-preview] Landed on main as a84def0b (from gated bf85487f), highlight follow-up 10baac8c. In-scope server hits stand as cards in the chronological timeline; outside hits behind a chip under the search bar (superseded in form by #1797's bucket row, same behaviour per bucket); quoted headers say "in thread <slug>" when known. Screenshot matrix in /tmp/mu-searchscope-shots (before: 48, after: 68) plus /tmp/mu-searchscope-shots/after2 (review fixes), all inspected; tests server_hits_browser 8/8, search_unmatched_browser phone+desk, JS search suite green.
