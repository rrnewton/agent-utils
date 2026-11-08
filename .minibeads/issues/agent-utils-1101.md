---
title: 'wrkslots-sandbox-show-config-handler: dispatch-shaped Rust handler for sandbox show-config'
status: open
priority: 2
issue_type: task
created_at: 2026-11-08T23:57:40.000000000+00:00
updated_at: 2026-11-08T23:57:40.000000000+00:00
---

# Description

Following #1095 wrkslots-image-status-handler, the next
dispatch-shaped handler is `wrkslots sandbox show-config`: it prints
the full sandbox settings as indented JSON. The pure core is landed
(`sandbox_state::SandboxSettings`, `settings_from_obj`,
`settings_to_obj`); what remains is the handler returning
`(exit, stdout, stderr)` byte-identical to the Python edition, in a
mu-rsimg-owned module the shared core can wire with one dispatch
arm.

Python behaviour to match (`py/wrkslots/imagecmd.py:1622`):
`json.dumps(sandbox.settings_to_obj(config.sandbox_settings), indent=2)`
plus a newline, exit 0. Keys appear in the Python dict's insertion
order, which a sorted map does not reproduce.

# Design

- Handler in the already-registered `sandbox_state` module (no new
  lib.rs line): input is the raw `sandbox` section value (or its
  absence), output the ordered JSON rendering.
- An in-crate oracle differential (same style as #1095) drives the
  Python handler as a subprocess on shared fixtures and compares
  bytes for: absent section (defaults), custom limits and env, and
  an invalid section refusal.
- No shared file is edited; the dispatch arm is proposed in the
  landing report, and the CLI-level differential lands with the
  wiring.

# Acceptance Criteria

- The oracle differential passes on all named cases.
- The handler lands unwired, gate green, with the dispatch arm
  text sent to the coordinator.
- Fixtures and temp files only.

# Notes

Part of #562 rust-primary-wrkslots P9. `limits` is the following
handler in the lane's running list.
