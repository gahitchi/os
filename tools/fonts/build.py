#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Build Alaya's fallback font, fonts/NotoSans-Alaya.ttf (milestone 7b).

One TrueType font holds every script Alaya draws beyond its main faces:
Noto Sans (Latin, Greek, Cyrillic), Noto Sans Arabic (with the Arabic
presentation forms Alaya uses to join letters), Noto Sans Hebrew, the
kana and about 1,300 of the most used kanji of Noto Sans JP, about 1,800
of the most used hanzi of Noto Sans SC, and a few hundred Noto Emoji. All
are under the SIL Open Font License 1.1 (fonts/OFL-NotoSans.txt).

Inputs are the @fontsource packages (npm pack @fontsource/noto-sans-arabic
and so on), unpacked into one folder:

    python3 tools/fonts/build.py <unpacked packages> fonts/NotoSans-Alaya.ttf

Needs fontTools (pip install fonttools). Layout tables (GSUB, GPOS) are
left out: Alaya joins Arabic letters itself, through the presentation
forms, and places glyphs by their advances.
"""
import glob
import os
import sys

from fontTools import subset
from fontTools.merge import Merger
from fontTools.ttLib import TTFont
from fontTools.ttLib.scaleUpem import scale_upem

src, out = sys.argv[1], sys.argv[2]
here = os.path.dirname(os.path.abspath(__file__))
base = os.path.join(here, "..", "..", "fonts", "NotoSans-Regular.ttf")


def files(pkg, pattern):
    found = sorted(glob.glob(os.path.join(src, pkg, "package", "files", pattern)))
    if not found:
        sys.exit(f"no {pattern} in {pkg}")
    return found


def chunk_chars(pkg, family, numbers):
    """The characters of the most used slices (fontsource's numbered
    slices: the higher the number, the more used)."""
    chars = set()
    for n in numbers:
        for f in files(pkg, f"{family}-{n}-400-normal.woff"):
            chars |= set(TTFont(f).getBestCmap())
    return chars


def cut(path, chars, name):
    """`path` cut down to `chars`, as a plain TrueType file."""
    f = TTFont(path)
    f.flavor = None
    opts = subset.Options()
    opts.layout_features = []
    opts.drop_tables += ["GSUB", "GPOS", "GDEF", "BASE", "JSTF", "MATH", "STAT", "meta"]
    opts.drop_tables += ["vhea", "vmtx", "gasp", "prep", "fpgm", "cvt ", "hdmx", "VDMX", "LTSH"]
    opts.hinting = False
    opts.notdef_outline = True
    opts.name_IDs = ["*"]
    s = subset.Subsetter(opts)
    s.populate(unicodes=sorted(chars))
    s.subset(f)
    if f["head"].unitsPerEm != 1000:
        scale_upem(f, 1000)
    p = os.path.join(os.path.dirname(os.path.abspath(out)), f".{name}.ttf")
    f.save(p)
    return p


def all_chars(path):
    return set(TTFont(path).getBestCmap())


parts = []
latin = all_chars(base)
parts.append(cut(base, latin, "latin"))
arabic = files("fontsource-noto-sans-arabic-5.3.0", "noto-sans-arabic-arabic-400-normal.woff")[0]
parts.append(cut(arabic, all_chars(arabic) - latin, "arabic"))
hebrew = files("fontsource-noto-sans-hebrew-5.3.0", "noto-sans-hebrew-hebrew-400-normal.woff")[0]
parts.append(cut(hebrew, all_chars(hebrew) - latin, "hebrew"))
jp = files("fontsource-noto-sans-jp-5.3.1", "noto-sans-jp-japanese-400-normal.woff")[0]
jp_chars = chunk_chars("fontsource-noto-sans-jp-5.3.1", "noto-sans-jp", range(90, 120))
jp_chars |= set(range(0x3000, 0x3100)) | set(range(0xFF01, 0xFF5F))
jp_chars &= all_chars(jp)
parts.append(cut(jp, jp_chars - latin, "jp"))
sc = files("fontsource-noto-sans-sc-5.3.1", "noto-sans-sc-chinese-simplified-400-normal.woff")[0]
sc_chars = chunk_chars("fontsource-noto-sans-sc-5.3.1", "noto-sans-sc", range(110, 119))
sc_chars &= all_chars(sc)
parts.append(cut(sc, sc_chars - latin - jp_chars, "sc"))
emoji_src = {}
for f in files("fontsource-noto-emoji-5.3.2", "noto-emoji-*-400-normal.woff"):
    cm = TTFont(f).getBestCmap()
    # The faces, hands, hearts and everyday things the emoji picker shows.
    want = {c for c in cm if 0x1F600 <= c <= 0x1F64F or 0x1F44D <= c <= 0x1F44F
            or c in (0x2764, 0x1F389, 0x1F680, 0x1F4A1, 0x1F525, 0x2705, 0x1F31F, 0x1F381)}
    if want:
        emoji_src[f] = want
for i, (f, want) in enumerate(sorted(emoji_src.items())):
    parts.append(cut(f, want, f"emoji{i}"))

merged = Merger().merge(parts)
merged["name"].setName("Noto Sans Alaya", 1, 3, 1, 0x409)
merged["name"].setName("Noto Sans Alaya", 4, 3, 1, 0x409)
merged.save(out)
for p in parts:
    os.remove(p)
print(out, os.path.getsize(out), "bytes,", len(merged.getGlyphOrder()), "glyphs")
