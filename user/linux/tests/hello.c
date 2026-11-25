/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * hello: the first Linux program in Alaya's Linux world (milestone 8b,
 * step 1). Prints only what is the same on any Linux machine, so its
 * output can be compared with a real Linux run (decision 166).
 */
#include "../rt/rt.h"

static int tls_word;

static int tls_works(void)
{
    static long block[4];
    block[0] = (long)block;
    block[1] = 0x4c696e7578; /* "Linux" */
#if defined(__x86_64__)
    if (sys2(NR_arch_prctl, 0x1002 /* ARCH_SET_FS */, (long)block) != 0)
        return 0;
    long v;
    __asm__ volatile("mov %%fs:8, %0" : "=r"(v));
#else
    __asm__ volatile("msr tpidr_el0, %0" ::"r"(block));
    long *p;
    __asm__ volatile("mrs %0, tpidr_el0" : "=r"(p));
    long v = p[1];
#endif
    tls_word = v == 0x4c696e7578;
    return tls_word;
}

int main(int argc, char **argv)
{
    rt_puts("hello from the Linux world\n");
    rt_puts("arguments: ");
    rt_putn(argc);
    for (int i = 1; i < argc; i++) {
        rt_puts(" ");
        rt_puts(argv[i]);
    }
    rt_puts("\n");
    char **env = argv + argc + 1;
    int lang = 0;
    for (; *env; env++)
        if (rt_streq(*env, "LANG=C.UTF-8"))
            lang = 1;
    rt_puts(lang ? "environment: LANG is C.UTF-8\n" : "environment: no LANG\n");

    struct utsname u;
    if (sys1(NR_uname, (long)&u) == 0)
        rt_puts(rt_streq(u.sysname, "Linux") ? "uname: Linux\n" : "uname: not Linux\n");

    /* Memory the old way: move the break. */
    char *b0 = (char *)sys1(NR_brk, 0);
    char *b1 = (char *)sys1(NR_brk, (long)(b0 + 65536));
    if (b1 == b0 + 65536) {
        for (int i = 0; i < 65536; i += 4096)
            b0[i] = (char)i;
        rt_puts("brk: 64 KiB more\n");
    } else {
        rt_puts("brk: refused\n");
    }

    /* And the new way: anonymous pages. */
    char *m = (char *)sys6(NR_mmap, 0, 3 * 4096, PROT_READ | PROT_WRITE,
                           MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if ((long)m < 0 && (long)m > -4096) {
        rt_puts("mmap: refused\n");
    } else {
        int zero = 1;
        for (int i = 0; i < 3 * 4096; i++)
            zero &= m[i] == 0;
        m[0] = 1;
        m[3 * 4096 - 1] = 2;
        rt_puts(zero ? "mmap: 3 zeroed pages\n" : "mmap: pages not zeroed\n");
        rt_puts(sys2(NR_munmap, (long)m, 3 * 4096) == 0 ? "munmap: ok\n" : "munmap: refused\n");
    }

    struct timespec t1, t2;
    sys2(NR_clock_gettime, CLOCK_MONOTONIC, (long)&t1);
    for (volatile int i = 0; i < 100000; i++)
        ;
    sys2(NR_clock_gettime, CLOCK_MONOTONIC, (long)&t2);
    int later = t2.sec > t1.sec || (t2.sec == t1.sec && t2.nsec >= t1.nsec);
    rt_puts(later ? "clock: goes forward\n" : "clock: went back\n");

    rt_puts(sys0(NR_getpid) > 0 ? "pid: yes\n" : "pid: no\n");
    rt_puts(tls_works() ? "thread pointer: works\n" : "thread pointer: broken\n");
    rt_puts("goodbye\n");
    return 0;
}
