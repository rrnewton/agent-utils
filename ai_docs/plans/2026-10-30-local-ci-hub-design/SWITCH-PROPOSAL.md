# First consumer-a switch: read paths only (proposal, not applied)

Principle: ci-hub remains the authority. local-ci-hub answers in
shadow behind a flag; every answer is compared and logged; the
exit code and verdict a caller acts on always come from ci-hub.

## Scope

Only two read paths: `validate-status --sha` verdicts and
`ledger qualified-rows`. No writer, publisher, admission, or
landing path changes.

## Exact wrapper diff (consumer-a, for the coordinator to apply)

In `ci-hub/ci-hub` (the shell front door), after the existing
`ensure-mirror-reaches` step and before `exec rust-script`:

```diff
+# Shadow read path (phase 3, read-only): when CI_HUB_SHADOW_LOCAL_CI_HUB=1
+# and the command is a receipt query, also ask local-ci-hub and log any
+# disagreement to ignored/ci-hub/shadow.log. ci-hub's own output and
+# exit code are unchanged and remain authoritative.
+if [[ "${CI_HUB_SHADOW_LOCAL_CI_HUB:-0}" == "1" && "$1" == "validate-status" ]]; then
+  ( agent-utils/bin/local-ci-hub --config local-ci-hub.toml receipt show "${@: -1}" \
+      >> ignored/ci-hub/shadow.log 2>&1 || true ) &
+fi
```

And add the committed config `local-ci-hub.toml` (drafted beside
this file as `consumer-a-local-ci-hub.toml`, with
`[ledger] evidence_root` pointing at the retained evidence archive
and `[receipt] digest_command` naming the ci-hub receipt-digest
entry point, so one canonical digest exists).

## Promotion criteria (before any authority change)

1. Zero "looser" verdicts across the full ledger (met: 0 of
   2,673; 209 stricter, each class explained in SHADOW.md).
2. Shadow log over at least one week of live queries with zero
   unexplained disagreements.
3. Digest answers byte-identical via the delegated Rust digest.
