#!/bin/sh
# Rust-only smoke: every Rust-primary dispatched tool answers --help.
# No interpreter is used (POSIX sh only), so this runs in the slim
# validation/rust-only container and in any shell-only environment.
# Python-primary tools are reported, not run: needing Python for them is
# the campaign's current meter reading, not a failure of this smoke.
set -u

BIN_DIR="${1:-/usr/local/bin}"
fail=0
for tool in agentctl herdr-agent dagrun cpuset-alloc tick-hub herdr-run pr-landing-planner; do
    if "$BIN_DIR/$tool" --help >/dev/null 2>&1; then
        echo "ok $tool"
    else
        echo "FAIL $tool: Rust-primary tool did not answer --help"
        fail=1
    fi
done
for tool in wrkslots wrkviz parallel-experiment-runner herdr-chat herdr-subagents; do
    echo "needs-python $tool"
done
echo "needs-python agentctl migrate (subcommand)"
echo "needs-python agentctl reset (subcommand)"
echo "needs-python agentctl repair (subcommand)"
echo "needs-python agentctl chat launch (subcommand)"
if command -v python3 >/dev/null 2>&1 || command -v python >/dev/null 2>&1; then
    echo "FAIL a Python interpreter is present on PATH in the Rust-only environment"
    fail=1
fi
exit "$fail"
