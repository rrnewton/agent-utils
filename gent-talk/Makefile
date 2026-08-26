# gent-talk's own validation, runnable from this directory alone.
#
# WHY THIS EXISTS SEPARATELY. gent-talk is a service, not one of the paired command-line tools, and
# it lives outside the `rs/` workspace with its own lockfile. The repository's `make check` and
# `make test` do not see it at all. The repository-wide `make validate` does -- it selects the
# gent-talk group when gent-talk changed -- but it is run from the root and reasons about the whole
# tree, and the loop you actually work in is "I changed one file in here, is this tool still good?"
#
# So: `make validate` at the root is the repository's gate, and `make validate` HERE is this tool's.
# The root one runs everything below by name, so the two cannot drift into different definitions of
# what validating gent-talk means.

.PHONY: validate validate-boxed dag-check fmt check test page container-smoke clean

# The whole gate for this tool, cheapest and most likely to fail first.
#
# UNBOXED, and deliberately the default: it needs nothing but a Rust and a node toolchain, so it
# works on a fresh clone and on a CI runner with no cgroup delegation. `validate-boxed` is the
# better one where it can run.
validate: fmt check test dag-check

# The same suites, run through dagrun: per-step wall and CPU timeouts, memory caps, and
# process-tree teardown inside nested cgroups.
#
# PREFER THIS LOCALLY. It is the only path that bounds what the screenshot harness starts -- the
# teardown is setsid-proof, so a browser or a throwaway server is reaped with the step whether or
# not the script's own trap fired. One that was not once ran for six days holding a port.
#
# It also adds the screenshot suite, which plain `validate` leaves out because it needs a browser.
validate-boxed:
	cd .. && ./bin/dagrun run --dag gent-talk/tests.dag.yaml

# `./bin/dagrun`, not a bare `dagrun`: this repository ships the tool it uses, and depending on
# the caller having installed it would make this tool's gate fail for a reason that has nothing to
# do with this tool.
#
# The graph itself, loaded without running anything. Offline and instant.
#
# IT PROVES LESS THAN IT LOOKS LIKE IT PROVES, and that is worth saying here rather than letting
# the next person assume otherwise: `dagrun list` parses the document and prints it. Measured, it
# does NOT reject an unknown field, a duplicate step tag, a missing dependency, or a cycle -- the
# user guide's "rejected before execution" holds only inside `run`, and not for all of those even
# there. So this catches a file that stopped being loadable YAML or lost a required field, and
# nothing about the shape of the graph. See the dagrun issue filed alongside this.
dag-check:
	cd .. && ./bin/dagrun list --dag gent-talk/tests.dag.yaml >/dev/null

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

# The image is the deployment artifact, and it is not part of `validate` because it needs a
# container engine and a minute of build. CI runs it on every change; run it here before a deploy.
container-smoke:
	./scripts/test-run-sh.sh

clean:
	cargo clean
