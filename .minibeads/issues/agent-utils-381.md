---
title: 'agent-watch: tell the coordinator when an agent is stuck at a prompt, idle without a verdict, stalled, or dead'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
created_at: 2026-10-19T21:53:26.640782828+00:00
updated_at: 2026-10-19T21:53:26.640782828+00:00
---

# Description

[muse watermelon] On 2026-10-19 three Muse sat frozen at a harness approval prompt for 6 h, 2 h and 1.5 h before anyone noticed, and were stuck again the same afternoon. The coordinator only notices an idle or stuck agent when something prompts it to sweep. agentctl has no watcher that tells the coordinator when an agent needs attention.

Add a long-running watcher (Rust edition subcommand agentctl watch-agents) that polls every registered agent in a registry and sends one message to a named coordinator agent through the same durable queue as agentctl send when an agent: is at a harness approval or permission prompt for more than N minutes; went idle or done without its last reply saying DONE: or BLOCKED: (configurable markers) for more than N minutes; shows no new output for more than M minutes while marked working (stalled); or died (harness process gone).

Rules: one notice per episode, not one per poll; rate-limited; it never answers prompts itself; it never types into any agent except the coordinator; the coordinator being busy must not lose notices (they stay queued). Thresholds and the coordinator name are flags with documented defaults. Include CLI help, a user guide section, and a sample systemd user unit in the docs (do not install it). Tests must include negative controls: an agent that is working and producing output is never reported, and one notice per episode.

# Acceptance Criteria

agentctl watch-agents runs long-lived and with --once; a stuck-at-prompt, idle-without-verdict, stalled, or dead fixture agent produces exactly one queued coordinator notice per episode; a working agent with changing output produces none; help and user guide document every flag, default, and a sample systemd user unit.
