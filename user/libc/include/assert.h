/* SPDX-License-Identifier: GPL-3.0-only WITH AdditionRef-Alaya-system-library-exception */
#ifndef _ASSERT_H
#define _ASSERT_H

_Noreturn void __alaya_assert(const char *what, const char *file, int line);

#ifdef NDEBUG
#define assert(x) ((void)0)
#else
#define assert(x) ((x) ? (void)0 : __alaya_assert(#x, __FILE__, __LINE__))
#endif

#endif
