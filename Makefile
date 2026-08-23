# `make` just runs ./setup (both python and rust), per repo convention.
.PHONY: all both py rs check check-deps check-test-suite-selector mypy test cross check-packages \
	check-python-packages check-rust-packages fmt clean

all: both

both:
	./setup both

py:
	./setup py

rs:
	./setup rs

# One fast repository-wide Python typing gate. `--strict` is repeated on the command line so the
# contract stays visible even if a local config is reorganized; check_no_any rejects the escape
# hatch of importing/referencing typing.Any at a dynamic boundary instead of narrowing to object.
mypy:
	python3 -m mypy --config-file py/pyproject.toml --strict --disallow-any-explicit --disallow-any-decorated .
	python3 scripts/check_no_any.py .

# Lint/typecheck gates (what CI runs).
check: check-deps mypy check-test-suite-selector check-validate-selector
	@host_target="$$(rustc -vV | sed -n 's/^host: //p')"; \
	test -n "$$host_target"; \
	cargo clippy --release --workspace --all-targets --manifest-path rs/Cargo.toml \
		--target "$$host_target" -- -D warnings

# Stdlib-only smoke check: every console entrypoint must start cleanly (--help / --version /
# no-args) with ZERO optional dependencies. Catches the "optional dep imported at module scope
# takes down --help on a bare host" class of bug. Runs in the bare runtime env (no mypy needed).
check-deps:
	python3 scripts/check_deps.py

TEST_SUITE ?= all
ifneq ($(words $(TEST_SUITE)),1)
$(error TEST_SUITE must be exactly one of: all, python, rust)
endif
ifeq ($(filter all python rust,$(TEST_SUITE)),)
$(error TEST_SUITE must be exactly one of: all, python, rust)
endif

# Run only the checks the current change can actually affect, and REPORT the rest as skipped.
# `--all` is the whole contract and is what CI uses; selection is for the edit-run loop.
# See scripts/validate.py for the path -> check mapping and why each rule is what it is.
validate:
	python3 scripts/validate.py

validate-all:
	python3 scripts/validate.py --all

# The selector must not be able to silently under-run. Offline, no build, no network.
check-validate-selector:
	python3 scripts/validate.py --self-test

check-test-suite-selector:
	@for valid in all python rust; do \
		$(MAKE) --no-print-directory -s -n test TEST_SUITE="$$valid" >/dev/null; \
	done
	@for invalid in bogus '%' 'p%' 'python rust'; do \
		if $(MAKE) --no-print-directory -s -n test TEST_SUITE="$$invalid" >/dev/null 2>&1; then \
			echo "test suite selector accepted invalid value: $$invalid" >&2; \
			exit 1; \
		fi; \
	done

test:
ifneq ($(TEST_SUITE),rust)
	cd py && python3 -m pytest -q
endif
ifneq ($(TEST_SUITE),python)
	@host_target="$$(rustc -vV | sed -n 's/^host: //p')"; \
	test -n "$$host_target"; \
	cargo test --release --workspace --manifest-path rs/Cargo.toml \
		--target "$$host_target"
endif

# Cross-language observable behavior for every paired command.
cross:
	python3 cross/differential.py --tool all

# Build and smoke the artifacts users actually install, one distribution at a time.
check-python-packages:
	python3 scripts/check_python_packages.py

check-rust-packages:
	python3 scripts/check_rust_packages.py

check-packages: check-python-packages check-rust-packages

fmt:
	cargo fmt --manifest-path rs/Cargo.toml

clean:
	mkdir -p rs/.agent-utils-locks
	flock rs/.agent-utils-locks/cache.lock rm -rf -- rs/target
	rm -f -- rs/bin/dagrun.provenance rs/bin/cpuset-alloc.provenance \
		rs/bin/tick-hub.provenance rs/bin/pr-landing-planner.provenance
	rm -f -- bin
	find . -name __pycache__  -type d -prune -exec rm -rf {} + 2>/dev/null || true
	find . -name .mypy_cache  -type d -prune -exec rm -rf {} + 2>/dev/null || true
	find . -name .pytest_cache -type d -prune -exec rm -rf {} + 2>/dev/null || true
