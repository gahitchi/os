/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * sandbox: a Linux program knocks on doors that are not Linux's
 * (milestone 8b, step 1). Call numbers Linux does not have, and numbers
 * far beyond any Linux call, all get Linux's own "not implemented"
 * answer, on a real Linux machine and in Alaya's Linux world alike. In
 * Alaya, a thread that speaks Linux reaches none of Alaya's own calls
 * (decision 163).
 */
#include "../rt/rt.h"

int main(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    static const long knocks[] = {600, 1000, 4095, 0x10000, 0x4c0000, 0x7fffffff};
    int refused = 0;
    for (unsigned i = 0; i < sizeof knocks / sizeof knocks[0]; i++) {
        long r = sys6(knocks[i], 1, 2, 3, 4, 5, 6);
        if (r == -ENOSYS)
            refused++;
    }
    rt_puts("unknown calls refused: ");
    rt_putn(refused);
    rt_puts(" of 6\n");
    /* A bad pointer is an error, not a crash. */
    long r = sys3(NR_write, 1, 16, 5);
    rt_puts(r == -14 ? "bad pointer: EFAULT\n" : "bad pointer: something else\n");
    /* A file descriptor nobody opened. */
    r = sys3(NR_write, 77, (long)"x", 1);
    rt_puts(r == -9 ? "unknown descriptor: EBADF\n" : "unknown descriptor: something else\n");
    return 3;
}
