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
check: check-deps mypy check-test-suite-selector check-validate-selector check-client-names \
	check-documented-defaults
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

# agent-utils is reusable; the projects that consume it must not be named anywhere in the tree.
check-client-names:
	python3 scripts/check_client_names.py --self-test
	python3 scripts/check_client_names.py

# A number a guide states as a default is a promise about the code. Compare the two artifacts:
# the value PARSED OUT OF THE PROSE against the constant, never a sentence formatted from it.
check-documented-defaults:
	python3 scripts/check_documented_defaults.py --self-test
	python3 scripts/check_documented_defaults.py

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

# test_lifecycle calls `lsof +D` and then inspects /proc before removing a slot.
# On a shared host both operations walk every unrelated process: measured with
# 3,754 processes, one empty two-directory test slot took about 30 seconds even
# though no process used it. A PID namespace keeps the real lsof and /proc checks
# while limiting them to the test and its children. Four tests below manage child
# PIDs whose signal/reap behaviour must remain host-visible. The root-helper test
# must instead exercise the initial identity user namespace. Run all five in the
# ordinary environment. If unprivileged namespaces are unavailable, run the
# original single pytest command; availability changes cost, never coverage.
test:
ifneq ($(TEST_SUITE),rust)
	@cd py && \
	if command -v unshare >/dev/null 2>&1 \
		&& unshare --user --map-root-user --pid --fork --mount-proc true >/dev/null 2>&1; then \
		python3 -m pytest -q --ignore=wrkslots/tests/test_lifecycle.py && \
		unshare --user --map-root-user --pid --fork --mount-proc \
			python3 -m pytest -q -c pyproject.toml --rootdir=. wrkslots/tests/test_lifecycle.py \
			-k 'not test_lock_conflict_refuses_without_state_change and not test_process_entering_after_final_scan_before_path_move_is_not_deleted and not test_adopt_refuses_pid_outside_invoking_process_ancestry and not test_remove_refuses_live_process_using_slot and not test_root_owned_executable_accepts_host_root_helper' && \
		python3 -m pytest -q \
			wrkslots/tests/test_lifecycle.py::test_lock_conflict_refuses_without_state_change \
			wrkslots/tests/test_lifecycle.py::test_process_entering_after_final_scan_before_path_move_is_not_deleted \
			wrkslots/tests/test_lifecycle.py::test_adopt_refuses_pid_outside_invoking_process_ancestry \
			wrkslots/tests/test_lifecycle.py::test_remove_refuses_live_process_using_slot \
			wrkslots/tests/test_lifecycle.py::test_root_owned_executable_accepts_host_root_helper; \
	else \
		python3 -m pytest -q; \
	fi
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
