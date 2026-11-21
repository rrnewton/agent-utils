---
title: 'restore-recovery-agent-plan: manifest + team act, then an always-on one-shot recovery shepherd'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
created_at: 2026-11-20T22:08:11+00:00
updated_at: 2026-11-21T13:12:57+00:00
---

# Description

[watermelon-full-preview] Owner 081 asked for recovery with less fragile deterministic code and more agent judgement; the lead-approved design is the harness living plan `ai_docs/plans/2026-11-15-restore-recovery-agent-plan.md` (harness repo), building on `ai_docs/plans/2026-11-12-team-restore-protocol.md`. Owner corrections on 2026-11-20 reshaped the agent half: 234 (recovery watches from the start), 235 (one-shot calls in a loop, muse first then claude then codex, "emergency recovery mode"), 242-244 (coordb's harness-local shepherd-v0; the shepherd always starts at boot from its own minimal unit, in parallel with restore, monitoring it), 245 (lane restore is both jobs idempotently; lanes may return before the lead; the coordinator sweeps), 243 (per-team YES = lead running + bridge healthy + handoff acked), 256 (restore stays outside Herdr; a `system > recovery` tab tails the recovery log), 257 (report records missing profile/bypass/effort/session; never rewrite them). Phase plan approved by coordb 2026-11-20: judgement-only one-shots, effects only via `team act`; Rust `team shepherd` loop + standalone wrapper script; no bind/record-repair in boot recovery; bounds 300 s per call, 20 rounds / 120 min, early escalation on two identical no-progress snapshots (configurable).

# Design

- Phase 1 (deterministic half, no agent yet): freeze `agentctl-team-manifest/v1` as a redacted allowlist projection of the status rows DETECT already computes — never raw records (record tokens must not reach an agent brief) — with `record_gaps` (missing profile/bypass/effort/session, ambiguous candidates with first-prompt evidence), queued census, unit booleans, refusals, `lost[]`, `errors[]`, and the embedded `agentctl-team-verify/v1` result. Then `agentctl team act --incident I --request-id R <action>`: validated against the manifest, idempotent per request-id, persisted in the driver record, verify-after, exits 0/75/1. Allowlist v1: facts, revive (recorded session only), start-unit/reset-unit, drain, kill-pid, bridge gap actions wrapping the landed pin-guarded gap verbs, declare-done (a full-VERIFY request), escalate. No `bind-session` and no record-repair writes anywhere in boot recovery (257); candidates are report-only. New persisted data (DriverRecord `manifest`/`actions[]`, shepherd stamp/state files) is reported to coordb before each landing because another team pins an older agentctl reader.
- Phase 2 (shepherd): a standalone `run-agent-prompt` wrapper tries `muse exec`, then `claude -p`, then `codex exec` (stateless, tool-less, hard process-group timeout, distinct all-fail exit 69, logs which harness answered). The shepherd loop starts unconditionally at boot from its own minimal unit in parallel with restore (no ordering on restore or Herdr), polls manifest/verify facts for every team in the restore-teams teams config, and per round accepts at most one own-team `team act` proposal from the one-shot's structured final JSON line. It finishes only on a deterministic YES per team — lead running, bridge healthy, handoff acked, each an explicit pass (held/skipped never count) — including other teams' leads judged read-only, or on bounded give-up with escalation + stamp. If deterministic checks pass with all harnesses down, the stamp still says YES with judge=none. The shepherd unit appears in `services list` and turnover like other agent-utils daemons; watch/turnover defer while an incident is non-HEALTHY. Visibility: per-boot recovery log under `~/.local/state/agentctl/shepherd/<boot-id>/`, best-effort `system > recovery` Herdr tab tailing it (display only).
- Split: #1975 (slot re-attach, sweep-notice wording, longer boot-restore timeout) and #2000 belong to mu-restoreboot; #1934/#1992/#1993 belong to mu-restorefix. This lane consumes their landed outputs and reports absent state; it does not duplicate them.

# Acceptance Criteria

- A human driving `team act` by hand recovers a scratch drill team end to end; duplicate request-id and stale incident are safe refusals/no-ops; held/skipped verify rows can never produce YES.
- Scratch-registry drill with a fake Herdr (`--herdr-bin`) and fake muse/claude/codex executables proves shepherd fallback order, timeout fall-through, deterministic YES/NO, cross-team read-only judging, and record-gap reporting — without a real reboot, restore, or live registry.
- Wrong-conversation safety: ambiguous or missing native sessions refuse with named candidates in both Rust and Python editions (where mu-restorefix has not already landed it); restore/recovery rewrite no records.
- Every liveness/restore/revive change gets a coordb cold review before gating; landings only on coordb LAND NOW; no real restore/reboot drill without coordb's explicit OK.

# Notes

Reserved 2026-11-15 with the plan; filed 2026-11-20 with landing 0 of the mu-recoveragent lane (coordb approved the phase plan the same day). Live drill evidence for the acceptance scenarios: shepherd-v0 stamp 2026-11-20 11:55 AM ET (`agent=muse`, unit success) and the harness report `ai_docs/reports/measurements/2026-11-20-reboot-drill-devbig014.md`.

Landing 1 (manifest + record-gap reporting) landed 2026-11-21 as `437a5262`: read-only `agentctl-team-manifest/v1` on one shared verify probe pass, raw-record gap judgement, bounded embedded verify, Python `team` Rust-only refusal; gate red only on `rust.agentctl.test` (foreign pidfd pair plus the console-in-harness artefact), waived by coordb after cold review and combined-tree verification. No persisted data was added.

Landing 1b (structured session candidates) landed 2026-11-21 as `5dd8708b` (verified tree `f10d1489`): session-less interactive records list report-only candidates from bounded newest-first store scans, matched by structured canonical cwd with sanitized bounded first prompts; revive walks whole stores head-only and its structured-cwd binds were verified as corrections against real stores (prefix collisions, mentions, parent dirs, Codex root-over-subdir). Gate red only on the upstream #1934 issue-parse heading (fixed on main in `080df8bf`, present in the combined tree) and the #2027 pidfd pair; landed under coordb's waiver after re-review GO and combined-tree verification. Still no persisted data and no record rewrites.

Landing 2 (`team act`) landed 2026-11-21 as `9f6fa482` (verified combined tree `d6de9d60` over `cadb5b08`): the driver-owned durable request queue with request-id idempotence and running markers, DriverRecord `manifest`/`actions[]` plus per-request ledger files (cleared with the pinned reader's owner; request files accumulate unbounded, pruning deferred), model `ActRequested` self-loops that never move a deadline, the allowlist verbs, and the `agentctl team act` CLI. Its gate was red only on the #2037 five-cycles host-process case (passes in isolation) and landed under coordb's waiver after cold review, a NO-GO fix round, and a same-reviewer GO. Bridge gap actions were deferred to a follow-up (2b) at design check.

Remaining for this issue: the `run-agent-prompt` wrapper (landing 3, design check first), then the shepherd loop/unit and deployment landings. Python revive still single-picks a Muse discovery and has no Claude/Codex discovery; whether that parity folds into a later landing is coordb's call at the landing-2 design check (restorefix owns #1934/#1992/#1993). Landing 2's per-request files under the team state directory's `actions/` accumulate without bound; pruning them is deliberately deferred to a later cleanup landing.
