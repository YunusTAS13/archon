#!/usr/bin/env python3
"""kernel/src/font.rs icindeki 8x16 fontu C basligi olarak yazar."""

import re

SOURCE = "kernel/src/font.rs"
TARGET = "tools/gui/font8x16.h"

text = open(SOURCE).read()
rows = re.findall(r"\[([0-9a-fx, ]+)\]", text)
glyphs = []
for row in rows:
    values = [int(value, 16) for value in re.findall(r"0x[0-9a-fA-F]+", row)]
    if len(values) == 16:
        glyphs.append(values)

assert len(glyphs) == 95, f"beklenen 95 glif, bulunan {len(glyphs)}"

lines = [
    "/* @generated tools/gui/gen_font_header.py tarafindan uretildi. */",
    "#ifndef ARCHON_FONT8X16_H",
    "#define ARCHON_FONT8X16_H",
    "",
    "/* ASCII 32..126, satir basina 8 bit; bit 0 en soldaki piksel. */",
    "static const unsigned char FONT8X16[95][16] = {",
]
for glyph in glyphs:
    values = ", ".join(f"0x{value:02x}" for value in glyph)
    lines.append(f"    {{{values}}},")
lines += ["};", "", "#endif", ""]

open(TARGET, "w").write("\n".join(lines))
print(f"{TARGET}: {len(glyphs)} glif")
