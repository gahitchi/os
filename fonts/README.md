# Fonts

All fonts here are under the SIL Open Font License 1.1 (the `OFL-*.txt`
files).

- `Figtree-Regular.ttf`, `Figtree-SemiBold.ttf`: Alaya's text and strong
  faces.
- `IBMPlexMono-Regular.ttf`: the code face.
- `NotoSans-Regular.ttf`: the base of the fallback font (kept as the
  source `tools/fonts/build.py` starts from).
- `NotoSans-Alaya.ttf`: the fallback font Alaya loads (milestone 7b):
  Noto Sans with Noto Sans Arabic, Noto Sans Hebrew, part of Noto Sans JP
  (kana and about 1,300 common kanji), part of Noto Sans SC (about 1,800
  common hanzi) and part of Noto Emoji. Copyright The Noto Project
  Authors (https://github.com/notofonts); built from the `@fontsource`
  packages with `python3 tools/fonts/build.py <packages> fonts/NotoSans-Alaya.ttf`.
