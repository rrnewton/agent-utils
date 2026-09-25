# vibe-talk's own validation, runnable from this directory alone.
#
# WHY THIS EXISTS SEPARATELY. vibe-talk is a service, not one of the paired command-line tools, and
# it lives outside the `rs/` workspace with its own lockfile. The repository's `make check` and
# `make test` do not see it at all. The repository-wide `make validate` does -- it selects the
# vibe-talk group when vibe-talk changed -- but it is run from the root and reasons about the whole
# tree, and the loop you actually work in is "I changed one file in here, is this tool still good?"
#
# So: `make validate` at the root is the repository's gate, and `make validate` HERE is this tool's.
# The root one runs everything below by name, so the two cannot drift into different definitions of
# what validating vibe-talk means.

.PHONY: validate validate-boxed dag-check fmt check test page pwa-installability offline-cache-browser android-speech-self-test container-smoke clean

# The whole gate for this tool, cheapest and most likely to fail first.
#
# UNBOXED, and deliberately the default: it needs nothing but a Rust and a node toolchain, so it
# works on a fresh clone and on a CI runner with no cgroup delegation. `validate-boxed` is the
# better one where it can run.
validate: fmt check test dag-check android-speech-self-test

# The same suites, run through dagrun: per-step wall and CPU timeouts, memory caps, and
# process-tree teardown inside nested cgroups.
#
# PREFER THIS LOCALLY. It is the only path that bounds what the screenshot harness starts -- the
# teardown is setsid-proof, so a browser or a throwaway server is reaped with the step whether or
# not the script's own trap fired. One that was not once ran for six days holding a port.
#
# It also adds the screenshot suite, which plain `validate` leaves out because it needs a browser.
validate-boxed:
	cd .. && ./common/bin/dagrun run --dag vibe-talk/tests.dag.yaml

# `./common/bin/dagrun`, not a bare `dagrun` and not `./bin/dagrun`. This repository ships the tool
# it uses, so depending on the caller having installed one would fail for a reason that has nothing
# to do with this tool -- and `./bin/` is GITIGNORED, existing only after `./setup` has run, so
# pointing at it failed on a fresh clone for a reason that had nothing to do with it either.
#
# The graph itself, checked without running anything. Offline and instant.
#
# This used to prove almost nothing, and said so: `dagrun list` parsed the document and printed it,
# accepting an unknown field, a duplicate step tag, a missing dependency and a cycle -- the last of
# which CRASHED the runner rather than being refused. `#96 dagrun-loader-validation` moved all of
# those into the loader, so every inspection subcommand gets them: this now refuses a closed-schema
# violation, a duplicate tag, a missing dependency, a named cycle, and a demand above its cap, at
# exit 2, having executed nothing.
dag-check:
	cd .. && ./common/bin/dagrun list --dag vibe-talk/tests.dag.yaml >/dev/null

fmt:
	cargo fmt --all -- --check

check:
	cargo clippy --all-targets --locked -- -D warnings

# `cargo test` runs BOTH served pages' suites through `node --test` as well as the Rust tests, so
# this is not only the Rust half. Those suites FAIL rather than skip when node is missing, which is
# what stops a page regression from passing silently on a machine without a node toolchain.
test:
	cargo test --locked

# The page suites on their own, for the edit-run loop. Seconds rather than a full cargo test.
#
# EVERY suite, by glob, not a named file: this page serves two -- `/` and `/voice` -- and naming one
# of them is how the second comes to exist without ever being run.
page:
	node --test tests/js/*.test.mjs

# A real current Chromium, speaking CDP. This validates the installability decision the DOM-only
# page suite cannot make. It uses a loopback secure context by default; set VIBE_TALK_CHROMIUM to
# inspect with a particular Chrome executable, or pass --url directly to the script for a deployed
# server. It deliberately states that this is not physical-device launcher proof.
pwa-installability:
	python3 tests/pwa_installability.py

# `#18 offline-message-cache` in a real Chromium at a phone's size: localStorage across a real
# reload, a real fetch failure, the Cache Storage and service-worker registries, and the pill's
# layout against the view tabs -- what the DOM-only suite cannot observe. Loopback fake API only.
# Pass SCREENSHOTS=dir to keep one PNG per step.
offline-cache-browser:
	python3 tests/offline_cache_browser.py $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# The physical-device run is opt-in, but its signal/language evaluator has offline negative
# controls. Keep those controls in the ordinary gate so a broken harness cannot silently bless a
# later Android recording.
android-speech-self-test:
	python3 scripts/android-device-speech.py --self-test

# The LAUNCHER -- run.sh, the script the owner types -- as opposed to the image it starts, which
# the `container` CI job covers separately. Not part of `validate` because it needs a container
# engine and a minute of build. Run it before a deploy.
#
# This comment used to claim "CI runs it on every change" while no workflow ran it at all, and
# that claim is why nobody checked: run.sh had shipped inheriting the host's log driver, so on a
# rootless host defaulting to journald `--logs` printed nothing while --status promised retained,
# readable logs. The suite below catches exactly that and had no runner. It has one now, in
# .github/workflows/vibe-talk.yml, so the sentence is true rather than reassuring.
container-smoke:
	./scripts/test-run-sh.sh

clean:
	cargo clean
