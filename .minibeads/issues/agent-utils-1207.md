---
title: 'codex-start-ready: Codex launch times out when a stale working OSC title hides an idle prompt'
status: open
priority: 1
issue_type: bug
assignee: mu-rsctl
created_at: 2026-11-10T14:05:00.000000000+00:00
updated_at: 2026-11-10T14:05:00.000000000+00:00
---

# Description

`agentctl start NAME --profile codex-astra-ultra` timed out twice in a row on this host (about 08:22 and 08:27 ET, 2026-11-10) with `{"error":{"code":"timeout","message":"timed out waiting for agent startup"}}`, while the same profile had worked about 30 minutes earlier. The pane screen showed Codex idle at its prompt ("Daybreak is on for new turns." | "» Ask Codex to do anything" | model footer), but Herdr reported the agent with status `working` from the OSC title rule (`osc_title_working`, region=osc_title, priority=1050): the title said working while Codex was idle, so startup readiness never fired and the Herdr agent name was never applied.

The failure strands the launch: the record is left `lifecycle=launch_failed` with a live orphan pane, `revive` refuses ("record lifecycle is not running"), and `stop` cannot find the agent (its identity check resolves the Herdr agent by name, which was never applied) until the pane is closed by hand. `agentctl promote` already adopts a launch_failed record whose harness is live, but only when the Herdr agent name still points at the pane, which is exactly what this failure removes.

# Design

- Readiness: a visible idle Codex prompt must win over a stale "working" OSC title. The post-timeout takeover (`codex_ready_after_startup_timeout` / `codex_idle_ready_with_cancellation` in `rs/agentctl/src/client.rs`) currently requires Herdr status `unknown` before it trusts the screen; extend it so a screen that proves a fresh idle composer (empty composer, recognized footer, no running-turn markers — including recognizing the current "Daybreak is on for new turns." startup screen) counts as ready even when the OSC title still says working. Keep the resumed-session working path and the identity re-verification (same pane/tab/terminal, pinned harness process, name applied and re-resolved) unchanged.
- Tests carry a captured idle Codex screen of the new shape (names and paths neutral) beside the existing `rs/agentctl/testdata/codex/` captures: the captured screen must read idle, a genuinely working screen must still not read idle, and the startup takeover must accept the stale-working case and apply the name.
- Cleanup: a `launch_failed` record whose recorded pane is still live must be cleanable. `stop` must find it by its recorded identity (pane, tab, workspace, cwd — re-verified immediately before closing) and close the pane even when the Herdr agent name was never applied and the harness is still running in the pane; today that path falls through to a by-name resolution that fails. Alternatively or additionally, `promote`/`revive` may adopt such a record by recorded pane identity. Closing targets only the recorded pane, never a tab sibling.

# Acceptance Criteria

- A regression test with the captured Daybreak idle screen passes: startup readiness accepts the launch, applies the name, and `start` returns success instead of the timeout error.
- A genuinely working Codex screen (running turn markers) is still not readiness; the resumed-session working proof is unchanged.
- A test shows `stop` on a `launch_failed` record with a live recorded pane and no applied Herdr name closes exactly the recorded pane and archives the record.
- The focused agentctl test suites for the harness screen rules and the client startup path pass, and the slice is gated on its own before landing.

# Notes

Filed 2026-11-10 from a same-hour defect report (owner rule: agentctl defects are reported and handled the same hour). Evidence kept with the reporter: two failed launch logs quoting the idle screen and the `osc_title_working` verdict, and the retained `launch_failed` record. Related: #921 codex-ready-detection (the post-timeout screen takeover this extends).
