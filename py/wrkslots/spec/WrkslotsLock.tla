-------------------------- MODULE WrkslotsLock --------------------------
(***************************************************************************)
(* Registry lock liveness (#2090).                                        *)
(*                                                                         *)
(* One flock, three clients.  Hog repeats a refusable recover-absent       *)
(* call unboundedly; Heartbeat and Create each want the lock once.  Lock   *)
(* acquisition, hold and release are separate steps, and the flock is      *)
(* non-FIFO: any waiter may win a free lock, so weak fairness on the       *)
(* acquire actions does not by itself save a waiter from a hog that        *)
(* releases and immediately re-acquires.                                   *)
(*                                                                         *)
(* Shape (constant):                                                       *)
(* * "pre-2088": the hog acquires, performs the expensive full-history     *)
(*   load while holding, only then refuses and releases.  Heartbeat and    *)
(*   Create starve: HogCycle repeats forever and neither ever holds.       *)
(* * "post-2088": the cheap refusal checks run before the lock from a      *)
(*   proven snapshot, so the hog refuses lock-free and never contends.     *)
(*   The lock stays free, weak fairness fires the waiters' acquires,       *)
(*   and liveness holds.                                                   *)
(*                                                                         *)
(* Checked: Safety (at most one holder) in both shapes; Liveness           *)
(* (HeartbeatWants \/ CreateWants) ~> (HeartbeatHolds \/ CreateHolds)      *)
(* fails pre-#2088 with the HogCycle counterexample and passes post.        *)
(***************************************************************************)
EXTENDS Integers, TLC

CONSTANT Shape  \* "pre-2088" | "post-2088"

VARIABLES holder, hogPC, hbPC, crPC

vars == <<holder, hogPC, hbPC, crPC>>

Init ==
  /\ holder = "none"
  /\ hogPC = "away" /\ hbPC = "away" /\ crPC = "away"

(* --- the repeating refusable client (recover-absent) --- *)
HogWant ==
  /\ hogPC = "away"
  /\ IF Shape = "post-2088"
     THEN hogPC' = "refused"  \* cheap checks refuse before the lock
     ELSE hogPC' = "want"
  /\ UNCHANGED <<holder, hbPC, crPC>>

HogRefusedAgain ==
  /\ hogPC = "refused"
  /\ hogPC' = "away"
  /\ UNCHANGED <<holder, hbPC, crPC>>

HogAcquire ==
  /\ hogPC = "want" /\ holder = "none"
  /\ holder' = "hog" /\ hogPC' = "load"
  /\ UNCHANGED <<hbPC, crPC>>

HogLoad ==
  /\ hogPC = "load"
  /\ hogPC' = "refuse"
  /\ UNCHANGED <<holder, hbPC, crPC>>

HogRelease ==
  /\ hogPC = "refuse"
  /\ holder' = "none" /\ hogPC' = "away"
  /\ UNCHANGED <<hbPC, crPC>>

(* --- heartbeat and create: want once, hold, release --- *)
HbWant ==
  /\ hbPC = "away" /\ hbPC' = "want"
  /\ UNCHANGED <<holder, hogPC, crPC>>

HbAcquire ==
  /\ hbPC = "want" /\ holder = "none"
  /\ holder' = "hb" /\ hbPC' = "hold"
  /\ UNCHANGED <<hogPC, crPC>>

HbRelease ==
  /\ hbPC = "hold"
  /\ holder' = "none" /\ hbPC' = "done"
  /\ UNCHANGED <<hogPC, crPC>>

CrWant ==
  /\ crPC = "away" /\ crPC' = "want"
  /\ UNCHANGED <<holder, hogPC, hbPC>>

CrAcquire ==
  /\ crPC = "want" /\ holder = "none"
  /\ holder' = "cr" /\ crPC' = "hold"
  /\ UNCHANGED <<hogPC, hbPC>>

CrRelease ==
  /\ crPC = "hold"
  /\ holder' = "none" /\ crPC' = "done"
  /\ UNCHANGED <<hogPC, hbPC>>

Next ==
  \/ HogWant \/ HogRefusedAgain \/ HogAcquire \/ HogLoad \/ HogRelease
  \/ HbWant \/ HbAcquire \/ HbRelease
  \/ CrWant \/ CrAcquire \/ CrRelease
  \/ (hbPC = "done" /\ crPC = "done" /\ UNCHANGED vars)

Fairness ==
  /\ WF_vars(HbAcquire) /\ WF_vars(CrAcquire)
  /\ WF_vars(HogAcquire) /\ WF_vars(HogRelease)
  /\ WF_vars(HbRelease) /\ WF_vars(CrRelease)

Spec == Init /\ [][Next]_vars /\ Fairness

Safety == holder \in {"none", "hog", "hb", "cr"}

Wants == hbPC = "want" \/ crPC = "want"
Holds == hbPC = "hold" \/ crPC = "hold"
Liveness == Wants ~> Holds

THEOREM Spec => []Safety
=============================================================================
