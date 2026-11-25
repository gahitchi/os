/* SPDX-License-Identifier: GPL-3.0-only WITH AdditionRef-Alaya-system-library-exception */
/* Files by number (unistd.h, fcntl.h) on Alaya's store. A file is read
 * whole when opened and written back whole when closed, if it changed:
 * the store keeps every version, so a program that crashes half way
 * leaves the file as it was. Standard output and errors go to whoever
 * runs the program (the terminal); standard input is empty. */
#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "libc.h"

#define MAX_FILES 16
#define FIRST 3

struct open_file {
    int used;
    int flags;
    uint64_t file;
    uint8_t *data;
    size_t size, cap, pos;
    int dirty;
};

static struct open_file files[MAX_FILES];

int __alaya_errno(uint64_t status) {
    switch (status) {
    case ALAYA_OK: return 0;
    case ALAYA_NOT_FOUND: return ENOENT;
    case ALAYA_EXISTS: return EEXIST;
    case ALAYA_FULL: return ENOSPC;
    case ALAYA_BAD_REQUEST: return EINVAL;
    default: return EIO;
    }
}

static int fail(uint64_t status) {
    errno = __alaya_errno(status);
    return -1;
}

/* "name", "/docs/name" or "/app/name": the folder and the name. */
static int resolve(const char *path, uint64_t *dir, const char **name) {
    uint64_t class_ = ALAYA_DOCUMENTS;
    if (!strncmp(path, "/docs/", 6)) {
        path += 6;
    } else if (!strncmp(path, "/app/", 5)) {
        path += 5;
        class_ = ALAYA_APP_FILES;
    }
    if (!*path || strchr(path, '/')) {
        errno = ENOENT;
        return -1;
    }
    uint64_t st = alaya_store_root(class_, dir);
    if (st)
        return fail(st);
    *name = path;
    return 0;
}

int open(const char *path, int flags, ...) {
    uint64_t dir, file;
    const char *name;
    if (resolve(path, &dir, &name))
        return -1;
    int fd = FIRST;
    while (fd < MAX_FILES && files[fd].used)
        fd++;
    if (fd == MAX_FILES) {
        errno = EMFILE;
        return -1;
    }
    size_t len = strlen(name);
    uint64_t st = alaya_store_lookup(dir, (const uint8_t *)name, len, &file);
    if (st == ALAYA_OK && (flags & O_CREAT) && (flags & O_EXCL)) {
        errno = EEXIST;
        return -1;
    }
    if (st == ALAYA_NOT_FOUND && (flags & O_CREAT))
        st = alaya_store_open(dir, (const uint8_t *)name, len, &file);
    if (st)
        return fail(st);
    struct open_file *f = &files[fd];
    memset(f, 0, sizeof *f);
    f->file = file;
    f->flags = flags;
    if (!(flags & O_TRUNC)) {
        uint64_t size;
        if ((st = alaya_store_size(file, &size)))
            return fail(st);
        f->data = malloc(size ? size : 1);
        if (!f->data) {
            errno = ENOMEM;
            return -1;
        }
        f->cap = size ? size : 1;
        if ((st = alaya_store_read(file, f->data, size, &f->size))) {
            free(f->data);
            return fail(st);
        }
    } else {
        f->dirty = 1;
    }
    f->used = 1;
    if (flags & O_APPEND)
        f->pos = f->size;
    return fd;
}

static struct open_file *get(int fd) {
    if (fd < FIRST || fd >= MAX_FILES || !files[fd].used) {
        errno = EBADF;
        return NULL;
    }
    return &files[fd];
}

ssize_t read(int fd, void *buf, size_t n) {
    if (fd == STDIN_FILENO)
        return 0;
    struct open_file *f = get(fd);
    if (!f)
        return -1;
    if ((f->flags & O_ACCMODE) == O_WRONLY) {
        errno = EBADF;
        return -1;
    }
    size_t left = f->pos < f->size ? f->size - f->pos : 0;
    if (n > left)
        n = left;
    memcpy(buf, f->data + f->pos, n);
    f->pos += n;
    return (ssize_t)n;
}

ssize_t write(int fd, const void *buf, size_t n) {
    if (fd == STDOUT_FILENO || fd == STDERR_FILENO) {
        alaya_out(fd, buf, n);
        return (ssize_t)n;
    }
    struct open_file *f = get(fd);
    if (!f)
        return -1;
    if ((f->flags & O_ACCMODE) == O_RDONLY) {
        errno = EBADF;
        return -1;
    }
    if (f->flags & O_APPEND)
        f->pos = f->size;
    size_t end = f->pos + n;
    if (end > f->cap) {
        size_t cap = f->cap * 2 > end ? f->cap * 2 : end;
        uint8_t *d = realloc(f->data, cap);
        if (!d) {
            errno = ENOMEM;
            return -1;
        }
        f->data = d;
        f->cap = cap;
    }
    if (f->pos > f->size)
        memset(f->data + f->size, 0, f->pos - f->size);
    memcpy(f->data + f->pos, buf, n);
    f->pos = end;
    if (end > f->size)
        f->size = end;
    f->dirty = 1;
    return (ssize_t)n;
}

off_t lseek(int fd, off_t off, int whence) {
    struct open_file *f = get(fd);
    if (!f)
        return -1;
    off_t base = whence == SEEK_SET ? 0 : whence == SEEK_CUR ? (off_t)f->pos : (off_t)f->size;
    if (whence > SEEK_END || base + off < 0) {
        errno = EINVAL;
        return -1;
    }
    f->pos = (size_t)(base + off);
    return (off_t)f->pos;
}

int close(int fd) {
    struct open_file *f = get(fd);
    if (!f)
        return -1;
    uint64_t st = 0;
    if (f->dirty) {
        st = alaya_store_write(f->file, f->data, f->size);
        if (!st)
            st = alaya_store_save();
    }
    free(f->data);
    memset(f, 0, sizeof *f);
    return st ? fail(st) : 0;
}

void __alaya_close_all(void) {
    for (int fd = FIRST; fd < MAX_FILES; fd++)
        if (files[fd].used)
            close(fd);
}

int unlink(const char *path) {
    uint64_t dir;
    const char *name;
    if (resolve(path, &dir, &name))
        return -1;
    uint64_t st = alaya_store_remove(dir, (const uint8_t *)name, strlen(name));
    return st ? fail(st) : 0;
}

unsigned sleep(unsigned seconds) {
    uint64_t until = alaya_now_ms() + (uint64_t)seconds * 1000;
    while (alaya_now_ms() < until)
        ;
    return 0;
}
