---
title: 'misroute-shared-text: a delivered prompt is quarantined when the same text shows in another pane'
status: open
priority: 0
issue_type: bug
created_at: 2026-10-30T00:50:50.044381340+00:00
updated_at: 2026-10-30T00:50:50.044381340+00:00
---

# Description

Found by the Herdr send-rate stress test on 2026-10-29 (report in the harness repo,
ai_docs/2026-10-29-herdr-send-rate.md).

agentctl's read-back quarantined prompts that had in fact been delivered: exit 76, "PROBABLE
MISROUTE ... cannot be attributed". It happened whenever the same text was visible in another
registered agent's pane:
- the same setup message sent to three agents a second apart: one copy was quarantined;
- five long messages sharing filler lines that another agent had received earlier: all five
  were quarantined.

Each harness's own session transcript shows every one of these prompts arrived exactly once.
The caller saw a failure for a delivered message, and a retry would have delivered it twice.

The read-back quarantined whenever any other pane was not `absent`: newly showing the text, or
already showing it with an unchanged count. It did this even when the target newly showed the
prompt.

## Fix

One paste lands in one pane. A target that printed the prompt (a printed submission receipt),
or newly shows it where typed input lands (its composer or a user turn, not a tool's output),
received it. That holds whatever other panes show: the same words sent there too, as in a
broadcast, or still on screen from earlier. Such a send is delivered and logged as
`attributed`. Without that target evidence the old rules stand. This applies to both editions.

This also closes Gap 3 of readback-gaps: an equal count in a peer that already showed the prompt
quarantined it.
