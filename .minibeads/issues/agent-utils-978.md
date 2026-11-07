---
title: 'retire-orphan-candidate: retire-stale-registration demands a session pin a bare-shell candidate cannot satisfy'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-06T20:51:28.861728015+00:00
updated_at: 2026-11-07T11:30:00.000000000+00:00
closed_at: 2026-11-07T11:30:00.000000000+00:00
---

# Description

A revive that failed after creating its candidate pane left an orphan bare-bash pane; retire-stale-registration then refused 'candidate pane is live; pin --replacement-pane/--replacement-session' even with both flags, because an idle shell names no session. Retire now classifies the candidate (check_candidate): a dead idle-shell candidate needs no pin; only a live harness does. Design+lessons for restore-in-place in ai_docs/transient/2026-11-06-restore-in-place/.

# Notes

## Resolution

Resolved by ed480bc9 "agentctl: let retire proceed past an orphan candidate's bare shell" (2026-11-06), whose body cites this issue. retire_revive_preflight classifies a live candidate pane with check_candidate: a dead idle-shell candidate needs no --replacement-session pin (only a live harness does), and a pin mismatch names the candidate pane. Test B8 retire_stale_needs_no_pin_for_a_candidate_pane_holding_only_a_shell covers it. Checked read-only on main by cl-sync (opus) on 2026-11-07.
