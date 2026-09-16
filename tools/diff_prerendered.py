#!/usr/bin/env python3
"""Compare decoded props against pre-existing PNG renders from an earlier session.

`~/palace-corpus/sprites_anim/` holds 44x44 RGBA PNGs named `<prop id>_unnamed.png`,
produced by a *different* pipeline before this crate existed. Where the matching
source blob can still be found in the loose prop dumps, that is a genuinely
independent end-to-end check: a different decoder, a different palette loader and a
different rendering path, so a mistake here would have to have been made twice in
the same way.

It is also the only check in the repository that speaks to *orientation*, because a
flipped decode would show up as a pixel mismatch against a render nobody produced
with this code.

    usage: diff_prerendered.py <PalacePalette.as> <render-dir> <prop-dir>...
"""

from __future__ import annotations

import sys
from pathlib import Path

from PIL import Image

import oracle_prop


def main(argv: list[str]) -> int:
    if len(argv) < 4:
        print(__doc__, file=sys.stderr)
        return 2
    palette = oracle_prop.load_palette(argv[1])
    render_dir = Path(argv[2])
    search = [Path(p) for p in argv[3:]]

    sources: dict[str, Path] = {}
    for directory in search:
        for path in directory.glob("*_unnamed.bin"):
            prop_id = path.stem.rsplit("_", 1)[0]
            sources.setdefault(prop_id, path)

    matched = 0
    mismatched = 0
    missing_source = 0
    failed = 0
    for png in sorted(render_dir.glob("*.png")):
        prop_id = png.stem.rsplit("_", 1)[0]
        source = sources.get(prop_id)
        if source is None:
            missing_source += 1
            continue
        try:
            width, height, rgba, fmt = oracle_prop.decode_openpalace(
                source.read_bytes(), palette
            )
        except Exception as error:  # noqa: BLE001
            print(f"{png.name}: decode failed: {error}")
            failed += 1
            continue
        reference = Image.open(png).convert("RGBA")
        if reference.size != (width, height) or reference.tobytes() != rgba:
            at = next(
                (
                    i
                    for i, (a, b) in enumerate(zip(reference.tobytes(), rgba))
                    if a != b
                ),
                None,
            )
            print(
                f"{png.name}: MISMATCH size={reference.size} expected={width}x{height} "
                f"first differing byte={at}"
            )
            mismatched += 1
        else:
            matched += 1

    print(
        f"pre-rendered comparison: matched={matched} mismatched={mismatched} "
        f"no-source-blob={missing_source} decode-failed={failed}"
    )
    return 1 if mismatched or failed else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
