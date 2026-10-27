# Agent-watch recovery delivery and validation

[GPT-6 Codex] Delivered on main: reservation `435de837`, source
`59543339cc46efd52e5bbfd392fbd0a3b46f5a16`, and live session-follow hotfix
`a76450de651d191b6bf4f117c8e1202bff45cac7`. Each publication was checked by
fresh fetch, fast-forward ancestry and diff inspection, ordinary push, and
remote ref verification. No generic policy or validation-timeout change remains.
Issues #585 agent-watch-recovery and #587 chat-reply-order are closed;
#586 host-baseline-contract remains open for the failures below.

## Published receipts

All full candidate validations used `nice -n 19 ionice -c3`, eight CPU/step
caps, supported executable chat-fixture scratch, and matching legacy timezone
data. No assertions, test counts, fault injections or fsync calls were removed.

| Exact commit | Canonical selected make validate result |
| --- | --- |
| `59543339` | PASS: 40 nodes, 457.4 seconds; 1,564 Rust cases across 12 binaries; Python shards 1,285 and 1,284 passes, four expected failures; 777 agentctl and 103 Herdr-agent paired comparisons |
| `b24f79a1` | First run FAIL: one reminder fixture; 29 nodes passed, one failed, three aborted. Unchanged full rerun PASS: 33 nodes, 490.7 seconds, all 1,565 Rust cases; Python 1,285 and 1,284 passes, four expected failures; 777 and 103 paired comparisons |
| `a76450de` | Rebased onto core changes `b19c1406`: PASS, 33 nodes, 473.1 seconds; all 1,585 Rust cases across 12 binaries; Python 1,285 and 1,284 passes, four expected failures; 777 and 103 paired comparisons |

The source publication additionally passed `python.dagrun.shard-0` (492 passes,
two skips) and the voice-call-drop browser node. The herdr-run comparison failed
with four divergences, then its permitted full focused rerun passed all 1,868
paired comparisons in 208 seconds (210.7 seconds including dependencies).
Both results are retained as a flaky check. Earlier failures included adjacent
read-only probe ordering, idle-tab output and one inherited-signal-mask
comparison; a proposed signal-mask race was not proven. The supplemental
long-message audio check failed distinct assertions on baseline and candidate,
then passed its focused candidate rerun in 54.2 seconds. These receipts are
source-publication evidence, not extra checks falsely attributed to the hotfix.

## Regressions and coverage limits

Typed stuck causes cover capacity, API rate limits, unsafe commands, safety
refusals, quota and stalled goals. Only opted-in capacity/rate limits can retry:
persisted intent precedes input, delays are 60/120/240 seconds, and the limit is
three attempts. Quota, safety and permission conditions notify only. Drafts,
menus, approvals, pauses, pending input, running turns and uncertain submissions
refuse automatic input. Actual managed-delivery tests exercise goal resume
followed by continue, and restart/uncertainty tests preserve the refusal state.
Three source mutation probes (quota retry, stale history, refusal retry) passed
their controls and were killed through the actual harness adapter.

Only the capacity error row was captured live; its footer was reconstructed.
All other provider-screen fixtures are explicitly synthetic. They exercise the
classifier but do not establish real-provider screen compatibility for every
cause. No live safety/permission bypass or quota retry was attempted.

The original Python shutdown bug let SIGTERM inherit Exception and be swallowed
by reconciliation retry. The deterministic regression failed before changing
it to BaseException; process, descendant and lock assertions remain unchanged.
Polling prompts and replies now share durable queue order. Four deterministic
cases cover fractional timestamps, timezone strings, tied clocks with reversed
enumeration, and backwards clocks. The first three failed before correction;
the focused polling/runtime suite passed 179 cases in 59.99 seconds. A proposed
streaming-cache change failed existing fairness assertions and was removed.

First live deployment exposed unsupported launch-spec provenance in session
following. The old positive fake returned no command line and missed this
branch. Two regression cases failed against the published source. The hotfix
uses existing anchor provenance and validates capture metadata before changing
the queue binding. All three session-follow tests pass, including interrupted
binding-first recovery and seven identity/refusal mutations.

The first hotfix gate's reminder fixture
`a_reply_the_transcript_holds_is_not_reported_missing_though_the_pane_does_not_show_it`
expected one typed reminder and saw none. Five isolated unchanged runs and its
Cargo rerun passed, followed by the complete unchanged-head gate and the rebased
gate. Both failed and passing logs are retained. Root cause is unknown; the lead
explicitly accepted recording this timing-sensitive failure as flaky.

## Live deployment

The fixed published release binary is installed independently of the private
worktree. Its user service is active and enabled, with exactly three opted-in
Codex lanes and default turn interruption explicitly disabled. The fresh report's
embedded checked_at is after restart: coordinator route correct, no probe error,
no unchecked agents, and all three session-follow rows error-free. Recovery is
empty because no automatic input was attempted during verification.

Stale registrations and an incomplete revive were retired with exact token/raw
record/journal pins through the supported CLI, preserving archived queues and
live runtimes. A later intermediate empty queue binding was repaired with normal
unregister/adopt/anchor/manage commands. The receipt confirms runtime preserved
and pane/tab not closed; the native process pin stayed unchanged. No records
were hand edited. Normal coordinator message delivery returned a delivered
receipt. Local deployment receipts, raw private logs and checksum manifests are
retained by the operator outside the reusable source tree.

The historical heal service exited 1 on October 15 at 2:01am ET after about
15 minutes. Its journal had rotated, so the exact old error cannot be recovered.
Its boot-time dead-session revival does not handle a live stalled goal; the new
watcher supplies that missing behavior. The old helper was not restarted blindly.

## Unrelated unchanged-main failures

On unchanged main `1f0a7a90`, the same-host canonical comparison of 22 earlier
failing nodes plus four dependencies ended with 17 failing nodes and nine
passing nodes in 1,564.2 seconds. This is node-level evidence; failing case
populations may differ. No whole-repository green result is claimed.

Reproduce each row with `nice -n 19 ionice -c3 common/bin/dagrun run --dag
validation.dag.yaml --selected TAG --max-steps 4 --max-cpus 4 --keep-going`.
Use matching legacy timezone data on hosts missing it.

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

Observed causes include valid opaque nsfs roots refused by workspace-tool mount
parsing, unavailable passwordless sudo, group-writable agent-teams fixtures,
missing PyYAML installed-file inventory, and media timing/contract assertions.
A minimal namespace parser patch was preserved separately and excluded here.

Dagrun shard 0, voice-call-drop browser, and herdr-run comparisons passed
unchanged main; candidate outcomes were checked separately above. The earlier
package-contract failure was caused by temporary diagnostic tests, since
removed, and is not misclassified as a baseline defect. The earlier disk-backed
Rust run naturally finished all 1,517 cases in 3,573 seconds; its broad overall
validation remains red. The published selected gates do not erase these limits.
