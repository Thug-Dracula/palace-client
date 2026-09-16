#!/usr/bin/env python3
"""Regenerate the `PALETTE` array in `src/palette.rs` from PalacePalette.as.

The palette is a 256-entry constant, and transcribing 256 hex values by hand is
exactly the kind of thing that silently goes wrong. This script is the generator of
record; `--check` verifies the committed file still matches, and is what a reviewer
should run rather than reading the numbers.

    tools/gen_palette.py ~/palace-corpus/reference/PalacePalette.as
    tools/gen_palette.py ~/palace-corpus/reference/PalacePalette.as --check src/palette.rs

Only `clutARGB` is exported. `imageClutARGB` (same file) is a *different* ordering
used only for hotspot images and deliberately not part of this crate.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ENTRIES_PER_LINE = 6


def extract_clut(path: str, name: str = "clutARGB") -> list[int]:
    text = Path(path).read_text(errors="replace")
    start = text.index(f"{name}:Array = [")
    chunk = text[start : text.index("];", start)]
    # Strip both comment styles before scanning for literals: the file keeps a
    # commented-out copy of an older table that would otherwise be picked up.
    chunk = re.sub(r"//[^\n]*", "", chunk)
    chunk = re.sub(r"/\*.*?\*/", "", chunk, flags=re.S)
    values = []
    for token in re.findall(r"0x[0-9a-fA-F]+|-?\d+", chunk):
        value = int(token, 16) if token.lower().startswith("0x") else int(token, 10)
        values.append(value & 0xFFFFFFFF)
    if len(values) < 256:
        raise SystemExit(f"{name}: found only {len(values)} entries, expected 256")
    return values[:256]


def render(values: list[int]) -> str:
    lines = []
    for i in range(0, 256, ENTRIES_PER_LINE):
        row = ", ".join(f"0x{v:08x}" for v in values[i : i + ENTRIES_PER_LINE])
        lines.append(f"    {row},")
    return "\n".join(lines)


def committed(path: str) -> list[int]:
    text = Path(path).read_text()
    start = text.index("pub static PALETTE: [u32; PALETTE_LEN] = [")
    chunk = text[start : text.index("];", start)]
    return [int(token, 16) for token in re.findall(r"0x[0-9a-fA-F]{8}", chunk)]


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    values = extract_clut(argv[1])
    if "--check" in argv:
        path = argv[argv.index("--check") + 1]
        have = committed(path)
        if have == values:
            print(f"{path}: matches PalacePalette.as ({len(values)} entries)")
            return 0
        for i, (a, b) in enumerate(zip(have, values)):
            if a != b:
                print(f"{path}: MISMATCH at index {i}: file 0x{a:08x} != source 0x{b:08x}")
        print(f"{path}: {len(have)} committed entries vs {len(values)} in source")
        return 1
    print(render(values))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
