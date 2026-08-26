# Validate THIS tool, from THIS directory, with every step resource-boxed.
#
# `make validate` in the repository root selects checks by which paths changed and runs the
# repository's whole contract. This file is the other half of that: the tool's own suite, runnable
# without knowing anything about the repository around it, by someone standing in the tool's
# source directory. The two do not disagree -- the root's `timeline-browser` group runs the same
# browser and benchmark targets defined here -- but they answer different questions. "Is my change
# to this tool sound" should not require running gent-talk's Rust suite to find out.
#
# EVERY step is boxed. `dagrun box` puts a command under cgroup-v2 CPU, memory and wall limits, so
# a test that leaks memory or wedges is killed and reported rather than taking the machine with
# it. That matters most for the browser steps: Chromium is the largest thing this repository
# starts, a Playwright run that loses its browser leaves a process holding a gigabyte, and the
# benchmark deliberately drives a heavy page. An unboxed hang here is indistinguishable from slow
# work, which is exactly the failure mode the fetcher's own hang taught us to stop tolerating.
#
# The limits are deliberately GENEROUS. They exist to tell finite from infinite, not fast from
# slow: a loaded machine may legitimately take several times the usual wall clock, and a bound
# tight enough to catch "slower than expected" buys a flaky gate. Every one of them is far above
# the measured cost of the step it bounds, and each says so beside itself.

REPO_ROOT := $(shell cd ../.. && pwd)
DAGRUN    ?= $(REPO_ROOT)/bin/dagrun
PYTHON    ?= python3
BROWSER   := $(REPO_ROOT)/py/tests/js/browser

# Extra flags for every box. Empty by default, so a developer gets real cgroup enforcement and a
# hard failure if the machine cannot provide it. CI sets `--allow-cgroup-failure`, because a
# hosted runner may not delegate a cgroup and refusing to run there would trade a resource bound
# for no test at all. That degradation is not silent and not total: `dagrun capabilities` reports
# `wall_timeout` and `run_timeout` as available UNCONTAINED, so the ceiling that catches a hang --
# the one the fetcher taught us to care about -- still holds. What is lost is the memory and CPU
# bound, which is the half a hosted runner already enforces its own way.
BOX_FLAGS ?=

# `dagrun box` needs the command after `--`. Wrapped in a variable so every step below reads as
# the command it runs rather than as boxing ceremony.
BOX = $(DAGRUN) box --label $(1) --mem $(2) --timeout $(3) --cores $(4) $(BOX_FLAGS) --

.DEFAULT_GOAL := help
.PHONY: help validate validate-fast unit browser benchmark typecheck clean-browser

help:
	@printf '%s\n' \
	  'Validate the agent timeline tool, from its own directory. Every step is resource-boxed.' \
	  '' \
	  '  make validate        typecheck + unit + browser + benchmark   (the full tool contract)' \
	  '  make validate-fast   typecheck + unit                         (no browser; the edit loop)' \
	  '' \
	  '  make typecheck       mypy over the package and its tests' \
	  '  make unit            the Python suite for this tool only' \
	  '  make browser         the Playwright suite against the real static assets' \
	  '  make benchmark       zoom/pan redraw timings, enforced against a budget' \
	  '' \
	  '  make clean-browser   remove Playwright test-results and traces' \
	  '' \
	  'The repository-wide contract is `make validate` in $(REPO_ROOT).'

validate: typecheck unit browser benchmark
	@printf '\n\033[32mtimeline: OK\033[0m  (typecheck, unit, browser, benchmark -- all boxed)\n'

validate-fast: typecheck unit
	@printf '\n\033[32mtimeline: OK (fast)\033[0m  no browser; run `make validate` before pushing\n'

# 4 GiB / 10 min / 4 cores. mypy over this package peaks near 1 GiB and finishes in about a
# minute; the ceiling is the order of magnitude above that which distinguishes a wedge.
typecheck:
	@printf '\n\033[1m==> typecheck\033[0m\n'
	cd $(REPO_ROOT) && $(call BOX,timeline-typecheck,4G,600,4) \
	  $(PYTHON) -m mypy py/agent_team_timeline py/tests

# 8 GiB / 30 min / 8 cores. The suite builds real archives in tmpdirs, so it is I/O heavy rather
# than CPU heavy; the memory ceiling is what actually matters, and it is eight times the observed
# peak.
unit:
	@printf '\n\033[1m==> unit\033[0m\n'
	cd $(REPO_ROOT)/py && $(call BOX,timeline-unit,8G,1800,8) \
	  $(PYTHON) -m pytest tests -q -k 'timeline or transcript or query or seekable or build_store'

# 6 GiB / 15 min / 4 cores. Chromium is the reason this target is boxed at all: a lost browser
# holds hundreds of megabytes and a wedged page holds it forever.
browser:
	@printf '\n\033[1m==> browser\033[0m\n'
	cd $(BROWSER) && $(call BOX,timeline-browser,6G,900,4) \
	  npx playwright test --reporter=line

# 6 GiB / 15 min / 4 cores, same reasoning as `browser`. This one is a pass/fail check AND a
# measurement: it prints the redraw timings it collected and fails if any breaches the budget.
# `dagrun box` summarises a passing step's stdout to a single line, so the measurements are read
# back from the file the check writes rather than from its console output. Printed here, after
# the box, so `make benchmark` shows its numbers whether it passed or failed.
benchmark:
	@printf '\n\033[1m==> benchmark\033[0m\n'
	cd $(BROWSER) && $(call BOX,timeline-benchmark,6G,900,4) \
	  node --test benchmark-budget.test.cjs
	@printf '\n'; cat $(BROWSER)/benchmark-report.txt

clean-browser:
	rm -rf $(BROWSER)/test-results
