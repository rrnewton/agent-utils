"""A wall backstop should be derived from the CPU budget, not baked into the graph.

`Step.timeout` used to default to a hardcoded 1800 the moment a document was loaded, which is
exactly the load-sensitive number a derivation exists to remove: it is the same on a laptop and
on a 300-core host, and it tells you nothing about the step. parallel-experiment-runner already
solved this — explicit wall wins, else 3x the CPU budget, else the default — and this brings the
same idiom here rather than inventing a second policy.

Two design questions had to be ANSWERED, not merely implemented, and both are pinned below:

* derive from the DECLARED cpu_timeout, never the canonical (default-filled) one, or every
  undeclared step would silently drop from a 1800-second ceiling to a 30-second one; and
* derive from the PLATFORM-SCALED budget, or the backstop would race the CPU guard on exactly
  the slow platform `cpu_timeout_multiplier` exists for.
"""

from __future__ import annotations

import dataclasses

from parallel_experiment_runner.model import (
    WALL_CPU_BACKSTOP_FACTOR as PER_WALL_CPU_BACKSTOP_FACTOR,
)
from safe_ci_dag_runner.io import dag_from_json, dag_to_json
from safe_ci_dag_runner.model import (
    DEFAULT_SMALL_CPU_TIMEOUT,
    DEFAULT_STEP_TIMEOUT,
    WALL_CPU_BACKSTOP_FACTOR,
    DagConfig,
    Step,
    canonical_cpu_timeout,
    resolved_wall_timeout,
)
from safe_ci_dag_runner.scheduler import steps_violating_run_timeout


def _step(**kwargs: int) -> Step:
    return Step("g", "a", "d", "true", **kwargs)  # type: ignore[arg-type]


def test_the_factor_is_the_one_the_other_runner_already_established() -> None:
    # Not a tautology: the two constants live in two independent packages and this is the only
    # thing stopping them from drifting into two policies with one name.
    assert WALL_CPU_BACKSTOP_FACTOR == PER_WALL_CPU_BACKSTOP_FACTOR
    assert WALL_CPU_BACKSTOP_FACTOR == 3


def test_an_explicit_step_budget_wins_over_everything() -> None:
    step = _step(timeout=42, cpu_timeout=7)
    assert resolved_wall_timeout(step, 600, 2.0) == 42


def test_a_document_default_wins_over_the_derivation() -> None:
    # An author who wrote a document-wide number said something; the derivation must not
    # second-guess it.
    assert resolved_wall_timeout(_step(cpu_timeout=7), 600) == 600


def test_a_declared_cpu_budget_derives_the_backstop() -> None:
    assert resolved_wall_timeout(_step(cpu_timeout=7), 0) == 21


def test_the_derivation_tracks_the_platform_scaled_budget() -> None:
    # 4 CPU-seconds on a platform 2.5x slower is a 10-second enforced budget, so the wall
    # backstop is 30 and keeps its 3x margin. Pinned to the DECLARED 4 it would be 12 — only
    # 1.2x the enforced guard, i.e. racing it on exactly the platform the multiplier exists for.
    assert resolved_wall_timeout(_step(cpu_timeout=4), 0, 2.5) == 30


def test_a_step_that_declared_nothing_keeps_the_1800_second_backstop() -> None:
    # THE ANSWER TO THE OPEN QUESTION. `canonical_cpu_timeout` fills in the small 10-second
    # default for such a step, and deriving from THAT would give it a 30-second wall ceiling.
    step = _step()
    assert canonical_cpu_timeout(step, DEFAULT_SMALL_CPU_TIMEOUT) == 10
    assert resolved_wall_timeout(step, 0) == DEFAULT_STEP_TIMEOUT == 1800


def test_the_sentinel_is_absence_and_round_trips_as_absence() -> None:
    doc = '{"steps": [{"group": "g", "job": "a", "cmd": "true", "cpu_timeout": 7}]}'
    cfg = dag_from_json(doc)
    assert cfg.steps[0].timeout == 0
    assert cfg.default_step_timeout == 0
    emitted = dag_to_json(cfg)
    assert '"timeout"' not in emitted, "0 written out would read as 'no wall bound'"
    assert '"default_step_timeout"' not in emitted
    # Reloading the emission must land on the same model, or the format has lost the sentinel.
    assert dag_from_json(emitted) == cfg
    assert dag_to_json(dag_from_json(emitted)) == emitted


def test_a_declared_budget_still_round_trips_as_a_number() -> None:
    # The other side: omission must not become "always omit".
    doc = '{"steps": [{"group": "g", "job": "a", "cmd": "true", "timeout": 42}]}'
    cfg = dag_from_json(doc)
    assert cfg.steps[0].timeout == 42
    assert '"timeout": 42' in dag_to_json(cfg)


def test_a_document_default_still_round_trips_as_a_number() -> None:
    doc = '{"default_step_timeout": 600, "steps": [{"group": "g", "job": "a", "cmd": "true"}]}'
    cfg = dag_from_json(doc)
    assert cfg.default_step_timeout == 600
    # The loader materializes it into the step, as it always has.
    assert cfg.steps[0].timeout == 600
    emitted = dag_to_json(cfg)
    assert '"default_step_timeout": 600' in emitted
    assert dag_from_json(emitted) == cfg


def test_the_run_budget_ordering_is_checked_on_the_resolved_value() -> None:
    # A 0 sentinel passes `>= run_timeout_s` trivially, so the fail-closed inner-below-outer
    # ordering has to be expressed on the value the step will actually run under.
    step = _step(cpu_timeout=100)  # derives a 300-second wall backstop
    cfg = DagConfig(steps=(step,))
    assert steps_violating_run_timeout(cfg, 200) == [("g.a", 300)]
    assert steps_violating_run_timeout(cfg, 400) == []


def test_an_undeclared_step_is_still_caught_by_the_run_budget_check() -> None:
    # Its resolved bound is 1800, so a 900-second run budget must still refuse it. Before the
    # resolved value was used, its declared 0 would have sailed through.
    cfg = DagConfig(steps=(_step(),))
    assert steps_violating_run_timeout(cfg, 900) == [("g.a", 1800)]


def test_a_step_the_derivation_brings_under_the_run_budget_is_not_refused() -> None:
    # And the direction that matters for usability: deriving a SMALL backstop lets a graph that
    # a baked-in 1800 would have refused actually run.
    cfg = DagConfig(steps=(dataclasses.replace(_step(), cpu_timeout=5),))
    assert steps_violating_run_timeout(cfg, 60) == []
