# `make` just runs ./setup (both python and rust), per repo convention.
.PHONY: all both py rs check test fmt clean

all: both

both:
	./setup both

py:
	./setup py

rs:
	./setup rs

# Lint/typecheck gates (what CI runs).
check:
	cd py && python3 -m mypy .
	cargo clippy --release --workspace --manifest-path rs/Cargo.toml -- -D warnings

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
