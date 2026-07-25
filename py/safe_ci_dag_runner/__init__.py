"""safe-ci-dag-runner: safely run a DAG of CI/build steps.

Two-level cgroup boxing, memory-aware concurrency, and always-on CPU/mem/ambient-load
logging. The full public API (Step, ResourceHint, DagConfig, run_dag, ...) is being
ported from a mature reference implementation; for now this package exposes the version
and a CLI entry point, and CI keeps it in lockstep with the Rust build.
"""

from __future__ import annotations

__version__: str = "0.1.0"

__all__ = ["__version__"]
