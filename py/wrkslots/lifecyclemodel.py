"""Exhaustive lifecycle model for one wrkslots slot (#1568).

This module is a checked abstraction, not runtime code: nothing in the
removal path imports it.  It compresses the observed slot lifecycle
(registry row, storage, owner proof, handoff, hold, history, occupancy,
journal, validate publication, coordinator generation) into a finite
state tuple and explores every reachable state under a scenario.

Two questions are answered mechanically:

* Safety: no ``Remove`` transition ever archives a slot whose owner is
  truly alive, is held, has an unread handoff, is occupied, or holds
  unarchived private history.  Violations are recorded in a sticky
  ghost field so a buggy variant produces a reachable counterexample
  instead of an assertion buried in one transition.
* Liveness: from every reachable state with a truly dead owner, no
  occupancy and no unread handoff, the *scheduled* edge set (the
  finisher policy plus automatic fix transitions and the passage of
  time) must reach the archived terminal.  Human-discretionary edges
  (manual unhold, reading a handoff, stopping a leftover process)
  are deliberately excluded from the scheduled set: a state that
  needs one is a named manual terminal, not a finisher obligation.

Fixes and historical bugs are scenario constants, never state, so a
baseline run reproduces each observed leak class as a reachable
liveness counterexample and single-fix runs show which fix closes it.
The TLA+ text in ``spec/WrkslotsLifecycle.tla`` mirrors this action
table; this explorer is the gated checker because it needs no
downloaded toolchain.

Simplifications, each conservative for the properties above: detached
HEAD folds into branch drift; finish/remove/batch-seal journals fold
into one lifecycle journal; cross-slot effects are two scenario flags
(another slot's blocking journal, another slot's Eden row); the
registered-liveness answer folds into the owner-proof axis.
"""

from __future__ import annotations

import dataclasses
from collections import deque
from typing import NamedTuple

# Fix flags (scenario constants).
FINISHER = "finisher"
DELEGATED = "delegated"
HOLD_EXPIRY = "hold-expiry"
REREGISTER = "reregister-branch"
PERSLOT = "per-slot-journals"
AUTO_UNBOUND = "auto-recover-unbound"
LEDGER_ABANDON = "ledger-abandon"
EDEN_REMOVE = "eden-remove"
AUDIT_SUBMODULE = "audit-submodule"

ALL_FIXES = frozenset(
    {
        FINISHER,
        DELEGATED,
        HOLD_EXPIRY,
        REREGISTER,
        PERSLOT,
        AUTO_UNBOUND,
        LEDGER_ABANDON,
        EDEN_REMOVE,
        AUDIT_SUBMODULE,
    }
)

# Historical-bug toggles (scenario constants).
BUG_EDEN_GUARD = "eden-guard"
BUG_TTL_UNFIXED = "ttl-unfixed"
BUG_APPLY_JOURNAL = "apply-journal"


class State(NamedTuple):
    row: str  # absent | active | archived
    storage: str  # absent | present
    truth: str  # alive | dead (environment truth about the owner)
    proof: str  # unbound | live | dead | indet (what guards may use)
    hb: int  # 0 fresh, 1 expired
    handoff: str  # none | unread | enqueued
    hold: str  # none | held
    hage: int  # hold age ticks 0..2
    history: str  # contained | visible | hidden (private history)
    branch: str  # ok | drift (detached folds into drift)
    nested: int  # nested registration without a row
    occ: str  # none | live | deleted (leftover in a deleted cwd)
    journal: str  # none | create | lifecycle
    pub: str  # unproven | mirror | proven | abandoned
    retries: int  # ledger publish retries 0..2
    coord: str  # orig | restarted
    viol: str  # none | sticky safety-violation tag


@dataclasses.dataclass(frozen=True)
class Scenario:
    slot_type: str = "agent"  # agent | validate
    vcs: str = "git"  # git | eden (subject slot)
    other_journal: int = 0  # another slot's blocking journal exists
    other_eden: int = 0  # another slot's Eden row exists
    mirror_fails: int = 0  # ledger mirror never succeeds
    fixes: frozenset[str] = frozenset()
    bugs: frozenset[str] = frozenset()


def initial_states(sc: Scenario) -> tuple[State, ...]:
    pub = "proven" if sc.slot_type == "agent" else "unproven"
    base = State(
        row="absent",
        storage="absent",
        truth="alive",
        proof="unbound",
        hb=0,
        handoff="none",
        hold="none",
        hage=0,
        history="contained",
        branch="ok",
        nested=0,
        occ="none",
        journal="none",
        pub=pub,
        retries=0,
        coord="orig",
        viol="none",
    )
    # A directory that exists before any row (imported or stray storage).
    stray = base._replace(storage="present")
    return (base, stray)


def is_terminal(s: State) -> bool:
    return s.row == "archived" and s.storage == "absent"


def _authorized(s: State, sc: Scenario) -> bool:
    return s.coord == "orig" or DELEGATED in sc.fixes


def _journal_blocked(s: State, sc: Scenario) -> bool:
    if s.journal != "none":
        return True
    return bool(sc.other_journal) and PERSLOT not in sc.fixes


def _owner_ok(s: State) -> bool:
    # A recorded generation proven dead, or no recorded owner at all
    # with an expired heartbeat (the ownerless admission path).
    return s.proof == "dead" or (s.proof == "unbound" and s.hb == 1)


def remove_guards(s: State, sc: Scenario, *, archive: bool, drift_ok: bool) -> bool:
    if s.row != "active" or s.storage != "present":
        return False
    if _journal_blocked(s, sc) or not _authorized(s, sc):
        return False
    if s.hold != "none" or s.handoff == "unread" or s.occ != "none":
        return False
    if s.nested or not _owner_ok(s):
        return False
    if not drift_ok and s.branch != "ok":
        return False
    if not archive and s.history != "contained":
        return False
    if sc.slot_type == "validate" and s.pub not in ("proven", "abandoned"):
        return False
    if sc.vcs == "eden" and EDEN_REMOVE not in sc.fixes:
        return False
    return True


def _archive(s: State, *, via_ttl: bool = False) -> State:
    # Archiving a row whose storage is already absent destroys no
    # work, so the safety ghost only judges archives of present
    # storage (the absent-row recovery commands re-prove their own
    # owner preconditions before acting).
    viol = s.viol
    if viol == "none" and s.storage == "present":
        if s.truth == "alive":
            viol = "live-owner"
        elif s.hold != "none":
            viol = "held"
        elif s.handoff == "unread":
            viol = "unread-handoff"
        elif s.occ != "none":
            viol = "occupied"
    return s._replace(row="archived", storage="absent", journal="none", viol=viol)


# Edge kinds: env (faults/clocks), owner, coord (exists today, run by a
# person), human (discretionary judgment), sched (finisher policy or an
# automatic fix transition; the liveness edge set also includes Tick).
def successors(s: State, sc: Scenario) -> list[tuple[str, str, State]]:
    out: list[tuple[str, str, State]] = []

    def add(name: str, kind: str, nxt: State) -> None:
        if nxt != s:
            out.append((name, kind, nxt))

    auth = _authorized(s, sc)
    blocked = _journal_blocked(s, sc)
    fin = FINISHER in sc.fixes

    # --- creation -------------------------------------------------
    if s.row == "absent" and s.storage == "absent" and s.journal == "none":
        eden_bug = BUG_EDEN_GUARD in sc.bugs and sc.other_eden and sc.vcs == "git"
        if not blocked and not eden_bug:
            add(
                "CreateBound",
                "owner",
                s._replace(row="active", storage="present", proof="live", occ="live"),
            )
            add(
                "CreateUnbound",
                "owner",
                s._replace(row="active", storage="present", proof="unbound", occ="live"),
            )
        add("CreateCrash", "env", s._replace(storage="present", journal="create"))
    if s.journal == "create" and auth:
        add(
            "RecoverCreate",
            "sched" if fin else "coord",
            s._replace(row="active", storage="present", proof="live", occ="live", journal="none"),
        )
        add(
            "AbortCreate",
            "coord",
            s._replace(row="absent", storage="absent", journal="none"),
        )

    if s.row != "active":
        return out

    # --- clocks and environment -----------------------------------
    if s.hb == 0 or (s.hold == "held" and s.hage < 2):
        add(
            "Tick",
            "env",
            s._replace(hb=1, hage=min(2, s.hage + 1) if s.hold == "held" else s.hage),
        )
    if s.coord == "orig":
        add("CoordRestart", "env", s._replace(coord="restarted"))
    if s.truth == "alive":
        add(
            "OwnerExit",
            "env",
            s._replace(truth="dead", occ="none" if s.occ == "live" else s.occ),
        )
        add("Heartbeat", "owner", s._replace(hb=0))
        if s.handoff == "none":
            add("WriteHandoff", "owner", s._replace(handoff="unread"))
        if s.history == "contained":
            add("MakePrivateVisible", "owner", s._replace(history="visible"))
            add("MakePrivateHidden", "owner", s._replace(history="hidden"))
        if s.branch == "ok":
            add("BranchDrift", "owner", s._replace(branch="drift"))
        if not s.nested:
            add("NestedAdd", "owner", s._replace(nested=1))
        if s.proof == "unbound":
            add("Adopt", "owner", s._replace(proof="live"))
    else:
        if s.proof in ("live", "indet"):
            add(
                "ProbeDead",
                "sched" if (fin or AUTO_UNBOUND in sc.fixes) else "coord",
                s._replace(proof="dead"),
            )
        if s.proof == "live":
            add("ProbeIndet", "env", s._replace(proof="indet"))
        if s.occ == "none":
            add("SpawnLeftover", "env", s._replace(occ="deleted"))
    if s.occ == "deleted":
        add("LeftoverExit", "human", s._replace(occ="none"))
    if s.storage == "present" and s.journal == "none":
        add("ExtDirDelete", "env", s._replace(storage="absent"))

    # --- coordinator / human --------------------------------------
    if s.handoff == "unread" and auth and not blocked:
        add("ReadHandoff", "human", s._replace(handoff="enqueued"))
    if s.hold == "none":
        add("Hold", "human", s._replace(hold="held", hage=0))
    else:
        add("Unhold", "human", s._replace(hold="none", hage=0))
        if s.hage:
            add("ReaffirmHold", "human", s._replace(hage=0))
        if HOLD_EXPIRY in sc.fixes and s.hage == 2:
            add("HoldExpire", "sched", s._replace(hold="none", hage=0))
    if s.proof == "unbound" and s.truth == "dead" and auth:
        add(
            "RecoverUnbound",
            "sched" if (fin and AUTO_UNBOUND in sc.fixes) else "coord",
            s._replace(proof="dead"),
        )
    if s.nested and auth and not blocked:
        add("AdoptCheckout", "sched" if fin else "coord", s._replace(nested=0))
    if REREGISTER in sc.fixes and s.branch == "drift" and s.proof == "dead" and auth:
        add("ReregisterBranch", "sched" if fin else "coord", s._replace(branch="ok"))
    if s.journal == "lifecycle" and auth:
        add("Recover", "sched" if fin else "coord", s._replace(journal="none"))
        add("ConfigAbortRetry", "coord", s._replace(journal="none"))

    # --- validate publication -------------------------------------
    if sc.slot_type == "validate" and s.pub in ("unproven", "mirror"):
        if not sc.mirror_fails:
            add(
                "LedgerPublishOk",
                "sched" if fin else "coord",
                s._replace(pub="proven"),
            )
        if s.retries < 2:
            add(
                "LedgerPublishFail",
                "sched" if fin else "coord",
                s._replace(pub="mirror", retries=s.retries + 1),
            )
        if LEDGER_ABANDON in sc.fixes and s.pub == "mirror" and s.retries == 2:
            add("LedgerAbandon", "sched" if fin else "coord", s._replace(pub="abandoned"))

    # --- terminal transitions -------------------------------------
    if remove_guards(s, sc, archive=False, drift_ok=False):
        add("Remove", "sched" if fin else "coord", _archive(s))
        add("RemoveCrash", "env", s._replace(journal="lifecycle"))
    if remove_guards(s, sc, archive=True, drift_ok=False):
        # Private history without an archive bundle must never archive.
        nxt = _archive(s)
        add("RemoveArchive", "sched" if fin else "coord", nxt)
    if remove_guards(s, sc, archive=True, drift_ok=True):
        add("RemoveDrift", "coord", _archive(s))
    if s.storage == "absent" and s.journal == "none" and auth and s.hold == "none":
        eden_absent_bug = BUG_EDEN_GUARD in sc.bugs and sc.vcs == "eden"
        if s.handoff != "unread" and _owner_ok(s) and not eden_absent_bug:
            if BUG_APPLY_JOURNAL in sc.bugs:
                add(
                    "RecoverAbsentRefusedApply",
                    "coord",
                    s._replace(journal="lifecycle"),
                )
            else:
                add(
                    "RecoverAbsent",
                    "sched" if fin else "coord",
                    _archive(s),
                )
    # TTL teardown: heartbeat rung plus live/held, handoff, and
    # occupancy gates (remove-grade since the teardown fix).  The
    # buggy toggle restores the heartbeat-only historical form.
    if s.storage == "present" and s.journal == "none" and s.hb == 1:
        if BUG_TTL_UNFIXED in sc.bugs:
            add("TtlTeardownBuggy", "coord", _archive(s, via_ttl=True))
        elif (
            s.hold == "none"
            and s.proof == "dead"
            and s.handoff != "unread"
            and s.occ == "none"
        ):
            add("TtlTeardown", "coord", _archive(s, via_ttl=True))
    return out


def audit_verdict(s: State, sc: Scenario) -> str:
    """The audit view: advisory, stored refs only, no caller axis."""
    if s.row != "active":
        return "NONE"
    if s.hold == "held":
        return "HELD"
    if s.storage == "absent":
        ok = s.handoff != "unread" and _owner_ok(s) and s.journal == "none"
        return "RECOVERABLE" if ok else "BLOCKED"
    view = s
    if s.history == "hidden" and AUDIT_SUBMODULE not in sc.fixes:
        view = s._replace(history="contained")
    probe = dataclasses.replace(sc, other_journal=0)
    view = view._replace(coord="orig", journal="none")
    if sc.vcs == "eden":
        probe = dataclasses.replace(probe, fixes=probe.fixes | {EDEN_REMOVE})
    return (
        "DELETABLE"
        if remove_guards(view, probe, archive=False, drift_ok=False)
        else "BLOCKED"
    )


def why_stayed(s: State, sc: Scenario) -> str:
    """Primary leak class, using the evidence precedence order."""
    if is_terminal(s):
        return "ARCHIVED"
    if s.hold == "held":
        return "HELD"
    if s.journal == "lifecycle" or (sc.other_journal and PERSLOT not in sc.fixes):
        return "JOURNAL_BLOCKED_DEAD_OWNER"
    if s.journal == "create":
        return "INTERRUPTED_CREATE_JOURNAL"
    if s.row == "active" and s.storage == "absent":
        return "ROW_WITHOUT_DIRECTORY"
    if s.row == "absent" and s.storage == "present":
        return "DIRECTORY_WITHOUT_ROW"
    if s.handoff == "unread":
        return "UNREAD_HANDOFF"
    if s.occ == "deleted":
        return "LEFTOVER_PROCESS_IN_DELETED_CWD"
    if s.truth == "alive" or s.occ == "live":
        return "LIVE_OWNER_OR_OCCUPANT"
    if s.branch == "drift":
        return "BRANCH_DRIFT_OR_DETACHED"
    if s.history != "contained":
        return "PRIVATE_OR_UNPUSHED_HISTORY"
    if s.proof in ("unbound", "indet", "live"):
        return "NO_OWNER_GENERATION_INDETERMINATE"
    if s.nested:
        return "NESTED_GIT_REGISTRATION_WITHOUT_ROW"
    if sc.slot_type == "validate" and s.pub in ("unproven", "mirror"):
        return "VALIDATE_LEDGER_NOT_PUBLISHED_PROVEN"
    if s.handoff == "enqueued":
        return "RETIREMENT_QUEUE_PENDING_NO_REMOVER"
    if remove_guards(s, sc, archive=True, drift_ok=False):
        return "DEAD_DELETABLE_NO_AUTOMATIC_REMOVER"
    return "DEAD_BLOCKED_OTHER"


def in_liveness_domain(s: State) -> bool:
    """Dead, unoccupied, no unread handoff: the finisher's obligation."""
    return (
        s.row == "active"
        and s.truth == "dead"
        and s.occ == "none"
        and s.handoff != "unread"
        and not is_terminal(s)
    )


@dataclasses.dataclass
class Exploration:
    scenario: Scenario
    states: set[State]
    edges: dict[State, list[tuple[str, str, State]]]
    parent: dict[State, tuple[State, str] | None]

    def trace(self, target: State) -> list[str]:
        steps: list[str] = []
        cur: State | None = target
        while cur is not None:
            prev = self.parent.get(cur)
            if prev is None:
                break
            steps.append(prev[1])
            cur = prev[0]
        return list(reversed(steps))

    def scheduled_reachable_terminals(self) -> set[State]:
        """States that reach a terminal using sched edges plus Tick."""
        reverse: dict[State, list[State]] = {}
        for src, outs in self.edges.items():
            for name, kind, dst in outs:
                if kind == "sched" or name == "Tick":
                    reverse.setdefault(dst, []).append(src)
        good = {s for s in self.states if is_terminal(s)}
        queue = deque(good)
        while queue:
            cur = queue.popleft()
            for prev in reverse.get(cur, []):
                if prev not in good:
                    good.add(prev)
                    queue.append(prev)
        return good

    def liveness_failures(self) -> list[State]:
        good = self.scheduled_reachable_terminals()
        return sorted(
            (s for s in self.states if in_liveness_domain(s) and s not in good),
            key=repr,
        )

    def safety_violations(self) -> list[State]:
        return sorted((s for s in self.states if s.viol != "none"), key=repr)

    def classes(self) -> dict[str, int]:
        counts: dict[str, int] = {}
        for s in self.states:
            key = why_stayed(s, self.scenario)
            counts[key] = counts.get(key, 0) + 1
        return counts


def explore(sc: Scenario) -> Exploration:
    starts = initial_states(sc)
    states: set[State] = set(starts)
    parent: dict[State, tuple[State, str] | None] = {s: None for s in starts}
    edges: dict[State, list[tuple[str, str, State]]] = {}
    queue = deque(starts)
    while queue:
        cur = queue.popleft()
        outs = successors(cur, sc)
        edges[cur] = outs
        for name, _kind, dst in outs:
            if dst not in states:
                states.add(dst)
                parent[dst] = (cur, name)
                queue.append(dst)
    return Exploration(scenario=sc, states=states, edges=edges, parent=parent)


def _main() -> int:
    runs = [
        ("agent baseline", Scenario()),
        ("agent all-fix", Scenario(fixes=ALL_FIXES)),
        (
            "validate baseline (mirror fails)",
            Scenario(slot_type="validate", mirror_fails=1),
        ),
        (
            "validate all-fix (mirror fails)",
            Scenario(slot_type="validate", mirror_fails=1, fixes=ALL_FIXES),
        ),
        (
            "agent other-journal baseline",
            Scenario(other_journal=1),
        ),
        (
            "agent other-journal all-fix",
            Scenario(other_journal=1, fixes=ALL_FIXES),
        ),
    ]
    for label, sc in runs:
        ex = explore(sc)
        fails = ex.liveness_failures()
        fail_classes: dict[str, int] = {}
        for s in fails:
            key = why_stayed(s, sc)
            fail_classes[key] = fail_classes.get(key, 0) + 1
        print(
            f"{label}: states={len(ex.states)} "
            f"liveness_failures={len(fails)} {fail_classes} "
            f"safety_violations={len(ex.safety_violations())}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())
