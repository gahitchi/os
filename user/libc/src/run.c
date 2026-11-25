/* SPDX-License-Identifier: GPL-3.0-only */
/* Running a C program's main inside an Alaya program: exit() comes back
 * here, files left open are written back, and the next program starts
 * clean. Also the clock. */
#include <assert.h>
#include <errno.h>
#include <setjmp.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

#include "libc.h"

static jmp_buf *running;
static int exit_code;

int alaya_run_main(int (*main_fn)(int, char **), int argc, char **argv) {
    jmp_buf here;
    jmp_buf *outer = running;
    int code;
    errno = 0;
    __alaya_stdio_reset();
    running = &here;
    if (setjmp(here) == 0)
        code = main_fn(argc, argv);
    else
        code = exit_code;
    running = outer;
    fflush(stdout);
    __alaya_close_all();
    return code;
}

_Noreturn void exit(int code) {
    exit_code = code;
    if (running)
        longjmp(*running, 1);
    /* Not inside alaya_run_main: nothing to go back to. */
    for (;;)
        ;
}

_Noreturn void abort(void) {
    fputs("abort\n", stderr);
    exit(134);
}

_Noreturn void __alaya_assert(const char *what, const char *file, int line) {
    fprintf(stderr, "%s:%d: assertion failed: %s\n", file, line, what);
    abort();
}

clock_t clock(void) { return (clock_t)alaya_now_ms(); }

int clock_gettime(int clock_id, struct timespec *ts) {
    (void)clock_id;
    uint64_t ms = alaya_now_ms();
    ts->tv_sec = (time_t)(ms / 1000);
    ts->tv_nsec = (long)(ms % 1000) * 1000000;
    return 0;
}

time_t time(time_t *out) {
    time_t t = (time_t)(alaya_now_ms() / 1000);
    if (out)
        *out = t;
    return t;
}
