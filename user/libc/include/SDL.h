/* SPDX-License-Identifier: GPL-3.0-only WITH AdditionRef-Alaya-system-library-exception */
/*
 * A small part of the SDL 2 interface, written for Alaya from scratch
 * (milestone 8, step 4): one window with a software surface, key events,
 * ticks and delays. Enough for simple games; names and key values follow
 * SDL 2 so that small games can be brought over with few changes. Drawing
 * is in software until Alaya has fast graphics (milestone 9).
 *
 * Full screen is the person's choice (Alaya key + Up), so
 * SDL_SetWindowFullscreen always says no.
 */
#ifndef ALAYA_SDL_H
#define ALAYA_SDL_H

#include <stddef.h>
#include <stdint.h>

typedef uint8_t Uint8;
typedef uint16_t Uint16;
typedef uint32_t Uint32;
typedef uint64_t Uint64;
typedef int32_t Sint32;
typedef int32_t SDL_Keycode;

#define SDL_INIT_TIMER 0x00000001u
#define SDL_INIT_VIDEO 0x00000020u
#define SDL_INIT_EVENTS 0x00004000u
#define SDL_WINDOWPOS_UNDEFINED 0x1FFF0000
#define SDL_WINDOWPOS_CENTERED 0x2FFF0000
#define SDL_WINDOW_SHOWN 0x00000004u
#define SDL_WINDOW_FULLSCREEN 0x00000001u

typedef struct SDL_Rect {
    int x, y, w, h;
} SDL_Rect;

typedef struct SDL_PixelFormat {
    Uint32 format;
    Uint8 BitsPerPixel;
    Uint8 BytesPerPixel;
} SDL_PixelFormat;

typedef struct SDL_Surface {
    Uint32 flags;
    SDL_PixelFormat *format;
    int w, h;
    int pitch;
    void *pixels;
} SDL_Surface;

typedef struct SDL_Window SDL_Window;

/* Event types. */
#define SDL_QUIT 0x100
#define SDL_WINDOWEVENT 0x200
#define SDL_KEYDOWN 0x300
#define SDL_TEXTINPUT 0x303
#define SDL_MOUSEBUTTONDOWN 0x401

/* Key values (SDL 2's). */
#define SDLK_RETURN '\r'
#define SDLK_ESCAPE '\033'
#define SDLK_BACKSPACE '\b'
#define SDLK_TAB '\t'
#define SDLK_SPACE ' '
#define SDLK_DELETE '\177'
#define SDLK_RIGHT 0x4000004F
#define SDLK_LEFT 0x40000050
#define SDLK_DOWN 0x40000051
#define SDLK_UP 0x40000052
#define SDLK_HOME 0x4000004A
#define SDLK_END 0x4000004D
#define KMOD_NONE 0x0000
#define KMOD_CTRL 0x00C0

typedef struct SDL_Keysym {
    int scancode;
    SDL_Keycode sym;
    Uint16 mod;
} SDL_Keysym;

typedef struct SDL_KeyboardEvent {
    Uint32 type;
    Uint32 timestamp;
    Uint32 windowID;
    Uint8 state;
    Uint8 repeat;
    SDL_Keysym keysym;
} SDL_KeyboardEvent;

typedef struct SDL_MouseButtonEvent {
    Uint32 type;
    Uint32 timestamp;
    Uint32 windowID;
    Uint8 button;
    int x, y;
} SDL_MouseButtonEvent;

typedef union SDL_Event {
    Uint32 type;
    SDL_KeyboardEvent key;
    SDL_MouseButtonEvent button;
    Uint8 padding[56];
} SDL_Event;

int SDL_Init(Uint32 flags);
void SDL_Quit(void);
const char *SDL_GetError(void);

SDL_Window *SDL_CreateWindow(const char *title, int x, int y, int w, int h, Uint32 flags);
void SDL_DestroyWindow(SDL_Window *window);
void SDL_SetWindowTitle(SDL_Window *window, const char *title);
int SDL_SetWindowFullscreen(SDL_Window *window, Uint32 flags);
SDL_Surface *SDL_GetWindowSurface(SDL_Window *window);
int SDL_UpdateWindowSurface(SDL_Window *window);

Uint32 SDL_MapRGB(const SDL_PixelFormat *format, Uint8 r, Uint8 g, Uint8 b);
int SDL_FillRect(SDL_Surface *dst, const SDL_Rect *rect, Uint32 color);

int SDL_PollEvent(SDL_Event *event);
int SDL_WaitEvent(SDL_Event *event);
Uint32 SDL_GetTicks(void);
void SDL_Delay(Uint32 ms);

#endif
