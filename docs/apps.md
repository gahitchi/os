# Apps you install (milestone 8)

Up to milestone 7b, every app was built into Alaya. From milestone 8, an app can also come as a file, be installed by you, and get only what you allow.

## An app is one signed file

An app bundle (a file ending in `.alaya`) holds:

- the app's **name**, its **version** and its **maker's key**;
- its **promise**: the list of everything it may ever ask for (open and save files you hand it, keep running with no window, send you notifications, use the network, capture the screen);
- the **program** itself;
- the maker's **signature** over all of it (Ed25519, checked by Alaya's own code in `crypto/src/ed25519.rs`).

The format is in `bundle/src/lib.rs`. A bundle with any byte changed, a promise edited after signing, another maker's key, extra bytes or a promise line Alaya does not know is refused, and nothing changes. The same maker and name are the same app, so a later version is an update of it (milestone 8, step 2), never a stranger.

## Installing

1. In the file browser, choose the bundle and press **Enter**. Alaya's install sheet opens (an app can offer a bundle, but only the person can go on from here).
2. The sheet shows the app, its maker's code (four groups of letters, such as `K4TR-9QMX-2B7D-WNHE`, to compare with what the maker publishes), and its promise, **one line each**. **Space** ticks or unticks the chosen line.
3. **Enter** installs it **on the side**: the bundle is kept in the machine's own app files, where no app can reach it. It is not in the dock and cannot run.
4. **Approve** (Enter again) puts it in the dock. This goes on the undo list: **Alaya key + Z** takes it away again, closing its window.

## What an unticked line means

The first program starts installed apps. It reads the bundle again from the disk, checks the signature again, and gives the app **only** the handles for what its promise says and you left ticked. With "Open and save the files you hand it" unticked, the app gets no way to the store at all: the place for it in the app's table is empty, and no transfer pages are mapped. This is the kernel's capability rule, not a setting the app could ignore.

## Removing, with a receipt

Choose the installed app's bundle in the file browser again and press **Enter**: the sheet says it is installed. Press **Delete** there (on the sheet, not in the file browser). Alaya:

- ends its running copy and closes its windows;
- takes back every file you handed it, then **checks again on the store** that none is left;
- counts the files it made: they are yours and stay;
- takes it out of the dock;

and shows a **receipt** with each of these lines, ending with "Nothing of this app is left that could act or see your files" only when the store's own check says so (decision 147). The bundle is kept so one undo can bring the app back.

## Updates that cannot take more on their own (milestone 8, step 2)

A later version of an installed app is a bundle with the same maker and name and a higher version. Choose it in the file browser and press **Enter**:

- It is **taken at once**, never held back (decision 88). It keeps what you allowed before; a line its new promise drops is gone.
- **If it also wants something new**, the new line stays **off**. The first time the app tries to use it, Alaya asks: **Allow** or **Not now** (it then stays off). The update note lists what it newly wants.
- An **older** version is never installed this way; undo is the way back.

The version before is kept, so **Alaya key + Z** swaps back (and redo swaps again). A new version starts the next time you open it.

### The trial and the Experience Guardian

A new version is **on trial for its next three starts**:

- Alaya notes what it **tried** beyond what you allowed (for now: notifications with that line unticked). Trying a line that is new in its version is not counted: that is when Alaya asks you. The install sheet shows it as what it tried, **never as proof that it is safe** (R12).
- The **Experience Guardian** times each start, from Alaya starting the program to its window opening. Off trial, it learns how long the app usually takes. On trial, a start **clearly slower** than usual (more than twice the usual time, and a second more) takes the app back to the version before, by itself, with a note saying why; the window of the slow version closes. A version its maker marked a **security fix** (a signed flag in the bundle) is never taken back for being slower.

## A rehearsal on your own files (decision 148)

On the install sheet, press **R** before installing. Alaya makes a **side copy of your documents** and runs the app for real on it, with what you left ticked. The app gets the keyboard; the strip says "Rehearsing …", and **Alaya key + R** ends it. Alaya then shows:

- the files it **read**;
- the files it **would have changed**, made or removed;
- that the side copy was **thrown away**, checked on the store.

Nothing real is touched, and nothing is installed. **Enter** goes back to the install sheet. (For now it rehearses on your documents as they are now. Rehearsing on an older moment of your history comes later.)

## No window, no processor time

An installed app whose last window closes is **ended**, unless you left its line "Keep running with no window open" ticked (or allowed it when it first asked). Alaya's own apps are not affected.

## Game mode

While a **full-screen window is in front**, nothing in the background competes with it:

- installed apps running with no window are **frozen** where they are, and go on when you leave full screen;
- the store **puts off its backups by itself** (at most two hours);
- **install and update offers wait** until the game ends, so no sheet covers your game and nothing about your apps changes under it;
- notifications are held (only Alaya's "now" shows), as before.

Alaya decides this from what is on your screen, so an app cannot switch game mode on to slow the others down.

## Ending an app for real

When an app is removed, taken away by undo, sent back by the Experience Guardian or done rehearsing, closing its windows is not enough: an app could ignore that. The desktop asks the first program to **end the program itself** (a new "end" bit, next to pause and go on), which takes back everything it holds.

## Answers instead of data (milestone 8, step 3)

Often an app needs one fact about your file, not the file. Postcards wants to know what your postcards cost: the **total of column 2** of a list you keep. With **Ctrl+T** it asks Alaya for an **answer window**: you pick the document, and Postcards gets **one number**, never the document. It holds no grant to the file, so reading it is still refused.

- The questions come only from **Alaya's fixed list**: how many lines, how many words, the total of a column (in hundredths; a heading line is skipped). An app never sends its own code to run on your files (rule R8).
- Each answer grant may ask **at most 10 times a day**; then it gets "no more questions today" until tomorrow (R9). Asking again gives the same answer, so a number cannot be rebuilt by asking around it.
- **Every answer is in your history**: "What happened?" shows "Postcards asked for the total of column 2 of postcard-costs.txt and got one number: 4.90" (R9).
- An answer from a **private** document is private too: the app is marked as if it had read the file (decision 27, R10).

For now the store itself works out the answer, since it already reads the file safely; separate answer helpers come with more kinds of questions (a resized photo without its location, "is a controller connected").

## For makers

Ask for an answer with `Window::ask_answer(question)` (from `files::question`); the document picked comes back as `Event::Picked`, then `Store::answer(file)` gives the number and the questions left today.

`cargo xtask` builds the example app **Postcards** (`user/apps/src/bin/postcards.rs`), cuts its program down to what is loaded, signs it with the **example maker key** (public on purpose, in `xtask/src/bundles.rs`; for test apps only) and puts it in the boot image as `example-apps`, in five versions: 2 asks for nothing new, 3 also wants to keep running with no window, 4 is slow to start on purpose, 5 is slow too but marked a security fix. A real maker keeps their secret key secret.

A bundle's flags word (bytes 148 to 152) holds `SECURITY_FIX`; it is signed like everything else, and a flag Alaya does not know is refused.

## Checked in every boot test (checks 110 to 122)

- 110: a bundle with one byte changed after signing is refused; nothing is installed.
- 111: installed on the side with the files line unticked, then approved; the app's store handle really is absent.
- 112: one undo takes the install away; its window closes and it cannot start.
- 113: installed with files, it writes a postcard; removed, the receipt shows nothing left, the postcard stays.
- 114: an update asking for nothing new is taken at once and goes on trial; its refused notification is noted as what it tried; closing its last window ends it.
- 115: version 3, which also wants background time, is taken at once with that line off; when it first tries, Alaya asks; allowed, it keeps running with no window.
- 116: one undo (on its line in "What happened?") takes the update back, and the version swapped out ends.
- 117: a clearly slower version 4 goes back to version 2 by itself; a slower security fix (version 5) is kept.
- 118: a rehearsal reads and adds to the postcard on the side copy; Alaya shows what it read and changed, throws the copy away, and the real postcard is unchanged.
- 119: the installer under a fuzzer: 40 bundles, each changed at a random byte, are all refused.
- 120: installed, then updated and undone 50 times in a row; it still starts at the right version.
- 121: Postcards asks for the total of a column of a document picked in the answer window and gets 4.90, never the document; each answer is in the ledger, and after 10 the day's limit holds.
- 122: game mode: with the editor full screen, Postcards 3 running with no window is frozen and an update offered meanwhile waits; leaving full screen, it goes on and the update comes.
