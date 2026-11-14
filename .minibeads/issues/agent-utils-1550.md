---
title: 'thread-list-pins: pinnable threads in the vibe-talk thread picker'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-14T19:32:00.000000000+00:00
updated_at: 2026-11-14T19:32:00.000000000+00:00
---

# Description

Owner UI request 072 (fast-tracked). Pinnable threads in the vibe-talk thread list (the pop-up thread picker, spool icon #1536):

- a PIN icon on the LEFT of each thread row in the pop-up thread list, gold styling like the existing star icon (reuse its active/inactive styling);
- pinned threads form a group at the TOP: after the fixed options, before the arbitrary recent threads; both groups sorted by most recent update;
- threads bound to a subagent (the bang-addressed kind, e.g. "!@liteinst") are pinned BY DEFAULT and labelled "@liteinst" in place of the name slug. A user can still unpin one: it stays unpinned.
- STORAGE, per the vibe-talk state-sync model: pins live SERVER-SIDE in vibe-talk's store, so they survive a reload and are shared across devices; the page reads and writes them through the server API (never localStorage as the source of truth, never Slack).

Earlier designs followed: `#194 thread-picker-polish`, `#281 view-keys`, `#427 server-thread-hues`, the spool icon for the picker, `#1265 documents-star-hide` / `#1306 doc-star-gold` (the star's `--pin-*` dress), `#206 pin-message` and `#240 reading-bookmark` (server-side owner state over HTTP).

# Design

- Message pins or thread pins? SEPARATE. The existing server lists are message pins (`pins`, #206: a snapshot of one message, bounded per channel) and the reading bookmark (#240: one anchor per channel). A thread pin keeps no snapshot, only the owner's choice per thread, so it gets its own store table (`thread_pins` + `thread_pin_revisions`, migration v26), its own routes (`GET/PUT/DELETE /api/v1/channels/{id}/thread-pins[/{thread_id}]`, read scope to list, write scope to change, no MCP tool), and its own contract roots (`ThreadPinsResponse`, `ThreadPinChangeResponse`). Message pins are untouched.
- Bang-bound threads: recorded by the bridge in `thread-bindings.json` (#917 chat-bang-commands), where `thread_id` is the `!@` message's own thread and `root_message_id` is that message. The root vibe-talk serves IS that message, so the page and the server's `bang_agent_of` read the binding from the root's `!@NAME` first word (the registry's agent-name rule) without coupling vibe-talk to the bridge's state directory. Such a thread is pinned by default with no stored record; unpinning writes a `pinned: false` record, and that record is what keeps it unpinned across devices and reloads.
- The picker: the native select menu cannot hold a button per row, so a pointer press on `#thread-select` opens the page's own pop-up (fixed options, then pinned threads, then recent threads, each group newest first; the pop-up opens upward when the bar is at the screen's foot). The select stays the value's owner — choosing a row sets it and fires its change, as a native pick does — and keyboard use keeps the native menu. Each thread row's left button is a push-pin in the star's dress: muted outline off, filled gold (`--pin-bg/--pin-ink/--pin-edge`) on. A bang-bound thread's row is labelled `@name` in place of its name slug, in the pop-up and in the select's options alike.
- Wiring without touching `web/voice.js` (off limits while #624 step 3 moves it): the view takes optional `readThreadPins`/`setThreadPin` dependencies, and the legacy composition point (`web/src/legacy/imports.ts`, the one place that may name the browser's ports) fills them in with the page's kept token until the app composition root takes the wiring over.

# Acceptance Criteria

- Server API test: pin set/unset are idempotent, listed, scoped (read lists, write changes, unknown channel 404), persist across a store reopen, and never touch the message-pin list; a bang root names its agent and an ordinary root does not.
- Browser test at 390 px and desktop: pin icon on the left of each thread row, gold when pinned, group order (fixed, pinned by recency, recent by recency), the `@liteinst` label, pinning a plain thread, and persistence of a pin and of a bang thread's unpin across reloads.
- Screenshots of the pop-up at both sizes, looked at.

# Notes

Sibling lanes on the Documents/Discuss page land independently (#1540–#1545); this change stays inside the thread picker, its store and its API. Unpinning a bang-bound thread: it stays unpinned — the `pinned: false` record stands until the owner pins it again.

## Resolution

[watermelon-full-preview] Landed as 3a338f8d on github/main (gated 2ee6d2cd on base 52bb068d, gate-ledger pass, 158 steps). Thread pins are separate from message pins (#206). Screenshots looked at: /tmp/shots-threadpin/threadpin-phone-1-popup-bang-pinned.png, threadpin-desk-1-popup-bang-pinned.png, threadpin-phone-2-popup-two-pinned.png (gold pin left of each row, @liteinst first). Browser test at 390x852 and 1280x800 plus the Rust API tests pass; the full validate DAG is green.
