# `make` just runs ./setup (both python and rust), per repo convention.
.PHONY: all both py rs check check-deps test fmt clean

all: both

both:
	./setup both

py:
	./setup py

rs:
	./setup rs

# Lint/typecheck gates (what CI runs).
check: check-deps
	cd py && python3 -m mypy .
	cargo clippy --release --workspace --manifest-path rs/Cargo.toml -- -D warnings

# Stdlib-only smoke check: every console entrypoint must start cleanly (--help / --version /
# no-args) with ZERO optional dependencies. Catches the "optional dep imported at module scope
# takes down --help on a bare host" class of bug. Runs in the bare runtime env (no mypy needed).
check-deps:
	python3 scripts/check_deps.py

test:
	cd py && python3 -m pytest -q
	cargo test --release --workspace --manifest-path rs/Cargo.toml

fmt:
	cargo fmt --manifest-path rs/Cargo.toml

clean:
	rm -rf rs/target rs/bin/* bin
	find . -name __pycache__  -type d -prune -exec rm -rf {} + 2>/dev/null || true
	find . -name .mypy_cache  -type d -prune -exec rm -rf {} + 2>/dev/null || true
	find . -name .pytest_cache -type d -prune -exec rm -rf {} + 2>/dev/null || true
