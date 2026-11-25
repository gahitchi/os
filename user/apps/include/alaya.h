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

/* Windows from C (step 8-4): one window per program, a picture the
 * program draws itself (0x00RRGGBB pixels), and its events. The person
 * decides full screen (Alaya key + Up), never the program. */
#define ALAYA_EVENT_KEY 1     /* a: the key (text, or ALAYA_KEY_*), b: 1 with Ctrl */
#define ALAYA_EVENT_QUIT 2    /* the person closed the window */
#define ALAYA_EVENT_RESIZED 3 /* a, b: the new size in points */
#define ALAYA_EVENT_FOCUS 4   /* a: 1 in front, 0 not */
#define ALAYA_EVENT_POINTER 5 /* a, b: where a button went down */

#define ALAYA_KEY_ENTER 13
#define ALAYA_KEY_BACKSPACE 8
#define ALAYA_KEY_TAB 9
#define ALAYA_KEY_ESCAPE 27
#define ALAYA_KEY_DELETE 127
#define ALAYA_KEY_UP 0x110001
#define ALAYA_KEY_DOWN 0x110002
#define ALAYA_KEY_LEFT 0x110003
#define ALAYA_KEY_RIGHT 0x110004
#define ALAYA_KEY_HOME 0x110005
#define ALAYA_KEY_END 0x110006

uint64_t alaya_window_open(int32_t w, int32_t h, const uint8_t *title, size_t len);
uint64_t alaya_window_present(const uint32_t *px, int32_t w, int32_t h);
uint64_t alaya_window_size(int32_t *w, int32_t *h);
/* 1 and the event when there is one, 0 when there is none now. */
uint64_t alaya_window_event(uint32_t *kind, int32_t *a, int32_t *b);
/* Until an event comes or the program's clock ticks. */
void alaya_window_wait(void);

/* Saved games: saves are documents in "Saved games/<game>", so every
 * save is kept, and an older one can be read back. */
uint64_t alaya_saved_games(const uint8_t *game, size_t len, uint64_t *out);
/* Each moment the file changed, newest first: the moment and its time. */
uint64_t alaya_save_versions(uint64_t file, uint64_t *moments, uint64_t *times, size_t max,
                             size_t *got);
uint64_t alaya_read_past(uint64_t file, uint64_t moment, uint8_t *buf, size_t cap, size_t *got);

#endif
