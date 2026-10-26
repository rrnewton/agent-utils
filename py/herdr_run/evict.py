"""Least-recently-used replacement of idle tabs once a workspace reaches ``max_panes``.

A new agent that needs a tab in a full workspace no longer has to wait for somebody to close one by
hand. Instead the tab whose last recorded ``herdr-run`` run is OLDEST is closed, provided its shell
is provably idle, and the new tab takes its place.

The same idle verdict also retires tabs on a lifetime (:func:`retire_idle_tabs`): a tab whose last
recorded run finished more than ``idle_tab_ttl_seconds`` ago is closed even when the workspace is
nowhere near the cap. Without that, the cap is the only closer, so every command workspace fills
to ``max_panes`` idle shells and stays there for as long as the server runs.

"Provably idle" is deliberately narrower than "looks quiet":

* no other ``herdr-run`` holds the pane's lock (the runner holds it for a whole command);
* the shell alone owns the terminal's foreground process group (:func:`assess_process`);
* the shell leads its own session and NO other process is in that session, so a background job
  (``cmd &``) or a stopped job keeps the tab open;
* the tab holds exactly one pane, because closing a tab closes every pane in it.

The verdict is taken under the pane lock, immediately before ``tab close``, never from an earlier
survey. When ``/proc`` cannot be read the tab counts as busy: failing closed only costs a refusal,
failing open would kill somebody's command. One window remains, and it is harmless: a caller that
resolved its target before the close, and takes the pane lock after it, finds the pane gone and
fails its readiness check before anything is typed.

Order comes from the run spool. Panes with no record at all -- tabs nobody has used through
``herdr-run``, including leaked ones ``reap`` cannot judge -- come first, in listing order.
"""

from __future__ import annotations

import fcntl
import math
import os
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from typing import BinaryIO

from herdr_run import audit, output
from herdr_run.client import HerdrClient, Pane
from herdr_run.config import Config
from herdr_run.errors import HerdrRunError, HerdrUnavailable
from herdr_run.identity import current_boot_id, process_start_ticks
from herdr_run.readiness import ProcessSignal, assess_process
from herdr_run.state import open_lock_file, pane_lock_path
from herdr_run.sweep import _scope_of, load_run_records
from herdr_run.textfmt import number_text, os_error_text, quote, quote_optional

__all__ = [
    "Candidate",
    "Eviction",
    "IdleSweep",
    "PaneReservation",
    "RetireCause",
    "RetiredTab",
    "Reuse",
    "SessionScan",
    "TtlCandidate",
    "describe_skipped",
    "evict_one",
    "judge_idle",
    "lru_candidates",
    "parse_session_id",
    "retire_idle_tabs",
    "reuse_idle_tab",
    "run_time",
    "scan_session",
    "ttl_candidates",
]

#: How many skipped tabs a refusal names before summarising the rest as a count.
_REFUSAL_DETAIL_LIMIT = 3


@dataclass(frozen=True)
class Candidate:
    """One tab considered for replacement, in least-recently-used order."""

    pane_id: str
    tab_id: str
    #: Run ID (spool directory name) of the pane's most recent recorded run, if any.
    last_run: str | None
    #: Agent label of that run, if any.
    agent: str | None
    #: Number of panes the listing shows in this tab.
    tab_panes: int


@dataclass(frozen=True)
class SessionScan:
    """What ``/proc`` says about the shell's session."""

    shell_pid: int
    #: Session ID of the shell, or None when its ``stat`` could not be read.
    shell_sid: int | None
    #: Other PIDs in the shell's session, ascending, or None when ``/proc`` could not be listed.
    others: tuple[int, ...] | None


@dataclass(frozen=True)
class Eviction:
    """One closed tab, and why it was judged idle."""

    candidate: Candidate
    reason: str


def lru_candidates(panes: Sequence[Pane], records: Sequence[Mapping[str, object]]) -> list[Candidate]:
    """Order the workspace's panes least-recently-used first.

    ``records`` must be oldest run first, as :func:`load_run_records` returns them, so the LAST
    record naming a pane is its most recent run. Panes without a record sort first; ties keep
    listing order.
    """
    latest: dict[str, tuple[str, str | None]] = {}
    for record in records:
        pane_id = record.get("pane_id")
        run_id = record.get("run_id")
        if not isinstance(pane_id, str) or not isinstance(run_id, str):
            continue
        agent = record.get("agent")
        latest[pane_id] = (run_id, agent if isinstance(agent, str) else None)
    candidates = []
    for pane in panes:
        last = latest.get(pane.pane_id)
        candidates.append(
            Candidate(
                pane_id=pane.pane_id,
                tab_id=pane.tab_id,
                last_run=last[0] if last else None,
                agent=last[1] if last else None,
                tab_panes=sum(1 for other in panes if other.tab_id == pane.tab_id),
            )
        )
    # Unrecorded panes first; `sorted` is stable, so ties keep listing order.
    return sorted(candidates, key=lambda c: (c.last_run is not None, c.last_run or ""))


def parse_session_id(stat_text: str) -> int | None:
    """Read the session ID (field 6) from the text of ``/proc/<pid>/stat``."""
    # The command name may itself contain spaces and parentheses; the fields resume after the LAST
    # ')'. They are then: state, ppid, pgrp, session.
    close = stat_text.rfind(")")
    if close < 0:
        return None
    fields = stat_text[close + 1 :].split()
    if len(fields) < 4:
        return None
    try:
        return int(fields[3])
    except ValueError:
        return None


def _read_session_id(pid: int, proc_root: str) -> int | None:
    try:
        with open(os.path.join(proc_root, str(pid), "stat"), encoding="utf-8", errors="replace") as handle:
            return parse_session_id(handle.read())
    except OSError:
        return None


def scan_session(shell_pid: int, proc_root: str = "/proc") -> SessionScan:
    """Find every other process in the session ``shell_pid`` leads."""
    shell_sid = _read_session_id(shell_pid, proc_root)
    others: tuple[int, ...] | None = None
    if shell_sid == shell_pid:
        try:
            names = os.listdir(proc_root)
        except OSError:
            names = None
        if names is not None:
            members = [
                int(name)
                for name in names
                if name.isdigit()
                and int(name) != shell_pid
                and _read_session_id(int(name), proc_root) == shell_pid
            ]
            others = tuple(sorted(members))
    return SessionScan(shell_pid=shell_pid, shell_sid=shell_sid, others=others)


def judge_idle(signal: ProcessSignal, session: SessionScan) -> tuple[bool, str]:
    """Combine the foreground-group verdict with the session scan into one idle verdict."""
    if not signal.idle:
        return False, signal.reason
    shell = session.shell_pid
    if session.shell_sid is None:
        return False, f"cannot read the session of shell {shell}; treating the tab as busy"
    if session.shell_sid != shell:
        return (
            False,
            f"shell {shell} does not lead its session (session {session.shell_sid}); "
            "treating the tab as busy",
        )
    if session.others is None:
        return False, f"cannot list processes in session {shell}; treating the tab as busy"
    if session.others:
        listed = ", ".join(str(pid) for pid in session.others)
        return False, f"session {shell} still holds {len(session.others)} other process(es): {listed}"
    return True, f"{signal.reason}; no other process in session {shell}"


def run_time(run_id: str) -> str | None:
    """Render a run ID's ``YYYYMMDDTHHMMSS`` prefix as an RFC 3339 UTC time."""
    stamp = run_id[:15]
    if len(stamp) != 15 or stamp[8] != "T" or not _ascii_digits(stamp[:8]) or not _ascii_digits(stamp[9:]):
        return None
    return f"{stamp[:4]}-{stamp[4:6]}-{stamp[6:8]}T{stamp[9:11]}:{stamp[11:13]}:{stamp[13:15]}Z"


def _ascii_digits(text: str) -> bool:
    return all("0" <= char <= "9" for char in text)


def evict_one(
    client: HerdrClient,
    config: Config,
    workspace_id: str,
    agent: str,
    proc_root: str = "/proc",
) -> Eviction | list[tuple[str, str]]:
    """Close the least-recently-used idle tab in ``workspace_id``.

    Returns the :class:`Eviction` after one close, or a list naming every tab that was considered
    and why it stayed open. Only listing the panes can fail outright.
    """
    panes = client.panes(workspace_id)
    records = load_run_records(config)
    skipped: list[tuple[str, str]] = []
    for candidate in lru_candidates(panes, records):
        verdict = _try_evict(client, config, candidate, workspace_id, proc_root)
        if isinstance(verdict, Eviction):
            _log_eviction(config, workspace_id, agent, verdict)
            return verdict
        skipped.append((candidate.pane_id, verdict))
    return skipped


def _try_evict(
    client: HerdrClient,
    config: Config,
    candidate: Candidate,
    workspace_id: str,
    proc_root: str,
    recheck: Callable[[Pane], str | None] | None = None,
) -> Eviction | str:
    claimed = _claim_idle(client, config, candidate, workspace_id, proc_root, recheck)
    if isinstance(claimed, str):
        return claimed
    eviction, lock = claimed
    try:
        client.close_tab(candidate.tab_id)
    except HerdrRunError as exc:
        return f"tab close failed: {exc}"
    finally:
        lock.close()
    return eviction


def _claim_idle(
    client: HerdrClient,
    config: Config,
    candidate: Candidate,
    workspace_id: str,
    proc_root: str,
    recheck: Callable[[Pane], str | None] | None = None,
) -> tuple[Eviction, BinaryIO] | str:
    """Take the candidate's pane lock without waiting and judge the tab idle under it.

    On success the lock is returned still HELD, with the idle verdict: the caller acts on the tab
    (closes it, or renames it for another agent) before letting go, so nothing can start in the
    pane between the verdict and the action. Every refusal releases the lock before returning.
    """
    refusal = _require_unsplit(candidate)
    if refusal is not None:
        return refusal
    try:
        lock = open_lock_file(pane_lock_path(candidate.pane_id))
    except HerdrRunError as exc:
        return f"cannot open the pane lock: {exc}"
    held = False
    try:
        try:
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return "another herdr-run holds the pane lock"
        except OSError as exc:
            return f"cannot lock the pane: {os_error_text(exc)}"
        # Judged now, under the lock, and closed at once: never from an earlier survey.
        # That includes the TAB's identity, not just the shell's: the listing the candidate
        # came from may be seconds old, and process-info names a pane and its shell, not the
        # tab `tab close` is about to destroy. Re-read the listing first: the pane may have
        # been split (a sibling this verdict never judged would die with the tab) or moved
        # to another tab (the close would land on a tab nobody judged).
        try:
            listed = client.panes(workspace_id)
        except HerdrRunError as exc:
            return f"herdr could not list panes before closing: {exc}"
        current = next((pane for pane in listed if pane.pane_id == candidate.pane_id), None)
        if current is None:
            return "herdr reports no such pane"
        if current.tab_id != candidate.tab_id:
            return (
                f"pane was in tab {quote(candidate.tab_id)} when listed but is now in tab "
                f"{quote(current.tab_id)}; refusing to guess"
            )
        fresh = Candidate(
            pane_id=candidate.pane_id,
            tab_id=candidate.tab_id,
            last_run=candidate.last_run,
            agent=candidate.agent,
            tab_panes=sum(1 for pane in listed if pane.tab_id == current.tab_id),
        )
        refusal = _require_unsplit(fresh)
        if refusal is not None:
            return refusal
        if recheck is not None:
            refusal = recheck(current)
            if refusal is not None:
                return refusal
        idle, reason = _judge_shell(client, config, fresh, proc_root)
        if not idle:
            return reason
        held = True
        return Eviction(candidate=fresh, reason=reason), lock
    finally:
        if not held:
            lock.close()


def _require_unsplit(candidate: Candidate) -> str | None:
    if candidate.tab_panes != 1:
        return f"tab {candidate.tab_id} holds {candidate.tab_panes} panes; only an unsplit tab is replaced"
    return None


def _judge_shell(client: HerdrClient, config: Config, candidate: Candidate, proc_root: str) -> tuple[bool, str]:
    try:
        info = client.process_info(candidate.pane_id)
    except HerdrRunError as exc:
        return False, f"process-info failed: {exc}"
    signal = assess_process(info, config)
    if not signal.idle:
        # The session scan reads every process's ``stat``; a shell that does not even own the
        # foreground is busy whatever it would find.
        return False, signal.reason
    return judge_idle(signal, scan_session(info.shell_pid, proc_root))


@dataclass(frozen=True)
class Survey:
    """What the cap path would find if a new agent arrived now."""

    #: Panes whose tab the cap path could replace, least-recently-used first.
    replaceable: list[str]
    #: Every other pane, with the reason its tab would stay open.
    kept: list[tuple[str, str]]


def survey(
    client: HerdrClient,
    config: Config,
    panes: Sequence[Pane],
    records: Sequence[Mapping[str, object]],
    proc_root: str = "/proc",
) -> Survey:
    """Judge every tab as :func:`evict_one` would, and close nothing.

    The pane lock is not taken: a report must not make a runner wait, and the lock is held only
    while a command runs, when the shell does not own the foreground anyway. So this is a reading
    of the moment, not a promise; the cap path decides again, under the lock, before any close.
    """
    replaceable: list[str] = []
    kept: list[tuple[str, str]] = []
    for candidate in lru_candidates(panes, records):
        refusal = _require_unsplit(candidate)
        if refusal is None:
            idle, reason = _judge_shell(client, config, candidate, proc_root)
            refusal = None if idle else reason
        if refusal is None:
            replaceable.append(candidate.pane_id)
        else:
            kept.append((candidate.pane_id, refusal))
    return Survey(replaceable=replaceable, kept=kept)


def survey_workspace(
    client: HerdrClient,
    config: Config,
    records: Sequence[Mapping[str, object]],
    proc_root: str = "/proc",
) -> Survey:
    """:func:`survey` the workspace ``config`` names, listed as the cap path lists it."""
    workspace_id = client.workspace_id_for_label(config.workspace)
    if workspace_id is None:
        raise HerdrUnavailable(f"herdr has no workspace labelled {quote(config.workspace)}")
    return survey(client, config, client.panes(workspace_id), records, proc_root)


def _log_eviction(config: Config, workspace_id: str, agent: str, eviction: Eviction) -> None:
    candidate = eviction.candidate
    last_run_at = run_time(candidate.last_run) if candidate.last_run is not None else None
    output.diagnostic(
        f"herdr-run: replaced idle tab {candidate.tab_id} (pane {candidate.pane_id}, agent "
        f"{candidate.agent or 'unknown'}, last run {last_run_at or 'none recorded'}) to make room "
        f"for '{agent}'"
    )
    path = audit.audit_path(config.project_root, config.spool_dir)
    if not audit.record(
        path,
        agent=agent,
        command=f"tab close {candidate.tab_id}",
        verdict="EVICTED",
        detail=eviction.reason,
        fields={
            "pane_id": candidate.pane_id,
            "tab_id": candidate.tab_id,
            "workspace_id": workspace_id,
            "evicted_agent": candidate.agent,
            "last_run": candidate.last_run,
            "last_run_at": last_run_at,
        },
    ):
        output.diagnostic(f"herdr-run: WARNING: could not append audit record to {path}")


def describe_skipped(skipped: Sequence[tuple[str, str]]) -> str:
    """Describe the tabs that stayed open, for the cap refusal."""
    parts = [f"pane {pane}: {reason}" for pane, reason in skipped[:_REFUSAL_DETAIL_LIMIT]]
    if len(skipped) > _REFUSAL_DETAIL_LIMIT:
        parts.append(f"and {len(skipped) - _REFUSAL_DETAIL_LIMIT} more")
    return f"None of its {len(skipped)} tab(s) could be replaced: {'; '.join(parts)}."


# --- idle retention: retire finished tabs past ``idle_tab_ttl_seconds`` or ``max_idle_tabs`` -----
#
# The cap above only ever closes a tab to make room for a new one, so a workspace fills to
# ``max_panes`` idle shells and stays there. Retirement is the deterministic counterpart, on two
# bounds: a tab whose last recorded run FINISHED more than the TTL ago is closed, and past
# ``max_idle_tabs`` idle tabs the least recently used ones are closed however recent they are --
# at the end of every run, or from ``herdr-run sweep --idle`` on a timer. Everything that makes
# the cap safe makes this safe, and it reuses those exact parts rather than growing a second
# notion of idle:
#
# * the candidate set is the panes a surviving run record names, in scope by the same
#   re-derivation ``reap`` applies (:func:`herdr_run.sweep._scope_of`). A tab with no record is
#   never a candidate here; ``reap`` judges those, and the cap replaces them first;
# * a pane whose latest records lack an exit code has work in flight and is kept -- ``reap``'s R1,
#   read over the records after the pane's last finished run (every run is recorded in flight the
#   moment it is typed; a later finished run proves the earlier ones ended). ``from_cache``
#   records count in full here: the flag
#   says the TARGET came from the session cache, and the command still ran and finished in this
#   pane, which is all a finish time needs. Such a pane is never COUNTED against
#   ``max_idle_tabs`` either, so the idle cap never limits how many commands run at once;
# * the finish is the completion marker's mtime, noted as ``finished_at`` by
#   :func:`herdr_run.sweep.load_run_records`. A finish that cannot be read is unknown, and an
#   unknown finish keeps the tab and keeps it out of the count;
# * the close itself is :func:`_try_evict`: unsplit tab, pane lock taken without waiting, idle
#   verdict under that lock immediately before ``tab close``. Expiry or rank is only ever the
#   reason to LOOK; it never authorises the close on its own.
#
# The idle count is read from the records, not probed: the most recently used ``max_idle_tabs``
# settled tabs are kept without a control call, and only the ones past them are probed, so the
# sweep at the end of a run costs one listing plus a probe per tab it actually closes.


@dataclass(frozen=True)
class TtlCandidate:
    """One live pane judged against the idle TTL, in listing order."""

    pane_id: str
    tab_id: str
    #: Run ID of the pane's most recent recorded run, if any record names the pane.
    last_run: str | None
    #: Agent label of that run, if recorded.
    agent: str | None
    #: Number of panes the listing shows in this tab.
    tab_panes: int
    #: Whether any run record names this pane at all.
    recorded: bool
    #: When the most recent recorded run finished (epoch seconds), when knowable.
    finished_at: float | None
    #: Whether every recorded run finished and the last finish is known: the tab counts as idle
    #: for ``max_idle_tabs`` (subject to scope) and may be ranked by how recently it was used.
    settled: bool
    #: Whether that finish is more than the TTL before ``now``.
    expired: bool
    #: Why the pane is or is not expired, recorded so a kept tab is never silent.
    reason: str


def ttl_candidates(
    panes: Sequence[Pane],
    records: Sequence[Mapping[str, object]],
    *,
    now: float,
    ttl_seconds: float,
) -> list[TtlCandidate]:
    """Judge every listed pane against the idle TTL. Pure: no client, no clock of its own.

    ``records`` must be oldest run first, as :func:`load_run_records` returns them, so the LAST
    record naming a pane is its most recent run. A pane is expired only when its most recent
    record recorded an exit code and that record's ``finished_at`` is a finite number more than
    ``ttl_seconds`` before ``now`` -- exactly at the boundary is within the TTL. Scope is
    not judged here; :func:`retire_idle_tabs` applies it before anything may close.
    """
    by_pane: dict[str, list[Mapping[str, object]]] = {}
    for record in records:
        pane_id = record.get("pane_id")
        if isinstance(pane_id, str):
            by_pane.setdefault(pane_id, []).append(record)
    candidates: list[TtlCandidate] = []
    for pane in panes:
        mine = by_pane.get(pane.pane_id, [])
        tab_panes = sum(1 for other in panes if other.tab_id == pane.tab_id)
        if not mine:
            candidates.append(
                TtlCandidate(
                    pane.pane_id, pane.tab_id, None, None, tab_panes, False, None, False, False, "no recorded run"
                )
            )
            continue
        last = mine[-1]
        run_id = last.get("run_id")
        agent = last.get("agent")
        finished = last.get("finished_at")
        finished_at = (
            float(finished)
            if isinstance(finished, (int, float))
            and not isinstance(finished, bool)
            and math.isfinite(finished)
            else None
        )
        # Only runs AFTER the pane's last finished one can still be in flight: runs in one pane are
        # serialised by its lock and by the idle check before each command, so a later run that
        # finished proves every earlier one ended (or was killed with the shell's prompt back),
        # even one whose own exit code never arrived.
        unfinished = 0
        for record in reversed(mine):
            if record.get("exit_code") is not None:
                break
            unfinished += 1
        settled = unfinished == 0 and finished_at is not None
        if unfinished:
            expired, reason = False, f"{unfinished} run(s) for this pane have no recorded exit_code"
        elif finished_at is None:
            expired, reason = False, "the finish time of this pane's last recorded run is unknown"
        elif ttl_seconds <= 0:
            expired, reason = False, "the idle TTL is disabled"
        elif now - finished_at > ttl_seconds:
            expired, reason = True, "last run finished more than the idle TTL ago"
        else:
            expired, reason = False, "last run finished within the idle TTL"
        candidates.append(
            TtlCandidate(
                pane_id=pane.pane_id,
                tab_id=pane.tab_id,
                last_run=run_id if isinstance(run_id, str) else None,
                agent=agent if isinstance(agent, str) else None,
                tab_panes=tab_panes,
                recorded=True,
                finished_at=finished_at,
                settled=settled,
                expired=expired,
                reason=reason,
            )
        )
    return candidates


class RetireCause:
    """Why an idle sweep closed a tab: the stable spellings used in JSON and the audit log."""

    #: Its last run finished more than ``idle_tab_ttl_seconds`` ago.
    IDLE_TTL = "idle_tab_ttl_seconds"
    #: It was less recently used than the ``max_idle_tabs`` idle tabs kept.
    MAX_IDLE_TABS = "max_idle_tabs"


@dataclass(frozen=True)
class RetiredTab:
    """One tab an idle sweep closed (or, in a dry run, would close), and why it may close."""

    pane_id: str
    tab_id: str
    agent: str | None
    last_run: str | None
    #: Which bound selected the tab: a :class:`RetireCause` value.
    cause: str
    reason: str


@dataclass(frozen=True)
class IdleSweep:
    """What one idle sweep did. ``listing_error`` set means herdr could not list the workspace."""

    #: TTL expiries oldest finish first, then the idle cap's, least recently used first.
    closed: tuple[RetiredTab, ...]
    would_close: tuple[RetiredTab, ...]
    #: Recorded panes left open, in listing order, each with its reason.
    kept: tuple[tuple[str, str], ...]
    listing_error: str | None


def _scope_refusal(
    verdict: TtlCandidate, records: Sequence[dict[str, object]], config: Config
) -> str | None:
    """Why a recorded pane is not ours to manage under the current configuration, if it is not."""
    in_scope, recorded_tab, tab_label, workspace_label = _scope_of(verdict.pane_id, records, config)
    if not in_scope:
        return (
            f"tab {quote_optional(tab_label)} in workspace "
            f"{quote_optional(workspace_label)} is not one of ours"
        )
    if recorded_tab is not None and recorded_tab != verdict.tab_id:
        return (
            f"pane was recorded in tab {quote(recorded_tab)} but is now in tab "
            f"{quote(verdict.tab_id)}; refusing to guess"
        )
    return None


def _most_recent_first(pool: Sequence[TtlCandidate]) -> list[TtlCandidate]:
    """Order settled tabs most recently used first: latest finish first, ties in listing order."""
    return sorted(pool, key=lambda item: -(item.finished_at or 0.0))


def _candidate_of(verdict: TtlCandidate) -> Candidate:
    return Candidate(
        pane_id=verdict.pane_id,
        tab_id=verdict.tab_id,
        last_run=verdict.last_run,
        agent=verdict.agent,
        tab_panes=verdict.tab_panes,
    )


def retire_idle_tabs(
    client: HerdrClient,
    config: Config,
    *,
    actor: str,
    now: float,
    dry_run: bool = False,
    exclude_pane_id: str | None = None,
    proc_root: str = "/proc",
) -> IdleSweep:
    """Close every tab in ``config``'s workspace that is idle past the TTL, then every idle tab past
    the ``max_idle_tabs`` most recently used.

    Expired tabs are examined oldest finish first. The idle cap then ranks the settled tabs still
    open by finish, newest first, keeps the first ``max_idle_tabs`` of them unprobed, and examines
    the rest least recently used first. ``exclude_pane_id`` keeps the sweep at the end of a run
    from closing the very tab the run just used (it still counts as idle). A dry run judges
    shells exactly as :func:`survey` does -- without the pane lock -- and closes nothing.
    """
    try:
        workspace_id = client.workspace_id_for_label(config.workspace)
        if workspace_id is None:
            return IdleSweep((), (), (), f"herdr has no workspace labelled {quote(config.workspace)}")
        panes = client.panes(workspace_id)
    except HerdrRunError as exc:
        return IdleSweep((), (), (), str(exc))
    records = load_run_records(config)
    verdicts = ttl_candidates(panes, records, now=now, ttl_seconds=config.idle_tab_ttl_seconds)

    # First pass: scope and incarnation, which need no probing. Outcomes are keyed by pane so
    # the kept list can be emitted in listing order even though tabs are examined in finish
    # order.
    kept_reasons: dict[str, str] = {}
    expired: list[TtlCandidate] = []
    pool: list[TtlCandidate] = []
    for verdict in verdicts:
        if not verdict.recorded:
            continue
        excluded = verdict.pane_id == exclude_pane_id
        refusal = _scope_refusal(verdict, records, config)
        if refusal is not None:
            if not excluded:
                kept_reasons[verdict.pane_id] = refusal
            continue
        if excluded:
            # The run that just finished here makes this the most recently used idle tab. It
            # takes its place in the count and is never examined.
            if verdict.settled and verdict.tab_panes == 1:
                pool.append(verdict)
            continue
        if verdict.expired:
            expired.append(verdict)
            continue
        kept_reasons[verdict.pane_id] = verdict.reason
        if verdict.settled and verdict.tab_panes == 1:
            pool.append(verdict)

    # Second pass: the idle cap ranks what the TTL left. Kept within the cap means kept for the
    # TTL's reason, which is still true; past it, the tab is examined like an expired one.
    over_cap: list[TtlCandidate] = []
    if config.max_idle_tabs > 0:
        over_cap = [
            verdict
            for verdict in _most_recent_first(pool)[config.max_idle_tabs :]
            if verdict.pane_id != exclude_pane_id
        ]
        over_cap.reverse()
    examine = [(verdict, RetireCause.IDLE_TTL) for verdict in sorted(expired, key=lambda item: item.finished_at or 0.0)]
    examine += [(verdict, RetireCause.MAX_IDLE_TABS) for verdict in over_cap]

    closed: list[RetiredTab] = []
    would_close: list[RetiredTab] = []
    for verdict, cause in examine:
        candidate = _candidate_of(verdict)
        if dry_run:
            refusal = _require_unsplit(candidate)
            if refusal is None:
                idle, reason = _judge_shell(client, config, candidate, proc_root)
                if idle:
                    kept_reasons.pop(verdict.pane_id, None)
                    would_close.append(
                        RetiredTab(verdict.pane_id, verdict.tab_id, verdict.agent, verdict.last_run, cause, reason)
                    )
                    continue
                refusal = reason
            kept_reasons[verdict.pane_id] = refusal
            continue

        def recheck(
            current: Pane, expected: TtlCandidate = verdict, need_expired: bool = cause == RetireCause.IDLE_TTL
        ) -> str | None:
            return _recheck_unchanged(config, current, expected, now, need_expired=need_expired)

        outcome = _try_evict(client, config, candidate, workspace_id, proc_root, recheck=recheck)
        if isinstance(outcome, Eviction):
            kept_reasons.pop(verdict.pane_id, None)
            done = RetiredTab(verdict.pane_id, verdict.tab_id, verdict.agent, verdict.last_run, cause, outcome.reason)
            closed.append(done)
            _log_retirement(config, workspace_id, actor, done)
        else:
            kept_reasons[verdict.pane_id] = outcome

    kept = tuple(
        (verdict.pane_id, kept_reasons[verdict.pane_id])
        for verdict in verdicts
        if verdict.pane_id in kept_reasons
    )
    return IdleSweep(tuple(closed), tuple(would_close), kept, None)


def _recheck_unchanged(
    config: Config, current: Pane, expected: TtlCandidate, now: float, *, need_expired: bool
) -> str | None:
    """Re-judge a candidate under the pane lock, from a fresh spool load.

    The sweep judged every candidate from one load, then examines them one by one; a run that
    starts AND finishes on a later candidate while earlier ones are examined would leave it idle
    with a free lock, and the stale verdict would close (or hand over) a tab whose last run
    finished seconds ago. Reloading here -- lock held, action imminent -- is the same discipline
    the shell verdict already follows. ``need_expired`` is the TTL's test; the idle cap and reuse
    need only that every run in the pane finished and its last run is still the one ranked.
    """
    fresh_records = load_run_records(config)
    fresh = ttl_candidates(
        [current], fresh_records, now=now, ttl_seconds=config.idle_tab_ttl_seconds
    )[0]
    if not fresh.recorded:
        return "no recorded run"
    refusal = _scope_refusal(fresh, fresh_records, config)
    if refusal is not None:
        return refusal
    if (need_expired and not fresh.expired) or not fresh.settled:
        return fresh.reason
    if fresh.last_run != expected.last_run:
        return "the pane's last recorded run changed during the sweep; treating the tab as busy"
    return None


def _log_retirement(config: Config, workspace_id: str, actor: str, retired: RetiredTab) -> None:
    last_run_at = run_time(retired.last_run) if retired.last_run is not None else None
    if retired.cause == RetireCause.IDLE_TTL:
        bound = f"idle past the {number_text(config.idle_tab_ttl_seconds)}s TTL"
    else:
        bound = f"less recently used than the {config.max_idle_tabs} idle tab(s) max_idle_tabs keeps"
    output.diagnostic(
        f"herdr-run: retired idle tab {retired.tab_id} (pane {retired.pane_id}, agent "
        f"{retired.agent or 'unknown'}, last run {last_run_at or 'none recorded'}) {bound}"
    )
    path = audit.audit_path(config.project_root, config.spool_dir)
    if not audit.record(
        path,
        agent=actor,
        command=f"tab close {retired.tab_id}",
        verdict="RETIRED",
        detail=retired.reason,
        fields={
            "pane_id": retired.pane_id,
            "tab_id": retired.tab_id,
            "workspace_id": workspace_id,
            "retired_agent": retired.agent,
            "last_run": retired.last_run,
            "last_run_at": last_run_at,
            "cause": retired.cause,
        },
    ):
        output.diagnostic(f"herdr-run: WARNING: could not append audit record to {path}")


# --- reuse: a new agent takes over the least recently used idle tab ----------------------------
#
# When a new agent needs a tab and the workspace already holds ``max_idle_tabs`` idle ones, a new
# tab would only push the least recently used of them out at the end of the run. Renaming that
# tab for the new agent reaches the same population without a close and a fresh shell. Below the
# cap a new tab is opened as before, so an agent that pauses between commands finds its own tab
# again. A command shares nothing with the previous one in the pane: the runner types
# ``command sh -c '...cd <dir> && <argv>...'`` and collects output from its own spool files.
#
# The tab is claimed exactly as a close is: unsplit, pane lock taken without waiting, every run in
# the pane finished and its last run unchanged from a fresh spool load, shell provably idle, all
# under the lock. Before the rename the live tab must still carry its recorded label and the shell
# must be the recorded process (:func:`_same_incarnation`), because a command is about to be typed
# there. The lock is then KEPT and handed to the run (:class:`PaneReservation`): between the
# verdict and the command nothing else can type into the pane. The previous owner's cached target
# still names the pane; its runner re-checks the tab's label once it holds the pane lock
# (:func:`herdr_run.runner.execute`) and resolves again when the tab went to another agent.


@dataclass(frozen=True)
class PaneReservation:
    """A pane lock held from the idle verdict that handed a tab over through the command typed
    into it. Closed when the resolved target that carries it is done with."""

    pane_id: str
    #: The open, exclusively locked pane lock file.
    lock: BinaryIO


@dataclass(frozen=True)
class Reuse:
    """A recorded idle tab renamed for a new agent, with its pane lock still held."""

    tab_id: str
    pane_id: str
    reservation: PaneReservation


def reuse_idle_tab(
    client: HerdrClient,
    config: Config,
    workspace_id: str,
    agent: str,
    tab_label: str,
    *,
    now: float,
    proc_root: str = "/proc",
) -> Reuse | None:
    """Take over the least recently used idle tab for ``agent`` instead of opening another one,
    when the workspace already holds ``max_idle_tabs`` idle tabs.

    None when the cap is disabled, when fewer idle tabs than the cap exist, when nothing can be
    listed, or when no candidate survives the under-lock verdict and the rename: the caller then
    opens a tab as it always has. Counted and ranked exactly as :func:`retire_idle_tabs` counts
    and ranks, except that expired tabs count too: they are idle, and the least recently used.
    """
    if config.max_idle_tabs <= 0:
        return None
    try:
        panes = client.panes(workspace_id)
    except HerdrRunError:
        return None
    records = load_run_records(config)
    verdicts = ttl_candidates(panes, records, now=now, ttl_seconds=config.idle_tab_ttl_seconds)
    pool = [
        verdict
        for verdict in verdicts
        if verdict.settled and verdict.tab_panes == 1 and _scope_refusal(verdict, records, config) is None
    ]
    if len(pool) < config.max_idle_tabs:
        return None
    for verdict in reversed(_most_recent_first(pool)):
        candidate = _candidate_of(verdict)

        def recheck(current: Pane, expected: TtlCandidate = verdict) -> str | None:
            return _recheck_unchanged(config, current, expected, now, need_expired=False)

        claimed = _claim_idle(client, config, candidate, workspace_id, proc_root, recheck)
        if isinstance(claimed, str):
            continue
        eviction, lock = claimed
        if _same_incarnation(client, config, candidate, workspace_id, proc_root) is not None:
            lock.close()
            continue
        try:
            client.rename_tab(candidate.tab_id, tab_label)
        except HerdrRunError:
            # The control call may have failed AFTER the rename took (a timeout): look the label
            # up rather than guess. Took: the tab is ours, lock still held. Did not: stop here,
            # because opening a tab under a label that may already be applied would leave two.
            try:
                found = client.tab_id_for_label(workspace_id, tab_label)
            except HerdrRunError:
                found = None
            if found != candidate.tab_id:
                lock.close()
                return None
        _log_reuse(config, workspace_id, agent, tab_label, candidate, eviction.reason)
        return Reuse(
            tab_id=candidate.tab_id,
            pane_id=candidate.pane_id,
            reservation=PaneReservation(pane_id=candidate.pane_id, lock=lock),
        )
    return None


def _same_incarnation(
    client: HerdrClient, config: Config, candidate: Candidate, workspace_id: str, proc_root: str
) -> str | None:
    """Why the live tab and shell are not the ones the pane's last run record describes, if not.

    Scope is judged from the records, and a record names a pane and tab by ID only. Before a tab is
    handed to another agent -- a command is about to be typed into it -- the live tab must still
    carry the label the record gave it (a human may have renamed and adopted it), and the pane's
    shell must be the recorded process: same pid, same start tick, same boot. A pane or tab ID
    Herdr reissued after a restart, or a shell that was replaced, fails here. Anything that cannot
    be read fails too.
    """
    records = load_run_records(config)
    latest = next(
        (record for record in reversed(records) if record.get("pane_id") == candidate.pane_id), None
    )
    if latest is None:
        return "no recorded run"
    tab = latest.get("tab")
    label = tab.get("label") if isinstance(tab, dict) else None
    if not isinstance(label, str):
        return "the last run record names no tab label"
    try:
        found = client.tab_id_for_label(workspace_id, label)
    except HerdrRunError:
        found = None
    if found != candidate.tab_id:
        return f"tab {candidate.tab_id} no longer carries the label {quote(label)}"
    readiness = latest.get("readiness")
    if not isinstance(readiness, dict):
        return "the last run record names no shell"
    recorded_pid = readiness.get("shell_pid")
    recorded_ticks = readiness.get("shell_start_ticks")
    recorded_boot = readiness.get("boot_id")
    try:
        info = client.process_info(candidate.pane_id)
    except HerdrRunError as exc:
        return f"process-info failed: {exc}"
    live_ticks = process_start_ticks(info.shell_pid, proc_root=proc_root)
    live_boot = current_boot_id(proc_root=proc_root)
    if (
        isinstance(recorded_pid, bool)
        or recorded_pid != info.shell_pid
        or not isinstance(recorded_ticks, int)
        or isinstance(recorded_ticks, bool)
        or recorded_ticks != live_ticks
        or not isinstance(recorded_boot, str)
        or recorded_boot != live_boot
    ):
        return "the pane's shell is not the one its last run recorded"
    return None


def _log_reuse(
    config: Config, workspace_id: str, agent: str, tab_label: str, candidate: Candidate, reason: str
) -> None:
    last_run_at = run_time(candidate.last_run) if candidate.last_run is not None else None
    output.diagnostic(
        f"herdr-run: reused idle tab {candidate.tab_id} (pane {candidate.pane_id}, agent "
        f"{candidate.agent or 'unknown'}, last run {last_run_at or 'none recorded'}) for '{agent}' "
        f"rather than open another past max_idle_tabs {config.max_idle_tabs}"
    )
    path = audit.audit_path(config.project_root, config.spool_dir)
    if not audit.record(
        path,
        agent=agent,
        command=f"tab rename {candidate.tab_id}",
        verdict="REUSED",
        detail=reason,
        fields={
            "pane_id": candidate.pane_id,
            "tab_id": candidate.tab_id,
            "tab_label": tab_label,
            "workspace_id": workspace_id,
            "previous_agent": candidate.agent,
            "last_run": candidate.last_run,
            "last_run_at": last_run_at,
        },
    ):
        output.diagnostic(f"herdr-run: WARNING: could not append audit record to {path}")
