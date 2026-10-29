---
title: 'muse-bridge-latency: Muse delivery, idle detection and steer recovery for the chat bridge'
status: closed
priority: 0
issue_type: bug
created_at: 2026-10-29T12:16:15.148946519+00:00
updated_at: 2026-10-29T15:54:36.772367075+00:00
closed_at: 2026-10-29T15:54:36.772366965+00:00
---

# Description

[opus 5.5] Owner (2026-10-29): "A goal should never block idle detection. All harnesses
continue receiving messages even when they have a longer-term goal set ... I want a proper low
latency bridge working with Muse."

A Muse coordinator inside a long /goal turn saw 63-196 s to delivery and 142-220 s to first
reply through `agentctl chat run --deliver-while-working`, and some requests never reached it.
Reproduced on a private test bridge (Muse 1.4.15, xhigh, YOLO, /goal). Causes:

1. Muse status misread as idle while a turn runs. `muse_editor.rs` accepted a progress row
   only as the last transcript row and only with the `◇`/`◈` glyphs, but the row animates
   through `◆` too, and a foreground command draws `└ (ctrl+b to send to background)` below
   it. A busy pane read "idle" on most frames, so the bridge took the idle path, whose 5 s
   working-timeout proof expired before Muse drew the steered turn: delivery_uncertain though
   the agent had the message. The post-turn "Double checking" phase (60-120 s at xhigh, Herdr
   already reports done) flickered between idle and working in `agentctl status`.
2. Muse sometimes refuses a steer mid tool call (`! steer failed: prior session tool history has
   tool call ... without a matching result`) and puts the prompt back in its editor as
   `[Pasted Content N chars]`. agentctl left it there, so every later request was refused as a
   draft until the goal turn ended (hours).
3. Muse can draw a steered user turn many seconds late (after the running tool), so the screen
   proof timed out; Muse's journal records `runtime.user_intent.accepted` with the exact prompt
   at once.

Fix (this issue): one running-turn rule for status and follow-ups (all three glyphs, a window
of rows); every ordinary prompt to a Muse composer goes through the steer path; the journal's
accepted intent proves a Muse submission; a prompt Muse refused is taken back out of the editor
and retried. Before/after numbers: the harness study report (2026-10-29-muse-bridge-study).
