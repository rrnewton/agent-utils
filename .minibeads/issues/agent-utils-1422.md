---
title: 'watch-unit-repair-honours-pins: watch-agents unit repair restarts units that a turnover pin deliberately holds'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-12T15:53:57+00:00
updated_at: 2026-11-12T15:53:57+00:00
---

# Description

[opus 5.5] Turnover pins (`REGISTRY/turnover.pin`, `REGISTRY/turnover-pins/<unit>.pin`, the services-root `global.pin`) mark a unit as deliberately held: it must not restart in a pinned window. The turnover reads them (services/turnover.rs read_pins/pin_reason), and #1416 team-restore-verify will report a pinned unit as "held (pin: <reason>)". But the watcher's `unit_repair` (restore.rs config, on by default) doesn't read pins, so it may restart a unit an operator deliberately stopped and pinned. Found while designing #1416 (mu-restdrv, 2026-11-12).

# Design

- `unit_repair` reads the same pins through the turnover's pin reader, and skips a pinned unit with a notice naming the pin and its reason (once per pin, not every pass).
- An empty pin file is invalid, as in turnover: report it, and don't treat it as a pin.

# Acceptance Criteria

- A fixture test: a pinned, inactive unit is not restarted and a notice is recorded; the same unit without its pin is restarted.

# Notes

Numbered from devbig030's interim block (1400-1469). Related: #1416 team-restore-verify.
