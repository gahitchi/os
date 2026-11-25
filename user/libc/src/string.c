/* SPDX-License-Identifier: GPL-3.0-only WITH AdditionRef-Alaya-system-library-exception */
/* string.h. memcpy, memmove, memset and memcmp come from the Rust side
 * (compiler-builtins). */
#include <errno.h>
#include <stdlib.h>
#include <string.h>

void *memchr(const void *s, int c, size_t n) {
    const unsigned char *p = s;
    for (size_t i = 0; i < n; i++)
        if (p[i] == (unsigned char)c)
            return (void *)(p + i);
    return NULL;
}

size_t strlen(const char *s) {
    size_t n = 0;
    while (s[n])
        n++;
    return n;
}

size_t strnlen(const char *s, size_t max) {
    size_t n = 0;
    while (n < max && s[n])
        n++;
    return n;
}

int strcmp(const char *a, const char *b) {
    while (*a && *a == *b) {
        a++;
        b++;
    }
    return (unsigned char)*a - (unsigned char)*b;
}

int strncmp(const char *a, const char *b, size_t n) {
    for (; n; n--, a++, b++) {
        if (*a != *b || !*a)
            return (unsigned char)*a - (unsigned char)*b;
    }
    return 0;
}

char *strcpy(char *dst, const char *src) {
    char *d = dst;
    while ((*d++ = *src++))
        ;
    return dst;
}

char *strncpy(char *dst, const char *src, size_t n) {
    size_t i = 0;
    for (; i < n && src[i]; i++)
        dst[i] = src[i];
    for (; i < n; i++)
        dst[i] = 0;
    return dst;
}

char *strcat(char *dst, const char *src) {
    strcpy(dst + strlen(dst), src);
    return dst;
}

char *strncat(char *dst, const char *src, size_t n) {
    char *d = dst + strlen(dst);
    size_t i = 0;
    for (; i < n && src[i]; i++)
        d[i] = src[i];
    d[i] = 0;
    return dst;
}

char *strchr(const char *s, int c) {
    for (;; s++) {
        if (*s == (char)c)
            return (char *)s;
        if (!*s)
            return NULL;
    }
}

char *strrchr(const char *s, int c) {
    const char *last = NULL;
    for (;; s++) {
        if (*s == (char)c)
            last = s;
        if (!*s)
            return (char *)last;
    }
}

char *strstr(const char *hay, const char *needle) {
    size_t n = strlen(needle);
    if (!n)
        return (char *)hay;
    for (; *hay; hay++)
        if (*hay == *needle && !strncmp(hay, needle, n))
            return (char *)hay;
    return NULL;
}

size_t strspn(const char *s, const char *accept) {
    size_t n = 0;
    while (s[n] && strchr(accept, s[n]))
        n++;
    return n;
}

size_t strcspn(const char *s, const char *reject) {
    size_t n = 0;
    while (s[n] && !strchr(reject, s[n]))
        n++;
    return n;
}

char *strtok(char *s, const char *delim) {
    static char *next;
    if (!s)
        s = next;
    if (!s)
        return NULL;
    s += strspn(s, delim);
    if (!*s) {
        next = NULL;
        return NULL;
    }
    char *end = s + strcspn(s, delim);
    if (*end) {
        *end = 0;
        next = end + 1;
    } else {
        next = NULL;
    }
    return s;
}

char *strndup(const char *s, size_t n) {
    size_t len = strnlen(s, n);
    char *d = malloc(len + 1);
    if (!d)
        return NULL;
    memcpy(d, s, len);
    d[len] = 0;
    return d;
}

char *strdup(const char *s) { return strndup(s, strlen(s)); }

char *strerror(int err) {
    switch (err) {
    case 0: return "no error";
    case ENOENT: return "no such file (or it was not handed over)";
    case EIO: return "the store could not do it";
    case EBADF: return "not an open file";
    case ENOMEM: return "out of memory";
    case EACCES: return "not allowed";
    case EEXIST: return "already there";
    case EISDIR: return "a folder";
    case EINVAL: return "not a valid request";
    case EMFILE: return "too many open files";
    case ENOSPC: return "no room left";
    case ERANGE: return "out of range";
    case ENOSYS: return "not in Alaya";
    default: return "an error";
    }
}
