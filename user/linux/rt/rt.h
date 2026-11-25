/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * A tiny runtime for the Linux test programs (milestone 8b, step 1).
 *
 * These are ordinary Linux programs: static, no C library, talking to
 * the kernel with Linux's own system calls. The same file runs on a real
 * Linux machine and inside Alaya's Linux world, and the two outputs must
 * match (decision 166).
 */
#ifndef ALAYA_LINUX_RT_H
#define ALAYA_LINUX_RT_H

typedef unsigned long size_t;
typedef long ssize_t;
typedef unsigned long uintptr_t;

#if defined(__x86_64__)
enum {
    NR_read = 0, NR_write = 1, NR_close = 3, NR_mmap = 9, NR_munmap = 11,
    NR_brk = 12, NR_getpid = 39, NR_uname = 63, NR_arch_prctl = 158,
    NR_clock_gettime = 228, NR_exit_group = 231, NR_openat = 257,
};
static inline long sys6(long n, long a, long b, long c, long d, long e, long f)
{
    register long r10 __asm__("r10") = d;
    register long r8 __asm__("r8") = e;
    register long r9 __asm__("r9") = f;
    long ret;
    __asm__ volatile("syscall"
                     : "=a"(ret)
                     : "a"(n), "D"(a), "S"(b), "d"(c), "r"(r10), "r"(r8), "r"(r9)
                     : "rcx", "r11", "memory");
    return ret;
}
#elif defined(__aarch64__)
enum {
    NR_read = 63, NR_write = 64, NR_close = 57, NR_mmap = 222, NR_munmap = 215,
    NR_brk = 214, NR_getpid = 172, NR_uname = 160, NR_clock_gettime = 113,
    NR_exit_group = 94, NR_openat = 56,
};
static inline long sys6(long n, long a, long b, long c, long d, long e, long f)
{
    register long x8 __asm__("x8") = n;
    register long x0 __asm__("x0") = a;
    register long x1 __asm__("x1") = b;
    register long x2 __asm__("x2") = c;
    register long x3 __asm__("x3") = d;
    register long x4 __asm__("x4") = e;
    register long x5 __asm__("x5") = f;
    __asm__ volatile("svc #0"
                     : "+r"(x0)
                     : "r"(x8), "r"(x1), "r"(x2), "r"(x3), "r"(x4), "r"(x5)
                     : "memory");
    return x0;
}
#else
#error "no Linux runtime for this processor"
#endif

static inline long sys0(long n) { return sys6(n, 0, 0, 0, 0, 0, 0); }
static inline long sys1(long n, long a) { return sys6(n, a, 0, 0, 0, 0, 0); }
static inline long sys2(long n, long a, long b) { return sys6(n, a, b, 0, 0, 0, 0); }
static inline long sys3(long n, long a, long b, long c) { return sys6(n, a, b, c, 0, 0, 0); }

#define AT_FDCWD (-100)
#define O_RDONLY 0
#define O_WRONLY 1
#define O_CREAT 0100
#define O_TRUNC 01000
#define PROT_READ 1
#define PROT_WRITE 2
#define MAP_PRIVATE 2
#define MAP_ANONYMOUS 0x20
#define CLOCK_MONOTONIC 1
#define ENOSYS 38

struct timespec {
    long sec;
    long nsec;
};

struct utsname {
    char sysname[65], nodename[65], release[65], version[65], machine[65], domain[65];
};

/* The program's main, called by the runtime's start. */
int main(int argc, char **argv);

size_t rt_strlen(const char *s);
void rt_puts(const char *s);
void rt_putn(long n);
int rt_streq(const char *a, const char *b);
void rt_cat(char *dst, const char *a, const char *b);
__attribute__((noreturn)) void rt_exit(int code);

#endif
