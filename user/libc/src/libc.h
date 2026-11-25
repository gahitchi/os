/* SPDX-License-Identifier: GPL-3.0-only WITH AdditionRef-Alaya-system-library-exception */
/* What the C library needs from Alaya: the SDK's C interface (alaya.h)
 * and a few calls only the library uses. */
#ifndef ALAYA_LIBC_INTERNAL_H
#define ALAYA_LIBC_INTERNAL_H
#include <stddef.h>
#include <stdint.h>
#include "../../apps/include/alaya.h"

/* Bytes for the program's standard output (fd 1) or errors (fd 2). */
void alaya_out(int fd, const uint8_t *data, size_t len);

/* errno from a store status. */
int __alaya_errno(uint64_t status);
/* Close every file the program left open (writing back what changed). */
void __alaya_close_all(void);
void __alaya_stdio_reset(void);

#endif
