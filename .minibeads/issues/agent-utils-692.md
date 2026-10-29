---
title: 'muse-bridge-transcript-status: derive Muse busy/idle from its session journal'
status: open
priority: 2
issue_type: feature
created_at: 2026-10-29T15:54:23.502717375+00:00
updated_at: 2026-10-29T15:54:23.502717375+00:00
---

# Description

[opus 5.5] From #679 muse-bridge-study. Muse's session journal (`.../muse/sessions/YYYY/MM/DD/
<session>/session.jsonl`, found through the `.session.lock` its runtime holds) records turn and
input events: `runtime.user_intent.accepted` with the exact prompt text, `inbox_item_queued` /
`inbox_item_drained` for steers, tool batch start/terminal, model requests. #678 uses the first
as a delivery proof; #574 reads the journal for watch-agents activity.

Proposal: derive Muse's busy/idle status from the journal (an open turn or tool batch, a model
request in flight), with the screen as a fallback, as Claude Code's and Codex's transcripts could
be. That would make status independent of glyph animation, redraw timing and wrapped rows.
Needs: a bounded tail reader shared with JournalWatch, and tests against captured journals.
