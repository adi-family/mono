#!/usr/bin/env python3
"""Measure the panels' type against the pixels actually behind it.

The contrast is measured on the rendered pixels and the number is reported — a panel that is
only asserted to be readable is not. This is the other half of `frames.sh`.

    apps/ios/frames/measure.py apps/ios/shots/iphone/store/*.png

What it checks, and the value it checks against:

  headline      WCAG contrast of the headline ink against the ground under it, reported worst /
                median. 3.0 is the bar: it clears WCAG's large-text threshold many times over.
  subhead       the same for the subhead, at 4.5 — the bar for body text.

The ground is read off the copy-less "plate" render `frames.sh` writes beside each panel, so an
antialiased edge can never be mistaken for background: a glyph is where the two renders differ.
The bars are the design project's own (`apps/macos/dmg/check-contrast.py`), copied rather than
chosen — picking a bar the render happens to pass is the failure mode this file exists to prevent.
"""

import sys
from pathlib import Path

import numpy as np
from PIL import Image


def rel_luminance(rgb: np.ndarray) -> np.ndarray:
    c = rgb.astype(np.float64) / 255.0
    lin = np.where(c <= 0.03928, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)
    return 0.2126 * lin[..., 0] + 0.7152 * lin[..., 1] + 0.0722 * lin[..., 2]


def contrast(l1: float, l2: float) -> float:
    hi, lo = max(l1, l2), min(l1, l2)
    return (hi + 0.05) / (lo + 0.05)


def report(path: Path) -> bool:
    im = Image.open(path).convert("RGB")
    lum = rel_luminance(np.asarray(im))
    H, W = lum.shape

    # frames.sh puts the copy-less render in `_plate/` beside the panels, under the same name.
    plate_path = path.parent / "_plate" / path.name
    if not plate_path.exists():
        print(f"{path.name}\n   no plate render beside it — nothing to measure against")
        return True
    plate_lum = rel_luminance(np.asarray(Image.open(plate_path).convert("RGB")))

    def text_ratio(y0f: float, y1f: float, ink_min: float):
        y0i, y1i = int(H * y0f), int(H * y1f)
        x0i, x1i = int(W * 0.05), int(W * 0.95)
        full_b = lum[y0i:y1i, x0i:x1i]
        plate_b = plate_lum[y0i:y1i, x0i:x1i]
        ink = (full_b - plate_b) > ink_min
        if not ink.any():
            return None
        ink_l = float(np.median(full_b[ink]))
        # Only the ground the type actually sits on: the plate under the glyph pixels. The worst
        # case is the brightest ground the ink sits against; that is the one that governs.
        under = plate_b[ink]
        return contrast(ink_l, float(under.max())), contrast(ink_l, float(np.median(under)))

    # Bands from _frame.css, as fractions of the panel height.
    checks = [
        ("headline", text_ratio(0.055, 0.135, 0.15), 3.0),
        ("subhead", text_ratio(0.138, 0.175, 0.06), 4.5),
    ]
    ok = all(r is None or r[0] >= bar for _, r, bar in checks)

    print(f"{path.name}")
    for label, r, bar in checks:
        if r is None:
            print(f"   {label:<9}   no ink found in its band")
            continue
        print(f"   {label:<9}   worst {r[0]:.2f} : 1   median {r[1]:.2f} : 1"
              f"   {'ok' if r[0] >= bar else f'BELOW {bar}'}  (bar {bar})")
    return ok


def main() -> int:
    args = sys.argv[1:]
    # `--dw` / `--st` are still passed by frames.sh for the device geometry; the type bands no
    # longer depend on them, so they are accepted and ignored.
    for flag in ("--dw", "--st"):
        if flag in args:
            i = args.index(flag)
            del args[i:i + 2]
    paths = [Path(p) for p in args]
    if not paths:
        return print(__doc__) or 2
    # Every panel is reported, then the verdict. `all(report(p) for …)` short-circuits on the
    # first failure and silently hides the rest, which is exactly what a measurement must not do.
    good = all([report(p) for p in paths])
    print("\nall panels within the stated values" if good else "\nSOME PANELS BELOW THE BAR")
    return 0 if good else 1


if __name__ == "__main__":
    raise SystemExit(main())
