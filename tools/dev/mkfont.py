#!/usr/bin/env python3
"""SDL demolari icin 8x16 bitmap yazi tipi uretir (95 glif, ASCII 32..126)."""
import sys
from PIL import Image, ImageDraw, ImageFont

target = sys.argv[1] if len(sys.argv) > 1 else "build/rootdir/usr/share/font8x16.bmp"
font = ImageFont.truetype("/usr/share/fonts/TTF/DejaVuSansMono.ttf", 13)
cell_w, cell_h = 8, 16
image = Image.new("RGB", (95 * cell_w, cell_h), (255, 0, 255))
draw = ImageDraw.Draw(image)
for index in range(95):
    draw.text((index * cell_w, 1), chr(32 + index), font=font, fill=(255, 255, 255))
image.save(target)
print(f"font: {target} {image.size[0]}x{image.size[1]}")
