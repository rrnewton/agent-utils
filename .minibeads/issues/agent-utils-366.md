---
title: 'public-internal-names: a vibe-talk example, its test and the README name an internal command-line client and its runtime'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- validation
created_at: 2026-10-19T09:35:55.905636075+00:00
updated_at: 2026-10-19T09:43:32.721312461+00:00
closed_at: 2026-10-19T09:43:32.721312140+00:00
---

# Description

[opus 5.5] vibe-talk's live-hint systemd example (`vibe-talk/systemd/vibe-talk-live-hint.service.example`), the test that pins it (`vibe-talk/tests/slack_deployment.rs`) and two README passages (the Slack limits section and the live-hint service section) name one company's internal command-line client, its subcommands and flags, its managed runtime, and its development hosts. A public repository must not name internal tools, hosts or companies; the adapter itself is already generic (any command that prints one JSON object per event).

Fix (forward only; history is not rewritten): describe the event source as "a vendor CLI tail command" and its runtime as "a managed runtime" in the example, the test and the README, keeping the resource and sandbox guidance and the reason for it. Extend `scripts/check_client_names.py` with a rule that would have caught these forms, and clean every other file that rule finds.

Acceptance: `make check-client-names` passes with the new rule, its self-test proves the rule fires on the forms that leaked and not on ordinary uses of the same word (a keyboard key, a page `<meta>` element), and the systemd example still pins its runtime constraints.
