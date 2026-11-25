/* SPDX-License-Identifier: GPL-3.0-only */
#ifndef _SETJMP_H
#define _SETJMP_H

/* Callee-saved registers, the stack and the return address (x86-64: 8
 * words; AArch64: 22). */
typedef unsigned long jmp_buf[24];

int setjmp(jmp_buf env);
_Noreturn void longjmp(jmp_buf env, int value);

#endif
