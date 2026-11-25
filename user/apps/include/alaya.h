/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * Alaya's app SDK, the small C interface (milestone 8, step 3).
 *
 * The same calls Rust apps make, for programs written in C and for the
 * POSIX layer to come. Every function returns a status: ALAYA_OK (0) or
 * one of the store's status words. Results come back through pointers,
 * which may be NULL when not wanted. The functions are in
 * user/apps/src/c.rs; `cargo xtask image` checks that this header lists
 * exactly those.
 */
#ifndef ALAYA_H
#define ALAYA_H

#include <stddef.h>
#include <stdint.h>

#define ALAYA_OK 0
#define ALAYA_NOT_FOUND 1
#define ALAYA_EXISTS 2
#define ALAYA_FULL 3
#define ALAYA_BAD_REQUEST 7
#define ALAYA_USED_UP 11

/* Classes of files, for alaya_store_root. */
#define ALAYA_DOCUMENTS 0
#define ALAYA_APP_FILES 1

uint64_t alaya_now_ms(void);
void alaya_say(const uint8_t *text, size_t len);

uint64_t alaya_store_root(uint64_t class_, uint64_t *out);
uint64_t alaya_store_open(uint64_t dir, const uint8_t *name, size_t len, uint64_t *out);
uint64_t alaya_store_size(uint64_t file, uint64_t *out);
uint64_t alaya_store_read(uint64_t file, uint8_t *buf, size_t cap, size_t *got);
uint64_t alaya_store_write(uint64_t file, const uint8_t *data, size_t len);
uint64_t alaya_store_save(void);
/* A file that is there, without making it. */
uint64_t alaya_store_lookup(uint64_t dir, const uint8_t *name, size_t len, uint64_t *out);
/* Take a file out of its folder. */
uint64_t alaya_store_remove(uint64_t dir, const uint8_t *name, size_t len);
/* The names in a folder that this program may see, one a line, folders
 * ending in '/'; `got` is how many bytes. */
uint64_t alaya_store_list(uint64_t dir, uint8_t *buf, size_t cap, size_t *got);

/* Answers instead of data: one number about a document the person
 * picked, never the document. */
uint64_t alaya_answer(uint64_t file, int64_t *value, uint64_t *left);

#endif
