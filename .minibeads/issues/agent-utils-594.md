---
title: 'headless-slack-trial: bridge a chat channel to a headless coordinator'
status: open
priority: 2
issue_type: feature
depends_on:
  agent-utils-565: parent-child
created_at: 2026-10-27T20:45:01.419898305+00:00
updated_at: 2026-10-27T20:45:01.419898305+00:00
---

# Description

[opus 5.5] Owner 2026-10-27: create a second chat channel and bridge it to the headless Claude coordinator (#565 headless-coordinator) for testing. Main had no bridge path to a headless agent: chat init and chat run read the coordinator's pane, which a headless record does not have (design note section 8, M3). This issue delivers a minimal M3 slice: chat run detects a headless target and runs a pane-free owner loop that enqueues requests through the existing headless send (urgent/now with --deliver-while-working), wakes the wrapper instead of draining, reads queue state for delivery, and gives prompts that reply only through agentctl chat reply. Reply reminders and the stale-composer guard are off for a headless target. Then deploy and measure end to end.

# Acceptance Criteria

chat init/run accept a headless target; a test message in the new channel is answered by the headless coordinator through chat reply; latencies (post, admitted, started, reply posted) recorded.
