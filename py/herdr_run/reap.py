"""Reaping policy for stale herdr tabs.

Agents are coined and destroyed continuously, and every one that ever ran a command through
``herdr-run`` leaves a tab behind. This module decides which of those tabs are *provably* finished
with, so they can be closed, and -- much more importantly -- which ones must be left alone.

WHY THE OBVIOUS SIGNALS DO NOT WORK
-----------------------------------
herdr exposes no tab creation time and no last-activity time. ``PaneInfo.revision`` looks like an
activity counter and is not: it counts pane *metadata* revisions, and stays put while the terminal
produces visible output. So there is no "last used" field to threshold on.

Worse, the tempting substitute -- "no output for N minutes" -- cannot work even in principle here.
A *finished* agent and a *dead* agent are identical on the silence axis. Any absence-of-output
detector conflates them, and the expensive direction of that mistake is killing an agent that is
merely thinking.

WHY THE RUN-DIRECTORY PID IS THE WRONG ANCHOR (measured, not assumed)
---------------------------------------------------------------------
The run spool is ``.herdr-run/runs/<utc>-<agent>-<pid>/``, and it is tempting to read that trailing
PID as "the process that owns this tab" and treat its death as evidence of staleness. **That PID is
the short-lived ``herdr-run`` CLI invocation, not the agent and not the pane.** It exits the moment
the command finishes, so it is dead by construction for every completed run.

Measured on the live spool (15 run directories, 2026-08-06): the run-directory PID was dead for
**15 of 15**, while the pane shell recorded in the same ``meta.json`` was alive for **15 of 15**.
A policy anchored on the run-directory PID would therefore have classified every tab whose commands
had all finished -- which is every healthy idle agent -- as stale, and reaped the entire workspace.

The correct anchor is the **pane's shell process**, which is what actually owns the tab.

THE PREDICATE
-------------
A tab is STALE only when all of these hold, each positive evidence rather than absence:

* **R1 no in-flight work.** Every run naming this pane recorded an ``exit_code``. A run with no
  recorded exit code is IN FLIGHT -- that is the "agent is thinking" case, and it is positively
  distinguishable from a finished one rather than being inferred from silence.
* **R2 the pane's shell is gone.** Not the run-directory PID; the shell PID.
* **R3 PID reuse is excluded.** ``kill(pid, 0)`` alone is not sufficient on a box running ~20
  agents: a recycled PID makes an unrelated live process look like proof of liveness, and a
  same-numbered stranger look like proof of death. Identity is bound as
  ``(pid, boot_id, start_ticks)`` -- field 22 of ``/proc/<pid>/stat`` -- which is the shape
  ``ci-hub/lib/validate_lock.rs`` already uses for lock owners.

Anything else -- unreadable ``/proc``, missing ``meta.json``, absent ``pane_id``, a boot-id
mismatch, a pane herdr does not know about -- is UNKNOWN, and unknown is never reaped. The cost
asymmetry is the whole reason: killing an agent mid-work is far worse than clutter, and a reaper
that is wrong once in the expensive direction gets switched off permanently.

SCOPE IS ENFORCED, NOT TRUSTED
------------------------------
Only panes belonging to tabs in *our* workspace, whose labels match the configured tab schema, are
even considered. A tab a human opened by hand, or another tool's workspace, is not a candidate no
matter how idle it looks.

REPORT-ONLY BY DEFAULT
----------------------
``plan_reap`` decides; it never closes anything. Closing is a separate, explicit step. A reaper that
is trusted before it has been checked against a known-good population is how the whole workspace
gets deleted once.

This module holds no I/O. Evidence is passed in, so the policy is testable without a live herdr and
without waiting for real processes to die -- and so the planted-stale and planted-live cases can
both be exercised deterministically.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field

__all__ = [
    "PaneEvidence",
    "ProcessIdentity",
    "ReapDecision",
    "ReapPlan",
    "Verdict",
    "plan_reap",
]


class Verdict:
    """Why a pane was or was not selected. Strings so they survive into JSON and logs."""

    STALE = "STALE"
    IN_FLIGHT = "IN_FLIGHT"
    SHELL_ALIVE = "SHELL_ALIVE"
    UNKNOWN = "UNKNOWN"
    OUT_OF_SCOPE = "OUT_OF_SCOPE"


@dataclass(frozen=True)
class ProcessIdentity:
    """A PID bound to the boot and start-tick that make it *that* process.

    ``boot_id`` and ``start_ticks`` are what stop a recycled PID from being mistaken for the
    original. Either being ``None`` means we could not establish identity, which is UNKNOWN --
    never a licence to reap.
    """

    pid: int | None
    boot_id: str | None = None
    start_ticks: int | None = None

    def is_bound(self) -> bool:
        """True when pid, boot_id and start_ticks are all known, so reuse can be excluded."""
        return self.pid is not None and self.boot_id is not None and self.start_ticks is not None

    def same_process_as(self, other: "ProcessIdentity") -> bool:
        """True only if both identities are bound AND agree on pid, boot and start tick."""
        if not (self.is_bound() and other.is_bound()):
            return False
        return (
            self.pid == other.pid
            and self.boot_id == other.boot_id
            and self.start_ticks == other.start_ticks
        )


@dataclass(frozen=True)
class PaneEvidence:
    """Everything the policy is allowed to look at for one pane.

    ``recorded_shell`` is the identity captured in ``meta.json`` when the run happened.
    ``live_shell`` is the identity of that PID *right now*, or ``None`` if the PID is gone. The two
    are compared rather than either being trusted alone: that comparison is what excludes reuse.
    """

    pane_id: str
    tab_id: str | None = None
    tab_label: str | None = None
    workspace_label: str | None = None
    in_scope: bool = False
    #: One entry per run naming this pane. ``True`` means an exit code was recorded.
    run_exit_codes_recorded: Sequence[bool] = field(default_factory=tuple)
    recorded_shell: ProcessIdentity | None = None
    live_shell: ProcessIdentity | None = None
    #: False when herdr no longer lists the pane, or we could not ask.
    pane_known_to_herdr: bool = True
    #: Set when evidence could not be gathered; forces UNKNOWN with this reason.
    evidence_error: str | None = None


@dataclass(frozen=True)
class ReapDecision:
    """One pane's verdict, with the reason recorded so a refusal can be audited."""

    pane_id: str
    tab_id: str | None
    tab_label: str | None
    verdict: str
    reason: str

    @property
    def reapable(self) -> bool:
        """Only a STALE verdict authorises closing the tab."""
        return self.verdict == Verdict.STALE


@dataclass(frozen=True)
class ReapPlan:
    """The full set of verdicts for one sweep. Decides only; closes nothing."""

    decisions: tuple[ReapDecision, ...]

    @property
    def reapable(self) -> tuple[ReapDecision, ...]:
        """Panes proven stale. Everything else is spared."""
        return tuple(d for d in self.decisions if d.reapable)

    @property
    def declined(self) -> tuple[ReapDecision, ...]:
        """Panes left alone, each with the reason -- so "reaped nothing" is never silent."""
        return tuple(d for d in self.decisions if not d.reapable)

    def counts(self) -> dict[str, int]:
        """Counts for EVERY verdict, including the zeros.

        Reported unconditionally because "reaped 0 because nothing was stale" and "reaped 0 because
        the detector is inert" are indistinguishable from a bare total.
        """
        out = {
            Verdict.STALE: 0,
            Verdict.IN_FLIGHT: 0,
            Verdict.SHELL_ALIVE: 0,
            Verdict.UNKNOWN: 0,
            Verdict.OUT_OF_SCOPE: 0,
        }
        for decision in self.decisions:
            out[decision.verdict] = out.get(decision.verdict, 0) + 1
        out["considered"] = len(self.decisions)
        return out


def _decide(evidence: PaneEvidence) -> ReapDecision:
    def decision(verdict: str, reason: str) -> ReapDecision:
        return ReapDecision(
            pane_id=evidence.pane_id,
            tab_id=evidence.tab_id,
            tab_label=evidence.tab_label,
            verdict=verdict,
            reason=reason,
        )

    # Scope first: a tab outside our workspace/schema is never a candidate, however idle it looks.
    if not evidence.in_scope:
        return decision(
            Verdict.OUT_OF_SCOPE,
            f"tab {evidence.tab_label!r} in workspace {evidence.workspace_label!r} "
            "is not one of ours",
        )

    if evidence.evidence_error is not None:
        return decision(Verdict.UNKNOWN, f"evidence unavailable: {evidence.evidence_error}")

    if not evidence.pane_known_to_herdr:
        # The tab may already be gone, or herdr may just not have answered. Either way there is
        # nothing to close and no basis to claim staleness.
        return decision(Verdict.UNKNOWN, "herdr does not list this pane")

    # R1 -- in-flight work beats every other signal. This is the "agent is thinking" case.
    unfinished = sum(1 for recorded in evidence.run_exit_codes_recorded if not recorded)
    if unfinished:
        return decision(
            Verdict.IN_FLIGHT,
            f"{unfinished} run(s) for this pane have no recorded exit_code",
        )

    # R3 -- we must have a bound identity from when the run happened, or we cannot reason at all.
    recorded = evidence.recorded_shell
    if recorded is None or not recorded.is_bound():
        return decision(
            Verdict.UNKNOWN,
            "no identity-bound shell recorded for this pane (need pid+boot_id+start_ticks)",
        )

    live = evidence.live_shell

    # R2 -- the shell is gone. Note this is the PANE shell, not the run-directory PID, which is the
    # short-lived herdr-run CLI and is dead for every completed run.
    if live is None:
        return decision(
            Verdict.STALE,
            f"all runs finished and pane shell {recorded.pid} is gone "
            f"(identity {recorded.boot_id}:{recorded.start_ticks})",
        )

    if not live.is_bound():
        return decision(Verdict.UNKNOWN, f"could not bind identity of live pid {live.pid}")

    if recorded.same_process_as(live):
        return decision(Verdict.SHELL_ALIVE, f"pane shell {live.pid} is still the original process")

    # Same PID number, different process: reuse. Deliberately UNKNOWN, not STALE -- the original is
    # probably gone, but "probably" is not the standard for closing someone's tab.
    return decision(
        Verdict.UNKNOWN,
        f"pid {recorded.pid} is now a DIFFERENT process "
        f"(recorded {recorded.boot_id}:{recorded.start_ticks}, "
        f"live {live.boot_id}:{live.start_ticks}) -- PID reuse, refusing to guess",
    )


def plan_reap(evidence: Iterable[PaneEvidence]) -> ReapPlan:
    """Decide, for each pane, whether it is provably stale. Closes nothing."""
    return ReapPlan(tuple(_decide(item) for item in evidence))


def evidence_from_runs(
    pane_id: str,
    runs: Iterable[Mapping[str, object]],
) -> tuple[tuple[bool, ...], ProcessIdentity | None]:
    """Fold this pane's run records into (exit-code-recorded flags, recorded shell identity).

    ``runs`` are parsed ``meta.json`` mappings. Only records naming ``pane_id`` contribute. A record
    with ``from_cache`` true never launched anything, so it is not evidence that a pane exists and
    is skipped -- otherwise a cache hit would manufacture a phantom candidate.
    """
    recorded_flags: list[bool] = []
    identity: ProcessIdentity | None = None
    for record in runs:
        if record.get("pane_id") != pane_id:
            continue
        if record.get("from_cache") is True:
            continue
        recorded_flags.append(record.get("exit_code") is not None)
        readiness = record.get("readiness")
        if identity is None and isinstance(readiness, Mapping):
            shell_pid = readiness.get("shell_pid")
            if isinstance(shell_pid, int):
                boot = readiness.get("boot_id")
                ticks = readiness.get("shell_start_ticks")
                identity = ProcessIdentity(
                    pid=shell_pid,
                    boot_id=boot if isinstance(boot, str) else None,
                    start_ticks=ticks if isinstance(ticks, int) else None,
                )
    return tuple(recorded_flags), identity
