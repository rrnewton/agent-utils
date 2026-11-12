---
title: 'eden-daemon-containment: an Eden slot''s edenfs@<state>.service runs outside every slot and gate cgroup limit'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-12T21:08:24+00:00
updated_at: 2026-11-12T21:08:24+00:00
---

# Description

[opus 5.5] Follow-up to #1409 eden-in-worktrees-image and #1204 eden-checkouts. Eden starts its daemon as the systemd user service `edenfs@<escaped state dir>.service` (from the packaged `edenfs@.service` template), not as a child of the command that ran `eden clone`. So the daemon escapes any containment wrapped around the clone, whether a `systemd-run --user --scope -p CPUQuota=... -p MemoryMax=...`, nice, or ionice. It runs with Nice 0, CPUQuota infinity and MemoryMax infinity.

Seen 2026-11-12 on a shared host during the #1409 sudo-image trial. The trial ran every heavy step inside a scope capped at 800% CPU and 16G of memory, at nice 19 with idle IO; that was the condition under which the host's other team allowed it to run alongside their validations. A `du` over the mounted checkout then made the probe's edenfs fetch the whole repository: 30 GiB peak memory, 25.5 GiB written to its state directory, and 23 CPU-minutes in about an hour, all outside the agreed limits. The slot image itself allocated only 7.4 MiB, because the state directory lived outside it.

# Design

- When wrkslots creates or mounts an Eden slot, it bounds the daemon itself, by one of:
  - a runtime drop-in for that slot's `edenfs@<state>.service` instance (`systemctl --user set-property --runtime` or a drop-in under `$XDG_RUNTIME_DIR/systemd/user/`) with CPUQuota, MemoryMax, Nice, IOSchedulingClass/IOWeight; or
  - a dedicated slice (for example `wrkslots-eden.slice`, or a per-slot slice) that the instance runs in (`Slice=` in the drop-in), carrying the limits.
- The limits come from the slot type's configuration with conservative defaults, and `wrkslots doctor` or `audit` reports an Eden slot whose daemon is unbounded.
- The daemon's state directory lives inside the slot's image or subvolume, so the slot's quota and accounting cover it. Today it can sit outside, where per-slot disk accounting misses it.
- Teardown confirms the instance unit is inactive before it removes the image.

# Acceptance Criteria

- A fixture test (stubbed systemctl) shows create/mount writing the drop-in or slice assignment with the configured limits, and teardown checking that the unit is inactive.
- A slot-level accounting test: an Eden slot's daemon memory and its state-directory bytes are attributed to the slot (cgroup and quota), and doctor flags an unbounded daemon.
- A real trial on a host shows `systemctl --user show edenfs@<state>.service -p CPUQuotaPerSecUSec -p MemoryMax -p Nice` with the configured values.
- Until this lands, hosts keep their Eden slot cap at 1 and nobody walks an Eden mount with du, find, or recursive grep.

# Notes

Numbered from devbig030's interim block (1400-1469). The other host lowered its eden_host_cap to 1 until this lands (2026-11-12).
