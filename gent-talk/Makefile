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

.PHONY: validate fmt check test page container-smoke clean

# The whole gate for this tool, cheapest and most likely to fail first.
validate: fmt check test

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
