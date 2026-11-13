---
title: 'mq-notice-registry-from-cwd: a submit notice addresses the project registry, not the caller''s'
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-13T00:05:12.428480586+00:00
updated_at: 2026-11-13T00:07:40.428366931+00:00
closed_at: 2026-11-13T00:07:40.428366800+00:00
---

# Description

A merge-queue submit CLI sends its precheck-refusal notice through agentctl, whose default registry is .agentctl in the caller's working directory. Run from a worktree slot (the normal place to submit from), the send fails with exit 75 (cannot inspect a registry under the slot) and the refusal notice never reaches the change's lane. The serve daemon's notices are unaffected because the daemon starts in the project root. Observed 2026-11-12: a submit from a slot against a queue whose daemon notices work fine; the notice failed attempt 1 of 3 with the slot's missing registry. A registry can already be configured (init --agentctl-registry, stored as notify.registry, resolved by notify::resolve_registry for daemon sweeps), but nothing records it by default and the submit path (notify::deliver_now) has no fallback when none is configured.

# Design

In rs/merge-queue: (1) init records a registry by default: when --agentctl-registry is not given, the nearest .agentctl directory above the init working directory is stored absolute as notify.registry (new notify::registry_above helper). (2) notify::deliver_now (the submit path) falls back, when no registry is configured, to the nearest .agentctl above the queue's repository, then above its state directory, and passes it explicitly. The daemon sweep path is unchanged. Update the --agentctl-registry help, the NotifyConfig.registry doc, and the user guide's registry row.

# Acceptance Criteria

A submit from a working directory outside the project whose head fails the precheck sends its refusal notice with --registry naming the project registry (the nearest .agentctl above the init directory), proven by a CLI test with a recording fake agentctl. init with an explicit --agentctl-registry stores exactly that path, as before. A queue with no .agentctl above init, repository or state keeps agentctl's own default. cargo test -p merge-queue (all targets) passes, including the new cases.

# Notes

Evidence: a 2026-11-12 submit-notice report relayed to this lane (exact exit-75 text in the report; host and queue paths stripped here). Reserved by the coordinator after the five drain defects (#1377-#1381); normal priority.
