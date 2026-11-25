/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * Glowroot, a small puzzle game made for Alaya (milestone 8, step 4).
 *
 * A firefly lights a dark garden one tile at a time. It can only fly to a
 * tile that is still dark, so the whole garden has to be lit in one path.
 * Each garden is grown from a random walk, so there is always a way.
 *
 * Arrows fly, U takes the last step back, R starts the garden again, S
 * saves, H shows every save (Enter brings one back, Escape goes back).
 *
 * It is written in plain C on the SDL subset of Alaya's C library, and its
 * saves are documents in "Saved games/Glowroot": every save stays in the
 * garden's history, and bringing an old one back never loses the newer
 * ones (decision 161).
 */
#include <SDL.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "../src/libc.h"

#define GW 15
#define GH 9
#define CELL 16
#define SW (GW * CELL)
#define SH (GH * CELL + 16)
#define ZOOM 2
#define CELLS (GW * GH)
#define SAVES_SHOWN 8

enum { STONE = 0, DARK = 1, LIT = 2 };

static uint8_t tile[CELLS];
static int path[CELLS];
static int steps;
static int level = 1;
static uint32_t screen[SW * SH];
static char note[48];

/* The save's folder and file, once made. */
static uint64_t save_file;

/* The save history screen. */
static int showing_saves;
static uint64_t moments[SAVES_SHOWN], times[SAVES_SHOWN];
static size_t saves_n;
static int chosen;

static void say(const char *s) {
    char line[96];
    int n = snprintf(line, sizeof line, "glowroot: %s", s);
    alaya_say((const uint8_t *)line, (size_t)n);
}

/* --- The garden ----------------------------------------------------- */

static uint32_t seed;
static uint32_t next_random(void) {
    seed ^= seed << 13;
    seed ^= seed >> 17;
    seed ^= seed << 5;
    return seed;
}

static int neighbour(int c, int d) {
    int x = c % GW, y = c / GW;
    switch (d) {
    case 0: return y > 0 ? c - GW : -1;
    case 1: return x < GW - 1 ? c + 1 : -1;
    case 2: return y < GH - 1 ? c + GW : -1;
    default: return x > 0 ? c - 1 : -1;
    }
}

/* Grow garden `lv`: a walk that never crosses itself; what it does not
 * reach is stone. The firefly starts where the walk did. */
static void grow(int lv) {
    seed = 0x9e3779b9u ^ (uint32_t)lv * 2654435761u;
    if (!seed) seed = 1;
    memset(tile, STONE, sizeof tile);
    int length = 10 + 6 * lv;
    if (length > CELLS) length = CELLS;
    int c = (int)(next_random() % CELLS);
    tile[c] = DARK;
    path[0] = c;
    for (int i = 1; i < length; i++) {
        int open[4], n = 0;
        for (int d = 0; d < 4; d++) {
            int m = neighbour(c, d);
            if (m >= 0 && tile[m] == STONE) open[n++] = m;
        }
        if (!n) break;
        c = open[next_random() % (uint32_t)n];
        tile[c] = DARK;
    }
    tile[path[0]] = LIT;
    steps = 0;
}

static int count(int kind) {
    int n = 0;
    for (int i = 0; i < CELLS; i++) n += tile[i] == kind;
    return n;
}

static int fly(int d) {
    int m = neighbour(path[steps], d);
    if (m < 0 || tile[m] != DARK) return 0;
    tile[m] = LIT;
    path[++steps] = m;
    return 1;
}

static void step_back(void) {
    if (steps == 0) return;
    tile[path[steps]] = DARK;
    steps--;
}

static int stuck(void) {
    if (count(DARK) == 0) return 0;
    for (int d = 0; d < 4; d++) {
        int m = neighbour(path[steps], d);
        if (m >= 0 && tile[m] == DARK) return 0;
    }
    return 1;
}

/* --- Saves ---------------------------------------------------------- */

static int open_save(void) {
    if (save_file) return 1;
    uint64_t dir;
    if (alaya_saved_games((const uint8_t *)"Glowroot", 8, &dir) != ALAYA_OK) return 0;
    return alaya_store_open(dir, (const uint8_t *)"save.txt", 8, &save_file) == ALAYA_OK;
}

/* The save is plain text, so the person can read it too. */
static int save_text(char *out, size_t cap) {
    int n = snprintf(out, cap, "glowroot save 1\nlevel %d\npath", level);
    for (int i = 0; i <= steps && (size_t)n + 8 < cap; i++) n += snprintf(out + n, cap - (size_t)n, " %d", path[i]);
    n += snprintf(out + n, cap - (size_t)n, "\n");
    return n;
}

static void save(void) {
    static char text[1024];
    int n = save_text(text, sizeof text);
    if (!open_save() || alaya_store_write(save_file, (const uint8_t *)text, (size_t)n) != ALAYA_OK ||
        alaya_store_save() != ALAYA_OK) {
        snprintf(note, sizeof note, "COULD NOT SAVE");
        say("the save failed");
        return;
    }
    snprintf(note, sizeof note, "SAVED");
    char line[64];
    snprintf(line, sizeof line, "saved garden %d, %d lit", level, steps + 1);
    say(line);
}

/* Read a save; 0 when it is not one this game made. */
static int load_text(const char *text) {
    int lv;
    if (strncmp(text, "glowroot save 1\nlevel ", 22) != 0) return 0;
    lv = (int)strtol(text + 22, NULL, 10);
    const char *p = strstr(text, "\npath");
    if (lv < 1 || lv > 999 || !p) return 0;
    p += 5;
    grow(lv);
    char *end;
    int first = (int)strtol(p, &end, 10);
    if (end == p || first != path[0]) return 0;
    p = end;
    for (;;) {
        int c = (int)strtol(p, &end, 10);
        if (end == p) break;
        p = end;
        int ok = 0;
        for (int d = 0; d < 4 && !ok; d++)
            if (neighbour(path[steps], d) == c) ok = fly(d);
        if (!ok) return 0;
    }
    level = lv;
    return 1;
}

static void list_saves(void) {
    saves_n = 0;
    if (open_save()) alaya_save_versions(save_file, moments, times, SAVES_SHOWN, &saves_n);
    /* The same garden saved twice in a row (the store may keep both) is
       shown once: each line on the list is a different garden. */
    static char a[1024], b[1024];
    size_t kept = 0, got_a = 0;
    for (size_t i = 0; i < saves_n; i++) {
        size_t got_b = 0;
        if (alaya_read_past(save_file, moments[i], (uint8_t *)b, sizeof b, &got_b) != ALAYA_OK) got_b = (size_t)-1;
        if (kept > 0 && got_b != (size_t)-1 && got_b == got_a && memcmp(a, b, got_b) == 0) continue;
        moments[kept] = moments[i];
        times[kept] = times[i];
        kept++;
        if (got_b != (size_t)-1) memcpy(a, b, got_b);
        got_a = got_b;
    }
    saves_n = kept;
    chosen = 0;
    char line[48];
    snprintf(line, sizeof line, "%d saves kept", (int)saves_n);
    say(line);
}

static void bring_back(int i) {
    static char text[1024];
    size_t got = 0;
    if (i < 0 || (size_t)i >= saves_n ||
        alaya_read_past(save_file, moments[i], (uint8_t *)text, sizeof text - 1, &got) != ALAYA_OK) {
        snprintf(note, sizeof note, "NOT BROUGHT BACK");
        return;
    }
    text[got] = 0;
    int keep_level = level;
    if (!load_text(text)) {
        grow(keep_level);
        snprintf(note, sizeof note, "NOT A GLOWROOT SAVE");
        return;
    }
    uint64_t t = times[i];
    snprintf(note, sizeof note, "BACK TO %d:%02d:%02d", (int)(t / 3600), (int)(t / 60 % 60), (int)(t % 60));
    char line[64];
    snprintf(line, sizeof line, "brought back the save of moment %d: garden %d, %d lit", (int)moments[i], level, steps + 1);
    say(line);
}

/* --- Drawing -------------------------------------------------------- */

/* A 3 by 5 font: digits, capitals and a few signs, 15 bits each. */
static const char glyph_chars[] = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ/:-.>";
static const uint16_t glyphs[] = {
    0x7B6F, 0x2C97, 0x73E7, 0x73CF, 0x5BC9, 0x79CF, 0x79EF, 0x7249, 0x7BEF, 0x7BCF,
    0x2BED, 0x6BAE, 0x3923, 0x6B6E, 0x79A7, 0x79A4, 0x396B, 0x5BED, 0x7497, 0x126A,
    0x5BAD, 0x4927, 0x5FED, 0x6B6D, 0x2B6A, 0x6BA4, 0x2B73, 0x6BAD, 0x388E, 0x7492,
    0x5B6F, 0x5B6A, 0x5BFD, 0x5AAD, 0x5A92, 0x72A7, 0x12A4, 0x0410, 0x01C0, 0x0002,
    0x4454,
};

static void rect(int x, int y, int w, int h, uint32_t c) {
    for (int j = y; j < y + h; j++)
        for (int i = x; i < x + w; i++)
            if (i >= 0 && i < SW && j >= 0 && j < SH) screen[j * SW + i] = c;
}

static void text(int x, int y, const char *s, uint32_t c) {
    for (; *s; s++, x += 4) {
        const char *at = strchr(glyph_chars, *s);
        if (!at || *s == 0) continue;
        uint16_t g = glyphs[at - glyph_chars];
        for (int r = 0; r < 5; r++)
            for (int k = 0; k < 3; k++)
                if (g >> (14 - r * 3 - k) & 1) rect(x + k, y + r, 1, 1, c);
    }
}

static void draw(SDL_Surface *s) {
    rect(0, 0, SW, SH, 0x0B1020);
    for (int i = 0; i < CELLS; i++) {
        int x = i % GW * CELL, y = 16 + i / GW * CELL;
        switch (tile[i]) {
        case STONE: rect(x + 1, y + 1, CELL - 2, CELL - 2, 0x262B36); break;
        case DARK: rect(x + 1, y + 1, CELL - 2, CELL - 2, 0x17223D); break;
        default:
            rect(x + 1, y + 1, CELL - 2, CELL - 2, 0x6B4E16);
            rect(x + 4, y + 4, CELL - 8, CELL - 8, 0xF2C14E);
        }
    }
    int f = path[steps];
    int fx = f % GW * CELL, fy = 16 + f / GW * CELL;
    rect(fx + 3, fy + 3, CELL - 6, CELL - 6, 0x1B1405);
    rect(fx + 5, fy + 5, CELL - 10, CELL - 10, 0xFFF6A0);

    char hud[64];
    snprintf(hud, sizeof hud, "GARDEN %d  LIT %d/%d", level, count(LIT), count(LIT) + count(DARK));
    text(4, 5, hud, 0xE8E4D8);
    text(SW - 4 * 19, 5, note[0] ? note : "S SAVE  H SAVES", note[0] ? 0xF2C14E : 0x8A93A6);
    if (stuck()) text(4 + 4 * 22, 5, "STUCK: U OR R", 0xE07A5F);

    if (showing_saves) {
        rect(30, 24, SW - 60, SH - 36, 0x0E1528);
        text(38, 30, "SAVES, NEWEST FIRST", 0xE8E4D8);
        if (!saves_n) text(38, 44, "NONE YET: S SAVES", 0x8A93A6);
        for (size_t i = 0; i < saves_n; i++) {
            char row[40];
            uint64_t t = times[i];
            snprintf(row, sizeof row, "%c %d  %d:%02d:%02d%s", (int)i == chosen ? '>' : ' ', (int)(i + 1),
                     (int)(t / 3600), (int)(t / 60 % 60), (int)(t % 60), i == 0 ? "  NOW" : "");
            text(38, 44 + (int)i * 10, row, (int)i == chosen ? 0xF2C14E : 0xC8CCD6);
        }
    }

    /* Into the window's surface, twice the size. */
    uint32_t *out = (uint32_t *)s->pixels;
    for (int y = 0; y < s->h; y++)
        for (int x = 0; x < s->w; x++) {
            int sx = x / ZOOM, sy = y / ZOOM;
            out[y * (s->pitch / 4) + x] = sx < SW && sy < SH ? screen[sy * SW + sx] : 0;
        }
}

/* --- The game ------------------------------------------------------- */

static void key(SDL_Keycode k) {
    if (showing_saves) {
        if (k == SDLK_UP && chosen > 0) chosen--;
        else if (k == SDLK_DOWN && (size_t)chosen + 1 < saves_n) chosen++;
        else if (k == SDLK_RETURN) {
            showing_saves = 0;
            bring_back(chosen);
        } else if (k == SDLK_ESCAPE || k == 'h') showing_saves = 0;
        return;
    }
    note[0] = 0;
    switch (k) {
    case SDLK_UP: fly(0); break;
    case SDLK_RIGHT: fly(1); break;
    case SDLK_DOWN: fly(2); break;
    case SDLK_LEFT: fly(3); break;
    case 'u': step_back(); break;
    case 'r': grow(level); break;
    case 's': save(); break;
    case 'h':
        list_saves();
        showing_saves = 1;
        break;
    default: return;
    }
    if (count(DARK) == 0) {
        char line[48];
        snprintf(line, sizeof line, "garden %d lit", level);
        say(line);
        level++;
        grow(level);
        snprintf(note, sizeof note, "GARDEN %d", level);
    }
}

int glowroot_main(int argc, char **argv) {
    (void)argc;
    (void)argv;
    if (SDL_Init(SDL_INIT_VIDEO) != 0) return 1;
    SDL_Window *w = SDL_CreateWindow("Glowroot", SDL_WINDOWPOS_UNDEFINED, SDL_WINDOWPOS_UNDEFINED, SW * ZOOM, SH * ZOOM,
                                     SDL_WINDOW_SHOWN);
    if (!w) {
        say(SDL_GetError());
        return 1;
    }
    SDL_Surface *s = SDL_GetWindowSurface(w);
    grow(level);
    /* Go on from the last save, if there is one. */
    static char text[1024];
    size_t got = 0;
    if (open_save() && alaya_store_read(save_file, (uint8_t *)text, sizeof text - 1, &got) == ALAYA_OK && got) {
        text[got] = 0;
        if (load_text(text)) snprintf(note, sizeof note, "WELCOME BACK");
        else grow(level = 1);
    }
    say("ready");
    for (;;) {
        draw(s);
        SDL_UpdateWindowSurface(w);
        SDL_Event e;
        SDL_WaitEvent(&e);
        if (e.type == SDL_QUIT) break;
        if (e.type == SDL_KEYDOWN) key(e.key.keysym.sym);
    }
    SDL_DestroyWindow(w);
    SDL_Quit();
    return 0;
}
