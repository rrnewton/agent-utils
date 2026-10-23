---
title: 'agentctl-answer-draft: safely replace an inspected unsent free-text answer'
status: open
priority: 2
issue_type: bug
created_at: 2026-10-23T10:42:06.999816384+00:00
updated_at: 2026-10-23T10:42:06.999816384+00:00
---

# Description

Explicit recovery is needed when a supported Codex single free-text question contains an inspected unsent draft, including a folded paste marker. Default answer must preserve existing drafts and refuse mismatches. Add --replace-draft that validates the replacement before effects, verifies the same question and recipient, clears once, verifies an empty same form, and retains exact literal paste verification and at-most-one submission. Never infer literal content from marker length. Unsupported forms and question/identity changes refuse. Add focused failure-path tests and CLI help.
