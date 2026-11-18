---
title: 'vibe-talk-discuss-title-and-body-match-scope: Discuss header and body always match the scope on screen'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-18T08:20:53-05:00
updated_at: 2026-11-18T08:20:53-05:00
---

# Description

[watermelon-full-preview] Owner reports (bridge 173, 175, same thread): the Discuss view changes when changing threads or going from a thread to the main channel, and it says "thread #agent-utils-014" — the name of the whole channel, not the thread. Then: "the TITLE changes but the actual summary doesn't". Likely sources: #1811 (Discuss scope follows the open thread, deploy 2674b671), #1821 (Discuss state persists across refresh, deploy fddd3b80), #1812 (Discuss off in Documents, 190f8e27).

Root cause (found before the fix): (1) Header — `scopeNow` in the Discuss view falls back to the channel label when the open thread's summary record is absent or unnamed at navigation time, so a thread scope is labelled with the channel's name and the title reads "Discuss: thread #<channel>". (2) Body — the panel's `rescope` only re-reads the preview and the saved record; it never clears the answer, terms, timeline or footer on screen, and a scope with no saved record draws nothing new, so the previous scope's summary stays under the new title. The persisted snapshot is a single panel entry, so switching scope has no per-scope state to restore.

Owner spec (bridge 176): "When I go from main=>thread I want to be essentially switching discuss windows. From summary of the whole space to summary of just the thread (if generated) or blank and you need to press button if not." One Discuss state per scope (the channel's main scope, and each thread), switched on navigation and restored on the way back; nothing carries across scopes.

# Design

- One Discuss state per scope: the panel keeps each scope's answer separately (in memory, keyed by channel plus thread-or-main), stashes the outgoing scope's state on a scope switch, and restores the incoming scope's own state — its summary if one was generated, otherwise a blank pane with the preview's unread count and the Summarize button. No scope's summary, terms, timeline or footer ever shows under another scope's title.
- On any scope change the outgoing scope's in-flight request is invalidated (the generation guard already drops a late answer; the switch clears the pending body so a late answer for the old scope cannot land in the new scope's pane).
- Header: a thread scope is named by the thread itself — its filed slug (`display_name` held to slug shape), or its first line (`title`) truncated — never the channel's name or label. A main scope reads "Discuss: main channel". The persisted restore path uses the same naming rule and never labels a thread with the channel name.
- Persistence: each scope's saved summary remains the server's per-scope record (read for that scope only); the device's kept panel state names the scope it restores, and a restored thread scope is titled by the thread rule above.

# Acceptance Criteria

- Inside a thread the header shows the thread (filed slug, or first line truncated), e.g. "Discuss: thread my-thread-slug"; in the main view "Discuss: main channel"; never "thread #<channel>".
- On any scope change (thread A -> thread B, thread -> main, main -> thread) the pane shows the new scope's summary/transcript, or clearly says there is none yet for this scope with Summarize available; it never keeps the old scope's summary under the new title.
- A late answer for the old scope arriving after a switch does not land in the new scope's pane.
- Deterministic test (condition waits only): open Discuss on thread A with a summary, switch to thread B, then main, then back to A; header and body match the scope each time, including a late answer for A arriving after the switch to B.
- Page tests at phone and desktop sizes; screenshot matrix (light/dark, keyboard open/closed, phone 412x915 and desktop 1280x800, Discuss half-pane open in phone shots) inspected.

# Notes

Owner-priority, deployed on landing. Sibling: the default bound since the last summary is a separate issue filed alongside this one.

## Resolution

[watermelon-full-preview] Fixed on this branch: Discuss now keeps one state per scope. The panel stashes each scope's answer (keyed by channel plus thread-or-main) and a scope switch — main -> thread, thread -> main, and sideways thread -> thread (new `sideways-rescopes-to-thread` binding rule) — shows the incoming scope's own stashed summary, or a blank body with that scope's preview and Summarize; the old scope's answer, terms, timeline and footer are cleared, and a late answer for the old scope is dropped by the generation guard. The header names a thread by its filed slug or its first line (truncated), never the channel: `scopeNow` no longer falls back to the channel label, the Discuss selection in the page stands in the store's thread summary (or the root's filed name/first line, as the thread button names it) when the selection carries none, a nameless bound label is replaced when the name lands, and the kept panel state now persists the scope's own label for the restore path. Tests: new deterministic A -> B -> main -> A unit test plus a late-answer/unnamed-thread test (both fail on the unpatched base, pass patched), updated sideways and persistence tests; catchup and settings JS suites and the web typecheck pass. Screenshot matrix on a fake-API test instance (fixture threads A/B): 25 images in /tmp/mu-discussscope2-shots-1841/ — phone 412x915 (scale 2.6) and desktop 1280x800, light and dark, keyboard open (viewport shrunk 336px, visualViewport 579) and closed, the A -> B -> main -> A sequence in each, all inspected.
