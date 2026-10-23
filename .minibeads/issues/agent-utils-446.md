---
title: 'agentctl-answer-draft: safely replace an inspected unsent free-text answer'
status: closed
priority: 2
issue_type: bug
created_at: 2026-10-23T10:42:06.999816384+00:00
updated_at: 2026-10-23T11:45:46.034133162+00:00
closed_at: 2026-10-23T11:45:46.034132508+00:00
---

# Description

Delivered on main at c823ee368c4d4c416eb8048d48eb293a2026a990; actual HTTPS remote and tracking head verified. Explicit --text TEXT --replace-draft supports one recognized Codex free-text question. It checks the same recipient/question, clears once, verifies an empty unchanged form, requires the literal replacement visible, and submits once. Default answering preserves different drafts. Unsupported forms, changed identity/question, failed clear, folded paste and uncertain dismissal stop safely. prompts remains inspection-only.

Verified 150 focused submission tests and all 32 selected validation steps: 2543 Python passes plus four existing expected failures, 1350 agentctl Rust cases, 760 agentctl and 103 herdr-agent paired cross-language checks, strict typing, packaging and repository hygiene. A real managed question was successfully recovered after inspecting an unsent folded draft; the recipient acknowledged the short interface-file reply.

Host qualification: the passing run used official user-space legacy timezone data and the canonical graph with exactly one environment prefix placing the heavy Rust test fixtures in executable RAM. Test selection, assertions, timeouts, dependencies and resource hints were unchanged. Default host provisioning still needs a portable setup fix; track that separately rather than leaving this delivered feature open.
