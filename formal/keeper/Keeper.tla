------------------------------- MODULE Keeper -------------------------------
(***************************************************************************)
(* Alaya, milestone 4: a model of the driver keeper's crash-and-resend    *)
(* rule (decisions 70 and 79) and of live updates (decision 80).          *)
(*                                                                         *)
(* SPDX-License-Identifier: GPL-3.0-only                                   *)
(*                                                                         *)
(* What is modelled                                                        *)
(*                                                                         *)
(* - Apps: one per sector, each with one request at a time: write the     *)
(*   next value, or read the sector back (as the disk test app does).     *)
(* - The keeper: takes a request into a free job slot (a number "posted"  *)
(*   it raises for every new job), and answers the app once the driver    *)
(*   reports the job done ("done" = "posted"). The job slots and the data *)
(*   buffers are the keeper's memory, so they outlive the driver.         *)
(* - The driver: sends each waiting job to the device once ("submitted"   *)
(*   is its own memory) and, when the device finishes a slot, marks it    *)
(*   done with the number it submitted.                                    *)
(* - The device: works on the jobs it was given in any order; a write     *)
(*   copies the slot's buffer to the disk, a read copies the disk into    *)
(*   the buffer; then it reports the slot finished.                       *)
(* - At any moment the driver may crash (bounded, so progress is still    *)
(*   possible). The keeper starts a new one, which resets the device      *)
(*   (dropping what it was working on) and sends every unfinished job     *)
(*   again. A live update hands the device to a new version with the      *)
(*   driver's state and no reset; the rollback after a failed trial is a  *)
(*   crash followed by a restart of the old version (reset and resend).   *)
(* - Every step is atomic.                                                 *)
(*                                                                         *)
(* What TLC checks                                                         *)
(*                                                                         *)
(* - Apps never see a wrong answer: a read returns the last value the app *)
(*   was told was written.                                                 *)
(* - The disk agrees: a sector with no request in flight holds the last   *)
(*   value whose write was answered.                                       *)
(* - Every request is answered (with fair steps and finitely many         *)
(*   crashes).                                                             *)
(*                                                                         *)
(* And that the model has teeth: a restart that does not reset the device *)
(* lets an old, duplicated job finish a newer one (wrong answer), and a   *)
(* keeper that drops unfinished jobs at a crash leaves apps waiting.      *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    Sectors,    \* one app per sector
    Slots,      \* the keeper's job slots
    MaxV,       \* each app writes the values 1..MaxV
    MaxOps,     \* requests per app
    MaxCrashes, \* crashes (and updates) the driver may suffer
    None,
    Bug         \* "none", "no_reset" or "forget_jobs"

ASSUME Bug \in {"none", "no_reset", "forget_jobs"}

VARIABLES
    req,        \* sector -> [kind, val] of its app's request, or None
    ops,        \* sector -> requests made so far
    answered,   \* sector -> last value the app was told was written
    wrong,      \* an app got a wrong answer
    disk,       \* sector -> value on the disk
    slot,       \* job slot -> sector of the job in it, or None
    kind,       \* job slot -> "r" or "w"
    buf,        \* job slot -> value in its data buffer
    posted,     \* job slot -> number of the job posted in it
    done,       \* job slot -> number of the job the driver reported done
    seq,        \* the keeper's job counter
    up,         \* a driver runs
    submitted,  \* job slot -> number the driver sent it with (its memory)
    inflight,   \* set of [slot, sector, kind, gen] the device works on
    finished,   \* job slots the device reported finished, not yet seen
    crashes,
    version,
    held        \* apps whose request the keeper took (it holds their reply)

vars == <<req, ops, answered, wrong, disk, slot, kind, buf, posted, done,
          seq, up, submitted, inflight, finished, crashes, version, held>>

Values == 0..MaxV
Requests == [kind : {"r", "w"}, val : Values]
\* A job the device holds, and which driver instance sent it.
Jobs == [slot : Slots, sector : Sectors, kind : {"r", "w"}, gen : 0..MaxCrashes]

TypeOK ==
    /\ req \in [Sectors -> Requests \cup {None}]
    /\ answered \in [Sectors -> Values]
    /\ disk \in [Sectors -> Values]
    /\ slot \in [Slots -> Sectors \cup {None}]
    /\ buf \in [Slots -> Values]
    /\ up \in BOOLEAN
    /\ inflight \subseteq Jobs
    /\ finished \subseteq Slots

Init ==
    /\ req = [s \in Sectors |-> None]
    /\ ops = [s \in Sectors |-> 0]
    /\ answered = [s \in Sectors |-> 0]
    /\ wrong = FALSE
    /\ disk = [s \in Sectors |-> 0]
    /\ slot = [j \in Slots |-> None]
    /\ kind = [j \in Slots |-> "r"]
    /\ buf = [j \in Slots |-> 0]
    /\ posted = [j \in Slots |-> 0]
    /\ done = [j \in Slots |-> 0]
    /\ seq = 0
    /\ up = TRUE
    /\ submitted = [j \in Slots |-> 0]
    /\ inflight = {}
    /\ finished = {}
    /\ crashes = 0
    /\ version = 1
    /\ held = {}

-----------------------------------------------------------------------------
(* Apps *)

Issue(s) ==
    /\ req[s] = None
    /\ ops[s] < MaxOps
    /\ \/ /\ answered[s] < MaxV
          /\ req' = [req EXCEPT ![s] = [kind |-> "w", val |-> answered[s] + 1]]
       \/ req' = [req EXCEPT ![s] = [kind |-> "r", val |-> 0]]
    /\ ops' = [ops EXCEPT ![s] = @ + 1]
    /\ UNCHANGED <<answered, wrong, disk, slot, kind, buf, posted, done, seq,
                   up, submitted, inflight, finished, crashes, version, held>>

-----------------------------------------------------------------------------
(* The keeper *)

Accept(s, j) ==
    /\ req[s] # None
    /\ s \notin held
    /\ slot[j] = None
    /\ held' = held \cup {s}
    /\ slot' = [slot EXCEPT ![j] = s]
    /\ kind' = [kind EXCEPT ![j] = req[s].kind]
    /\ buf' = IF req[s].kind = "w" THEN [buf EXCEPT ![j] = req[s].val] ELSE buf
    /\ seq' = seq + 1
    /\ posted' = [posted EXCEPT ![j] = seq + 1]
    /\ UNCHANGED <<req, ops, answered, wrong, disk, done, up, submitted,
                   inflight, finished, crashes, version>>

Answer(j) ==
    /\ slot[j] # None
    /\ posted[j] = done[j]
    /\ LET s == slot[j] IN
       /\ IF kind[j] = "r"
          THEN /\ wrong' = (wrong \/ buf[j] # answered[s])
               /\ UNCHANGED answered
          ELSE /\ answered' = [answered EXCEPT ![s] = buf[j]]
               /\ UNCHANGED wrong
       /\ req' = [req EXCEPT ![s] = None]
       /\ held' = held \ {s}
    /\ slot' = [slot EXCEPT ![j] = None]
    /\ UNCHANGED <<ops, disk, kind, buf, posted, done, seq, up, submitted,
                   inflight, finished, crashes, version>>

-----------------------------------------------------------------------------
(* The driver *)

Submit(j) ==
    /\ up
    /\ slot[j] # None
    /\ posted[j] # done[j]
    /\ submitted[j] # posted[j]
    /\ submitted' = [submitted EXCEPT ![j] = posted[j]]
    /\ inflight' = inflight \cup {[slot |-> j, sector |-> slot[j], kind |-> kind[j], gen |-> crashes]}
    /\ UNCHANGED <<req, ops, answered, wrong, disk, slot, kind, buf, posted,
                   done, seq, up, finished, crashes, version, held>>

Collect(j) ==
    /\ up
    /\ j \in finished
    /\ done' = [done EXCEPT ![j] = submitted[j]]
    /\ finished' = finished \ {j}
    /\ UNCHANGED <<req, ops, answered, wrong, disk, slot, kind, buf, posted,
                   seq, up, submitted, inflight, crashes, version, held>>

Crash ==
    /\ up
    /\ crashes < MaxCrashes
    /\ up' = FALSE
    /\ crashes' = crashes + 1
    \* A keeper with the bug forgets the unfinished jobs, without answering
    \* their apps.
    /\ IF Bug = "forget_jobs"
       THEN /\ slot' = [j \in Slots |-> IF posted[j] # done[j] THEN None ELSE slot[j]]
            /\ done' = posted
       ELSE UNCHANGED <<slot, done>>
    /\ UNCHANGED <<req, ops, answered, wrong, disk, kind, buf, posted, seq,
                   submitted, inflight, finished, version, held>>

\* The keeper starts a new driver (or, after a failed trial, the old
\* version again): it resets the device and has sent nothing yet.
Restart ==
    /\ ~up
    /\ up' = TRUE
    /\ submitted' = [j \in Slots |-> 0]
    /\ IF Bug = "no_reset"
       THEN UNCHANGED <<inflight, finished>>
       ELSE /\ inflight' = {}
            /\ finished' = {}
    /\ UNCHANGED <<req, ops, answered, wrong, disk, slot, kind, buf, posted,
                   done, seq, crashes, version, held>>

\* A live update: the new version takes the driver's state over, and the
\* device keeps working.
Update ==
    /\ up
    /\ crashes < MaxCrashes
    /\ crashes' = crashes + 1
    /\ version' = 3 - version
    /\ UNCHANGED <<req, ops, answered, wrong, disk, slot, kind, buf, posted,
                   done, seq, up, submitted, inflight, finished, held>>

-----------------------------------------------------------------------------
(* The device *)

Work(x) ==
    /\ x \in inflight
    /\ IF x.kind = "w"
       THEN /\ disk' = [disk EXCEPT ![x.sector] = buf[x.slot]]
            /\ UNCHANGED buf
       ELSE /\ buf' = [buf EXCEPT ![x.slot] = disk[x.sector]]
            /\ UNCHANGED disk
    /\ inflight' = inflight \ {x}
    /\ finished' = finished \cup {x.slot}
    /\ UNCHANGED <<req, ops, answered, wrong, slot, kind, posted, done, seq,
                   up, submitted, crashes, version, held>>

-----------------------------------------------------------------------------

Next ==
    \/ \E s \in Sectors : Issue(s)
    \/ \E s \in Sectors, j \in Slots : Accept(s, j)
    \/ \E j \in Slots : Answer(j) \/ Submit(j) \/ Collect(j)
    \/ \E x \in inflight : Work(x)
    \/ Crash \/ Restart \/ Update

Fairness ==
    /\ \A s \in Sectors, j \in Slots : WF_vars(Accept(s, j))
    /\ \A j \in Slots : WF_vars(Answer(j)) /\ WF_vars(Submit(j)) /\ WF_vars(Collect(j))
    /\ \A x \in Jobs : WF_vars(Work(x))
    /\ WF_vars(Restart)

Spec == Init /\ [][Next]_vars /\ Fairness

-----------------------------------------------------------------------------
(* Properties *)

NoWrongAnswers == ~wrong

\* A sector nobody is working on holds what its app was told.
DiskAgrees ==
    \A s \in Sectors :
        (req[s] = None /\ ~\E x \in inflight : x.sector = s) => disk[s] = answered[s]

\* Every request is answered.
RequestsAnswered == \A s \in Sectors : req[s] # None ~> req[s] = None

=============================================================================
