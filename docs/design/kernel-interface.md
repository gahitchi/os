# Kernel interface: draft for review

19 November 2026. This follows from decisions 5 (microkernel), 7 (capabilities), 8 (calls plus notifications), 9 (seL4-style memory), 10 (many cores) and 11 (time budgets).

## Shape

The kernel has very few calls. Almost everything a program does is a **call on a capability**: "do this to the object this handle points at". The kernel checks the handle's rights and runs the operation. This keeps the kernel small, and it means one permission system for everything.

## The calls (10)

| Call | What it does |
| --- | --- |
| `call` | Send a message on an endpoint and wait for the reply. Also used to invoke any kernel object |
| `send` | Send a message and wait until it is taken, without waiting for a reply |
| `try_send` | Send only if a receiver is waiting right now; never blocks |
| `recv` | Wait for a message on an endpoint, or a signal on a notification |
| `reply_recv` | Reply to the last caller and wait for the next one, in one step. This is the server's main loop |
| `signal` | Raise bits on a notification. Never blocks. Used for interrupts, doorbells and events |
| `wait` | Wait for bits on a notification |
| `poll` | Check a notification without waiting |
| `yield` | Give up the rest of this time slice |
| `debug` | Print to the serial port. Only in debug builds |

## The kernel objects

Each object is created from untyped memory (decision 9) and used through calls on its capability.

| Object | What it is | Main operations |
| --- | --- | --- |
| Untyped | Raw memory not yet turned into anything | retype (make objects from it), revoke (take back everything made from it) |
| Capability table | A program's list of handles | copy, mint (copy with fewer rights or a badge), move, delete, revoke |
| Endpoint | A meeting point for calls | used by `call`, `send`, `recv`, `reply_recv` |
| Notification | A word of signal bits | used by `signal`, `wait`, `poll` |
| Reply | The right to answer one waiting caller | used by `reply_recv` |
| Thread | A running flow of code | configure, set priority, set core, start, stop, read and write registers |
| Scheduling context | A time budget: X microseconds every Y milliseconds, on a given core type (decision 11) | configure, bind to a thread, unbind |
| Address space | A program's memory map | map and unmap pages: 4 KB, 2 MB and 1 GB |
| Page | One page of memory | map, unmap, get its physical address (for drivers) |
| Interrupt | One hardware interrupt line | connect to a notification, acknowledge |
| I/O port range | Legacy PC I/O ports | read, write |
| IOMMU domain | What memory a device may reach by DMA (Intel VT-d) | map, unmap |

## Rules

- **Every call returns an error code on bad input. The kernel never crashes on what a program sends it.**
- **Every long operation can be interrupted** (revoking, retyping, clearing large memory), so no single call can freeze the system.
- **Drivers can only reach their own device's memory.** Their DMA goes through an IOMMU domain, so a buggy or hostile driver cannot overwrite the kernel or another program.
- **Time comes from budgets.** A thread with no bound scheduling context cannot run.
- **Messages are short**: a small fixed number of words in registers, plus capabilities. Anything bigger goes through shared pages (battle test, decision 5).

## What is not in the kernel

Files, history, grants, the ledger, networking, drivers, the compositor, the POSIX layer and memory policy (including decision 24) are all user-space programs built on these calls.

## Compared with seL4

This is deliberately close to seL4 with its MCS scheduling extension, because those choices are proven. The differences are the 2 MB and 1 GB pages from the start, the IOMMU as a first-class object, core types (performance and efficiency) in scheduling contexts, and a design for many cores from day one.
