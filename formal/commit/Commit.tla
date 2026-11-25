------------------------------- MODULE Commit -------------------------------
(***************************************************************************)
(* Alaya, milestone 5: a model of how the store commits (decision 16).    *)
(*                                                                         *)
(* SPDX-License-Identifier: GPL-3.0-only                                   *)
(*                                                                         *)
(* What is modelled                                                        *)
(*                                                                         *)
(* - The state of each commit is a set of blocks, each with the commit    *)
(*   that wrote it (its birth). The fingerprints in the real store make a *)
(*   state readable exactly when every one of its blocks still holds what *)
(*   that commit wrote there; here a block holds the birth of its last    *)
(*   write.                                                                *)
(* - A commit replaces some blocks of the state with fresh ones: blocks   *)
(*   no kept state uses, and not given up in the last Defer commits. It   *)
(*   writes them (into the disk's cache), flushes, writes the front page  *)
(*   of its number (page number mod 2) and flushes again; only then is    *)
(*   the commit promised.                                                  *)
(* - The disk keeps writes in a cache until a flush. At any moment the    *)
(*   power may go: any part of the cached writes reach the disk, a cached *)
(*   front page may be torn, and (at most MaxDamage times) the newest     *)
(*   whole front page gets a bad spot (not in the same cut as a torn      *)
(*   front page: that would be two faults at once).                        *)
(* - Then the store opens at the newest front page whose state is whole.  *)
(*                                                                         *)
(* What TLC checks                                                         *)
(*                                                                         *)
(* - The store can always be opened after a cut.                           *)
(* - No promised commit is lost (one step back only when the newest front *)
(*   page itself was damaged).                                             *)
(* - The state it opens is exactly a state that was committed.             *)
(*                                                                         *)
(* Milestone 7 adds wiping (forget, decision 62): blocks that held file   *)
(* keys are overwritten with junk when they are freed, ScrubDefer commits *)
(* after they were given up. TLC checks that this never breaks the above, *)
(* and that once a commit doing it is promised, none of those blocks     *)
(* still holds what it held (KeysWiped). Wiping one commit early breaks  *)
(* opening after a bad front page (EarlyScrub.cfg must fail).             *)
(*                                                                         *)
(* And that the model has teeth: reusing given-up blocks after one commit *)
(* instead of two leaves nothing to open after a bad front page            *)
(* (EarlyReuse.cfg), and writing the front page without flushing the      *)
(* blocks first loses the state (NoFlush.cfg).                             *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    Blocks,     \* block numbers (the front pages are separate)
    Size,       \* blocks in every state
    MaxGen,     \* commits to explore
    Defer,      \* blocks given up in commit g are reused from g + Defer
    MaxDamage,  \* bad spots on front pages
    Flush,      \* flush the blocks before writing the front page
    ScrubDefer  \* given-up blocks are wiped this many commits later

VARIABLES
    states,     \* states[g]: the set of <<block, birth>> of commit g
    released,   \* released[g]: blocks commit g gave up
    disk,       \* disk[b]: birth of the last write that reached block b
    cache,      \* writes not flushed yet: a set of <<block, birth>>
    front,      \* front[s]: [gen, ok] of front page s, on the disk
    frontCache, \* a front page write not flushed yet, or none
    gen,        \* the newest promised commit, as the running store knows
    phase,      \* "idle", "write", "front", "frontflush"
    toWrite,    \* blocks of the commit being made, not written yet
    damage,     \* bad spots used
    broken,     \* opening found nothing to open
    lost,       \* opening lost a promised commit
    gaveUp,     \* <<block, birth, commit>> given up and not yet wiped
    wiping,     \* the part of gaveUp the commit being made wipes
    real,       \* the commit being made writes blocks (not an open's bump)
    wiped       \* <<block, birth>> whose wiping was promised

vars == <<states, released, disk, cache, front, frontCache, gen, phase,
          toWrite, damage, broken, lost, gaveUp, wiping, real, wiped>>

\* What a wiped block holds: no commit's birth.
Junk == MaxGen + 10

Gens == 0..(MaxGen + 4)

None == [gen |-> 0, ok |-> FALSE, slot |-> 0]

BlocksOf(S) == {p[1] : p \in S}

\* Blocks that may be written in commit n: not in the state it builds on,
\* not given up in the last Defer commits.
Free(n) == Blocks \ (BlocksOf(states[n - 1]) \cup
              UNION {released[g] : g \in {x \in 1..(n - 1) : x + Defer > n}})

Init ==
    /\ states = [g \in Gens |-> IF g = 0
                    THEN {<<b, 0>> : b \in {x \in Blocks : x <= Size}}
                    ELSE {}]
    /\ released = [g \in Gens |-> {}]
    /\ disk = [b \in Blocks |-> 0]
    /\ cache = {}
    /\ front = [s \in 0..1 |-> IF s = 0 THEN [gen |-> 0, ok |-> TRUE]
                                        ELSE [gen |-> 0, ok |-> FALSE]]
    /\ frontCache = None
    /\ gen = 0
    /\ phase = "idle"
    /\ toWrite = {}
    /\ damage = 0
    /\ broken = FALSE
    /\ lost = FALSE
    /\ gaveUp = {}
    /\ wiping = {}
    /\ real = FALSE
    /\ wiped = {}

\* Start commit gen + 1: replace a non-empty part of the state.
Begin ==
    /\ phase = "idle" /\ gen < MaxGen
    /\ LET n == gen + 1 IN
       \E R \in SUBSET states[gen] :
         /\ R # {}
         /\ \E N \in SUBSET Free(n) :
              /\ Cardinality(N) = Cardinality(R)
              /\ states' = [states EXCEPT ![n] =
                               (states[gen] \ R) \cup {<<b, n>> : b \in N}]
              /\ released' = [released EXCEPT ![n] = BlocksOf(R)]
              \* Wipe what was given up ScrubDefer commits ago, unless
              \* this commit reuses the block anyway.
              /\ LET due == {p \in gaveUp : p[3] + ScrubDefer <= n}
                     junk == {<<p[1], Junk>> : p \in {q \in due : q[1] \notin N}}
                 IN /\ toWrite' = {<<b, n>> : b \in N} \cup junk
                    /\ wiping' = due
              /\ gaveUp' = gaveUp \cup {<<p[1], p[2], n>> : p \in R}
    /\ phase' = "write"
    /\ real' = TRUE
    /\ UNCHANGED <<disk, cache, front, frontCache, gen, damage, broken, lost,
                   wiped>>

Write ==
    /\ phase = "write"
    /\ \E p \in toWrite :
         /\ cache' = cache \cup {p}
         /\ toWrite' = toWrite \ {p}
    /\ UNCHANGED <<states, released, disk, front, frontCache, gen, phase,
                   damage, broken, lost, gaveUp, wiping, real, wiped>>

ApplyCache(d, C) == [b \in Blocks |->
    IF \E p \in C : p[1] = b THEN (CHOOSE p \in C : p[1] = b)[2] ELSE d[b]]

\* All blocks written: flush them (unless the model leaves that out), then
\* write the front page.
WriteFront ==
    /\ phase = "write" /\ toWrite = {}
    /\ disk' = IF Flush THEN ApplyCache(disk, cache) ELSE disk
    /\ cache' = IF Flush THEN {} ELSE cache
    /\ frontCache' = [gen |-> gen + 1, ok |-> TRUE, slot |-> (gen + 1) % 2]
    /\ phase' = "frontflush"
    /\ UNCHANGED <<states, released, front, gen, toWrite, damage, broken, lost,
                   gaveUp, wiping, real, wiped>>

FlushFront ==
    /\ phase = "frontflush"
    /\ disk' = ApplyCache(disk, cache)
    /\ cache' = {}
    /\ front' = [front EXCEPT ![frontCache.slot] =
                    [gen |-> frontCache.gen, ok |-> TRUE]]
    /\ frontCache' = None
    /\ gen' = frontCache.gen
    /\ phase' = "idle"
    \* A promised commit has wiped (or reused) what it was due to.
    /\ gaveUp' = IF real THEN gaveUp \ wiping ELSE gaveUp
    /\ wiped' = IF real THEN wiped \cup {<<p[1], p[2]>> : p \in wiping} ELSE wiped
    /\ wiping' = {}
    /\ real' = FALSE
    /\ UNCHANGED <<states, released, toWrite, damage, broken, lost>>

Whole(f, d) == f.ok /\ states[f.gen] # {} /\ \A p \in states[f.gen] : d[p[1]] = p[2]

\* Opening at commit `at` writes a front page of a fresh number for the
\* same state (the store's "bump"): past the highest readable front page
\* h, and on the other page than the one just opened, so that page stays
\* as the older page. (A newer page with a bad spot may have used a number
\* the store cannot see; that is harmless, because a block's key stream
\* comes from its own fingerprint, not from the commit number alone.)
BumpOf(h, at) == IF (h + 1) % 2 = at % 2 THEN h + 2 ELSE h + 1

\* The power goes: some cached writes made it, a cached front page may be
\* torn, the newest whole front page may get a bad spot. Then open.
Cut ==
    /\ phase # "idle" \/ gen < MaxGen
    /\ \A s \in 0..1 : front[s].gen <= MaxGen
    /\ frontCache.gen <= MaxGen
    /\ \E kept \in SUBSET cache, frontFate \in {"lost", "kept", "torn"},
          hurt \in {FALSE, TRUE} :
         /\ hurt => damage < MaxDamage
         \* A bad spot is survivable only once both front pages exist
         \* (the store writes two commits when it formats a disk).
         /\ hurt => (front[0].ok /\ front[1].ok)
         \* A torn front page already spends the older page (it is the
         \* one being overwritten); a bad spot on the other at the same
         \* moment is two faults at once, which two pages cannot cover.
         /\ hurt => frontFate # "torn"
         /\ (frontCache = None) => frontFate = "lost"
         /\ LET d == ApplyCache(disk, kept)
                f1 == IF frontFate = "lost" THEN front
                      ELSE [front EXCEPT ![frontCache.slot] =
                              [gen |-> frontCache.gen,
                               ok |-> frontFate = "kept"]]
                \* The newest front page that would open.
                w1 == {s \in 0..1 : Whole(f1[s], d)}
                newest == IF w1 = {} THEN 0
                          ELSE CHOOSE s \in w1 : \A t \in w1 : f1[s].gen >= f1[t].gen
                f2 == IF hurt THEN [f1 EXCEPT ![newest].ok = FALSE] ELSE f1
                readable == {s \in 0..1 : f2[s].ok}
                h == IF readable = {} THEN 0
                     ELSE f2[CHOOSE s \in readable :
                                \A t \in readable : f2[s].gen >= f2[t].gen].gen
                good == {s \in 0..1 : Whole(f2[s], d)}
                opened == IF good = {} THEN 0
                          ELSE (CHOOSE s \in good :
                                  \A t \in good : f2[s].gen >= f2[t].gen)
                at == f2[opened].gen
                n == BumpOf(h, at)
            IN /\ disk' = d
               /\ front' = f2
               /\ broken' = (good = {})
               \* Lost: the opened state is not the promised one, unless the
               \* promised page got the bad spot and the older page opened.
               \* (A commit cut off after its front page made it is kept.)
               /\ lost' = (good # {} /\ at < gen /\ states[at] # states[gen]
                             /\ ~(hurt /\ opened # newest))
               /\ IF good = {}
                     THEN /\ UNCHANGED <<states, released, gen, gaveUp>>
                          /\ frontCache' = None
                          /\ phase' = "idle"
                     ELSE /\ states' = [g \in Gens |->
                                IF g = n THEN states[at] ELSE states[g]]
                          \* What dead commits gave up is not given up.
                          /\ released' = [g \in Gens |->
                                IF g > at THEN {} ELSE released[g]]
                          /\ gaveUp' = {p \in gaveUp : p[3] <= at}
                          /\ gen' = at
                          /\ frontCache' = [gen |-> n, ok |-> TRUE, slot |-> n % 2]
                          /\ phase' = "frontflush"
         /\ damage' = damage + (IF hurt THEN 1 ELSE 0)
    /\ cache' = {}
    /\ toWrite' = {}
    /\ wiping' = {}
    /\ real' = FALSE
    /\ UNCHANGED wiped

Next == Begin \/ Write \/ WriteFront \/ FlushFront \/ (~broken /\ Cut)

Spec == Init /\ [][Next]_vars

\* ---- Properties ----

TypeOK ==
    /\ gen \in Gens
    /\ phase \in {"idle", "write", "frontflush"}

AlwaysOpens == ~broken

NoLostCommit == ~lost

\* Every state of a commit has Size distinct blocks.
StatesWellFormed == Cardinality(BlocksOf(states[gen])) = Size

\* Once wiping a block was promised, the disk no longer holds what it held
\* (it holds junk, or a later commit's block).
KeysWiped == \A p \in wiped : disk[p[1]] # p[2]
=============================================================================
