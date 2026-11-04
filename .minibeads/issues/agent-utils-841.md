---
title: 'page-test-runaway-memory: a page test failing with an unhandled rejection can grow without bound'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-04T15:37:04.174037341+00:00
updated_at: 2026-11-04T15:37:04.174037341+00:00
---

# Description

[opus 5.5] Found 2026-11-04 after the host's oomd killed a lane's sandbox twice (swap below 5%; the killed scope held ~594 GB).

The page suite's done.mjs can grow without bound after a page error. The trigger was a voice.js edit in progress that left one stale name. `placeInlineReply` then threw `ReferenceError: replyList is not defined` out of a row button's click. done.mjs's last test, "r REPLIES with the box ready; p pins; ...", failed with failureType `unhandledRejection`. After that, the done.mjs test process grew at about 1 GB/s of RSS. That is far past `--max-old-space-size=1024`, so the memory is off the V8 heap.

- **Run alone** (`node --test tests/js/page/done.mjs`, unthrottled, about 7 s), the file usually ends before the growth matters.
- **Run alone CPU-throttled** (`systemd-run --user --scope -p MemoryMax=3G -p MemorySwapMax=0 -p CPUQuota=25%`), it reached the 3 GB cap.
- **Run with the suite** at `--test-concurrency=8`, it reached 5 GB RSS within 6 s.
- **Only test 49** (`--test-name-pattern`): it fails the same way but peaks at 158 MB. The runaway needs the pages the file's earlier tests leave behind.
- **With the stale name fixed**, done.mjs passes and peaks at 489 MB under the same throttle.

The growth is a defect of its own: any future page error of this shape can take the host down.

# Design

- Find what keeps running after the failed test: a timer, microtask or animation loop on a fixture page that is never torn down, or a retry that allocates on every pass. Make the fixture tear every page down at test end, or bound it.
- Validation should run the page suites under a hard memory cap per test process, for example a cgroup scope, so a runaway fails one step instead of the host.

# Acceptance Criteria

- Reintroducing a ReferenceError in `placeInlineReply` (voice.js) makes done.mjs fail fast, with bounded memory (under 1 GB), throttled or under the suite's concurrency.
- The page suites in validation and in the lanes' inner loops run under a hard per-process memory cap.

# Notes

Repro material: /tmp/r837/capped-all.sh and /tmp/r837/capped.sh on the lane host (scoped, capped runners).
