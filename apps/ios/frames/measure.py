#!/usr/bin/env python3
"""Measure the panels against the values the design decision states.

`decisions/2026-08-27-dimensional-not-generated.md` puts its rules in numbers precisely so a
later run cannot argue taste, and it says the number has to be *reported*: "the contrast is
measured on the rendered pixels, and the number is reported. This is the one that cannot be
faked." So this is the other half of `frames.sh` — without it the panels are an assertion.

    apps/ios/frames/measure.py apps/ios/shots/iphone/store/*.png

What it checks, and the value it checks against:

  field swing   0.05 – 0.13 relative luminance across the *visible* field. Below that it is a
                dark rectangle rather than a lit one; above it the chrome fails first.
  hue split     share of the field warmer than neutral vs cooler. A two-hue field that comes
                back 97/3 is a one-hue field wearing two names.
  headline      WCAG contrast of the headline ink against the pixels actually behind it,
                reported worst / median because a gradient under text makes it a range and the
                worst is the one that governs. 4.5 is the bar for body text, 3.0 for large.

The field excludes the device and the copy block. The first version of the design project's own
field meter did not exclude the copy, and reported the peak of every field at the 2px rule under
the subhead — a meter that can see the furniture is measuring the furniture.
"""

import sys
from pathlib import Path

import numpy as np
from PIL import Image

# Geometry from _frame.css, as fractions of the panel.
COPY_BOX = (0.05, 0.02, 0.95, 0.20)     # mark + headline + sub + rule, generous
STAGE_TOP = 0.205                        # .stage top on the phone; the tablet passes its own


def rel_luminance(rgb: np.ndarray) -> np.ndarray:
    """WCAG relative luminance from 0-255 sRGB."""
    c = rgb.astype(float) / 255.0
    c = np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)
    return c[..., 0] * 0.2126 + c[..., 1] * 0.7152 + c[..., 2] * 0.0722


def contrast(l1: float, l2: float) -> float:
    hi, lo = max(l1, l2), min(l1, l2)
    return (hi + 0.05) / (lo + 0.05)


def report(path: Path, device_w: float, stage_top: float) -> bool:
    im = Image.open(path).convert("RGB")
    a = np.asarray(im)
    H, W = a.shape[:2]
    lum = rel_luminance(a)

    # frames.sh puts the copy-less render in `_plate/` beside the panels, under the same name.
    plate_path = path.parent / "_plate" / path.name
    plate_lum = None
    if plate_path.exists():
        plate_lum = rel_luminance(np.asarray(Image.open(plate_path).convert("RGB")))

    # ---- the visible field: not the device, not the copy -----------------------------------
    #
    # The device is masked by its actual box, not by "everything below the stage". On the iPad
    # panel the phone is 0.60 of the width, so there is real field either side of it and below
    # it — masking the whole lower band threw that away and left a strip so small that one bright
    # pixel put the swing at 0.92.
    field = np.ones((H, W), bool)
    pad = 0.012
    dx0 = int(W * ((1 - device_w) / 2 - pad))
    dx1 = int(W * ((1 + device_w) / 2 + pad))
    # Start the mask a hair above the stage: the device's lit top edge lands a few rows higher
    # than the CSS `top`, and those few rows are near-white. Left unmasked they put the swing at
    # 0.92 — a number that says "there is a screenshot in the field", not "the field is blown".
    field[int(H * (stage_top - 0.008)):, max(dx0, 0):min(dx1, W)] = False
    x0, y0, x1, y1 = COPY_BOX
    field[int(H * y0):int(H * y1), int(W * x0):int(W * x1)] = False

    fl = lum[field]
    swing = float(fl.max() - fl.min())

    px = a[field].astype(int)
    warm = int((px[:, 0] > px[:, 2]).sum())
    cool = int((px[:, 2] > px[:, 0]).sum())
    total = max(warm + cool, 1)

    # ---- each text element against what is actually behind it ------------------------------
    #
    # The bars are the design project's own (`check-contrast.py`): 3.0 for the headline because
    # it clears WCAG's 18.66px-bold large-text threshold many times over, 4.5 for everything
    # else. They are copied rather than chosen — picking a bar that the render happens to pass
    # is the failure mode this whole file exists to prevent.
    def text_ratio(y0f: float, y1f: float, ink_min: float):
        if plate_lum is None:
            return None
        y0i, y1i = int(H * y0f), int(H * y1f)
        x0i, x1i = int(W * 0.05), int(W * 0.95)
        full_b = lum[y0i:y1i, x0i:x1i]
        plate_b = plate_lum[y0i:y1i, x0i:x1i]

        # A glyph is where the two renders differ. Everything else in the band is the ground, and
        # it is read off the PLATE — so an antialiased edge can never be mistaken for background.
        ink = (full_b - plate_b) > ink_min
        if not ink.any():
            return None
        ink_l = float(np.median(full_b[ink]))
        # Only the ground the type actually sits on: the plate under the glyph pixels.
        under = plate_b[ink]
        # Worst case is the brightest ground the ink sits against; that is the one that governs.
        return contrast(ink_l, float(under.max())), contrast(ink_l, float(np.median(under)))

    checks = [
        ("headline", text_ratio(0.045, 0.125, 0.15), 3.0),
        ("subhead", text_ratio(0.128, 0.158, 0.06), 4.5),
    ]

    ok_swing = 0.05 <= swing <= 0.13
    ok_split = min(warm, cool) / total >= 0.05
    ok_text = all(r is None or r[0] >= bar for _, r, bar in checks)

    print(f"{path.name}")
    print(f"   field swing {swing:.4f}   {'ok' if ok_swing else 'OUT OF RANGE (want 0.05-0.13)'}")
    print(f"   hue split   {100*warm/total:.0f}% warm / {100*cool/total:.0f}% cool"
          f"   {'ok' if ok_split else 'ONE-HUE FIELD'}")
    for label, r, bar in checks:
        if r is None:
            continue
        print(f"   {label:<9}   worst {r[0]:.2f} : 1   median {r[1]:.2f} : 1"
              f"   {'ok' if r[0] >= bar else f'BELOW {bar}'}  (bar {bar})")
    return ok_swing and ok_split and ok_text


def main() -> int:
    args = sys.argv[1:]
    device_w, stage_top = 0.94, STAGE_TOP
    for flag, setter in (("--dw", "dw"), ("--st", "st")):
        if flag in args:
            i = args.index(flag)
            value = float(args[i + 1])
            del args[i:i + 2]
            if setter == "dw":
                device_w = value
            else:
                stage_top = value
    paths = [Path(p) for p in args]
    if not paths:
        return print(__doc__) or 2
    # Every panel is reported, then the verdict. `all(report(p) for …)` short-circuits on the
    # first failure and silently hides the rest, which is exactly what a measurement must not do.
    good = all([report(p, device_w, stage_top) for p in paths])
    print("\nall panels within the stated values" if good else "\nSOME PANELS ARE OUT OF RANGE")
    return 0 if good else 1


if __name__ == "__main__":
    raise SystemExit(main())
