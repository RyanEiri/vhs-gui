# Bundled fonts

Static TrueType files embedded in the binary (egui cannot use variable fonts). Both are SIL Open Font
License 1.1; the licence texts are beside the fonts.

| Font | Files | Source |
|---|---|---|
| Inter 4.1 | Inter-Regular, Inter-Medium, Inter-SemiBold, Inter-Bold | `extras/ttf/` in https://github.com/rsms/inter/releases/tag/v4.1 |
| JetBrains Mono 2.304 | JetBrainsMono-Regular, JetBrainsMono-Bold | `fonts/ttf/` in https://github.com/JetBrains/JetBrainsMono/releases/tag/v2.304 |

Icons come from the `egui-phosphor` crate (Phosphor Icons, MIT).

## Modification

The text fonts have their Private Use Area codepoints (U+E000 to U+F8FF) removed from the character map
(`scripts/strip-font-pua.py`). Inter and JetBrains Mono put some of their own glyphs there, and Phosphor
uses the same range, so without this the text fonts answered for 13 of the 30 icons. The glyphs stay in
the files, unreachable. Neither OFL declares a Reserved Font Name, so the licence allows the modified
files. Run the script again after replacing a font file.
