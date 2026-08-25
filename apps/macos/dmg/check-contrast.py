#!/usr/bin/env python3
"""Verify the disk image background stays readable under both system appearances.

Finder paints each icon's label straight onto the background — black under Light
appearance, white under Dark — and a disk image cannot override either choice. So the
pixels under the label text have to clear 4.5:1 against *both* colours, which pins them
to luminance 0.175..0.183. That is the whole reason the background has opaque cards.

The check renders the exact glyphs Finder will draw (same face, size and anchor as the
real label), dilates the mask by a pixel to include the antialias fringe, and reports the
worst single pixel under the text. A bounding box would drag in the surrounding field and
condemn art that is fine; an average would hide one dark texture speck sitting under one
letter -- which is exactly how an earlier draft of this background shipped a measured
4.60:1 that was really 3.42:1.

Usage: check-contrast.py background@2x.png [more.png ...]
"""
import sys

try:
    from PIL import Image, ImageDraw, ImageFont, ImageFilter
except ImportError:
    sys.exit("check-contrast.py needs Pillow: pip install pillow")

WINDOW_W = 680                      # must match layout.DS_Store's window bounds
ICON = 128                          # icon size set on the icon view options
SLOTS = ((176, 210, "ADI"), (504, 210, "Applications"))
LABEL_GAP = 5                       # Finder's gap between icon bottom and label top
MIN_RATIO = 4.5

FONTS = ("/System/Library/Fonts/SFNS.ttf", "/System/Library/Fonts/HelveticaNeue.ttc")


def _font(size):
    for path in FONTS:
        try:
            return ImageFont.truetype(path, size)
        except OSError:
            continue
    raise SystemExit("no system UI font found to render the label mask")


def _luminance(pixel):
    channels = [v / 255 for v in pixel[:3]]
    channels = [c / 12.92 if c <= .04045 else ((c + .055) / 1.055) ** 2.4 for c in channels]
    return .2126 * channels[0] + .7152 * channels[1] + .0722 * channels[2]


def check(path):
    image = Image.open(path).convert("RGB")
    if image.width % WINDOW_W:
        raise SystemExit(f"{path}: width {image.width} is not a whole multiple of {WINDOW_W}")
    scale = image.width // WINDOW_W

    mask = Image.new("L", image.size, 0)
    draw = ImageDraw.Draw(mask)
    font = _font(12 * scale)
    for cx, cy, name in SLOTS:
        draw.text((cx * scale, (cy + ICON // 2 + LABEL_GAP) * scale),
                  name, font=font, fill=255, anchor="ma")
    mask = mask.filter(ImageFilter.MaxFilter(3))

    pixels, mask_px = image.load(), mask.load()
    on_black = on_white = 99.0
    where = None
    for y in range(image.height):
        for x in range(image.width):
            if mask_px[x, y] < 64:
                continue
            lum = _luminance(pixels[x, y])
            black, white = (lum + .05) / .05, 1.05 / (lum + .05)
            if min(black, white) < min(on_black, on_white):
                where = (x // scale, y // scale)
            on_black, on_white = min(on_black, black), min(on_white, white)

    ok = min(on_black, on_white) >= MIN_RATIO
    print(f"{'ok  ' if ok else 'FAIL'} {path}  black {on_black:.2f}:1  white {on_white:.2f}:1"
          + ("" if ok else f"  worst pixel at {where}"))
    return ok


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__.strip().splitlines()[-1])
    sys.exit(0 if all([check(p) for p in sys.argv[1:]]) else 1)
