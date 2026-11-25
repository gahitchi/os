/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * crash: a Linux program that does a few ordinary things and then
 * touches memory it never asked for (milestone 8b, step 1). On Linux it
 * dies of a segmentation fault; in Alaya the Linux world says in plain
 * words what it was doing just before (decision 165), and nothing else
 * is disturbed.
 */
#include "../rt/rt.h"

int main(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    rt_puts("about to crash\n");
    char *m = (char *)sys6(NR_mmap, 0, 4096, PROT_READ | PROT_WRITE,
                           MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    m[0] = 1;
    sys0(NR_getpid);
    volatile int *nowhere = (volatile int *)16;
    *nowhere = 1;
    rt_puts("still here\n");
    return 0;
}
