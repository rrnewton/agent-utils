---
title: 'agentctl-asks-never-block: a permission or guardrail ask in any managed agent becomes a fast, logged denial instead of a wait for a person'
status: open
priority: 0
issue_type: bug
labels:
- agentctl
created_at: 2026-11-27T19:20:00-05:00
updated_at: 2026-11-27T19:20:00-05:00
---

# Description

[opus 5.5] Owner 2026-11-27 7:06 PM ET: "routine problem -- deny or
approve I don't care. The problem is that it asks at all ... the
worst outcome is locking up our agents which is what happens with
this question ... just deny is better than freeze forever."

Every managed agent runs on its harness's permission bypass (#1822),
yet some asks still stop an agent until a person acts:

- A headless Claude agent's permission prompts (a hook's or
  classifier's ask, a built-in safety check such as a dangerous `rm`
  on a variable path) are held for the owner for 600 s by default,
  then denied (#671 headless-approval-surface). Only the bridge's own
  target agent's prompts reach chat at all, so most of them simply
  wait out the timeout. The only deny-at-once option is the
  `run-headless --no-approvals` flag, which no profile or registry
  can set and which logs no reason.
- An interactive Claude Code lane at a hook confirmation, a built-in
  safety check or a permission-rule confirmation waits until a person
  answers in its pane. The watcher reports it (#675, #1784) and
  alarms the owner for hook confirmations, but nothing answers it, so
  a lane can sit there for hours.
- An interactive Muse lane at its peer-approval dialog or its command
  approval form waits the same way (#974: detection only).

# Design

- Deny, never approve. No automatic path ever selects an option that
  lets the call or message through; an ask nobody answers within a
  short bound is refused, the agent reads why, and it carries on.
  This supersedes the "never answered" rule of #1784 and #974 for the
  deny direction only: approving still needs a person.
- Headless: `approval_timeout_s` per profile, then a registry default
  in profiles.json, then the built-in default. 0 means deny at once:
  the wrapper still logs `approval.requested` and
  `approval.decided` (by `policy`), waits only a short grace for the
  host's own permission hooks to settle the prompt, then denies with a
  message naming why the harness asked and telling the model to use a
  way that needs no approval or report what it needs. 10 to 86400
  keeps the owner surface of #671. `run-headless --approval-timeout`
  wins when given. The built-in default becomes 0.
- Interactive: the watcher (full pass and quick check) recognises a
  permission prompt it can classify per harness (Claude Code hook
  confirmations, safety-check and permission-rule confirmations; Muse
  peer approvals and command approvals) and, once the same prompt has
  shown for `permission_deny_after_s` (per profile, then registry,
  default 60), answers its deny option through a verified keypress
  flow that re-reads the pane before every key, refuses to press the
  submit key unless the selected option is a deny label, and checks
  the prompt left. It then queues the agent a note saying what was
  denied and that nothing ran, and gives the agent's manager one line.
  `permission_prompts: "report"` keeps today's report-only behaviour
  for a profile or registry. A prompt it cannot classify is reported,
  never answered. Codex runs with approvals off and has no recorded
  approval screen, so it stays report-only.
- The harness support matrix gains the row in the same commit, and
  the watcher's one new persisted field is reported before gating.
- Configuration that turns asks into denials before agentctl has to
  act (a Claude Code settings file with a permission-request hook that
  denies, deny rules, approval policies) is proposed to the
  coordinator, not written into any registry by this change.

# Acceptance Criteria

- A headless prompt under `approval_timeout_s: 0` is denied within a
  few seconds with the reason in the model's message and in the event
  log, and the chat bridge posts nothing for it.
- Hermetic tests with recorded screens and fake panes: each
  recognised interactive prompt is denied after the bound and never
  before it; no test path ever sends the key sequence for an approve
  option; a changed or unrecognised prompt is reported with no key
  sent; `permission_prompts: "report"` sends nothing.
- The harness support matrix is updated in the same commit.
- Focused Rust suites pass on the gated tree.

# Notes

Filed by the lane before coding. The coordinator's own headless
approvals are already denied at once by a harness relay outside this
repository; this issue makes that the tool's behaviour for every
agent.
