---
title: 'headless-second-reaction: the second reaction marks when a headless agent admits a chat request'
status: closed
priority: 2
issue_type: feature
depends_on:
  agent-utils-565: parent-child
created_at: 2026-10-28T01:13:15.677767441+00:00
updated_at: 2026-10-28T02:17:59.703812106+00:00
closed_at: 2026-10-28T02:17:59.703811946+00:00
---

# Description

[opus 5.5] Owner 2026-10-27 18:06: on the headless channel only the first reaction (the bridge has the message) appears. The second stage should stay and mean that the agent admitted the message as input to the LLM: for a TUI, appearing in the transcript; for a headless agent, which may be mid-turn on other work, the moment the wrapper records that the model started that input. #594 headless-slack-trial deliberately gave headless requests no receipt reaction. Add it: the bridge adds the receipt exactly when the queue item moves to processed (model started it), never on enqueue; mid-turn (urgent) and turn-boundary delivery both count once started; an input set aside untyped (cancelled or expired) earns none; a restart reconciles. chat status shows per-request stages (admitted / queued / started / replied) for headless targets too.

# Acceptance Criteria

Tests with a fake headless wrapper and fake reaction transport: queued while a turn runs gets the receipt only after started; a bridge restart in between still adds it once; a cancelled input gets none. chat status lists the stage of each request. Landed through the gate with a release agentctl built.
