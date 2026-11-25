# The Linux world

Milestone 8b, step 1 (decisions 66, 92 and 163 to 166).

Most programs people use today are made for Linux, Claude Code among them.
Alaya runs them **unchanged** in the Linux world: a program inside Alaya
that answers a Linux program's requests ("write this", "give me memory",
"what time is it") exactly as Linux would. The Linux program never knows it
is not on Linux.

## How a Linux program runs

- **Its threads speak Linux** (decision 163). The kernel has one switch per
  thread (`THREAD_LINUX`). When it is on, every system call the thread makes
  is a *Linux request*: the kernel sends it, with its number and six
  arguments, as a message to the thread's Linux world (label
  `LINUX_LABEL + number`), and the world's answer becomes the call's result.
  None of Alaya's own calls are reachable at all, and numbers beyond any
  Linux call are refused by the kernel itself with Linux's "not implemented".
- **It holds nothing** of Alaya's: no capabilities, no table. Everything it
  gets comes through the world's answers.
- **Its memory** is its own piece of the world's memory (decision 164). The
  world makes its address space, thread and pages from that piece, and every
  page is also mapped in a window of the world's own, so the world reads and
  writes the program's memory directly. Ending a program takes everything
  back with one revoke.
- **Start**: the world loads the static ELF file, sets up the stack as Linux
  does (argument count, arguments, environment, and the auxiliary vector with
  the program headers, page size, entry point, user and 16 random bytes),
  switches the thread to Linux and starts it.

## What it answers today

`read` (standard input is empty), `write` and `writev` (to the screen),
`openat` (no files yet: "no such file"), `close`, `brk`, `mmap` (anonymous
memory), `munmap`, `mprotect`, `ioctl` ("not a terminal"), `getpid`,
`gettid`, `getuid` and friends, `uname` ("Linux"), `arch_prctl` (the thread
pointer on a PC; ARM programs set theirs themselves), `set_tid_address`,
`clock_gettime`, `rt_sigaction`, `rt_sigprocmask`, `exit` and `exit_group`.
Anything else gets Linux's own "not implemented" and is counted, so the
missing pieces show up by name.

What a program prints appears line by line in Alaya's log as
`linux: <program>> <text>`.

## The flight recorder (decision 165)

The world keeps each program's last 64 requests. When a program crashes,
Alaya says in plain words what it was doing just before, for example:

> crash crashed. Just before, it wrote 15 bytes to the screen, asked for a
> page of memory, asked who it is; then it touched memory at 0x10 that it
> never asked for

## The two-world check (decision 166)

The Linux test programs (`user/linux`) are ordinary static Linux programs,
built from source with clang and lld for each processor; a copy of each is
put in `build/linux-tests/<processor>/`. Every boot test that runs them
also runs the very same files on the test machine itself when it is a
Linux machine with the same processor, with the same arguments and
environment, and what they print must match byte for byte:

```
xtask: two-world check: hello printed the same on Linux and in Alaya (254 bytes)
```

The test programs:

- `hello`: arguments, environment, `uname`, memory through `brk` and
  `mmap`, the clock, the process number and the thread pointer.
- `sandbox`: six calls Linux does not have, a bad pointer and an unknown
  descriptor; all get Linux's own answers.
- `crash`: prints, asks for memory, then touches memory it never asked for.

## Next

Processes (fork, execve, threads, futex, signals, pipes), a real C library
(musl), files in Linux home (a folder in your documents, with history and
undo), pseudo-terminals in the terminal, a simple network, and then Claude
Code (`/mnt/project-files/os-design/milestone-8b-plan.md`).
