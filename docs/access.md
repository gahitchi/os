# Seeing and the keyboard (milestone 7b, step 2)

Everything in Alaya can be used without a mouse and without seeing the
screen well. All of it is in Settings, under "Seeing", and on keys that
work anywhere.

## High contrast (Alaya key + C)

Black ground, white text, yellow for what is chosen or has the keyboard,
bright colours for states (changed, needs a look, from the past). Frames
are drawn twice as thick. Only colours change, never what anything does.

It is done in one place: the toolkit's canvas turns every colour into
its high-contrast one as it draws (`canvas::high_contrast`). Alaya's own
colours each have a chosen partner; any other colour becomes black or
white by how light it is, or a bright colour of the same hue when it is
a strong one. So every app is in high contrast at once, without changing
a line of it. A host test checks that every pair of text and ground in
Alaya's colours is at least 7 to 1 (the strictest level of the usual
accessibility rules).

## The screen reader (Alaya key + Enter)

It says what has the keyboard:

- **The window in front**: its title, and how many of your files its app
  can see ("The text editor can see 3 of your files"; Alaya's own apps,
  like Settings, "can see all your files"). This is new: a person who
  cannot see the screen gets the same privacy picture as one who reads
  Settings.
- **The piece that has the keyboard**: its name, its role (button, list,
  text box) and its state (a list's chosen row, the start of a text).
- **Alaya's own parts** during the walk (below).

Apps hand the reader their toolkit's accessibility tree in their own
window pictures (after the pixels, `dk::TREE_AT`), so there is no new
channel and no app can read another's tree; the reader reads the window
in front when its tree changes. Until Alaya has sound (milestone 11) the
reader shows its words in a caption bar at the top of the main screen,
and in the log; the same words will go to sound and braille.

## The walk (Alaya key + Tab)

Alaya key + Tab goes through the strip's buttons (Undo, What happened?,
Search), the dock and the group labels, with a thick ring around where
it is. Tab or the arrows move on (Shift + Tab back), Enter presses, Escape
goes back to the window in front. Inside apps, Tab moves between buttons,
lists and text boxes, and a ring always shows which one has the keyboard.

## Checks

- Host tests: the contrast of every text and ground pair; the tree an app
  sends reads back the same and rubbish from an app ends it; what the
  reader says for a list and a text box.
- Check 105: Alaya key + Enter turns the reader on; it says Settings and
  that Settings sees everything, follows Tab to a button, says what the
  editor can see; Alaya key + C turns high contrast on and off.
- Check 106: Alaya key + Tab walks Undo, What happened?, Search, the
  dock; Shift + Tab goes back; Enter on What happened? brings it to the
  front; Escape goes back to the window.
