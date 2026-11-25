/* SPDX-License-Identifier: GPL-3.0-only */
#ifndef _FCNTL_H
#define _FCNTL_H

#define O_RDONLY 0
#define O_WRONLY 1
#define O_RDWR 2
#define O_ACCMODE 3
#define O_CREAT 0100
#define O_EXCL 0200
#define O_TRUNC 01000
#define O_APPEND 02000

/* Paths: "name" or "/docs/name" is a document, "/app/name" one of the
 * program's own files. Nothing else exists. */
int open(const char *path, int flags, ...);

#endif
