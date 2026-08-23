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

    from dagrun import Step, ResourceHint, DagConfig, run_dag_limited, to_ascii
    cfg = DagConfig(steps=(Step("build", "app", "compile", "make build"),))
    print(to_ascii(cfg))
    result = run_dag_limited(cfg, max_steps=2, max_cpus=8)  # uncontained library execution
"""

from __future__ import annotations

from dagrun.ambient import (
    AmbientSnapshot,
    ambient_bucket,
    capture_ambient_snapshot,
)
from dagrun.analyze import summarize
from dagrun.capabilities import (
    ENFORCEMENT_REGISTRY,
    Capability,
    Lane,
    enforcement_manifest,
    is_enforced,
)
from dagrun.cgroup import Cgroups, CgroupEnforcementKind, NoopCgroups
from dagrun.estimates import (
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
from dagrun.io import (
    DagJsonError,
    dag_from_json,
    dag_from_yaml,
    dag_to_json,
    dag_to_yaml,
)
from dagrun.model import (
    DAG_CONFIG_FIELDS,
    DEFAULT_JOBS_FLAG,
    DEFAULT_STEP_TIMEOUT,
    WALL_CPU_BACKSTOP_FACTOR,
    DagConfig,
    ResourceHint,
    Step,
    StepClass,
    WriteDomainGuarantee,
    WriteDomainPolicy,
    command_with_inner_jobs,
    dag_config_carry_diff,
    effective_jobs_flag,
    preferred_inner_jobs,
    render_jobs_flag,
    resolved_wall_timeout,
    step_classification,
    step_failure_reason,
    undeclared_resource_demands,
    write_domain_violations,
)
from dagrun.perflog import CsvMetricsSink, PerfWindow
from dagrun.protocols import (
    CgroupManager,
    MetricsSink,
    RunResult,
    RunWindow,
    StepOutcome,
)
from dagrun.scheduler import (
    Runner,
    cap_config_cpu_jobs,
    cap_config_max_cpus,
    run_dag,
    run_dag_limited,
    steps_violating_run_timeout,
)
from dagrun.sizing import (
    jobs_footprint_bytes,
    jobs_for_budget,
    mem_available_bytes,
    parse_size,
    schedulable_peak_mem_bytes,
    step_mem_cap_bytes,
    step_mem_cap_for_inner_jobs,
    transitive_deps,
)
from dagrun.cgroup import (
    ScopeAttempt,
    ScopeAttemptKind,
    policy_skip_reason,
)
from dagrun.teardown import reap
from dagrun.viz import to_ascii, to_dot

__version__: str = "0.15.0"

__all__ = [
    "__version__",
    # Enforcement manifest (derived per lane from the guards that implement it)
    "Capability",
    "ENFORCEMENT_REGISTRY",
    "Lane",
    "enforcement_manifest",
    "is_enforced",
    # DAG model
    "Step",
    "StepClass",
    "ResourceHint",
    "DagConfig",
    "WriteDomainGuarantee",
    "WriteDomainPolicy",
    "DEFAULT_STEP_TIMEOUT",
    "WALL_CPU_BACKSTOP_FACTOR",
    "resolved_wall_timeout",
    "DEFAULT_JOBS_FLAG",
    "step_classification",
    "preferred_inner_jobs",
    "step_failure_reason",
    "DAG_CONFIG_FIELDS",
    "dag_config_carry_diff",
    "undeclared_resource_demands",
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
