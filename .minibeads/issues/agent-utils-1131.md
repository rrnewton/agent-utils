---
title: 'restore-resume-brief: restore delivers a standard reboot resume brief to every revived lane'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-09T13:45:45.707238082+00:00
updated_at: 2026-11-09T13:45:50.279662856+00:00
---

# Description

Found during the 2026-11-09 host reboot. After the lanes were revived, the coordinator sent each one a hand-written host-event message: the host rebooted at a stated time, slot rebind is done, check the slot against the last known tip, restart only the interrupted step, and never delete pre-boot gate tickets by hand. Without that message a revived lane cannot tell a reboot from an ordinary stall, and may either redo finished work or clean up state that a reaper owns.

Today restore queues one recovery notice for the coordinator and a catch-up summary, but there is no standard per-lane resume brief a team can template once and have delivered to every revived lane. Each reboot therefore depends on the coordinator remembering the wording and the recipient list.

Impact: lanes come back without the facts they need to resume safely, and the coordinator's first post-reboot hour is message composition instead of supervision.

# Design

Add an optional resume-brief template to the restore configuration (a restore.json key naming template text or a template file beside it). After a successful revive, restore renders the template once per revived or promoted lane and queues it through the same durable delivery path as an ordinary agent message, with a per-boot message identity so a rerun never sends it twice.

Template variables are limited to facts restore knows: boot identifier and kernel boot time, restore finish time, agent name, coordinator name, slot name and transfer outcome, and the report path. The default template states the host rebooted, that slot rebind is done (or names its refusal), to check the slot status and tip, to restart only the interrupted step, and to leave pre-boot admission tickets to their reaper. A team may replace the text; a registry with no template keeps today's behaviour.

Delivery is recorded per agent in the restore report (queued, already-queued, or refused with reason). A brief that cannot be queued does not fail the revive, but it is named in the report and the host table; the coordinator notice lists which lanes did not get a brief. Skipped, excluded, and refused lanes receive nothing.

# Acceptance Criteria

- After a simulated reboot restore, each revived lane has exactly one queued resume brief naming the boot time, its slot outcome, and the report path; the coordinator notice and the report agree on who was briefed.
- An immediate same-boot rerun queues no second brief (already-queued by message identity) and the report says so.
- A lane whose slot transfer was refused gets a brief that names the refusal instead of claiming rebind is done.
- Skipped, excluded, and refused lanes get no brief; a registry without a template restores exactly as today.
- A custom template with an unknown variable is refused at configuration load, naming the variable, rather than delivered half-rendered.

# Notes

Scope gap (c) from the 2026-11-09 reboot restore. The hand-sent wording is the coordinator's host-event message from that morning (reboot time, rebind done, check the slot, restart only the interrupted step, leave pre-boot tickets alone); this issue turns it into a delivered template. Builds on the existing coordinator notice and catch-up delivery; it does not replace either.
