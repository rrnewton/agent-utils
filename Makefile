# `make` just runs ./setup (both python and rust), per repo convention.
.PHONY: all both py rs check check-deps mypy test cross check-packages \
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
check: check-deps mypy
	@host_target="$$(rustc -vV | sed -n 's/^host: //p')"; \
	test -n "$$host_target"; \
	cargo clippy --release --workspace --manifest-path rs/Cargo.toml \
		--target "$$host_target" -- -D warnings

# Stdlib-only smoke check: every console entrypoint must start cleanly (--help / --version /
# no-args) with ZERO optional dependencies. Catches the "optional dep imported at module scope
# takes down --help on a bare host" class of bug. Runs in the bare runtime env (no mypy needed).
check-deps:
	python3 scripts/check_deps.py

test:
	cd py && python3 -m pytest -q
	cargo test --release --workspace --manifest-path rs/Cargo.toml

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
	rm -f -- rs/bin/safe-ci-dag-runner.provenance rs/bin/cpuset-alloc.provenance \
		rs/bin/tick-hub.provenance rs/bin/pr-landing-planner.provenance
	rm -f -- bin
	find . -name __pycache__  -type d -prune -exec rm -rf {} + 2>/dev/null || true
	find . -name .mypy_cache  -type d -prune -exec rm -rf {} + 2>/dev/null || true
	find . -name .pytest_cache -type d -prune -exec rm -rf {} + 2>/dev/null || true
