------------------------ MODULE WrkslotsTransition ------------------------
(***************************************************************************)
(* Concurrent registry transition specification (#2043).                   *)
(*                                                                         *)
(* Temporal mirror of the gated Python explicit-state explorer in          *)
(* py/wrkslots/transitionmodel.py (#1828), at the same abstraction: two    *)
(* slots, actors with program counters, the event-log revision chain, the  *)
(* ACTIVE snapshot as a separate lagging copy, per-slot journals, the      *)
(* validation-batch seal, the mutation lock, and a crash at every phase    *)
(* boundary.  Action names match the Python model so TLC counterexamples   *)
(* read as Python traces.  The Python explorer stays the checker that      *)
(* runs in validation; this spec is checked only by an opt-in TLC run      *)
(* (see run_tlc.sh) and validation never downloads a toolchain.            *)
(*                                                                         *)
(* Two dimensions the Python model folds away are explicit here, because   *)
(* the lane exists for them:                                               *)
(*                                                                         *)
(* * Journal CONTENT where it matters (#1757, #2041).  A finish journal    *)
(*   whose containment inventory exceeds the inline limit is written in    *)
(*   sidecar form (first entries inline plus a digest-named side file);    *)
(*   loading expands it back.  jform records the form the writer holds     *)
(*   in memory; the reloaded form is derived.  With NormalizeCompare off   *)
(*   (current code) the out-of-lock completion compares the two raw        *)
(*   forms and refuses whenever they differ, which is exactly when the     *)
(*   inventory was sidecarred, leaving RECOVERY REQUIRED.                  *)
(*                                                                         *)
(* * Out-of-lock windows.  The deferred/private finish deletes the fenced  *)
(*   tree with no lock held and revalidates under the lock afterwards      *)
(*   (the #894/#1735 shape the Python model compresses into its locked     *)
(*   tail delete), so the journal identity check above sits in its own     *)
(*   window beside the prepared-remove salvage window.                     *)
(*                                                                         *)
(* Constants Fixes select code variants exactly as Scenario.fixes does;    *)
(* RefsLarge selects the >64-ref inventory class; NormalizeCompare is the  *)
(* #2041 fix shape (compare after expanding both sides).                   *)
(***************************************************************************)
EXTENDS Integers, TLC

CONSTANTS RegA,            \* "none" | "own" | "foreign" (slot A git lock)
          BPresent,        \* slot B starts with an active record
          BUnbound,        \* slot B starts ownerless (adoptable)
          MisjudgedA,      \* A truly alive, proven dead
          RetainedWriter,  \* retire-pending retained-writer shape
          Contention,      \* heartbeat repeats; adopt actor enabled
          MaxHeartbeats,   \* total Heartbeat runs allowed
          Fixes,           \* subset of FixNames
          RefsLarge,       \* finish inventories exceed the inline limit
          NormalizeCompare,\* compare journals after expansion (#2041 fix)
          WindowWriteHypothesis, \* suspected bug: a disjoint create/adopt
                            \* commits during the out-of-lock finish window
          Disable          \* subset of {"rm", "cr", "vb"}

FixNames == {"pre-delete-classify", "rebase-on-advance",
             "stale-previous-refusal", "assert-compares-revision"}
Slots == {"A", "B"}
A == "A"
B == "B"
DONE == 90
BUSY == 91
FinishKinds == {"fpend", "ffence", "vprep", "vfence"}
JrnlVals == {"none", "create0", "create1"} \cup FinishKinds

VARIABLES tip, ok, forkTip, logHb, logPres, snapRev, snapHb, snapPres,
          hbExp, truth, proof, hold, handoff, jrnl, jlive, jform,
          delDone, seal, sealTgt, storage, reg, arch, partial, cfgTtl,
          version, hbn, holdn, lockHolder, actors, viol, formStuck

vars == <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb, snapPres,
          hbExp, truth, proof, hold, handoff, jrnl, jlive, jform,
          delDone, seal, sealTgt, storage, reg, arch, partial, cfgTtl,
          version, hbn, holdn, lockHolder, actors, viol, formStuck>>

Idle == [pc |-> 0, p |-> 0, q |-> 0, r |-> 0, s |-> 0]
Me(i) == actors[i]

\* Reloading a finish journal expands a sidecarred inventory in place;
\* the in-memory copy the writer kept is never rewritten.
ReloadForm(i) == IF jform[i] = "sidecar" THEN "expanded" ELSE jform[i]

\* The out-of-lock completion's identity check on raw forms.
FormRefused(i) == jform[i] /= ReloadForm(i) /\ ~NormalizeCompare

Removable(i) ==
  /\ ok = 1 /\ logPres[i] = 1 /\ proof[i] = "dead" /\ hbExp[i] = 1
  /\ hold[i] = 0 /\ handoff[i] = "read" /\ jrnl[i] = "none"
  /\ storage[i] = "present"

Init ==
  /\ tip = 2 /\ ok = 1 /\ forkTip = 0
  /\ logHb = [s \in Slots |-> 1]
  /\ logPres = [s \in Slots |-> IF s = A THEN 1 ELSE (IF BPresent THEN 1 ELSE 0)]
  /\ snapRev = 2 /\ snapHb = logHb /\ snapPres = logPres
  /\ hbExp = [s \in Slots |-> IF Contention THEN 0 ELSE (IF s = A THEN 1 ELSE 0)]
  /\ truth = [s \in Slots |-> IF (s = A /\ (MisjudgedA \/ Contention)) \/ (s = B /\ BPresent) THEN 1 ELSE 0]
  /\ proof = [s \in Slots |-> IF s = A THEN (IF Contention THEN "live" ELSE "dead")
                              ELSE (IF BUnbound THEN "unbound" ELSE "live")]
  /\ hold = [s \in Slots |-> 0]
  /\ handoff = [s \in Slots |-> IF Contention THEN "none" ELSE (IF s = A THEN "read" ELSE "none")]
  /\ jrnl = [s \in Slots |-> "none"] /\ jlive = [s \in Slots |-> 0]
  /\ jform = [s \in Slots |-> "none"] /\ delDone = [s \in Slots |-> FALSE]
  /\ seal = "none" /\ sealTgt = ""
  /\ storage = [s \in Slots |-> IF s = A THEN "present" ELSE (IF BPresent THEN "present" ELSE "absent")]
  /\ reg = [s \in Slots |-> IF s = A THEN RegA ELSE "none"]
  /\ arch = [s \in Slots |-> 0] /\ partial = 0 /\ cfgTtl = 0
  /\ version = 0 /\ hbn = 0 /\ holdn = 0 /\ lockHolder = -1
  /\ actors = [i \in 0..4 |-> Idle]
  /\ viol = "none" /\ formStuck = FALSE

\* One event append with an explicit previous revision: a passed previous
\* is trusted, continuity is judged at load, a fork lands as a valid file.
\* hbSlot / presOff are a slot name or "" for none.
AppendRec(prev, hbSlot, presOff) ==
  IF prev /= tip
  THEN [tip |-> tip + 1, ok |-> 0, forkTip |-> 1,
        viol |-> IF viol = "none" THEN "forked-chain" ELSE viol,
        logHb |-> logHb, logPres |-> logPres]
  ELSE [tip |-> tip + 1, ok |-> ok,
        forkTip |-> IF ok = 0 THEN 0 ELSE forkTip, viol |-> viol,
        logHb |-> IF hbSlot \in Slots
                  THEN [logHb EXCEPT ![hbSlot] = IF logHb[hbSlot] < 2 THEN logHb[hbSlot] + 1 ELSE 2]
                  ELSE logHb,
        logPres |-> IF presOff \in Slots
                    THEN [logPres EXCEPT ![presOff] = 0] ELSE logPres]

LockFree == UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
  snapPres, hbExp, truth, proof, hold, handoff, jrnl, jlive, jform,
  delDone, seal, sealTgt, storage, reg, arch, partial, cfgTtl, hbn,
  holdn, viol, formStuck>>

(***************************************************************************)
(* Actor 0: PreparedRemove(A).  Locals: p prepared tip, q captured         *)
(* registry version, r prepared log heartbeat of B, s prepared snapshot    *)
(* heartbeat of A (plus 8: the basis commit advanced a stale state).       *)
(* PCs 9-10 are the locked-tail completion; PCs 11-13 are the deferred     *)
(* out-of-lock deletion and its under-lock completion (#894/#1735).        *)
(***************************************************************************)
RmLoadFree ==
  /\ ~("rm" \in Disable) /\ Me(0).pc = 0
  /\ Removable(A) /\ jrnl[B] = "none" /\ seal = "none"
  /\ actors' = [actors EXCEPT ![0] = [pc |-> 1, p |-> tip, q |-> 0,
                                      r |-> logHb[B], s |-> snapHb[A]]]
  /\ LockFree /\ UNCHANGED <<version, lockHolder>>

RmSalvage ==
  /\ Me(0).pc = 1
  /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = 2]]
  /\ LockFree /\ UNCHANGED <<version, lockHolder>>

RmCaptureIdentity ==
  /\ Me(0).pc = 2
  /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = 3, !.q = version]]
  /\ LockFree /\ UNCHANGED <<version, lockHolder>>

RmLock ==
  /\ Me(0).pc = 3 /\ lockHolder = -1
  /\ lockHolder' = 0
  /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = 4]]
  /\ LockFree /\ UNCHANGED version

RmAssert ==
  /\ Me(0).pc = 4
  /\ IF version /= Me(0).q \/ snapHb[A] /= Me(0).s \/ snapPres[A] /= 1
     THEN /\ lockHolder' = -1
          /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = DONE]]
     ELSE IF "rebase-on-advance" \in Fixes
          THEN /\ actors' = [actors EXCEPT ![0] =
                    [Me(0) EXCEPT !.pc = 5, !.p = tip, !.r = logHb[B]]]
               /\ UNCHANGED lockHolder
          ELSE IF "assert-compares-revision" \in Fixes /\ tip /= Me(0).p
               THEN /\ lockHolder' = -1
                    /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = DONE]]
               ELSE /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = 5]]
                    /\ UNCHANGED lockHolder
  /\ LockFree /\ UNCHANGED version

RmAppendBasis ==
  /\ Me(0).pc = 5
  /\ IF partial = 1 \/ ("stale-previous-refusal" \in Fixes /\ Me(0).p /= tip)
          \/ (RetainedWriter /\ Me(0).p /= tip)
     THEN /\ lockHolder' = -1
          /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = DONE]]
          /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, viol, version>>
     ELSE LET a == AppendRec(Me(0).p, "", "") IN
          /\ tip' = a.tip /\ ok' = a.ok /\ forkTip' = a.forkTip
          /\ viol' = a.viol /\ logHb' = a.logHb /\ logPres' = a.logPres
          /\ version' = version + 1 /\ UNCHANGED lockHolder
          /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = 6,
                !.s = IF Me(0).p /= tip THEN Me(0).s + 8 ELSE Me(0).s]]
  /\ UNCHANGED <<snapRev, snapHb, snapPres, hbExp, truth, proof, hold,
    handoff, jrnl, jlive, jform, delDone, seal, sealTgt, storage, reg,
    arch, partial, cfgTtl, hbn, holdn, formStuck>>

RmWriteActive ==
  /\ Me(0).pc = 6
  /\ IF Me(0).s >= 8
     THEN /\ snapRev' = Me(0).p + 1
          /\ snapHb' = [snapHb EXCEPT ![B] = Me(0).r] /\ UNCHANGED snapPres
     ELSE /\ snapRev' = tip /\ snapHb' = logHb /\ snapPres' = logPres
  /\ version' = version + 1
  /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = 7]]
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, hbExp, truth, proof,
    hold, handoff, jrnl, jlive, jform, delDone, seal, sealTgt, storage,
    reg, arch, partial, cfgTtl, hbn, holdn, lockHolder, viol, formStuck>>

RmJournalEvent ==
  /\ Me(0).pc = 7
  /\ IF hold[A] = 1 \/ partial = 1
     THEN /\ lockHolder' = -1
          /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = DONE]]
          /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, viol, jrnl,
            jlive, version>>
     ELSE LET a == AppendRec(tip, "", "") IN
          /\ tip' = a.tip /\ ok' = a.ok /\ forkTip' = a.forkTip
          /\ viol' = a.viol /\ logHb' = a.logHb /\ logPres' = a.logPres
          /\ jrnl' = [jrnl EXCEPT ![A] = "fpend"]
          /\ jlive' = [jlive EXCEPT ![A] = 1] /\ version' = version + 1
          /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = 8]]
          /\ UNCHANGED lockHolder
  /\ UNCHANGED <<snapRev, snapHb, snapPres, hbExp, truth, proof, hold,
    handoff, jform, delDone, seal, sealTgt, storage, reg, arch, partial,
    cfgTtl, hbn, holdn, formStuck>>

\* The finish journal file lands fenced; its in-memory form is sidecar
\* exactly when the containment inventory exceeded the inline limit.
RmJournalFileFence ==
  /\ Me(0).pc = 8
  /\ jrnl' = [jrnl EXCEPT ![A] = "ffence"]
  /\ jform' = [jform EXCEPT ![A] = IF RefsLarge THEN "sidecar" ELSE "inline"]
  /\ storage' = [storage EXCEPT ![A] = "fenced"]
  /\ lockHolder' = -1 /\ version' = version + 1
  /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = 9]]
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jlive, delDone, seal,
    sealTgt, reg, arch, partial, cfgTtl, hbn, holdn, viol, formStuck>>

RmTailDelete ==
  /\ Me(0).pc = 9
  /\ storage' = [storage EXCEPT ![A] = "deleted"]
  /\ delDone' = [delDone EXCEPT ![A] = TRUE]
  /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = 10]]
  /\ LockFree /\ UNCHANGED <<version, lockHolder>>

RmComplete ==
  /\ Me(0).pc = 10 /\ lockHolder = -1
  /\ IF ok = 0
     THEN /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = DONE]]
          /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, viol, version,
            lockHolder, snapRev, snapHb, snapPres, jrnl, jlive, jform,
            delDone, arch>>
     ELSE LET a == AppendRec(tip, "", A) IN
          /\ tip' = a.tip /\ ok' = a.ok /\ forkTip' = a.forkTip
          /\ viol' = a.viol /\ logHb' = a.logHb /\ logPres' = a.logPres
          /\ snapRev' = a.tip /\ snapHb' = a.logHb /\ snapPres' = a.logPres
          /\ arch' = [arch EXCEPT ![A] = 1]
          /\ jrnl' = [jrnl EXCEPT ![A] = "none"]
          /\ jlive' = [jlive EXCEPT ![A] = 0]
          /\ jform' = [jform EXCEPT ![A] = "none"]
          /\ delDone' = [delDone EXCEPT ![A] = FALSE]
          /\ version' = version + 1 /\ UNCHANGED lockHolder
          /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = DONE]]
  /\ UNCHANGED <<hbExp, truth, proof, hold, handoff, seal, sealTgt,
    storage, reg, partial, cfgTtl, hbn, holdn, formStuck>>

\* Deferred window: release the lock, delete with no lock held, then
\* complete under the lock against the reloaded journal.
RmReleaseForDelete ==
  /\ Me(0).pc = 9
  /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = 11]]
  /\ LockFree /\ UNCHANGED <<version, lockHolder>>

RmDeleteOutOfLock ==
  /\ Me(0).pc = 11 /\ lockHolder /= 0
  /\ storage' = [storage EXCEPT ![A] = "deleted"]
  /\ delDone' = [delDone EXCEPT ![A] = TRUE]
  /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = 12]]
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jrnl, jlive, jform,
    seal, sealTgt, reg, arch, partial, cfgTtl, version, hbn, holdn,
    lockHolder, viol, formStuck>>

RmCompleteDeferred ==
  /\ Me(0).pc = 12 /\ lockHolder = -1 /\ delDone[A]
  /\ IF FormRefused(A)
     THEN /\ formStuck' = TRUE
          /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = DONE]]
          /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, viol, lockHolder,
            version, snapRev, snapHb, snapPres, jrnl, jlive, jform,
            delDone, arch>>
     ELSE IF ok = 0
          THEN /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = DONE]]
               /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, viol,
                 lockHolder, version, snapRev, snapHb, snapPres, jrnl,
                 jlive, jform, delDone, arch, formStuck>>
          ELSE LET a == AppendRec(tip, "", A) IN
               /\ tip' = a.tip /\ ok' = a.ok /\ forkTip' = a.forkTip
               /\ viol' = a.viol /\ logHb' = a.logHb /\ logPres' = a.logPres
               /\ snapRev' = a.tip /\ snapHb' = a.logHb /\ snapPres' = a.logPres
               /\ arch' = [arch EXCEPT ![A] = 1]
               /\ jrnl' = [jrnl EXCEPT ![A] = "none"]
               /\ jlive' = [jlive EXCEPT ![A] = 0]
               /\ jform' = [jform EXCEPT ![A] = "none"]
               /\ delDone' = [delDone EXCEPT ![A] = FALSE]
               /\ version' = version + 1 /\ UNCHANGED lockHolder
               /\ actors' = [actors EXCEPT ![0] = [Me(0) EXCEPT !.pc = DONE]]
               /\ UNCHANGED formStuck
  /\ UNCHANGED <<hbExp, truth, proof, hold, handoff, seal, sealTgt,
    storage, reg, partial, cfgTtl, hbn, holdn>>

(***************************************************************************)
(* Actor 1: Heartbeat (B, or A in contention runs): fully locked.          *)
(***************************************************************************)
HbTgt == IF Contention THEN A ELSE B
HbLock ==
  /\ Me(1).pc = 0 /\ hbn < MaxHeartbeats /\ ok = 1
  /\ logPres[HbTgt] = 1 /\ truth[HbTgt] = 1 /\ proof[HbTgt] = "live"
  /\ lockHolder = -1 /\ ~(seal /= "none" /\ sealTgt = HbTgt)
  /\ lockHolder' = 1
  /\ actors' = [actors EXCEPT ![1] = [Me(1) EXCEPT !.pc = 1]]
  /\ LockFree /\ UNCHANGED version

HbAppend ==
  /\ Me(1).pc = 1
  /\ IF partial = 1
     THEN /\ lockHolder' = -1
          /\ actors' = [actors EXCEPT ![1] = [Me(1) EXCEPT !.pc = DONE]]
          /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, viol, hbExp,
            hbn, version>>
     ELSE LET a == AppendRec(tip, HbTgt, "") IN
          /\ tip' = a.tip /\ ok' = a.ok /\ forkTip' = a.forkTip
          /\ viol' = a.viol /\ logHb' = a.logHb /\ logPres' = a.logPres
          /\ hbExp' = [hbExp EXCEPT ![HbTgt] = 0] /\ hbn' = hbn + 1
          /\ version' = version + 1 /\ UNCHANGED lockHolder
          /\ actors' = [actors EXCEPT ![1] = [Me(1) EXCEPT !.pc = 2]]
  /\ UNCHANGED <<snapRev, snapHb, snapPres, truth, proof, hold, handoff,
    jrnl, jlive, jform, delDone, seal, sealTgt, storage, reg, arch,
    partial, cfgTtl, holdn, formStuck>>

HbWriteActive ==
  /\ Me(1).pc = 2
  /\ snapRev' = tip /\ snapHb' = logHb /\ snapPres' = logPres
  /\ lockHolder' = -1 /\ version' = version + 1
  /\ actors' = [actors EXCEPT ![1] = [Me(1) EXCEPT !.pc = IF Contention THEN 0 ELSE DONE]]
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, hbExp, truth, proof,
    hold, handoff, jrnl, jlive, jform, delDone, seal, sealTgt, storage,
    reg, arch, partial, cfgTtl, hbn, holdn, viol, formStuck>>

(***************************************************************************)
(* Actor 2: Create(B): journal before worktrees, publish row, clear.       *)
(***************************************************************************)
CrJournal ==
  /\ ~("cr" \in Disable) /\ Me(2).pc = 0 /\ ok = 1
  /\ logPres[B] = 0 /\ storage[B] = "absent" /\ jrnl[B] = "none"
  /\ lockHolder = -1 /\ seal = "none"
  \* A commit first meets _assert_no_journal: a live finish journal
  \* on any slot raises _JournalBusy and nothing is written (#1367).
  \* WindowWriteHypothesis removes that gate (the suspected bug).
  /\ (WindowWriteHypothesis \/ ~(jrnl[A] \in FinishKinds /\ jlive[A] = 1))
  /\ lockHolder' = 2 /\ jrnl' = [jrnl EXCEPT ![B] = "create0"]
  /\ jlive' = [jlive EXCEPT ![B] = 1] /\ version' = version + 1
  /\ actors' = [actors EXCEPT ![2] = [Me(2) EXCEPT !.pc = 1,
        !.q = IF Me(0).pc \in {11, 12} /\ jlive[A] = 1 THEN 1 ELSE Me(2).q]]
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jform, delDone, seal,
    sealTgt, storage, reg, arch, partial, cfgTtl, hbn, holdn, viol,
    formStuck>>

CrWorktrees ==
  /\ Me(2).pc = 1
  /\ storage' = [storage EXCEPT ![B] = "present"]
  /\ actors' = [actors EXCEPT ![2] = [Me(2) EXCEPT !.pc = 2]]
  /\ LockFree /\ UNCHANGED <<version, lockHolder>>

CrPublish ==
  /\ Me(2).pc = 2
  /\ IF partial = 1
     THEN /\ lockHolder' = -1 /\ jlive' = [jlive EXCEPT ![B] = 0]
          /\ actors' = [actors EXCEPT ![2] = [Me(2) EXCEPT !.pc = DONE]]
          /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, viol, snapRev,
            snapHb, snapPres, jrnl, version>>
     ELSE LET a == AppendRec(tip, "", "") IN
          /\ tip' = a.tip /\ ok' = a.ok /\ forkTip' = a.forkTip
          /\ viol' = a.viol /\ logHb' = a.logHb
          /\ logPres' = [a.logPres EXCEPT ![B] = 1]
          /\ snapRev' = a.tip /\ snapHb' = a.logHb
          /\ snapPres' = [a.logPres EXCEPT ![B] = 1]
          /\ proof' = [proof EXCEPT ![B] = "live"]
          /\ truth' = [truth EXCEPT ![B] = 1]
          /\ hbExp' = [hbExp EXCEPT ![B] = 0]
          /\ jrnl' = [jrnl EXCEPT ![B] = "create1"] /\ version' = version + 1
          /\ actors' = [actors EXCEPT ![2] = [Me(2) EXCEPT !.pc = 3]]
          /\ UNCHANGED lockHolder
  /\ UNCHANGED <<hold, handoff, jlive, jform, delDone, seal, sealTgt,
    storage, reg, arch, partial, cfgTtl, hbn, holdn, formStuck>>

CrClearJournal ==
  /\ Me(2).pc = 3
  /\ jrnl' = [jrnl EXCEPT ![B] = "none"] /\ jlive' = [jlive EXCEPT ![B] = 0]
  /\ lockHolder' = -1 /\ version' = version + 1
  /\ actors' = [actors EXCEPT ![2] = [Me(2) EXCEPT !.pc = DONE, !.q = 0]]
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jform, delDone, seal,
    sealTgt, storage, reg, arch, partial, cfgTtl, hbn, holdn, viol,
    formStuck>>

(***************************************************************************)
(* Actor 3: ValidateBatchRemove(A): seal, finish journal, fence, then      *)
(* (fixed) classify-before-delete or (pre-fix) delete-first.               *)
(***************************************************************************)
VbSeal ==
  /\ ~("vb" \in Disable) /\ Me(3).pc = 0 /\ ok = 1
  /\ logPres[A] = 1 /\ jrnl[A] = "none" /\ seal = "none"
  /\ lockHolder = -1 /\ storage[A] = "present" /\ hold[A] = 0
  /\ lockHolder' = 3 /\ seal' = "live" /\ sealTgt' = A
  /\ version' = version + 1
  /\ actors' = [actors EXCEPT ![3] = [Me(3) EXCEPT !.pc = 1]]
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jrnl, jlive, jform,
    delDone, storage, reg, arch, partial, cfgTtl, hbn, holdn, viol,
    formStuck>>

VbFinishPrepared ==
  /\ Me(3).pc = 1
  /\ jrnl' = [jrnl EXCEPT ![A] = "vprep"] /\ jlive' = [jlive EXCEPT ![A] = 1]
  /\ version' = version + 1
  /\ actors' = [actors EXCEPT ![3] = [Me(3) EXCEPT !.pc = 2]]
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jform, delDone, seal,
    sealTgt, storage, reg, arch, partial, cfgTtl, hbn, holdn, lockHolder,
    viol, formStuck>>

VbFence ==
  /\ Me(3).pc = 2
  /\ jrnl' = [jrnl EXCEPT ![A] = "vfence"]
  /\ jform' = [jform EXCEPT ![A] = IF RefsLarge THEN "sidecar" ELSE "inline"]
  /\ storage' = [storage EXCEPT ![A] = "fenced"] /\ version' = version + 1
  /\ actors' = [actors EXCEPT ![3] = [Me(3) EXCEPT !.pc = 3]]
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jlive, delDone, seal,
    sealTgt, reg, arch, partial, cfgTtl, hbn, holdn, lockHolder, viol,
    formStuck>>

VbClassify ==
  /\ Me(3).pc = 3
  /\ IF "pre-delete-classify" \in Fixes
     THEN IF reg[A] = "foreign"
          THEN /\ lockHolder' = -1 /\ seal' = "dead"
               /\ jlive' = [jlive EXCEPT ![A] = 0]
               /\ actors' = [actors EXCEPT ![3] = [Me(3) EXCEPT !.pc = DONE]]
               /\ UNCHANGED reg
          ELSE IF reg[A] = "own"
               THEN /\ reg' = [reg EXCEPT ![A] = "none"]
                    /\ actors' = [actors EXCEPT ![3] = [Me(3) EXCEPT !.pc = 4]]
                    /\ UNCHANGED <<lockHolder, seal, jlive>>
               ELSE /\ actors' = [actors EXCEPT ![3] = [Me(3) EXCEPT !.pc = 4]]
                    /\ UNCHANGED <<lockHolder, seal, jlive, reg>>
     ELSE /\ actors' = [actors EXCEPT ![3] = [Me(3) EXCEPT !.pc = 4]]
          /\ UNCHANGED <<lockHolder, seal, jlive, reg>>
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jrnl, jform, delDone,
    sealTgt, storage, arch, partial, cfgTtl, version, hbn, holdn, viol,
    formStuck>>

VbDeleteSubvolume ==
  /\ Me(3).pc = 4
  /\ viol' = IF viol = "none" /\ reg[A] /= "none"
             THEN "deleted-while-locked" ELSE viol
  /\ storage' = [storage EXCEPT ![A] = "deleted"]
  /\ delDone' = [delDone EXCEPT ![A] = TRUE] /\ version' = version + 1
  /\ actors' = [actors EXCEPT ![3] = [Me(3) EXCEPT !.pc = 5]]
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jrnl, jlive, jform,
    seal, sealTgt, reg, arch, partial, cfgTtl, hbn, holdn, lockHolder,
    formStuck>>

VbPrune ==
  /\ Me(3).pc = 5
  /\ IF "pre-delete-classify" \notin Fixes /\ reg[A] /= "none"
     THEN /\ lockHolder' = -1 /\ seal' = "dead"
          /\ jlive' = [jlive EXCEPT ![A] = 0]
          /\ actors' = [actors EXCEPT ![3] = [Me(3) EXCEPT !.pc = DONE]]
     ELSE /\ actors' = [actors EXCEPT ![3] = [Me(3) EXCEPT !.pc = 6]]
          /\ UNCHANGED <<lockHolder, seal, jlive>>
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jrnl, jform, delDone,
    sealTgt, storage, reg, arch, partial, cfgTtl, version, hbn, holdn,
    viol, formStuck>>

VbArchive ==
  /\ Me(3).pc = 6
  /\ IF partial = 1
     THEN /\ lockHolder' = -1 /\ seal' = "dead"
          /\ jlive' = [jlive EXCEPT ![A] = 0]
          /\ actors' = [actors EXCEPT ![3] = [Me(3) EXCEPT !.pc = DONE]]
          /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, viol, snapRev,
            snapHb, snapPres, jrnl, jform, delDone, arch, version, sealTgt>>
     ELSE LET a == AppendRec(tip, "", A) IN
          /\ tip' = a.tip /\ ok' = a.ok /\ forkTip' = a.forkTip
          /\ viol' = a.viol /\ logHb' = a.logHb /\ logPres' = a.logPres
          /\ snapRev' = a.tip /\ snapHb' = a.logHb /\ snapPres' = a.logPres
          /\ arch' = [arch EXCEPT ![A] = 1]
          /\ jrnl' = [jrnl EXCEPT ![A] = "none"]
          /\ jlive' = [jlive EXCEPT ![A] = 0]
          /\ jform' = [jform EXCEPT ![A] = "none"]
          /\ delDone' = [delDone EXCEPT ![A] = FALSE]
          /\ seal' = "none" /\ sealTgt' = "" /\ lockHolder' = -1
          /\ version' = version + 1
          /\ actors' = [actors EXCEPT ![3] = [Me(3) EXCEPT !.pc = DONE]]
  /\ UNCHANGED <<hbExp, truth, proof, hold, handoff, storage, reg,
    partial, cfgTtl, hbn, holdn, formStuck>>

(***************************************************************************)
(* Actor 4: PrevalidatedAdopt(B), contention runs only: capture identity,  *)
(* prepare lock-free, commit only if identity still matches; 5 rounds.     *)
(***************************************************************************)
AdPrepareFree ==
  /\ BUnbound /\ Me(4).pc = 0 /\ ok = 1 /\ logPres[B] = 1
  /\ proof[B] = "unbound" /\ jrnl[A] = "none" /\ jrnl[B] = "none"
  /\ actors' = [actors EXCEPT ![4] = [pc |-> 1, p |-> version,
                                      q |-> Me(4).q, r |-> 0, s |-> 0]]
  /\ LockFree /\ UNCHANGED <<version, lockHolder>>

AdLock ==
  /\ BUnbound /\ Me(4).pc = 1 /\ lockHolder = -1
  /\ lockHolder' = 4
  /\ actors' = [actors EXCEPT ![4] = [Me(4) EXCEPT !.pc = 2]]
  /\ LockFree /\ UNCHANGED version

AdCommit ==
  /\ BUnbound /\ Me(4).pc = 2 /\ ok = 1 /\ partial = 0
  /\ version = Me(4).p
  \* Same _assert_no_journal gate as create (adopt commit, prepared
  \* is None exactly when a journal was outstanding at prepare).
  /\ (WindowWriteHypothesis \/ ~(jrnl[A] \in FinishKinds /\ jlive[A] = 1))
  /\ LET a == AppendRec(tip, B, "") IN
     /\ tip' = a.tip /\ ok' = a.ok /\ forkTip' = a.forkTip
     /\ viol' = a.viol /\ logHb' = a.logHb /\ logPres' = a.logPres
     /\ snapRev' = a.tip /\ snapHb' = a.logHb /\ snapPres' = a.logPres
     /\ proof' = [proof EXCEPT ![B] = "live"]
     /\ hbExp' = [hbExp EXCEPT ![B] = 0]
     /\ lockHolder' = -1 /\ version' = version + 1
     /\ actors' = [actors EXCEPT ![4] = [Me(4) EXCEPT !.pc = DONE,
           !.s = IF Me(0).pc \in {11, 12} /\ jlive[A] = 1 THEN 1 ELSE Me(4).s]]
  /\ UNCHANGED <<truth, hold, handoff, jrnl, jlive, jform, delDone, seal,
    sealTgt, storage, reg, arch, partial, cfgTtl, hbn, holdn, formStuck>>

AdRetry ==
  /\ BUnbound /\ Me(4).pc = 2 /\ (ok = 0 \/ partial = 1 \/ version /= Me(4).p)
  /\ lockHolder' = -1
  /\ IF ok = 0 \/ partial = 1
     THEN actors' = [actors EXCEPT ![4] = [Me(4) EXCEPT !.pc = DONE]]
     ELSE IF Me(4).q + 1 >= 5
          THEN actors' = [actors EXCEPT ![4] = [Me(4) EXCEPT !.pc = BUSY, !.q = Me(4).q + 1]]
          ELSE actors' = [actors EXCEPT ![4] = [Me(4) EXCEPT !.pc = 0, !.q = Me(4).q + 1]]
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jrnl, jlive, jform,
    delDone, seal, sealTgt, storage, reg, arch, partial, cfgTtl, version,
    hbn, holdn, viol, formStuck>>

(***************************************************************************)
(* Disjoint create/adopt meeting the out-of-lock finish window: the        *)
(* prevalidated rounds take the locks, _assert_no_journal sees the live    *)
(* finish journal, waits bounded, and refuses busy with nothing written    *)
(* (#1367).  These are the refusal edges; the commit edges above carry     *)
(* the same gate, and WindowWriteHypothesis is the control that removes    *)
(* it (actor locals q / s record a commit taken inside the window).        *)
(***************************************************************************)
GjCreateBusy ==
  /\ ~WindowWriteHypothesis /\ Me(2).pc = 0 /\ Me(0).pc \in {11, 12}
  /\ jlive[A] = 1 /\ lockHolder = -1
  /\ actors' = [actors EXCEPT ![2] = [Me(2) EXCEPT !.pc = BUSY]]
  /\ LockFree /\ UNCHANGED <<version, lockHolder>>

GjAdoptBusy ==
  /\ ~WindowWriteHypothesis /\ BUnbound /\ Me(4).pc = 2
  /\ lockHolder = 4 /\ jrnl[A] \in FinishKinds /\ jlive[A] = 1
  /\ lockHolder' = -1
  /\ actors' = [actors EXCEPT ![4] = [Me(4) EXCEPT !.pc = BUSY]]
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jrnl, jlive, jform,
    delDone, seal, sealTgt, storage, reg, arch, partial, cfgTtl, version,
    hbn, holdn, viol, formStuck>>

(***************************************************************************)
(* Recover edges (atomic, lock-free entry): shard heal, partial discard,   *)
(* tail truncate, finish archive, create clear/complete/abort, seal retire.*)
(* Finish recovery consumes the expanded journal by record, so the form    *)
(* refusal never gates it: RECOVERY REQUIRED leaves recover as the exit.   *)
(***************************************************************************)
RecShardSync ==
  /\ lockHolder = -1 /\ ok = 1
  /\ <<snapRev, snapHb, snapPres>> /= <<tip, logHb, logPres>>
  /\ snapRev' = tip /\ snapHb' = logHb /\ snapPres' = logPres
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, hbExp, truth, proof,
    hold, handoff, jrnl, jlive, jform, delDone, seal, sealTgt, storage,
    reg, arch, partial, cfgTtl, version, hbn, holdn, lockHolder, actors,
    viol, formStuck>>

RecDiscardPartial ==
  /\ lockHolder = -1 /\ partial = 1
  /\ partial' = 0 /\ version' = version + 1
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jrnl, jlive, jform,
    delDone, seal, sealTgt, storage, reg, arch, cfgTtl, hbn, holdn,
    lockHolder, actors, viol, formStuck>>

RecTailTruncate ==
  /\ lockHolder = -1 /\ ok = 0 /\ forkTip = 1
  /\ jrnl = [s \in Slots |-> "none"] /\ seal = "none" /\ partial = 0
  /\ tip' = tip - 1 /\ ok' = 1 /\ forkTip' = 0 /\ snapRev' = tip - 1
  /\ snapHb' = logHb /\ snapPres' = logPres /\ version' = version + 1
  /\ UNCHANGED <<logHb, logPres, hbExp, truth, proof, hold, handoff,
    jrnl, jlive, jform, delDone, seal, sealTgt, storage, reg, arch,
    partial, cfgTtl, hbn, holdn, lockHolder, actors, viol, formStuck>>

RecFinishArchive(i) ==
  /\ lockHolder = -1 /\ ok = 1 /\ jrnl[i] \in FinishKinds /\ jlive[i] = 0
  /\ hold[i] = 0 /\ hbExp[i] = 1 /\ proof[i] = "dead"
  /\ ~(reg[i] = "foreign" /\ jrnl[i] \in {"vprep", "vfence"})
  /\ LET a == AppendRec(tip, "", i) IN
     /\ tip' = a.tip /\ ok' = a.ok /\ forkTip' = a.forkTip
     /\ viol' = IF viol = "none"
                   /\ "pre-delete-classify" \notin Fixes
                   /\ jrnl[i] \in {"vprep", "vfence"}
                   /\ reg[i] = "own" /\ storage[i] /= "deleted"
                THEN "deleted-while-locked" ELSE a.viol
     /\ logHb' = a.logHb /\ logPres' = a.logPres
     /\ snapRev' = a.tip /\ snapHb' = a.logHb /\ snapPres' = a.logPres
     /\ arch' = [arch EXCEPT ![i] = 1]
     /\ jrnl' = [jrnl EXCEPT ![i] = "none"]
     /\ jlive' = [jlive EXCEPT ![i] = 0]
     /\ jform' = [jform EXCEPT ![i] = "none"]
     /\ delDone' = [delDone EXCEPT ![i] = FALSE]
     /\ storage' = [storage EXCEPT ![i] = "deleted"]
     /\ reg' = IF jrnl[i] \in {"vprep", "vfence"}
               THEN [reg EXCEPT ![i] = "none"] ELSE reg
     /\ seal' = IF sealTgt = i THEN "none" ELSE seal
     /\ sealTgt' = IF sealTgt = i THEN "" ELSE sealTgt
     /\ version' = version + 1
  /\ UNCHANGED <<hbExp, truth, proof, hold, handoff, partial, cfgTtl,
    hbn, holdn, lockHolder, actors, formStuck>>

RecCreate(i) ==
  /\ lockHolder = -1 /\ ok = 1 /\ jrnl[i] \in {"create0", "create1"}
  /\ jlive[i] = 0 /\ cfgTtl = 0
  /\ IF jrnl[i] = "create1"
     THEN /\ jrnl' = [jrnl EXCEPT ![i] = "none"] /\ version' = version + 1
          /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, viol, snapRev,
            snapHb, snapPres, storage, proof, truth, hbExp>>
     ELSE \/ LET a == AppendRec(tip, "", "") IN
             /\ tip' = a.tip /\ ok' = a.ok /\ forkTip' = a.forkTip
             /\ viol' = a.viol /\ logHb' = a.logHb
             /\ logPres' = [a.logPres EXCEPT ![i] = 1]
             /\ snapRev' = a.tip /\ snapHb' = a.logHb
             /\ snapPres' = [a.logPres EXCEPT ![i] = 1]
             /\ proof' = [proof EXCEPT ![i] = "live"]
             /\ truth' = [truth EXCEPT ![i] = 1]
             /\ hbExp' = [hbExp EXCEPT ![i] = 0]
             /\ storage' = [storage EXCEPT ![i] = "present"]
             /\ jrnl' = [jrnl EXCEPT ![i] = "none"] /\ version' = version + 1
          \/ /\ jrnl' = [jrnl EXCEPT ![i] = "none"]
             /\ storage' = [storage EXCEPT ![i] = "absent"]
             /\ version' = version + 1
             /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, viol,
               snapRev, snapHb, snapPres, proof, truth, hbExp>>
  /\ UNCHANGED <<hold, handoff, jlive, jform, delDone, seal, sealTgt,
    reg, arch, partial, cfgTtl, hbn, holdn, lockHolder, actors, formStuck>>

RecSealRetire ==
  /\ lockHolder = -1 /\ ok = 1 /\ seal = "dead" /\ sealTgt \in Slots
  /\ jrnl[sealTgt] = "none"
  /\ seal' = "none" /\ sealTgt' = "" /\ version' = version + 1
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jrnl, jlive, jform,
    delDone, storage, reg, arch, partial, cfgTtl, hbn, holdn, lockHolder,
    actors, viol, formStuck>>

(***************************************************************************)
(* Environment: crashes at every phase boundary, a mid-write crash that    *)
(* leaves a partial tmp, clocks, owner facts, holds, config edits, and     *)
(* hand actions outside wrkslots.                                          *)
(***************************************************************************)
CrashOf(i) ==
  /\ Me(i).pc \notin {0, DONE, BUSY}
  /\ actors' = [actors EXCEPT ![i] = [Me(i) EXCEPT !.pc = DONE]]
  /\ lockHolder' = IF lockHolder = i THEN -1 ELSE lockHolder
  /\ jlive' = IF i \in {0, 3} /\ jrnl[A] /= "none"
              THEN [jlive EXCEPT ![A] = 0]
              ELSE IF i = 2 /\ jrnl[B] /= "none"
                   THEN [jlive EXCEPT ![B] = 0] ELSE jlive
  /\ seal' = IF i = 3 /\ seal = "live" THEN "dead" ELSE seal
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jrnl, jform, delDone,
    sealTgt, storage, reg, arch, partial, cfgTtl, version, hbn, holdn,
    viol, formStuck>>

CrashMidWrite ==
  /\ lockHolder \in {0, 1} /\ partial = 0
  /\ partial' = 1 /\ lockHolder' = -1
  /\ actors' = [j \in 0..4 |-> IF j = lockHolder
                               THEN [actors[j] EXCEPT !.pc = DONE]
                               ELSE actors[j]]
  /\ jlive' = IF lockHolder = 0 /\ jrnl[A] /= "none"
              THEN [jlive EXCEPT ![A] = 0] ELSE jlive
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jrnl, jform, delDone,
    seal, sealTgt, storage, reg, arch, cfgTtl, version, hbn, holdn, viol,
    formStuck>>

Tick ==
  /\ \E i \in Slots: logPres[i] = 1 /\ hbExp[i] = 0
  /\ hbExp' = [i \in Slots |-> IF logPres[i] = 1 THEN 1 ELSE hbExp[i]]
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, truth, proof, hold, handoff, jrnl, jlive, jform, delDone,
    seal, sealTgt, storage, reg, arch, partial, cfgTtl, version, hbn,
    holdn, lockHolder, actors, viol, formStuck>>

OwnerDeath(i) ==
  /\ truth[i] = 1 /\ logPres[i] = 1
  /\ truth' = [truth EXCEPT ![i] = 0]
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, proof, hold, handoff, jrnl, jlive, jform, delDone,
    seal, sealTgt, storage, reg, arch, partial, cfgTtl, version, hbn,
    holdn, lockHolder, actors, viol, formStuck>>

ProbeDead(i) ==
  /\ truth[i] = 0 /\ proof[i] = "live" /\ logPres[i] = 1
  /\ proof' = [proof EXCEPT ![i] = "dead"]
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, hold, handoff, jrnl, jlive, jform, delDone,
    seal, sealTgt, storage, reg, arch, partial, cfgTtl, version, hbn,
    holdn, lockHolder, actors, viol, formStuck>>

RenewHeartbeatA ==
  /\ ok = 1 /\ partial = 0 /\ lockHolder = -1 /\ logPres[A] = 1
  /\ truth[A] = 1 /\ hbExp[A] = 1 /\ logHb[A] < 2
  /\ ~(seal /= "none" /\ sealTgt = A)
  /\ LET a == AppendRec(tip, A, "") IN
     /\ tip' = a.tip /\ ok' = a.ok /\ forkTip' = a.forkTip
     /\ viol' = a.viol /\ logHb' = a.logHb /\ logPres' = a.logPres
     /\ snapRev' = a.tip /\ snapHb' = a.logHb /\ snapPres' = a.logPres
     /\ hbExp' = [hbExp EXCEPT ![A] = 0] /\ version' = version + 1
  /\ UNCHANGED <<truth, proof, hold, handoff, jrnl, jlive, jform,
    delDone, seal, sealTgt, storage, reg, arch, partial, cfgTtl, hbn,
    holdn, lockHolder, actors, formStuck>>

HoldToggle ==
  /\ ok = 1 /\ lockHolder = -1 /\ logPres[A] = 1 /\ holdn < 2
  /\ ((hold[A] = 0 /\ jrnl[A] = "none") \/ hold[A] = 1)
  /\ hold' = [hold EXCEPT ![A] = 1 - hold[A]] /\ holdn' = holdn + 1
  /\ version' = version + 1
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, handoff, jrnl, jlive, jform, delDone,
    seal, sealTgt, storage, reg, arch, partial, cfgTtl, hbn, lockHolder,
    actors, viol, formStuck>>

ConfigTtlEdit ==
  /\ cfgTtl = 0 /\ \E i \in Slots: jrnl[i] \in {"create0", "create1"}
  /\ cfgTtl' = 1
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jrnl, jlive, jform,
    delDone, seal, sealTgt, storage, reg, arch, partial, version, hbn,
    holdn, lockHolder, actors, viol, formStuck>>

ConfigTtlRevert ==  \* hand action outside wrkslots
  /\ cfgTtl = 1 /\ cfgTtl' = 0
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jrnl, jlive, jform,
    delDone, seal, sealTgt, storage, reg, arch, partial, version, hbn,
    holdn, lockHolder, actors, viol, formStuck>>

HandGitUnlock ==  \* hand action outside wrkslots
  /\ reg[A] /= "none"
  /\ reg' = [reg EXCEPT ![A] = "none"]
  /\ UNCHANGED <<tip, ok, forkTip, logHb, logPres, snapRev, snapHb,
    snapPres, hbExp, truth, proof, hold, handoff, jrnl, jlive, jform,
    delDone, seal, sealTgt, storage, arch, partial, cfgTtl, version,
    hbn, holdn, lockHolder, actors, viol, formStuck>>

Resting == lockHolder = -1 /\ \A i \in 0..4: Me(i).pc \in {0, DONE, BUSY}

IsClean ==
  /\ ok = 1 /\ partial = 0 /\ seal = "none" /\ lockHolder = -1
  /\ jrnl = [s \in Slots |-> "none"]
  /\ snapRev = tip /\ snapHb = logHb /\ snapPres = logPres
  /\ \A i \in 0..4: Me(i).pc \in {0, DONE, BUSY}
  /\ \A i \in Slots:
       IF arch[i] = 1 THEN logPres[i] = 0 /\ storage[i] \in {"deleted", "absent"}
       ELSE IF logPres[i] = 1 THEN storage[i] = "present"
            ELSE storage[i] = "absent"

Next ==
  \/ RmLoadFree \/ RmSalvage \/ RmCaptureIdentity \/ RmLock \/ RmAssert
  \/ RmAppendBasis \/ RmWriteActive \/ RmJournalEvent \/ RmJournalFileFence
  \/ RmTailDelete \/ RmComplete \/ RmReleaseForDelete \/ RmDeleteOutOfLock
  \/ RmCompleteDeferred
  \/ HbLock \/ HbAppend \/ HbWriteActive
  \/ CrJournal \/ CrWorktrees \/ CrPublish \/ CrClearJournal
  \/ VbSeal \/ VbFinishPrepared \/ VbFence \/ VbClassify
  \/ VbDeleteSubvolume \/ VbPrune \/ VbArchive
  \/ AdPrepareFree \/ AdLock \/ AdCommit \/ AdRetry
  \/ GjCreateBusy \/ GjAdoptBusy
  \/ RecShardSync \/ RecDiscardPartial \/ RecTailTruncate
  \/ RecFinishArchive(A) \/ RecFinishArchive(B)
  \/ RecCreate(A) \/ RecCreate(B) \/ RecSealRetire
  \/ CrashOf(0) \/ CrashOf(1) \/ CrashOf(2) \/ CrashOf(3) \/ CrashOf(4)
  \/ CrashMidWrite \/ Tick
  \/ OwnerDeath(A) \/ OwnerDeath(B) \/ ProbeDead(A) \/ ProbeDead(B)
  \/ RenewHeartbeatA \/ HoldToggle \/ ConfigTtlEdit \/ ConfigTtlRevert
  \/ HandGitUnlock
  \/ (Resting /\ UNCHANGED vars)

Fairness ==
  /\ WF_vars(RecFinishArchive(A)) /\ WF_vars(RecFinishArchive(B))
  /\ WF_vars(RecCreate(A)) /\ WF_vars(RecCreate(B))
  /\ WF_vars(RecTailTruncate) /\ WF_vars(Tick)
  /\ WF_vars(RecDiscardPartial) /\ WF_vars(RecShardSync)
  /\ WF_vars(RecSealRetire)
  \* Operator hand actions are fair too: the liveness claim is
  \* completion by recover plus at most the documented hand exits
  \* (config revert, hand unlock), i.e. no wedge and no manual
  \* terminal beyond those two.
  /\ WF_vars(ConfigTtlRevert) /\ WF_vars(HandGitUnlock)
  \* Commands are short-lived processes: a live actor eventually
  \* exits (in this model CrashOf stands for exit-or-crash; either
  \* frees its lock and kills its journal's liveness), which is the
  \* fairness reading of the Python explorer's reachability check.
  /\ WF_vars(CrashOf(0)) /\ WF_vars(CrashOf(1)) /\ WF_vars(CrashOf(2))
  /\ WF_vars(CrashOf(3)) /\ WF_vars(CrashOf(4)) /\ WF_vars(HbWriteActive)

Spec == Init /\ [][Next]_vars /\ Fairness

(***************************************************************************)
(* Checked properties.  Per-config expectations (which fail, and why that   *)
(* failure is the cross-check) are tabulated in TRANSITIONMODEL.md.        *)
(***************************************************************************)
TypeOK ==
  /\ tip \in 0..12 /\ ok \in {0, 1} /\ forkTip \in {0, 1}
  /\ lockHolder \in -1..4 /\ partial \in {0, 1} /\ cfgTtl \in {0, 1}
  /\ viol \in {"none", "forked-chain", "deleted-while-locked"}
  /\ \A i \in Slots: jrnl[i] \in JrnlVals /\ logHb[i] \in 0..2

NoViol == viol = "none"
NoFormStuck == ~formStuck

\* The reloaded journal equals the in-memory journal (raw forms).
ReloadEqualsMemory ==
  \A i \in Slots: jrnl[i] \in FinishKinds => ReloadForm(i) = jform[i]

\* No refusal on a quiescent consistent store: resting, loadable,
\* snapshot in sync, yet the form ghost fired earlier on this path.
QuiescentNoRefusal ==
  (Resting /\ ok = 1 /\ snapRev = tip /\ snapHb = logHb
   /\ snapPres = logPres) => ~formStuck

\* The event chain always loads, or its failure carries a ghost:
\* no silent unloadable resting state.  (The ghost is first-violation
\* sticky, so an unloadable state reached after another violation
\* carries that earlier tag instead.)
LoadableOrGhosted == Resting /\ ok = 0 => viol /= "none"

\* C1/#1830 wedge shapes (expected reachable on current main).
NotBuriedForkJournal == ~(ok = 0 /\ forkTip = 0 /\ jrnl[A] \in {"fpend", "ffence"})
NotTipForkDeadSeal ==
  ~(ok = 0 /\ forkTip = 1 /\ jrnl = [s \in Slots |-> "none"] /\ seal = "dead")

NoAdoptBusy == Me(4).pc /= BUSY

\* Disjoint-commit-in-window ghosts: q / s are set only by a create /
\* adopt commit taken inside the live out-of-lock finish window.
NoWindowJournal == Me(2).q = 0
NoWindowAdopt == Me(4).s = 0
NoCreateBusyInWindow == ~(Me(0).pc \in {11, 12} /\ Me(2).pc = BUSY)
NoAdoptBusyWithFinish == ~(jrnl[A] \in FinishKinds /\ Me(4).pc = BUSY)

\* Every interrupted state is completed by one recover (liveness form).
Interrupted == \E i \in Slots: jrnl[i] /= "none" /\ jlive[i] = 0
OneRecoverCompletes == Interrupted ~> IsClean

THEOREM Spec => []TypeOK
=============================================================================
