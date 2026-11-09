---
title: 'owner-alarm-says-why: the chat delivery alarm names the blocked prompt and keeps the warning after resolution'
status: open
priority: 1
issue_type: feature
assignee: mu-inbox
labels:
- agentctl
created_at: 2026-11-09T17:14:39.075325205+00:00
updated_at: 2026-11-09T17:14:39.075325205+00:00
---

# Description

Owner reply 254 (2026-11-09 12:05 PM ET), via the lead coordinator, after a chat-driven coordinator sat blocked at an in-TUI question on 2026-11-09: the owner got no visible chat warning and noticed only from the lack of response; the sole chat trace was "[chat-bridge] Resolved 11:41 ET: your messages to <coordinator> are being delivered again". The chat owner/delivery alarm (#830/#981 coalescing) posts a first warning top-level, rewrites repeats in place, and on recovery rewrites the same post to the short Resolved line. An in-place edit does not re-notify, and the rewrite erases the warning - so the one notification the owner ever saw said everything was fine.

Fix in the chat alarm: (1) the warning must say WHY delivery is stuck, using the watcher's blocked-prompt detection (#675 watch-blocked-fast-alerts), for example "<agent> is waiting at a question or permission prompt in its TUI" - covering question, trust, permission, and Codex approval prompts, not one harness's question tool only; (2) do not erase the evidence: keep the original warning text and append the resolution with its time, or post the resolution as a thread reply, so the owner can see what happened and how long it lasted. Sibling routing rule (a blocked-coordinator notice goes to the owner's chat, never only to the blocked coordinator's own queue) lives in coordinator-never-blocks; this issue owns the alarm text and rewrite behaviour. Whether the 2026-11-09 warning was in fact posted and rewritten is being established from bridge logs; record the timeline in the resolution.

# Acceptance Criteria

- Alarm warning text names the stuck agent and the detected reason (blocked at a question/permission/approval prompt) whenever blocked-prompt detection knows it, and degrades to the current wording when it does not.
- Recovery no longer erases the warning: the resolved post/thread still shows the original warning, its start time, and the resolution time; tests cover post -> rewrite(s) -> resolved sequence and assert the warning survives.
- Coalescing for repeats is otherwise preserved (no re-notification spam per repeat). Full gate passes.
