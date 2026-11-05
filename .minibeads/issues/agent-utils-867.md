---
title: 'dagrun-runtime-skip: report steps SKIPPED with a named reason when a host capability is truly unavailable'
status: open
priority: 3
issue_type: feature
assignee: mu-validate
created_at: 2026-11-05T04:40:00.000000000+00:00
updated_at: 2026-11-05T04:40:00.000000000+00:00
---

# Description

dagrun has no runtime conditional skip. `skip_reason` (`IntentionalSkipReason`) is a closed, load-time enum with one value (`empty-manifest-bucket`): a step is either always skipped when the graph is authored, or it runs. When a setup step fails, its dependents are dependency-skipped, but the setup step itself FAILs and the gate goes red.

#865 gate-node-provision hit this directly. The vibe-talk page and contract steps now provision Node 22 first (`setup.node22`), so on a host whose system node is v16 they run under the pinned toolchain. But if provisioning is truly impossible (offline, no cached archive, no nvm Node 22), the desired outcome is those steps SKIPPED with a named, visible reason (for example `host node v16 < 20; Node 22 not provisioned`), never FAILED, and never reported as passed. With (C), landed for #865, a provisioning failure stays a FAIL on `setup.node22` alone, whose output now names the reason. This issue is the follow-up that makes the skip real.

# Design

Option (B), chosen in the #865 discussion:

- A `scripts/validate.py` preflight attempts the capability first (for the Node 22 case: `scripts/provision-node22`, which is flock-guarded, sha256-verified, and cached per user, so a preflight attempt is cheap when it has already succeeded). Provisioning always comes first, so the tests still run on hosts where it can succeed.
- Only when the preflight proves the capability unavailable does validate.py pass the affected step tags to dagrun as skipped-with-reason, through a small dagrun CLI flag (for example `--skip TAG=REASON`, repeatable). dagrun marks those steps SKIPPED with the given reason in its journal and summary, does not spawn them, dependency-skips their dependents, and does not count them as passed. The overall verdict must distinguish a run with runtime skips from a clean pass (as PARTIAL-style reporting already does for cross coverage), so a gate never reports skipped steps as green.
- Implement the flag in both dagrun editions (Python `py/dagrun` and Rust `rs/dagrun`) with identical CLI surface and journal records, following the #860 priority-hints pattern: model/CLI changes in both, differential coverage where the cross harness exercises dagrun, and the user guide, rendered guides, help text, and the dagrun skill updated in the same change.
- The reason string is supplied by the caller (validate.py composes it from the observed host state, for example the system node version and the provisioning failure), not hard-coded in dagrun.

# Acceptance Criteria

- A step passed as skipped-with-reason is reported SKIPPED with that exact reason in the dagrun summary and journal; it is not spawned, and it is never counted as passed.
- Dependents of a runtime-skipped step are dependency-skipped, not failed.
- A run containing runtime skips does not report a plain clean pass; the skip and its reason are visible in the final verdict output.
- With the capability available, no step is skipped: the preflight path runs the same steps as today.
- Both editions accept the flag with the same spelling and semantics; unknown tags and empty reasons are refused before any step runs.
- Tests cover: skipped step not spawned, dependents dependency-skipped, verdict not a clean pass, bad tag/reason refused, and the all-available path unchanged.

# Notes

Filed from #865 gate-node-provision (coordinator decision (C), 2026-11-05): land the with-node22 wrapping now, keep the provisioning failure as a named FAIL on `setup.node22`, and track the runtime skip here. #866 is latency-tests-cpu-budget, so this takes #867. Queue position: after #864 wrkslots-unlisted-module-salvage. Assigned to mu-validate.
