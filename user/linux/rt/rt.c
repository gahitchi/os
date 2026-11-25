/* SPDX-License-Identifier: GPL-3.0-only */
/* The Linux test programs' start and a few helpers (see rt.h). */
#include "rt.h"

void *memset(void *d, int c, size_t n)
{
    unsigned char *p = d;
    while (n--)
        *p++ = (unsigned char)c;
    return d;
}

void *memcpy(void *d, const void *s, size_t n)
{
    unsigned char *p = d;
    const unsigned char *q = s;
    while (n--)
        *p++ = *q++;
    return d;
}

size_t rt_strlen(const char *s)
{
    size_t n = 0;
    while (s[n])
        n++;
    return n;
}

void rt_puts(const char *s) { sys3(NR_write, 1, (long)s, (long)rt_strlen(s)); }

void rt_putn(long n)
{
    char b[24];
    int i = 23;
    int neg = n < 0;
    unsigned long u = neg ? -(unsigned long)n : (unsigned long)n;
    b[i] = 0;
    do {
        b[--i] = (char)('0' + u % 10);
        u /= 10;
    } while (u);
    if (neg)
        b[--i] = '-';
    rt_puts(b + i);
}

int rt_streq(const char *a, const char *b)
{
    while (*a && *a == *b)
        a++, b++;
    return *a == *b;
}

void rt_cat(char *dst, const char *a, const char *b)
{
    while (*a)
        *dst++ = *a++;
    while (*b)
        *dst++ = *b++;
    *dst = 0;
}

void rt_exit(int code)
{
    for (;;)
        sys1(NR_exit_group, code);
}

__attribute__((used)) void rt_start_c(long *sp)
{
    int argc = (int)sp[0];
    char **argv = (char **)(sp + 1);
    rt_exit(main(argc, argv));
}

#if defined(__x86_64__)
__asm__(".globl _start\n"
        "_start:\n"
        "  xor %rbp, %rbp\n"
        "  mov %rsp, %rdi\n"
        "  and $-16, %rsp\n"
        "  call rt_start_c\n"
        "  hlt\n");
#else
__asm__(".globl _start\n"
        "_start:\n"
        "  mov x29, #0\n"
        "  mov x30, #0\n"
        "  mov x0, sp\n"
        "  bl rt_start_c\n"
        "  brk #0\n");
#endif
