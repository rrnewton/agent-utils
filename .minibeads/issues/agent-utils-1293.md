---
title: 'wrkslots-ttl-escalation-teardown: increasingly aggressive enforcement past TTL culminating in deterministic teardown with best-effort rescue'
status: open
priority: 0
issue_type: feature
labels:
- wrkslots
created_at: 2026-11-11T12:45:00+00:00
updated_at: 2026-11-11T12:45:00+00:00
---

# Description

[watermelon-full-preview] Owner directive (Slack 6224, 2026-11-11 7:38 AM ET, verbatim):

> That's not what I asked for when we designed wrkslots. I asked for increasingly aggressive enforcement as we go past TTL culminating in deterministic teardown which would use a best effort code based deterministic rescue (eg push any stranded code to dated rescue branches) which would necessarily be heuristics. We could also potentially have that deterministic cleanup invoke a muse agent to make decisions about how rescue is done but not whether reclamation happens.

Today a slot past its heartbeat or owner TTL can refuse removal indefinitely (unproven private history, lock holds), so slots pile up: one consuming project holds 132 slot gitdirs (about 265 GiB), many dead, and removals keep refusing. Already landed: #1255 through #1257 (refusals and ref inventory), #1278 (salvage bundle object store), #1280 (the kept set: what counts as slot-private). #1287 (retire-pending lock narrowing) is landing now. Open and related: #400 slot-reclaim-deadlock. No prior TTL escalation or rescue design was found in the tracker or in ai_docs; this issue is that design.

# Design

- (1) Escalation ladder past the slot TTL (heartbeat or owner expiry), configurable: T+0 warn the owner agent and the coordinator; T+1 d an alarm (a tick-hub HEALTH line, plus a daily refused-or-overdue report with exact reasons); T+2 d stop the slot's agent; T+3 d deterministic teardown. Thresholds are per-project configuration, not literals.
- (2) Rescue before teardown, best effort and code-based: every unpushed branch or commit (detached HEADs, stash entries, nested worktrees and private submodule commits included) goes to dated rescue refs, e.g. refs/heads/rescue/<slot>/<YYYYMMDD>/<branch> on the slot's recorded push remote, falling back to a verified local bundle under the control dir. Uncommitted changes become a WIP commit on the rescue branch. Every path is logged; heuristics are fine. Reuse the existing salvage machinery (#1278/#1280 receipts, private_history_counts) instead of a parallel path.
- (3) Optional hook: a Muse agent may decide HOW to rescue (which branches matter, naming, messages), never WHETHER. Reclamation happens on schedule regardless; a hook failure or timeout falls back to the code-based rescue.
- (4) Past T+3 d refusals are impossible. An ambiguous or failed rescue still tears down, after writing the bundle; it is reported, never kept. Before T+3 d, today's refusals and safety checks stay as they are.
- (5) Per-project opt-in plus config (TTL thresholds, rescue remote, bundle dir), so a consuming project can enable it first, plus a dry-run/report mode that shows what each overdue slot would get (rescue refs, bundle, teardown time) without acting. Rescue and teardown follow #1287's lock pattern: slow work runs outside the project-root flock. Keep Python and Rust (rs/wrkslotsd remove_write.rs) in parity, or file the Rust parity as a follow-up.

# Acceptance Criteria

- Dry-run/report mode lists each overdue slot with its ladder stage, rescue refs, bundle path, and teardown time, and acts on nothing; a consuming project can enable report and alarm first, then rescue-and-teardown behind opt-in.
- At T+0 the owner agent and coordinator are warned; at T+1 d the tick-hub HEALTH line and the daily refused-or-overdue report name the slot with exact reasons; at T+2 d the slot's agent is stopped; each emits a dated receipt.
- At T+3 d teardown always happens: rescue refs or a verified bundle are written first (including detached HEAD, stash, nested worktree, submodule-private and uncommitted-WIP cases), the slot is removed, and an ambiguous or failed rescue is still reported and never keeps the slot.
- Before T+3 d every existing refusal and safety check behaves exactly as today; holds, liveness, and private-history proofs are untouched.
- Tests count operations and assert refs or bundles, never wall-clock budgets; fixtures and scratch projects only, never live slots on a shared host.

# Notes

P0 owner directive; land slice by slice (report/dry-run and alarm first, then rescue-and-teardown behind opt-in), each with a path-selected owner-tier gate. Filed by mu-ttlteardown.
