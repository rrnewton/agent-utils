---
title: 'wrkslots-census-foreign-procs: slot-use census must not refuse on unrelated processes unreadable fds'
status: open
priority: 2
issue_type: task
created_at: 2026-10-28T10:46:28.888320719+00:00
updated_at: 2026-10-28T10:46:28.888320719+00:00
---

# Description

The Python wrkslots process-use census refuses the whole remove/validate operation as 'process use is indeterminate' when readlink on ANY host process's fd fails, e.g. ENAMETOOLONG on /proc/1186775/fd/26 (an unrelated dev-hermit test binary) or a stuck PID-namespace init (PID 2991802, zap_pid_ns_processes). This blocks slot removal for processes unrelated to the slot (observed in gate 52 of #562 slice 36: packages.standalone.wrkslots, python.wrkslots.shard-0, python-lifecycle.host.shard-0/1 on the host; all pass in a PID namespace).

Do NOT simply treat unreadable fds as not-using-the-slot: a very long path inside the slot could also raise ENAMETOOLONG and fail open. Instead, when readlink fails, stat /proc/PID/fd/N (stat follows the fd even when readlink cannot) and compare st_dev with the slot's filesystem device. Slots are separate ext4 images, so a different st_dev proves the fd is elsewhere. Keep 'indeterminate' only when stat also fails.

Scope: py/wrkslots/cli.py census (_readable_link_target and callers), with tests: a foreign fd whose readlink fails but st_dev differs from the slot device must not refuse; a same-device unreadable fd must still refuse. Rust wrkslotsd use-scan parity follows in the same fix if it shares the failure mode. Filed by mu-rust at coordinator direction after #562 slice 36.
