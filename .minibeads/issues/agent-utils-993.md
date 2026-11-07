---
title: 'wrkslots-unattributed-pane-scope: one unregistered agent pane anywhere vetoes every recordless agent-slot removal'
status: open
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-07T02:05:00.000000000+00:00
updated_at: 2026-11-07T02:05:00.000000000+00:00
---

# Description

Removing an agent slot whose agent has no live registry record refuses with "registered liveness authority is unverifiable ... 1 live herdr pane(s) run an agent-capable process, but no record in [the registries] claims them: <pane> (bash, codex ...) ... Any of them could be <name>".

On 2026-11-06 a consumer's removals of three such slots were all refused because of ONE pane: the owner's own interactive Codex session, started by hand in a repository root, in no slot and referencing none of the target slots. So any unregistered agent-capable pane anywhere on the host vetoes every recordless agent-slot removal. Neither `--coordinator-authorized` nor `--attest-agent-stopped` gets past it today; the only way through is the owner's override.

# Design

- Scope the unattributed-pane check to panes whose processes reference the target slot: a cwd inside the slot, an open fd under it, or the slot path or name in cmdline or environ. A pane that references nothing of the slot is not evidence about that slot's agent.
- Optionally, an explicit exclusion naming panes known to be human sessions (e.g. `--ignore-pane <id>`, recorded in the journal with who passed it).
- Keep the refusal when an unattributed pane does reference the slot, and say which reference matched.
- Both editions.

# Acceptance Criteria

- A test: an unregistered agent-capable pane whose cwd, fds, cmdline and environ never touch the target slot does not block removal of a recordless agent slot.
- A test: an unregistered pane whose cwd is inside the target slot still refuses, naming the matching reference.
- Both editions agree (differential or paired tests).

# Notes

Reported 2026-11-06 by the lead coordinator from a consumer's cleanup lane (full refusal texts kept on the lead's host). Related: #954, #955, #956 (other remove refusals), #994.
