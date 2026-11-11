# Branch triage, 2026-10-14

[opus 5.5] agent-utils is meant to stay main-only. On 2026-10-14 GitHub had no open pull requests
(61 in its history, all merged or closed) but 134 branches besides `main`. Every one of them is now
under `archived/`; `main` is the only other branch. Nothing was deleted: each branch was moved to
`archived/<original name>` (the three under the old `archive/` prefix became `archived/<rest>`),
with each deletion guarded by the commit it pointed at.

**Redaction.** Some original branch names contain a consuming project's name or a machine name.
In this record those parts read `consumer-a` and `host-a`; the branches on GitHub keep their names.

## How each branch was judged

- **Landed (60):** the tip is an ancestor of `main`, every commit has a patch-equivalent on `main`,
  or its pull request was merged.
- **The other 74** were read one by one against `main` (bee58027): *landed elsewhere* (the substance
  is on `main` in another form), *obsolete* (it targets code `main` replaced or removed), or
  *valuable* (a real change `main` still lacks). A second reader tried to refute every *valuable*
  verdict; three were overturned to *obsolete*. None was judged to be anyone's active work.

The ten *valuable* branches are tracked by seven issues, so the work is not lost with the branch:

| Issue | What is missing on main | Archived branches |
|---|---|---|
| #288 dagrun-capture-races | a perf acknowledgement written as the step ends is lost (both editions); Rust signals wprof at the window boundary without re-checking its exit | `archived/rescue/20260911-pr119-dagrun-results-cgroup-integrity-7d047119`, `archived/ci/dagrun-results-after-strict-reader`, `archived/ci/dagrun-results-cgroup-integrity` |
| #289 census-zombie-leader-threads | the slot-use census treats a zombie thread-group leader as gone while its other threads still hold files (fail-open) | `archived/salvage/agent-utils-wrkslots-final-review-20261010` |
| #290 speech-ticket-refresh | vibe-talk plays a prepared read-aloud URL after its server ticket expired, and never falls back | `archived/rescue/20260911-pr118-speech-ticket-refresh-c18f3d51`, `archived/speech-ticket-refresh` |
| #291 stop-lost-wakeup | a chat bridge stop can be lost between check and wait, and a stop that cancels a Herdr call exits 1 | `archived/chat-bridge/185-186-wip-7` |
| #292 reply-in-thinking-block | nothing reminds a coordinator of a delivered chat request it never answered | `archived/chat-bridge/196-wip-12` |
| #293 host-admission | a shared host resource-admission core (needs an owner decision first) | `archived/rescue/host-a/codex-host-admission-phase1-20260924-88c76cfb` |
| #294 dagrun-wall-timeout-multiplier | no per-machine scaling of wall-clock step ceilings (low priority) | `archived/rescue/20260918-b7366e40-independent-dagrun-wall-timeout-multiplier` |

## The 74 branches with commits not on main

| Archived as | Verdict | Why |
|---|---|---|
| `archived/coord-leftover-20261005/coord-sealfix-20261004` | obsolete | second reader: commit 1 was superseded by 3e042a9f and e5a2fe03, and commits 2-3 only harden a function commit 1 introduced. |
| `archived/salvage-pr89-6ffe5b70d6e841591d88d161810cda12d8657afa` | landed elsewhere | Ported to main as 6a3c2d7b (2026-08-20) 'safe-ci-dag-runner: make --keep-going keep going, and name what never ran'; |
| `archived/salvage-pr90-849ee30ceb01ebf42234c67ba143b0362d63827c` | landed elsewhere | 6a3c2d7b (2026-08-20) carries the behaviour. |
| `archived/chat-bridge/185-186-wip-7` | valuable | see #291 stop-lost-wakeup |
| `archived/chat-bridge/196-wip-12` | valuable | see #292 reply-in-thinking-block |
| `archived/chat-bridge/prompt-delivery-receipt-213-l1` | landed elsewhere | Main commit 229e0d46 (2026-10-04) has the same subject. |
| `archived/ci/dagrun-results-after-strict-reader` | valuable | see #288 dagrun-capture-races |
| `archived/ci/dagrun-results-cgroup-integrity` | valuable | see #288 dagrun-capture-races |
| `archived/claude/validate-tmp-wrkslots-20261001` | obsolete | Main has no 'validate_tmp' anywhere. |
| `archived/cml/validate-tmp-wrkslots-20261002` | obsolete | 841fa5b8 and fbf2def0 have the same patch ID (236397b3). |
| `archived/codex/wrkslots-multiline-execstart-20260911` | landed elsewhere | Main commit 29942c49 (2026-09-25, 'wrkslots: frame user systemd census with D-Bus JSON') removed the line-based parser altogether. |
| `archived/codex/wrkslots-multiline-execstart-rebased-20260911` | landed elsewhere | Superseded on main by 29942c49 (2026-09-25), which swapped the systemctl text parsing for strictly typed busctl JSON that keeps multiline values. |
| `archived/dagrun-only-should-take-the-transitive-dependency-closure` | landed elsewhere | Both commits landed squashed as main commit 064b4632 (2026-08-27, 'dagrun: select transitive dependencies by default'). |
| `archived/deploy/wrkslots-consented-release-d81ae95` | landed elsewhere | Every original is an ancestor of main: 27fbdf96 (owner-consented release), c225531c (keep retire-pending off owner-consented release unless opted in), cdbce992 (remove provably disposable nested repos), 40b0ce82 (judge every commi… |
| `archived/fix/dagrun-jobs-env-parity` | landed elsewhere | The jobs-env commit landed as main c254523b (2026-08-24), with the same subject and the same 27 files. |
| `archived/fix/herdr-lazy-resolution` | landed elsewhere | Main commit 35492f90 (2026-09-03) has the same title, body and file set (it changes 3 lines of cross/herdr_differential.py where the branch changes 1). |
| `archived/rescue/20260824-6ffe5b70d` | landed elsewhere | Landed as main 6a3c2d7b (2026-08-20, 'make --keep-going keep going, and name what never ran'). |
| `archived/rescue/20260824-849ee30ce` | landed elsewhere | Main commit 6a3c2d7b (2026-08-20), 'safe-ci-dag-runner: make --keep-going keep going, and name what never ran'. |
| `archived/rescue/20260825-0f0b1cb70` | landed elsewhere | Landed as main 34abde0b (2026-08-27, 'wrkslots: package the command and preserve lifecycle coverage'), which says it reconciles the two preserved wrkslots commits with main. |
| `archived/rescue/20260825-46a0bc18e` | landed elsewhere | Landed as main 9b85649e (2026-08-09, same subject). |
| `archived/rescue/20260825-rebased-main` | landed elsewhere | Main commit 34abde0b (2026-08-27), 'wrkslots: package the command and preserve lifecycle coverage', says it reconciles 'the two preserved wrkslots commits' with main, including configured remote selection. |
| `archived/rescue/20260911-c1afea1-wrkslots-ordinary-environment` | landed elsewhere | Main has 26e29bb5 'wrkslots: run trusted Git tests in the ordinary environment' (2026-09-11), the same patch. |
| `archived/rescue/20260911-pr118-speech-ticket-refresh-c18f3d51` | valuable | see #290 speech-ticket-refresh |
| `archived/rescue/20260911-pr119-dagrun-results-cgroup-integrity-7d047119` | valuable | see #288 dagrun-capture-races |
| `archived/rescue/20260918-084ae55d-herdr-worker-cap-original` | landed elsewhere | Landed as main e7eb2fbd (2026-08-10, same subject), with evidence added in 59da8697. |
| `archived/rescue/20260918-43d06199-wrkslots-ordinary-environment-post-120` | landed elsewhere | The combined diff 7cde6d10..43d06199 is byte-identical to main's 26e29bb5, which has the same parent, 7cde6d10. |
| `archived/rescue/20260918-440f1808-cc-dag-runner-bounds` | landed elsewhere | 82db5d07 (#81 runner-capture-memory-bound) lands all of this in both editions, and its message says parity was why the archived work was not landed. |
| `archived/rescue/20260918-b7366e40-independent-dagrun-wall-timeout-multiplier` | valuable | see #294 dagrun-wall-timeout-multiplier |
| `archived/rescue/20260918-f0554dcb-scheduler-run-to-completion` | landed elsewhere | 6a3c2d7b (2026-08-20, 'make --keep-going keep going'): --keep-going now keeps launching independent ready steps and skips only failed dependents, which is exactly run_to_completion. |
| `archived/rescue/auto-017c672` | landed elsewhere | dfefbdb8 (2026-08-04, 'remove deprecated --cgroups no-op flag (hard-error in both engines)'). |
| `archived/rescue/auto-0482691` | landed elsewhere | 14bc7cfc (2026-08-21, #78 host-memory-admission) and 7cd8768b ship admission in both editions: py/dagrun/admission.py, rs/dagrun/src/admission.rs, `run --admission` at py/dagrun/cli.py:668. |
| `archived/rescue/auto-0cb9576` | landed elsewhere | Landed as 4e21c85b (2026-08-20, 'make a dying step supervisor say so instead of wedging the run') and e43dd15c (Rust crash guards). |
| `archived/rescue/auto-0cfa1c9` | landed elsewhere | a8fc13ea (--unsafe-no-cgroups) and 068ac92f (SMALL default cap; |
| `archived/rescue/auto-15dbf80` | obsolete | second reader: the first commit of the core-allocator stack preserved elsewhere; main replaced the design with cpuset-alloc and measured IRQ-rate ranking. |
| `archived/rescue/auto-1f3ec98` | landed elsewhere | 14bc7cfc (#78 host-memory-admission) delivers whole-run admission before cgroup bring-up. |
| `archived/rescue/auto-22a401f` | landed elsewhere | Main has `run --admission WAIT_S` in both editions, from 14bc7cfc and 7cd8768b: py/dagrun/cli.py:668, 4005-4044 (ADMISSION_EXIT_CODE=4, MAX_ADMISSION_WAIT_S), rs/dagrun/src/cli.rs:1394. |
| `archived/rescue/auto-4be959a` | landed elsewhere | Memory admission landed as 14bc7cfc and 7cd8768b (2026-08-21, #78 host-memory-admission). |
| `archived/rescue/auto-61580d0` | landed elsewhere | The same field and comment are on main at py/dagrun/model.py:1002-1009, introduced by main commit 7950f3ac (2026-08-10, 'rescue: snapshot runner-known-failure-allowlist'). |
| `archived/rescue/auto-75dc72f` | landed elsewhere | Landed as main dccc1a83 (2026-08-20, 'stop reading an absent number as a measured zero'), which says it was recovered from this work and also fixes the Python engine. |
| `archived/rescue/auto-7dbcc96` | landed elsewhere | The same change landed on main as a8fc13ea (2026-08-04, same subject). |
| `archived/rescue/auto-7dc20de` | landed elsewhere | Landed as main 438535ee (2026-08-10, same subject), with follow-up adc9dac2. |
| `archived/rescue/auto-8d7d057` | landed elsewhere | Landed as main 2fb4df82 (2026-08-24, 'dagrun: enforce unboxed CPU budgets with a procfs floor'). |
| `archived/rescue/auto-a0ee7dd` | obsolete | second reader: the registry-derived manifest landed as 33f31043; the holder-file exclusivity targets the renamed runner and would need a fresh design in the core ledger. |
| `archived/rescue/auto-a3f6db0` | landed elsewhere | Landed as fddb44d2 (2026-08-03, 'Make PyYAML a truly-optional dependency'). |
| `archived/rescue/auto-c7992c3` | landed elsewhere | Landed in refined form as 6f991dd7 (2026-08-20, 'derive a step's wall backstop instead of baking 1800 into the graph') plus 54e93ab8 (floors the derived wall backstop). |
| `archived/rescue/auto-ff16cc3` | obsolete | Replaced by main's cgroup-cpuset allocator: f3c52676 (reservation ledger), e0427b8e (cpuset-alloc), 3fffd726 (`run --cores`). |
| `archived/rescue/host-a/backup-voice-on-3819a76-bf6e4905` | landed elsewhere | Both are on main with the same subjects and the same 15-file and 2-file diffs, differing only in context offsets. |
| `archived/rescue/host-a/backup-voice-on-763b748-30b8a28b` | landed elsewhere | b0071a97 landed as main d278e954 'vibe-talk: draw saved channel messages before the network answers'. |
| `archived/rescue/host-a/claude-wrkslots-scoped-finish-private-identity-20260925-d49cdc6e` | landed elsewhere | main py/wrkslots/cli.py:8745-8751: the scoped-finish shape check now refuses only validation_removal_proof (relaxed in 90d061d5 'wrkslots: release registry locks during validation census', the writer lineage this rescue was workin… |
| `archived/rescue/host-a/codex-agentctl-liveness-e6-8f7bfa1a` | obsolete | Main kept the flat record writer and took a different design. |
| `archived/rescue/host-a/codex-agentctl-liveness-pre-e6-42f832bc` | obsolete | Main never adopted this state model. |
| `archived/rescue/host-a/codex-host-admission-phase1-20260924-88c76cfb` | valuable | see #293 host-admission |
| `archived/rescue/host-a/reply-breaker-followup-3c539a35` | landed elsewhere | Main commit 30d7e315 (2026-09-28) has the same title and is a reviewed superset of this change: range-diff shows the same code plus extra bound comments, prune persistence, more tests and a check_documented_defaults pin. |
| `archived/rescue/host-a/reply-breaker-followup-c79c879a-45369f0b` | landed elsewhere | Main commit 30d7e315, 'agentctl: let normal reply bursts through the thread breaker and show held threads', is the later, reviewed revision. |
| `archived/rescue/host-a/wrkslots-liveness-verdict-binding-9b9f2af4` | landed elsewhere | main py/wrkslots/cli.py:17106 defines _bound_alive_verdict, and :17142-17179 _registered_liveness_state uses it with errors="replace". |
| `archived/rescue/host-a/wrkslots-salvage-transport-fallback-2dfc6544` | landed elsewhere | Main commit 6e29657c (2026-10-02), 'wrkslots: archive locally when salvage cannot reach the remote', squashes all four commits. |
| `archived/rescue/host-a/wrkslotsd-shadow-policy-pre-rebase-ce2a3b2-ce2a3b2a` | landed elsewhere | Main commit a83d23f5 (2026-09-25), 'wrkslots: add a fail-closed shadow policy and create-recovery checks', is the squashed, rebased version. |
| `archived/rescue/recon-validate-cleanup-batching-agent-utils-20260902` | landed elsewhere | Main commit c16b896b (2026-09-02), 'wrkslots: share process proof across cleanup sweeps', added VALIDATE_REMOVE_BATCH_LIMIT = 8 and the shared process census. |
| `archived/rescue/slots-20260928/ci-signal-au-history-20260919` | landed elsewhere | Main ded39ae0 'wrkslots: preserve authenticated finish recovery through census motion' (2026-09-20) says it 'Carries forward reviewed repairs 6f967247 and 2a1cb900 onto current main'. |
| `archived/rescue/slots-20260928/ci-signal-au-history-integration-20260920--branch-codex-ci-signal-au-history-20260919` | landed elsewhere | Carried onto main as ded39ae0, whose message names 6f967247 and 2a1cb900. |
| `archived/rescue/slots-20260928/consumer-a-ci-provenance-20260915--consumer-a-agent-utils` | obsolete | The producer shipped a different schema 6: the schema-5 fields plus a {"status":"delegated"} scorecard write-back, and no could_not_run_kind. |
| `archived/rescue/slots-20260928/consumer-a-ops-authority-20260915--wip` | landed elsewhere | git cherry marks 7b8e342b as patch-equivalent to main commit 11c925d0 (2026-09-16), which has the same title. |
| `archived/rescue/slots-20260928/consumer-a-verify-results-20260915--wip` | landed elsewhere | git cherry marks all eight code commits as patch-equivalent on main: 420d7874, d7d85cd1, 3b42a871, 2548dd93, 11c925d0, 0877f435, 8b3f3ac4 and 0e119a92 (all 2026-09-16). |
| `archived/rescue/slots-20260928/parent-receipt-snapshot-repair--consumer-a-agent-utils` | obsolete | The test-result commits landed as 420d7874. |
| `archived/rescue/slots-20260928/recon-2-plugin-workflow-fix--au-tail-wip` | landed elsewhere | Main commit 2afd7113 (2026-09-17), 'dagrun: carry a refusing step's own words into the structured-results refusal', implements the same idea with last_lines() (scheduler.rs:2688) and with_captured_tail() (scheduler.rs:3007), using… |
| `archived/salvage/20260827-ignored-agent-utils-dagrun-jobs-env` | landed elsewhere | Main commit 4254ae82 (2026-09-19) 'dagrun: validate build-width channels before shell startup' says it forward-ports 'the unique protections from https://github.com/rrnewton/agent-utils/pull/99 onto current source'. |
| `archived/salvage/agent-utils-wrkslots-final-review-20261010` | valuable | see #289 census-zombie-leader-threads |
| `archived/salvage/host-a/allow-completed-validation-checkouts/agent-utils-1-6965a4b80818` | obsolete | All 15 files are build or test caches: .pytest_cache/*, py/wrkslots/.pytest_cache/*, py/wrkslots/__pycache__/*.pyc and py/wrkslots/tests/__pycache__/*.pyc. |
| `archived/salvage/host-a/dagrun-research/agent-utils-1-c809c734f9e8` | obsolete | git diff --stat shows only binary .cpython-312.pyc files, 0 lines added or removed. |
| `archived/salvage/host-a/owner-watch-authority-parent/worktree-consumer-a-agent-utils-65501f0ec605` | obsolete | git diff --stat shows only binary .cpython-312.pyc files, 0 lines added or removed. |
| `archived/salvage/host-a/validate-cleanup-agent-utils/agent-utils-1-157850e5eb1d` | obsolete | `git show --name-only 157850e5` with cache paths filtered out leaves only .dagrun/profiles/*.csv(.lock), rs/dagrun/.dagrun/profiles/*, rs/.agent-utils-locks/cache.lock and `bin`. |
| `archived/speech-ticket-refresh` | valuable | see #290 speech-ticket-refresh |
| `archived/type-audit/tests-passed-exact` | obsolete | On the same day main took a different design: 0cdc8867 'dagrun: carry typed per-test results' (2026-08-29) added rs/dagrun/src/test_results.rs. |
| `archived/wip/tickhub-wrkslots-json` | landed elsewhere | Landed on main as 00d8395d (2026-10-03), which has the identical subject. |

## The 60 landed branches

`archived/chat-bridge/concise-20261001`, `archived/chat-bridge/elision-20261001`, `archived/ci/strict-validation-cleanup-reader`, `archived/codex/wrkslots-ignore-cache-root-porcelain`, `archived/dag-diagnostic-result`, `archived/dagrun-library-selection`, `archived/dagrun-step-result-manifests`, `archived/fix/soft-green-planner-policy`, `archived/mega-lander/mapped-root-lifecycle-partition`, `archived/remove-agent-identity-planner`, `archived/rescue/20260825-26a196270`, `archived/rescue/20260825-814944a22`, `archived/rescue/20260825-8857c0285`, `archived/rescue/20260825-929b363fe`, `archived/rescue/20260825-fa58b6eef`, `archived/rescue/auto-3c64150`, `archived/rescue/auto-41144e7`, `archived/rescue/auto-4796499`, `archived/rescue/auto-825cf28`, `archived/rescue/auto-b3ea08f`, `archived/rescue/auto-c84ef55`, `archived/rescue/auto-d9d9437`, `archived/rescue/auto-e533590`, `archived/rescue/auto-e884b57`, `archived/rescue/auto-ebd549b`, `archived/rescue/coord-20260923-agent-utils-tfa-parallel-fa085144`, `archived/rescue/host-a/chat-async-ack-pre-screen-20260928-68b3d7c6`, `archived/rescue/host-a/chat-async-ack-prefix-pre-feedback-20260928-3d18f2fb`, `archived/rescue/host-a/exact-boundary-gap-retry-20260928-b5ef1e5a`, `archived/rescue/host-a/reply-breaker-followup-c79c879a-v2-5b84ec4b`, `archived/rescue/reclaim-20260901T233832Z/sibling-path-boundary`, `archived/rescue/reclaim-20260901T233832Z/submodule-visibility-wrkslots`, `archived/rescue/reclaim-20260901T233832Z/wrkslots-cgroup-fix`, `archived/rescue/reclaim-20260901T233832Z/wrkslots-ignored-root-classifier`, `archived/rescue/reclaim-20260901T233832Z/wrkslots-ownerless-nested-submodule-recovery`, `archived/rescue/reclaim-20260901T233832Z/wrkslots-registry-repair`, `archived/rescue/slots-20260928/ci-signal-au-history-integration-20260920--branch-codex-ci-signal-au-history-validated-0009-20260920`, `archived/rescue/slots-20260928/consumer-a-ops-authority-20260915`, `archived/rescue/slots-20260928/consumer-a-verify-results-20260915`, `archived/rescue/slots-20260928/tick-2--consumer-a-agent-utils--branch-tick-2-tick-hub-drop-non-drop`, `archived/rescue/wrkslots-20260831-sibling-path-boundary`, `archived/rescue/wrkslots/host-a/submodule-visibility-wrkslots/agent-utils-1-0064548dfbf8`, `archived/rescue/wrkslots/host-a/wrkslots-cgroup-fix/agent-utils-1-2d2161736bbd`, `archived/rescue/wrkslots/host-a/wrkslots-ignored-root-classifier/agent-utils-1-1de97af45531`, `archived/rescue/wrkslots/host-a/wrkslots-ownerless-nested-submodule-recovery/agent-utils-1-bf09769daafc`, `archived/salvage/20260827-ignored-envcaus-agent-utils-d2fdfc830`, `archived/salvage/20260827-ignored-greenvalidate-agent-utils-b4951000c`, `archived/sibling-path-boundary`, `archived/slot-images`, `archived/tick-hub-drop-non-drop`, `archived/tickhub-ops/frozen-removal-path`, `archived/tickhub-ops/wrkslots-absent-batch-overlap`, `archived/tickhub-ops/wrkslots-exiting-pidns-init`, `archived/tickhub-reap-gate-on-cancel`, `archived/type-audit/dag-manifest-field`, `archived/validation-removal-proof`, `archived/wrkslots-pin-safety`, `archived/wrkslots-recover-absent-validate-rows`, `archived/wrkslots-registry-repair`, `archived/wrkslots-status-reports-each`
