/* SPDX-License-Identifier: GPL-3.0-only */
/* stdio.h on top of the file numbers (fd.c), and printf's formatting. */
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "libc.h"

#define F_EOF 1
#define F_ERR 2

struct alaya_file {
    int fd;
    int state;
    int used;
};

#define STREAMS 16
static struct alaya_file streams[STREAMS] = {
    {STDIN_FILENO, 0, 1},
    {STDOUT_FILENO, 0, 1},
    {STDERR_FILENO, 0, 1},
};
FILE *stdin = &streams[0];
FILE *stdout = &streams[1];
FILE *stderr = &streams[2];

void __alaya_stdio_reset(void) {
    for (int i = 0; i < 3; i++)
        streams[i].state = 0;
    for (int i = 3; i < STREAMS; i++)
        streams[i].used = 0;
}

/* ---- Formatting ---- */

struct sink {
    char *buf;
    size_t cap, len;
    FILE *f;
};

static void emit(struct sink *s, const char *p, size_t n) {
    if (s->f) {
        fwrite(p, 1, n, s->f);
    } else if (s->len + 1 < s->cap) {
        size_t room = s->cap - 1 - s->len;
        memcpy(s->buf + s->len, p, n < room ? n : room);
    }
    s->len += n;
}

static void pad(struct sink *s, char c, int n) {
    while (n-- > 0)
        emit(s, &c, 1);
}

static int format(struct sink *s, const char *fmt, va_list ap) {
    for (; *fmt; fmt++) {
        if (*fmt != '%') {
            const char *start = fmt;
            while (fmt[1] && fmt[1] != '%')
                fmt++;
            emit(s, start, (size_t)(fmt - start + 1));
            continue;
        }
        fmt++;
        int left = 0, zero = 0, plus = 0, space = 0, alt = 0;
        for (;; fmt++) {
            if (*fmt == '-') left = 1;
            else if (*fmt == '0') zero = 1;
            else if (*fmt == '+') plus = 1;
            else if (*fmt == ' ') space = 1;
            else if (*fmt == '#') alt = 1;
            else break;
        }
        int width = 0, prec = -1;
        if (*fmt == '*') {
            width = va_arg(ap, int);
            if (width < 0) {
                left = 1;
                width = -width;
            }
            fmt++;
        } else {
            while (*fmt >= '0' && *fmt <= '9')
                width = width * 10 + (*fmt++ - '0');
        }
        if (*fmt == '.') {
            fmt++;
            prec = 0;
            if (*fmt == '*') {
                prec = va_arg(ap, int);
                fmt++;
            } else {
                while (*fmt >= '0' && *fmt <= '9')
                    prec = prec * 10 + (*fmt++ - '0');
            }
        }
        int size = 0; /* -2 hh, -1 h, 1 l, 2 ll, 3 z/j/t */
        if (*fmt == 'h') {
            size = -1;
            if (*++fmt == 'h') {
                size = -2;
                fmt++;
            }
        } else if (*fmt == 'l') {
            size = 1;
            if (*++fmt == 'l') {
                size = 2;
                fmt++;
            }
        } else if (*fmt == 'z' || *fmt == 'j' || *fmt == 't') {
            size = 3;
            fmt++;
        }
        char c = *fmt;
        if (!c)
            break;
        char tmp[72];
        const char *str = tmp;
        size_t n = 0;
        char sign = 0;
        const char *prefix = "";
        switch (c) {
        case 'd':
        case 'i': {
            long long v;
            if (size >= 2 || size == 1 || size == 3) v = size == 1 ? va_arg(ap, long) : va_arg(ap, long long);
            else v = va_arg(ap, int);
            if (size == -1) v = (short)v;
            if (size == -2) v = (signed char)v;
            unsigned long long u = v < 0 ? 0 - (unsigned long long)v : (unsigned long long)v;
            sign = v < 0 ? '-' : plus ? '+' : space ? ' ' : 0;
            char *e = tmp + sizeof tmp, *p = e;
            do *--p = (char)('0' + u % 10); while (u /= 10);
            if (prec == 0 && v == 0) p = e;
            str = p;
            n = (size_t)(e - p);
            break;
        }
        case 'u': case 'x': case 'X': case 'o': case 'p': {
            unsigned long long u;
            int base = c == 'o' ? 8 : c == 'u' ? 10 : 16;
            if (c == 'p') {
                u = (uintptr_t)va_arg(ap, void *);
                prefix = "0x";
            } else if (size == 1) u = va_arg(ap, unsigned long);
            else if (size >= 2) u = va_arg(ap, unsigned long long);
            else u = va_arg(ap, unsigned);
            if (size == -1) u = (unsigned short)u;
            if (size == -2) u = (unsigned char)u;
            const char *digits = c == 'X' ? "0123456789ABCDEF" : "0123456789abcdef";
            char *e = tmp + sizeof tmp, *p = e;
            unsigned long long w = u;
            do *--p = digits[w % base]; while (w /= base);
            if (prec == 0 && u == 0) p = e;
            if (alt && u && base == 16) prefix = c == 'X' ? "0X" : "0x";
            if (alt && base == 8 && *p != '0') *--p = '0';
            str = p;
            n = (size_t)(e - p);
            break;
        }
        case 'c':
            tmp[0] = (char)va_arg(ap, int);
            n = 1;
            prec = -1;
            break;
        case 's':
            str = va_arg(ap, const char *);
            if (!str) str = "(null)";
            n = prec >= 0 ? strnlen(str, (size_t)prec) : strlen(str);
            prec = -1;
            break;
        case '%':
            tmp[0] = '%';
            n = 1;
            prec = -1;
            break;
        case 'n':
            *va_arg(ap, int *) = (int)s->len;
            continue;
        default:
            /* Unknown: write it as it is. */
            tmp[0] = '%';
            tmp[1] = c;
            n = 2;
            prec = -1;
            break;
        }
        int zeros = prec > (int)n ? prec - (int)n : 0;
        int body = (int)n + zeros + (sign ? 1 : 0) + (int)strlen(prefix);
        int fill = width > body ? width - body : 0;
        if (!left && !(zero && prec < 0))
            pad(s, ' ', fill);
        if (sign)
            emit(s, &sign, 1);
        emit(s, prefix, strlen(prefix));
        if (!left && zero && prec < 0)
            pad(s, '0', fill);
        pad(s, '0', zeros);
        emit(s, str, n);
        if (left)
            pad(s, ' ', fill);
    }
    return (int)s->len;
}

int vsnprintf(char *out, size_t n, const char *fmt, va_list ap) {
    struct sink s = {out, n, 0, NULL};
    int len = format(&s, fmt, ap);
    if (n)
        out[s.len < n ? s.len : n - 1] = 0;
    return len;
}

int snprintf(char *out, size_t n, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int r = vsnprintf(out, n, fmt, ap);
    va_end(ap);
    return r;
}

int vsprintf(char *out, const char *fmt, va_list ap) { return vsnprintf(out, SIZE_MAX, fmt, ap); }

int sprintf(char *out, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int r = vsprintf(out, fmt, ap);
    va_end(ap);
    return r;
}

int vfprintf(FILE *f, const char *fmt, va_list ap) {
    struct sink s = {NULL, 0, 0, f};
    return format(&s, fmt, ap);
}

int fprintf(FILE *f, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int r = vfprintf(f, fmt, ap);
    va_end(ap);
    return r;
}

int vprintf(const char *fmt, va_list ap) { return vfprintf(stdout, fmt, ap); }

int printf(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int r = vfprintf(stdout, fmt, ap);
    va_end(ap);
    return r;
}

/* ---- Streams ---- */

FILE *fopen(const char *path, const char *mode) {
    int flags;
    switch (mode[0]) {
    case 'r': flags = O_RDONLY; break;
    case 'w': flags = O_WRONLY | O_CREAT | O_TRUNC; break;
    case 'a': flags = O_WRONLY | O_CREAT | O_APPEND; break;
    default: errno = EINVAL; return NULL;
    }
    if (strchr(mode, '+'))
        flags = (flags & ~O_ACCMODE) | O_RDWR;
    int i = 3;
    while (i < STREAMS && streams[i].used)
        i++;
    if (i == STREAMS) {
        errno = EMFILE;
        return NULL;
    }
    int fd = open(path, flags);
    if (fd < 0)
        return NULL;
    streams[i] = (struct alaya_file){fd, 0, 1};
    return &streams[i];
}

int fclose(FILE *f) {
    if (!f || !f->used)
        return EOF;
    int r = f->fd > STDERR_FILENO ? close(f->fd) : 0;
    if (f >= &streams[3])
        f->used = 0;
    return r ? EOF : 0;
}

int fflush(FILE *f) {
    (void)f;
    return 0;
}

size_t fwrite(const void *data, size_t size, size_t n, FILE *f) {
    if (!size || !n)
        return 0;
    ssize_t w = write(f->fd, data, size * n);
    if (w < 0) {
        f->state |= F_ERR;
        return 0;
    }
    return (size_t)w / size;
}

size_t fread(void *out, size_t size, size_t n, FILE *f) {
    if (!size || !n)
        return 0;
    ssize_t r = read(f->fd, out, size * n);
    if (r < 0) {
        f->state |= F_ERR;
        return 0;
    }
    if ((size_t)r < size * n)
        f->state |= F_EOF;
    return (size_t)r / size;
}

int fputc(int c, FILE *f) {
    unsigned char b = (unsigned char)c;
    return fwrite(&b, 1, 1, f) == 1 ? b : EOF;
}
int putc(int c, FILE *f) { return fputc(c, f); }
int putchar(int c) { return fputc(c, stdout); }

int fputs(const char *s, FILE *f) {
    size_t n = strlen(s);
    return fwrite(s, 1, n, f) == n ? 0 : EOF;
}

int puts(const char *s) {
    if (fputs(s, stdout) == EOF)
        return EOF;
    return fputc('\n', stdout) == EOF ? EOF : 0;
}

int fgetc(FILE *f) {
    unsigned char b;
    return fread(&b, 1, 1, f) == 1 ? b : EOF;
}
int getc(FILE *f) { return fgetc(f); }
int getchar(void) { return fgetc(stdin); }

char *fgets(char *out, int n, FILE *f) {
    int i = 0;
    while (i + 1 < n) {
        int c = fgetc(f);
        if (c == EOF)
            break;
        out[i++] = (char)c;
        if (c == '\n')
            break;
    }
    if (i == 0)
        return NULL;
    out[i] = 0;
    return out;
}

int fseek(FILE *f, long off, int whence) {
    if (lseek(f->fd, off, whence) < 0)
        return -1;
    f->state &= ~F_EOF;
    return 0;
}

long ftell(FILE *f) { return (long)lseek(f->fd, 0, SEEK_CUR); }
void rewind(FILE *f) {
    fseek(f, 0, SEEK_SET);
    f->state = 0;
}
int feof(FILE *f) { return f->state & F_EOF; }
int ferror(FILE *f) { return f->state & F_ERR; }
int fileno(FILE *f) { return f->fd; }
int remove(const char *path) { return unlink(path); }

void perror(const char *s) {
    if (s && *s)
        fprintf(stderr, "%s: %s\n", s, strerror(errno));
    else
        fprintf(stderr, "%s\n", strerror(errno));
}
