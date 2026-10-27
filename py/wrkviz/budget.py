"""Plan a summarize run's model spend before it starts, and hold the run to a budget.

A summarize run is a map-reduce tree: phase summaries (leaves), hindsight agent names, one project
overview, then calendar rollups built from the leaves. A run that stops partway leaves leaves
without the rollups above them, so the budget is checked twice:

* before any model call, against an estimate of the whole remaining tree (:class:`SummaryPlan`),
  so a run that cannot finish within the budget refuses to start; and
* while it runs, by :class:`SpendGuard`, which reserves each batch's estimated cost before the
  call starts and stops admitting new batches once the next one would cross the budget. Batches
  already running finish and are kept; the run then stops with :class:`BudgetStop`, and the team's
  ``summarize_state.json`` records the stop so the next run reports it and resumes.

Estimates use the mean recorded usage per job of each kind from this cache's own batch receipts
(same backend and model), falling back to built-in figures when there is no history.
"""

from __future__ import annotations

import os
import threading
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

from wrkviz.archive import (
    JsonValue,
    as_object,
    read_json,
    write_json_if_changed,
)
from wrkviz.summary_registry import (
    AGENT_LIFETIME_STYLE,
    PHASE_STYLE,
    PLAIN_LANGUAGE_ROLLUP_STYLE,
    PROJECT_OVERVIEW_STYLE,
    TECHNICAL_ROLLUP_STYLE,
)
from wrkviz.token_usage import TokenUsage, load_batch_receipt

#: Job kinds the planner prices separately: the summary style of each job.
PHASE_KIND = PHASE_STYLE
TECHNICAL_ROLLUP_KIND = TECHNICAL_ROLLUP_STYLE
PLAIN_ROLLUP_KIND = PLAIN_LANGUAGE_ROLLUP_STYLE
OVERVIEW_KIND = PROJECT_OVERVIEW_STYLE
AGENT_NAME_KIND = AGENT_LIFETIME_STYLE

STATE_FILE = "summarize_state.json"
STATE_SCHEMA_VERSION = 1


class BudgetError(RuntimeError):
    """A summarize run was refused or stopped to stay within its budget."""


class BudgetExceeded(BudgetError):
    """The whole remaining tree is estimated to cost more than the budget; nothing was spent."""

    def __init__(self, plan: SummaryPlan) -> None:
        super().__init__(
            f"refusing to start: the remaining summary tree is estimated at "
            f"${plan.estimated_usd or 0:.2f}, over the ${plan.budget_usd or 0:.2f} budget; "
            "nothing was spent. Raise --budget-usd, narrow the summary window, or drop rollup "
            "levels."
        )
        self.plan = plan


class BudgetStop(BudgetError):
    """The run stopped admitting model calls at the budget; finished work is cached."""


@dataclass(frozen=True)
class Prices:
    """USD per million tokens. Uncached input is ``input_tokens`` minus both cache counters."""

    input: float
    output: float
    cached_input: float
    cache_write_input: float

    @classmethod
    def parse(cls, text: str) -> Prices:
        """Parse ``input=2,output=10[,cached_input=0.2][,cache_write_input=2.5]``.

        ``input`` and ``output`` are required; either cache rate defaults to ``input``.
        """

        values: dict[str, float] = {}
        allowed = {"input", "output", "cached_input", "cache_write_input"}
        for part in text.split(","):
            name, sep, raw = part.partition("=")
            name = name.strip()
            if not sep or name not in allowed:
                raise ValueError(
                    f"--price: expected NAME=USD_PER_MTOK with NAME in {sorted(allowed)}, "
                    f"got {part.strip()!r}"
                )
            if name in values:
                raise ValueError(f"--price: {name} given twice")
            try:
                value = float(raw)
            except ValueError:
                raise ValueError(f"--price: {name} is not a number: {raw!r}") from None
            if not value >= 0 or value == float("inf"):
                raise ValueError(f"--price: {name} must be a finite non-negative number")
            values[name] = value
        if "input" not in values or "output" not in values:
            raise ValueError("--price: input and output rates are required")
        return cls(
            input=values["input"],
            output=values["output"],
            cached_input=values.get("cached_input", values["input"]),
            cache_write_input=values.get("cache_write_input", values["input"]),
        )

    def cost_usd(self, usage: TokenUsage) -> float:
        """Return what *usage* cost at these rates."""

        return self._cost(
            usage.input_tokens,
            usage.cached_input_tokens,
            usage.cache_write_input_tokens,
            usage.output_tokens,
        )

    def _cost(self, inp: float, cached: float, write: float, out: float) -> float:
        uncached = max(0.0, inp - cached - write)
        return (
            uncached * self.input
            + cached * self.cached_input
            + write * self.cache_write_input
            + out * self.output
        ) / 1_000_000


@dataclass(frozen=True)
class JobUsage:
    """Mean token usage of one job of a kind, and how many jobs that mean is drawn from."""

    input_tokens: float
    cached_input_tokens: float
    cache_write_input_tokens: float
    output_tokens: float
    samples: int = 0

    def times(self, jobs: int) -> TokenUsage:
        """Return the usage of *jobs* such jobs, rounded up to whole tokens."""

        def scale(value: float) -> int:
            return int(-(-value * jobs // 1))

        inp = scale(self.input_tokens)
        return TokenUsage(
            input_tokens=inp,
            cached_input_tokens=min(inp, scale(self.cached_input_tokens)),
            cache_write_input_tokens=min(inp, scale(self.cache_write_input_tokens)),
            output_tokens=scale(self.output_tokens),
        )


#: Used when a cache has no receipts for a kind yet. Measured means per job from a real archive
#: of Claude and Codex runs, rounded up and priced as uncached input, so a first run errs high.
DEFAULT_JOB_USAGE: Mapping[str, JobUsage] = {
    PHASE_KIND: JobUsage(21_000, 0, 0, 2_500),
    AGENT_NAME_KIND: JobUsage(29_000, 0, 0, 900),
    OVERVIEW_KIND: JobUsage(24_000, 0, 0, 1_200),
    TECHNICAL_ROLLUP_KIND: JobUsage(60_000, 0, 0, 3_800),
    PLAIN_ROLLUP_KIND: JobUsage(50_000, 0, 0, 2_900),
}

_ZERO = JobUsage(0, 0, 0, 0)


@dataclass(frozen=True)
class CostModel:
    """Mean usage per job kind for one backend and model."""

    per_job: Mapping[str, JobUsage]
    free: bool = False

    def usage(self, kind: str, jobs: int) -> TokenUsage:
        """Return the estimated usage of *jobs* jobs of *kind*."""

        if self.free or jobs <= 0:
            return TokenUsage()
        default = DEFAULT_JOB_USAGE.get(kind, DEFAULT_JOB_USAGE[TECHNICAL_ROLLUP_KIND])
        return self.per_job.get(kind, default).times(jobs)

    def usd(self, kind: str, jobs: int, prices: Prices) -> float:
        """Return the estimated cost of *jobs* jobs of *kind*, from unrounded means.

        The plan and the spend guard both price through here, so the guard's per-batch
        reservations add up to exactly the plan's per-stage estimate.
        """

        if self.free or jobs <= 0:
            return 0.0
        default = DEFAULT_JOB_USAGE.get(kind, DEFAULT_JOB_USAGE[TECHNICAL_ROLLUP_KIND])
        mean = self.per_job.get(kind, default)
        return prices._cost(
            mean.input_tokens * jobs,
            mean.cached_input_tokens * jobs,
            mean.cache_write_input_tokens * jobs,
            mean.output_tokens * jobs,
        )

    def source(self, kind: str) -> str:
        """Say where the estimate for *kind* comes from."""

        if self.free:
            return "offline backend"
        item = self.per_job.get(kind)
        if item is None or item.samples == 0:
            return "built-in"
        return f"mean of {item.samples} recorded"


def _receipt_kind(cache_dir: Path, first_hash: str, name_cache: bool) -> str | None:
    if name_cache:
        return AGENT_NAME_KIND
    try:
        root = as_object(read_json(cache_dir / f"{first_hash}.json"), "summary cache")
        result = as_object(root.get("result"), "summary cache.result")
    except (OSError, ValueError):
        return None
    key = result.get("key")
    if not isinstance(key, str):
        return None
    for prefix, kind in (
        ("rollup-plain:", PLAIN_ROLLUP_KIND),
        ("rollup:", TECHNICAL_ROLLUP_KIND),
        ("project-overview:", OVERVIEW_KIND),
    ):
        if key.startswith(prefix):
            return kind
    return PHASE_KIND


def calibrate(
    summary_cache: Path,
    name_cache: Path,
    backend: str,
    model: str,
    reasoning_effort: str | None,
) -> CostModel:
    """Return mean usage per job kind from completed receipts of this backend/model/effort."""

    if backend == "heuristic":
        return CostModel(per_job={}, free=True)
    totals: dict[str, list[float]] = {}
    for cache_dir, is_names in ((summary_cache, False), (name_cache, True)):
        receipts = cache_dir / "_usage" / "receipts"
        if not receipts.is_dir():
            continue
        for path in sorted(receipts.glob("*.json")):
            receipt = load_batch_receipt(cache_dir / "_usage", path.stem)
            if (
                receipt is None
                or receipt.status != "completed"
                or receipt.usage is None
                or not receipt.input_hashes
                or receipt.backend != backend
                or receipt.model != model
                or receipt.reasoning_effort != reasoning_effort
            ):
                continue
            kind = _receipt_kind(cache_dir, receipt.input_hashes[0], is_names)
            if kind is None:
                continue
            item = totals.setdefault(kind, [0.0, 0.0, 0.0, 0.0, 0.0])
            item[0] += len(receipt.input_hashes)
            item[1] += receipt.usage.input_tokens
            item[2] += receipt.usage.cached_input_tokens
            item[3] += receipt.usage.cache_write_input_tokens
            item[4] += receipt.usage.output_tokens
    per_job = {
        kind: JobUsage(
            input_tokens=values[1] / values[0],
            cached_input_tokens=values[2] / values[0],
            cache_write_input_tokens=values[3] / values[0],
            output_tokens=values[4] / values[0],
            samples=int(values[0]),
        )
        for kind, values in totals.items()
        if values[0] > 0
    }
    return CostModel(per_job=per_job)


@dataclass
class StageCount:
    """What one stage of the tree needs: cached, carried forward, or new model work."""

    name: str
    kind: str
    total: int = 0
    cached: int = 0
    carried_forward: int = 0
    to_generate: int = 0


@dataclass
class SummaryPlan:
    """The estimated remaining work and cost of one summarize run, computed before spending."""

    team_slug: str
    stages: list[StageCount]
    costs: CostModel
    prices: Prices | None
    budget_usd: float | None
    previous_state: Mapping[str, JsonValue] | None = None
    request: Mapping[str, JsonValue] | None = None

    def stage_usage(self, stage: StageCount) -> TokenUsage:
        """Return the estimated usage of one stage's new model work."""

        return self.costs.usage(stage.kind, stage.to_generate)

    @property
    def usage(self) -> TokenUsage:
        """Return the estimated usage of all new model work."""

        total = TokenUsage()
        for stage in self.stages:
            total += self.stage_usage(stage)
        return total

    def stage_usd(self, stage: StageCount) -> float | None:
        """Return the estimated cost of one stage, if prices are known."""

        if self.prices is None:
            return None
        return self.costs.usd(stage.kind, stage.to_generate, self.prices)

    @property
    def estimated_usd(self) -> float | None:
        """Return the estimated cost of the whole remaining tree, if prices are known."""

        if self.prices is None:
            return None
        return sum(
            self.costs.usd(stage.kind, stage.to_generate, self.prices)
            for stage in self.stages
        )

    @property
    def to_generate(self) -> int:
        """Return how many jobs need a model call."""

        return sum(stage.to_generate for stage in self.stages)

    @property
    def over_budget(self) -> bool:
        """Return whether the estimate exceeds the budget."""

        estimate = self.estimated_usd
        return (
            self.budget_usd is not None
            and estimate is not None
            and estimate > self.budget_usd
        )

    def lines(self) -> list[str]:
        """Return a human-readable plan, one line per stage plus a total."""

        out = [f"summary plan for {self.team_slug} (before any model call):"]
        if self.previous_state is not None:
            out.append("  " + describe_previous_state(self.previous_state, self.request))
        for stage in self.stages:
            usage = self.stage_usage(stage)
            usd = self.stage_usd(stage)
            parts = [f"{stage.total} total", f"{stage.cached} cached"]
            if stage.carried_forward:
                parts.append(f"{stage.carried_forward} carried forward")
            parts.append(f"{stage.to_generate} to generate")
            cost = ""
            if stage.to_generate:
                cost = (
                    f"; ~{usage.input_tokens:,} in / {usage.output_tokens:,} out tokens"
                    + (f", ~${usd:.2f}" if usd is not None else "")
                    + f" ({self.costs.source(stage.kind)})"
                )
            out.append(f"  {stage.name}: " + ", ".join(parts) + cost)
        usage = self.usage
        total = f"  total: {self.to_generate} model jobs, ~{usage.input_tokens:,} in / " + (
            f"{usage.output_tokens:,} out tokens"
        )
        estimate = self.estimated_usd
        if estimate is not None:
            total += f", ~${estimate:.2f}"
        if self.budget_usd is not None:
            total += f"; budget ${self.budget_usd:.2f}"
            if estimate is not None:
                total += " (over budget)" if self.over_budget else " (within budget)"
        out.append(total)
        return out

    def to_json(self) -> dict[str, JsonValue]:
        """Return the plan as JSON for the run-state file."""

        stages: list[JsonValue] = [
            {
                "name": stage.name,
                "kind": stage.kind,
                "total": stage.total,
                "cached": stage.cached,
                "carried_forward": stage.carried_forward,
                "to_generate": stage.to_generate,
                "estimated_usage": self.stage_usage(stage).to_json(),
                "estimated_usd": _rounded(self.stage_usd(stage)),
            }
            for stage in self.stages
        ]
        return {
            "stages": stages,
            "to_generate": self.to_generate,
            "estimated_usage": self.usage.to_json(),
            "estimated_usd": _rounded(self.estimated_usd),
            "budget_usd": self.budget_usd,
        }


def _rounded(value: float | None) -> JsonValue:
    return None if value is None else round(value, 4)


class SpendGuard:
    """Admit model batches only while the next one's estimated cost still fits the budget.

    Thread-safe: batches run on worker threads. A batch reserves its estimate before its call
    starts and settles to the recorded usage afterwards (or keeps the estimate when the backend
    reported none), so at most the in-flight batches' estimation error can exceed the budget.
    """

    def __init__(self, budget_usd: float, prices: Prices, costs: CostModel) -> None:
        if not budget_usd >= 0:
            raise ValueError("budget must be a non-negative number of US dollars")
        self.budget_usd = budget_usd
        self.prices = prices
        self.costs = costs
        self._lock = threading.Lock()
        self._spent = 0.0
        self._reserved = 0.0
        self._stopped = False

    def reserve(self, kind: str, jobs: int) -> float | None:
        """Return a reservation for one batch, or ``None`` once the budget would be crossed."""

        estimate = self.costs.usd(kind, jobs, self.prices)
        with self._lock:
            # The tolerance absorbs float summation order only: per-batch reservations of a
            # plan that exactly equals the budget must all be admitted.
            if self._stopped or (
                self._spent + self._reserved + estimate > self.budget_usd + 1e-9
            ):
                self._stopped = True
                return None
            self._reserved += estimate
            return estimate

    def settle(self, reservation: float, usage: TokenUsage | None) -> None:
        """Replace a reservation with what the batch actually cost."""

        actual = reservation if usage is None else self.prices.cost_usd(usage)
        with self._lock:
            self._reserved -= reservation
            self._spent += actual

    @property
    def spent_usd(self) -> float:
        """Return what settled batches cost so far."""

        with self._lock:
            return self._spent

    @property
    def stopped(self) -> bool:
        """Return whether a batch has been refused."""

        with self._lock:
            return self._stopped


def _now() -> str:
    return datetime.now(timezone.utc).isoformat()


def load_state(summary_root: Path) -> dict[str, JsonValue] | None:
    """Return the last run-state record, or ``None`` when absent or unreadable."""

    try:
        state = as_object(read_json(summary_root / STATE_FILE), STATE_FILE)
    except (OSError, ValueError):
        return None
    if state.get("schema_version") != STATE_SCHEMA_VERSION:
        return None
    return state


def write_state(summary_root: Path, state: Mapping[str, JsonValue]) -> None:
    """Persist the run-state record."""

    record = dict(state)
    record["schema_version"] = STATE_SCHEMA_VERSION
    record["updated_at"] = _now()
    write_json_if_changed(summary_root / STATE_FILE, record)


def running_state(
    *,
    request: Mapping[str, JsonValue],
    plan: SummaryPlan,
    previous: Mapping[str, JsonValue] | None,
) -> dict[str, JsonValue]:
    """Return the record written before the first model call."""

    resumed: JsonValue = None
    if previous is not None and previous.get("status") != "complete":
        resumed = {
            key: previous.get(key)
            for key in ("status", "started_at", "updated_at", "stopped", "spent_usd")
        }
    return {
        "status": "running",
        "pid": os.getpid(),
        "started_at": _now(),
        "request": dict(request),
        "plan": plan.to_json(),
        "budget_usd": plan.budget_usd,
        "spent_usd": None,
        "stopped": None,
        "resumed_from": resumed,
    }


def describe_previous_state(
    state: Mapping[str, JsonValue], request: Mapping[str, JsonValue] | None = None
) -> str:
    """Return one line describing an earlier run that did not finish, for the plan."""

    status = state.get("status")
    started = state.get("started_at")
    differs = (
        "; it was asked for a different window, rollup levels or model, so the counts below "
        "are for this run's request"
        if request is not None and state.get("request") != dict(request)
        else ""
    )
    if status == "running":
        return (
            f"resuming: the run started {started} was interrupted before it finished "
            "(it was killed or crashed); its finished work is cached" + differs
        )
    stopped = state.get("stopped")
    stage = reason = None
    if isinstance(stopped, dict):
        stage = stopped.get("stage")
        reason = stopped.get("reason")
    return (
        f"resuming: the run started {started} stopped partway"
        + (f" during {stage}" if stage else "")
        + (f" ({reason})" if reason else "")
        + "; its finished work is cached"
        + differs
    )


def partial_warning(state: Mapping[str, JsonValue] | None) -> str | None:
    """Return a warning line when the last summarize run did not complete."""

    if state is None or state.get("status") == "complete":
        return None
    status = state.get("status")
    what = "was interrupted" if status == "running" else "stopped partway"
    stopped = state.get("stopped")
    stage = stopped.get("stage") if isinstance(stopped, dict) else None
    return (
        f"summaries are partial: the last summarize run {what}"
        + (f" during {stage}" if isinstance(stage, str) else "")
        + "; run summarize again with that run's arguments (its request in "
        + STATE_FILE
        + ") to finish"
    )


__all__ = [
    "AGENT_NAME_KIND",
    "BudgetError",
    "BudgetExceeded",
    "BudgetStop",
    "CostModel",
    "DEFAULT_JOB_USAGE",
    "JobUsage",
    "OVERVIEW_KIND",
    "PHASE_KIND",
    "PLAIN_ROLLUP_KIND",
    "Prices",
    "STATE_FILE",
    "SpendGuard",
    "StageCount",
    "SummaryPlan",
    "TECHNICAL_ROLLUP_KIND",
    "calibrate",
    "describe_previous_state",
    "load_state",
    "partial_warning",
    "running_state",
    "write_state",
]
