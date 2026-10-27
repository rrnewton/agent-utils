---
title: 'host-baseline-contract: reproduce unrelated portable validation failures on unchanged main'
status: open
priority: 2
issue_type: bug
created_at: 2026-10-27T16:01:30.599636281+00:00
updated_at: 2026-10-27T17:17:54.320791416+00:00
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

The original relevant Python shutdown failure is fixed separately: SIGTERM inherited Exception and could be swallowed by a reconciliation retry handler. The deterministic regression fails before the BaseException correction; process, descendant and lock assertions are unchanged. The selected candidate gate on `36717a97` PASS: 40 DAG nodes, 1530 Rust cases across 12 binaries, 2565 passing Python shard cases, 777 agentctl paired comparisons and 103 herdr-agent paired comparisons, with package and housekeeping checks. Duration 564.2 seconds. Its dedicated chat scratch setting preserves all real fsync and fault-injection calls. The earlier disk-backed run naturally completed its 1517 Rust cases in 3573 seconds, but its broad overall result remains red.

This issue remains open for the reproduced host/tool failures and transient checks. Detailed receipt matrix is in `ai_docs/transient/2026-10-27-agent-watch-recovery/VALIDATION.md`; final candidate additional checks are recorded there before publication.

Final extra candidate checks: dagrun Python shard PASS (492 passes, two skips), voice-call-drop browser PASS. Herdr-run first failed only in adjacent read-only process-info call order, then passed its permitted focused rerun (1868 paired checks, 208 seconds). Both results are recorded as a flake. The supplemental long-message audio failure and 54.2-second passing rerun are recorded distinctly. The 36717a97 source gate passed; final immutable-head validation follows record consolidation because policy-only AGENTS restoration happened during that source run.
