"""safe-ci-dag-runner: safely run a DAG of CI/build steps.

Define your graph as :class:`Step` values (each carrying a :class:`ResourceHint`) in a
:class:`DagConfig`, then call :func:`run_dag`. The runner gives you:

* two-level cgroup CPU/memory boxing with fast, zombie-free teardown (Linux cgroup-v2),
* memory-aware concurrency (largest ``-j`` that fits a RAM budget),
* always-on per-step + whole-run CPU/mem/ambient-load logging,
* Graphviz + ASCII DAG visualization.

Containment and metrics are pluggable via the :class:`CgroupManager` / :class:`MetricsSink`
protocols; use :class:`NoopCgroups` on non-Linux / when you don't want boxing.

    from safe_ci_dag_runner import Step, ResourceHint, DagConfig, run_dag, to_ascii
    cfg = DagConfig(steps=(Step("build", "app", "compile", "make build"),))
    print(to_ascii(cfg))
    result = run_dag(cfg, jobs=4)   # RunResult; result.ok is the overall pass/fail
"""

from __future__ import annotations

from safe_ci_dag_runner.ambient import (
    AmbientSnapshot,
    ambient_bucket,
    capture_ambient_snapshot,
)
from safe_ci_dag_runner.analyze import summarize
from safe_ci_dag_runner.cgroup import Cgroups, CgroupEnforcementKind, NoopCgroups
from safe_ci_dag_runner.estimates import (
    DEFAULT_MIN_SAMPLES,
    Plan,
    PlanEntry,
    Planner,
    StepSamples,
    apply_plan_to_config,
    build_plan,
    feedback_identity,
    load_step_samples,
    plan_to_json,
    plan_to_text,
)
from safe_ci_dag_runner.io import (
    DagJsonError,
    dag_from_json,
    dag_from_yaml,
    dag_to_json,
    dag_to_yaml,
)
from safe_ci_dag_runner.model import (
    DEFAULT_JOBS_FLAG,
    DEFAULT_STEP_TIMEOUT,
    DagConfig,
    ResourceHint,
    Step,
    StepClass,
    command_with_inner_jobs,
    effective_jobs_flag,
    preferred_inner_jobs,
    render_jobs_flag,
    step_classification,
    step_failure_reason,
)
from safe_ci_dag_runner.perflog import CsvMetricsSink, PerfWindow
from safe_ci_dag_runner.protocols import (
    CgroupManager,
    MetricsSink,
    RunResult,
    RunWindow,
    StepOutcome,
)
from safe_ci_dag_runner.scheduler import Runner, run_dag
from safe_ci_dag_runner.sizing import (
    jobs_footprint_bytes,
    jobs_for_budget,
    mem_available_bytes,
    parse_size,
    schedulable_peak_mem_bytes,
    step_mem_cap_bytes,
    step_mem_cap_for_inner_jobs,
    transitive_deps,
)
from safe_ci_dag_runner.teardown import reap
from safe_ci_dag_runner.viz import to_ascii, to_dot

__version__: str = "0.6.0"

__all__ = [
    "__version__",
    # DAG model
    "Step",
    "StepClass",
    "ResourceHint",
    "DagConfig",
    "DEFAULT_STEP_TIMEOUT",
    "DEFAULT_JOBS_FLAG",
    "step_classification",
    "preferred_inner_jobs",
    "step_failure_reason",
    "render_jobs_flag",
    "effective_jobs_flag",
    "command_with_inner_jobs",
    # running
    "run_dag",
    "Runner",
    "RunResult",
    "StepOutcome",
    # pluggable protocols
    "CgroupManager",
    "MetricsSink",
    "RunWindow",
    # memory-aware sizing
    "schedulable_peak_mem_bytes",
    "jobs_footprint_bytes",
    "jobs_for_budget",
    "step_mem_cap_bytes",
    "step_mem_cap_for_inner_jobs",
    "transitive_deps",
    "parse_size",
    "mem_available_bytes",
    # containment
    "Cgroups",
    "NoopCgroups",
    "CgroupEnforcementKind",
    # metrics
    "CsvMetricsSink",
    "PerfWindow",
    # ambient load
    "AmbientSnapshot",
    "ambient_bucket",
    "capture_ambient_snapshot",
    # teardown
    "reap",
    # visualization
    "to_dot",
    "to_ascii",
    # analysis
    "summarize",
    # profile-store feedback + planner
    "Planner",
    "StepSamples",
    "PlanEntry",
    "Plan",
    "DEFAULT_MIN_SAMPLES",
    "feedback_identity",
    "load_step_samples",
    "build_plan",
    "apply_plan_to_config",
    "plan_to_json",
    "plan_to_text",
    # serialization
    "dag_from_json",
    "dag_from_yaml",
    "dag_to_json",
    "dag_to_yaml",
    "DagJsonError",
]
