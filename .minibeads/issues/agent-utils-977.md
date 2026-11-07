---
title: 'retire-orphan-candidate: retire-stale-registration demands a session pin a bare-shell candidate cannot satisfy'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-06T20:51:28.861728015+00:00
updated_at: 2026-11-06T20:51:28.861728015+00:00
---

# Description

A revive that failed after creating its candidate pane left an orphan bare-bash pane; retire-stale-registration then refused 'candidate pane is live; pin --replacement-pane/--replacement-session' even with both flags, because an idle shell names no session. Retire now classifies the candidate (check_candidate): a dead idle-shell candidate needs no pin; only a live harness does. Design+lessons for restore-in-place in ai_docs/transient/2026-11-06-restore-in-place/.
