-------------------------- MODULE WrkslotsLifecycle --------------------------
(****************************************************************************)
(* Slot lifecycle specification (#1568).                                    *)
(*                                                                          *)
(* This text mirrors the gated Python explorer in                           *)
(* py/wrkslots/lifecyclemodel.py action for action.  The explorer is the    *)
(* checker that runs in validation (no downloaded toolchain); this spec    *)
(* is the same model in temporal form for readers who want fairness        *)
(* stated explicitly.  To check it manually:                                *)
(*                                                                          *)
(*   gh release download v1.8.0 -R tlaplus/tlaplus -p tla2tools.jar         *)
(*   <java17> -jar tla2tools.jar WrkslotsLifecycle.tla                      *)
(*                                                                          *)
(* with WrkslotsLifecycle.cfg selecting constants.  Constants FINISHER etc. *)
(* are the fix flags; BUGS selects historical-bug toggles.  Safety is an    *)
(* invariant over the sticky ghost variable viol; liveness is stated       *)
(* under weak fairness of the finisher policy actions and of Tick.          *)
(****************************************************************************)
EXTENDS Integers, TLC

CONSTANTS SlotType,          \* "agent" or "validate" (scenario constant)
          SubjectVcs,        \* "git" or "eden"
          OtherJournal,      \* TRUE: another slot's journal blocks us
          OtherEden,         \* TRUE: another slot's Eden row exists
          MirrorFails,       \* TRUE: ledger mirror never succeeds
          Fixes,             \* subset of FixNames
          Bugs               \* subset of BugNames

FixNames == {"finisher", "delegated", "hold-expiry", "reregister-branch",
             "per-slot-journals", "auto-recover-unbound", "ledger-abandon",
             "eden-remove", "audit-submodule"}
BugNames == {"eden-guard", "ttl-unfixed", "apply-journal"}

VARIABLES row, storage, truth, proof, hb, handoff, hold, hage, history,
          branch, nested, occ, journal, pub, retries, coord, viol

vars == <<row, storage, truth, proof, hb, handoff, hold, hage, history,
          branch, nested, occ, journal, pub, retries, coord, viol>>

Finisher == "finisher" \in Fixes
Authorized == (coord = "orig") \/ ("delegated" \in Fixes)
JournalBlocked == (journal /= "none")
                    \/ (OtherJournal /\ "per-slot-journals" \notin Fixes)
OwnerOk == (proof = "dead") \/ (proof = "unbound" /\ hb = 1)
Terminal == row = "archived" /\ storage = "absent"

RemoveGuards(archive, driftOk) ==
  /\ row = "active" /\ storage = "present"
  /\ ~JournalBlocked /\ Authorized
  /\ hold = "none" /\ handoff /= "unread" /\ occ = "none"
  /\ nested = 0 /\ OwnerOk
  /\ (driftOk \/ branch = "ok")
  /\ (archive \/ history = "contained")
  /\ (SlotType /= "validate" \/ pub \in {"proven", "abandoned"})
  /\ (SubjectVcs /= "eden" \/ "eden-remove" \in Fixes)

ArchiveStep ==
  /\ row' = "archived" /\ storage' = "absent" /\ journal' = "none"
  /\ viol' = IF viol /= "none" \/ storage = "absent" THEN viol
             ELSE IF truth = "alive" THEN "live-owner"
             ELSE IF hold /= "none" THEN "held"
             ELSE IF handoff = "unread" THEN "unread-handoff"
             ELSE IF occ /= "none" THEN "occupied"
             ELSE viol
  /\ UNCHANGED <<truth, proof, hb, handoff, hold, hage, history,
                 branch, nested, occ, pub, retries, coord>>

Init ==
  /\ row = "absent" /\ storage \in {"absent", "present"}
  /\ truth = "alive" /\ proof = "unbound" /\ hb = 0
  /\ handoff = "none" /\ hold = "none" /\ hage = 0
  /\ history = "contained" /\ branch = "ok" /\ nested = 0
  /\ occ = "none" /\ journal = "none"
  /\ pub = IF SlotType = "agent" THEN "proven" ELSE "unproven"
  /\ retries = 0 /\ coord = "orig" /\ viol = "none"

CreateBound ==
  /\ row = "absent" /\ storage = "absent" /\ journal = "none"
  /\ ~JournalBlocked
  /\ ~("eden-guard" \in Bugs /\ OtherEden /\ SubjectVcs = "git")
  /\ row' = "active" /\ storage' = "present" /\ proof' = "live"
  /\ occ' = "live"
  /\ UNCHANGED <<truth, hb, handoff, hold, hage, history, branch,
                 nested, journal, pub, retries, coord, viol>>

CreateUnbound ==
  /\ row = "absent" /\ storage = "absent" /\ journal = "none"
  /\ ~JournalBlocked
  /\ ~("eden-guard" \in Bugs /\ OtherEden /\ SubjectVcs = "git")
  /\ row' = "active" /\ storage' = "present" /\ proof' = "unbound"
  /\ occ' = "live"
  /\ UNCHANGED <<truth, hb, handoff, hold, hage, history, branch,
                 nested, journal, pub, retries, coord, viol>>

CreateCrash ==
  /\ row = "absent" /\ storage = "absent" /\ journal = "none"
  /\ storage' = "present" /\ journal' = "create"
  /\ UNCHANGED <<row, truth, proof, hb, handoff, hold, hage, history,
                 branch, nested, occ, pub, retries, coord, viol>>

RecoverCreate ==
  /\ journal = "create" /\ Authorized
  /\ row' = "active" /\ storage' = "present" /\ proof' = "live"
  /\ occ' = "live" /\ journal' = "none"
  /\ UNCHANGED <<truth, hb, handoff, hold, hage, history, branch,
                 nested, pub, retries, coord, viol>>

AbortCreate ==
  /\ journal = "create" /\ Authorized
  /\ row' = "absent" /\ storage' = "absent" /\ journal' = "none"
  /\ UNCHANGED <<truth, proof, hb, handoff, hold, hage, history,
                 branch, nested, occ, pub, retries, coord, viol>>

Tick ==
  /\ row = "active"
  /\ (hb = 0 \/ (hold = "held" /\ hage < 2))
  /\ hb' = 1
  /\ hage' = IF hold = "held" THEN (IF hage < 2 THEN hage + 1 ELSE 2)
             ELSE hage
  /\ UNCHANGED <<row, storage, truth, proof, handoff, hold, history,
                 branch, nested, occ, journal, pub, retries, coord, viol>>

CoordRestart ==
  /\ row = "active" /\ coord = "orig"
  /\ coord' = "restarted"
  /\ UNCHANGED <<row, storage, truth, proof, hb, handoff, hold, hage,
                 history, branch, nested, occ, journal, pub, retries, viol>>

OwnerExit ==
  /\ row = "active" /\ truth = "alive"
  /\ truth' = "dead"
  /\ occ' = IF occ = "live" THEN "none" ELSE occ
  /\ UNCHANGED <<row, storage, proof, hb, handoff, hold, hage, history,
                 branch, nested, journal, pub, retries, coord, viol>>

Heartbeat ==
  /\ row = "active" /\ truth = "alive" /\ hb = 1
  /\ hb' = 0
  /\ UNCHANGED <<row, storage, truth, proof, handoff, hold, hage,
                 history, branch, nested, occ, journal, pub, retries,
                 coord, viol>>

WriteHandoff ==
  /\ row = "active" /\ truth = "alive" /\ handoff = "none"
  /\ handoff' = "unread"
  /\ UNCHANGED <<row, storage, truth, proof, hb, hold, hage, history,
                 branch, nested, occ, journal, pub, retries, coord, viol>>

ReadHandoff ==
  /\ row = "active" /\ handoff = "unread" /\ Authorized /\ ~JournalBlocked
  /\ handoff' = "enqueued"
  /\ UNCHANGED <<row, storage, truth, proof, hb, hold, hage, history,
                 branch, nested, occ, journal, pub, retries, coord, viol>>

HoldStep ==
  /\ row = "active" /\ hold = "none"
  /\ hold' = "held" /\ hage' = 0
  /\ UNCHANGED <<row, storage, truth, proof, hb, handoff, history,
                 branch, nested, occ, journal, pub, retries, coord, viol>>

Unhold ==
  /\ row = "active" /\ hold = "held"
  /\ hold' = "none" /\ hage' = 0
  /\ UNCHANGED <<row, storage, truth, proof, hb, handoff, history,
                 branch, nested, occ, journal, pub, retries, coord, viol>>

HoldExpire ==
  /\ row = "active" /\ hold = "held" /\ hage = 2
  /\ "hold-expiry" \in Fixes
  /\ hold' = "none" /\ hage' = 0
  /\ UNCHANGED <<row, storage, truth, proof, hb, handoff, history,
                 branch, nested, occ, journal, pub, retries, coord, viol>>

ProbeDead ==
  /\ row = "active" /\ truth = "dead" /\ proof \in {"live", "indet"}
  /\ proof' = "dead"
  /\ UNCHANGED <<row, storage, truth, hb, handoff, hold, hage, history,
                 branch, nested, occ, journal, pub, retries, coord, viol>>

ProbeIndet ==
  /\ row = "active" /\ truth = "dead" /\ proof = "live"
  /\ proof' = "indet"
  /\ UNCHANGED <<row, storage, truth, hb, handoff, hold, hage, history,
                 branch, nested, occ, journal, pub, retries, coord, viol>>

RecoverUnbound ==
  /\ row = "active" /\ proof = "unbound" /\ truth = "dead" /\ Authorized
  /\ proof' = "dead"
  /\ UNCHANGED <<row, storage, truth, hb, handoff, hold, hage, history,
                 branch, nested, occ, journal, pub, retries, coord, viol>>

Adopt ==
  /\ row = "active" /\ truth = "alive" /\ proof = "unbound"
  /\ proof' = "live"
  /\ UNCHANGED <<row, storage, truth, hb, handoff, hold, hage, history,
                 branch, nested, occ, journal, pub, retries, coord, viol>>

AdoptCheckout ==
  /\ row = "active" /\ nested = 1 /\ Authorized /\ ~JournalBlocked
  /\ nested' = 0
  /\ UNCHANGED <<row, storage, truth, proof, hb, handoff, hold, hage,
                 history, branch, occ, journal, pub, retries, coord, viol>>

ReregisterBranch ==
  /\ row = "active" /\ branch = "drift" /\ proof = "dead" /\ Authorized
  /\ "reregister-branch" \in Fixes
  /\ branch' = "ok"
  /\ UNCHANGED <<row, storage, truth, proof, hb, handoff, hold, hage,
                 history, nested, occ, journal, pub, retries, coord, viol>>

BranchDrift ==
  /\ row = "active" /\ truth = "alive" /\ branch = "ok"
  /\ branch' = "drift"
  /\ UNCHANGED <<row, storage, truth, proof, hb, handoff, hold, hage,
                 history, nested, occ, journal, pub, retries, coord, viol>>

NestedAdd ==
  /\ row = "active" /\ truth = "alive" /\ nested = 0
  /\ nested' = 1
  /\ UNCHANGED <<row, storage, truth, proof, hb, handoff, hold, hage,
                 history, branch, occ, journal, pub, retries, coord, viol>>

MakePrivate(v) ==
  /\ row = "active" /\ truth = "alive" /\ history = "contained"
  /\ history' = v
  /\ UNCHANGED <<row, storage, truth, proof, hb, handoff, hold, hage,
                 branch, nested, occ, journal, pub, retries, coord, viol>>

SpawnLeftover ==
  /\ row = "active" /\ truth = "dead" /\ occ = "none"
  /\ occ' = "deleted"
  /\ UNCHANGED <<row, storage, truth, proof, hb, handoff, hold, hage,
                 history, branch, nested, journal, pub, retries,
                 coord, viol>>

LeftoverExit ==
  /\ row = "active" /\ occ = "deleted"
  /\ occ' = "none"
  /\ UNCHANGED <<row, storage, truth, proof, hb, handoff, hold, hage,
                 history, branch, nested, journal, pub, retries,
                 coord, viol>>

ExtDirDelete ==
  /\ row = "active" /\ storage = "present" /\ journal = "none"
  /\ storage' = "absent"
  /\ UNCHANGED <<row, truth, proof, hb, handoff, hold, hage, history,
                 branch, nested, occ, journal, pub, retries, coord, viol>>

Recover ==
  /\ row = "active" /\ journal = "lifecycle" /\ Authorized
  /\ journal' = "none"
  /\ UNCHANGED <<row, storage, truth, proof, hb, handoff, hold, hage,
                 history, branch, nested, occ, pub, retries, coord, viol>>

LedgerPublishOk ==
  /\ row = "active" /\ SlotType = "validate" /\ ~MirrorFails
  /\ pub \in {"unproven", "mirror"}
  /\ pub' = "proven"
  /\ UNCHANGED <<row, storage, truth, proof, hb, handoff, hold, hage,
                 history, branch, nested, occ, journal, retries,
                 coord, viol>>

LedgerPublishFail ==
  /\ row = "active" /\ SlotType = "validate"
  /\ pub \in {"unproven", "mirror"} /\ retries < 2
  /\ pub' = "mirror" /\ retries' = retries + 1
  /\ UNCHANGED <<row, storage, truth, proof, hb, handoff, hold, hage,
                 history, branch, nested, occ, journal, coord, viol>>

LedgerAbandon ==
  /\ row = "active" /\ SlotType = "validate"
  /\ pub = "mirror" /\ retries = 2 /\ "ledger-abandon" \in Fixes
  /\ pub' = "abandoned"
  /\ UNCHANGED <<row, storage, truth, proof, hb, handoff, hold, hage,
                 history, branch, nested, occ, journal, retries,
                 coord, viol>>

Remove == RemoveGuards(FALSE, FALSE) /\ ArchiveStep
RemoveArchive == RemoveGuards(TRUE, FALSE) /\ ArchiveStep
RemoveDrift == RemoveGuards(TRUE, TRUE) /\ ArchiveStep

RemoveCrash ==
  /\ RemoveGuards(FALSE, FALSE)
  /\ journal' = "lifecycle"
  /\ UNCHANGED <<row, storage, truth, proof, hb, handoff, hold, hage,
                 history, branch, nested, occ, pub, retries, coord, viol>>

RecoverAbsent ==
  /\ row = "active" /\ storage = "absent" /\ journal = "none"
  /\ Authorized /\ hold = "none" /\ handoff /= "unread" /\ OwnerOk
  /\ ~("eden-guard" \in Bugs /\ SubjectVcs = "eden")
  /\ "apply-journal" \notin Bugs
  /\ ArchiveStep

RecoverAbsentRefusedApply ==
  /\ row = "active" /\ storage = "absent" /\ journal = "none"
  /\ Authorized /\ hold = "none" /\ handoff /= "unread" /\ OwnerOk
  /\ ~("eden-guard" \in Bugs /\ SubjectVcs = "eden")
  /\ "apply-journal" \in Bugs
  /\ journal' = "lifecycle"
  /\ UNCHANGED <<row, storage, truth, proof, hb, handoff, hold, hage,
                 history, branch, nested, occ, pub, retries, coord, viol>>

TtlTeardown ==
  /\ row = "active" /\ storage = "present" /\ journal = "none" /\ hb = 1
  /\ (("ttl-unfixed" \in Bugs)
      \/ (hold = "none" /\ proof = "dead" /\ handoff /= "unread"
          /\ occ = "none"))
  /\ ArchiveStep

Next ==
  \/ CreateBound \/ CreateUnbound \/ CreateCrash \/ RecoverCreate
  \/ AbortCreate \/ Tick \/ CoordRestart \/ OwnerExit \/ Heartbeat
  \/ WriteHandoff \/ ReadHandoff \/ HoldStep \/ Unhold \/ HoldExpire
  \/ ProbeDead \/ ProbeIndet \/ RecoverUnbound \/ Adopt \/ AdoptCheckout
  \/ ReregisterBranch \/ BranchDrift \/ NestedAdd
  \/ MakePrivate("visible") \/ MakePrivate("hidden")
  \/ SpawnLeftover \/ LeftoverExit \/ ExtDirDelete \/ Recover
  \/ LedgerPublishOk \/ LedgerPublishFail \/ LedgerAbandon
  \/ Remove \/ RemoveArchive \/ RemoveDrift \/ RemoveCrash
  \/ RecoverAbsent \/ RecoverAbsentRefusedApply \/ TtlTeardown
  \/ (Terminal /\ UNCHANGED vars)
  \* The stray-storage start (row absent, storage present) has no
  \* enabled action and is not terminal; stutter there so TLC's
  \* deadlock check treats it as the leaf the Python explorer
  \* already treats it as (#2043).
  \/ (row = "absent" /\ storage = "present" /\ UNCHANGED vars)

\* The finisher policy actions: the same transitions, taken as scheduled
\* work.  Fairness is asserted for them (and Tick) only.
Schedule ==
  /\ WF_vars(Tick)
  /\ (Finisher =>
        /\ WF_vars(Remove) /\ WF_vars(RemoveArchive)
        /\ WF_vars(RecoverAbsent) /\ WF_vars(Recover)
        /\ WF_vars(RecoverCreate) /\ WF_vars(AdoptCheckout)
        /\ WF_vars(LedgerPublishOk) /\ WF_vars(LedgerPublishFail))
  /\ ("auto-recover-unbound" \in Fixes =>
        (WF_vars(ProbeDead) /\ WF_vars(RecoverUnbound)))
  /\ ("hold-expiry" \in Fixes => WF_vars(HoldExpire))
  /\ ("ledger-abandon" \in Fixes => WF_vars(LedgerAbandon))
  /\ ("reregister-branch" \in Fixes => WF_vars(ReregisterBranch))

Spec == Init /\ [][Next]_vars /\ Schedule

Safety == viol = "none"

InDomain == row = "active" /\ truth = "dead" /\ occ = "none"
              /\ handoff /= "unread"

Liveness == InDomain ~> Terminal

THEOREM Spec => []Safety   \* modulo the TTL occupancy/handoff gap:
                           \* with TtlTeardown enabled, TLC finds the
                           \* occupied/unread counterexamples the
                           \* Python explorer asserts as a known gap.
=============================================================================
