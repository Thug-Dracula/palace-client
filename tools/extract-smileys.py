#!/usr/bin/env python3
"""Regenerate crates/palace-render/assets/smileys.png from the reference sheet.

The client draws a user's face from a sprite sheet indexed by the two fields the
Palace protocol already carries for every user: `face` (the column) and `color`
(the row). `defaultsmileys.png` in the OpenPalace reference is exactly that grid,
but its cells are 45px while the face art inside them is 42x42 at offset (1,1),
leaving a transparent border the renderer would have to compensate for (the
reference draws at -1,-1 for this reason).

This trims each cell to its content and repacks it, so a cell address becomes
simply (color * 42, face * 42) and the draw offset is (0, 0).

    python3 tools/extract-smileys.py [source.png] [output.png]
"""
import sys
from pathlib import Path

import numpy as np
from PIL import Image

SRC_DEFAULT = Path.home() / "colosseum/reference/repos/OpenPalace/PalaceClient/src/assets/faces/defaultsmileys.png"
OUT_DEFAULT = Path(__file__).resolve().parent.parent / "crates/palace-render/assets/smileys.png"

CELL = 45
OFFSET = 1
CONTENT = 42


def main() -> int:
    src = Path(sys.argv[1]) if len(sys.argv) > 1 else SRC_DEFAULT
    out = Path(sys.argv[2]) if len(sys.argv) > 2 else OUT_DEFAULT
    if not src.is_file():
        print(f"error: source sheet not found: {src}", file=sys.stderr)
        return 1

    sheet = np.array(Image.open(src).convert("RGBA"))
    cols = sheet.shape[1] // CELL
    rows = sheet.shape[0] // CELL
    # The reference sheet is 586px wide, one pixel past 13 whole cells, so a few
    # trailing pixels past the last cell are expected and ignored.
    if sheet.shape[1] - cols * CELL >= CELL or sheet.shape[0] - rows * CELL >= CELL:
        print(f"error: {sheet.shape[1]}x{sheet.shape[0]} is not a {CELL}px grid", file=sys.stderr)
        return 1
    if cols == 0 or rows == 0:
        print(f"error: {sheet.shape[1]}x{sheet.shape[0]} holds no {CELL}px cells", file=sys.stderr)
        return 1
    if sheet.shape[1] % CELL or sheet.shape[0] % CELL:
        print(f"note: ignoring {sheet.shape[1] % CELL}x{sheet.shape[0] % CELL}px trailing edge")

    packed = np.zeros((rows * CONTENT, cols * CONTENT, 4), np.uint8)
    for row in range(rows):
        for col in range(cols):
            y = row * CELL + OFFSET
            x = col * CELL + OFFSET
            packed[row * CONTENT:(row + 1) * CONTENT, col * CONTENT:(col + 1) * CONTENT] = sheet[
                y:y + CONTENT, x:x + CONTENT
            ]

    out.parent.mkdir(parents=True, exist_ok=True)
    Image.fromarray(packed).save(out)
    print(f"{out}: {cols} faces x {rows} colours, {CONTENT}px cells -> {packed.shape[1]}x{packed.shape[0]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
