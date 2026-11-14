---
title: 'muse-headless-parity: does Muse have a headless mode on par with Claude headless'
status: closed
priority: 2
issue_type: task
created_at: 2026-11-14T18:31:00.000000000+00:00
updated_at: 2026-11-14T21:20:00.000000000+00:00
---

# Description

Owner 069 asks whether Muse has a headless mode on par with agentctl's
Claude headless mode (`run-headless`: no TUI, prompts via a queue,
transcript/tail, permission prompts routed to the owner in chat,
restart resumes the session), based on the Muse CLI's own help/docs
and agentctl's support for it.

# Design

Research only: read `muse --help` and subcommand help, the local Muse
configuration, and grep agentctl (both editions) for headless adapters.
Write the answer with evidence and a plan (what agentctl would need,
risks, how to trial it on the liaison without switching it) in
`ai_docs/reports/research/2026-11-14-muse-headless-parity.md`.

# Acceptance Criteria

- The report names the Muse modes found, with the exact help output or
  file evidence for each claim.
- The report compares them against the Claude headless contract point
  by point and lists what agentctl would need to build.
- A trial plan that switches nothing until the owner approves.
- A CHANGELOG bullet in the harness repository.

# Notes

Nothing switches until the owner approves. This issue is the separate
deliverable from #1546 agentctl-unsubmitted-paste-backstop.

## Resolution

Answered with a research report and trial plan in the coordinator
harness (muse exec/serve exist; agentctl headless is Claude-only;
parity needs a Muse driver on muse serve plus the approval relay). The
plan switches nothing; building it waits for the owner's approval and
gets its own issue then.
