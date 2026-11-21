#!/bin/sh
# Opt-in TLC check of WrkslotsTransition.tla (#2043).  Never run by
# validation and never downloads anything: point TLA2TOOLS_JAR at a
# tla2tools.jar obtained from the official tlaplus release, e.g.
#   TLA2TOOLS_JAR=~/agent_scratch/tla/v1.7.4/tla2tools.jar \
#     py/wrkslots/spec/run_tlc.sh [config ...]
# With no arguments every WrkslotsTransition-*.cfg runs.  Each run
# happens in a scratch copy so TLC state files never enter the repo.
# Expected results (a FAIL below is evidence, not a script error):
#   current       PASS all listed invariants
#   fork          FAIL NoViol (forked-chain, #1807)
#   shapes        FAIL NotBuriedForkJournal (C1 buried fork + journal)
#   shapes2       FAIL NotTipForkDeadSeal (C1 fork at tip + dead seal)
#   pre1766       FAIL NoViol (deleted-while-locked, #1766)
#   allfix        PASS
#   sidecar       FAIL ReloadEqualsMemory (#2041 raw-form mismatch)
#   sidecar-form  FAIL NoFormStuck (#2041 refusal, RECOVERY REQUIRED)
#   sidecar-fixed PASS (NormalizeCompare: compare after expansion)
#   contention    FAIL NoAdoptBusy (starvation witness reachable);
#                 NoViol in the same run must not be the failure
#   contention-safety PASS (NoViol alone over the contention space)
#   liveness      FAIL OneRecoverCompletes on current main (C1 wedge:
#                 interrupted states no fair recover path completes)
#   liveness-fixed PASS (all fixes; completion may use the two
#                 documented hand exits, config revert / hand unlock)
#   globaljournal PASS: no disjoint create/adopt commits inside
#                 the live out-of-lock finish window (#1367 gate)
#   globaljournal-witness FAIL NoCreateBusyInWindow: the busy
#                 refusal itself is reachable in the window
#   globaljournal-control FAIL (WindowWriteHypothesis): with the
#                 gate removed the window commit appears, proving
#                 the check has teeth
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
jar=${TLA2TOOLS_JAR:?set TLA2TOOLS_JAR to a downloaded tla2tools.jar}
if [ -x /usr/lib/jvm/jre-21/bin/java ]; then
  java=/usr/lib/jvm/jre-21/bin/java
else
  java=${JAVA:-java}
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp "$here/WrkslotsTransition.tla" "$work/"
if [ $# -gt 0 ]; then configs=$*; else
  configs=$(cd "$here" && ls WrkslotsTransition-*.cfg)
fi
for cfg in $configs; do
  name=$(basename "$cfg" .cfg)
  cp "$here/$cfg" "$work/$name.cfg"
  echo "=== $name ==="
  (cd "$work" && "$java" -XX:+UseParallelGC -jar "$jar" \
    -config "$name.cfg" WrkslotsTransition.tla) 2>&1 \
    | grep -E "distinct states|depth of|Error:|Invariant|violated|Finished|states generated" \
    | head -8 || true
done
