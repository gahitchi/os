# Capabilities and memory (milestone 2)

This is how Alaya's kernel keeps capabilities, hands out memory and takes it
back. It follows `design/kernel-interface.md` (decision 26) and decision 51.

## The rules

1. **A program can only use what it holds a capability to.** A capability
   names one kernel object and carries rights: read, write and grant
   (allowed to be copied onward).
2. **The kernel allocates nothing.** At boot it gives every free byte of
   memory to the first program as *untyped* capabilities. Kernel objects are
   made by *retyping* untyped memory: capability tables, endpoints,
   notifications, pages (4 KiB, 2 MiB, 1 GiB) and smaller untyped blocks.
3. **Each program has one flat table.** A handle is an index into it, like a
   file descriptor. Slots of other tables are named by a handle to that table
   plus an index.
4. **Everything derived can be taken back.** A copy is a child of the
   capability it was copied from; an object made by retype is a child of
   its untyped. `revoke` deletes everything below a capability. So whoever
   hands something out can always take it back, and so can whoever handed
   it to them.
5. **Memory comes back only through revoke** (decision 51). Deleting a
   capability, even the last one to an object, frees nothing. Revoking an
   untyped deletes every capability to every object made from it, then
   resets its watermark so the memory can be retyped again.
6. **Revoke can always be interrupted, and always finishes.** It works in
   steps and returns to the program after a short slice (50 µs); the call
   then restarts by itself and carries on. While it runs, nothing can be
   copied or retyped from the capability being revoked, and in each turn of
   the big kernel lock it deletes more than all other cores together can
   create, so no program can keep a revoked capability alive by copying it
   in a loop.

## How the kernel keeps track

Every slot is 64 bytes: the capability (32 bytes) and four links that place
it in the *derivation tree*: its first and last child, and its previous and
next sibling. The first child's "previous" and the last child's "next" point
back at the parent. With these links every change is O(1), with no search:

- **copy** puts the new capability in as the first child of its source;
- **move** rewrites the neighbours' links to point at the new slot;
- **delete** splices the capability's children into its place among its
  siblings, so nothing derived from it escapes its parent's revoke;
- **revoke** deletes the first child, again and again, until there is none.

All capabilities to one object sit next to each other in the tree, so
"is this the last capability to this object" is answered by looking at the
neighbours.

### Tables nobody can reach

A capability table can hold capabilities to objects elsewhere. If the last
capability to such a table is deleted, nobody can reach the table again,
but the capabilities inside it still exist. The kernel puts the table on a
**purge list**. Every revoke of an untyped first empties the whole purge
list, and only then resets the untyped. This guarantees that no slot ever
lives in memory that is handed out again, and that no capability ever names
such memory.

### Retype

Retype makes up to 16 objects per call and says how many it made; the
system call library loops for more. Objects are aligned to their size. The
memory is zeroed before use, 64 KiB per step, and the progress is kept in
the untyped's *clean* offset, so an interrupted retype carries on where it
stopped. New untyped blocks are not zeroed when made, only when they are
retyped in turn.

## System calls

A program enters the kernel with the `syscall` instruction:

| Register | In | Out |
| --- | --- | --- |
| `rax` | call number | error code (0 is success) |
| `rdi` | handle | |
| `rsi` | operation | |
| `rdx`, `r10`, `r8`, `r9`, `r12`, `r13` | arguments | results in `rdx`, `r10`, `r8`, `r9` |

Milestone 2 has `call` on capability tables and untyped memory, `yield` and
`debug`; the message calls (`send`, `recv` and the rest) come in milestone 3.

| Object | Operation | Arguments |
| --- | --- | --- |
| Table | copy | destination index, source table, source index, rights mask |
| Table | mint | as copy, plus a badge (endpoints and notifications) |
| Table | move | destination index, source table, source index |
| Table | delete | index |
| Table | revoke | index |
| Table | inspect | index |
| Untyped | retype | kind, size, count, destination table, first destination index |

The numbers are in [`abi/src/lib.rs`](../abi/src/lib.rs); the wrappers in
[`user/sys`](../user/sys/src/lib.rs).

## The first program

`user/init` starts with its own table of 4096 slots (handle 1), one untyped
capability per free memory block (from handle 2), and a page of boot
information at a fixed address. In milestone 2 it is a test: it checks
retype, copy, mint, move, delete and revoke, including a revoke of 16383
capabilities that the kernel interrupts many times, and the reuse of memory
after an untyped is revoked. When all checks pass the kernel prints
`alaya: boot ok`.

## How it was checked

The model came first, then the code:

| What | Where | Checks |
| --- | --- | --- |
| TLA+ model | [`formal/revoke/Revoke.tla`](../formal/revoke/Revoke.tla) | On 2 cores, every interleaving: no capability names reset memory, no slot lives in reset memory, retype never reuses live memory, new tables are empty, every revoke ends. On 3 cores, the same safety checks. A third run with a revoke that is too slow must find an endless revoke, which shows the liveness check works |
| Kani proofs | [`cap/src/proofs.rs`](../cap/src/proofs.rs) | Any sequence of operations keeps the tree's links consistent and the O(1) "last capability" test exact; each revoke step deletes exactly one descendant and keeps the rest below it, so a revoke deletes exactly the descendants however often it is interrupted; an interrupted revoke leaves everything consistent and blocks copies, moves, deletes and retypes; slot lookup stays inside the table; retype never overflows, stays inside the untyped and aligns objects |
| Host tests | [`cap/src/tests.rs`](../cap/src/tests.rs) | Hand-written cases, plus thousands of random operations, with revokes interrupted at random points, checked against a brute-force reference |
| Boot test | [`user/init`](../user/init/src/main.rs) | The real kernel in QEMU, from user mode, on 1, 2 and 8 cores |

Run them:

```sh
cargo test -p alaya-cap         # host tests
formal/check.sh                 # TLC (needs Java 11 or newer)
cargo install --locked kani-verifier && cargo kani setup
cargo kani -p alaya-cap         # Kani proofs
cargo xtask test --cpus 8       # the boot test
```

## Not yet

Threads, address spaces and mapping pages, endpoints and notifications in
use, scheduling contexts and interrupts are milestone 3 and 4. Device memory
(untyped blocks for hardware registers) comes with the driver framework in
milestone 4.
