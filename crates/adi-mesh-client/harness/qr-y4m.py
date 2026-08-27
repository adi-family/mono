#!/usr/bin/env python3
"""Turn what `adi-mono mesh invite --qr` printed into a video of a machine showing it.

Chrome can be handed a `.y4m` file as its camera (`--use-file-for-fake-video-capture`), so this is
how the end-to-end test points a browser at a QR code without a phone and a laptop on a desk. The
frames are built from the **exact characters the CLI wrote** — the half-blocks are read back into a
module grid, the same way `adi-cli/src/qr.rs`'s own test does it — so what the client decodes is
what the command prints, and not a second rendering that could differ from it.

    adi-mono mesh invite --qr | qr-y4m.py > camera.y4m

The output frames carry a live pairing token in readable form. `.gitignore` excludes them.
"""

import sys

# The colours `qr::terminal` wraps every line in. A line without them is prose, not the code.
INK = "\x1b[30;107m"
RESET = "\x1b[0m"
HALVES = {" ": (0, 0), "▀": (1, 0), "▄": (0, 1), "█": (1, 1)}

WIDTH, HEIGHT = 1280, 720
FRAMES = 10
# The room, the screen, and the ink on it. Deliberately ordinary: how *badly* lit a screen can be
# and still decode is settled in `src/scan.rs`'s unit tests, which can try twenty of them a second.
ROOM, LIGHT, DARK = 40, 235, 25


def modules(text):
    """The QR's module grid, read back out of the printed characters."""
    rows = []
    for line in text.splitlines():
        if not line.startswith(INK):
            continue
        cells = line[len(INK):]
        if cells.endswith(RESET):
            cells = cells[: -len(RESET)]
        upper, lower = [], []
        for char in cells:
            if char not in HALVES:
                raise SystemExit(f"unexpected character {char!r} in the rendered QR")
            top, bottom = HALVES[char]
            upper.append(top)
            lower.append(bottom)
        rows.append(upper)
        rows.append(lower)
    if not rows:
        raise SystemExit("no QR code in that input — was `mesh invite` run with --qr?")
    return rows


def frame(grid):
    """One greyscale frame: a lit screen in a darker room, with the code on it."""
    across = len(grid[0])
    # Whole pixels a module, so no edge in the code lands mid-pixel — a fake camera has no optics
    # to blur one, and a half-pixel module edge is a decoding problem this test is not about.
    cell = (HEIGHT * 6 // 7) // across
    span = cell * across
    left, top = (WIDTH - span) // 2, (HEIGHT - span) // 2

    plane = bytearray([ROOM]) * (WIDTH * HEIGHT)
    for y in range(top, top + span):
        start = y * WIDTH + left
        plane[start : start + span] = bytes([LIGHT]) * span
    for gy, row in enumerate(grid):
        for gx, dark in enumerate(row):
            if not dark:
                continue
            for y in range(cell):
                start = (top + gy * cell + y) * WIDTH + left + gx * cell
                plane[start : start + cell] = bytes([DARK]) * cell
    return plane


def main():
    grid = modules(sys.stdin.read())
    plane = frame(grid)
    # Colourless: a QR is brightness, and both chroma planes at 128 is neutral grey.
    chroma = bytes([128]) * (WIDTH // 2 * (HEIGHT // 2))

    out = sys.stdout.buffer
    out.write(f"YUV4MPEG2 W{WIDTH} H{HEIGHT} F10:1 Ip A1:1 C420mpeg2\n".encode())
    # Chrome loops the file, so a second of identical frames is a camera that never looks away.
    for _ in range(FRAMES):
        out.write(b"FRAME\n")
        out.write(plane)
        out.write(chroma)
        out.write(chroma)


if __name__ == "__main__":
    main()
