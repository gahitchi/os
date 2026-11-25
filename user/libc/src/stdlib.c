/* SPDX-License-Identifier: GPL-3.0-only WITH AdditionRef-Alaya-system-library-exception */
/* stdlib.h. malloc, calloc, realloc and free are on the Rust side
 * (Alaya's allocator). */
#include <ctype.h>
#include <errno.h>
#include <limits.h>
#include <stdlib.h>
#include <string.h>

int errno;

static unsigned long long to_ull(const char *s, char **end, int base, int *neg, int *ok) {
    const char *p = s;
    while (isspace((unsigned char)*p))
        p++;
    *neg = 0;
    if (*p == '-' || *p == '+')
        *neg = *p++ == '-';
    if ((base == 0 || base == 16) && p[0] == '0' && (p[1] == 'x' || p[1] == 'X') &&
        isxdigit((unsigned char)p[2])) {
        p += 2;
        base = 16;
    } else if (base == 0 && p[0] == '0') {
        base = 8;
    } else if (base == 0) {
        base = 10;
    }
    unsigned long long v = 0;
    int any = 0, over = 0;
    for (;; p++) {
        int d;
        if (isdigit((unsigned char)*p))
            d = *p - '0';
        else if (isalpha((unsigned char)*p))
            d = tolower((unsigned char)*p) - 'a' + 10;
        else
            break;
        if (d >= base)
            break;
        if (v > (ULLONG_MAX - (unsigned)d) / (unsigned)base)
            over = 1;
        v = v * base + d;
        any = 1;
    }
    if (end)
        *end = (char *)(any ? p : s);
    *ok = !over;
    return over ? ULLONG_MAX : v;
}

long long strtoll(const char *s, char **end, int base) {
    int neg, ok;
    unsigned long long v = to_ull(s, end, base, &neg, &ok);
    if (!ok || (!neg && v > LLONG_MAX) || (neg && v > (unsigned long long)LLONG_MAX + 1)) {
        errno = ERANGE;
        return neg ? LLONG_MIN : LLONG_MAX;
    }
    return neg ? (long long)(0 - v) : (long long)v;
}

unsigned long long strtoull(const char *s, char **end, int base) {
    int neg, ok;
    unsigned long long v = to_ull(s, end, base, &neg, &ok);
    if (!ok) {
        errno = ERANGE;
        return ULLONG_MAX;
    }
    return neg ? 0 - v : v;
}

long strtol(const char *s, char **end, int base) { return (long)strtoll(s, end, base); }
unsigned long strtoul(const char *s, char **end, int base) { return (unsigned long)strtoull(s, end, base); }
int atoi(const char *s) { return (int)strtol(s, NULL, 10); }
long atol(const char *s) { return strtol(s, NULL, 10); }
long long atoll(const char *s) { return strtoll(s, NULL, 10); }
int abs(int x) { return x < 0 ? -x : x; }
long labs(long x) { return x < 0 ? -x : x; }
long long llabs(long long x) { return x < 0 ? -x : x; }

static void swap(char *a, char *b, size_t size) {
    while (size--) {
        char t = *a;
        *a++ = *b;
        *b++ = t;
    }
}

/* Heap sort: no recursion, no extra memory, n log n always. */
static void sift(char *base, size_t start, size_t n, size_t size, int (*cmp)(const void *, const void *)) {
    size_t root = start;
    while (2 * root + 1 < n) {
        size_t child = 2 * root + 1, top = root;
        if (cmp(base + top * size, base + child * size) < 0)
            top = child;
        if (child + 1 < n && cmp(base + top * size, base + (child + 1) * size) < 0)
            top = child + 1;
        if (top == root)
            return;
        swap(base + root * size, base + top * size, size);
        root = top;
    }
}

void qsort(void *b, size_t n, size_t size, int (*cmp)(const void *, const void *)) {
    char *base = b;
    if (n < 2)
        return;
    for (size_t i = n / 2; i-- > 0;)
        sift(base, i, n, size, cmp);
    for (size_t end = n - 1; end > 0; end--) {
        swap(base, base + end * size, size);
        sift(base, 0, end, size, cmp);
    }
}

void *bsearch(const void *key, const void *b, size_t n, size_t size,
              int (*cmp)(const void *, const void *)) {
    const char *base = b;
    size_t lo = 0, hi = n;
    while (lo < hi) {
        size_t mid = lo + (hi - lo) / 2;
        int c = cmp(key, base + mid * size);
        if (c == 0)
            return (void *)(base + mid * size);
        if (c < 0)
            hi = mid;
        else
            lo = mid + 1;
    }
    return NULL;
}

static unsigned long long seed = 1;
void srand(unsigned s) { seed = s; }
int rand(void) {
    seed = seed * 6364136223846793005ULL + 1442695040888963407ULL;
    return (int)(seed >> 33);
}

/* There are no environment variables in Alaya. */
char *getenv(const char *name) {
    (void)name;
    return NULL;
}
