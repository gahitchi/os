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

## For makers

`cargo xtask` builds the example app **Postcards** (`user/apps/src/bin/postcards.rs`), cuts its program down to what is loaded, signs it with the **example maker key** (public on purpose, in `xtask/src/bundles.rs`; for test apps only) and puts it in the boot image as `example-apps`. A real maker keeps their secret key secret.

## Checked in every boot test (checks 110 to 113)

- 110: a bundle with one byte changed after signing is refused; nothing is installed.
- 111: installed on the side with the files line unticked, then approved; the app's store handle really is absent.
- 112: one undo takes the install away; its window closes and it cannot start.
- 113: installed with files, it writes a postcard; removed, the receipt shows nothing left, the postcard stays.
