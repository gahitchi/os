/* SPDX-License-Identifier: GPL-3.0-only */
#ifndef _TIME_H
#define _TIME_H
#include <sys/types.h>

#define CLOCKS_PER_SEC 1000
typedef long clock_t;

struct timespec {
    time_t tv_sec;
    long tv_nsec;
};

#define CLOCK_MONOTONIC 1
#define CLOCK_REALTIME 0

/* Milliseconds since Alaya started, for both clocks (there is no wall
 * clock yet). */
clock_t clock(void);
int clock_gettime(int clock_id, struct timespec *ts);
time_t time(time_t *out);

#endif
