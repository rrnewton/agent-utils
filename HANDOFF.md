# Frozen continuation

Owned branch: read git symbolic-ref --short HEAD in this registered flat AU slot.
HEAD: 4e11fa6fa1c6b9a97e5c1301e846fcb7f958ecb8
TREE: ee3fd4685a60bd003efae868045b2ba66f895273
Latest delta base: 531ce9fa6920bd6401c6d67eaa6e2c3e04a73874
Original base: 14589e875c08e458d2f8af5a6f78b12128d5a367
Source frozen; Git status has only this untracked HANDOFF.md. No publication.
All prior source checkpoints, failed runs, raw reviews and artifacts remain intact.
Older full continuation is archived under profile-capture-followup-v2/
 handoff-before-4e11-freeze.source. Paths below are relative to
 target/typed-attempt-results/ unless explicitly stated otherwise.

Latest one-file test correction: +31/-2 in py/tests/test_profile_capture.py.
Diff SHA256: 53a139c8e25e73c37433a3e98e2c71ab8ae573577c0a8ce4c0829ce36210ae1d.
Test SHA256: 94c5e6a43aa0a8278ab1e9fb0f550873a2893bb2c92e495f3cb685e47744cd13.
Production is unchanged. This explicitly removes the incidental roughly20ms
export deadline at the live callback checkpoint, using the preexisting2s
finalization grace instead. Exact data/trace bytes are required after actual
finalization. Real delegated INT success is separately required per trial at
the existing90ms live-workload checkpoint, with no early call-start and exact
final2 count. Call-start is not completion/kernel-delivery time. The checkpoint
is not a hard realtime bound. Width8,2 trials,20ms selected window,80/90/40ms
phases, readiness/private modes and existing artifact assertions remain.

Final focused evidence: profile-capture-followup-v2/. Actual scopeexit0 in11.761s,
22 profile/CLI cases passed, delayed export both trials passed exact bytes,
missing/late second stop failed live checkpoint, duplicate second real stop
after that checkpoint failed final count. Strict mypy/no-Any/client/whitespace
all passed. Committed source hashes match before/after tested bytes. Prior
draft is retained by refs/archive/profile-capture-initial-20260916.
Root and independent native exact-head reviews APPROVE. Their certificates
explicitly disclose the timing-contract correction, not an all-NO claim.

Independent Claude exact-delta review APPROVE: PID365930 /exec78417, actual
exit0 in128.359s,3 turns, claude-opus-5[1m], no timeout/denials. Dedicated
session f5804c12-2a02-4b75-9f5b-8e76265becca. Read-only plan mode,600s bound.
Verbatim certificate plus separate factual addendum preserve limitations:
no hard realtime90ms bound; removed export deadline is an explicit contract
change; real boxed controls previously passed; fake binary receives actual
OS signals but does not certify a real profiler implementation. Nonblocking
P-1 suggests an early-stop negative for the new call-start lower bound. Artifacts:
 profile-capture-followup-v2/claude-review/{prompt.txt,invocation.json,
 stdout.json,stderr.log,status.json}. Inspect actual terminal status before
parsing or claiming a verdict. Source remains frozen throughout review.

Ordinary full attempt8 FAILED at base531: makeexit2 in575.633s, scopeexit2
in576.171s. First Python2767 passed/1 failed/2 deliberate fixture warnings;
later groups unrun. Failure was empty wprof.data at trial2 live checkpoint.
Original cause remains uncertain: late INT and slow export both fit it.
Archived original manifest/log show final exact artifacts and profiler0 after
teardown. Full actual logs/resources/status remain official-validation-8/.
Original fixture archive and extracted manifest/log are preserved in
 profile-capture-diagnosis-531/. Original test and production were byte-identical
at14589 and531. Untouched test passed once;150ms delayed-export injection failed
the old byte assertion. That first collector itself failed due duplicate glob
through a current symlink; original failure and separate corrected extraction
remain intact. Do not report that collector as a passing scope.

Earlier ordinary attempts1-7 remain retained. Attempt7 was authored handoff
hygiene failure before tests; neutral wording corrected without exemption.
Attempt6 passed all Python partitions2768/555/98 then failed two Rust test
expectations, corrected and independently approved in531. No ordinary full
receipt exists yet. Prior all-group checks must not be assembled into one.

Successful split remaining-group PREFLIGHT at531 is retained: original first4
commands passed (Rust900 tests, differential mypy, cross1492 checks,7 Python
and4 Rust packages); browser setup interruption preserved as actualscope2.
Continuation7 commands passed, scope0/126.525s:38 browser tests, benchmark,
page-tool fmt/Clippy/Cargo/439 JS cases/DAG list. Exact11-command/source proof:
 boxed-execution-proof-531/split-preflight-proof.json. Actual existing CPU/OOM
boxed controls passed with --nocapture/no SKIP,4/4 neighbor artifacts preserved;
scope0/4.326s. Exit0-plus-kills regression is separate planted-counter evidence,
not a claim about that real kernel OOM experiment. These checks concern source
unchanged by the latest test-only correction; they are not a full receipt.

Required runtime for the next ordinary full: installed Node24.19.0 full bin
/nix/store/glcp73hgagq2b24i80jlgbvj28vdb6kk-nodejs-24.19.0/bin first in PATH.
PLAYWRIGHT_BROWSERS_PATH and npm_config_cache use task-local v2 setup caches,
identical to successful browser setup/continuation. Node canonical identity,
Playwright1.61.0 and Chromium build1228 are recorded in browser-runtime-setup-531-v4/.
Original failed setup attempts and proven-new generated lock bytes remain.
No engine suppression, dependency downgrade, source ignore or selector changed.

LIVE ordinary full attempt9, explicitly acknowledged by coordinator after both
exact-source approvals and the early-stop negative: PID573274 /exec17938,
unit au-combined-4e11fa6f-validate-9.scope,
InvocationIDdf33ff7112df48f7aecb17511a825c6e. Resource observer PID607266 /exec44637.
Evidence official-validation-9/. Same make validate/all7 groups,4CPU/16GiB/
zero swap/8192 tasks/7200s, supported Node/browser environment,no lineage override.
Hygiene/docs/build/global mypy/no-Any/Clippy passed; first Python partition running.
No terminal full result yet. Early-stop control required the exact new >= check
to fail on a second call roughly20ms after launch versus required70ms; actual
child1, discriminator scope0/1.231s, committed bytes unchanged. Evidence:
 profile-capture-followup-v2/early-stop-control/. Brief same-session read-only
Claude correction acknowledgement runs concurrently, PID607523 /exec4906,
under profile-capture-followup-v2/claude-correction-ack/. It is not another full
audit or payload run. Check actual terminal statuses before any completion claim. No redundant broader checks after
its full result absent a new failure or source change. Publication requires
full PASS, coordinator readback and fresh remote identity; not worker action.
No paired-run production cutover or downstream pin is activated here.
