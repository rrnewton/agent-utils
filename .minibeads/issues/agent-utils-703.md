---
title: 'send-lock-free-wait: send and drain hold no lock while waiting for the pane to become ready'
status: closed
priority: 0
issue_type: bug
created_at: 2026-10-29T22:31:19.062065253+00:00
updated_at: 2026-10-29T23:19:58.641046337+00:00
closed_at: 2026-10-29T23:19:58.641046197+00:00
---

# Description

Follow-up to #702 send-hard-timeout, agreed with the coordinator on 2026-10-29.

A managed `agentctl send` or `agentctl drain` held the agent's lifecycle lock, the queue
delivery lock and the host-wide pane lock for its whole readiness wait (up to
`--ready-timeout`, default 900 s, now cut to the hard deadline). Anyone else needing the agent
was blocked meanwhile: the chat bridge, `stop`, and every other sender.

## Wanted

Never hold a lock while waiting for the pane to become ready. A send queues its prompt under
the locks and probes readiness once. If the pane is not ready it releases every lock, waits
without any lock, then takes the locks again and probes. It repeats until the prompt is typed,
set aside or quarantined, or until its readiness budget runs out (the smaller of
`--ready-timeout` and the hard deadline). Then it exits 75 with the prompt pending. A drain
does the same for its queue. Each probe reuses the drain's own readiness checks, so every
harness keeps its rules.
