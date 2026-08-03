"""parallel-experiment-runner: run N concurrent seed-sweep workers BOXED under safe-ci-dag-runner.

This is a thin, additive generalization of ``safe-ci-dag-runner`` for the seed-sweep shape: a
single command template with a ``{seed}`` placeholder, run over a range of seeds, with each worker
contained by the SAME two-level cgroup-v2 boxing CI steps use — real per-worker ``memory.max`` /
``cpu.max`` / CPU-second / wall caps and a clean ``cgroup.kill`` on breach.

The four hard requirements, each realized by a specific piece:

1. CPU-TIME (not wall) budgets -> :attr:`model.WorkerLimits.cpu_timeout_s` lowered onto
   ``safe_ci_dag_runner.Step.cpu_timeout`` (measured from the cgroup ``cpu.stat``; ``None`` = UNSET,
   never wall-derived).
2. DECLARED + ENFORCED concurrency -> :mod:`calibrate` resolves a width from the lane, live
   capacity, and measured per-worker footprint; the round runs EXACTLY that many workers.
3. Up-front ESTIMATE + measured ACTUAL -> :class:`profile.ProfileStore` (derived estimate or an
   honest UNSET) and :class:`model.RoundResult` / :class:`execute.SweepResult` (measured actuals).
4. Clean kill NAMING the breach -> :func:`execute._breach_message` + the breach statuses in
   :mod:`model`.

The programmatic entry point is :func:`execute.run_sweep`; the CLI is :func:`cli.main`.
"""

from __future__ import annotations

__version__: str = "0.1.0"

__all__ = ["__version__"]
