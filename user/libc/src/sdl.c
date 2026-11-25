/* SPDX-License-Identifier: GPL-3.0-only WITH AdditionRef-Alaya-system-library-exception */
/* The SDL subset (SDL.h) on Alaya's windows from C (alaya.h): one window,
 * a software surface of 32-bit pixels, key events, ticks and delays. */
#include <SDL.h>
#include <stdlib.h>
#include <string.h>

#include "libc.h"

struct SDL_Window {
    SDL_Surface surface;
    int open;
};

static SDL_PixelFormat rgb888 = {0x16161804u, 32, 4};
static SDL_Window the_window;
static const char *error = "";

int SDL_Init(Uint32 flags) {
    (void)flags;
    return 0;
}

void SDL_Quit(void) {
    if (the_window.open) SDL_DestroyWindow(&the_window);
}

const char *SDL_GetError(void) { return error; }

SDL_Window *SDL_CreateWindow(const char *title, int x, int y, int w, int h, Uint32 flags) {
    (void)x;
    (void)y;
    (void)flags;
    if (the_window.open) {
        error = "Alaya gives a program one window";
        return NULL;
    }
    if (w <= 0 || h <= 0 || w > 1280 || h > 800) {
        error = "the window is too large";
        return NULL;
    }
    if (alaya_window_open(w, h, (const uint8_t *)title, strlen(title)) != ALAYA_OK) {
        error = "Alaya did not open the window";
        return NULL;
    }
    void *px = calloc((size_t)w * (size_t)h, 4);
    if (!px) {
        error = "out of memory";
        return NULL;
    }
    the_window.surface.flags = 0;
    the_window.surface.format = &rgb888;
    the_window.surface.w = w;
    the_window.surface.h = h;
    the_window.surface.pitch = w * 4;
    the_window.surface.pixels = px;
    the_window.open = 1;
    return &the_window;
}

void SDL_DestroyWindow(SDL_Window *window) {
    if (window != &the_window || !the_window.open) return;
    free(the_window.surface.pixels);
    the_window.surface.pixels = NULL;
    the_window.open = 0;
}

void SDL_SetWindowTitle(SDL_Window *window, const char *title) {
    if (window == &the_window && the_window.open)
        alaya_window_open(1, 1, (const uint8_t *)title, strlen(title));
}

int SDL_SetWindowFullscreen(SDL_Window *window, Uint32 flags) {
    (void)window;
    if (flags == 0) return 0;
    error = "full screen is the person's choice: Alaya key + Up";
    return -1;
}

SDL_Surface *SDL_GetWindowSurface(SDL_Window *window) {
    if (window != &the_window || !the_window.open) return NULL;
    return &the_window.surface;
}

int SDL_UpdateWindowSurface(SDL_Window *window) {
    if (window != &the_window || !the_window.open) return -1;
    SDL_Surface *s = &the_window.surface;
    return alaya_window_present((const uint32_t *)s->pixels, s->w, s->h) == ALAYA_OK ? 0 : -1;
}

Uint32 SDL_MapRGB(const SDL_PixelFormat *format, Uint8 r, Uint8 g, Uint8 b) {
    (void)format;
    return (Uint32)r << 16 | (Uint32)g << 8 | b;
}

int SDL_FillRect(SDL_Surface *dst, const SDL_Rect *rect, Uint32 color) {
    if (!dst || !dst->pixels) return -1;
    int x0 = 0, y0 = 0, x1 = dst->w, y1 = dst->h;
    if (rect) {
        x0 = rect->x < 0 ? 0 : rect->x;
        y0 = rect->y < 0 ? 0 : rect->y;
        x1 = rect->x + rect->w > dst->w ? dst->w : rect->x + rect->w;
        y1 = rect->y + rect->h > dst->h ? dst->h : rect->y + rect->h;
    }
    for (int y = y0; y < y1; y++) {
        Uint32 *row = (Uint32 *)((Uint8 *)dst->pixels + (size_t)y * (size_t)dst->pitch);
        for (int x = x0; x < x1; x++) row[x] = color;
    }
    return 0;
}

static SDL_Keycode sdl_key(int32_t a) {
    switch (a) {
    case ALAYA_KEY_UP: return SDLK_UP;
    case ALAYA_KEY_DOWN: return SDLK_DOWN;
    case ALAYA_KEY_LEFT: return SDLK_LEFT;
    case ALAYA_KEY_RIGHT: return SDLK_RIGHT;
    case ALAYA_KEY_HOME: return SDLK_HOME;
    case ALAYA_KEY_END: return SDLK_END;
    default: return a;
    }
}

int SDL_PollEvent(SDL_Event *event) {
    uint32_t kind;
    int32_t a, b;
    while (alaya_window_event(&kind, &a, &b)) {
        SDL_Event e;
        memset(&e, 0, sizeof e);
        switch (kind) {
        case ALAYA_EVENT_QUIT: e.type = SDL_QUIT; break;
        case ALAYA_EVENT_KEY:
            e.type = SDL_KEYDOWN;
            e.key.state = 1;
            e.key.keysym.sym = sdl_key(a);
            e.key.keysym.mod = b ? KMOD_CTRL : KMOD_NONE;
            break;
        case ALAYA_EVENT_POINTER:
            e.type = SDL_MOUSEBUTTONDOWN;
            e.button.button = 1;
            e.button.x = a;
            e.button.y = b;
            break;
        case ALAYA_EVENT_RESIZED:
        case ALAYA_EVENT_FOCUS: e.type = SDL_WINDOWEVENT; break;
        default: continue;
        }
        e.key.timestamp = SDL_GetTicks();
        if (event) *event = e;
        return 1;
    }
    return 0;
}

int SDL_WaitEvent(SDL_Event *event) {
    while (!SDL_PollEvent(event)) alaya_window_wait();
    return 1;
}

Uint32 SDL_GetTicks(void) { return (Uint32)alaya_now_ms(); }

void SDL_Delay(Uint32 ms) {
    uint64_t until = alaya_now_ms() + ms;
    /* The program's clock ticks often enough for a game's frames. */
    while (alaya_now_ms() < until) alaya_window_wait();
}
