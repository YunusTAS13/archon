#!/usr/bin/env python3
"""Noto Sans Mono'dan 8x16 bitmap font uretir (ASCII 32..126).

Cikti: kernel/src/font.rs — satir basina 8 bit, bit 0 en soldaki piksel.
Framebuffer konsolu bu tabloyu cizerek metin basar.
"""

from PIL import Image, ImageDraw, ImageFont

FONT_PATH = "/usr/share/fonts/noto/NotoSansMono-Regular.ttf"
SIZE = 13
WIDTH, HEIGHT = 8, 16
FIRST, LAST = 32, 126

font = ImageFont.truetype(FONT_PATH, SIZE)
glyphs = []

for code in range(FIRST, LAST + 1):
    image = Image.new("L", (WIDTH, HEIGHT), 0)
    draw = ImageDraw.Draw(image)
    draw.text((0, 1), chr(code), font=font, fill=255)
    rows = []
    for y in range(HEIGHT):
        bits = 0
        for x in range(WIDTH):
            if image.getpixel((x, y)) >= 96:
                bits |= 1 << x
        rows.append(bits)
    glyphs.append(rows)

lines = [
    "// @generated tools/gen_font.py tarafindan uretildi; elle duzenlemeyin.",
    "//",
    "// 8x16 bitmap font, ASCII 32..126. Her satir bir bayt: bit 0 = en sol.",
    "",
    "pub const GLYPH_WIDTH: usize = 8;",
    "pub const GLYPH_HEIGHT: usize = 16;",
    "pub const FIRST_CHAR: u8 = 32;",
    "",
    "pub static FONT: [[u8; GLYPH_HEIGHT]; 95] = [",
]
for rows in glyphs:
    lines.append("    [" + ", ".join(f"0x{row:02x}" for row in rows) + "],")
lines.append("];")

open("kernel/src/font.rs", "w").write("\n".join(lines) + "\n")
print("kernel/src/font.rs yazildi:", len(glyphs), "glif")
