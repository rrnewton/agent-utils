---
title: 'mq-submit-approval-gate: gate CLI submit on an approval command'
status: closed
priority: 0
issue_type: bug
created_at: 2026-11-12T16:54:01.477933042+00:00
updated_at: 2026-11-12T21:21:53.200274195+00:00
closed_at: 2026-11-12T21:21:53.200273925+00:00
---

# Description

A change can be submitted from the command line with no review verdict and no submitter validation claim, and the queue accepts it. The approval rule that exists (require_approval_at_head) gates only the hosting-service label-sync path (an approving review at the head, read in the batched list call); the direct submit path checks neither an allowlist nor a claim. The only submit-time hook, the precheck command, cannot do this job: it never learns who submitted, its pass/fail verdict is cached per head and command (so a head refused before its approval arrives stays refused), and its could-not-run exits queue the change anyway. Observed in a 2026-11-12 queue drain: a change was submitted directly by a lane with claim null and no review, passed the precheck, and queued; it was withdrawn by hand once noticed. These are cooperative guards (every lane shares one Unix user), not a security boundary; the #1362 agent-utils-merge-queue-dogfood plan makes the same point for its pushed-ref submit path (verify the ref, the SHA, and the sender) and relies on the queue, not the submitter, to enforce readiness.

# Design

In rs/merge-queue: add a submit approval gate that runs on every direct submission (ops::submit_resolved, when the submission has no sync source; sync submissions keep the existing approval-at-head rule and are exempt). Config: a new submit section with approval_command (shell command; empty means no gate, today's behaviour) and an approval timeout. The command runs fresh on every submit, never cached per head, in the repository directory with MQ_PR, MQ_HEAD and MQ_BY set (the resolved head commit and the submitter as given to submit --by). Exit 0 approves. Any other exit refuses the submission before anything is queued, and the refusal names the reason (the command's first non-empty output line, or its exit status). A command that cannot run at all (cannot start, exits 126/127, or times out) also refuses, naming why: a gate that cannot run must not silently queue, unlike the precheck. The refusal is returned as the submit error, so the submitter hears it at once; nothing is saved as queued or ejected. Document the gate in the CLI help for submit and in the embedded user guide, including that it is a cooperative guard.

# Acceptance Criteria

With submit.approval_command set to a refusing command, a direct submit is refused with the command's reason, queues nothing, and a later submit after the command approves succeeds at the same head (no cached refusal). With an approving command, a direct submit queues as today and the approval is recorded in the submission history. With no approval_command configured, submits behave exactly as today. A sync-source submission is unaffected by the gate. A gate command that cannot run refuses with a reason naming that, and queues nothing. cargo test -p merge-queue (all targets) passes, including the new cases.

# Notes

Evidence: a 2026-11-12 drain report relayed to this lane (the unstamped submission's persisted item record: submitter set, claim null, precheck passed). The report offered two shapes (an approval command, or a required claim plus an allowed-submitters list); this issue takes the approval command because it can express either rule and names its own refusal reason. The #1362 agent-utils-merge-queue-dogfood plan's pushed-ref submit design (verify ref, SHA and sender at the queue) is the same cooperative-guard stance for another submit channel. Fix first with #1379 mq-unknown-probe-singleton (safety/progress pair).
