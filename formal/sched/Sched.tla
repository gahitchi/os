------------------------------- MODULE Sched --------------------------------
(***************************************************************************)
(* Alaya, milestone 3: a model of the scheduler, time budgets, lent time   *)
(* and notifications on several cores.                                     *)
(*                                                                         *)
(* SPDX-License-Identifier: GPL-3.0-only                                   *)
(*                                                                         *)
(* What is modelled                                                        *)
(*                                                                         *)
(* - Time moves in ticks. Each client thread has a scheduling context with *)
(*   a budget of B ticks in every P ticks (sporadic refills: a tick used   *)
(*   comes back P ticks later). A thread runs only while its context has   *)
(*   time left; when it runs out the thread leaves its core and waits for  *)
(*   the refill, which puts it back in a queue.                            *)
(* - Each core has a current thread and a first-in, first-out queue. An    *)
(*   idle core takes the head of its queue, or steals the head of another  *)
(*   core's queue; a busy core with a queue lets the next thread run (the  *)
(*   time slice). Where a waking thread goes is left open: any core.       *)
(* - One passive server (no time of its own) answers calls. A call lends   *)
(*   the caller's context to the server, which then runs on the caller's   *)
(*   time and on the caller's core; the answer gives it back. Callers that *)
(*   find the server busy wait in the endpoint's queue, and the server     *)
(*   takes the next one when it answers. If the lent time runs out, the    *)
(*   server waits for the caller's refill (decision 55).                   *)
(* - One notification: a thread waits for it, any running thread signals.  *)
(* - From outside, any client can be stopped and started again at any      *)
(*   moment, also while it waits, calls, or its time is lent out. A server *)
(*   keeps lent time until it answers; the answer gives the time back to   *)
(*   its own thread even if that thread no longer waits.                   *)
(* - Every step is atomic: in the kernel it runs under the big lock.       *)
(*                                                                         *)
(* The refill arithmetic itself (late timers, overruns, merging refills)   *)
(* is tested by the unit tests of the alaya-sched crate; here a tick is    *)
(* exact, and the question is how threads, cores and loans interact.       *)
(*                                                                         *)
(* What TLC checks                                                         *)
(*                                                                         *)
(* - Budgets hold: no context runs a tick without time left, and none uses *)
(*   more than B ticks in any P ticks in a row.                            *)
(* - No lost wake-ups: a thread that may run is on exactly one core or in  *)
(*   exactly one queue, and a thread that may not run is on none.          *)
(* - Lent time is accounted: the server holds at most one context, the one *)
(*   of the client it serves.                                              *)
(* - Liveness: every thread that may run gets to run, every call is        *)
(*   answered (or its caller stopped), and lent time always comes back.    *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
    Cores,      \* the cores, as numbers 1..N
    Clients,    \* client threads; each has a context of its own
    Server,     \* the passive server thread (a model value)
    B,          \* budget: ticks per period
    P,          \* period, in ticks
    None,       \* a model value
    Bug         \* "none", or a deliberate mistake the checks must catch

ASSUME B \in Nat \ {0} /\ P \in Nat \ {0} /\ B <= P /\ Server \notin Clients
ASSUME Bug \in {"none", "refill_forgets_thread", "stopped_caller_keeps_loan"}

Threads == Clients \cup {Server}
States == {"run", "call", "recv", "wait", "stop"}

VARIABLES
    st,        \* thread -> state ("run": ready or running)
    holder,    \* context (named after its client) -> thread holding it
    avail,     \* context -> ticks it may still use now
    pend,      \* context -> [1..P -> ticks coming back in that many ticks]
    cur,       \* core -> thread running there, or None
    q,         \* core -> queue of threads
    epq,       \* clients waiting for the server
    caller,    \* the client the server answers, or None
    lent,      \* whose context the server holds, or None
    bits,      \* the notification's word (0 or 1)
    waiters,   \* threads waiting for the notification
    bad        \* set when a context ran a tick without time

vars == <<st, holder, avail, pend, cur, q, epq, caller, lent, bits, waiters, bad>>

Sum(f) == LET S[D \in SUBSET DOMAIN f] ==
                IF D = {} THEN 0
                ELSE LET x == CHOOSE x \in D : TRUE IN f[x] + S[D \ {x}]
          IN S[DOMAIN f]

SetToSeq(S) == CHOOSE f \in [1..Cardinality(S) -> S] :
                   \A i, j \in DOMAIN f : i # j => f[i] # f[j]

\* The context thread t runs on.
Ctx(t) == IF \E s \in Clients : holder[s] = t
          THEN CHOOSE s \in Clients : holder[s] = t ELSE None

\* Thread t may run given these states, holders and time left.
MayRunIn(t, x, h, a) ==
    /\ x[t] = "run"
    /\ \E s \in Clients : h[s] = t /\ a[s] > 0
MayRun(t) == MayRunIn(t, st, holder, avail)

Count(seq, t) == Cardinality({i \in DOMAIN seq : seq[i] = t})
Placed(t) == Cardinality({c \in Cores : cur[c] = t})
             + Sum([c \in Cores |-> Count(q[c], t)])

Without(seq, t) == SelectSeq(seq, LAMBDA x : x # t)

\* Put t at the end of some core's queue.
Enqueue(t) == \E c \in Cores : q' = [q EXCEPT ![c] = Append(@, t)]

Init ==
    /\ st = [t \in Threads |-> IF t = Server THEN "recv" ELSE "run"]
    /\ holder = [s \in Clients |-> s]
    /\ avail = [s \in Clients |-> B]
    /\ pend = [s \in Clients |-> [i \in 1..P |-> 0]]
    /\ cur = [c \in Cores |-> None]
    /\ q = [c \in Cores |-> IF c = 1 THEN SetToSeq(Clients) ELSE <<>>]
    /\ epq = <<>>
    /\ caller = None
    /\ lent = None
    /\ bits = 0
    /\ waiters = {}
    /\ bad = FALSE

-----------------------------------------------------------------------------
(* The kernel *)

\* One tick passes: running contexts are charged, refills come due, threads
\* out of time leave their cores, and threads given time are queued.
Tick ==
    LET used == {s \in Clients : \E c \in Cores : cur[c] # None /\ holder[s] = cur[c]}
        u(s) == IF s \in used THEN 1 ELSE 0
        a2 == [s \in Clients |-> avail[s] - u(s) + pend[s][1]]
        waking == IF Bug = "refill_forgets_thread" THEN {}
                  ELSE {t \in Threads : MayRunIn(t, st, holder, a2) /\ ~MayRun(t)}
    IN
    /\ bad' = (bad \/ \E s \in used : avail[s] = 0)
    /\ avail' = a2
    /\ pend' = [s \in Clients |-> [i \in 1..P |-> IF i = P THEN u(s) ELSE pend[s][i + 1]]]
    /\ cur' = [c \in Cores |-> IF cur[c] # None /\ ~MayRunIn(cur[c], st, holder, a2)
                               THEN None ELSE cur[c]]
    /\ \E f \in [waking -> Cores] :
          q' = [c \in Cores |-> q[c] \o SetToSeq({t \in waking : f[t] = c})]
    /\ UNCHANGED <<st, holder, epq, caller, lent, bits, waiters>>

\* An idle core runs the head of its queue.
Schedule(c) ==
    /\ cur[c] = None /\ q[c] # <<>>
    /\ cur' = [cur EXCEPT ![c] = Head(q[c])]
    /\ q' = [q EXCEPT ![c] = Tail(@)]
    /\ UNCHANGED <<st, holder, avail, pend, epq, caller, lent, bits, waiters, bad>>

\* An idle core with nothing queued takes a queued thread from another.
Steal(c) ==
    /\ cur[c] = None /\ q[c] = <<>>
    /\ \E d \in Cores \ {c} :
          /\ q[d] # <<>>
          /\ cur' = [cur EXCEPT ![c] = Head(q[d])]
          /\ q' = [q EXCEPT ![d] = Tail(@)]
    /\ UNCHANGED <<st, holder, avail, pend, epq, caller, lent, bits, waiters, bad>>

\* The time slice ends: the next queued thread runs.
Preempt(c) ==
    /\ cur[c] # None /\ q[c] # <<>>
    /\ cur' = [cur EXCEPT ![c] = Head(q[c])]
    /\ q' = [q EXCEPT ![c] = Append(Tail(@), cur[c])]
    /\ UNCHANGED <<st, holder, avail, pend, epq, caller, lent, bits, waiters, bad>>

-----------------------------------------------------------------------------
(* What running threads do *)

\* Client t, running on core c, calls the server.
Call(c) ==
    LET t == cur[c] IN
    /\ t \in Clients
    /\ st' = [st EXCEPT ![t] = "call",
                        ![Server] = IF @ = "recv" THEN "run" ELSE @]
    /\ IF st[Server] = "recv"
       THEN \* Lend the context, and run the server here at once.
            /\ holder' = [holder EXCEPT ![Ctx(t)] = Server]
            /\ lent' = Ctx(t)
            /\ caller' = t
            /\ cur' = [cur EXCEPT ![c] = Server]
            /\ UNCHANGED epq
       ELSE /\ epq' = Append(epq, t)
            /\ cur' = [cur EXCEPT ![c] = None]
            /\ UNCHANGED <<holder, lent, caller>>
    /\ UNCHANGED <<avail, pend, q, bits, waiters, bad>>

\* The server, running on core c, answers and takes the next caller.
Reply(c) ==
    /\ cur[c] = Server /\ lent # None
    /\ LET h == lent   \* the context's own thread
           back == caller = h
           st1 == [st EXCEPT ![h] = IF back THEN "run" ELSE @]
           next == IF epq # <<>> THEN Head(epq) ELSE None
           holder1 == IF Bug = "stopped_caller_keeps_loan" /\ ~back
                      THEN [holder EXCEPT ![h] = None]
                      ELSE [holder EXCEPT ![h] = h]
           holder2 == IF next # None THEN [holder1 EXCEPT ![next] = Server] ELSE holder1
           st2 == [st1 EXCEPT ![Server] = IF next # None THEN "run" ELSE "recv"]
           \* The answered caller may run now (if it has time).
           hRuns == MayRunIn(h, st2, holder2, avail)
           sRuns == next # None /\ avail[next] > 0
       IN
       /\ st' = st2
       /\ holder' = holder2
       /\ lent' = next
       /\ caller' = next
       /\ epq' = IF next # None THEN Tail(epq) ELSE epq
       /\ IF sRuns
          THEN \* The server goes on with the next caller; the answered
               \* one is queued somewhere.
               /\ cur' = cur
               /\ IF hRuns THEN Enqueue(h) ELSE UNCHANGED q
          ELSE \* Switch straight back to the answered caller.
               /\ cur' = [cur EXCEPT ![c] = IF hRuns THEN h ELSE None]
               /\ UNCHANGED q
    /\ UNCHANGED <<avail, pend, bits, waiters, bad>>

\* The thread on core c waits for the notification.
Wait(c) ==
    LET t == cur[c] IN
    /\ t \in Clients
    /\ IF bits = 1
       THEN /\ bits' = 0
            /\ UNCHANGED <<st, cur, waiters>>
       ELSE /\ st' = [st EXCEPT ![t] = "wait"]
            /\ waiters' = waiters \cup {t}
            /\ cur' = [cur EXCEPT ![c] = None]
            /\ UNCHANGED bits
    /\ UNCHANGED <<holder, avail, pend, q, epq, caller, lent, bad>>

\* The thread on core c signals the notification.
Signal(c) ==
    /\ cur[c] # None
    /\ IF waiters = {}
       THEN /\ bits' = 1
            /\ UNCHANGED <<st, waiters, q>>
       ELSE \E w \in waiters :
              /\ st' = [st EXCEPT ![w] = "run"]
              /\ waiters' = waiters \ {w}
              /\ IF MayRunIn(w, st', holder, avail) THEN Enqueue(w) ELSE UNCHANGED q
              /\ UNCHANGED bits
    /\ UNCHANGED <<holder, avail, pend, cur, epq, caller, lent, bad>>

-----------------------------------------------------------------------------
(* From outside: stop and start clients at any moment *)

Stop(t) ==
    /\ st[t] # "stop"
    /\ st' = [st EXCEPT ![t] = "stop"]
    /\ epq' = Without(epq, t)
    \* A server answering t keeps the lent time until it answers.
    /\ caller' = IF caller = t THEN None ELSE caller
    /\ waiters' = waiters \ {t}
    /\ cur' = [c \in Cores |-> IF cur[c] = t THEN None ELSE cur[c]]
    /\ q' = [c \in Cores |-> Without(q[c], t)]
    /\ UNCHANGED <<holder, avail, pend, lent, bits, bad>>

Start(t) ==
    /\ st[t] = "stop"
    /\ st' = [st EXCEPT ![t] = "run"]
    /\ IF MayRunIn(t, st', holder, avail) THEN Enqueue(t) ELSE UNCHANGED q
    /\ UNCHANGED <<holder, avail, pend, cur, epq, caller, lent, bits, waiters, bad>>

-----------------------------------------------------------------------------

Kernel == Tick \/ \E c \in Cores : Schedule(c) \/ Steal(c) \/ Preempt(c) \/ Reply(c)
Programs == \E c \in Cores : Call(c) \/ Wait(c) \/ Signal(c)
Outside == \E t \in Clients : Stop(t) \/ Start(t)

Next == Kernel \/ Programs \/ Outside

\* The kernel is fair; programs and the outside world need not be. The
\* server answers whenever it gets to run often enough.
Fairness ==
    /\ WF_vars(Tick)
    /\ \A c \in Cores : WF_vars(Schedule(c)) /\ WF_vars(Steal(c)) /\ SF_vars(Preempt(c))
    /\ \A c \in Cores : SF_vars(Reply(c))

Spec == Init /\ [][Next]_vars /\ Fairness

-----------------------------------------------------------------------------
(* Properties *)

TypeOK ==
    /\ st \in [Threads -> States]
    /\ holder \in [Clients -> Threads \cup {None}]
    /\ avail \in [Clients -> 0..B]
    /\ pend \in [Clients -> [1..P -> 0..1]]
    /\ cur \in [Cores -> Threads \cup {None}]
    /\ caller \in Clients \cup {None}
    /\ lent \in Clients \cup {None}
    /\ bits \in {0, 1}
    /\ waiters \subseteq Clients

\* No context ever runs without time, nor more than B ticks in P.
BudgetsHold ==
    /\ ~bad
    /\ \A s \in Clients : avail[s] + Sum(pend[s]) = B

\* A thread that may run is in exactly one place; one that may not, nowhere.
NoLostWakeups ==
    \A t \in Threads : Placed(t) = IF MayRun(t) THEN 1 ELSE 0

\* Only the server borrows, one context at a time, from the client it serves
\* (or served, if that client was stopped meanwhile).
LoansAccounted ==
    /\ \A s \in Clients : holder[s] \in {s, Server}
    /\ \A s \in Clients : holder[s] = Server <=> lent = s
    /\ caller # None => caller = lent
    /\ (st[Server] = "recv") <=> (lent = None)
    /\ \A t \in Clients : st[t] = "call" <=> (t = caller \/ \E i \in DOMAIN epq : epq[i] = t)

\* Liveness.
ReadyThreadsRun ==
    \A t \in Threads : MayRun(t) ~> (\E c \in Cores : cur[c] = t) \/ ~MayRun(t)
CallsAnswered ==
    \A t \in Clients : st[t] = "call" ~> st[t] # "call"
LoansReturn ==
    \A s \in Clients : holder[s] = Server ~> holder[s] = s
=============================================================================
