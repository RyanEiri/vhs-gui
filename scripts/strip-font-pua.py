#!/usr/bin/env python3
"""Remove the Private Use Area codepoints (U+E000..U+F8FF) from the bundled text fonts' character maps.

Inter and JetBrains Mono map some of their own glyphs there, and the Phosphor icon font uses the same
range. egui takes a glyph from the first font in a family that has it, so those text fonts were
drawing their own glyphs for 13 of the 30 icons (a "." for refresh, "≈" for upscale). Dropping the
mappings (the glyphs stay in the files, unreachable) lets Phosphor answer. Neither font declares a
Reserved Font Name, so the SIL OFL 1.1 allows the modified files; assets/fonts/README.md says so.

Run from the repo root after replacing a font file:  python3 scripts/strip-font-pua.py
Needs fontTools (pip install fonttools).
"""
import glob
import sys

from fontTools.ttLib import TTFont

LO, HI = 0xE000, 0xF8FF
changed = 0
for path in sorted(glob.glob("assets/fonts/*.ttf")):
    font = TTFont(path)
    removed = 0
    for table in font["cmap"].tables:
        for cp in [c for c in table.cmap if LO <= c <= HI]:
            del table.cmap[cp]
            removed += 1
    if removed:
        font.save(path)
        changed += 1
    print(f"{path}: {removed} mappings removed")
sys.exit(0)
