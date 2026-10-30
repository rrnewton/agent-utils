---
title: 'oversized-prompt: send refuses a prompt too large to type and warns about long ones'
status: closed
priority: 1
issue_type: feature
created_at: 2026-10-30T00:54:30.108490524+00:00
updated_at: 2026-10-30T00:54:30.113162762+00:00
closed_at: 2026-10-30T00:54:30.113162292+00:00
---

# Description

Found by the Herdr send-rate stress test on 2026-10-29 (report in the harness repo,
ai_docs/2026-10-29-herdr-send-rate.md).

agentctl types a prompt for an interactive agent through one `herdr pane send-text` (or
`agent prompt`) call. The system refuses a single argument over 128 KiB (`MAX_ARG_STRLEN`): a
140 KB call failed with E2BIG and sent nothing. Pastes over about 50 KB already show only as a
collapsed placeholder in every harness: Claude Code "[Pasted text #N +M lines]", Muse and Codex
"[Pasted Content N chars]". Muse also caps long pastes in its transcript.

## Fix (both editions)

- `agentctl send` to an interactive agent refuses a prompt over 120 KiB (122,880 bytes,
  measured after the sender header) with exit 2 and a clear message, and queues nothing.
- Over 16 KiB it warns on standard error and suggests writing the content to a file and sending
  a short prompt that names it.
- Headless and agentcloud agents are unaffected: they take prompts through stdin or a journal.
