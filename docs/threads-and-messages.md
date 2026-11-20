# Threads, time budgets and messages (milestone 3)

This is how Alaya's kernel runs programs: threads on every core, time
budgets that a thread cannot overrun, messages between programs, and
address spaces that keep programs apart. It follows
`design/kernel-interface.md` (decision 26) and decisions 53 to 56.

## Why it matters

- **Steady frame times under load.** Every thread runs on a *time budget*:
  "this much time in every period". A game's render thread can be given
  4 ms in every 8 ms, and no other program, however busy, can take that
  time away; a runaway program cannot use more than its own budget.
- **Programs that cannot break each other.** Each program has its own
  memory map and its own capability table. A crash in a program becomes a
  message to whoever looks after it; the kernel and the other programs go
  on.
- **Fast messages.** Drivers, file systems and the desktop are separate
  programs (decision 5), so a call from one to another must be cheap. A
  call and its answer go through the kernel in one step each, with the
  message in registers.

## Objects

All are made by retyping untyped memory, like every kernel object
(decision 51): the kernel still allocates nothing.

| Object | Size | What it is |
| --- | --- | --- |
| Thread | 4 KiB | Saved registers, the FPU and vector state (x87, SSE, AVX, AVX-512, saved with XSAVE), priority, its capability table, address space and fault endpoint |
| Scheduling context | 256 bytes | A time budget: budget and period, its refills, the core type it wants or the one core it is pinned to |
| Reply | 64 bytes | Holds a caller waiting for its answer, and the time it lent |
| Endpoint | 64 bytes | Where messages meet: senders and receivers queue here |
| Notification | 64 bytes | A word of bits that threads signal and wait for |
| Address space | 8 KiB | A program's memory map; page tables come from an untyped given to it |
| Scheduling control | none | One per machine, handed to the first program: the right to give out time |

## Time budgets (decisions 53 and 54)

A scheduling context gives its thread *budget* microseconds in every
*period*. Time comes back by **sporadic refills** (as in seL4's MCS
kernel): time used in a stretch that started at time *s* comes back at
*s* + period. So in **any** window of one period a thread runs at most its
budget, however its runs and sleeps fall, which is what makes guarantees
for audio and frames hold under stress. A context whose budget equals its
period is plain round robin.

If a timer fires late and a thread overran its budget a little, the
overrun is a *debt*, paid from its next refills (at most half a budget is
carried), so late timers cannot add up to extra time.

**Priorities**: 256 levels. The highest-priority thread that may run, runs;
threads of equal priority share a core in 2 ms slices. A thread can only
give priorities up to its own *maximum controlled priority*, so a program
cannot raise itself above what it was given.

**Placement** (decision 54): a context names a core type (any, performance
or efficiency; read from CPUID on hybrid CPUs like the reference laptop's
i7 13th gen) or pins its thread to one core. When a thread wakes the kernel
picks, among the allowed cores: the core it last ran on if that core is
idle; else the waking core if it is about to stop its current thread
(handing over at once, as in a call); else an idle core; else the core
running the least important work below the waking thread; else the
shortest queue. Idle cores take waiting work from others. Running threads
are never moved.

## Messages

| Call | `rax` | What it does |
| --- | --- | --- |
| `call` | 0 | On an endpoint: send and wait for the answer. On any other capability: an operation (below) |
| `send` | 1 | Send, waiting until a receiver takes it. On a Reply: answer its caller |
| `try_send` | 2 | Send only if a receiver is waiting now |
| `recv` | 3 | Wait for a message (or a signal of the bound notification) |
| `reply_recv` | 4 | Answer the last caller and wait for the next message, in one step |
| `signal` | 5 | Raise bits on a notification |
| `wait` | 6 | Wait for bits on a notification, and clear them |
| `poll` | 7 | Take the bits without waiting |
| `yield` | 8 | Give the rest of the slice to the next thread of the same priority |
| `debug` | 9 | Print, report the boot tests, counters (debug builds) |

A message is an info word (label, length, flags) and up to six data
words, all in registers:

| Register | In | Out |
| --- | --- | --- |
| `rax` | call number | error code |
| `rdi` | handle | badge of the sender (or the notification's bits) |
| `rsi` | info: label << 8, length, flags | info |
| `rdx`, `r10`, `r8`, `r9`, `r12`, `r13` | data words | data words |
| `r14` | capability to send, and (<< 32) the slot to receive one in | |
| `r15` | the Reply object for `recv` and `reply_recv` | |

A message can carry one capability, if the endpoint capability has the
grant right. *Badges*, set by `mint`, tell a server who is calling.

**Lent time.** A server may have no time of its own (a *passive* server).
A call then lends the caller's scheduling context to the server, which
runs on the caller's time and at the higher of the two priorities, and the
answer gives it back. So a server costs nothing while idle, each caller
pays for its own requests, and a low-priority caller can never hold up a
high-priority one through a shared server (no priority inversion). If the
lent time runs out mid-request, the server waits for the caller's next
refill (decision 55): the slow caller pays for itself.

**Faults.** When a thread faults (a bad memory access, an invalid
instruction, a division by zero), the kernel sends a message to the
thread's fault endpoint, with label `0xfa17` and the vector, error code,
address, `rip` and `rsp`. Whoever receives it can repair the thread and
answer, or stop it. With no fault endpoint the thread stops and the kernel
prints one line about it.

## Address spaces (decision 56)

An address space is given an untyped of its own; the kernel takes page
tables from it as mappings need them. Programs only map pages: `page_map`
puts a page capability into an address space at an address, with write and
execute rights as asked (and as the capability allows). Each page
capability remembers where it is mapped; deleting it unmaps the page, and
the kernel removes it from every core's TLB before the call returns. A page
can be shared by mapping copies of its capability into two address
spaces.

## Operations

| Object | Operation | Arguments |
| --- | --- | --- |
| Thread | configure | table, address space, fault endpoint |
| Thread | priority | priority, maximum controlled priority |
| Thread | write | `rip`, `rsp`, two arguments, thread-local storage base (stopped threads) |
| Thread | read | (returns `rip`, `rsp`, `rax`, state) |
| Thread | start, stop | |
| Thread | bind | notification, to wait for messages and signals at once |
| Scheduling context | bind, unbind | thread |
| Scheduling context | info | (returns budget, period, time used, last core) |
| Scheduling control | configure | context, budget µs, period µs, core type, pinned core + 1 |
| Address space | assign | untyped for page tables |
| Page | map | address space, address, rights |
| Page | unmap, address | |

The numbers are in [`abi/src/lib.rs`](../abi/src/lib.rs); the wrappers in
[`user/sys`](../user/sys/src/lib.rs).

## Inside the kernel

All kernel work still runs under one big lock, with one kernel stack per
core (as seL4 does): calls are short, and long ones (revoke, retype,
finalising objects) are interruptible and restart. Each core has 256 run
queues and a bitmap of the non-empty ones. FPU state is saved and restored
on every switch (eager switching; lazy switching leaks state through
speculative execution). The return to user mode uses `sysret` when the
thread entered by `syscall`, otherwise `iretq`.

Deleting a thread, scheduling context, endpoint or address space that
another core is using first takes it off that core (an interrupt asks the
core to let go, and the delete restarts until it has), so no core ever
runs on memory that was handed out again.

## How it was checked

| What | Where | Checks |
| --- | --- | --- |
| TLA+ model | [`formal/sched/Sched.tla`](../formal/sched/Sched.tla) | Threads, per-core queues, stealing, time slices, budgets with sporadic refills, a passive server on lent time, a notification, and threads stopped and started from outside at any moment. Every interleaving on 2 cores (and 3 cores for safety): no context runs without time or more than its budget in any period; a thread that may run is on exactly one core or queue, and one that may not is on none (no lost wake-ups); only the server borrows, one context at a time; every thread that may run gets to run, every call is answered and lent time always comes back. Two planted mistakes (a refill that forgets its thread, a loan not returned when its caller was stopped) must be caught |
| Host tests | [`sched/src/tests.rs`](../sched/src/tests.rs) | The refill arithmetic: the window bound under random run patterns, overruns and late timers, merging when the refill list is full |
| Boot checks | [`user/init/src/m3.rs`](../user/init/src/m3.rs) | 13 checks on the real kernel: threads on every core, budgets holding a thread that never stops, priorities, calls and replies with badges, rights and capabilities, notifications, calls from every core at once, a passive server on lent time, faults as messages, a second program in its own address space with a shared page, unmapping, and one revoke taking back everything while threads run |
| Fuzzer | [`user/init/src/fuzz.rs`](../user/init/src/fuzz.rs) | Threads on every core make random system calls, aimed at real objects with odd arguments, on a sandbox program; stuck threads are reset; at the end everything is revoked while they run, and the kernel must still work. CI runs it for a minute emulated and two minutes with KVM |

Run them:

```sh
cargo test -p alaya-sched           # refill arithmetic
formal/check.sh                     # TLC (needs Java 11 or newer)
cargo xtask test --cpus 8           # the boot checks
cargo xtask test --cpus 8 --fuzz 60 # the boot checks, then a minute of fuzzing
```

## Measured

Call and answer, round trip, as printed at boot (`ipc:` line). Under
emulation (TCG) the numbers mean nothing; with KVM they are the real cost.
Numbers from the reference laptop go here once measured.

## Not yet

Interrupts delivered to drivers as notifications, timeout messages for
servers whose lent time runs out, a small helper that
reads Intel's hardware feedback and sets each budget's core type (placement
itself stays in the kernel, decision 54), and device memory come in
milestone 4.
