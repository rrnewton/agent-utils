# Agent-watch recovery validation

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

## Task-local publication ruling

The lead requires the complete selected agentctl contract and housekeeping on the exact publication candidate, plus explicit candidate checks of dagrun Python shard 0, voice-call-drop browser, and herdr-run comparisons. A failure in those three gets one focused candidate rerun: a pass is recorded as a flake, a second failure must be fixed. Only the 17 reproduced baseline nodes may be excluded. This ruling is local to this task and does not change reusable repository policy. All full runs use nice 19 and idle I/O; hold if another integration gate’s browser/network work would be harmed.

## Additional candidate checks completed

The candidate dagrun Python shard passed (492 passes, two skips). The candidate
voice-call-drop browser check passed. The herdr-run comparison first reported
one divergence: adjacent read-only `pane process-info` requests for the same two
panes occurred in opposite order. Its one focused canonical rerun passed all
1868 paired checks in 208 seconds (210.9 seconds including dependencies). Both
results are retained as a flaky check, per the task-local ruling. The supplemental
long-message audio failure and passing 54.2-second rerun are both retained.

The source run at 36717a97 passed 40 selected nodes in 564.2 seconds. The
policy-only restoration of AGENTS happened during that run, so its receipts are
source evidence rather than a final immutable-head receipt. After consolidating
these records and restoring policy to main, the final publication commit is
validated again with the canonical selected gate before push. No generic policy
or timeout change remains, and no assertion was weakened.

## Exact-head gate found and fixed polling reply order

The consolidated bce70e05 gate failed the polling test that requires two
captured request replies to be delivered first then second. No publication
occurred. Three forced timestamp/directory-order cases failed before the fix;
a fourth covers a backwards receipt clock. Polling prompts sorted durable
queue IDs, but polling replies sorted receipt-clock strings. Both now use the
same durable prompt queue order. A streaming-cache prototype failed existing
fairness tests and was removed; streaming fairness and its assertions remain.
The final focused polling and runtime suites pass 179 tests in 59.99 seconds.
The original failed fixture was automatically cleaned, so its exact wall-clock
sample is unknown. Track this correction as #587 chat-reply-order. Run the
complete canonical gate on the fixed publication head; this prior failure is
not a baseline exclusion or a passing receipt.
