---
title: 'host-baseline-contract: reproduce unrelated portable validation failures on unchanged main'
status: open
priority: 2
issue_type: bug
created_at: 2026-10-27T16:01:30.599636281+00:00
updated_at: 2026-10-27T18:24:50.018664828+00:00
---

# Description

[GPT-6 Codex] On unchanged main `1f0a7a90`, a same-host canonical DAG comparison of 22 earlier failing nodes plus four dependencies completed with 17 failing nodes and nine passing nodes (1564.2 seconds). Every reproduced node is listed below with its unchanged-main reproducer. This is node-level evidence; failing case populations can differ between runs. No broad green result is claimed.

Reproducer for each row from that unchanged checkout: `nice -n 19 ionice -c3 common/bin/dagrun run --dag validation.dag.yaml --selected TAG --max-steps 4 --max-cpus 4 --keep-going`. Use real matching legacy timezone data on hosts missing it. No assertions, test counts or fsync calls were removed.

Reproduced failing nodes:

- `python.wrkslots.shard-0`: FAIL on unchanged main.
- `python.wrkslots.shard-1`: FAIL on unchanged main.
- `python-lifecycle.namespace.shard-0`: FAIL on unchanged main.
- `python-lifecycle.namespace.shard-1`: FAIL on unchanged main.
- `python-lifecycle.namespace.shard-2`: FAIL on unchanged main.
- `python-lifecycle.namespace.shard-4`: FAIL on unchanged main.
- `python-lifecycle.namespace.shard-3`: FAIL on unchanged main.
- `python-lifecycle.host.shard-1`: FAIL on unchanged main.
- `python-lifecycle.host.shard-0`: FAIL on unchanged main.
- `vibe-talk.check.read-aloud-views-browser`: FAIL on unchanged main.
- `python-lifecycle.host.shard-2`: FAIL on unchanged main.
- `vibe-talk.check.read-aloud-browser`: FAIL on unchanged main.
- `packages.standalone.wrkslots`: FAIL on unchanged main.
- `packages.standalone.tick-hub`: FAIL on unchanged main.
- `python-lifecycle.namespace.shard-5`: FAIL on unchanged main.
- `rust.agent-teams.test`: FAIL on unchanged main.
- `vibe-talk.test.unit`: FAIL on unchanged main.

Observed causes include valid opaque nsfs roots refused by workspace-tool mount parsing, unavailable passwordless sudo for privileged host tests, group-writable fixture routing data in agent-teams, absent PyYAML installed-file inventory in the tick-hub package smoke, and media timing/contract assertions. Source or host corrections remain follow-up work, rather than being silently called validation success. A minimal namespace-parser correction and four tests were preserved outside this repository; it reaches the next missing host prerequisite (sudo), and is not part of the watcher change.

Other outcomes must not be called pre-existing failures: `python.dagrun.shard-0`, `vibe-talk.check.voice-call-drops-browser`, and `cross.herdr-run.differential` PASS on unchanged main. Candidate runs are required separately. `python.repository-infrastructure.package-contract` also PASS: its earlier failure came from temporary diagnostic tests added during investigation, since removed; this was our own diagnostic-patch effect, not a baseline defect.

`vibe-talk.check.read-aloud-blocks-browser` PASS on first baseline, FAIL on a second at the media-resume assertion; on the candidate it FAIL at the sentence-highlight population assertion, then PASS on a focused canonical rerun (54.2 seconds). These are distinct assertions, recorded as a flaky supplemental check, not a fabricated identical reproduction.

Current delivery receipts: source 59543339 passed all 40 selected contract nodes in 457.4 seconds. The three extra candidate checks passed, with the herdr-run failure/full focused passing pair retained. Session-follow hotfix a76450de passed all 33 selected nodes, including 1,585 Rust cases and Python shards with 1,285 and 1,284 passes (four expected failures), 777 agentctl and 103 Herdr-agent comparisons. No broad green result is claimed.

A first hotfix run on b24f79a1 had one timing-sensitive reminder fixture fail: a_reply_the_transcript_holds_is_not_reported_missing_though_the_pane_does_not_show_it expected one typed reminder and saw none. Five isolated runs of the unchanged failed test and a Cargo rerun passed; the full unchanged-head rerun passed all 33 nodes in 490.7 seconds. The rebased exact-head a76450de run also passed the fixture and its full selected gate. Both failed and passing receipts are preserved. Root cause is not established; the lead explicitly accepted recording it as flaky rather than blocking publication.

This issue remains open for the reproduced host/tool failures and flaky checks. The earlier relevant shutdown and polling reply-order bugs were fixed separately. See ai_docs/transient/2026-10-27-agent-watch-recovery/VALIDATION.md for the receipt matrix. The source change has no validation-timeout or generic policy change.
