"""Model, plan, visualize, and execute DAGs of CI/build steps.

Define your graph as :class:`Step` values (each carrying a :class:`ResourceHint`) in a
:class:`DagConfig`, then call :func:`run_dag`. The runner gives you:

* memory-aware concurrency (largest ``-j`` that fits a RAM budget),
* optional per-step containment and measurement through injected protocols,
* Graphviz + ASCII DAG visualization.

The command-line application establishes its Linux cgroup scope before invoking the scheduler.
Library calls to :func:`run_dag` are uncontained unless the caller supplies an enabled
:class:`CgroupManager`; passing no manager is an explicit process-group-only execution path.
Metrics are likewise opt-in through :class:`MetricsSink`.

    from safe_ci_dag_runner import Step, ResourceHint, DagConfig, run_dag, to_ascii
    cfg = DagConfig(steps=(Step("build", "app", "compile", "make build"),))
    print(to_ascii(cfg))
    result = run_dag(cfg, jobs=4)   # uncontained library execution
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
    Allocation,
    Plan,
    PlanEntry,
    Planner,
    SpeedupLevel,
    StepSamples,
    StepSpeedup,
    allocate_widths,
    apply_plan_to_config,
    build_plan,
    feedback_identity,
    load_step_samples,
    load_step_speedups,
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

__version__: str = "0.12.0"

#: Machine-readable manifest emitted by the ``capabilities`` subcommand. Keys are sorted;
#: values describe the enforcement guards implemented by this package:
#:   cpu_affinity  opt-in --cores K: constrain the WHOLE run tree to K least-busy free cores
#:                 with an exact, verified cgroup cpuset; refuse when unavailable
#:   cpu_timeout   per-step user+system CPU budget (cgroup cpu.stat), reaped over budget
#:   memory_max    per-step inner memory.max cap (kernel OOM-kills the step at its cap)
#:   oom_detection failure attributed to OOM via cgroup memory.events oom_kill count
#:   pids_guard    per-step PID/thread ceiling (plumbed in both, enforced in neither -> false)
#:   wall_timeout  per-step wall-clock ceiling (load-dependent; active with or without boxing)
#: The cgroup-dependent guards take effect only under boxing; the boxed smoke tests in each build
#: anchor these declarations to real behavior wherever a cgroup-v2 + systemd --user scope exists.
ENFORCEMENT_CAPABILITIES: str = (
    '{"cpu_affinity":true,"cpu_timeout":true,"memory_max":true,"oom_detection":true,'
    '"pids_guard":false,"wall_timeout":true}'
)

__all__ = [
    "__version__",
    "ENFORCEMENT_CAPABILITIES",
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
    "SpeedupLevel",
    "StepSpeedup",
    "PlanEntry",
    "Allocation",
    "Plan",
    "DEFAULT_MIN_SAMPLES",
    "feedback_identity",
    "load_step_samples",
    "load_step_speedups",
    "allocate_widths",
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
