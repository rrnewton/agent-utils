---
title: 'wrkslots-census-test-hermetic: census-sensitive tests need their own fixture filesystem'
status: open
priority: 2
issue_type: task
created_at: 2026-10-28T14:24:08.912375344+00:00
updated_at: 2026-10-28T14:24:08.912375344+00:00
---

# Description

Since #618, the wrkslots process-use census proves an unreadable descriptor foreign only by filesystem device. Test fixtures created under pytest's tmp_path share the /tmp device with every other process on the host, so an unrelated host process holding an unreadable descriptor whose target is also on the /tmp device (measured: a foreign test binary's fd 26, ENAMETOOLONG) makes census-sensitive tests refuse as indeterminate. This now fails gate steps on every run on that host (the Rust remove differential tests in python.wrkslots.shard-0/1; the same mechanism also hits packages.standalone.wrkslots and two Python lifecycle host tests).

Fix: census-sensitive tests must place their fixture slot on their own filesystem, e.g. a private tmpfs such as /dev/shm (a distinct device from /tmp), or skip with a clear reason when no distinct-device filesystem is available. Production slots are separate filesystem images, so the device proof is correct there; this issue is only about test hermeticity.

Scope order (mu-rust lane): (1) py/wrkslots/tests/test_remove_write_rust.py fixtures on a distinct device with a clear skip; (2) survey the other census-sensitive harnesses (check_standalone lifecycle, Python lifecycle host tests) and apply the same treatment or record why they differ. Filed at coordinator direction after #562 slice 38.
