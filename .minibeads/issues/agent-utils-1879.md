---
title: 'agentctl-headless-console-paved-flow: console input urgent, /interrupt MSG with hold, console viewer under systemd unit, headless refusal by real support'
status: closed
priority: 0
issue_type: feature
labels:
- agentctl
created_at: 2026-11-18T21:11:10-05:00
updated_at: 2026-11-18T21:11:10-05:00
---

# Description

[watermelon-full-preview] Owner messages threads 195-200 and 201/203 (2026-11-18): a headless agent opened the old tail view instead of the console, typed console input queued behind agent sends, bare `/interrupt` was the only interrupt form, and `start --mode headless` printed `agentctl tail NAME --follow` as the next step. Wanted: one paved flow like making a normal TUI subagent. This issue supersedes the viewer part of the workspace-cultivation work and builds on the console rename work; the headless Muse adapter work is re-scoped by the verification in this issue.

Work:
- A. Console input is urgent priority and deliver now (front of the queue, between tool calls mid-turn), like owner chat or Esc-then-type in the Claude TUI.
- B. `/interrupt MSG` interrupts and MSG becomes the next turn; bare `/interrupt` still works.
- C. After a bare `/interrupt`, hold the queue (until the next console line or about 30 s) before starting a queued item.
- D. `start NAME --profile P --mode headless` (any existing profile) opens a Herdr tab running `agentctl console NAME` and records it as the viewer tab; restore, turnover and takeover recreate the viewer as console; watch tab hygiene treats a tail viewer as stale. The wrapper always runs under its own systemd user unit (install-units by default; never parented to Herdr or the caller's shell), then the Herdr tab is a pure client over the control socket; if Herdr dies only the views go and restore reopens the tabs. The detached setsid launch without a unit stays available only behind an explicit flag.
- E. start, restore and turnover refuse up front (changing nothing) when the profile's harness has no headless adapter, naming the harness-support-matrix row; the refusal follows real support (Muse verified by a real headless run; Codex refused). Update the harness-support-matrix in the same commit.

# Design

- Wrapper queue: console input enqueues priority urgent and deliver now; pending order already sorts by priority, and `delivers_now` already treats urgent as now, so the change is the enqueue stamps plus tests proving a console line jumps ahead of normal agent sends mid-turn.
- Interrupt with message: the console sends one control request carrying the message; the wrapper interrupts the running turn and enqueues the message as an urgent console turn, so it is the next turn. A bare interrupt instead arms a hold deadline; `maybe_deliver` delivers nothing while the hold is live, the next console line clears it, and it expires after about 30 s.
- Viewer: `ensure_headless_viewer` runs `agentctl console NAME` in the new tab. The record keeps naming the tab as the viewer; hygiene learns the viewer's command (a tail viewer for a live headless agent is stale and is recreated as console).
- Launch: `start --mode headless` installs and starts the per-agent systemd user unit by default and waits for the control socket; `--detached` keeps the setsid launch. No Herdr parenting: the unit is the wrapper's parent, the console tab only connects to the socket.
- Refusal: the adapter registry is the source of truth (Claude and Muse today); start, restore and turnover check it before any record, unit or tab change and name the matrix row in the error.

# Acceptance Criteria

- A console line typed mid-turn is delivered between tool calls ahead of queued normal agent sends; a console line at a turn boundary goes first in its batch.
- `/interrupt MSG` ends the running turn and MSG runs as the next turn; bare `/interrupt` still interrupts; after a bare interrupt no queued item starts until the next console line or about 30 s pass.
- `start --mode headless` with any existing profile yields a recorded Herdr viewer tab running `agentctl console NAME`; restore, turnover and takeover recreate the viewer as console; a tail viewer is treated as stale by watch hygiene; every hint and doc names console.
- The wrapper runs under its own systemd user unit by default (Herdr death loses only the views); the setsid launch requires an explicit flag.
- start, restore and turnover refuse before changing anything when the harness has no headless adapter, naming the matrix row; Muse passes the refusal (verified by a real headless Muse run) and Codex is refused; the harness-support-matrix matches the code in the same commit.
- Dynamic proof: the flows are exercised by hand on a real headless test agent (never a coordinator or lead agent) and the console transcript is pasted as evidence.

# Notes

Owner-priority gate, deployed on landing. Split into 2-3 landings if faster (console input first, then viewer/launch/refusal). Related: the console rename issue, the workspace-cultivation issue, the headless Muse adapter issue (re-scoped here by real-support verification).

## Resolution

[watermelon-full-preview] Landed on main as bd5cb785, f2051be8, f92eb7ea (gate pass on the rebased tree, 2026-11-19). Console input is urgent/deliver-now, `/interrupt MSG` makes MSG the next turn with a bare-interrupt queue hold, the viewer is a recorded console tab (explicit tail viewers heal as console; older records untouched), the wrapper launches under its own systemd user unit (`--detached` is the explicit escape; turnover keeps its supervisor), profile mode switches to headless, and start/restore/turnover refuse harnesses with no headless adapter naming the matrix row. Verified dynamically: a pty console session on a test agent (urgent mid-turn start, interrupt-with-message as turn 2) and a real Muse agent under its unit answering exactly. The harness-support-matrix records Muse headless as supported in the same change.
