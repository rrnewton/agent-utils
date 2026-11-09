---
title: 'raw-message-bridge: owner-only raw keystroke delivery to the bridged agent'
status: in_progress
priority: 1
issue_type: feature
assignee: mu-raw
created_at: 2026-11-09T14:01:57.599336424+00:00
updated_at: 2026-11-09T14:07:33.833788441+00:00
claimed_at: 2026-11-09T14:02:35.614042985+00:00
claimed_until: 2026-11-11T14:02:35.613931587+00:00
---

# Description

Owner request (reply 240, 2026-11-09): a naked message capability that bypasses the whole protocol of attaching headers and footers and sends the keystrokes outright directly to the TUI of the bridged agent, exactly like typing in the terminal, so that for example a slash command works.

Today every chat message is wrapped with a sender header and reply instructions before it reaches the agent, so nothing typed from the phone can act as direct terminal input. This issue is the delivery half; the companion page issue adds the checkbox that sets the marker.

# Design

vibe-talk marks the post as raw: event_type stays vibe_talk_op_key and event_payload gains raw: true next to op_key (the Slack shim accepts only a top-level event_type string plus event_payload object, 8 KB, and constrains nothing inside the payload; old readers ignore the extra field). The agentctl chat bridge (Rust, Slack plugin) reads the marker and delivers the text to the bridged agent's TUI as plain keystrokes: no sender header, no reply instructions, no footer.

- Owner-only: the bridge honours raw only when the Slack sender is the owner's user id (vibe-talk posts as the owner), using the existing sender_grants authority. Anyone else's raw marker is ignored and the message is delivered normally with a note. Direct terminal input fails closed.
- Delivery: single-line text is typed, then submitted. Multi-line text goes as a bracketed paste, then submit, only where each harness (the three supported terminal harnesses) treats that correctly; a slash command must actually run, sent as its own one-line raw message.
- Ordering: raw messages keep their place in the bridge's existing per-agent delivery order; no skipping the queue. Raw is its own queue item: never merged by the queued-messages combiner, never wrapped by the request builder.
- Reply cycle: no reply is expected. The bridge records delivery (the usual receipt or reaction) and retires the message at once, with no reply prompt and no waiting for a chat reply.
- Display: the Slack and vibe-talk message shows the text as typed, plus a small raw marker.
- Safety: a bounded length (cap stated in code and docs) and control characters stripped or refused except newline. Behaviour documented in the bridge help, including the one-line slash-command rule.
- No shim change is expected (the marker rides inside the existing payload); anything the shim would need is a separate deploy for the coordinator, named explicitly.

# Acceptance Criteria

- An owner raw message reaches the bridged agent's TUI exactly as typed (no header, no footer) and a harmless slash command actually runs on each supported harness.
- A non-owner raw marker is ignored: the message is delivered normally with a note, never as raw keystrokes.
- Single-line and multi-line raw text both submit correctly; over-length text and disallowed control characters are refused or stripped as documented, with newline preserved.
- Raw messages preserve per-agent delivery order and retire on the delivery receipt with no reply prompt.
- Bridge tests cover owner raw, non-owner raw, multi-line, slash command, ordering, and receipt without a reply prompt, against scratch bridges and registries only.

# Notes

Owner request reply 240 (2026-11-09). Companion: #1135 raw-message-page. Slot raw, branch mu-raw/raw-message. Bridge home: Rust agentctl chat Slack plugin. Shim on the deployment host is read-only; any shim change is a separate deploy for the coordinator.

Renumbered (2026-11-09): filed first under a number another lane landed on main meanwhile; this number is the one on main.
