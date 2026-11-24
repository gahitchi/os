# Several screens (milestone 7b)

Alaya shows every screen the graphics card reports, side by side in one
picture. The strip at the top and the dock stay on the main screen (the
first one); the other screens show the wallpaper and windows.

- **Moving a window**: Alaya key + Shift + left or right sends the window
  in front to the screen before or after, at the same place on it.
  Alaya key + up fills the screen the window is on, and only that one.
- **Unplugging**: the windows of a screen that goes away come to the
  main screen, whole and below the strip. Each remembers where it was.
- **Plugging back in**: when the same screen (the same output of the
  card) comes back, its windows go back to it, as they were.

The screen driver asks the card which outputs have a screen, gives each
its part of one picture, and is told of changes by the card's display
event. In tests, a pretend screen can be plugged into an unused output
(QEMU's card has two outputs).

Check 103 plugs a 1024 by 768 screen in and out while apps run: the
editor moves to it, fills it, comes back whole when it is unplugged, and
returns to its place when it is plugged in again.

## How big things are (scale)

Each screen has its own scale. Alaya picks one from the screen's size
(100 % for most, 150 % for one wider than 2500 pixels, 200 % wider than
3200), or the person picks 100, 125, 150, 175 or 200 % in Settings, under
"Size on this screen", for the screen the Settings window is on (S steps
through the choices). The choices are kept with the person's app files
(`scales`), one per output of the card.

Apps draw in **points**. The toolkit's canvas turns points into pixels in
one place (at 150 %, 2 points are 3 pixels; letters are drawn at the
pixel size, so they stay sharp), and the window tells the app its size
and the pointer in points. So a window keeps its size in points: moved to
a screen with another scale, or when the scale changes, it grows or
shrinks in pixels and draws again.

Check 104 moves the editor and Settings to a second screen, sets 150 %
there with three presses of S, sees the editor grow by half, and sees it
come back to its own size on the main screen.

Not yet: Alaya's own bars (the strip, the dock, title bars) stay at
100 %; they follow in the next step. A window is at most 1280 by 800
pixels until pictures can be bigger.
