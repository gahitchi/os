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

Still to come in this step: a scale for each screen (100 %, 125 %, 150 %,
200 %), set in Settings.
