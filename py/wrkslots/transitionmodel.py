"""Concurrent transition model for wrkslots registry state (#1828).

Checked abstraction, not runtime code: nothing imports this module.
Where ``lifecyclemodel`` (#1568) compresses one slot's lifecycle and
folds cross-slot effects into scenario flags, this model keeps the
dimension that module deliberately dropped: two slots, concurrent
actors with explicit program counters, the event-log revision chain,
the ACTIVE snapshot, journals, the mutation lock, and crashes.

Each actor is a transcription of one command's real phase structure
in ``py/wrkslots/cli.py`` (line numbers cite ``e16fbeb9``): a
lock-free read captures some state version; work proceeds lock-free;
the lock is acquired; the under-lock checks compare exactly the
fields the code compares; events are appended with an explicit
previous revision; the ACTIVE snapshot is rewritten from the state
the writer advanced.  No transition encodes a known bug: a forked
revision chain, a rolled-back snapshot, or a deleted-while-locked
checkout can only emerge from those generic steps interleaving.
Scenario constants select code variants (current main, the #1766
fix, the unlanded #1807 fix) by changing which fields a transcribed
check compares or the order of transcribed phases.

Properties searched over the reachable graph:

* Safety ghosts (sticky ``viol``): an append whose previous
  revision is not the log tip (the chain forks; every later load
  refuses at cli.py:10067), and a subvolume deleted while its
  checkout registration is still locked (cli.py:59162 pre-fix).
* Wedges: reachable states that are not clean and cannot reach a
  clean state at all, versus manual terminals that reach clean
  only through hand actions outside wrkslots (a hand
  ``git worktree unlock``, reverting a config edit).
* Journal liveness: a dead actor's journal that no scheduled edge
  (recover plus the passage of time) can resolve.
* Bounded contention: a prevalidated actor whose retry budget a
  stream of heartbeats can exhaust (#1276 shape), reported as a
  witness trace, not a fairness proof.

Bounds: 2 slots on one machine; actors PreparedRemove(A),
Heartbeat(B), Create(B), ValidateBatchRemove(A), PrevalidatedAdopt(B,
contention runs only), plus atomic Recover / RecoverTailEvent edges;
each actor runs once (Heartbeat repeats only in contention runs,
capped); heartbeat versions cap at 2; the revision grows by at most
the number of append steps.  Recover is atomic (a crash inside
recover re-enters the same journal state; see TRANSITIONMODEL.md
for everything not modelled).
"""

from __future__ import annotations

import dataclasses
from collections import deque
from typing import Callable, NamedTuple, TypeVar, cast

# Code-variant flags (scenario constants, never state).
REVCHECK = "assert-compares-revision"  # proposed minimal guard
REBASE = "rebase-on-advance"  # ac1499dc (#1807 fix, unlanded)
STALEREF = "stale-previous-refusal"  # ac1499dc defence in depth
PRECLASS = "pre-delete-classify"  # 67e5787c (#1766 fix, on main)

FIX_1766 = frozenset({PRECLASS})
FIX_1807 = frozenset({REBASE, STALEREF})
ALL_FIXES = FIX_1766 | FIX_1807
# github/main at e16fbeb9: #1766 fixed, #1807 not.
CURRENT_MAIN = FIX_1766

DONE = 90  # actor program counter: finished (incl. deferred/refused)
BUSY = 91  # actor finished by exhausting its retry budget

A, B = 0, 1  # slot indices


class Actor(NamedTuple):
    pc: int
    p: int  # local: captured revision / version (per actor, see code)
    q: int  # local: captured version / attempts
    r: int  # local: captured heartbeat version of the other slot
    s: int  # local: captured snapshot heartbeat of the target / flags


IDLE = Actor(0, 0, 0, 0, 0)


class State(NamedTuple):
    """One abstract registry state; field values documented inline."""

    tip: int  # event-log derived revision (cli.py:10288 fold)
    ok: int  # 1 while the revision chain is continuous (10067)
    fork_tip: int  # 1 while the discontinuity is the last event
    log_hb: tuple[int, int]  # replay heartbeat version per slot
    log_pres: tuple[int, int]  # replay: slot has an active record
    snap_rev: int  # ACTIVE.<machine>.json revision (6077)
    snap_hb: tuple[int, int]  # snapshot heartbeat versions
    snap_pres: tuple[int, int]  # snapshot active rows
    hb_exp: tuple[int, int]  # heartbeat expired (27794)
    truth: tuple[int, int]  # owner process truly alive
    proof: tuple[str, str]  # live | dead | unbound (what guards use)
    hold: tuple[int, int]  # slot-held (44561)
    handoff: tuple[str, str]  # none | unread | read
    jrnl: tuple[str, str]  # none|create0|create1|fpend|ffence|vprep|vfence
    jlive: tuple[int, int]  # journal actor process alive
    seal: str  # none | live | dead (validate-batch seal, 6128)
    seal_tgt: int  # sealed slot index, -1 when no seal
    storage: tuple[str, str]  # absent | present | fenced | deleted
    reg: tuple[str, str]  # git registration lock: none | own | foreign
    arch: tuple[int, int]  # archive record exists
    partial: int  # leftover *.tmp.* from a crashed atomic write (7506)
    cfg_ttl: int  # config heartbeat_ttl_seconds edited since journal
    version: int  # registry identity abstraction (6955): bumps per write
    hbn: int  # heartbeats performed (contention bound)
    holdn: int  # hold toggles used (bound: one hold, one unhold)
    lock: int  # mutation-lock holder actor index, -1 free (6745)
    # (PreparedRemove, Heartbeat, Create, ValidateBatch, Adopt)
    actors: tuple[Actor, Actor, Actor, Actor, Actor]
    viol: str  # none | forked-chain | deleted-while-locked


@dataclasses.dataclass(frozen=True)
class Scenario:
    """Constants for one exploration: code variants and bounds."""

    fixes: frozenset[str] = CURRENT_MAIN
    reg_a: str = "none"  # slot A registration lock: none | own | foreign
    b_present: bool = True  # slot B starts with an active record
    b_unbound: bool = False  # slot B starts ownerless (adoptable)
    misjudged_a: bool = False  # A: truly alive, proven dead
    retained_writer: bool = False  # retire-pending writer shape (61662)
    contention: bool = False  # Heartbeat repeats; Adopt actor enabled
    max_heartbeats: int = 1  # total Heartbeat(B) runs allowed
    disable: frozenset[str] = frozenset()  # actor keys: rm | cr | vb


_T = TypeVar("_T")

# Transition sink shared by every actor: (action name, kind, next).
AddFn = Callable[[str, str, "State"], None]


def _pair(t: tuple[_T, _T], i: int, v: _T) -> tuple[_T, _T]:
    return (v, t[1]) if i == 0 else (t[0], v)


def _actor(s: State, i: int, a: Actor) -> State:
    actors = list(s.actors)
    actors[i] = a
    return s._replace(
        actors=cast(
            "tuple[Actor, Actor, Actor, Actor, Actor]", tuple(actors)
        )
    )


def initial_states(sc: Scenario) -> tuple[State, ...]:
    """One start: A removable, B per scenario; log and snapshot agree."""
    b_pres = 1 if sc.b_present else 0
    base = State(
        tip=2,
        ok=1,
        fork_tip=0,
        log_hb=(1, 1),
        log_pres=(1, b_pres),
        snap_rev=2,
        snap_hb=(1, 1),
        snap_pres=(1, b_pres),
        hb_exp=(0, 0) if sc.contention else (1, 0),
        truth=(1 if (sc.misjudged_a or sc.contention) else 0, b_pres),
        proof=(
            "live" if sc.contention else "dead",
            "unbound" if sc.b_unbound else "live",
        ),
        hold=(0, 0),
        handoff=("none", "none") if sc.contention else ("read", "none"),
        jrnl=("none", "none"),
        jlive=(0, 0),
        seal="none",
        seal_tgt=-1,
        storage=("present", "present" if sc.b_present else "absent"),
        reg=(sc.reg_a, "none"),
        arch=(0, 0),
        partial=0,
        cfg_ttl=0,
        version=0,
        hbn=0,
        holdn=0,
        lock=-1,
        actors=(IDLE, IDLE, IDLE, IDLE, IDLE),
        viol="none",
    )
    return (base,)


def _append(s: State, prev: int, *, hb_slot: int = -1, pres_off: int = -1) -> State:
    """One event append with explicit previous revision (9783, 10629).

    ``prev`` is whatever the caller passes.  The fold at load time
    (10067) requires ``prev`` to equal the derived tip; an append
    that violates it lands as a hash-valid file and forks the
    chain.  A forked event's payload never becomes derived state,
    so replay fields change only on a continuous append.  The
    ACTIVE snapshot is a separate later step (10648-10653).
    """
    if prev != s.tip:
        viol = s.viol if s.viol != "none" else "forked-chain"
        return s._replace(tip=s.tip + 1, ok=0, fork_tip=1, viol=viol,
                          version=s.version + 1)
    log_hb, log_pres = s.log_hb, s.log_pres
    if hb_slot >= 0:
        log_hb = _pair(log_hb, hb_slot, min(2, log_hb[hb_slot] + 1))
    if pres_off >= 0:
        log_pres = _pair(log_pres, pres_off, 0)
    # A continuous append atop a fork keeps the chain broken and
    # buries the discontinuity below the tip (14989: tail recovery
    # then refuses, the log being broken before the tail).
    return s._replace(tip=s.tip + 1, log_hb=log_hb, log_pres=log_pres,
                      fork_tip=0 if not s.ok else s.fork_tip,
                      version=s.version + 1)


def _sync_snap(s: State) -> State:
    """ACTIVE rewritten from the just-advanced derived state (10653)."""
    return s._replace(snap_rev=s.tip, snap_hb=s.log_hb, snap_pres=s.log_pres,
                      version=s.version + 1)


def _removable(s: State, i: int) -> bool:
    """Removal admission, abstracted (60983-61185): expired or
    released heartbeat (modelled as expired), owner proven dead,
    no hold, handoff read, no journal on the slot, chain loadable."""
    return (
        s.ok == 1
        and s.log_pres[i] == 1
        and s.proof[i] == "dead"
        and s.hb_exp[i] == 1
        and s.hold[i] == 0
        and s.handoff[i] == "read"
        and s.jrnl[i] == "none"
        and s.storage[i] == "present"
    )


# ------------------------------------------------------------------
# Actor 0: PreparedRemove(A) — remove / retire-pending /
# remove-agent-batch prepared path (61512 prepare, 62438 commit).
# Locals: p=prepared tip, q=captured registry version,
# r=prepared log heartbeat of B, s=prepared snapshot heartbeat of A
# (plus bit 8: the basis commit advanced a stale state).
# ------------------------------------------------------------------

def _prepared_remove(s: State, sc: Scenario, add: AddFn) -> None:
    me = s.actors[0]
    pc = me.pc
    if pc == 0:
        # Lock-free whole-registry load (61569-61573).  Outstanding
        # journals divert to the fully locked path (61557), which
        # reloads under the lock, so prepare requires none.
        if _removable(s, A) and s.jrnl[B] == "none" and s.seal == "none":
            add("RmLoadFree", "cmd", _actor(s, 0, Actor(
                1, s.tip, 0, s.log_hb[B], s.snap_hb[A])))
    elif pc == 1:
        # Lock-free validation and salvage (61693, 61740): the
        # unbounded window.  No persistent change by this actor.
        add("RmSalvage", "cmd", _actor(s, 0, me._replace(pc=2)))
    elif pc == 2:
        # Registry identity captured in the return statement,
        # after salvage, while state/record stay the load copies
        # (61767).  This ordering is the transcription; nothing
        # here mentions revisions of other slots.
        add("RmCaptureIdentity", "cmd",
            _actor(s, 0, me._replace(pc=3, q=s.version)))
    elif pc == 3:
        if s.lock == -1:
            add("RmLock", "cmd",
                _actor(s._replace(lock=0), 0, me._replace(pc=4)))
    elif pc == 4:
        # _assert_prepared_removal_current (61471): registry
        # identity, journal fingerprint (both folded into version),
        # this slot's record read from the ACTIVE snapshot (61493,
        # _load_active_snapshot 9050), lock-scope fingerprint
        # (folded into the same record fields).  Variants add a
        # revision comparison or rebase onto the current state.
        stale_identity = s.version != me.q
        record_changed = (
            s.snap_hb[A] != me.s or s.snap_pres[A] != 1
        )
        if stale_identity or record_changed:
            add("RmDeferStale", "cmd", _actor(
                s._replace(lock=-1), 0, me._replace(pc=DONE)))
        elif REBASE in sc.fixes:
            # Commit advances the current log-derived state;
            # this slot's record was just shown unchanged.
            add("RmAssertRebase", "cmd", _actor(
                s, 0, me._replace(pc=5, p=s.tip, r=s.log_hb[B])))
        elif REVCHECK in sc.fixes and s.tip != me.p:
            add("RmDeferRevision", "cmd", _actor(
                s._replace(lock=-1), 0, me._replace(pc=DONE)))
        else:
            add("RmAssertCurrent", "cmd", _actor(s, 0, me._replace(pc=5)))
    elif pc == 5:
        # Basis write _write_active_state(previous=prepared.state)
        # (62517 via 10600-10606): a passed previous is trusted;
        # replayability is judged against it, not the log (10641).
        # The writer is fresh at the chain tip (62497) unless the
        # retire-pending retained-writer shape applies (61662),
        # whose stale sequence collides with the existing file
        # (9796) instead of forking.
        if s.partial:
            # Atomic write refuses on a leftover tmp (7506); the
            # command exits, releasing the lock, writing nothing.
            add("RmRefusePartial", "cmd", _actor(
                s._replace(lock=-1), 0, me._replace(pc=DONE)))
            return
        if STALEREF in sc.fixes and me.p != s.tip:
            add("RmRefuseStalePrevious", "cmd", _actor(
                s._replace(lock=-1), 0, me._replace(pc=DONE)))
        elif sc.retained_writer and me.p != s.tip:
            add("RmRefuseStaleSequence", "cmd", _actor(
                s._replace(lock=-1), 0, me._replace(pc=DONE)))
        else:
            nxt = _append(s, me.p)
            stale = 8 if me.p != s.tip else 0
            add("RmAppendBasis", "cmd",
                _actor(nxt, 0, me._replace(pc=6, s=me.s | stale)))
            add("RmCrashMidAppend", "env", _actor(
                s._replace(lock=-1, partial=1), 0, me._replace(pc=DONE)))
    elif pc == 6:
        # ACTIVE rewritten from the state this writer advanced
        # (10653): the prepared (possibly stale) whole registry,
        # so a stale commit rolls the other slot back in the file
        # readers of the snapshot see.
        if me.s & 8:
            nxt = s._replace(
                snap_rev=me.p + 1,
                snap_hb=(s.snap_hb[A], me.r),
                version=s.version + 1,
            )
        else:
            nxt = _sync_snap(s)
        add("RmWriteActive", "cmd", _actor(nxt, 0, me._replace(pc=7)))
    elif pc == 7:
        # _begin_finish: checkout-evidence append then the finish
        # journal (60241-60288).  Journal writes are event-first
        # (10768): fpend is the log-pending journal with no file.
        if s.hold[A] == 1:
            # _begin_finish asserts not-held under the lock before
            # publishing anything (60228): a hold that landed
            # during salvage refuses here, journal never created.
            add("RmRefuseHeld", "cmd", _actor(
                s._replace(lock=-1), 0, me._replace(pc=DONE)))
            return
        if s.partial:
            add("RmRefusePartialJournal", "cmd", _actor(
                s._replace(lock=-1), 0, me._replace(pc=DONE)))
            return
        nxt = _append(s, s.tip)  # evidence event at the writer tip
        nxt = nxt._replace(jrnl=_pair(nxt.jrnl, A, "fpend"),
                           jlive=_pair(nxt.jlive, A, 1))
        add("RmJournalEvent", "cmd", _actor(nxt, 0, me._replace(pc=8)))
    elif pc == 8:
        nxt = s._replace(jrnl=_pair(s.jrnl, A, "ffence"),
                         storage=_pair(s.storage, A, "fenced"),
                         lock=-1, version=s.version + 1)
        add("RmJournalFileFence", "cmd", _actor(nxt, 0, me._replace(pc=9)))
    elif pc == 9:
        # Lock-free deletion of the fenced tree (63136 tail).
        nxt = s._replace(storage=_pair(s.storage, A, "deleted"))
        add("RmTailDelete", "cmd", _actor(nxt, 0, me._replace(pc=10)))
    elif pc == 10:
        if s.lock != -1:
            return
        # Completion deliberately reloads fresh state (60519,
        # 60191): on a forked log that reload itself refuses,
        # leaving journal and deleted storage behind.
        if not s.ok:
            add("RmCompleteRefusedFork", "cmd",
                _actor(s, 0, me._replace(pc=DONE)))
            return
        nxt = _append(s._replace(lock=0), s.tip, pres_off=A)
        nxt = _sync_snap(nxt._replace(
            arch=_pair(s.arch, A, 1), jrnl=_pair(s.jrnl, A, "none"),
            jlive=_pair(s.jlive, A, 0), lock=-1))
        add("RmComplete", "cmd", _actor(nxt, 0, me._replace(pc=DONE)))


# ------------------------------------------------------------------
# Actor 1: Heartbeat — fully locked single-record update
# (_cmd_heartbeat 44295): no lock-free state is trusted; the
# event-then-snapshot order (44363, 10648) leaves a crash window.
# Target is slot B normally; in contention runs slot A is the
# bound, heartbeating neighbour whose appends churn the registry
# identity the adopter of unbound B must match.
# ------------------------------------------------------------------

def _heartbeat(s: State, sc: Scenario, add: AddFn) -> None:
    tgt = A if sc.contention else B
    me = s.actors[1]
    pc = me.pc
    if pc == DONE:
        return
    if pc == 0:
        if s.hbn >= sc.max_heartbeats:
            return
        if (
            s.ok == 1 and s.log_pres[tgt] == 1 and s.truth[tgt] == 1
            and s.proof[tgt] == "live" and s.lock == -1
            and not (s.seal != "none" and s.seal_tgt == tgt)
        ):
            add("HbLock", "cmd",
                _actor(s._replace(lock=1), 1, me._replace(pc=1)))
    elif pc == 1:
        if s.partial:
            add("HbRefusePartial", "cmd", _actor(
                s._replace(lock=-1), 1, me._replace(pc=DONE)))
            return
        nxt = _append(s, s.tip, hb_slot=tgt)
        nxt = nxt._replace(hb_exp=_pair(nxt.hb_exp, tgt, 0), hbn=s.hbn + 1)
        add("HbAppend", "cmd", _actor(nxt, 1, me._replace(pc=2)))
        add("HbCrashMidAppend", "env", _actor(
            s._replace(lock=-1, partial=1), 1, me._replace(pc=DONE)))
    elif pc == 2:
        nxt = _sync_snap(s._replace(lock=-1))
        nxt_pc = 0 if sc.contention else DONE
        add("HbWriteActive", "cmd", _actor(nxt, 1, me._replace(pc=nxt_pc)))


# ------------------------------------------------------------------
# Actor 2: Create(B) — prevalidated create (_cmd_create 41942).
# Phases compressed to the journal-visible points: publish journal
# before worktrees (42319), worktrees, publish row (42516), clear
# journal (42521).  create0: no durable row yet; create1: row
# durable, journal not yet cleared.
# ------------------------------------------------------------------

def _create(s: State, sc: Scenario, add: AddFn) -> None:
    me = s.actors[2]
    pc = me.pc
    if pc == 0:
        if (
            s.ok == 1 and s.log_pres[B] == 0 and s.storage[B] == "absent"
            and s.jrnl[B] == "none" and s.lock == -1 and s.seal == "none"
        ):
            nxt = s._replace(lock=2, jrnl=_pair(s.jrnl, B, "create0"),
                             jlive=_pair(s.jlive, B, 1),
                             version=s.version + 1)
            add("CrJournal", "cmd", _actor(nxt, 2, me._replace(pc=1)))
    elif pc == 1:
        nxt = s._replace(storage=_pair(s.storage, B, "present"))
        add("CrWorktrees", "cmd", _actor(nxt, 2, me._replace(pc=2)))
    elif pc == 2:
        if s.partial:
            add("CrRefusePartial", "cmd", _actor(
                s._replace(lock=-1, jlive=_pair(s.jlive, B, 0)),
                2, me._replace(pc=DONE)))
            return
        nxt = _append(s, s.tip)
        nxt = nxt._replace(log_pres=_pair(nxt.log_pres, B, 1),
                           proof=_pair(nxt.proof, B, "live"),
                           truth=_pair(nxt.truth, B, 1),
                           hb_exp=_pair(nxt.hb_exp, B, 0),
                           jrnl=_pair(nxt.jrnl, B, "create1"))
        nxt = _sync_snap(nxt)
        add("CrPublish", "cmd", _actor(nxt, 2, me._replace(pc=3)))
    elif pc == 3:
        nxt = s._replace(jrnl=_pair(s.jrnl, B, "none"),
                         jlive=_pair(s.jlive, B, 0), lock=-1,
                         version=s.version + 1)
        add("CrClearJournal", "cmd", _actor(nxt, 2, me._replace(pc=DONE)))


# ------------------------------------------------------------------
# Actor 3: ValidateBatchRemove(A) — remove-validate-batch subvolume
# path.  Phase order per code variant: with PRECLASS, registrations
# are enumerated and classified before the delete (59118-59228);
# pre-fix (eb14b70d:58454) the delete runs first and a locked
# registration is discovered by the still-registered check after
# the prune of the recorded repository only.  The batch holds the
# lock throughout in this abstraction (its seal window is what
# other commands observe; per-scope unlocks add no interleaving
# the seal does not already gate).
# ------------------------------------------------------------------

def _validate_batch(s: State, sc: Scenario, add: AddFn) -> None:
    me = s.actors[3]
    pc = me.pc
    fixed = PRECLASS in sc.fixes
    if pc == 0:
        if (
            s.ok == 1 and s.log_pres[A] == 1 and s.jrnl[A] == "none"
            and s.seal == "none" and s.lock == -1
            and s.storage[A] == "present" and s.hold[A] == 0
        ):
            nxt = s._replace(lock=3, seal="live", seal_tgt=A,
                             version=s.version + 1)
            add("VbSeal", "cmd", _actor(nxt, 3, me._replace(pc=1)))
    elif pc == 1:
        nxt = s._replace(jrnl=_pair(s.jrnl, A, "vprep"),
                         jlive=_pair(s.jlive, A, 1), version=s.version + 1)
        add("VbFinishPrepared", "cmd", _actor(nxt, 3, me._replace(pc=2)))
    elif pc == 2:
        nxt = s._replace(jrnl=_pair(s.jrnl, A, "vfence"),
                         storage=_pair(s.storage, A, "fenced"),
                         version=s.version + 1)
        add("VbFence", "cmd", _actor(nxt, 3, me._replace(pc=3)))
    elif pc == 3 and fixed:
        # Classify every registration first (58989-59025): a
        # foreign lock refuses with nothing deleted; wrkslots'
        # own lock (reason "in worktrees image") is dropped.
        if s.reg[A] == "foreign":
            nxt = s._replace(lock=-1, seal="dead",
                             jlive=_pair(s.jlive, A, 0))
            add("VbRefuseForeignPreDelete", "cmd",
                _actor(nxt, 3, me._replace(pc=DONE)))
        elif s.reg[A] == "own":
            nxt = s._replace(reg=_pair(s.reg, A, "none"))
            add("VbDropOwnLock", "cmd", _actor(nxt, 3, me._replace(pc=4)))
        else:
            add("VbClassifyClean", "cmd", _actor(s, 3, me._replace(pc=4)))
    elif pc == 3 and not fixed:
        add("VbSkipClassify", "cmd", _actor(s, 3, me._replace(pc=4)))
    elif pc == 4:
        # removing intent journaled, then the subvolume delete
        # (59155-59179).  Deleting under a still-locked
        # registration is the point of no rollback.
        viol = s.viol
        if viol == "none" and s.reg[A] != "none":
            viol = "deleted-while-locked"
        nxt = s._replace(storage=_pair(s.storage, A, "deleted"), viol=viol,
                         version=s.version + 1)
        add("VbDeleteSubvolume", "cmd", _actor(nxt, 3, me._replace(pc=5)))
    elif pc == 5 and fixed:
        # Prune every registering repository (59181); own locks
        # already dropped, so nothing can still register.
        add("VbPruneAll", "cmd", _actor(s, 3, me._replace(pc=6)))
    elif pc == 5 and not fixed:
        # Pre-fix prune touches the recorded repository only and
        # git prune skips locked entries: a locked registration
        # (in any repository) survives to the still-registered
        # refusal, after the delete (eb14b70d:58561).
        if s.reg[A] != "none":
            nxt = s._replace(lock=-1, seal="dead",
                             jlive=_pair(s.jlive, A, 0))
            add("VbRefuseStillRegistered", "cmd",
                _actor(nxt, 3, me._replace(pc=DONE)))
        else:
            add("VbPruneRecorded", "cmd", _actor(s, 3, me._replace(pc=6)))
    elif pc == 6:
        if s.partial:
            add("VbRefusePartial", "cmd", _actor(
                s._replace(lock=-1, seal="dead",
                           jlive=_pair(s.jlive, A, 0)),
                3, me._replace(pc=DONE)))
            return
        nxt = _append(s, s.tip, pres_off=A)
        nxt = _sync_snap(nxt._replace(
            arch=_pair(s.arch, A, 1), jrnl=_pair(s.jrnl, A, "none"),
            jlive=_pair(s.jlive, A, 0), seal="none", seal_tgt=-1,
            lock=-1))
        add("VbArchive", "cmd", _actor(nxt, 3, me._replace(pc=DONE)))


# ------------------------------------------------------------------
# Actor 4: PrevalidatedAdopt(B) — contention runs only.  The
# _run_prevalidated_mutation shape (7082): capture registry
# identity, prepare lock-free, commit only if the identity still
# matches under the locks; REGISTRY_VALIDATION_ATTEMPTS=5 rounds
# (6930), then a busy refusal with nothing written (7182).
# Locals: p=captured version, q=failed rounds.
# ------------------------------------------------------------------

def _adopt(s: State, sc: Scenario, add: AddFn) -> None:
    if not sc.contention:
        return
    me = s.actors[4]
    pc = me.pc
    if pc == 0:
        if (
            s.ok == 1 and s.log_pres[B] == 1 and s.proof[B] == "unbound"
            and s.jrnl[A] == "none" and s.jrnl[B] == "none"
        ):
            add("AdPrepareFree", "cmd",
                _actor(s, 4, Actor(1, s.version, me.q, 0, 0)))
    elif pc == 1:
        if s.lock == -1:
            add("AdLock", "cmd",
                _actor(s._replace(lock=4), 4, me._replace(pc=2)))
    elif pc == 2:
        if not s.ok or s.partial:
            add("AdRefuseUnloadable", "cmd", _actor(
                s._replace(lock=-1), 4, me._replace(pc=DONE)))
            return
        if s.version == me.p:
            nxt = _append(s, s.tip, hb_slot=B)
            nxt = _sync_snap(nxt._replace(
                proof=_pair(s.proof, B, "live"),
                hb_exp=_pair(s.hb_exp, B, 0), lock=-1))
            add("AdCommit", "cmd", _actor(nxt, 4, me._replace(pc=DONE)))
        elif me.q + 1 >= 5:
            add("AdBusyExhausted", "cmd", _actor(
                s._replace(lock=-1), 4, me._replace(pc=BUSY, q=me.q + 1)))
        else:
            add("AdRetryRound", "cmd", _actor(
                s._replace(lock=-1), 4, me._replace(pc=0, q=me.q + 1)))


# ------------------------------------------------------------------
# Recover and RecoverTailEvent as atomic locked edges.  Recover
# dispatches on journal kind (90692) and re-gates finish recovery
# on current heartbeat/hold/owner facts (69229, 69350); a refusal
# leaves the state unchanged, hence no edge.  RecoverTailEvent
# (15106) refuses while any journal is outstanding (15131), on a
# partial tmp (15130), or when the log is broken before the tail
# (14989).  Kind "sched": runnable by any later participant.
# ------------------------------------------------------------------

# vprep/vfence are the validate-batch finish journal (paired
# with the seal); fpend/ffence the agent prepared-remove one.
# Registration-lock gates in recovery are the validate path's
# (#1766); the agent path archives past a second-repository
# registration it never enumerates.
_FINISH_KINDS = ("fpend", "ffence", "vprep", "vfence", "fdone")


def _recover(s: State, sc: Scenario, add: AddFn) -> None:
    if s.lock != -1:
        return
    # _ensure_state_shard (9322-9330) rewrites a lagging ACTIVE
    # snapshot from the log-derived state; on a forked log the
    # derivation itself refuses, so this heals only crash lag.
    if s.ok == 1 and (s.snap_rev, s.snap_hb, s.snap_pres) != (
        s.tip, s.log_hb, s.log_pres
    ):
        add("EnsureShardSync", "sched", _sync_snap(s))
    if s.partial:
        add("RecDiscardPartial", "sched", s._replace(partial=0,
                                                     version=s.version + 1))
        return
    if not s.ok:
        # Every recover dispatch replays the log first (92413).
        if (
            s.fork_tip == 1 and s.jrnl == ("none", "none")
            and s.seal == "none"
        ):
            add("RecTailTruncate", "cmd", s._replace(
                tip=s.tip - 1, ok=1, fork_tip=0, snap_rev=s.tip - 1,
                snap_hb=s.log_hb, snap_pres=s.log_pres,
                version=s.version + 1))
        return
    fixed = PRECLASS in sc.fixes
    for i in (A, B):
        kind = s.jrnl[i]
        if kind == "none" or s.jlive[i] == 1:
            continue
        if kind in _FINISH_KINDS:
            if s.hold[i] == 1 or s.hb_exp[i] == 0 or s.proof[i] != "dead":
                continue  # re-gated refusal (69229, 69350, 69356)
            if s.reg[i] == "foreign" and kind.startswith("v"):
                continue  # never dropped by wrkslots (22852)
            if s.reg[i] == "own" and not fixed and kind.startswith("v"):
                if s.storage[i] == "deleted":
                    # Pre-fix missing-checkout branch refuses
                    # unconditionally (eb14b70d:58775).
                    continue
                viol = s.viol if s.viol != "none" else "deleted-while-locked"
                add("RecFinishDeleteLocked", "sched", s._replace(
                    storage=_pair(s.storage, i, "deleted"), viol=viol,
                    version=s.version + 1))
                continue
            nxt = _append(s, s.tip, pres_off=i)
            seal, tgt = s.seal, s.seal_tgt
            if tgt == i:
                seal, tgt = "none", -1
            nxt = _sync_snap(nxt._replace(
                arch=_pair(s.arch, i, 1), jrnl=_pair(s.jrnl, i, "none"),
                storage=_pair(s.storage, i, "deleted"),
                reg=_pair(s.reg, i, "none") if kind.startswith("v")
                else s.reg,
                seal=seal, seal_tgt=tgt))
            add("RecFinishArchive", "sched", nxt)
        elif kind in ("create0", "create1"):
            if s.cfg_ttl:
                continue  # TTL equality check refuses (68800)
            if kind == "create1":
                add("RecCreateClear", "sched", s._replace(
                    jrnl=_pair(s.jrnl, i, "none"), version=s.version + 1))
            else:
                nxt = _append(s, s.tip)
                nxt = nxt._replace(
                    log_pres=_pair(nxt.log_pres, i, 1),
                    proof=_pair(nxt.proof, i, "live"),
                    truth=_pair(nxt.truth, i, 1),
                    hb_exp=_pair(nxt.hb_exp, i, 0),
                    storage=_pair(nxt.storage, i, "present"),
                    jrnl=_pair(nxt.jrnl, i, "none"))
                add("RecCreateComplete", "sched", _sync_snap(nxt))
                add("RecCreateAbort", "sched", s._replace(
                    jrnl=_pair(s.jrnl, i, "none"),
                    storage=_pair(s.storage, i, "absent"),
                    version=s.version + 1))
    if s.seal == "dead" and s.seal_tgt >= 0 and s.jrnl[s.seal_tgt] == "none":
        add("RecSealRetire", "sched",
            s._replace(seal="none", seal_tgt=-1, version=s.version + 1))


# ------------------------------------------------------------------
# Environment: crashes at every phase boundary, clocks, owner
# facts, and hand actions outside wrkslots (kind "hand", excluded
# from legal wrkslots exits).  flock state itself never persists:
# a dead holder's lock is free (7022 excludes lock files).
# ------------------------------------------------------------------

_CRASH_NAMES = ("CrashRm", "CrashHb", "CrashCr", "CrashVb", "CrashAd")


def _environment(s: State, sc: Scenario, add: AddFn) -> None:
    for idx in range(5):
        me = s.actors[idx]
        if me.pc in (0, DONE, BUSY):
            continue
        nxt = _actor(s, idx, me._replace(pc=DONE))
        if nxt.lock == idx:
            nxt = nxt._replace(lock=-1)
        if idx in (0, 3) and nxt.jrnl[A] != "none":
            nxt = nxt._replace(jlive=_pair(nxt.jlive, A, 0))
        if idx == 2 and nxt.jrnl[B] != "none":
            nxt = nxt._replace(jlive=_pair(nxt.jlive, B, 0))
        if idx == 3 and nxt.seal == "live":
            nxt = nxt._replace(seal="dead")
        add(_CRASH_NAMES[idx], "env", nxt)
    # Passage of time expires every live record's heartbeat.
    if any(s.log_pres[i] == 1 and s.hb_exp[i] == 0 for i in (A, B)):
        exp = (
            1 if s.log_pres[A] == 1 else s.hb_exp[A],
            1 if s.log_pres[B] == 1 else s.hb_exp[B],
        )
        add("Tick", "env", s._replace(hb_exp=exp))
    for i, death, probe in ((A, "OwnerDeathA", "ProbeDeadA"),
                            (B, "OwnerDeathB", "ProbeDeadB")):
        if s.truth[i] == 1 and s.log_pres[i] == 1:
            add(death, "env", s._replace(truth=_pair(s.truth, i, 0)))
        if s.truth[i] == 0 and s.proof[i] == "live" and s.log_pres[i] == 1:
            add(probe, "sched", s._replace(proof=_pair(s.proof, i, "dead")))
    # A misjudged (truly live) owner of A renews its heartbeat:
    # an atomic locked append, refused on seal overlap (44306).
    # Bounded to the expiry-crossing renewal (the case recover's
    # re-gate at 69350 observes); repeat renewals add no state.
    if (
        s.ok == 1 and not s.partial and s.lock == -1
        and s.log_pres[A] == 1 and s.truth[A] == 1
        and s.hb_exp[A] == 1 and s.log_hb[A] < 2
        and not (s.seal != "none" and s.seal_tgt == A)
    ):
        nxt = _append(s, s.tip, hb_slot=A)
        nxt = _sync_snap(nxt._replace(hb_exp=_pair(nxt.hb_exp, A, 0)))
        add("RenewHeartbeatA", "owner", nxt)
    # hold/unhold are ordinary wrkslots commands (44538, 44595):
    # legal exits, gated on no journal for the target (44546).
    if s.ok == 1 and s.lock == -1 and s.log_pres[A] == 1 and s.holdn < 2:
        if s.hold[A] == 0 and s.jrnl[A] == "none":
            add("HoldA", "human", s._replace(
                hold=_pair(s.hold, A, 1), holdn=s.holdn + 1,
                version=s.version + 1))
        if s.hold[A] == 1:
            add("UnholdA", "human", s._replace(
                hold=_pair(s.hold, A, 0), holdn=s.holdn + 1,
                version=s.version + 1))
    # A config TTL edit is observable only through create
    # recovery's equality check (68800), so the edit is offered
    # once a create journal exists; earlier/later edits are the
    # same state by the time anything reads the flag.
    if s.cfg_ttl == 0 and any(j.startswith("create") for j in s.jrnl):
        add("ConfigTtlEdit", "env", s._replace(cfg_ttl=1))
    else:
        add("ConfigTtlRevert", "hand", s._replace(cfg_ttl=0))
    if s.reg[A] != "none":
        # Hand `git worktree unlock` in the locking repository
        # (plus a hand prune pre-fix): available to an operator
        # for any lock, but outside wrkslots in both variants;
        # post-fix the prune and archive then happen inside
        # recover (59011).  Wrkslots itself drops only its own.
        add("HandGitUnlock", "hand",
            s._replace(reg=_pair(s.reg, A, "none")))


def successors(s: State, sc: Scenario) -> list[tuple[str, str, State]]:
    """Every enabled transition as (action name, edge kind, next)."""
    out: list[tuple[str, str, State]] = []

    def add(name: str, kind: str, nxt: State) -> None:
        if nxt != s:
            out.append((name, kind, nxt))

    if "rm" not in sc.disable:
        _prepared_remove(s, sc, add)
    _heartbeat(s, sc, add)
    if "cr" not in sc.disable:
        _create(s, sc, add)
    if "vb" not in sc.disable:
        _validate_batch(s, sc, add)
    _adopt(s, sc, add)
    _recover(s, sc, add)
    _environment(s, sc, add)
    return out


def is_clean(s: State) -> bool:
    """A healthy resting state: loadable chain, snapshot in sync,
    no journal or seal outstanding, no lock or actor mid-flight,
    each slot either live with its storage, archived with its
    storage gone, or never created.  The sticky ghost is history,
    not health: a quarantined fork that tail recovery repaired
    rests clean here while still testifying via viol."""
    if (
        s.ok != 1 or s.partial or s.seal != "none" or s.lock != -1
        or s.jrnl != ("none", "none")
        or s.snap_rev != s.tip or s.snap_hb != s.log_hb
        or s.snap_pres != s.log_pres
        or any(a.pc not in (0, DONE, BUSY) for a in s.actors)
    ):
        return False
    for i in (A, B):
        if s.arch[i] == 1:
            if s.log_pres[i] != 0 or s.storage[i] not in ("deleted", "absent"):
                return False
        elif s.log_pres[i] == 1:
            if s.storage[i] != "present":
                return False
        elif s.storage[i] != "absent":
            return False
    return True


@dataclasses.dataclass
class Exploration:
    """Reachable graph for one scenario, with check helpers."""

    scenario: Scenario
    states: set[State]
    edges: dict[State, list[tuple[str, str, State]]]
    parent: dict[State, tuple[State, str] | None]

    def trace(self, target: State) -> list[str]:
        """Shortest action-name path from a start state to target."""
        steps: list[str] = []
        cur: State | None = target
        while cur is not None:
            prev = self.parent.get(cur)
            if prev is None:
                break
            steps.append(prev[1])
            cur = prev[0]
        return list(reversed(steps))

    def violations(self, tag: str | None = None) -> list[State]:
        """Reachable states whose safety ghost recorded a violation."""
        return sorted(
            (s for s in self.states
             if s.viol != "none" and (tag is None or s.viol == tag)),
            key=repr,
        )

    def _reaching_clean(self, kinds: frozenset[str] | None) -> set[State]:
        """States that reach a clean state; kinds=None means all."""
        reverse: dict[State, list[State]] = {}
        for src, outs in self.edges.items():
            for _name, kind, dst in outs:
                if kinds is None or kind in kinds:
                    reverse.setdefault(dst, []).append(src)
        good = {s for s in self.states if is_clean(s)}
        queue = deque(good)
        while queue:
            cur = queue.popleft()
            for prev in reverse.get(cur, []):
                if prev not in good:
                    good.add(prev)
                    queue.append(prev)
        return good

    def wedges(self) -> list[State]:
        """Not clean and no path to clean even using hand actions."""
        good = self._reaching_clean(None)
        return sorted(
            (s for s in self.states if not is_clean(s) and s not in good),
            key=repr,
        )

    def manual_terminals(self) -> list[State]:
        """Not clean; clean is reachable, but only via a hand edge
        (an action outside wrkslots: hand git unlock, config revert)."""
        all_good = self._reaching_clean(None)
        legal = self._reaching_clean(
            frozenset({"cmd", "sched", "env", "owner", "human"}))
        return sorted(
            (s for s in self.states
             if not is_clean(s) and s in all_good and s not in legal),
            key=repr,
        )

    def dead_journals(self) -> list[State]:
        """States carrying a dead actor's journal from which no
        legal (non-hand) path clears every journal."""
        reverse: dict[State, list[State]] = {}
        for src, outs in self.edges.items():
            for _name, kind, dst in outs:
                if kind != "hand":
                    reverse.setdefault(dst, []).append(src)
        clear = {s for s in self.states if s.jrnl == ("none", "none")}
        queue = deque(clear)
        while queue:
            cur = queue.popleft()
            for prev in reverse.get(cur, []):
                if prev not in clear:
                    clear.add(prev)
                    queue.append(prev)
        return sorted(
            (s for s in self.states
             if any(s.jrnl[i] != "none" and s.jlive[i] == 0 for i in (A, B))
             and s not in clear),
            key=repr,
        )

    def shortest(self, states: list[State]) -> State | None:
        """The listed state with the shortest counterexample trace."""
        if not states:
            return None
        return min(states, key=lambda s: len(self.trace(s)))


def _canon(s: State) -> State:
    """Quotient states that differ only in dead bookkeeping.

    The registry version is an equality token (6955): only
    changes between an actor's capture and its under-lock
    comparison are observable, so when no actor holds a capture
    (PreparedRemove between capture and assert, Adopt between
    prepare and commit) the epoch restarts at zero.  Finished
    actors' locals and spent capture locals are likewise dead.
    """
    actors = list(s.actors)
    for idx, a in enumerate(actors):
        if a.pc in (DONE, BUSY):
            actors[idx] = Actor(a.pc, 0, 0, 0, 0)
    a0 = actors[0]
    if a0.pc >= 7:  # basis committed; capture locals are spent
        actors[0] = Actor(a0.pc, 0, 0, 0, 0)
    holds_capture = actors[0].pc in (3, 4) or actors[4].pc in (1, 2)
    return s._replace(
        actors=cast(
            "tuple[Actor, Actor, Actor, Actor, Actor]", tuple(actors)
        ),
        version=s.version if holds_capture else 0,
    )


def explore(sc: Scenario, *, max_states: int = 900_000) -> Exploration:
    """Explore every state reachable under one scenario (BFS)."""
    starts = tuple(_canon(s) for s in initial_states(sc))
    states: set[State] = set(starts)
    parent: dict[State, tuple[State, str] | None] = {s: None for s in starts}
    edges: dict[State, list[tuple[str, str, State]]] = {}
    queue = deque(starts)
    while queue:
        cur = queue.popleft()
        outs = [(n, k, _canon(d)) for n, k, d in successors(cur, sc)]
        edges[cur] = outs
        for name, _kind, dst in outs:
            if dst not in states:
                if len(states) >= max_states:
                    raise RuntimeError(
                        f"state bound {max_states} exceeded; "
                        "tighten the scenario, do not raise silently"
                    )
                states.add(dst)
                parent[dst] = (cur, name)
                queue.append(dst)
    return Exploration(scenario=sc, states=states, edges=edges, parent=parent)


def is_resting(s: State) -> bool:
    """No lock held and no actor mid-program: the states an
    operator actually meets between commands."""
    return s.lock == -1 and all(a.pc in (0, DONE, BUSY) for a in s.actors)


def _describe(ex: Exploration, label: str, found: list[State]) -> None:
    resting = [s for s in found if is_resting(s)]
    best = ex.shortest(resting or found)
    if best is None:
        print(f"  {label}: none")
        return
    print(f"  {label}: {len(found)} state(s) ({len(resting)} resting); "
          f"shortest resting trace ({len(ex.trace(best))} steps):")
    print("    " + " -> ".join(ex.trace(best)))
    print(f"    end: viol={best.viol} ok={best.ok} tip={best.tip} "
          f"snap_rev={best.snap_rev} jrnl={best.jrnl} seal={best.seal} "
          f"storage={best.storage} reg={best.reg} arch={best.arch}")


def _main() -> int:
    runs = [
        ("current-main reg=none", Scenario()),
        ("current-main reg=own", Scenario(reg_a="own")),
        ("current-main reg=foreign", Scenario(reg_a="foreign")),
        ("pre-1766 reg=own", Scenario(fixes=frozenset(), reg_a="own")),
        ("pre-1766 reg=foreign", Scenario(fixes=frozenset(), reg_a="foreign")),
        ("all-fix reg=own", Scenario(fixes=ALL_FIXES, reg_a="own")),
        ("all-fix reg=foreign", Scenario(fixes=ALL_FIXES, reg_a="foreign")),
        ("stale-refusal-only", Scenario(fixes=CURRENT_MAIN | {STALEREF})),
        ("retained-writer", Scenario(retained_writer=True)),
        ("revcheck-only", Scenario(fixes=CURRENT_MAIN | {REVCHECK})),
        ("create-recovery", Scenario(b_present=False)),
        ("misjudged-owner", Scenario(misjudged_a=True)),
        (
            "contention (heartbeat/adopt isolated)",
            Scenario(contention=True, b_unbound=True,
                     max_heartbeats=4,
                     disable=frozenset({"rm", "cr", "vb"})),
        ),
    ]
    for label, sc in runs:
        ex = explore(sc)
        print(f"{label}: states={len(ex.states)}")
        _describe(ex, "forked-chain", ex.violations("forked-chain"))
        _describe(ex, "deleted-while-locked",
                  ex.violations("deleted-while-locked"))
        _describe(ex, "wedges", ex.wedges())
        _describe(ex, "manual-terminals", ex.manual_terminals())
        _describe(ex, "dead-journals", ex.dead_journals())
        busy = [s for s in ex.states if s.actors[4].pc == BUSY]
        if sc.contention:
            _describe(ex, "adopt-busy", busy)
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())
