---
title: 'post-to-main: a quote-reply checkbox that starts a clean top-level thread'
status: open
priority: 2
issue_type: feature
created_at: 2026-11-08T10:06:03.000000000+00:00
updated_at: 2026-11-08T10:06:03.000000000+00:00
---

# Description

The owner (2026-11-08): while reading a status update inside an existing thread with regular updates, they want to start a new discussion from one point it mentions, without forking the conversation inside that status thread. Today quote-reply in a thread can only reply in that thread, so promoting one item into its own clean, isolated single-topic thread means abandoning the quote context or copying text by hand into a fresh top-level post, which loses the link back entirely.

Wanted, in the owner's words: "when I click quote reply and I get a compose box below the current message box, I would like a small checkbox to the left of the send cancel buttons" that makes Send post to the main channel instead of the current thread, while vibe-talk's reply-to edge still points at the quoted message — "that would actually give me a blue arrow to click and follow across threads", so the root message of the new thread is itself a reply to a message in another thread. Not Slack's also-send-to-channel broadcast: the owner does not expect the post to appear in both places. On every backend it is a plain top-level post that starts a new thread.

# Design

- The inline quote-reply composer under the message gains a small "Post to main" checkbox to the left of its Send/Cancel buttons. Unchecked is today's behaviour exactly. The checkbox does not persist: every quote-reply starts unchecked, so the exceptional destination is chosen each time and never surprises a later ordinary reply.
- Checked, Send posts a new top-level channel message through the same send path a top-level post already takes, carrying the same reply-to the thread send would have carried. Nothing provider-specific is used — in particular not a broadcast flag that would show the post in both places — so the behaviour is the same on every backend.
- The reply-to edge is a vibe-talk-level record and must survive being cross-thread everywhere it is read: the new root renders with its reply arrow, following it crosses to the quoted message in its own thread, and nothing that lists, nests or counts replies may assume a root has no reply-to or that a reply-to target shares the thread. Where such an assumption exists it is fixed as part of this change, and the server accepts a top-level post whose reply-to names a message in another thread of the channel (refusing only what it already refuses: unknown messages, other channels).
- The composer now lives under `vibe-talk/web/src/` (quote-reply and quick-reply modules); touched ranges there are cleared with the in-flight page-modularity work through the co-ordinator before editing.

# Acceptance Criteria

- Page/unit tests: the checkbox renders left of Send/Cancel in the inline quote-reply composer, starts unchecked on every open (including after a checked send), and the send path posts top-level with the quoted message as reply-to when checked and in-thread when not.
- A server/API test: a top-level post carrying a reply-to to a message in another thread is accepted, stored as a thread root with that edge, and read back with it; cross-channel and unknown targets keep their current refusals.
- A browser check: quote-reply inside a thread, check the box, send, and the new root appears in the main channel (not the old thread) with a reply arrow that follows to the quoted message.
- The exact push tree passes the full validation gate, `check_client_names` and `cargo fmt --check`.

# Notes

[watermelon-full-preview] Filed by mu-spark. The checkbox's non-persistence is the co-ordinator's call, put to the owner as the default in this design.

## Resolution

Pending.
