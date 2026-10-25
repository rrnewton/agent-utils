---
title: 'agentctl-standalone-doc: describe recovery stamps without a foreign package term'
status: closed
priority: 2
issue_type: chore
created_at: 2026-10-25T22:03:42.386270497+00:00
updated_at: 2026-10-25T22:13:05.746640526+00:00
closed_at: 2026-10-25T22:06:37.844362316+00:00
---

# Description

[GPT-6 Codex codex-vibe-discord] The selected package-contract check rejected a foreign-package term in current-main recovery-stamp rustdoc. A local comment-only correction passed all 30 packaging tests. Before publication another agent independently landed aa884118 with the same correction; retained that upstream wording when rebasing. No local executable change is needed for this issue.
