# Typing in many languages (milestone 7b, step 3)

Apps never read the keyboard for text. Key presses for the window in
front go to the desktop's **input method** first (decision 50), which
turns them into text by the person's keyboard and way of typing, and
hands the window only the text (`event::TEXT`). Keys that are not text
(Enter, arrows, Backspace, Ctrl shortcuts) reach the window as keys, as
before. The logic is in `ui/src/input.rs` and is tested on the host; the
desktop part is `user/apps/src/bin/desktop/typing.rs`.

## Keyboards

Settings, Typing: English (US), English (UK), Deutsch, Français
(AZERTY), Italiano and Español, with AltGr (right Alt) for the third
symbol on a key (€, @, [ ...). Kept with the person's app files.

- **Dead keys**: the accent keys of each keyboard wait for the next key:
  ´ then e gives é, ^ then e gives ê. A dead key then Space gives the
  accent itself; a dead key then a letter it cannot go on gives both.
- **Compose** (the Compose or Menu key), then two keys: a e gives æ,
  s s gives ß, e = gives €, ' e gives é, < < gives «, 1 2 gives ½, and so on.

## Japanese and Chinese

Alaya key + Space switches between Latin letters, Japanese and Chinese.
What is being made shows in a small box at the bottom of the window in
front.

- **Japanese**: type romaji, see hiragana as you type (nihongo: にほんご;
  double consonants give a small tsu, n before a consonant gives ん).
  Enter or Space keeps it. (Kanji conversion comes later.)
- **Chinese**: type pinyin; Space shows the characters it can be, the
  most used first (words like nihao: 你好 come before single
  characters); Space again takes the first, a number takes that one,
  Enter keeps the letters as they are.

## Emoji and the on-screen keyboard

- Alaya key + . opens the emoji picker: arrows choose, Enter types it,
  Escape closes; a click picks one. The screen reader says each name.
- Alaya key + K shows a keyboard on the screen, for touch screens: it
  shows the person's layout, and its keys type through the same input
  method (so dead keys and Japanese work on it too).

## Right-to-left text and other scripts

Hebrew and Arabic lines are drawn in the order they are seen: a small
version of the Unicode bidirectional rules (strong letters set their
run's direction, numbers read left to right, spaces and punctuation take
the direction around them, brackets turn round in right-to-left runs).
Arabic letters are joined, with lam and alef as one sign, through the
Unicode presentation forms (`ui/src/script.rs`). Every app gets this
without changing: the toolkit's text drawing does it.

Alaya's fallback font, `fonts/NotoSans-Alaya.ttf`, holds Noto Sans with
Noto Sans Arabic, Hebrew, the kana and about 1,300 common kanji of Noto
Sans JP, about 1,800 common hanzi of Noto Sans SC, and Noto Emoji's faces
and everyday signs, all under the SIL Open Font License. It is made by
`tools/fonts/build.py` (see `fonts/README.md`).

## Checks

- Host tests: bidi on known lines (Hebrew in English, English in Hebrew,
  numbers, brackets), Arabic joining, the six layouts, dead keys,
  Compose, romaji to kana, pinyin with candidates.
- Check 107: a German keyboard gives z for the US y key, and é from the
  dead acute; Compose a e gives æ.
- Check 108: nihongo becomes にほんご; nihao becomes 你好.
- Check 109: the emoji picker types its second emoji; a click on the
  on-screen keyboard's Q types q.
