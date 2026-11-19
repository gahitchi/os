------------------------------- MODULE Revoke -------------------------------
(***************************************************************************)
(* Alaya, milestone 2: a model of capabilities, untyped memory, retype and *)
(* interruptible revoke on several cores.                                  *)
(*                                                                         *)
(* SPDX-License-Identifier: GPL-3.0-only                                   *)
(*                                                                         *)
(* What is modelled                                                        *)
(*                                                                         *)
(* - Memory is a few cells. The boot untyped covers all of them. Retype    *)
(*   turns a free cell into a table, an endpoint or a smaller untyped      *)
(*   (one cell). A smaller untyped can retype its cell once more, one      *)
(*   level down (lvl 1).                                                   *)
(* - A table here has exactly one slot, named after its cell. The root     *)
(*   table (RootSlots) is the first program's own table, made at boot.     *)
(* - Every capability has a parent in the derivation tree: retype makes    *)
(*   children of the untyped, copy makes children of the source.           *)
(* - Decision 51: deleting a capability never frees an object. Memory      *)
(*   comes back only when an untyped is revoked; at the end of that revoke *)
(*   the untyped is reset and its cells can be retyped again.              *)
(* - All kernel work runs under the big kernel lock, which hands turns to  *)
(*   the cores in order (it is a ticket lock). In its turn a core does one *)
(*   operation, or K steps of a revoke. A revoke step deletes one          *)
(*   capability. Between turns the revoke is interrupted: in the kernel it *)
(*   returns to user mode and the call is restarted.                       *)
(* - Every core may act on every slot it can reach, in any order. That is  *)
(*   the hostile case: other programs copy, move and delete while a revoke *)
(*   runs.                                                                 *)
(*                                                                         *)
(* The rules being checked                                                 *)
(*                                                                         *)
(* - A revoke marks its capability "revoking": nothing may be copied or    *)
(*   retyped from it, and it cannot be moved or deleted, until it ends.    *)
(* - Deleting the last capability to a table that still holds something    *)
(*   puts the table on one global purge list ("Orphans"). A user delete of *)
(*   such a capability is refused; only a revoke does it.                  *)
(* - A revoke of an untyped first deletes every descendant, then empties   *)
(*   every table on the purge list, and only then resets the untyped.      *)
(* - A revoke needs at least K steps per turn; K must be larger than the   *)
(*   number of other cores, each of which can add one capability per turn. *)
(*                                                                         *)
(* What TLC checks                                                         *)
(*                                                                         *)
(* - Safety: no capability ever names memory that was reset, no slot ever  *)
(*   lives in memory that was reset, retype never hands out memory that is *)
(*   in use, and a new table is always empty.                              *)
(* - Liveness: every revoke that starts ends.                              *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    Cores,      \* the cores, as numbers 1..N
    RootSlots,  \* slots of the first program's table
    BootSlot,   \* the root slot holding the boot untyped
    Cells,      \* memory cells (numbers, 0 is not a cell)
    K,          \* revoke steps per turn
    None        \* a model value: no capability, no parent, no revoke

ASSUME BootSlot \in RootSlots /\ 0 \notin Cells /\ K \in Nat \ {0}

Slots == RootSlots \cup Cells
Levels == {0, 1}
Kinds == {"ut", "table", "ep"}

\* The boot untyped. Other capabilities name their object by cell and level.
BootUt == [kind |-> "ut", cell |-> 0, lvl |-> 0]

VARIABLES
    cap,       \* slot -> capability or None
    parent,    \* slot -> slot or None: the derivation tree
    revoking,  \* slot -> BOOLEAN
    rv,        \* core -> slot it is revoking, or None
    orphans,   \* cells of unreachable tables still waiting to be emptied
    utFree,    \* untyped id (0 = boot, else its cell) -> its free cells
    objAt,     \* cell -> level -> "free" or the kind of object there
    turn,      \* the core holding the big kernel lock
    spent,     \* revoke steps taken in this turn
    bad        \* set when an operation broke a safety rule

vars == <<cap, parent, revoking, rv, orphans, utFree, objAt, turn, spent, bad>>

---------------------------------------------------------------------------
\* Helpers

Live(s) == cap[s] # None

SameObject(a, b) == a.kind = b.kind /\ a.cell = b.cell /\ a.lvl = b.lvl

\* A cell's slot exists while a table object lives in that cell.
TableAt(x) == \E l \in Levels : objAt[x][l] = "table"

\* A program can name a slot through its root table, or through a
\* capability to the table that holds it.
Reachable(s) ==
    \/ s \in RootSlots
    \/ \E t \in Slots : Live(t) /\ cap[t].kind = "table" /\ cap[t].cell = s

\* The last capability to its object.
Final(s) == ~\E t \in Slots \ {s} : Live(t) /\ SameObject(cap[t], cap[s])

Children(c) == {s \in Slots : Live(s) /\ parent[s] = c}

RECURSIVE Closure(_, _)
Closure(S, n) ==
    IF n = 0 THEN S
    ELSE Closure(S \cup {s \in Slots : Live(s) /\ parent[s] \in S}, n - 1)

Descendants(c) == Closure({c}, Cardinality(Slots)) \ {c}

\* Untyped id of an untyped capability: 0 for the boot one, else its cell.
UtId(c) == c.cell

\* Does object o lie in the memory of untyped u?
Inside(o, u) ==
    IF u.cell = 0 THEN o.cell \in Cells
    ELSE o.cell = u.cell /\ o.lvl = 1

NextTurn == IF turn = Cardinality(Cores) THEN 1 ELSE turn + 1

\* Delete the capability in slot s. Its children move up to its parent.
\* Revokes running on s end. A table left unreachable but not empty goes
\* on the purge list.
DeleteSlot(s) ==
    LET c == cap[s]
        orphaned == c.kind = "table" /\ Final(s) /\ c.cell # s /\ Live(c.cell)
    IN
    /\ cap' = [cap EXCEPT ![s] = None]
    /\ parent' = [t \in Slots |->
                    IF t = s THEN None
                    ELSE IF parent[t] = s THEN parent[s] ELSE parent[t]]
    /\ revoking' = [revoking EXCEPT ![s] = FALSE]
    /\ orphans' = IF orphaned THEN orphans \cup {c.cell} ELSE orphans

---------------------------------------------------------------------------
\* Operations a core can do in its turn (when it is not revoking).

Retype(u, d, k) ==
    /\ Reachable(u) /\ Live(u) /\ cap[u].kind = "ut" /\ ~revoking[u]
    /\ Reachable(d) /\ ~Live(d) /\ d # u
    /\ utFree[UtId(cap[u])] # {}
    /\ k \in (IF UtId(cap[u]) = 0 THEN Kinds ELSE Kinds \ {"ut"})
    /\ LET id == UtId(cap[u])
           x == CHOOSE x \in utFree[id] : TRUE
           l == IF id = 0 THEN 0 ELSE 1
       IN
       /\ bad' = (bad \/ objAt[x][l] # "free"
                      \/ (k = "table" /\ Live(x)))
       /\ objAt' = [objAt EXCEPT ![x][l] = k]
       /\ cap' = [cap EXCEPT ![d] = [kind |-> k, cell |-> x, lvl |-> l]]
       /\ parent' = [parent EXCEPT ![d] = u]
       /\ utFree' = [utFree EXCEPT ![id] = @ \ {x},
                                   ![x] = IF k = "ut" THEN {x} ELSE @]
    /\ UNCHANGED <<revoking, rv, orphans>>

Copy(s, d) ==
    /\ Reachable(s) /\ Live(s) /\ cap[s].kind # "ut" /\ ~revoking[s]
    /\ Reachable(d) /\ ~Live(d)
    /\ cap' = [cap EXCEPT ![d] = cap[s]]
    /\ parent' = [parent EXCEPT ![d] = s]
    /\ UNCHANGED <<revoking, rv, orphans, utFree, objAt, bad>>

Move(s, d) ==
    /\ Reachable(s) /\ Live(s) /\ ~revoking[s]
    /\ Reachable(d) /\ ~Live(d)
    /\ cap' = [cap EXCEPT ![d] = cap[s], ![s] = None]
    /\ parent' = [t \in Slots |->
                    IF t = d THEN parent[s]
                    ELSE IF t = s THEN None
                    ELSE IF parent[t] = s THEN d ELSE parent[t]]
    /\ UNCHANGED <<revoking, rv, orphans, utFree, objAt, bad>>

UserDelete(s) ==
    /\ Reachable(s) /\ Live(s) /\ ~revoking[s]
    \* Refused: it would orphan a table that still holds something.
    /\ ~(cap[s].kind = "table" /\ Final(s) /\ cap[s].cell # s /\ Live(cap[s].cell))
    /\ DeleteSlot(s)
    /\ UNCHANGED <<rv, utFree, objAt, bad>>

RevokeStart(p, c) ==
    /\ rv[p] = None
    /\ Reachable(c) /\ Live(c) /\ ~revoking[c]
    /\ revoking' = [revoking EXCEPT ![c] = TRUE]
    /\ rv' = [rv EXCEPT ![p] = c]
    /\ UNCHANGED <<cap, parent, orphans, utFree, objAt, bad>>

---------------------------------------------------------------------------
\* One step of a revoke.

\* Revokes whose capability is deleted end with it.
EndRevokesOn(s) == rv' = [q \in Cores |-> IF rv[q] = s THEN None ELSE rv[q]]

ResetUntyped(c) ==
    LET u == cap[c]
        StillNamed == \E t \in Slots \ {c} : Live(t) /\ Inside(cap[t], u)
        TableIn(x, l) == /\ objAt[x][l] = "table"
                         /\ Inside([kind |-> "table", cell |-> x, lvl |-> l], u)
        SlotInside == \E x \in Cells : Live(x) /\ \E l \in Levels : TableIn(x, l)
    IN
    /\ bad' = (bad \/ StillNamed \/ SlotInside)
    /\ IF u.cell = 0
         THEN /\ objAt' = [x \in Cells |-> [l \in Levels |-> "free"]]
              /\ utFree' = [utFree EXCEPT ![0] = Cells]
         ELSE /\ objAt' = [objAt EXCEPT ![u.cell][1] = "free"]
              /\ utFree' = [utFree EXCEPT ![u.cell] = {u.cell}]

RevokeStep(p) ==
    LET c == rv[p] IN
    \/ \* The capability can no longer be named: the call cannot be restarted.
       /\ ~Reachable(c)
       /\ rv' = [rv EXCEPT ![p] = None]
       /\ UNCHANGED <<cap, parent, revoking, orphans, utFree, objAt, bad>>
    \/ \* Delete a descendant.
       /\ Reachable(c) /\ Children(c) # {}
       /\ LET s == CHOOSE s \in Children(c) : TRUE IN
            /\ DeleteSlot(s)
            /\ EndRevokesOn(s)
       /\ UNCHANGED <<utFree, objAt, bad>>
    \/ \* Untyped: empty a table on the purge list.
       /\ Reachable(c) /\ Children(c) = {} /\ cap[c].kind = "ut"
       /\ orphans # {}
       /\ LET x == CHOOSE x \in orphans : TRUE IN
            IF Live(x)
              THEN /\ DeleteSlot(x)
                   /\ EndRevokesOn(x)
              ELSE /\ orphans' = orphans \ {x}
                   /\ UNCHANGED <<cap, parent, revoking, rv>>
       /\ UNCHANGED <<utFree, objAt, bad>>
    \/ \* Done.
       /\ Reachable(c) /\ Children(c) = {}
       /\ (cap[c].kind # "ut" \/ orphans = {})
       /\ IF cap[c].kind = "ut"
            THEN ResetUntyped(c)
            ELSE UNCHANGED <<utFree, objAt, bad>>
       /\ revoking' = [revoking EXCEPT ![c] = FALSE]
       /\ rv' = [rv EXCEPT ![p] = None]
       /\ UNCHANGED <<cap, parent, orphans>>

---------------------------------------------------------------------------
\* Turns.

Init ==
    /\ cap = [s \in Slots |-> IF s = BootSlot THEN BootUt ELSE None]
    /\ parent = [s \in Slots |-> None]
    /\ revoking = [s \in Slots |-> FALSE]
    /\ rv = [p \in Cores |-> None]
    /\ orphans = {}
    /\ utFree = [i \in {0} \cup Cells |-> IF i = 0 THEN Cells ELSE {}]
    /\ objAt = [x \in Cells |-> [l \in Levels |-> "free"]]
    /\ turn = 1
    /\ spent = 0
    /\ bad = FALSE

\* A revoking core spends up to K steps, then hands over the lock.
RevokeTurn ==
    /\ rv[turn] # None
    /\ RevokeStep(turn)
    /\ IF spent + 1 = K \/ rv'[turn] = None
         THEN turn' = NextTurn /\ spent' = 0
         ELSE turn' = turn /\ spent' = spent + 1

OpTurn ==
    /\ rv[turn] = None
    /\ \/ \E u, d \in Slots, k \in Kinds : Retype(u, d, k)
       \/ \E s, d \in Slots : Copy(s, d) \/ Move(s, d)
       \/ \E s \in Slots : UserDelete(s)
       \/ \E c \in Slots : RevokeStart(turn, c)
       \/ UNCHANGED <<cap, parent, revoking, rv, orphans, utFree, objAt, bad>>
    /\ turn' = NextTurn
    /\ spent' = 0

Next == RevokeTurn \/ OpTurn

Spec == Init /\ [][Next]_vars /\ WF_vars(Next)

---------------------------------------------------------------------------
\* Properties.

NoBadStep == ~bad

\* Every live capability names an object that exists.
CapsNameLiveObjects ==
    \A s \in Slots : Live(s) /\ cap[s].cell # 0 =>
        objAt[cap[s].cell][cap[s].lvl] = cap[s].kind

\* Every live slot outside the root table lives in a table that exists.
SlotsLiveInTables == \A x \in Cells : Live(x) => TableAt(x)

\* The derivation tree has no dangling parents.
ParentsLive == \A s \in Slots : Live(s) /\ parent[s] # None => Live(parent[s])

\* Untypeds are never copied: one capability per untyped.
OneCapPerUntyped ==
    \A s, t \in Slots : s # t /\ Live(s) /\ Live(t) /\ cap[s].kind = "ut" =>
        ~SameObject(cap[s], cap[t])

\* Every revoke that starts ends.
RevokesEnd == \A p \in Cores : [](rv[p] # None => <>(rv[p] = None))

=============================================================================
