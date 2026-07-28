"""Core vocabulary for pr-landing-planner: pure data + enums, no I/O.

A landing planner answers one question over a set of open pull requests (PRs): *given the conflict
graph and live CI health, which PRs can be landed now, which must be rebased first, which reds are
real vs. benign, and in what order should the coordinator act?*

Everything here is a frozen dataclass or an :class:`enum.Enum`. The interesting work
(:mod:`pr_landing_planner.graph`, :mod:`pr_landing_planner.classify`, :mod:`pr_landing_planner.plan`,
:mod:`pr_landing_planner.emit`) is PURE and operates on these values, so it is unit-testable with a
:class:`pr_landing_planner.fakehost.FakeHost` and byte-stable for a future Rust port. The two
side-effecting boundaries (talking to a VCS host, running git) live behind the
:class:`pr_landing_planner.host.VcsHost` protocol.
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass, field
from enum import Enum

DEFAULT_REPO = "DeepScryAI/DeepScry"
DEFAULT_BASE = "integration"
#: The single required merge-gate check most callers gate on (ds-4171 / ds-xdc7m9 semantics).
DEFAULT_GATE_CHECK = "merge-gate"


class CiState(Enum):
    """The coarse CI verdict for a PR's whole check rollup."""

    GREEN = "green"
    RED = "red"
    PENDING = "pending"
    #: No checks configured at all.
    NONE = "none"


class RedClass(Enum):
    """Why a PR's CI is (or looks) red — the headline classification of tool #3.

    Grounded in four real failure modes recorded in the DeepScry minibeads:

    * :attr:`REAL` — a genuine regression; hold and dispatch a fix.
    * :attr:`FLAKY` — a red whose check name / message matches a caller-supplied flaky signature; a
      re-fire of CI is expected to clear it.
    * :attr:`STALE_REQUIRED_CHECK` — the underlying CI is green on the head commit but the required
      gate check froze on a stale result (ds-4171); re-fire the gate, do not treat as a failure.
    * :attr:`EVALUATE_ONCE_RACE` — the gate fired once while full CI was still queued and exited with
      a benign "still queued" message (ds-xdc7m9 / ds-96k1wa); this is noise — treat as pending.
    * :attr:`RUNNER_OUTAGE` — the gate job itself never executed (blank runner / BlobNotFound /
      near-zero duration), typically across many branches (ds-69ih3r); escalate the CI outage.
    """

    REAL = "real"
    FLAKY = "flaky"
    STALE_REQUIRED_CHECK = "stale-required-check"
    EVALUATE_ONCE_RACE = "evaluate-once-race"
    RUNNER_OUTAGE = "runner-outage"


class PrAction(Enum):
    """The recommended next action for one PR. Advisory only — the planner never mutates anything."""

    LAND_NOW = "land-now"
    REBASE_THEN_LAND = "rebase-then-land"
    REFIRE_STALE_GATE = "refire-stale-gate"
    ESCALATE_RUNNER_OUTAGE = "escalate-runner-outage"
    REFIRE_CI = "refire-ci"
    HOLD_FIX = "hold-fix"
    WAIT = "wait"


@dataclass(frozen=True)
class CheckRun:
    """One entry in a PR's status-check rollup, narrowed to the fields the classifier needs.

    ``status`` and ``conclusion`` are upper-cased GitHub tokens (e.g. ``COMPLETED`` / ``SUCCESS``);
    ``text`` carries an optional human message (a check title / description) used only for flaky /
    evaluate-once / outage signature matching; ``workflow`` is the owning workflow name when known;
    ``duration_secs`` is the check's run time when known (a near-zero gate run is an outage signal).
    """

    name: str
    status: str = ""
    conclusion: str = ""
    text: str = ""
    workflow: str = ""
    duration_secs: int | None = None


@dataclass(frozen=True)
class RawPr:
    """A single open PR as produced by a :class:`pr_landing_planner.host.VcsHost`.

    This is the host boundary's output: everything the pure core needs about a PR *before* git
    fetches / merge-tree probes. ``api_head_sha`` is the head commit the host API reported; the
    collector re-fetches the head and aborts if it drifted (the content-identity guard). ``checks``
    is the already-narrowed rollup, and ``labels`` the PR's labels (both used by the pure classifier
    and priority logic).
    """

    number: int
    head_ref: str
    base_ref: str
    api_head_sha: str
    title: str = ""
    author: str = ""
    is_draft: bool = False
    mergeable: str = ""
    review_decision: str = ""
    created_at: str = ""
    updated_at: str = ""
    additions: int = 0
    deletions: int = 0
    labels: tuple[str, ...] = ()
    checks: tuple[CheckRun, ...] = ()


@dataclass(frozen=True)
class CiVerdict:
    """The classifier's refined verdict for one PR (see :mod:`pr_landing_planner.classify`)."""

    raw_state: CiState
    #: Set whenever the PR presents a red-ish anomaly (real red, flaky, stale gate, race, outage).
    red_class: RedClass | None
    #: True when the required gate check is present in the rollup.
    gate_present: bool
    #: True when the required gate check concluded successfully.
    gate_ok: bool
    #: True when the gate check shows the "never actually ran" outage signature.
    gate_missing_run: bool
    detail: str = ""


@dataclass(frozen=True)
class PrNode:
    """A fully-collected PR: its metadata, changed files, freshness, CI verdict, and hold state.

    This is what the pure graph / plan layers consume. ``files`` is the changed-file set (base..head);
    ``base_conflict_paths`` are paths that conflict with the base itself (a real merge-tree probe, not
    just GitHub's ``mergeable`` flag); ``commits_behind`` is how far the head trails the base (the
    freshness signal); ``priority`` is the resolved ordering priority (lower = more urgent).
    """

    number: int
    head_ref: str
    base_ref: str
    head_sha: str
    base_sha: str
    title: str = ""
    author: str = ""
    is_draft: bool = False
    mergeable: str = ""
    review_decision: str = ""
    created_at: str = ""
    additions: int = 0
    deletions: int = 0
    labels: tuple[str, ...] = ()
    files: frozenset[str] = field(default_factory=frozenset)
    base_conflict_paths: tuple[str, ...] = ()
    commits_behind: int = 0
    ci: CiVerdict = field(
        default_factory=lambda: CiVerdict(CiState.NONE, None, False, False, False, "")
    )
    priority: int = 0

    @property
    def size(self) -> int:
        return self.additions + self.deletions

    @property
    def base_conflicting(self) -> bool:
        return bool(self.base_conflict_paths) or self.mergeable == "CONFLICTING"


@dataclass(frozen=True)
class ConflictEdge:
    """Two PRs Git confirms cannot merge cleanly with each other (a real merge-tree conflict)."""

    a: int
    b: int
    paths: tuple[str, ...]


@dataclass(frozen=True)
class OverlapEdge:
    """Two PRs that touch the same file(s) but that Git *can* auto-merge — a semantic-review risk."""

    a: int
    b: int
    paths: tuple[str, ...]


@dataclass(frozen=True)
class OrderingEdge:
    """A directed "``before`` must land before ``after``" constraint (base-stacking or ancestry)."""

    before: int
    after: int
    reason: str  # "base-ref" | "ancestry"


@dataclass(frozen=True)
class HeldPr:
    """A PR that cannot land yet, with the reason(s) why (draft / base-conflict / depends-on-held)."""

    pr: int
    reasons: tuple[str, ...]


@dataclass(frozen=True)
class CollectedGraph:
    """The output of the collection layer: nodes + all raw edges, ready for the pure graph/plan."""

    repository: str
    base: str
    nodes: tuple[PrNode, ...]
    conflict_edges: tuple[ConflictEdge, ...]
    overlap_edges: tuple[OverlapEdge, ...]
    ordering_edges: tuple[OrderingEdge, ...]


@dataclass(frozen=True)
class PrActionDecision:
    """One PR's recommended action, the plain-language reason, and its parallel-safe group index."""

    pr: int
    action: PrAction
    why: str
    group: int | None = None


@dataclass(frozen=True)
class Plan:
    """The fused landing plan: parallel-safe groups, the land-now set, an order, and per-PR actions."""

    parallel_safe_groups: tuple[tuple[int, ...], ...]
    land_now: tuple[int, ...]
    order: tuple[int, ...]
    per_pr_actions: tuple[PrActionDecision, ...]
    batch: tuple[int, ...] = ()


@dataclass(frozen=True)
class Diagnostics:
    """The loud, first-class CI diagnostics (No Silent Failure): which reds are which, is CI down."""

    stale_gates: tuple[int, ...]
    flaky_reds: tuple[int, ...]
    real_reds: tuple[int, ...]
    evaluate_once_race: tuple[int, ...]
    outage_prs: tuple[int, ...]
    outage_suspected: bool


@dataclass(frozen=True)
class PlanResult:
    """The whole planner output: the collected graph, stacks, held PRs, the plan, and diagnostics."""

    graph: CollectedGraph
    stacks: tuple[tuple[int, ...], ...]
    held: tuple[HeldPr, ...]
    plan: Plan
    diagnostics: Diagnostics


def edge_key(a: int, b: int) -> tuple[int, int]:
    """Canonical unordered-pair key (min, max) for pairwise edges."""
    return (a, b) if a <= b else (b, a)


def dedupe_priority(labels: Sequence[str]) -> tuple[str, ...]:
    """Deterministic, order-preserving de-duplication of a label sequence."""
    seen: set[str] = set()
    out: list[str] = []
    for label in labels:
        if label not in seen:
            seen.add(label)
            out.append(label)
    return tuple(out)
