#!/usr/bin/env python3
"""Byte-for-byte differential between two `.prp` files.

Reports the *first* divergent offset, the expected and actual byte there, where
that offset falls inside the `.prp` container (file header / data region / asset
map / names), and a short hex context window from each file. Also reports a pure
length mismatch when one file is a prefix of the other.

This is the localiser used to compare the Rust writer's output against a golden
fixture, and to compare a `.prp` against a deliberately corrupted copy.

Usage:
    python3 diff_prp.py A.prp B.prp [--context 16]

Exit status:
    0  the files are byte-identical
    1  the files differ (diagnostic printed)
    2  a file could not be read
"""
from __future__ import annotations

import argparse
import struct
import sys

ASSET_FILE_HEADER = 16
ASSET_MAP_HEADER = 24
TYPE_REC = 12
ASSET_REC = 32


def _u32(buf: bytes, at: int) -> int | None:
    if at + 4 > len(buf):
        return None
    return struct.unpack_from("<I", buf, at)[0]


def _fourcc(value: int) -> str:
    raw = value.to_bytes(4, "little", signed=False)
    if all(32 <= b < 127 for b in raw):
        return raw.decode("ascii")
    return f"0x{value:08x}"


def locate(buf: bytes, offset: int) -> str:
    """Describe where `offset` falls inside the container."""
    if len(buf) < ASSET_FILE_HEADER:
        return "short file (< 16 bytes; no complete header)"
    data_off, data_size, map_off, map_size = struct.unpack_from("<IIII", buf, 0)
    if offset < ASSET_FILE_HEADER:
        field = ["dataOffset", "dataSize", "assetMapOffset", "assetMapSize"][offset // 4]
        return f"AssetFileHeader field {field} (byte {offset - (offset // 4) * 4} of it)"
    if offset < map_off:
        return (f"data region (blob bytes); data-relative offset "
                f"0x{offset - data_off:x} (file {offset:#x})")
    if offset >= map_off + map_size:
        return f"past assetMapOffset+assetMapSize (trailing bytes) at {offset:#x}"
    # Inside the asset map: name the sub-region from its own header when valid.
    rel = offset - map_off
    n_types = _u32(buf, map_off)
    n_assets = _u32(buf, map_off + 4)
    len_names = _u32(buf, map_off + 8)
    types_off = _u32(buf, map_off + 12)
    recs_off = _u32(buf, map_off + 16)
    names_off = _u32(buf, map_off + 20)
    if None not in (n_types, n_assets, len_names, types_off, recs_off, names_off):
        if types_off <= rel < recs_off:
            i = (rel - types_off) // TYPE_REC
            tag = _u32(buf, map_off + types_off + i * TYPE_REC)
            return f"asset map / AssetTypeRec[{i}] type={_fourcc(tag) if tag is not None else '?'} (map-relative {rel:#x})"
        if recs_off <= rel < names_off:
            i = (rel - recs_off) // ASSET_REC
            fld = ["id", "rHandle", "dataOffset", "dataSize",
                   "lastUseTime", "nameOffset", "flags", "crc"][(rel - recs_off) % ASSET_REC // 4]
            return f"asset map / AssetRec[{i}].{fld} (map-relative {rel:#x})"
        if names_off <= rel < names_off + len_names:
            return f"asset map / names blob (map-relative {rel:#x})"
    return f"asset map (map-relative {rel:#x})"


def hex_window(buf: bytes, offset: int, width: int) -> list[str]:
    start = max(0, offset - width)
    chunk = buf[start:offset + width]
    lines = []
    for row in range(0, len(chunk), 16):
        addr = start + row
        cells = chunk[row:row + 16]
        hexpart = " ".join(f"{b:02x}" for b in cells)
        lines.append(f"0x{addr:08x}  {hexpart:<47}")
        if addr <= offset < addr + len(cells):
            col = 10 + (offset - addr) * 3
            lines.append(" " * col + f"^ offset {offset:#x}")
    return lines


def first_difference(a: bytes, b: bytes) -> int | None:
    n = min(len(a), len(b))
    for i in range(n):
        if a[i] != b[i]:
            return i
    if len(a) != len(b):
        return n
    return None


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description="Byte-for-byte differential of two .prp files.")
    ap.add_argument("a", help="first .prp (the expected / golden file)")
    ap.add_argument("b", help="second .prp (the actual / candidate file)")
    ap.add_argument("--context", type=int, default=16,
                    help="bytes of hex context to show around the divergence (default 16)")
    args = ap.parse_args(argv)

    try:
        with open(args.a, "rb") as fh:
            a = fh.read()
        with open(args.b, "rb") as fh:
            b = fh.read()
    except OSError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    off = first_difference(a, b)
    if off is None:
        print(f"IDENTICAL  {args.a}  ==  {args.b}  ({len(a)} bytes)")
        return 0

    # A pure length mismatch (one is a prefix of the other) has off == min length.
    if off == min(len(a), len(b)) and len(a) != len(b):
        print(f"LENGTH MISMATCH  {args.a} ({len(a)} bytes) vs {args.b} ({len(b)} bytes)")
        print(f"  first length difference at offset {off:#x} ({off}): "
              f"{args.a} {'ends' if len(a) < len(b) else 'continues'} here")
        print(f"  expected: {'EOF' if len(a) <= off else f'{a[off]:#04x}'}   "
              f"actual: {'EOF' if len(b) <= off else f'{b[off]:#04x}'}")
        print(f"  context: {locate(a, off)}")
        return 1

    expected = a[off]
    actual = b[off]
    print(f"DIVERGENCE at offset {off:#x} ({off} decimal)")
    print(f"  expected (A) = {expected:#04x}   actual (B) = {actual:#04x}")
    print(f"  location     = {locate(a, off)}")
    print(f"  lengths      = A {len(a)} bytes, B {len(b)} bytes")
    print("  --- A context ---")
    for line in hex_window(a, off, args.context):
        print("  " + line)
    print("  --- B context ---")
    for line in hex_window(b, off, args.context):
        print("  " + line)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
