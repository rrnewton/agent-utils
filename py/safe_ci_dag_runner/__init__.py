"""Model, plan, visualize, and execute DAGs of CI/build steps.

Define your graph as :class:`Step` values (each carrying a :class:`ResourceHint`) in a
:class:`DagConfig`, then call :func:`run_dag_limited`. The runner gives you:

* independent active-step and total-CPU limits, with memory-aware step sizing,
* optional per-step containment and measurement through injected protocols,
* Graphviz + ASCII DAG visualization.

The command-line application establishes its Linux cgroup scope before invoking the scheduler.
Library calls to :func:`run_dag` are uncontained unless the caller supplies an enabled
:class:`CgroupManager`; passing no manager is an explicit process-group-only execution path.
Metrics are likewise opt-in through :class:`MetricsSink`.

    from safe_ci_dag_runner import Step, ResourceHint, DagConfig, run_dag_limited, to_ascii
    cfg = DagConfig(steps=(Step("build", "app", "compile", "make build"),))
    print(to_ascii(cfg))
    result = run_dag_limited(cfg, max_steps=2, max_cpus=8)  # uncontained library execution
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
    InfeasibleAllocationError,
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
    WriteDomainGuarantee,
    WriteDomainPolicy,
    command_with_inner_jobs,
    effective_jobs_flag,
    preferred_inner_jobs,
    render_jobs_flag,
    step_classification,
    step_failure_reason,
    write_domain_violations,
)
from safe_ci_dag_runner.perflog import CsvMetricsSink, PerfWindow
from safe_ci_dag_runner.protocols import (
    CgroupManager,
    MetricsSink,
    RunResult,
    RunWindow,
    StepOutcome,
)
from safe_ci_dag_runner.scheduler import (
    Runner,
    cap_config_cpu_jobs,
    cap_config_max_cpus,
    run_dag,
    run_dag_limited,
    steps_violating_run_timeout,
)
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
from safe_ci_dag_runner.cgroup import (
    ScopeAttempt,
    ScopeAttemptKind,
    policy_skip_reason,
)
from safe_ci_dag_runner.teardown import reap
from safe_ci_dag_runner.viz import to_ascii, to_dot

__version__: str = "0.14.0"

#: Machine-readable manifest emitted by the ``capabilities`` subcommand. Keys are sorted;
#: values describe the enforcement guards implemented by this package:
#:   cpu_affinity  opt-in --cores K: constrain the WHOLE run tree to K least-busy free cores
#:                 with an exact, verified cgroup cpuset; refuse when unavailable
#:   cpu_bandwidth boxed run: exact outer cpu.max = --max-cpus x period, read back before execution
#:   cpu_timeout   per-step user+system CPU budget (cgroup cpu.stat), reaped over budget
#:   memory_max    per-step inner memory.max cap (kernel OOM-kills the step at its cap)
#:   oom_detection failure attributed to OOM via cgroup memory.events oom_kill count
#:   pids_guard    per-step PID/thread ceiling (plumbed in both, enforced in neither -> false)
#:   run_timeout   OUTER wall budget for the WHOLE run: the scheduler cuts in-flight steps and
#:                 still reports (works boxed or unboxed); under boxing it is additionally backed
#:                 by the scope's systemd RuntimeMaxSec, set strictly later so the reporting
#:                 bound fires first
#:   wall_timeout  per-step wall-clock ceiling (load-dependent; active with or without boxing)
#:   write_domains pre-execution closed-vocabulary declaration guard; omission/unknown/duplicate
#:                 domains refuse before any node starts when the DAG opts in
#: The cgroup-dependent guards take effect only under boxing; the boxed smoke tests in each build
#: anchor these declarations to real behavior wherever a cgroup-v2 + systemd --user scope exists.
ENFORCEMENT_CAPABILITIES: str = (
    '{"cpu_affinity":true,"cpu_bandwidth":true,"cpu_timeout":true,"memory_max":true,'
    '"oom_detection":true,"pids_guard":false,"run_timeout":true,"wall_timeout":true,'
    '"write_domains":true}'
)

__all__ = [
    "__version__",
    "ENFORCEMENT_CAPABILITIES",
    # DAG model
    "Step",
    "StepClass",
    "ResourceHint",
    "DagConfig",
    "WriteDomainGuarantee",
    "WriteDomainPolicy",
    "DEFAULT_STEP_TIMEOUT",
    "DEFAULT_JOBS_FLAG",
    "step_classification",
    "preferred_inner_jobs",
    "step_failure_reason",
    "write_domain_violations",
    "render_jobs_flag",
    "effective_jobs_flag",
    "command_with_inner_jobs",
    # containment outcome
    "ScopeAttempt",
    "ScopeAttemptKind",
    "policy_skip_reason",
    # running
    "run_dag",
    "run_dag_limited",
    "cap_config_max_cpus",
    "cap_config_cpu_jobs",
    "steps_violating_run_timeout",
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
    # profile analysis library helper (not a console command)
    "summarize",
    # profile-store feedback + planner
    "Planner",
    "StepSamples",
    "SpeedupLevel",
    "StepSpeedup",
    "PlanEntry",
    "Allocation",
    "InfeasibleAllocationError",
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
