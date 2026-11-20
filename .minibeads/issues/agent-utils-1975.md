---
title: 'restore-boot-recovery: restore restarts lanes and re-attaches slots idempotently after a reboot'
status: open
priority: 1
issue_type: task
labels:
- agentctl
- restore
- wrkslots
created_at: 2026-11-20T13:51:20-05:00
updated_at: 2026-11-20T13:51:20-05:00
---

# Description

[watermelon-full-preview] From the 2026-11-20 reboot drill on a 20-lane host (owner requests 245/246/256/257). Boot restore ran 17m12s, revived 5 of 20 lanes, and refused 15; the owner-facing chat bridge started only at +20m because restore starts its configured systemd units after every lane revive. Restore revives serially at about one lane per minute. Hand recovery then had to re-bind resumed Muse candidates, revive records with no recorded session id, and clear a record stuck in "stopping", none of which had a supported path. The slot-image check inside restore crashed at boot (fine when rerun later), wrkslots heartbeat/transfer-owner held the registry lock for minutes while about 30 lanes re-attached (400-600s waits), and a lane that wrote a checkpoint handoff before the reboot could not write its final one ("immutable sidecar already occupied"). The owner wants both halves: the deterministic infra restarts lanes and re-attaches work slots idempotently, and the coordinator sweeps what is not back. Recovery must not correct records (owner 257): restore reports records missing a profile, bypass flag, effort or session id and never rewrites them; the only record changes are the ones a revive of the same session needs (identity, terminal_id/tab_id, queue target). Prevention lives in #1822 (landed) and #1982 (doctor audit, separate).

# Design

Lane mu-restoreboot; plan approved by the lead coordinator 2026-11-20. Phases 2-3 wait for #1934 agentctl-restore-muse-sessions-and-timeout to land and rebase on it.

(1) wrkslots: give `transfer-owner` the heartbeat/adopt prevalidated treatment (full validation and storage-inconsistency scan outside `config.lock`, registry-identity compare plus single-record write under it) in the Python edition and the Rust mirror; guard `image status` against a dead FUSE mount (bounded probe instead of a bare `statvfs`) so the boot-time remount check degrades per-image instead of crashing.
(2) Revive refusals: publish a resumed candidate with its queue `target.json` rebound to the new pane; add the explicit operator flag `agentctl revive NAME --session ID` (verifies the id is a session for the record's cwd and harness; never automatic, never used by restore); restore lists session candidates and record gaps (missing profile/bypass/effort/session) in the report JSON and one summary line, and otherwise leaves those records byte-identical; root-cause and narrow the `revive_proof` presentation comparison that refused a bare-shell pane; let a record stuck in `stopping` be stopped/archived under the existing dead-pane proof. Adopted records with `anchor_rule` null gain the rule only through a real revive's pin, never by a restore fix-up.
(3) Ordering: new optional `restore.json:early_units[]` start right after the Herdr wait, before lane revives (owner-facing bridge/watcher first); then coordinator and `priority_agents[]`, then the remaining lanes through a bounded pool (`revive_parallelism`, default 4, cap 8; 1 reproduces today's serial order). Report row order stays deterministic. The coordinator notice gains the sweep instruction.
(4) Timeout: both shipped restore units move from TimeoutStartSec=1800 to a fixed 7200, keeping #1934's partial-report/resume so a watchdog kill loses nothing.
(5) Observability (owner 256): restore keeps running under systemd outside Herdr and appends a plain per-boot log beside the report JSON, every phase line stamped in local time. Once Herdr answers, a best-effort step idempotently ensures a `system` workspace with a `recovery` tab tailing that log (plus the shepherd log when present); it never fails restore, and `recovery_tab: false` disables it. The shepherd (#1559 restore-recovery-agent-plan) reads the same log file. The tab pass from #1971 agentctl-tabs-scheduled-hygiene must never close the recovery tab (a live tail, not a bare shell).
(6) Handoff sidecar: allow a final handoff after a pre-reboot checkpoint. Design priced before code: (a) owner-authenticated supersede of one's own checkpoint through the existing generation-bump proofs, or (b) a separate mutable checkpoint file. Persisted changes reported to the lead coordinator before landing.
Slots re-attach idempotently throughout: a second restore of the same boot replays, live harnesses are skipped, and slot holds/transfers repeat as no-ops. All testing on scratch registries with a fake Herdr; never another team's registry.

# Acceptance Criteria

- With a fake Herdr and scratch registry, a simulated reboot restore starts early units before the first non-coordinator lane revive, revives lanes with at most `revive_parallelism` concurrent launches (condition-based waits), and `revive_parallelism: 1` reproduces the previous serial command sequence.
- A resumed candidate in a new pane is published with refreshed terminal_id/tab_id and a queue target naming the new pane, with no second harness launched; `revive --session ID` resumes exactly the named session for the record's cwd/harness and refuses an id for another cwd or harness.
- A record with no session id, no profile, or no bypass flag is reported in `record_gaps` plus one summary line and is byte-identical after restore; a bare-shell pane whose non-identity presentation fields flap revives, while a changed pane/tab/shell identity still refuses; a `stopping` record with a provably dead pane archives, and with a live harness still refuses.
- `transfer-owner` at ~250 rows does zero fleet scan work under `config.lock` (lock-scope test), revalidation still refuses a changed tip, and both editions agree (differential tests); `image status` against a dead mount returns per-image errors and exit 0 degraded instead of a traceback.
- The per-boot log has a local-time stamp on every phase line; the recovery tab is ensured exactly once across two restore runs, survives the tab-hygiene pass, and restore still completes with Herdr down.
- Rendered restore units carry TimeoutStartSec=7200; new `restore.json` keys are top-level and ignored-with-warning by an agentctl that predates them (pinned by test), and `restore --write-agents --apply` preserves them.
- A checkpoint handoff written before a simulated reboot can be followed by the lane's final handoff after owner transfer, while a different-bytes overwrite of an immutable sidecar still refuses and preserves both.
- New test files are classified in validation/components.json; `cargo fmt --all -- --check` is clean; each landing passes the gate.

# Notes

Related: #1934 agentctl-restore-muse-sessions-and-timeout (session capture at launch, live-candidate publish, restore resume; phases 2-3 rebase on its landing), #1559 restore-recovery-agent-plan (shepherd consumes the log and slot state), #1971 agentctl-tabs-scheduled-hygiene (restore tab pass must spare the recovery tab), #1977 eden-daemon-restart (per-slot Eden daemons, same lane, lands separately). Persisted fields: no new agent-record fields; new optional top-level `restore.json` keys (`early_units`, `priority_agents`, `revive_parallelism`, `recovery_tab`) ride the existing unknown-key tolerance; report JSON gains additive keys only. A consumer pins an older agentctl, so every persisted addition is reported to the lead coordinator before landing.
Follow-up: a hung (not dead) FUSE mount can still block `statvfs` uninterruptibly in-process in both editions (the bounded child probe was not reused: slotimage cannot import cli, the Rust core injects facts); the fast ENOTCONN crash from the 2026-11-20 boot is covered.
